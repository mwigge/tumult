# Changelog

All notable changes to kronika are documented here.

Format: `## [version] — YYYY-MM-DD` / `### Added|Fixed|Changed|Removed|Roadmap`.

---

## [0.4.0] — 2026-07-28

### Added
- **Compliance-grade report pipeline** (`kronika-docs` crate, ADR 0003): a
  renderer-agnostic content model (`ReportDoc`/`Block`/`ChartSpec`) with
  two outputs — embedded-Typst PDFs (typst 0.15 compiled in-process, no
  external runtime) and print-styled HTML previews. Charts are shared
  vector SVGs (Okabe–Ito palette, direct labels); fonts are vendored OFL
  Inter + Source Serif 4 (`assets/fonts/`, embedded in the binary) so
  docker builds stay offline-reproducible.
- **Three report templates**: R1 executive digest (deterministic BLUF,
  portfolio KPIs, target scores, issues discovered/fixed + MTTR, open
  weaknesses, outlook), R3 game-day report (run summary, blast radius &
  rollback, span timeline, verdict, findings, config appendix), and an R2
  evidence-pack skeleton for DORA/NIS2/ISO 27001/SOC 2 with a traceability
  matrix, test register, findings log, sign-off and the mandatory
  "verify clause references against the licensed framework text" footnote.
  Document IDs `KRK-<code>-<yyyymmdd>-<hash6>`; artifacts persisted as
  `{id}.pdf`/`.html`/`.json` with the PDF's SHA-256 in the meta.
- **Resilience scoring** (`kronika_docs::scoring`): Gremlin-style scores
  with 30-day freshness decay (passed 100 / stale 75 / failed 50 / never
  run 0; bands >70 good, 50–70 fair, <50 poor), target and portfolio
  rollups with a period-over-period delta, served at `GET /api/scores`.
- **`/api/reports/v2/*`**: `POST …/generate {type,period?,experiment_id?,
  framework?}`, `GET …/v2` (metas newest first), `GET …/v2/{id}/pdf|html`
  (strict doc-id validation). Integration tests cover the scorecard, all
  three template round-trips and the validation paths.
- **Reports UI v2**: template picker with conditional
  framework/experiment/period controls, artifact list with type badge and
  short SHA, iframe print preview, PDF download; quick metric digests kept
  below. Tabular numerals adopted in `theme.css` (`table.data`, `.mono`).
- Docs: `docs/research-compliance.md`, `docs/research-ux.md`,
  `docs/adr/0003-typst-report-pipeline.md`.
- Report visual polish: composed covers (wordmark + accent rule,
  classification chip, prominent period, document control anchored to the
  page bottom), R1 score-trend line and coverage donut, per-experiment bar
  charts with value labels, balanced KPI grids, glyph+label status cells
  (`Cell::glyph`, never hue alone), readable R3 timeline statuses (OTel
  codes mapped), and fraction-width table columns with justification and
  hyphenation disabled in cells.

### Roadmap (deferred from this cycle)
- ⌘K command palette (wants a global id-search backend first).
- Brush/range-select on Overview charts (needs a coordinated selection
  model; likely with Phase-2 Mosaic crossfiltering).
- BubbleUp-style "explain this spike" drill-downs.
- Notebook-style ad-hoc reports on top of the v2 content model.
- R4/R5 templates (service deep-dive, regulator run-log) pending auditor
  feedback; triage inbox for open weaknesses.

---

### Added
- Logs explorer: `GET /api/logs` (range/severity/service/q/limit; severity a
  case-insensitive exact match, `q` an escaped contains-match; newest first,
  `experiment_id` lifted from log attributes for linking) and
  `GET /api/logs/volume` (bucketed counts per severity). `/logs` UI page with
  a stacked-bar volume chart, URL-synced filters and expandable rows exposing
  attributes plus experiment/trace links.
