# Deploying gambas on Fly.io

The public cluster is three machines in `dfw`, one Raft node each, behind Fly's
anycast proxy. nginx is not involved — it stays in `docker-compose.yml` for local
runs only.

```
 internet ──TLS──►  Fly anycast proxy        (replaces nginx: TLS + load balancing)
                      │
                      ├── gambas-1  [vol wal → /data]
                      ├── gambas-2  [vol wal → /data]
                      └── gambas-3  [vol wal → /data]
                            └─ 6PN private IPv6: raft on 7001, leader-forward on 8080
```

One region, not three. Raft ticks every 10ms and a commit costs one round trip,
so a single region commits in ~2ms with the default 100ms election timeout.
Spreading across continents would mean raising that timeout roughly 15x, trading
real write latency for a geography story this workload does not need.

## Bootstrap (once)

```bash
fly auth login
fly deploy --build-only --push --image-label "sha-$(git rev-parse --short=12 HEAD)"
./deploy/fly-bootstrap.sh registry.fly.io/gambas:sha-<tag>
```

`fly-bootstrap.sh` is commented with the reasoning behind each step. The three
that are easy to get wrong:

**Membership is static.** raft-kv builds it from `raft::Config::new(id, peers)`
on every boot; nothing is persisted. A node started without `--peer` is a
one-node cluster that elects itself and commits with a quorum of one. Booting all
three that way produces a three-way split brain, so the bootstrap starts them
with the entrypoint replaced by `sleep` and only gives them the real command once
every machine id is known.

**Bind to `[::]`, never `0.0.0.0`.** The 6PN is IPv6-only and the Fly proxy
reaches the machine over it, so a v4-only bind accepts nothing at all.

**Start the three together.** Configure with `--skip-start`, then start them at
once. Letting each machine start as it is updated leaves the first one
campaigning alone every ~100ms while the others sleep; the cluster settles
correctly but arrives at a term in the hundreds.

## Rolling deploys

`deploy/fly-deploy.sh` builds one image, pushes it, and updates the machines one
at a time. A 3-node cluster tolerates exactly one node down, so the canvas stays
live. Per-machine args are preserved across an image-only update, so the script
never re-specifies them.

CI runs it on green `main` once the repo variable `DEPLOY_ENABLED` is `true` and
the secret `FLY_API_TOKEN` exists (`fly tokens create deploy -a gambas`).

## Smoke test

```bash
curl https://gambas.fly.dev/api/status    # repeat: anycast balances; all must agree on leader_id
curl https://gambas.fly.dev/metrics | grep raft_kv_current_term   # single digits on a fresh cluster

curl -X POST https://gambas.fly.dev/api/pixel \
  -H 'content-type: application/json' -d '{"x":10,"y":10,"color":5}'
# raft_kv_commit_index advances by one on every node

fly machine stop <leader-machine-id>   # re-election is sub-second, writes keep working at 2/3
fly machine start <same-id>            # rejoins and catches up
```

Security checks that must all FAIL to bypass anything:

```bash
curl -H 'x-gambas-internal: 1' ...      # presence alone is not enough; the value must match the secret
curl -H 'X-Forwarded-For: 1.2.3.4' ...  # nodes read fly-client-ip, not XFF
```

## Gotchas that cost time

- **Fly has no `qro` region.** `dfw` is the closest to MX. Check with
  `fly platform regions`.
- **Allocate the public IPs.** Without `fly ips allocate-v4 --shared` and
  `fly ips allocate-v6`, `gambas.fly.dev` resolves to nothing while every machine
  looks perfectly healthy.
- **`fly machine update` takes no trailing args after `--`** (it errors with
  "accepts between 0 and 1 arg(s)"). The command goes into `--command` as a
  single string, which flyctl splits on spaces.
- **Peer DNS works by machine id only**: `<machine-id>.vm.gambas.internal`.
  Addressing by machine *name* does not resolve.
- **A local Tailscale interface breaks flyctl.** Its global IPv6 address makes
  glibc prefer IPv6 while there is no default IPv6 route, so API calls fail
  intermittently. Fix with `precedence ::ffff:0:0/96  100` in `/etc/gai.conf`.

## Custom domain

`gambas.cacdu.dev` points at the app with A/AAAA records (Cloudflare, DNS-only —
the orange cloud would put another proxy in front of Fly's).

Getting the certificate issued needs one extra record, and the reason is worth
knowing. Fly's proxy normally intercepts `/.well-known/acme-challenge/` so
Let's Encrypt can validate over HTTP-01. It does not do that here: the machines
were configured by hand with `--port 80:8080/tcp:http`, which sends the whole of
port 80 straight to the app, and the app answers every unknown path with
`index.html`. Let's Encrypt asked for a token and got 3237 bytes of HTML, so
HTTP-01 could never succeed.

The fix is to validate over DNS instead, which bypasses the app entirely:

```
CNAME _acme-challenge.gambas.cacdu.dev → gambas.cacdu.dev.<id>.flydns.net.
```

That delegates just the validation record to Fly's nameservers so Fly can publish
the token itself. Get the exact target from `fly certs setup <hostname>`.

If a certificate was already added before the CNAME existed, it can sit stuck —
`fly certs list` reporting `Issued` while `fly certs check` reports
`Not verified`, with no token published at the flydns target. Remove it and add
it again to force a fresh ACME order.

Note that port 80 currently serves plain HTTP without redirecting. `force_https`
in fly.toml only applies to machines created by `fly deploy`; machines built with
explicit `--port` flags do not inherit it.

## Teardown

```bash
fly machine list && fly volumes list
fly apps destroy gambas     # machines, volumes and IPs
```
