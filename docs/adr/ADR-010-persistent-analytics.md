---
title: "ADR-010: Persistent Analytics"
parent: Architecture Decisions
nav_order: 10
---

# ADR-010: Persistent Analytics Store Architecture

## Status

Accepted

## Context

Phase 2 delivered in-memory DuckDB analytics — every `tumult analyze` invocation loaded journals from disk, processed them, and discarded the store. This works for ad-hoc queries but prevents cross-run trend analysis, automatic anomaly detection, and retention management.

Phase 4 requires a persistent store that accumulates experiment history automatically, enables SQL queries across all historical runs, and provides backup/restore capabilities.

## Decision

### Persistent Store Location

Store at `~/.tumult/analytics.duckdb`. The directory is created automatically on first use. DuckDB's file-backed mode uses WAL (Write-Ahead Logging) by default for crash safety.

### Incremental Ingestion

Every journal is deduplicated by `experiment_id` via a unique index on the `experiments` table. Ingesting a journal that already exists is a no-op (returns false, counted as skipped). This makes ingestion idempotent — safe to run multiple times.

### Auto-Ingest on Run

`tumult run` automatically ingests the journal into the persistent store after writing it to disk. This is the default behavior. Users can disable it with `--no-ingest` for testing or offline scenarios.

### Schema Versioning

A `schema_meta` table tracks the schema version (currently v1). On store open, the version is checked and future migrations can be applied automatically. This prevents manual migration steps as the schema evolves.

### Backup and Restore

- `tumult store backup` exports both tables to Parquet files (experiments.parquet, activities.parquet) using ZSTD compression
- `tumult import <dir>` restores from a Parquet backup into the persistent store
- Round-trip integrity is verified by TDD tests

### Retention Policy

`tumult store purge --older-than-days N` removes experiments (and their activity results) older than N days. This prevents unbounded growth of the store.

### Analyze Without Path

`tumult analyze` without a journals path queries the persistent store directly. This is the most common use case for cross-run analysis — no need to specify a directory.

## Alternatives Considered

1. **SQLite** — Mature but lacks columnar storage and Arrow integration. DuckDB provides native Arrow appender and analytical query performance.
2. **External database (PostgreSQL)** — Adds infrastructure dependency. Tumult's value proposition is zero-dependency embedded analytics.
3. **Flat-file Parquet accumulation** — Simple but no SQL query support without loading into memory first.

## Consequences

- Users get automatic experiment history accumulation
- `tumult analyze` works without specifying journal paths
- Cross-run trends are computed from full history
- Store grows over time — retention policy is essential for long-running deployments
- Backup/restore via Parquet provides vendor-neutral data portability
