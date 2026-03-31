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
#   make lint            Run fmt + clippy
#   make build           Build release binary

COMPOSE_TARGETS = docker compose -f docker/docker-compose.yml
COMPOSE_OBSERVE = docker compose -f docker/docker-compose.observability.yml
COMPOSE_FULL    = $(COMPOSE_TARGETS) -f docker/docker-compose.observability.yml
COMPOSE_CLASSIC = $(COMPOSE_FULL) --profile classic

.PHONY: up up-targets up-observe up-classic down status reset logs \
        ssh-key test e2e lint build clean

# ── Docker Infrastructure ──────────────────────────────────────

up:
	$(COMPOSE_FULL) up -d
	@echo "Waiting for services to be healthy..."
	@sleep 5
	$(COMPOSE_FULL) ps
	@echo ""
	@echo "SigNoz UI:      http://localhost:13301"
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
	@bash docker/signoz/dashboards/import-dashboards.sh http://localhost:13301
	@echo ""
	@echo "Open SigNoz: http://localhost:13301 → Dashboards"

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
	RUSTFLAGS="-Dwarnings" cargo clippy --all-targets --all-features

build:
	cargo build --release -p tumult-cli

clean:
	cargo clean
	$(COMPOSE_FULL) --profile classic down -v 2>/dev/null || true
