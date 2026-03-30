# Tumult — development commands
#
# Usage:
#   make infra-up        Start all Docker test infrastructure
#   make infra-down      Stop and remove all containers + volumes
#   make infra-status    Show container health status
#   make infra-reset     Stop, remove, and restart fresh
#   make test            Run all Rust tests
#   make e2e             Run e2e tests (requires infra-up)
#   make lint            Run fmt + clippy
#   make build           Build release binary

.PHONY: infra-up infra-down infra-status infra-reset infra-logs test e2e lint build clean

# ── Docker Infrastructure ──────────────────────────────────────

infra-up:
	cd docker && docker compose up -d
	@echo "Waiting for services to be healthy..."
	@sleep 5
	cd docker && docker compose ps

infra-down:
	cd docker && docker compose down -v

infra-status:
	cd docker && docker compose ps

infra-reset: infra-down infra-up

infra-logs:
	cd docker && docker compose logs -f

# ── Extract SSH test key from container ────────────────────────

ssh-key:
	docker cp docker-sshd-1:/test_key /tmp/tumult-test-key
	chmod 600 /tmp/tumult-test-key
	@echo "SSH test key saved to /tmp/tumult-test-key"
	@echo "Test: ssh -p 12222 -i /tmp/tumult-test-key -o StrictHostKeyChecking=no tumult@localhost uname -a"

# ── Testing ────────────────────────────────────────────────────

test:
	cargo test --workspace

e2e: infra-up
	@echo "Running e2e tests against Docker infrastructure..."
	TUMULT_PG_HOST=localhost TUMULT_PG_PORT=15432 TUMULT_PG_USER=tumult TUMULT_PG_PASSWORD=tumult_test TUMULT_PG_DATABASE=tumult_test \
	TUMULT_REDIS_HOST=localhost TUMULT_REDIS_PORT=16379 \
	TUMULT_KAFKA_BOOTSTRAP=localhost:19092 \
	OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317 \
	cargo test --workspace -- --ignored e2e 2>&1 || true
	@echo "E2E tests complete. Check Jaeger at http://localhost:16686"

# ── Quality ────────────────────────────────────────────────────

lint:
	cargo fmt --all -- --check
	RUSTFLAGS="-Dwarnings" cargo clippy --all-targets --all-features

build:
	cargo build --release -p tumult-cli

clean:
	cargo clean
	cd docker && docker compose down -v 2>/dev/null || true
