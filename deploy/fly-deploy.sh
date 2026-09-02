#!/usr/bin/env bash
#
# Rolling deploy for the gambas cluster on Fly.io.
#
# Builds the image once, pushes it to Fly's registry, then updates the three
# machines one at a time. `fly machine update` blocks until each machine is back
# up, so the cluster never loses more than one node at once — a 3-node raft
# cluster tolerates exactly one node down, so the canvas stays live throughout.
#
# The per-machine args (--id, --peer, --app-peer, timers, --trusted-ip-header)
# are set at bootstrap and preserved across an image-only update, so we never
# re-specify them here. The internal secret is injected as GAMBAS_INTERNAL_SECRET
# via `fly secrets`, not passed as an arg.
#
# Requires: FLY_API_TOKEN in the environment, `fly` (flyctl) and `jq` on PATH.
#
# Verified against flyctl 0.4.91: `fly deploy --build-only --push --image-label`
# and `fly machine update --image --yes` all exist and behave as used here.
# Note that `fly machine update` takes NO trailing args after `--`; anything that
# needs to change a node's command must go through --command as one string.

set -euo pipefail

APP="${FLY_APP:-gambas}"
LABEL="sha-$(git rev-parse --short=12 HEAD)"
IMAGE="registry.fly.io/${APP}:${LABEL}"

echo "▶ building and pushing ${IMAGE}"
# --build-only --push builds remotely and pushes to the Fly registry without
# touching the running machines; --image-label pins a deterministic tag.
fly deploy --app "$APP" --build-only --push --image-label "$LABEL"

echo "▶ rolling ${IMAGE} over each machine"
machine_ids="$(fly machine list --app "$APP" --json | jq -r '.[].id')"
if [ -z "$machine_ids" ]; then
  echo "✖ no machines found for app ${APP} — run the bootstrap first" >&2
  exit 1
fi

for id in $machine_ids; do
  echo "  ↻ updating machine ${id} (waits for it to come back before the next)"
  fly machine update "$id" --app "$APP" --image "$IMAGE" --yes
done

echo "✔ deploy complete: ${IMAGE}"