- Traces explorer: `GET /api/traces` (spans grouped into traces — root
  name/service, span/error counts, experiment outcome where applicable;
  service/min-duration/outcome filters), `GET /api/traces/durations`
  (root-span duration points plus p50/p95/p99 via `quantile_cont`) and
  `GET /api/traces/{id}` (every span and log of one trace). `/traces` UI page
  with a log-scale duration scatter (percentile mark lines, click-through)
  and a slowest-first table; `/traces/[id]` reuses the waterfall and span
  drawer.
- Raw metrics explorer: `GET /api/metrics/catalog` (names across
  sums/gauges/histograms with the attribute keys seen on their points) and
  `GET /api/metrics/query` (bucketed series; sums `SUM`, gauges `AVG`,
  histograms aggregate avg plus an interpolated p95 computed in Rust;
  optional split by a strict-charset attribute key; unknown names 404).
  `/metrics` UI page with typed picker, group-by dropdown, line/area/bar
  toggle, interval and range controls.
- Topology: `GET /api/topology` (service/target nodes with runs/errors/avg
  aggregates from `service_name` and tumult's `resilience.target.name`
  attribute; edges from parent→child span joins and service→target calls;
  capped at 100 nodes). `/topology` UI page with a force-directed graph —
  node size by span count, services colored by error rate, click-through
  from a service to its traces.
- Grounded LLM narratives (`kronika_report::narrative`): a facts package
  built from the report's own KPI/table numbers goes to the LLM; only
  sentences whose numerals are grounded in those facts survive (percent
  matches `x` and `x/100` forms; 1% tolerance for rounding). Unreachable
  LLM, 30s timeout or a fully ungrounded reply leaves the digest unchanged.
  Wired into the daemon's report scheduler and `POST /api/reports/generate`.
  ADR 0002 updated: Phase 2 landed.

### Changed
- `EChart.svelte` accepts an optional click handler; ECharts registers the
  scatter and graph charts.

---

## [0.2.0] — 2026-07-28

### Added
- `kronika-api`: read-only JSON query API backing the UI, mounted on the
  daemon's HTTP server — `GET /api/overview` (KPI cards with value, delta vs
  the previous equal window and sparklines; experiments per day; target
  leaderboard; fault breakdown), `GET /api/timeseries` (any semantic metric
  as a bucketed series), `GET /api/experiments` + `/api/experiments/{id}`
  (outcome joined from tumult's `experiment.completed` log attributes; spans,
  correlated logs and metric points for the waterfall), `GET /api/dimensions`,
  `GET /api/metrics`, `POST /api/ask`, `GET /api/reports[/{name}]`. Every
  query runs on a fresh read-only connection inside `spawn_blocking`.
- `kronika_metrics::to_sql_bucketed` — compile a metric definition into a
  time-bucketed series query (integer-division buckets on `time_col`).
- Web UI (`web/`): SvelteKit 2 + Svelte 5 static SPA (adapter-static,
  `200.html` fallback) — Overview (KPI row, calendar heatmap, fault donut,
  target leaderboard), Experiments (URL-synced filters), experiment detail
  with the custom span waterfall (ruler, indented tree, status-coloured
  bars, click-through drawer with attributes, events and correlated logs),
  Ask (golden answers without an LLM; graceful setup hint when
  `{configured:false}`), Reports. ECharts tree-shaken to bar/heatmap/pie;
  hand-rolled near-black theme. `package-lock.json` committed.
- `kronikad` rust-embeds `web/build/` and serves the SPA on the HTTP port
  (fingerprinted assets cached immutably; non-API paths fall back to the app
  shell). Dockerfile gains a `node:22-bookworm-slim` web stage so the full
  UI ships in the one-command demo at `http://localhost:14318/`.
- Automatic reporting: `KRONIKA_REPORT_INTERVAL` (e.g. `1h`, off by default)
  makes the daemon render a metric digest per interval into
  `<db dir>/reports/report_<epoch>.html`; `/api/reports` lists them and the
  demo compose enables it with `1h`.
