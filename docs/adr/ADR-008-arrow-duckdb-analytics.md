# ADR-008: Arrow + DuckDB as Embedded Analytics Engine

## Status

Accepted

## Context

Tumult produces structured journal data from every experiment run. Users need to query this data for trend analysis, compliance reporting, and operational insights. We need an analytics solution that:

1. Works offline (no external database server)
2. Ships inside the single binary
3. Handles columnar analytics efficiently
4. Exports to standard formats (Parquet, CSV)

## Decision

Use **Apache Arrow** as the in-memory columnar format and **DuckDB** as the embedded SQL engine.

### Data Pipeline

```
Journal (.toon) → Arrow RecordBatch → DuckDB (SQL) → Parquet/CSV (export)
```

### Key Choices

1. **Arrow RecordBatch** as the interchange format between TOON journals and DuckDB. Zero-copy ingestion via DuckDB's `appender-arrow` feature.

2. **DuckDB embedded** (bundled build) — compiles into the Tumult binary. No server, no installation, no configuration. OLAP-optimized for analytical queries.

3. **Parquet export** via `arrow-rs` parquet crate — compressed columnar files readable by any data tool (Spark, Polars, pandas, DuckDB CLI).

4. **In-memory by default** — Phase 2 loads journals into in-memory DuckDB per invocation. Phase 4 adds persistent storage at `~/.tumult/analytics.duckdb`.

### Schema Design

Two tables:
- `experiments` — one row per journal (status, duration, scores)
- `activity_results` — one row per activity (phase, timing, output)

This denormalized schema enables fast aggregation queries without joins.

## Consequences

- Binary size increases by ~15-20MB (DuckDB bundled + Arrow)
- Compilation time increases significantly (DuckDB C++ build)
- Users get SQL analytics without any external dependencies
- Parquet export enables integration with any data ecosystem
- Future: persistent store enables cross-run trend analysis
