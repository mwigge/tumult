# kronika

*The chronicle of your resilience work.*

Kronika is a graphical analytics & reporting app for chaos-engineering data —
the companion piece to [tumult](https://github.com/mwigge/tumult) (chaos
execution CLI) and smedja. It ingests resilience telemetry automatically over
OpenTelemetry and manually from files, stores it in embedded DuckDB, and
turns it into rollups, drill-downs, dashboards and scheduled narrative
reports — with a guarded AI analytics layer in later phases.

<!-- status badges: placeholder until CI exists -->
<!-- ![ci](...) ![license](https://img.shields.io/badge/license-MIT-blue) -->

## Features

- **Automatic ingest** — OTLP/gRPC (`:4317`, tumult's exporter) and
  OTLP/HTTP protobuf (`:4318`, `/v1/*`, smedja's exporter) on one daemon.
- **Manual import** — CSV files and tumult journal JSON
  (`kronikad import <file>`), tracked in `import_batches`.
- **Embedded DuckDB store** — wide, ClickHouse-exporter-aligned tables +
  `MAP(VARCHAR, VARCHAR)` attrs; single-writer with coexisting read-only
  readers (tumult-analytics pattern).
- **Domain-native schema** — promotes the tumult `resilience.*` metadata
  standard (v2.0) into materialized columns: experiment identity, outcome,
  fault taxonomy, target, hypothesis verdict, recovery time.
- **Semantic metrics layer** — Rill-style YAML metric views
  (`metrics/*.yaml`) compiled to strictly validated SQL (injection-impossible
  by construction).
- **Reports** — self-contained HTML digests (`kronikad report --metric …`),
  scheduled reports via tokio interval; email delivery in Phase 2.
- **AI groundwork** — OpenAI-compatible LLM interface + SQL guardrail
  pipeline (read-only, allow-listed, single-SELECT, injected LIMIT). No live
  LLM calls yet; see [docs/adr/0002-ai-layer.md](docs/adr/0002-ai-layer.md).
- **Web UI** — SvelteKit skeleton in `web/` (KPI row → trend → leaderboard →
  drill-down, custom span-waterfall as the signature piece). See
  [web/README.md](web/README.md).

## Docker demo (easiest path)

One command exercises the full pipeline — OTLP/gRPC + OTLP/HTTP ingest,
DuckDB storage, semantic metrics, HTML reports:

```sh
docker compose -f docker/docker-compose.demo.yml up
```

What happens:

1. `kronikad` starts (host ports `14317`/`14318`, store in a named volume).
2. `seed` fires **40 synthetic chaos experiments** at it over real OTLP/gRPC —
   full `resilience.experiment` span trees (hypothesis → action → probe →
   rollback), `tumult.*` metrics and correlated logs, spread over the past 14
   days so rollups have shape (deterministic via `--seed 7`).
3. `report` fetches one self-contained HTML report per semantic metric from
   kronikad's live `GET /report?metric=<name>` endpoint into **`demo-out/`** —
   open `demo-out/hypothesis_pass_rate.html` in a browser.

Re-seed with more (or different) data:

```sh
docker compose -f docker/docker-compose.demo.yml run --rm seed \
  kronika-demo --endpoint http://kronikad:4317 --experiments 40 --seed 8
```

While the stack is up, point **real** telemetry at the same ports: tumult via
`TUMULT_OTEL_ENABLED=true OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317`,
smedja via `SMEDJA_OTLP_ENDPOINT=http://localhost:14318`.

> Host ports are `14317`/`14318` (not the OTLP-standard `4317`/`4318`) so the
> demo runs side-by-side with an existing collector — e.g. a local SigNoz —
> that already owns `4317`/`4318`. Container-internal traffic still uses the
> standard ports.

Clean up (drops the demo volume):

```sh
docker compose -f docker/docker-compose.demo.yml down -v
```

## Quickstart (from source)

```sh
# Run the daemon (DB at ~/.kronika/kronika.duckdb; override with KRONIKA_DB)
cargo run -p kronikad

# Point tumult at it (OTLP/gRPC, bare host, no path)
export TUMULT_OTEL_ENABLED=true
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
tumult run experiment.toon

# Point smedja at it (OTLP/HTTP, /v1/* paths)
export SMEDJA_OTLP_ENDPOINT=http://localhost:4318

# Manual import (tumult journal JSON or CSV with headers)
cargo run -p kronikad -- import journal.json --label "march game day"

# Ad-hoc HTML report for a semantic metric
# (from the running daemon — DuckDB allows only one read-write process,
# so the report subcommand below requires the daemon to be stopped)
curl "localhost:4318/report?metric=hypothesis_pass_rate" > report.html

# Same report via the CLI (writes to stdout or --out <file>)
cargo run -p kronikad -- report --metric hypothesis_pass_rate > report.html

# Health check
curl localhost:4318/healthz
```

Configuration (all env vars, see `kronika-ingest/src/config.rs`):

| Var | Default | Purpose |
|---|---|---|
| `KRONIKA_OTLP_GRPC_ADDR` | `0.0.0.0:4317` | OTLP/gRPC listen address |
| `KRONIKA_OTLP_HTTP_ADDR` | `0.0.0.0:4318` | OTLP/HTTP listen address |
| `KRONIKA_DB` | `~/.kronika/kronika.duckdb` | DuckDB store path |
| `KRONIKA_METRICS_DIR` | `metrics/` | semantic metric definitions |
| `KRONIKA_LLM_BASE_URL` | `http://localhost:11434/v1` | LLM endpoint (Ollama) |
| `KRONIKA_LLM_API_KEY` | — | LLM API key |
| `KRONIKA_LLM_MODEL` | `qwen2.5:7b` | LLM model |

## Repository layout

```
bin/kronikad        daemon + CLI (serve / import / report)
bin/kronika-demo    synthetic chaos generator (real OTLP client; demo seeding)
crates/kronika-store    embedded DuckDB store (single-writer + RO readers)
crates/kronika-otel     OTLP proto → row translation (pure)
crates/kronika-ingest   gRPC/HTTP servers, writer channel, manual import
crates/kronika-metrics  YAML semantic layer → validated SQL
crates/kronika-report   report model, HTML renderer, scheduler
crates/kronika-ai       Llm trait, OpenAI-compatible client, SQL guardrails
metrics/            starter semantic metric definitions
web/                SvelteKit UI skeleton (hand-written, not installed)
docs/               research, architecture, ADRs
docker/             Dockerfile, one-command demo stack, optional otel-collector dev tooling
```

## Docs

- [docs/research.md](docs/research.md) — the research behind the decisions
- [docs/architecture.md](docs/architecture.md) — components, data flow,
  single-writer model, lake-export roadmap
- [docs/adr/0001-stack.md](docs/adr/0001-stack.md),
  [docs/adr/0002-ai-layer.md](docs/adr/0002-ai-layer.md)

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## License

MIT — Morgan Wigge. See [LICENSE](LICENSE).
