# tumult-clickhouse

ClickHouse analytics backend for Tumult -- enables shared storage with SigNoz for cross-tool observability.

## Key Types

- `ClickHouseClient` -- connection and query interface
- `ClickHouseIngester` -- writes experiment data to ClickHouse tables

## Usage

```rust
use tumult_clickhouse::ClickHouseClient;

let client = ClickHouseClient::new("http://localhost:8123")?;
client.ingest_journal(&journal).await?;
```

## More Information

See the [main README](../README.md) for project overview and setup.
