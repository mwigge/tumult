//! HTTP API: router, handlers, and JSON error mapping.
//!
//! Every handler runs in its own tracing span (which becomes an `OTel` span).
//! DB calls made through the [`OrderStore`] trait emit child spans (see
//! `db.rs`), and `/checkout/{id}` builds a multi-step parent span with
//! `db query`, `inventory check`, and `payment` children — the richest trace
//! in the demo.
//!
//! Failures never panic: when the database is unreachable `/health` returns
//! 503 and the data endpoints return 5xx JSON errors, and the handler span is
//! marked with an error status so the dashboards light up.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rand::Rng;
use serde_json::{json, Value};

use crate::db::{random_order, record_span_error, Order, OrderStore, StoreError};

/// Budget for the `/health` DB ping; keeps probes fast while faults are active.
const HEALTH_PING_TIMEOUT: Duration = Duration::from_millis(900);

/// Budget for a data-endpoint DB operation. Bounds how long a handler can hang
/// when the database is paused/partitioned, so faults surface as 5xx (not
/// stuck requests).
const DB_OP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn OrderStore>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/orders", get(list_orders).post(create_order))
        .route("/orders/{id}", get(get_order))
        .route("/checkout/{id}", get(checkout))
        .with_state(state)
}

pub enum ApiError {
    NotFound(String),
    Unavailable(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// Run a store operation under [`DB_OP_TIMEOUT`], mapping both errors and
/// timeouts to a 503 and marking the current handler span as errored.
async fn db_op<F, T>(fut: F) -> Result<T, ApiError>
where
    F: Future<Output = Result<T, StoreError>>,
{
    match tokio::time::timeout(DB_OP_TIMEOUT, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => {
            record_span_error(&e.0);
            tracing::error!(error = %e.0, "database operation failed");
            Err(ApiError::Unavailable(e.0))
        }
        Err(_) => {
            let msg = "database operation timed out";
            record_span_error(msg);
            tracing::error!("{msg}");
            Err(ApiError::Unavailable(msg.to_owned()))
        }
    }
}

/// `GET /health` — 200 `{"status":"ok"}` when the DB is reachable, else 503.
/// This is the endpoint Tumult probes and the compose healthcheck hit.
#[tracing::instrument(
    name = "GET /health",
    skip(state),
    fields(otel.status_code = tracing::field::Empty, otel.status_message = tracing::field::Empty)
)]
async fn health(State(state): State<AppState>) -> Response {
    match tokio::time::timeout(HEALTH_PING_TIMEOUT, state.store.ping()).await {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response(),
        Ok(Err(e)) => health_down(&e.0),
        Err(_) => health_down("database ping timed out"),
    }
}

fn health_down(message: &str) -> Response {
    record_span_error(message);
    tracing::warn!(error = %message, "health check: database unreachable");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "status": "unavailable" })),
    )
        .into_response()
}

/// `GET /orders`
#[tracing::instrument(
    name = "GET /orders",
    skip(state),
    fields(otel.status_code = tracing::field::Empty, otel.status_message = tracing::field::Empty)
)]
async fn list_orders(State(state): State<AppState>) -> Result<Json<Vec<Order>>, ApiError> {
    let orders = db_op(state.store.list_orders()).await?;
    Ok(Json(orders))
}

/// `GET /orders/{id}`
#[tracing::instrument(
    name = "GET /orders/:id",
    skip(state),
    fields(otel.status_code = tracing::field::Empty, otel.status_message = tracing::field::Empty)
)]
async fn get_order(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Result<Json<Order>, ApiError> {
    match db_op(state.store.get_order(id)).await? {
        Some(order) => Ok(Json(order)),
        None => Err(ApiError::NotFound(format!("order {id} not found"))),
    }
}

/// `POST /orders` — inserts a randomly generated order.
#[tracing::instrument(
    name = "POST /orders",
    skip(state),
    fields(otel.status_code = tracing::field::Empty, otel.status_message = tracing::field::Empty)
)]
async fn create_order(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<Order>), ApiError> {
    let (name, amount_cents) = random_order();
    let order = db_op(state.store.insert_order(&name, amount_cents)).await?;
    tracing::info!(order.id = order.id, name = %order.name, "order created");
    Ok((StatusCode::CREATED, Json(order)))
}

