# Build context is the PARENT directory holding both repos side by side
# (gambas depends on ../raft-kv by path):
#   docker build -f gambas/Dockerfile ..

# ── frontend ──────────────────────────────────────────────────────────────────
FROM node:22-alpine AS web
WORKDIR /web
COPY gambas/web/package.json gambas/web/package-lock.json ./
RUN npm ci --no-fund --no-audit
COPY gambas/web .
RUN npm run build

# ── backend ───────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY raft-kv ./raft-kv
COPY gambas/Cargo.toml gambas/Cargo.lock ./gambas/
COPY gambas/src ./gambas/src
WORKDIR /src/gambas
RUN cargo build --release

# ── runtime ───────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /src/gambas/target/release/gambas /usr/local/bin/gambas
COPY --from=web /web/dist ./web/dist
EXPOSE 8080 7001
ENTRYPOINT ["gambas"]
