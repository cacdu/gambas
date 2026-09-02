#!/usr/bin/env bash
#
# One-time bootstrap of the gambas raft cluster on Fly.io.
#
# Why this is a script and not `fly deploy`: every node needs its OWN command
# (--id, --peer, --app-peer), and `fly deploy` gives all machines the identical
# one. So machines are created and configured individually.
#
# The ordering below is not arbitrary — each step exists to avoid a specific
# failure that this bootstrap hit for real:
#
#   * Membership in raft-kv is STATIC: `raft::Config::new(id, peers)` reads it
#     from the flags on every boot; nothing is persisted. A node started without
#     --peer is therefore a one-node cluster that elects ITSELF and can commit
#     with a quorum of one. If all three booted that way we would start from a
#     three-way split brain. So phase 1 boots them with the entrypoint replaced
#     by `sleep`: no raft, no WAL, just an ID and a DNS name.
#
#   * Peers are addressed as <machine-id>.vm.<app>.internal. Machine ids only
#     exist after creation, which is what forces the two phases. Addressing by
#     machine NAME does not resolve — only by id (verified on flyctl 0.4.91).
#
#   * Phase 2 uses `--skip-start` on all three, then starts them together. If
#     you let each machine start as it is updated, the first one campaigns alone
#     every ~100ms while the others are still asleep, and the cluster reaches a
#     term in the hundreds before it settles. Correct, but it looks broken.
#
#   * `fly machine update` does NOT accept trailing args after `--`
#     ("accepts between 0 and 1 arg(s)"). The command goes in --command as one
#     string, which flyctl splits on spaces into argv.
#
# Requires: fly (flyctl >= 0.4.91), jq, an authenticated session.

set -euo pipefail

APP="${FLY_APP:-gambas}"
REGION="${FLY_REGION:-dfw}"          # Fly has no qro; dfw is the closest to MX
IMAGE="${1:-}"

if [ -z "$IMAGE" ]; then
  echo "usage: $0 registry.fly.io/${APP}:<tag>" >&2
  echo "  build one with: fly deploy --build-only --push --image-label sha-\$(git rev-parse --short=12 HEAD)" >&2
  exit 1
fi

common_args=(
  --app "$APP" --region "$REGION"
  --vm-size shared-cpu-1x --vm-memory 256
  --port 80:8080/tcp:http --port 443:8080/tcp:http:tls
)

echo "▶ 1/6  app and secret"
fly apps create "$APP" 2>/dev/null || echo "  (app already exists)"
if ! fly secrets list --app "$APP" 2>/dev/null | grep -q GAMBAS_INTERNAL_SECRET; then
  fly secrets set GAMBAS_INTERNAL_SECRET="$(openssl rand -hex 32)" --app "$APP" --stage
else
  echo "  (secret already set)"
fi

echo "▶ 2/6  three 1GB volumes in ${REGION}"
while [ "$(fly volumes list --app "$APP" --json | jq '[.[] | select(.name=="wal")] | length')" -lt 3 ]; do
  fly volumes create wal --app "$APP" --region "$REGION" --size 1 -y >/dev/null
done
mapfile -t VOLS < <(fly volumes list --app "$APP" --json | jq -r '.[] | select(.name=="wal" and .attached_machine_id==null) | .id')

echo "▶ 3/6  three machines, raft disabled (entrypoint replaced by sleep)"
for i in 1 2 3; do
  fly machine run "$IMAGE" infinity "${common_args[@]}" \
    --name "${APP}-${i}" --volume "${VOLS[$((i-1))]}:/data" \
    --restart no --entrypoint /bin/sleep >/dev/null
  echo "  ${APP}-${i} created"
done

# Machine ids, ordered by name, so node N is always ${APP}-N.
mapfile -t IDS < <(fly machine list --app "$APP" --json | jq -r 'sort_by(.name) | .[].id')
echo "  ids: ${IDS[*]}"

echo "▶ 4/6  checking internal DNS"
fly ssh console --app "$APP" --machine "${IDS[0]}" \
  -C "getent hosts ${IDS[1]}.vm.${APP}.internal" >/dev/null \
  || { echo "✖ <machine-id>.vm.${APP}.internal does not resolve; fall back to the 6PN IPs from 'fly machine list'" >&2; exit 1; }
echo "  resolves"

echo "▶ 5/6  real raft command on each machine (still stopped)"
for i in 1 2 3; do
  peers=""
  for j in 1 2 3; do
    [ "$i" = "$j" ] && continue
    host="${IDS[$((j-1))]}.vm.${APP}.internal"
    peers+=" --peer ${j}=${host}:7001 --app-peer ${j}=${host}:8080"
  done
  # [::] and not 0.0.0.0: the 6PN is IPv6-only, and the Fly proxy reaches the
  # machine over it — a v4-only bind accepts nothing.
  fly machine update "${IDS[$((i-1))]}" --app "$APP" --yes --skip-start \
    --entrypoint gambas --restart always \
    --command "--id ${i} --raft-addr [::]:7001 --http-addr [::]:8080${peers} --data-dir /data --behind-proxy --trusted-ip-header fly-client-ip" >/dev/null
  echo "  ${APP}-${i} configured"
done

echo "▶ 6/6  starting all three together, then allocating public IPs"
for id in "${IDS[@]}"; do fly machine start "$id" --app "$APP" >/dev/null & done
wait
# Without an allocated IP the app has no public route at all and <app>.fly.dev
# resolves to nothing — easy to miss, since every machine looks healthy.
fly ips list --app "$APP" 2>/dev/null | grep -q "v4" || fly ips allocate-v4 --shared --app "$APP"
fly ips list --app "$APP" 2>/dev/null | grep -q "v6" || fly ips allocate-v6 --app "$APP"

echo "✔ bootstrap complete — check with:"
echo "    curl https://${APP}.fly.dev/api/status   # all nodes must agree on leader_id"
echo "    curl https://${APP}.fly.dev/metrics | grep raft_kv_current_term   # single digits on a fresh cluster"
