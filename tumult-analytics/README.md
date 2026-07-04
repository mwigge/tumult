# tumult-analytics

Embedded analytics for Tumult -- DuckDB, Arrow, and Parquet support for querying experiment journals with SQL.

## Key Types

- `AnalyticsEngine` -- DuckDB-backed query engine
- `JournalIngester` -- converts TOON journals to Arrow RecordBatches
- `PersistentStore` -- manages the local DuckDB store at `~/.tumult/analytics.duckdb`

## Usage

```rust
use tumult_analytics::AnalyticsEngine;

let engine = AnalyticsEngine::open_default()?;
let results = engine.query("SELECT status, count(*) FROM experiments GROUP BY status")?;
```

## Concurrency: single-writer store

`DuckDB` is **single-writer per file**. A read-write connection takes an
exclusive lock on `~/.tumult/analytics.duckdb`, so only one process can hold the
store open read-write at a time. Tumult opens this store from two places — the
CLI (`tumult run` ingest, `tumult analyze`, `tumult chaosgraph`, …) and the
long-running MCP server — so the store API distinguishes reads from writes:

- **Reads open read-only** via `AnalyticsStore::open_read_only(path)`
  (`access_mode = READ_ONLY`). Read-only opens do **not** take the exclusive
  write lock, so multiple read-only processes coexist — the CLI can query the
  store while the MCP server also holds it open. Use this for every read path.
- **Writes open read-write** via `AnalyticsStore::open(path)`, which also
  initialises/migrates the schema. This is the ingest path.
- **Two writers conflict.** A read-write open is exclusive: while it is held it
  blocks every other opener (readers included). If an open fails because another
  process holds the store — most often the MCP server mid-ingest — the opaque
  DuckDB lock error is mapped to `AnalyticsError::StoreLocked`, whose message
  tells you to stop the MCP server or point the command at a separate `--store`
  path. A short bounded retry first absorbs the brief window while the other
  process finishes a write.

```rust
use tumult_analytics::AnalyticsStore;

// Read path — coexists with the running MCP server.
let store = AnalyticsStore::open_read_only(&AnalyticsStore::default_path())?;
let rows = store.query("SELECT status, count(*) FROM experiments GROUP BY status")?;

// Write path — exclusive; may return AnalyticsError::StoreLocked.
let store = AnalyticsStore::open(&AnalyticsStore::default_path())?;
```

## More Information

See the [main README](../README.md) for project overview and setup.
