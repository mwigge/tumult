# ADR-008: Arrow + DuckDB as Embedded Analytics Engine

## Status

Accepted

## Context

Tumult produces structured journal data from every experiment. Users need SQL analytics for trend analysis, compliance reporting, and operational insights — without requiring external database infrastructure.

## Decision

Use **Apache Arrow** as the in-memory columnar format and **DuckDB** as the embedded SQL engine.

### Data Pipeline

```
Journal (.toon) → Arrow RecordBatch → DuckDB (SQL) → Parquet/CSV (export)
```

### Key Choices

1. **Arrow RecordBatch** as the interchange format. Zero-copy ingestion via DuckDB's `appender-arrow` feature.
2. **DuckDB embedded** (bundled build) — compiles into the Tumult binary. No server needed.
3. **Parquet export** — compressed columnar files readable by any data tool.
4. **In-memory by default** — Phase 4 adds persistent storage at `~/.tumult/analytics.duckdb`.

### Schema

Two tables: `experiments` (one row per journal) and `activity_results` (one row per activity execution with phase tracking).

## Consequences

- Binary size increases (~15-20MB for DuckDB bundled + Arrow)
- Users get SQL analytics without external dependencies
- Parquet export enables integration with any data ecosystem
