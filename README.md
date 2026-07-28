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

## Quickstart

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
crates/kronika-store    embedded DuckDB store (single-writer + RO readers)
crates/kronika-otel     OTLP proto → row translation (pure)
crates/kronika-ingest   gRPC/HTTP servers, writer channel, manual import
crates/kronika-metrics  YAML semantic layer → validated SQL
crates/kronika-report   report model, HTML renderer, scheduler
crates/kronika-ai       Llm trait, OpenAI-compatible client, SQL guardrails
metrics/            starter semantic metric definitions
web/                SvelteKit UI skeleton (hand-written, not installed)
docs/               research, architecture, ADRs
docker/             optional otel-collector dev tooling
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
