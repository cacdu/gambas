# Build context is the repo root: docker build .
# raft-kv is fetched by Cargo from its pinned git tag during the build.

# ── frontend ──────────────────────────────────────────────────────────────────
FROM node:22-alpine AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-fund --no-audit
COPY web .
RUN npm run build

# ── backend ───────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

# ── runtime ───────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /src/target/release/gambas /usr/local/bin/gambas
COPY --from=web /web/dist ./web/dist
EXPOSE 8080 7001
ENTRYPOINT ["gambas"]
