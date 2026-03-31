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

## More Information

See the [main README](../README.md) for project overview and setup.
