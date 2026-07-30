# Architecture — Krönika

Krönika ingests chaos/resilience telemetry (OTLP + manual files), stores it
in embedded DuckDB, and serves a presentation-first UI plus scheduled
reports. Companion to [tumult](https://github.com/mwigge/tumult) (chaos
execution) and smedja.

## Component diagram

```
                        ┌────────────────────────────────────────────┐
                        │                 kronikad                   │
                        │                                            │
 tumult ──OTLP/gRPC──▶  │  kronika-ingest (grpc :4317)               │
 (TUMULT_OTEL_ENABLED, │  kronika-ingest (http :4318, /v1/*,         │
  OTEL_EXPORTER_       │                          /healthz)         │
  OTLP_ENDPOINT)       │        │ decode prost (opentelemetry-proto) │
                       │        ▼                                    │
 smedja ──OTLP/HTTP──▶ │  kronika-otel — pure translation:           │
 (SMEDJA_OTLP_         │  promote resilience.* + service.name to      │
  ENDPOINT, /v1/*)     │  columns, rest into MAP attrs                │
                       │        │ row batches                         │
 files (CSV / tumult   │  ManualImporter ─────────────┐              │
  journal JSON) ──────▶│  (kronikad import <file>)    │              │
                       │        ▼                     ▼              │
                        │  single bounded mpsc channel (backpressure) │
                        │    ▲ Batch::Exec — manual-evidence lifecycle│
                        │    │ closures from kronika-api ride the same│
                        │    │ channel (the API opens no write conn)  │
                        │        ▼                                    │
                        │  kronika-store WRITER (single DuckDB conn)  │
                        └────────┬───────────────────────────────────┘
                                 │ exclusive write lock
                                 ▼
                    ┌─────────────────────────┐
                    │  ~/.kronika/kronika.duckdb  (dir mode 0o700)  │
                    │  spans · logs · metric_sums/gauges/histograms │
                    │  import_batches · view: experiment_runs       │
                    │  manual_experiments · manual_experiment_audit │
                    │  evidence_attachments (schema v2)             │
                    └────────┬─────────────────┘
              read-only conns (AccessMode::ReadOnly, many, coexist)
              ┌──────────────┼───────────────────────────┐
              ▼              ▼                           ▼
      kronika-metrics   kronika-report            kronika-api
      YAML semantic     digest renderer           read-only JSON API
      layer → SQL       (ad-hoc + scheduled)      (/api/*, spawn_blocking)
              │              │                    │  overview · series ·
              │   kronika-docs                    │  experiments · logs ·
              │   compliance reports              │  traces · metrics ·
              │   (R1/R2/R3 → PDF +               ▼  topology · ask  ▲
              │    HTML, scores)              web/ SPA (rust-embed, same
              │         │                     HTTP port) ──┘
              ▼         ▼                        ▲  POST /api/ask ──▶ kronika-ai
         <db dir>/reports/  (+ reports/v2/       │   Llm → sql_guard → read-only
         report_<epoch>.html  KRK-*.pdf/.html/.json)  execution (guarded, LIMIT)
         (store closed)                          │
```

## Data flow

1. **OTLP in** — gRPC (tonic) and HTTP (axum, `application/x-protobuf`)
   servers receive `Export{Trace,Metrics,Logs}ServiceRequest`s.
2. **Normalize** — `kronika-otel` converts proto values to plain row structs,
   promoting the tumult metadata standard's (`resilience.*` v2.0)
   low-cardinality keys and `service.name`/`service.version` into wide-table
   columns; dynamic keys stay in `MAP(VARCHAR, VARCHAR)` columns.
3. **Store** — batches travel a bounded tokio mpsc channel (batching +
   backpressure) to the single writer connection.
4. **Semantic layer** — `metrics/*.yaml` definitions compile to strictly
   validated SQL (`[a-z0-9_.]` identifiers only → injection-impossible).
5. **UI / reports** — `kronika-api` answers the UI's queries through fresh
   read-only connections (never touching the write lock); the SPA itself is
   rust-embedded into kronikad and served from the same HTTP port. Beyond
   Overview/Experiments it backs the explorer pages: `/api/logs[ /volume]`
   (raw log search + severity volume), `/api/traces[ /durations, /{id}]`
   (spans grouped into traces, duration percentiles, per-trace detail),
   `/api/metrics/catalog` + `/api/metrics/query` (raw sums/gauges/histograms
   with optional attribute grouping; histogram p95 interpolated in Rust) and
   `/api/topology` (service/target call graph). Ad-hoc
   digests come from `kronikad report` / `GET /report`; with
   `KRONIKA_REPORT_INTERVAL` set, the daemon additionally renders a digest
   per interval into `<db dir>/reports/` (surfaced by `/api/reports`). When
   an LLM is reachable, digests (scheduled and `POST /api/reports/generate`)
   gain a narrative section via `kronika_report::narrative`, which keeps
   only sentences whose numbers are grounded in the report's own facts.

