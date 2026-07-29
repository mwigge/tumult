# kronika

*The chronicle of your resilience work.*

Kronika is a graphical analytics & reporting app for chaos-engineering data —
the companion piece to [tumult](https://github.com/mwigge/tumult) (chaos
execution CLI) and smedja. It ingests resilience telemetry automatically over
OpenTelemetry and manually from files, stores it in embedded DuckDB, and
turns it into rollups, drill-downs, dashboards and scheduled narrative
reports — with a guarded AI analytics layer in later phases.

<!-- status badges: placeholder until CI exists -->
<!-- ![ci](...) ![license](https://img.shields.io/badge/license-Apache--2.0-blue) -->

## Features

- **Automatic ingest** — OTLP/gRPC (`:4317`, tumult's exporter) and
  OTLP/HTTP protobuf (`:4318`, `/v1/*`, smedja's exporter) on one daemon.
- **Manual import** — CSV files and tumult journal JSON
  (`kronikad import <file>`), tracked in `import_batches`.
- **Embedded DuckDB store** — wide, ClickHouse-exporter-aligned tables +
  `MAP(VARCHAR, VARCHAR)` attrs; single-writer with coexisting read-only
  readers (tumult-analytics pattern).
- **Parquet lake + retention** — incremental, watermark-driven export of
  every table to immutable day-partitioned parquet files
  (`KRONIKA_LAKE_DIR`, default `<db dir>/lake`) on a scheduled daemon job
  (`KRONIKA_LAKE_INTERVAL`, default `24h`) or `POST /api/lake/export`;
  `GET /api/lake/status` reports watermarks/files/bytes. Optional
  retention (`KRONIKA_RETENTION_DAYS`, default 0 = keep forever) deletes
  only already-exported hot rows; the manual-evidence tables are never
  deleted. Immutability as a compliance feature: write-once parquet +
  the hash-chained audit = a WORM-shaped evidence trail (ADR 0005).
- **Domain-native schema** — promotes the tumult `resilience.*` metadata
  standard (v2.0) into materialized columns: experiment identity, outcome,
  fault taxonomy, target, hypothesis verdict, recovery time.
- **Semantic metrics layer** — Rill-style YAML metric views
  (`metrics/*.yaml`) compiled to strictly validated SQL (injection-impossible
  by construction).
- **Reports** — compliance-grade documents (v2): R1 executive resilience
  digest (with a By-domain org rollup and an automated/manual evidence-mix
  footnote), R3 per-run game-day report and an R2 evidence-pack skeleton
  (DORA/NIS2/ISO 27001/SOC 2 clause lists; the test register carries
  manual-evidence provenance and a per-entry attestation appendix)
  rendered as embedded-Typst PDFs
  plus print-HTML previews via `kronika-docs`, generated from the UI or
  `POST /api/reports/v2/generate`, persisted with SHA-256 metas under
  `<db dir>/reports/v2/`. Resilience scoring (Gremlin-style with 30-day
  freshness decay) feeds both the digest and `GET /api/scores`. Classic
  self-contained HTML metric digests (`kronikad report --metric …`) remain:
  set `KRONIKA_REPORT_INTERVAL=1h` and the daemon renders one digest per
  interval into `<db dir>/reports/`, browsable from the UI.
- **Org hierarchy rollups** — a Backstage-style `org.yaml`
  (`KRONIKA_ORG_FILE`, default `<db dir>/org.yaml`) declares a
  single-parent tree (team → unit → domain) with glob assignments and
  per-experiment criticality (critical ×3, high ×2). Node scores are
  criticality-weighted means recomputed from every leaf in the subtree —
  never averages of child means — with scored/expected coverage next to
  every number and unmapped experiments visible in a synthetic
  `(unassigned)` bucket. Served at `GET /api/scores/tree` and browsable on
  the UI's Scores page (treemap → click-to-drill → tree table).
- **Manual evidence** — hand-executed tests (game days, tabletops,
  failovers, pentests, drills) entered via `POST /api/manual/experiments`
  or the UI's Manual page: a draft → submitted → verified/rejected
  lifecycle with mandatory attestation on submit, reviewer ≠ enterer
  enforcement (DORA Art. 24(4) / ISO 27001 A.5.35), an append-only
  hash-chained audit trail, and URI-only evidence attachments. Verified
  records score exactly like automated runs (inconclusive excluded);
  drafts/submitted count toward coverage as pending. Bulk import
  (`POST /api/manual/import`) lands records as attested drafts. No auth:
  the "acting as" name is a plain string — workflow scaffolding, not
  access control.
- **Query API** — read-only JSON under `/api/*` (overview KPIs with deltas
  and sparklines, bucketed time series for any semantic metric, experiment
  list/detail with waterfall spans + correlated logs, logs search + volume,
  trace grouping + durations + detail, raw metric catalog + grouped queries,
  service/target topology, dimensions, reports, resilience scores, guarded
  NL→SQL ask),
  executed on read-only connections that coexist with the ingest writer.
- **Web UI** — SvelteKit SPA embedded into the kronikad binary and served on
  the HTTP port: Overview KPIs, calendar heatmap, fault donut, filterable
  experiment list, custom span waterfall with a span detail drawer, Logs,
  Traces, Metrics and Topology explorers, NL Ask, Reports. Experiment runs
  overlay the Overview/Metrics charts as outcome-coloured bands
  (click → the run), and attribute values in the log/span detail views
  offer ⊕/⊖ click-to-filter. See
  [web/README.md](web/README.md).
- **AI analytics** — OpenAI-compatible LLM interface + SQL guardrail
  pipeline (read-only, allow-listed, single-SELECT, injected LIMIT), live
  behind `POST /api/ask` with curated golden answers when no LLM is
  configured; digest narratives grounded sentence-by-sentence against the
  report's own numbers (`kronika_report::narrative`); see
  [docs/adr/0002-ai-layer.md](docs/adr/0002-ai-layer.md).

## Docker demo (easiest path)

One command exercises the full pipeline with **real chaos experiments** —
tumult runs, genuine OTLP/gRPC traces + metrics + logs, DuckDB storage,
semantic metrics, the web UI:

```sh
docker compose -f docker/docker-compose.demo.yml up
```

Then open **http://localhost:14318/** — the kronika UI (Overview →
Experiments → waterfall drill-down → Ask → Reports).

What happens:

1. `kronikad` starts (host ports `14317`/`14318`, store in a named volume).
2. `seed` runs the **real tumult experiment suite** in `demo/experiments/` —
   eight `.toon` experiments executed by the pinned tumult **v2.18.0** release
   binary (fetched from GitHub releases and checksum-verified against the
   published `SHA256SUMS.txt` at image build time). Each run emits genuine
   OTLP into kronikad: `resilience.experiment` span trees, `tumult.*`
   metrics, and structured logs. Six experiments are designed to pass, one to
   **deviate** (config corruption, rolled back) and one to **fail**
   (dependency restart, rolled back) — so the reports show real deviations
   and rollbacks, not just green runs.
3. `report` fetches one self-contained HTML report per semantic metric from
   kronikad's live `GET /report?metric=<name>` endpoint into **`demo-out/`** —
   open `demo-out/hypothesis_pass_rate.html` in a browser. The daemon also
   renders its own digest hourly (`KRONIKA_REPORT_INTERVAL=1h` in the demo)
   into `/data/reports/`, listed on the UI's **Reports** page.

This is the cross-repo contract in action: kronika ingests exactly what
[tumult](https://github.com/mwigge/tumult) emits. Extend the suite by
dropping your own tumult experiment files into `demo/experiments/` — the seed
runs every `*.toon` it finds (they must be safe in a plain container:
process/script actions, no SSH/k8s/network targets).

Re-run the suite against the same store (data accumulates):

```sh
docker compose -f docker/docker-compose.demo.yml run --rm seed
```

Optional synthetic backfill: the `kronika-demo` generator (40 generated
experiments spread over the past 14 days, for time-series shape) stays
available behind a profile — it is a backfill, not the demo's seed:

```sh
docker compose -f docker/docker-compose.demo.yml --profile synthetic up
# reports are generated after the tumult suite; re-fetch them once the
# synthetic backfill has landed:
docker compose -f docker/docker-compose.demo.yml run --rm report
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

# Automatic scheduled digests (renders into <db dir>/reports/ hourly;
# browse them on the UI's Reports page)
KRONIKA_REPORT_INTERVAL=1h cargo run -p kronikad

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
| `KRONIKA_ORG_FILE` | `<db dir>/org.yaml` | org hierarchy for scores/tree + R1 By-domain |
| `KRONIKA_REPORT_INTERVAL` | off | automatic digest interval (`45s`, `30m`, `1h`, `1d`) |
| `KRONIKA_LAKE_DIR` | `<db dir>/lake` | parquet lake root (day-partitioned immutable export) |
| `KRONIKA_LAKE_INTERVAL` | `24h` | lake export interval; `0`/`off` disables the job |
| `KRONIKA_RETENTION_DAYS` | `0` (keep forever) | delete already-exported hot rows older than N days; the manual-evidence tables are never deleted |
| `KRONIKA_LLM_BASE_URL` | `http://localhost:11434/v1` | LLM endpoint (Ollama) |
| `KRONIKA_LLM_API_KEY` | — | LLM API key |
| `KRONIKA_LLM_MODEL` | `qwen2.5:7b` | LLM model |

## Repository layout

```
bin/kronikad        daemon + CLI (serve / import / report), embeds + serves web/
bin/kronika-demo    synthetic chaos generator (optional demo backfill)
crates/kronika-store    embedded DuckDB store (single-writer + RO readers),
                        manual evidence tables + lifecycle
crates/kronika-otel     OTLP proto → row translation (pure)
crates/kronika-ingest   gRPC/HTTP servers, writer channel, manual import
crates/kronika-metrics  YAML semantic layer → validated SQL (+ bucketed series)
crates/kronika-report   report model, HTML renderer, scheduler
crates/kronika-ai       Llm trait, OpenAI-compatible client, SQL guardrails
crates/kronika-api      JSON query API backing the UI (/api/*, incl. manual writes
                        via the daemon's single writer)
crates/kronika-docs     compliance report pipeline (Typst PDF/HTML), resilience
                        scoring, org hierarchy rollups
metrics/            starter semantic metric definitions
demo/experiments/   tumult experiment suite run by the docker demo seed
demo/org.yaml       demo org hierarchy (mounted to /data/org.yaml)
web/                SvelteKit SPA (embedded into kronikad; see web/README.md)
docs/               research, architecture, ADRs
docker/             Dockerfile, one-command demo stack, optional otel-collector dev tooling
```

## Docs

- [docs/research.md](docs/research.md) — the research behind the decisions
- [docs/architecture.md](docs/architecture.md) — components, data flow,
  single-writer model, lake-export roadmap
- [docs/research-org-rollups.md](docs/research-org-rollups.md) /
  [docs/research-manual-evidence.md](docs/research-manual-evidence.md) —
  v0.5.0 research (weighted org rollups; attested manual evidence)
- [docs/adr/0001-stack.md](docs/adr/0001-stack.md),
  [docs/adr/0002-ai-layer.md](docs/adr/0002-ai-layer.md),
  [docs/adr/0003-typst-report-pipeline.md](docs/adr/0003-typst-report-pipeline.md),
  [docs/adr/0004-org-hierarchy-and-manual-evidence.md](docs/adr/0004-org-hierarchy-and-manual-evidence.md)

## Development

```sh
cd web && npm ci && npm run build && cd ..  # required once: kronikad embeds web/build/
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## License

Apache-2.0 — Morgan Wigge. See [LICENSE](LICENSE).

### Third-party attributions

- [Apache ECharts](https://echarts.apache.org/) (Apache-2.0) — charting in the web UI.
  Copyright 2017-2024 The Apache Software Foundation. This product includes
  software developed at The Apache Software Foundation (https://www.apache.org/).
- [zrender](https://github.com/ecomfe/zrender) (BSD-3-Clause) — ECharts rendering engine.
- [tslib](https://github.com/microsoft/tslib) (0BSD) — TypeScript runtime helpers.
- [Typst](https://typst.app/) (Apache-2.0) — embedded PDF typesetting for reports.
- [DuckDB](https://duckdb.org/) (MIT) — embedded analytical store.
- Fonts: [Inter](https://rsms.me/inter/) and
  [Source Serif 4](https://github.com/adobe-fonts/source-serif) (SIL OFL 1.1) —
  vendored with their license texts under `assets/fonts/`.
