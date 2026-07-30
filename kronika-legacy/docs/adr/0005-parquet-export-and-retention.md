# ADR 0005: Parquet lake export and retention

- Status: accepted (live in v0.6.0)
- Date: 2026-07-29

## Context

Krönika's embedded DuckDB store has no lifecycle story: it grows forever,
and there is no durable, tool-agnostic copy of the telemetry outside the
proprietary database file. The OpenObserve gap analysis
(`docs/research-openobserve-gap.md`) identified retention and a columnar
lake export as the two highest-priority gaps, with "immutability as a
compliance feature" as the framing that ties them to Krönika's existing
hash-chained manual-evidence audit.

**License boundary.** OpenObserve is AGPL-3.0; Krönika is Apache-2.0.
This design borrows *ideas* (hot store + immutable columnar cold tier,
write-once files as a compliance property, documented durability
guarantees) and is a clean-room implementation against DuckDB's documented
behaviour. No OpenObserve code was read, copied, ported or translated.

## Decision

### Layout: one parquet file per table per day-partition

Per table, the exporter writes

```
<lake>/<table>/date=<YYYY-MM-DD>/data-<run_ns>.parquet
```

via `COPY (SELECT …) TO '…' (FORMAT PARQUET)` — one file per day directory,
a new uniquely-named file per run rather than partitioned-overwrite, so
lake files are write-once and never mutated in place. Tables: `spans`,
`logs`, `metric_sums`, `metric_gauges`, `metric_histograms`,
`manual_experiment_audit`, and `manual_experiments`.

### Incremental export with a persistent watermark

- Telemetry and audit tables export only rows newer than the table's
  watermark (`ts_ns`, resp. `changed_at_ns`); the watermark then advances
  to the table's max event time.
- The watermark lives in `<lake>/_meta.json`, written tmp+rename after
  *every* table exported successfully — a failed or crashed run retries
  from the last good watermark, making re-runs idempotent (no new rows,
  no new files).
- `manual_experiments` is the exception: records mutate through the
  draft → verified lifecycle, so each run writes a **full snapshot** (one
  file, latest wins) instead of pretending event-time incrementality
  applies to mutable rows. Snapshots are fingerprint-gated: a run compares
  an md5 of all `content_hash` values against the one recorded in
  `_meta.json` and skips the write when the register has not changed, so
  an unchanged register produces no new file.

### Retention gated on the watermark

`KRONIKA_RETENTION_DAYS=0` (default) keeps everything forever. When >0,
the job deletes hot rows older than the cutoff **and at or below the
table's watermark** — rows above the watermark are provably unexported and
are never touched. `manual_experiment_audit` and `manual_experiments` are
never deleted: append-only compliance evidence in hot and cold tiers
alike.

### Scheduling and triggering

The daemon runs the job on `KRONIKA_LAKE_INTERVAL` (default `24h`,
`0`/`off` disables), like the report scheduler: export on a *fresh*
read-only connection (a long-lived read-only DuckDB connection pins its
snapshot and would not see rows committed after it opened), then retention
through the single-writer channel (`Batch::Exec`), never a second writer.
`POST /api/lake/export` triggers the same code path on demand;
`GET /api/lake/status` reports watermarks, file and byte totals, and the
configured policy.

## Consequences

- **Durability story** (documented in `docs/architecture.md`): DuckDB ACID
  hot store for the recent-query tier + immutable write-once parquet files
  for the cold tier + the v0.5.0 hash-chained audit = a WORM-shaped,
  tamper-evident evidence trail. This is "immutability as a compliance
  feature", not a storage limitation.
- **Event-time watermark caveat**: rows arriving with `ts_ns` at or below
  the current watermark are invisible to incremental export (irrelevant
  for real-time tumult/smedja telemetry; matters only for hand-backfilled
  data — re-export from scratch for those). Retention's `ts_ns <=
  watermark` clause can reclaim such late arrivals unexported; accepted
  and documented rather than adding an ingest-time column.
- The lake is readable by any parquet-capable tool
  (`read_parquet('<lake>/spans/date=*/*.parquet')` in DuckDB, polars,
  pandas, …) — the future external-tooling substrate.
- The demo compose sets `KRONIKA_RETENTION_DAYS=0` explicitly: the demo
  accumulates, never deletes.
