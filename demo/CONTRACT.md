# Tumult Demo — Interface Contract (2.21)

This file is the shared contract for the one-command demo. All demo components
build to these names/ports so they compose without collision. Do not change a
value here without updating every component that references it.

## One command
- `make demo` — bring the whole stack up on a single docker network, import
  dashboards, print URLs, run a per-domain fault-injection sweep once so the
  dashboards populate immediately.
- `make demo-down` — tear down, remove volumes.
- `make demo-check` — headless smoke test: bring up, wait for health, run the
  sweep, assert traces+metrics landed, exit non-zero on any failure. Used as a
  functional smoke test in development.

## Docker
- Compose file: `docker/docker-compose.demo.yml`
- Network name: `tumult-demo` (external: false, single network for all services)
- Project name: `tumult-demo` (COMPOSE_PROJECT_NAME)

## Services (name → purpose → host port)
- `demo-postgres` — Postgres 16 the app depends on → 5433:5432
- `demo-app` — purpose-built axum order service, OTel-instrumented → 8080:8080
- `demo-sshd` — sshd container as the ssh fault target (reuse docker/Dockerfile.sshd) → 2222:22
- `signoz` — observability UI (traces+metrics+dashboards) → 3301:3301
- `tumult-collector` — OTel collector (reuse docker/tumult-collector/config.yaml) → OTLP grpc 4317 (host 14317), http 4318 (host 14318)
- `tumult-mcp` — Tumult MCP server, HTTP transport → 3100:3100
- `demo-control-panel` — web control panel (MCP client) → 8088:8088
- `demo-traffic` — lightweight traffic generator hitting demo-app (so there is baseline load to disrupt) — no host port

## Auth / env
- `TUMULT_MCP_TOKEN=tumult-demo` (dev-only token, same everywhere)
- App → collector OTLP: `OTEL_EXPORTER_OTLP_ENDPOINT=http://tumult-collector:4318`
- App service name: `demo-app`; control panel service name: `demo-control-panel`
- Postgres: db `orders`, user `demo`, password `demo` (dev-only)

## demo-app HTTP surface (axum)
- `GET /health` → 200 `{status:"ok"}` when DB reachable, 503 otherwise
- `GET /orders` → list orders (DB read; emits a span)
- `POST /orders` → create order (DB write; emits a span)
- `GET /checkout/:id` → simulated multi-step op (DB + internal work; the richest trace)
- Every handler emits OTel spans to the collector; service name `demo-app`.

## Fault domains (docker-viable — NO k8s in demo)
`demo/experiments/` holds 13 experiments targeting the demo stack. The fault
sweep (`make demo-check`) is one per domain, named `demo-<domain>.toon`:
- `demo-net.toon` — tumult-net userspace proxy: latency between demo-app and demo-postgres
- `demo-postgres.toon` — tumult-db-postgres script plugin: kill connections
- `demo-container.toon` — tumult-pumba or container pause: pause demo-postgres briefly
- `demo-stress.toon` — tumult-stress: CPU/memory pressure on demo-app container
- `demo-process.toon` — process fault against demo-app
- `demo-ssh.toon` — tumult-ssh native execute against demo-sshd
- `demo-agentic.toon` — an agentic scenario smoke (no external API — bundled fake adapter)
- `demo-agentic-trajectory.toon` — multi-turn agent-graph trajectory poisoning (bundled fake adapters)
- `demo-timewarp-clock.toon` — tumult-timewarp clock skew past a short-TTL token's expiry
- `demo-timewarp-entropy.toon` — tumult-timewarp sustained RNG/crypto pressure

Three experiments are deliberately outside the sweep:
- `demo-guard-halt.toon` — auto-halt guardrail; expected outcome is `Halted`, not `Completed` (control panel's Safety guardrail card)
- `demo-topo-blind.toon` — recovery validation with a blind guard, wired as the autopilot playbook in `demo/topology/autopilot-blind.toml`
- `demo-topo-recommended.toon` — the recommender loop's closing run (`scripts/demo-topology.sh`)

Each experiment must `validate` clean and run against the live demo stack.

## Control panel (demo-control-panel)
- Small web app; serves a single dashboard page at `http://localhost:8088`.
- It is an MCP client of `tumult-mcp` (http://tumult-mcp:3100, bearer TUMULT_MCP_TOKEN).
- Per-domain cards: name, description, a Run button (calls MCP tumult_run_experiment
  with the domain's experiment), live status (running/passed/failed + duration),
  and a "View traces" deep link into SigNoz filtered to service demo-app around
  the run window.
- Uses the MCP tool annotations to badge destructive actions (a confirm step).
- No build-time secrets; token from env.

## SigNoz
- Reuse `docker/signoz/dashboards/*.json` + `import-dashboards.sh` (import against
  http://localhost:3301). The demo-app dashboard focus: the experiment-phases,
  experiments-overview, postgres, and resilience-score dashboards are the headline ones.

## Ports summary (host)
5433 postgres · 8080 app · 2222 sshd · 3301 signoz · 14317/14318 collector ·
3100 mcp · 8088 control-panel