/// `GET /checkout/{id}` — the demo's showcase multi-step trace.
///
/// Parent span `GET /checkout/:id` with three child spans:
///   1. `db query`        — load the order (emits nested `db.get_order`)
///   2. `inventory check`  — simulated downstream inventory reservation
///   3. `payment`          — simulated payment authorization
///
/// A DB fault turns step 1 into a 503 with an error span status; the healthy
/// path returns 200 with a confirmation body.
#[tracing::instrument(
    name = "GET /checkout/:id",
    skip(state),
    fields(otel.status_code = tracing::field::Empty, otel.status_message = tracing::field::Empty)
)]
async fn checkout(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Result<Json<Value>, ApiError> {
    // Step 1 — load the order under an explicit "db query" span.
    let order = {
        let span = tracing::info_span!("db query", order.id = id);
        let lookup = db_op(state.store.get_order(id));
        let result = { tracing::Instrument::instrument(lookup, span).await? };
        result.ok_or_else(|| ApiError::NotFound(format!("order {id} not found")))?
    };

    // Step 2 — inventory reservation (simulated downstream work).
    let reserved = inventory_check(order.amount_cents).await;

    // Step 3 — payment authorization (simulated downstream work).
    let authorized = payment(&order.name, order.amount_cents).await;

    tracing::info!(order.id = order.id, reserved, authorized, "checkout complete");
    Ok(Json(json!({
        "order_id": order.id,
        "name": order.name,
        "amount_cents": order.amount_cents,
        "inventory_reserved": reserved,
        "payment_authorized": authorized,
        "status": "confirmed",
    })))
}

/// Simulated inventory reservation. Emits an `inventory check` child span with
/// a little latency so the checkout trace has visible depth.
#[tracing::instrument(name = "inventory check")]
async fn inventory_check(amount_cents: i64) -> bool {
    tokio::time::sleep(jitter_ms(15, 45)).await;
    // Pretend very large orders are occasionally backordered.
    let reserved = amount_cents < 90_000;
    tracing::info!(reserved, "inventory checked");
    reserved
}

/// Simulated payment authorization. Emits a `payment` child span.
#[tracing::instrument(name = "payment", skip(name))]
async fn payment(name: &str, amount_cents: i64) -> bool {
    tokio::time::sleep(jitter_ms(25, 60)).await;
    tracing::info!(item = %name, amount_cents, "payment authorized");
    true
}

/// Uniformly random `base..base+span` millisecond delay.
fn jitter_ms(base: u64, span: u64) -> Duration {
    let extra = rand::rng().random_range(0..span);
    Duration::from_millis(base + extra)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// In-memory stub standing in for Postgres.
    struct StubStore {
        orders: Vec<Order>,
        fail: bool,
    }

    impl StubStore {
        fn healthy() -> Self {
            Self {
                orders: vec![
                    Order {
                        id: 1,
                        name: "amber widget".into(),
                        amount_cents: 1299,
                        created_at: "2026-07-04 12:00:00+00".into(),
                    },
                    Order {
                        id: 2,
                        name: "cobalt rotor".into(),
                        amount_cents: 4550,
                        created_at: "2026-07-04 12:01:00+00".into(),
                    },
                ],
                fail: false,
            }
        }

        fn broken() -> Self {
            Self {
                orders: Vec::new(),
                fail: true,
            }
        }

        fn check(&self) -> Result<(), StoreError> {
            if self.fail {
                Err(StoreError("connection refused".into()))
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl OrderStore for StubStore {
        async fn ping(&self) -> Result<(), StoreError> {
            self.check()
        }

        async fn list_orders(&self) -> Result<Vec<Order>, StoreError> {
            self.check()?;
            Ok(self.orders.clone())
        }

        async fn get_order(&self, id: i32) -> Result<Option<Order>, StoreError> {
            self.check()?;
            Ok(self.orders.iter().find(|o| o.id == id).cloned())
        }

        async fn insert_order(&self, name: &str, amount_cents: i64) -> Result<Order, StoreError> {
            self.check()?;
            Ok(Order {
                id: 42,
                name: name.to_owned(),
                amount_cents,
                created_at: "2026-07-04 12:02:00+00".into(),
            })
        }
    }

    fn test_router(store: StubStore) -> Router {
        router(AppState {
            store: Arc::new(store),
        })
    }

    async fn send(router: Router, method: &str, uri: &str) -> (StatusCode, Value) {
        let response = router
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap();
        (status, value)
    }

    #[tokio::test]
    async fn health_is_200_ok_when_db_reachable() {
        let (status, body) = send(test_router(StubStore::healthy()), "GET", "/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn health_is_503_when_db_down() {
        let (status, body) = send(test_router(StubStore::broken()), "GET", "/health").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, json!({ "status": "unavailable" }));
    }

    #[tokio::test]
    async fn list_orders_returns_json_array() {
        let (status, body) = send(test_router(StubStore::healthy()), "GET", "/orders").await;
        assert_eq!(status, StatusCode::OK);
        let orders = body.as_array().expect("array body");
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0]["name"], "amber widget");
        assert_eq!(orders[0]["amount_cents"], 1299);
    }

    #[tokio::test]
    async fn create_order_returns_201_with_order() {
        let (status, body) = send(test_router(StubStore::healthy()), "POST", "/orders").await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["id"], 42);
        assert!(body["name"].as_str().is_some_and(|n| !n.is_empty()));
        assert!(body["amount_cents"].as_i64().is_some_and(|a| a >= 199));
    }

    #[tokio::test]
    async fn checkout_confirms_existing_order() {
        let (status, body) = send(test_router(StubStore::healthy()), "GET", "/checkout/2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["order_id"], 2);
        assert_eq!(body["status"], "confirmed");
        assert_eq!(body["payment_authorized"], true);
    }

    #[tokio::test]
    async fn checkout_missing_order_is_404() {
        let (status, body) = send(test_router(StubStore::healthy()), "GET", "/checkout/999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "order 999 not found");
    }

    #[tokio::test]
    async fn db_failure_maps_to_503_json_error() {
        let (status, body) = send(test_router(StubStore::broken()), "GET", "/orders").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"], "connection refused");
    }

    #[tokio::test]
    async fn checkout_db_failure_maps_to_503() {
        let (status, _body) = send(test_router(StubStore::broken()), "GET", "/checkout/1").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
}
