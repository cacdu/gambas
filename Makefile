CARGO = cargo
DATA  = /tmp/gambas

.PHONY: build test frontend clean node1 node2 node3 up down

build:
	$(CARGO) build

test:
	$(CARGO) test

frontend:
	cd web && npm install && npm run build

clean:
	rm -rf $(DATA)

# ── local 3-node cluster (three terminals) ───────────────────────────────────

node1:
	mkdir -p $(DATA)/node1
	RUST_LOG=info $(CARGO) run -- \
		--id 1 --raft-addr 127.0.0.1:7101 --http-addr 127.0.0.1:8081 \
		--peer 2=127.0.0.1:7102 --peer 3=127.0.0.1:7103 \
		--app-peer 2=127.0.0.1:8082 --app-peer 3=127.0.0.1:8083 \
		--data-dir $(DATA)/node1

node2:
	mkdir -p $(DATA)/node2
	RUST_LOG=info $(CARGO) run -- \
		--id 2 --raft-addr 127.0.0.1:7102 --http-addr 127.0.0.1:8082 \
		--peer 1=127.0.0.1:7101 --peer 3=127.0.0.1:7103 \
		--app-peer 1=127.0.0.1:8081 --app-peer 3=127.0.0.1:8083 \
		--data-dir $(DATA)/node2

node3:
	mkdir -p $(DATA)/node3
	RUST_LOG=info $(CARGO) run -- \
		--id 3 --raft-addr 127.0.0.1:7103 --http-addr 127.0.0.1:8083 \
		--peer 1=127.0.0.1:7101 --peer 2=127.0.0.1:7102 \
		--app-peer 1=127.0.0.1:8081 --app-peer 2=127.0.0.1:8082 \
		--data-dir $(DATA)/node3

# ── docker ────────────────────────────────────────────────────────────────────

up:
	docker compose up --build -d

down:
	docker compose down -v
