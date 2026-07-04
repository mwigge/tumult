# Tumult — development commands
#
# Usage:
#   make up              Start chaos targets + SigNoz observability
#   make up-targets      Start chaos targets only (PostgreSQL, Redis, Kafka, SSH)
#   make up-observe      Start observability only (SigNoz + OTel Collector)
#   make up-classic      Start chaos targets + Jaeger/Prometheus/Grafana
#   make down            Stop everything
#   make status          Show container health
#   make test            Run all Rust tests
#   make e2e             Run e2e tests (requires up)
#   make lint            Run fmt + clippy + pedantic warnings
#   make precommit       Run all quality gates (lint + test + audit)
#   make build           Build release binary

COMPOSE_TARGETS = docker compose -f docker/docker-compose.yml
COMPOSE_OBSERVE = docker compose -f docker/docker-compose.observability.yml
COMPOSE_FULL    = $(COMPOSE_TARGETS) -f docker/docker-compose.observability.yml
COMPOSE_CLASSIC = $(COMPOSE_FULL) --profile classic
COMPOSE_DEMO    = docker compose -f docker/docker-compose.demo.yml

.PHONY: up up-targets up-observe up-classic down status reset logs \
        ssh-key test e2e lint precommit build clean \
        demo demo-down demo-check demo-base

# ── Docker Infrastructure ──────────────────────────────────────

up:
	$(COMPOSE_FULL) up -d
	@echo "Waiting for services to be healthy..."
	@sleep 10
	$(COMPOSE_FULL) ps
	@echo ""
	@echo "Importing SigNoz dashboards..."
	@bash docker/signoz/dashboards/import-dashboards.sh http://localhost:3301 2>/dev/null || echo "  (dashboards will import once SigNoz is fully ready — run 'make dashboards' to retry)"
	@echo ""
	@echo "SigNoz UI:      http://localhost:3301"
	@echo "OTLP endpoint:  http://localhost:14317"

up-targets:
	$(COMPOSE_TARGETS) up -d
	@sleep 3
	$(COMPOSE_TARGETS) ps

up-observe:
	$(COMPOSE_OBSERVE) up -d
	@sleep 5
	$(COMPOSE_OBSERVE) ps
	@echo ""
	@echo "SigNoz UI:      http://localhost:13301"
	@echo "OTLP endpoint:  http://localhost:14317"

up-classic:
	$(COMPOSE_CLASSIC) up -d
	@sleep 5
	$(COMPOSE_CLASSIC) ps
	@echo ""
	@echo "Jaeger:     http://localhost:16686"
	@echo "Grafana:    http://localhost:13000  (admin/tumult)"
	@echo "Prometheus: http://localhost:19090"

dashboards:
	@echo "Importing SigNoz dashboards..."
	@bash docker/signoz/dashboards/import-dashboards.sh http://localhost:3301
	@echo ""
	@echo "Open SigNoz: http://localhost:3301 → Dashboards"

# ── Tumult 2.2 one-command demo ────────────────────────────────
# See demo/CONTRACT.md and demo/README.md. Single network `tumult-demo`.

# The tumult-mcp image builds FROM the full `tumult` image, so build that
# base first. Reused by both `demo` and `demo-check`.
demo-base:
	@echo "Building tumult base image (base for tumult-mcp)..."
	docker build -f docker/Dockerfile.tumult -t tumult .

demo: demo-base
	$(COMPOSE_DEMO) build
	$(COMPOSE_DEMO) up -d
	@echo ""
	@echo "Waiting for health + running the fault sweep once to populate dashboards..."
	@COMPOSE_DEMO="$(COMPOSE_DEMO)" bash scripts/demo-check.sh --mode populate
	@echo ""
	@echo "Importing SigNoz dashboards..."
	@bash docker/signoz/dashboards/import-dashboards.sh http://localhost:3301 2>/dev/null || echo "  (SigNoz not ready yet — retry with: make dashboards)"
	@echo ""
	@echo "=================================================================="
	@echo "  Tumult 2.2 demo is up on the 'tumult-demo' network"
	@echo "=================================================================="
	@echo "  SigNoz (traces/metrics) ... http://localhost:3301"
	@echo "  Control panel ............. http://localhost:8088"
	@echo "  Demo app (order service) .. http://localhost:8080"
	@echo "  Tumult MCP (HTTP) ......... http://localhost:3100/mcp"
	@echo "  OTLP collector ............ grpc :14317 / http :14318"
	@echo "------------------------------------------------------------------"
	@echo "  try: open the control panel and click Run on any fault card"
	@echo "=================================================================="

demo-check: demo-base
	$(COMPOSE_DEMO) build
	$(COMPOSE_DEMO) up -d
	@COMPOSE_DEMO="$(COMPOSE_DEMO)" bash scripts/demo-check.sh --mode full

demo-proof:
	@echo "Validating Tumult's claims against the live demo (no mocks)..."
	@python3 demo/proof/validate.py

demo-down:
	$(COMPOSE_DEMO) down -v 2>/dev/null || true

down:
	$(COMPOSE_FULL) --profile classic down -v 2>/dev/null || true

status:
	$(COMPOSE_FULL) ps 2>/dev/null || $(COMPOSE_TARGETS) ps

reset: down up

logs:
	$(COMPOSE_FULL) logs -f

# Keep backwards compat
infra-up: up
infra-down: down
infra-status: status
infra-reset: reset

# ── Extract SSH test key from container ────────────────────────

ssh-key:
	docker cp $$($(COMPOSE_TARGETS) ps -q sshd):/test_key /tmp/tumult-test-key
	chmod 600 /tmp/tumult-test-key
	@echo "SSH test key saved to /tmp/tumult-test-key"
	@echo "Test: ssh -p 12222 -i /tmp/tumult-test-key -o StrictHostKeyChecking=no tumult@localhost uname -a"

# ── Testing ────────────────────────────────────────────────────

test:
	cargo test --workspace

e2e: build up
	@echo "Running e2e tests against Docker infrastructure..."
	TUMULT_PG_HOST=localhost TUMULT_PG_PORT=15432 TUMULT_PG_USER=tumult TUMULT_PG_PASSWORD=tumult_test TUMULT_PG_DATABASE=tumult_test \
	TUMULT_REDIS_HOST=localhost TUMULT_REDIS_PORT=16379 \
	TUMULT_KAFKA_BOOTSTRAP=localhost:19092 \
	OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317 \
	cargo test --workspace -- --ignored e2e 2>&1
	@echo "E2E tests complete. Check SigNoz at http://localhost:13301"

# ── Quality ────────────────────────────────────────────────────

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings -W clippy::pedantic

precommit: lint
	cargo test --workspace
	cargo audit

build:
	cargo build --release -p tumult-cli

clean:
	cargo clean
	$(COMPOSE_FULL) --profile classic down -v 2>/dev/null || true