The docker demo (`docker/docker-compose.demo.yml`) is the **reference
ingestion flow** end to end: the pinned tumult release binary runs the
experiment suite in `demo/experiments/` and emits genuine OTLP/gRPC
(traces, metrics, logs) into kronikad, which normalizes, stores and renders
it into the HTML reports under `demo-out/`. Whatever tumult emits on the
wire is exactly what Krönika's semantic layer computes over.

## Single-writer model (mirrors tumult-lake)

DuckDB is single-writer per file; a read-write open holds an exclusive lock.

- **One writer per process** — every ingest path funnels through the channel
  onto one `Writer`. A second read-write open maps the opaque DuckDB lock
  error to `StoreError::StoreLocked` (after a short bounded retry).
- **Many readers** — `AccessMode::ReadOnly` connections do not take the write
  lock, so reports and the UI API coexist with the writer *inside the daemon
  process*. Cross-process, DuckDB permits only one process with the file open
  read-write: `kronikad report` therefore requires the daemon to be stopped,
  and the live `GET /report?metric=<name>` endpoint exists precisely so
  reports can be produced while the daemon holds the store.
- **At rest** — DuckDB has no encryption at rest; the store directory is
  created `0o700` and should sit on an encrypted volume for sensitive data.

## Parquet lake + retention (durability story)

Two tiers, one guarantee: **nothing leaves the hot store before an
immutable copy exists in the lake.**

- **Hot tier** — the embedded DuckDB store: ACID, single-writer, WAL-backed
  (crash-safe to the last committed batch). Optimised for the recent-query
  workload of the UI and reports.
- **Cold tier** — the parquet lake (`KRONIKA_LAKE_DIR`, default
  `<db dir>/lake`): per table, one write-once file per day-partition
  (`spans/date=2026-07-29/data-<run>.parquet`). Files are never rewritten —
  *immutability as a compliance feature*: next to the v0.5.0 hash-chained
  manual-evidence audit, the trail of what Krönika recorded is
  WORM-shaped and tamper-evident, and readable by any parquet-capable
  tool (`read_parquet('lake/spans/date=*/*.parquet')`).
- **Export** — incremental against a per-table event-time watermark in
  `<lake>/_meta.json` (tmp+rename, advanced only after every table
  succeeded → idempotent retries). `manual_experiments` exports as a full
  snapshot per run (records mutate through their review lifecycle;
  fingerprint-gated so an unchanged register writes no new file); its
  audit table exports incrementally on `changed_at_ns`. Runs on
  `KRONIKA_LAKE_INTERVAL` (default `24h`) or on demand via
  `POST /api/lake/export`; `GET /api/lake/status` shows watermarks, files
  and bytes.
- **Retention** — `KRONIKA_RETENTION_DAYS=0` (default) keeps everything.
  When >0, hot rows older than the cutoff are deleted **only if already
  exported** (`ts_ns <= watermark`), through the single-writer channel.
  `manual_experiment_audit` and `manual_experiments` are never deleted:
  append-only compliance evidence in both tiers.

Caveat (event-time watermarking): rows arriving with `ts_ns` at or below
the current watermark are invisible to incremental export — irrelevant for
real-time telemetry; re-export from scratch after hand-backfills. See
ADR 0005.

## Schema v2

Wide, ClickHouse-exporter-aligned tables: `spans`, `logs`, `metric_sums`,
`metric_gauges`, `metric_histograms` (+ `import_batches` for manual imports,
`schema_meta` versioning). Rollup view `experiment_runs` projects one row per
`resilience.experiment` span.

v2 adds manual evidence: `manual_experiments` (content + draft → submitted →
verified/rejected lifecycle + provenance + `content_hash`),
`manual_experiment_audit` (append-only, `prev_hash → new_hash` hash chain),
and `evidence_attachments` (external URIs only). The org hierarchy is not a
table — it is `org.yaml` (`KRONIKA_ORG_FILE`, default `<db dir>/org.yaml`)
loaded at daemon start; org rollups are computed at read time from the
latest-run scoring SQL (see ADR 0004).

## Roadmap: external tooling on the lake

The parquet lake (above) is the substrate for future external tooling —
any parquet-capable engine can query Krönika's history without touching
the hot store.