- `/api/ask` NL→SQL: curated golden question bank, LLM fallback through
  `kronika-ai`'s OpenAI-compatible client with the sql_guard pipeline
  (allow-listed tables, single SELECT, injected `LIMIT 500`), 30s LLM and
  15s query wall-clock bounds.

---

## [0.1.0] — 2026-07-28

### Added
- Scaffold: Rust workspace with six library crates and the `kronikad` daemon.
- `kronika-store`: embedded DuckDB store (bundled), schema v1 with wide
  ClickHouse-exporter-aligned tables (`spans`, `logs`, `metric_sums`,
  `metric_gauges`, `metric_histograms`, `import_batches`), `MAP` attribute
  columns, `experiment_runs` rollup view, single-writer + read-only reader
  model with `StoreLocked` mapping (tumult-analytics pattern, 0o700 dir).
- `kronika-otel`: pure OTLP → row translation promoting `resilience.*` and
  `service.name` attributes into materialized columns.
- `kronika-ingest`: OTLP/gRPC (`:4317`) and OTLP/HTTP (`:4318`, `/v1/*`,
  `/healthz`) servers funneling through a bounded single-writer channel;
  manual importers for CSV and tumult journal JSON.
- `kronika-metrics`: YAML semantic metric layer compiled to strictly
  identifier-validated SQL; starter definitions in `metrics/`
  (hypothesis_pass_rate, experiment_count, deviation_rate, mttr, coverage,
  action_duration_p95 placeholder).
- `kronika-report`: report model, self-contained HTML renderer, tokio
  interval scheduler.
- `kronika-ai`: Phase 1 groundwork — `Llm` trait, OpenAI-compatible client
  (Ollama default), SQL guardrail pipeline.
- `kronikad`: `serve` (default), `import <file>`, `report --metric <name>`.
- `web/` SvelteKit skeleton (hand-written; not installed), `docs/` (research,
  architecture, ADRs 0001–0002), optional otel-collector dev compose.
- Docker demo: the pinned tumult v2.18.0 release binary (fetched from GitHub
  releases, checksum-verified against `SHA256SUMS.txt` at image build) runs
  the real experiment suite in `demo/experiments/` — eight `.toon`
  experiments (six pass, one deviates, one fails; both rolled back) emitting
  genuine OTLP/gRPC into kronikad; HTML reports land in `demo-out/`. The
  synthetic `kronika-demo` generator remains as optional backfill behind
  `--profile synthetic`.
- `kronikad`: live `GET /report?metric=<name>` endpoint (DuckDB is
  single-process read-write, so reports against a running daemon must be
  served by the daemon); `report --out <file>`.
- `kronika-metrics`: rate terms accept AND-lists of equality conditions.
- `kronika-report`: dimensioned metrics render real breakdown tables; the
  headline KPI uses an ungrouped query.
- `kronika-otel`/`kronika-store`: promote the keys tumult actually emits
  (`resilience.experiment.title`, `resilience.plugin.name`) alongside the
  metadata-standard names; `metric_histograms` gains promoted-dim columns
  (idempotent `ALTER` for existing stores).

### Changed
- Semantic metrics retargeted at tumult's real wire emission:
  `hypothesis_pass_rate` and `deviation_rate` compute over the
  `tumult.experiments.total` / `tumult.hypothesis.deviations.total`
  counters; new `experiment_duration_s`, `experiment_coverage` and
  `action_duration_s`; the span-based `mttr`, `coverage` and
  `action_duration_p95` definitions remain for the synthetic profile.

### Roadmap
- Web UI data plumbing + span-waterfall component.
- Report delivery (email/webhook) and static chart rendering.
- Mosaic crossfiltering (Phase 2, pinned + wrapped), Perspective widget.
- AI phases: NL query → narrative digests → anomaly explanation → insights.
- Parquet lake export partitioned by date.
