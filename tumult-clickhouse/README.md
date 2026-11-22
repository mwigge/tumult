# tumult-clickhouse

A ClickHouse store for Tumult journals, shared with SigNoz.

**Deprecated.** Tumult 3.1 removes this crate. When `TUMULT_CLICKHOUSE_URL`
is set, `tumult run` writes to ClickHouse and logs a warning once. Unset it
to write to the lake.

## Key types

| Item | What it does |
|---|---|
| `ClickHouseStore` | Connects, writes journals and queries them |
| `ClickHouseConfig` | The URL, database, user and password, read from `TUMULT_CLICKHOUSE_*` |

## Use it from Rust

```rust
use tumult_clickhouse::{ClickHouseConfig, ClickHouseStore};

let store = ClickHouseStore::connect(&ClickHouseConfig::from_env()).await?;
store.ingest_journal_async(&journal).await?;
```
