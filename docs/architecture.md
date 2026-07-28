# Architecture — kronika

Kronika ingests chaos/resilience telemetry (OTLP + manual files), stores it
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
                        │        ▼                                    │
                        │  kronika-store WRITER (single DuckDB conn)  │
                        └────────┬───────────────────────────────────┘
                                 │ exclusive write lock
                                 ▼
                    ┌─────────────────────────┐
                    │  ~/.kronika/kronika.duckdb  (dir mode 0o700)  │
                    │  spans · logs · metric_sums/gauges/histograms │
                    │  import_batches · view: experiment_runs       │
                    └────────┬─────────────────┘
              read-only conns (AccessMode::ReadOnly, many, coexist)
              ┌──────────────┼───────────────────────────┐
              ▼              ▼                           ▼
      kronika-metrics   kronika-report            kronika-api
      YAML semantic     digest renderer           read-only JSON API
      layer → SQL       (ad-hoc + scheduled)      (/api/*, spawn_blocking)
              │              │                    │  overview · series ·
              │              ▼                    │  experiments · logs ·
              │         <db dir>/reports/         │  traces · metrics ·
              │         report_<epoch>.html       ▼  topology · ask  ▲
              │              │                web/ SPA (rust-embed, same
              │              │                HTTP port) ──┘
              ▼              ▼                   ▲  POST /api/ask ──▶ kronika-ai
         kronikad       UI Reports page          │   Llm → sql_guard → read-only
         report CLI     (/api/reports)           │   execution (guarded, LIMIT)
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
wire is exactly what kronika's semantic layer computes over.

## Single-writer model (mirrors tumult-analytics)

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

## Schema v1

Wide, ClickHouse-exporter-aligned tables: `spans`, `logs`, `metric_sums`,
`metric_gauges`, `metric_histograms` (+ `import_batches` for manual imports,
`schema_meta` versioning). Rollup view `experiment_runs` projects one row per
`resilience.experiment` span.

## Roadmap: lake export

Nightly/scheduled export of the wide tables to a Parquet lake:

```sql
COPY spans TO 'lake/spans' (FORMAT PARQUET, PARTITION_BY (date));
```

with `date` derived from `ts_ns`. The DuckDB store stays the hot tier; the
Parquet lake is the durable, shareable cold tier (and the future Mosaic /
external-tooling substrate).
