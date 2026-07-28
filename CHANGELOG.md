# Changelog

All notable changes to kronika are documented here.

Format: `## [version] — YYYY-MM-DD` / `### Added|Fixed|Changed|Removed|Roadmap`.

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
