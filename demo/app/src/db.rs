//! Order storage: an [`OrderStore`] trait (so handlers can be tested against
//! a stub) and the Postgres implementation backed by a deadpool connection
//! pool. All pool/connect operations carry short timeouts so `/health` stays
//! responsive while the database is down or being fault-injected.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use rand::Rng;
use serde::Serialize;
use tokio_postgres::{NoTls, Row};

/// A single row of the demo `orders` table.
#[derive(Debug, Clone, Serialize)]
pub struct Order {
    pub id: i32,
    pub name: String,
    pub amount_cents: i64,
    pub created_at: String,
}

/// Opaque storage error; the API layer maps this to a JSON 500.
#[derive(Debug)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

fn store_err<E: fmt::Display>(context: &str) -> impl Fn(E) -> StoreError + '_ {
    move |e| StoreError(format!("{context}: {e}"))
}

/// Mark the current tracing span as errored so the exported `OTel` span carries
/// an error status (turns the span red in `SigNoz`). No-op if the current span
/// did not declare the `otel.status_code` / `otel.status_message` fields.
pub fn record_span_error(message: &str) {
    let span = tracing::Span::current();
    span.record("otel.status_code", "ERROR");
    span.record("otel.status_message", message);
}

#[async_trait]
pub trait OrderStore: Send + Sync {
    /// Cheap connectivity check (`SELECT 1`).
    async fn ping(&self) -> Result<(), StoreError>;
    async fn list_orders(&self) -> Result<Vec<Order>, StoreError>;
    async fn get_order(&self, id: i32) -> Result<Option<Order>, StoreError>;
    async fn insert_order(&self, name: &str, amount_cents: i64) -> Result<Order, StoreError>;
}

const ADJECTIVES: [&str; 10] = [
    "amber", "brisk", "cobalt", "dusty", "ember", "frosted", "gilded", "hollow", "ivory", "jade",
];
const ITEMS: [&str; 10] = [
    "widget", "sprocket", "gizmo", "flange", "gasket", "dynamo", "gearbox", "valve", "coupler",
    "rotor",
];

/// Random `(name, amount_cents)` for POST /orders and seed data.
pub fn random_order() -> (String, i64) {
    let mut rng = rand::rng();
    let adjective = ADJECTIVES[rng.random_range(0..ADJECTIVES.len())];
    let item = ITEMS[rng.random_range(0..ITEMS.len())];
    let amount_cents = rng.random_range(199..=99_999);
    (format!("{adjective} {item}"), amount_cents)
}

/// Postgres-backed store.
pub struct PgStore {
    pool: Pool,
}

const ORDER_COLUMNS: &str = "id, name, amount_cents, created_at::text";

fn row_to_order(row: &Row) -> Order {
    Order {
        id: row.get(0),
        name: row.get(1),
        amount_cents: row.get(2),
        created_at: row.get(3),
    }
}

impl PgStore {
    /// Build a lazy connection pool. This does not touch the network, so it
    /// succeeds even while the database is still starting.
    pub fn connect(database_url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut pg_config = tokio_postgres::Config::from_str(database_url)?;
        pg_config.connect_timeout(Duration::from_secs(2));

        let manager = Manager::from_config(
            pg_config,
            NoTls,
            ManagerConfig {
                recycling_method: RecyclingMethod::Fast,
            },
        );
        let pool = Pool::builder(manager)
            .max_size(8)
            .create_timeout(Some(Duration::from_secs(2)))
            .wait_timeout(Some(Duration::from_secs(2)))
            .recycle_timeout(Some(Duration::from_secs(2)))
            .runtime(Runtime::Tokio1)
            .build()?;
        Ok(Self { pool })
    }

    async fn client(&self) -> Result<deadpool_postgres::Object, StoreError> {
        self.pool.get().await.map_err(store_err("pool checkout"))
    }

    /// Create the `orders` table and seed ~20 rows if empty. Idempotent —
    /// safe to retry until the database comes up.
    #[tracing::instrument(name = "db.bootstrap", skip(self))]
    pub async fn bootstrap(&self) -> Result<(), StoreError> {
        let client = self.client().await?;
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS orders (
                    id           SERIAL PRIMARY KEY,
                    name         TEXT NOT NULL,
                    amount_cents BIGINT NOT NULL,
                    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
                )",
            )
            .await
            .map_err(store_err("create table"))?;

        let count: i64 = client
            .query_one("SELECT count(*) FROM orders", &[])
            .await
            .map_err(store_err("count orders"))?
            .get(0);

        if count == 0 {
            let statement = client
                .prepare("INSERT INTO orders (name, amount_cents) VALUES ($1, $2)")
                .await
                .map_err(store_err("prepare seed insert"))?;
            for _ in 0..20 {
                let (name, amount_cents) = random_order();
                client
                    .execute(&statement, &[&name, &amount_cents])
                    .await
                    .map_err(store_err("seed insert"))?;
            }
            tracing::info!("seeded orders table with 20 rows");
        }
        Ok(())
    }
}

#[async_trait]
impl OrderStore for PgStore {
    #[tracing::instrument(name = "db.ping", skip(self))]
    async fn ping(&self) -> Result<(), StoreError> {
        let client = self.client().await?;
        client
            .query_one("SELECT 1", &[])
            .await
            .map_err(store_err("ping"))?;
        Ok(())
    }

    #[tracing::instrument(name = "db.list_orders", skip(self))]
    async fn list_orders(&self) -> Result<Vec<Order>, StoreError> {
        let client = self.client().await?;
        let rows = client
            .query(
                &format!("SELECT {ORDER_COLUMNS} FROM orders ORDER BY id DESC LIMIT 50"),
                &[],
            )
            .await
            .map_err(store_err("list orders"))?;
        Ok(rows.iter().map(row_to_order).collect())
    }

    #[tracing::instrument(name = "db.get_order", skip(self))]
    async fn get_order(&self, id: i32) -> Result<Option<Order>, StoreError> {
        let client = self.client().await?;
        let row = client
            .query_opt(
                &format!("SELECT {ORDER_COLUMNS} FROM orders WHERE id = $1"),
                &[&id],
            )
            .await
            .map_err(store_err("get order"))?;
        Ok(row.as_ref().map(row_to_order))
    }

    #[tracing::instrument(name = "db.insert_order", skip(self))]
    async fn insert_order(&self, name: &str, amount_cents: i64) -> Result<Order, StoreError> {
        let client = self.client().await?;
        let row = client
            .query_one(
                &format!(
                    "INSERT INTO orders (name, amount_cents) VALUES ($1, $2) \
                     RETURNING {ORDER_COLUMNS}"
                ),
                &[&name, &amount_cents],
            )
            .await
            .map_err(store_err("insert order"))?;
        Ok(row_to_order(&row))
    }
}
