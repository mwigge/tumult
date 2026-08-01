---
title: SigNoz Bulk Import
parent: Guides
nav_order: 24
---

# SigNoz Bulk Import

`scripts/signoz-bulk-import.sh` backfills spans from the Tumult Parquet lake straight into SigNoz's ClickHouse trace tables. It bypasses the OTel collector entirely — useful when you have days of lake history that never went through a collector, or when you want to rehydrate a fresh SigNoz install from the lake.

## What it does

- Reads `<lake>/spans/date=YYYY-MM-DD/data-*.parquet` (the lake's incremental span export).
- Inserts into `signoz_traces.signoz_index_v3` (through its `Distributed` wrapper when present), with full column mapping: timestamps, status, kind, attributes, and the materialized resilience dimensions.
- Populates the companion resource table (`traces_v3_resource`) so the SigNoz UI can resolve services — without it your spans land but no service shows up.
- Keeps a local ledger of imported files so re-runs are safe and incremental: a new lake export produces new files, the script imports just those.

Spans only, for now — the lake's `logs` and `metric_*` tables are out of scope.

## Prerequisites

- A Parquet lake: `tumultd` exports one on `KRONIKA_LAKE_INTERVAL` (default 24h) into `KRONIKA_LAKE_DIR` (default `<store dir>/lake`), or immediately via `POST /api/lake/export`.
- A SigNoz ClickHouse, reachable one of two ways:
  - **Docker mode (recommended):** the script runs `clickhouse-client` inside the SigNoz container and stages each parquet file with `docker cp`. No mounts, no local client install.
  - **Local mode:** a local `clickhouse-client`, and the parquet files readable by the ClickHouse *server* under its `user_files` directory (e.g. symlink the lake into `/var/lib/clickhouse/user_files/`).

## Quickstart

Bring up the observability stack from this repo:

```bash
docker compose -f docker/docker-compose.observability.yml up -d
```

Make sure the lake has span exports. If the daemon is already running, force an export against its API address (`KRONIKA_OTLP_HTTP_ADDR`, default `:4318` — note the compose stack above already binds 4318 for SigNoz, so run tumultd on a different port, e.g. `KRONIKA_OTLP_HTTP_ADDR=127.0.0.1:24318`):

```bash
curl -X POST http://127.0.0.1:24318/api/lake/export
```

Dry-run first — it prints row counts per date partition and what the ledger already covers:

```bash
SIGNOZ_DOCKER_CONTAINER=docker-signoz-1 \
KRONIKA_LAKE_DIR=~/.tumult/lake \
scripts/signoz-bulk-import.sh --dry-run
```

Then import for real:

```bash
SIGNOZ_DOCKER_CONTAINER=docker-signoz-1 \
KRONIKA_LAKE_DIR=~/.tumult/lake \
scripts/signoz-bulk-import.sh
```

Open SigNoz at `http://localhost:3301` → Services → `tumult`, and the backfilled traces are there. Imported spans carry a `tumult.import=signoz-bulk-import` attribute so you can tell them apart from collector-ingested spans.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `SIGNOZ_DOCKER_CONTAINER` | — | Container to `docker exec` into (enables Docker mode) |
| `SIGNOZ_CLICKHOUSE_DSN` | — | `clickhouse://[user[:pass]@]host[:port][/db]`, overrides the discrete vars |
| `CLICKHOUSE_HOST` | `localhost` | ClickHouse native host (local mode) |
| `CLICKHOUSE_PORT` | `9000` | ClickHouse native port |
| `CLICKHOUSE_USER` | `default` | ClickHouse user |
| `CLICKHOUSE_PASSWORD` | — | ClickHouse password |
| `CLICKHOUSE_DB` | `signoz_traces` | Target database |
| `KRONIKA_LAKE_DIR` | `~/.tumult/lake` | Lake root |
| `PARQUET_GLOB` | `<lake>/spans/date=*/*.parquet` | Explicit file glob, overrides the lake layout |
| `PARQUET_SERVER_DIR` | — | Server-side path for `KRONIKA_LAKE_DIR` (local mode, must be under `user_files`) |
| `SIGNOZ_IMPORT_STATE` | `<lake>/.signoz-import-ledger` | Ledger file path |

Flags: `--dry-run` (count, don't insert), `--force` (ignore the ledger — **duplicates rows**).

## Column mapping

| Lake `spans` column | SigNoz (`signoz_index_v3`) | Notes |
|---|---|---|
| `ts_ns` | `timestamp` | epoch ns → `DateTime64(9)` |
| `ts_ns` | `ts_bucket_start` | 30-minute bucket, epoch seconds |
| `trace_id` | `trace_id` | `FixedString(32)`; longer ids are truncated, shorter padded |
| `span_id` / `parent_span_id` | `span_id` / `parent_span_id` | NULL parent → empty string |
| `span_name` | `name` | |
| `span_kind` | `kind` + `kind_string` | Internal=1, Server=2, Client=3, Producer=4, Consumer=5 |
| `duration_ns` | `duration_nano` | |
| `status_code` | `status_code` + `status_code_string` + `has_error` | Unset=0, Ok=1, Error=2; Error sets `has_error` |
| `status_message` | `status_message` | |
| `service_name`, `service_version`, `resource_attrs` | `resources_string` | `service.name` always present (`unknown` fallback) |
| materialized dims (`experiment_id`, `fault_type`, `blast_radius`, …) | `attributes_string` | re-emitted under the same key names |
| `span_attrs` | `attributes_string` | merged with the dims plus the `tumult.import` marker |
| `hypothesis_met` | `attributes_bool` | |
| `recovery_time_s` | `attributes_number` | |
| `events` (JSON array string) | `events` | one raw-JSON element per event |

Synthesized per row: `resource_fingerprint` (`service.name=…;host.name=…;hash=<cityHash64>`) and the `labels` JSON — both also written to `traces_v3_resource` so the index and resource tables join.

## Caveats

- **No dedup on the ClickHouse side.** `signoz_index_v3` is a plain `MergeTree`, not `ReplacingMergeTree` — re-inserting the same file duplicates rows, permanently. The ledger is the only guard; treat `--force` as a footgun.
- **SigNoz TTL still applies.** Both trace tables carry a 15-day TTL (`toIntervalSecond(1296000)`). Spans older than that will be dropped at the next merge — backfilling months of history is pointless unless you raise the TTL first.
- **The collector's staleness drop does not apply.** SigNoz's collector drops spans older than an hour on ingest; this path writes straight to the table, so old spans land fine (subject to the TTL above).
- **Append-only.** The script never deletes or updates; imported rows stay until TTL takes them.
- **Service identity is rebuilt, not copied.** The fingerprint hash is computed by the script (cityHash64 over the resource labels), self-consistently across both tables. It will not match fingerprints the SigNoz collector computed for the same service — the same service may appear twice (once per ingest path).
- **Bypassing the collector means bypassing its pipelines** — no spanmetrics, no tail sampling, no collector-side enrichment for backfilled spans.

## Troubleshooting

**`no parquet files matched`** — the lake hasn't exported yet. Run `POST /api/lake/export` on tumultd, or point `KRONIKA_LAKE_DIR`/`PARQUET_GLOB` at the right place.

**`cannot reach ClickHouse`** — in Docker mode, check the container name (`docker ps`) and that it's running. In local mode, check host/port and that you're using the native protocol port (9000), not HTTP (8123).

**`target tables not found`** — the database you're pointing at isn't SigNoz's. Check `CLICKHOUSE_DB` (default `signoz_traces`) and that the schema migrator has run.

**Rows imported but no service in the UI** — query the resource table:

```sql
SELECT labels, fingerprint FROM signoz_traces.distributed_traces_v3_resource
WHERE labels LIKE '%tumult%';
```

If it's empty, the resource insert failed; if it's populated but fingerprints differ from `signoz_index_v3.resource_fingerprint`, you're on a SigNoz version with a different resource-table layout — the script targets the `labels`/`fingerprint`/`seen_at_ts_bucket_start` schema.

**Local mode: `file()` can't see the parquet** — ClickHouse's `file()` only reads under its `user_files` directory (`/var/lib/clickhouse/user_files/` by default). Symlink or copy the lake there and set `PARQUET_SERVER_DIR` to the server-side path.

**Find or undo a bad import** — everything the script wrote is tagged:

```sql
SELECT count() FROM signoz_traces.distributed_signoz_index_v3
WHERE attributes_string['tumult.import'] = 'signoz-bulk-import';

ALTER TABLE signoz_traces.signoz_index_v3
  DELETE WHERE attributes_string['tumult.import'] = 'signoz-bulk-import';
```
