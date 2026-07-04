//! demo-traffic — continuous, low-rate load generator for the Tumult demo.
//!
//! Hits `demo-app` with a mix of `GET /orders`, `POST /orders`, and
//! `GET /checkout/{id}` so there is always a baseline trace stream for the
//! fault-injection experiments to disrupt. Runs forever and is resilient to
//! `demo-app` being down (it logs and keeps going — never exits).
//!
//! Environment (defaults match the demo CONTRACT):
//! - `TARGET_URL`     (default `http://demo-app:8080`)
//! - `REQS_PER_SEC`   (default `3`) — approximate request rate

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use serde_json::Value;

/// Cap on how many recent order ids we remember for checkout requests.
const MAX_TRACKED_IDS: usize = 32;

#[tokio::main]
async fn main() {
    let target = env_or("TARGET_URL", "http://demo-app:8080");
    let target = target.trim_end_matches('/').to_owned();
    let rps = std::env::var("REQS_PER_SEC")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(3.0);
    let interval = Duration::from_secs_f64(1.0 / rps);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    // Ids discovered from POST /orders responses, reused for checkout so the
    // traffic references real rows most of the time.
    let known_ids: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));

    println!("demo-traffic -> {target} at ~{rps} req/s");

    loop {
        let action = pick_action();
        let outcome = match action {
            Action::List => list_orders(&client, &target).await,
            Action::Create => create_order(&client, &target, &known_ids).await,
            Action::Checkout => checkout(&client, &target, &known_ids).await,
        };
        if let Err(e) = outcome {
            // demo-app may be down or fault-injected; that's expected — keep going.
            eprintln!("request failed ({action:?}): {e}");
        }
        tokio::time::sleep(interval).await;
    }
}

#[derive(Debug, Clone, Copy)]
enum Action {
    List,
    Create,
    Checkout,
}

/// Weighted mix: ~40% list, ~20% create, ~40% checkout.
fn pick_action() -> Action {
    match rand::rng().random_range(0..5) {
        0 | 1 => Action::List,
        2 => Action::Create,
        _ => Action::Checkout,
    }
}

async fn list_orders(client: &reqwest::Client, target: &str) -> Result<(), reqwest::Error> {
    let resp = client.get(format!("{target}/orders")).send().await?;
    println!("GET /orders -> {}", resp.status());
    Ok(())
}

async fn create_order(
    client: &reqwest::Client,
    target: &str,
    known_ids: &Arc<Mutex<Vec<i64>>>,
) -> Result<(), reqwest::Error> {
    let resp = client.post(format!("{target}/orders")).send().await?;
    let status = resp.status();
    if status.is_success() {
        if let Ok(body) = resp.json::<Value>().await {
            if let Some(id) = body.get("id").and_then(Value::as_i64) {
                remember_id(known_ids, id);
            }
        }
    }
    println!("POST /orders -> {status}");
    Ok(())
}

async fn checkout(
    client: &reqwest::Client,
    target: &str,
    known_ids: &Arc<Mutex<Vec<i64>>>,
) -> Result<(), reqwest::Error> {
    let id = pick_id(known_ids);
    let resp = client
        .get(format!("{target}/checkout/{id}"))
        .send()
        .await?;
    println!("GET /checkout/{id} -> {}", resp.status());
    Ok(())
}

fn remember_id(known_ids: &Arc<Mutex<Vec<i64>>>, id: i64) {
    let mut ids = known_ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !ids.contains(&id) {
        ids.push(id);
        if ids.len() > MAX_TRACKED_IDS {
            ids.remove(0);
        }
    }
}

/// Pick a checkout id: a known id if we have any, otherwise a small random id
/// (the seed data uses ids 1..=20, so these usually hit real rows).
fn pick_id(known_ids: &Arc<Mutex<Vec<i64>>>) -> i64 {
    let ids = known_ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut rng = rand::rng();
    if ids.is_empty() || rng.random_bool(0.3) {
        rng.random_range(1..=20)
    } else {
        ids[rng.random_range(0..ids.len())]
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}
