---
title: "ADR-005: Analytics"
parent: Architecture Decisions
nav_order: 5
---

# ADR-005: Embedded Analytics with Arrow, DuckDB, and Persistent Storage

## Status

Accepted

## Context

Tumult produces structured journal data from every experiment. Users need SQL analytics for trend analysis, compliance reporting, and operational insights — without requiring external database infrastructure. Analytics must work both ad-hoc (single query) and persistently (accumulated history across all runs).

## Decision

### Data Pipeline

```
Journal (.toon) → Arrow RecordBatch → DuckDB (SQL) → Parquet/CSV (export)
```

### Apache Arrow as Interchange Format

Arrow RecordBatch is the in-memory columnar representation. Two schemas:
- **experiments** — one row per journal (12 columns: id, title, status, timestamps, counts, hypothesis results, analysis metrics)
- **activity_results** — one row per activity execution (9 columns including phase label)

Zero-copy ingestion via DuckDB's `appender-arrow` feature.

### DuckDB as Embedded SQL Engine

DuckDB (bundled build) compiles into the Tumult binary. No server installation, no connection management, no ports.

- **In-memory mode** for ad-hoc queries (`tumult analyze journals/`)
- **Persistent mode** at `~/.tumult/analytics.duckdb` for accumulated history

### Persistent Store

Every `tumult run` automatically ingests the journal into the persistent store (disable with `--no-ingest`). Key properties:

- **Incremental ingestion** — dedup by `experiment_id` via unique index
- **WAL mode** — crash safety for file-backed DuckDB (default behavior)
- **Schema versioning** — `schema_meta` table tracks version for future migrations
- **Retention** — `tumult store purge --older-than-days N` prevents unbounded growth

### Backup and Restore

- `tumult store backup` exports both tables to ZSTD-compressed Parquet files
- `tumult import <dir>` restores from Parquet backup
- Round-trip integrity verified by tests

### Export Formats

| Format | Compression | Use Case |
|--------|-------------|----------|
| Parquet | ZSTD | Long-term storage, BI tools (Spark, Polars, pandas) |
| Arrow IPC | None | Language interop, streaming |
| CSV | None | Spreadsheets, simple downstream |

## Consequences

- Binary size increases ~15-20MB for DuckDB bundled + Arrow
- Users get SQL analytics without external dependencies
- Persistent store grows over time — retention policy is essential
- Parquet export enables integration with any data ecosystem
- `tumult analyze` without a path queries accumulated history directly
