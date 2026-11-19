# tumult-clickhouse

ClickHouse analytics backend for Tumult -- enables shared storage with SigNoz for cross-tool observability.

## Key Types

- `ClickHouseStore` -- connection and query interface, and writes experiment data to ClickHouse tables
- `ClickHouseConfig` -- the URL, database and user, read from `TUMULT_CLICKHOUSE_*`

## Usage

```rust
use tumult_clickhouse::{ClickHouseConfig, ClickHouseStore};

let store = ClickHouseStore::connect(&ClickHouseConfig::from_env()).await?;
store.ingest_journal_async(&journal).await?;
```

## More Information

See the [main README](../README.md) for project overview and setup.
