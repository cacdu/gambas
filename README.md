# gambas

**A distributed pixel canvas.** *(gamba: a shrimp — the only crustacean that rhymes with canvas.)*

An r/place-style shared canvas where **every web replica is a full Raft node**. There is no database behind this app: pixels are keys in [raft-kv](https://github.com/cacdu/raft-kv), a from-scratch Raft implementation embedded directly in each server process — the same model as rqlite, dqlite, or embedded etcd.

```
paint a pixel on any node → Raft consensus → applied on every replica → pushed to every browser
```

<!-- TODO: demo GIF — two browsers on different nodes painting live -->

## Why this exists

I built [raft-kv](https://github.com/cacdu/raft-kv) to understand consensus from first principles: leader election, log replication, snapshots, membership changes, linearizable reads. **gambas is the proof that it works** — a real-time, multi-user application where the consensus layer is doing the actual work:

- **Replicated writes**: each painted pixel is a command in the Raft log, committed by quorum before the client gets a 200.
- **Change feed**: browsers get live updates through `RaftKv::subscribe()` — a watch API (à la etcd) hanging off the Raft apply loop. Every node delivers the same event sequence, so WebSocket fan-out works from *any* replica.
- **Fault tolerance**: kill the leader mid-painting; the cluster re-elects in under a second and nobody's canvas breaks.
- **Durability**: every pixel survives a full cluster restart via WAL replay.

## Architecture

```
                        ┌───────────┐
        browsers        │  nginx LB │        browsers
           │            └─────┬─────┘            │
           │ WS + HTTP        │                  │
     ┌─────▼──────┐    ┌──────▼─────┐     ┌──────▼─────┐
     │  gambas 1  │    │  gambas 2  │     │  gambas 3  │
     │ ┌────────┐ │    │ ┌────────┐ │     │ ┌────────┐ │
     │ │  axum  │ │    │ │  axum  │ │     │ │  axum  │ │
     │ │  + WS  │ │    │ │  + WS  │ │     │ │  + WS  │ │
     │ ├────────┤ │    │ ├────────┤ │     │ ├────────┤ │
     │ │raft-kv │◄┼────┼►│raft-kv │◄┼─────┼►│raft-kv │ │
     │ │ (lib)  │ │gRPC│ │(leader)│ │ gRPC│ │ (lib)  │ │
     │ ├────────┤ │    │ ├────────┤ │     │ ├────────┤ │
     │ │  WAL   │ │    │ │  WAL   │ │     │ │  WAL   │ │
     │ └────────┘ │    │ └────────┘ │     │ └────────┘ │
     └────────────┘    └────────────┘     └────────────┘
```

**Paint path** — `POST /api/pixel {x, y, color}` to *any* node:
1. Edge node validates and charges the per-IP cooldown.
2. If it's a follower, it forwards to the leader over the internal network (marked `x-gambas-internal` so the leader neither re-charges the cooldown nor re-forwards).
3. The leader proposes `px:{x}:{y} = color` to the Raft log; quorum commits; the client gets its 200.
4. As the entry **applies on each node**, that node's apply loop broadcasts an event, and its WebSocket sessions push a 6-byte delta to their browsers.

**Board bootstrap** — a WebSocket client subscribes to the event stream *before* snapshotting the board, so no pixel can fall in the gap; a delta that repeats a snapshotted pixel is idempotent. Wire format:

```
server → client (binary):
  [0x01][65536 bytes]                     full board, row-major palette indices
  [0x02][x: u16 BE][y: u16 BE][color u8]  one pixel changed
```

## Design decisions

**Embedded consensus instead of an external store.** The app *is* the cluster: fewer moving parts, no network hop to a database, and the change feed is a `tokio::sync::broadcast` channel hanging off the apply loop instead of a polling layer. This required refactoring raft-kv from a standalone server into a library (`RaftKv::start/put/get/subscribe`) — the standalone server is now a thin wrapper over the same API.

**One key per pixel** (`px:{x:03}:{y:03}` → palette index). Writes are single-pixel, so this maps 1:1 to Raft log entries; fixed-width coordinates keep the whole board inside one lexicographic prefix scan; unpainted pixels don't exist, so an empty canvas costs nothing and Raft snapshots only carry paint.

**Deltas over WebSocket, paints over HTTP.** The WS is a one-way event stream — reconnect logic stays trivial (reconnect = fresh board frame). Paints go through `POST /api/pixel`, where per-IP cooldowns, validation errors, and leader forwarding get regular HTTP status codes instead of a bespoke WS protocol.

**Cooldown charged at the edge, rolled back on failure.** The node the client actually hit enforces one pixel per IP per 5s (atomic check-and-set under one lock), and returns the slot if the write doesn't commit. Forwarded requests carry a trusted internal header — which is why replicas are never exposed directly, only through the LB.

## Run it

**Docker (the full 3-node cluster + LB):**

```bash
git clone https://github.com/cacdu/gambas
cd gambas
docker compose up --build -d      # → http://localhost:8080
```

Watch consensus survive failure while you paint:

```bash
docker compose kill gambas3       # if 3 is the leader: re-election < 1s
docker compose start gambas3      # rejoins and catches up from the leader
```

**Frontend hot-reload (against the Docker cluster):**

```bash
docker compose up --build -d           # cluster + LB on :8080
cd web && npm install && npm run dev   # Vite on :5173, /api and /ws proxied to the LB
```

Edits under `web/` show up immediately — no image rebuild needed.

**Local dev without Docker (three terminals):**

```bash
make node1   # terminal 1 — http://127.0.0.1:8081
make node2   # terminal 2
make node3   # terminal 3
```

Vite's proxy targets `:8080` (`web/vite.config.ts`); point it at `:8081` to run the
dev server against `make node1` instead of the Docker LB.

## HTTP API

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/pixel` | `{x, y, color}` — paint one pixel (429 + `retry_after_ms` on cooldown) |
| `GET` | `/api/status` | Node id/role, canvas config, caller's remaining cooldown |
| `GET` | `/ws` | WebSocket: full board frame, then live pixel deltas |
| `GET` | `/metrics` | Prometheus — gambas counters plus the embedded node's Raft metrics |

## Stack

| Layer | Tech |
|-------|------|
| Consensus & storage | [raft-kv](https://github.com/cacdu/raft-kv) (embedded; hand-rolled Raft, WAL, ReadIndex) |
| HTTP + WebSocket | `axum` |
| Node-to-node | gRPC (`tonic`) between the embedded Raft nodes |
| Frontend | TypeScript + Vite, no framework — Canvas API with zoom/pan |
| Observability | Prometheus metrics |
| Deploy | Docker Compose: 3 replicas + nginx |

## Tests

```bash
cargo test    # canvas model, cooldown semantics, WS wire format, IP extraction
```

The consensus layer has its own suite in [raft-kv](https://github.com/cacdu/raft-kv): 28 unit tests over the pure state machine and the I/O layer, plus integration chaos tests (leader kill, partition & heal, WAL replay, membership changes).
