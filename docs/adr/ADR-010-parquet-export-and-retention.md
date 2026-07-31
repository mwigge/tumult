---
title: "ADR-010: Parquet Export and Retention"
parent: Architecture Decisions
nav_order: 10
---

# ADR-010: Parquet Export and Retention (imported from kronika ADR 0005)

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
`manual_experiment_audit`, `manual_experiments`, and — since the analytics
fold — the journal-detail tables `experiments`, `activity_results`,
`load_results`, the autopilot history `autopilot_decisions`,
`autopilot_events`, `autopilot_change_events`, the `ChaosGraph` tables
`graph_nodes`/`graph_edges`, and the four `agentic_*` tables.

### Incremental export with a persistent watermark

- Telemetry, audit and journal-detail tables export only rows newer than
  the table's watermark (`ts_ns`, `changed_at_ns`, resp. `started_at_ns`);
  the watermark then advances to the table's max event time.
- The watermark lives in `<lake>/_meta.json`, written tmp+rename after
  *every* table exported successfully — a failed or crashed run retries
  from the last good watermark, making re-runs idempotent (no new rows,
  no new files).
- Mutable and event-sourced tables are the exception: each run writes a
  **full snapshot** (one file, latest wins) instead of pretending
  event-time incrementality applies to mutable rows. This covers
  `manual_experiments` (records mutate through the draft → verified
  lifecycle), the INSERT-ONLY `autopilot_*` history, `graph_nodes` /
  `graph_edges` (topology refreshes rewrite edges with `ts = 0`, so a
  watermark would lose updates) and the timestamp-less `agentic_*` tables.
  Snapshots are fingerprint-gated: a run compares a content fingerprint
  (md5 over the register's `content_hash` values for `manual_experiments`;
  md5 over ordered per-row hashes of the full row JSON elsewhere) against
  the one recorded per table in `_meta.json` and skips the write when the
  table has not changed.

### Retention gated on the watermark

`KRONIKA_RETENTION_DAYS=0` (default) keeps everything forever. When >0,
the job deletes hot rows older than the cutoff **and at or below the
table's watermark** — rows above the watermark are provably unexported and
are never touched. The journal-detail tables follow the same watermark
guard. Snapshot-exported tables have no trustworthy watermark, so the
`autopilot_*` tables are purged only while their current fingerprint
matches the last exported one (fingerprint equality proves every hot row
is already in the lake; a single unexported row disables the purge).
`manual_experiment_audit`, `manual_experiments`, `graph_*` and `agentic_*`
are never deleted: append-only compliance evidence, or mutable /
timestamp-less rows no watermark can protect.

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
