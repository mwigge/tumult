//! Webhook dispatcher tests: due audit events are delivered to enabled
//! webhooks with a correct HMAC-SHA256 signature; the cursor advances past
//! delivered (and filtered) events; a failing endpoint is retried under
//! exponential backoff and dead-lettered after `max_attempts`; and one
//! hung endpoint cannot stall the others.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tumult_ingest::IngestWriter;
use tumult_lake::{NewRun, Store, WebhookRow};

/// One captured delivery: (signature, timestamp, signature-v2, body).
type Hits = Arc<Mutex<Vec<(String, String, String, String)>>>;

/// Run one dispatch against a local receiver; returns the captured
/// (signature, timestamp, signature-v2, body) tuples and the delivery count.
struct Receiver {
    hits: Hits,
    url: String,
}

async fn spawn_receiver() -> Receiver {
    let hits = Arc::new(Mutex::new(Vec::new()));
    let app = {
        let hits = Arc::clone(&hits);
        axum::Router::new().route(
            "/hook",
            axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
                let hits = Arc::clone(&hits);
                async move {
                    let header = |name: &str| {
                        headers
                            .get(name)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or_default()
                            .to_string()
                    };
                    hits.lock().unwrap().push((
                        header("x-tumult-signature"),
                        header("x-tumult-timestamp"),
                        header("x-tumult-signature-v2"),
                        body,
                    ));
                    "ok"
                }
            }),
        )
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Receiver { hits, url }
}

fn webhook(url: &str, events: Vec<String>) -> WebhookRow {
    WebhookRow {
        id: "w-1".into(),
        name: "test hook".into(),
        url: url.into(),
        secret: "test-secret".into(),
        events,
        enabled: true,
        created_by: Some("test".into()),
        created_at_ns: 1,
    }
}

async fn seed(ingest: &IngestWriter, hook: &WebhookRow) {
    let hook = hook.clone();
    let id = hook.id.clone();
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.create_webhook(&hook).map_err(|e| e.to_string())?;
            writer
                .set_webhook_cursor(&id, 1)
                .map_err(|e| e.to_string())?;
            writer
                .insert_run(&NewRun {
                    id: "run-1".into(),
                    registry_id: "reg-1".into(),
                    params_json: None,
                    queued_at_ns: 2,
                    actor: Some("tester".into()),
                })
                .map_err(|e| e.to_string())
        })))
        .await
        .unwrap();
}

#[tokio::test]
async fn dispatcher_posts_signed_events_and_advances_the_cursor() {
    // The test receiver is loopback+http: the SSRF guard needs the explicit
    // opt-ins (this is a separate test binary, so the env is contained).
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_INSECURE", "1");
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_LOCAL", "1");

    let receiver = spawn_receiver().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    seed(&ingest, &webhook(&receiver.url, vec![])).await;

    let delivered = tumult_ingest::webhooks::dispatch_pending(&db_path, &ingest)
        .await
        .unwrap();
    assert_eq!(delivered, 1, "one due event (the run's enqueued)");

    {
        let hits = receiver.hits.lock().unwrap();
        assert_eq!(hits.len(), 1);
        let (sig, timestamp, sig_v2, body) = &hits[0];
        // X-Tumult-Signature: sha256=<hmac-sha256(secret, body)> — unchanged,
        // so receivers built before the timestamp scheme keep verifying.
        let expected = tumult_ingest::webhooks::hmac_sha256_hex("test-secret", body);
        assert_eq!(sig, &format!("sha256={expected}"));
        // The additive replay protection: a parseable, fresh timestamp whose
        // v2 signature covers "{timestamp}.{body}".
        let ts: i64 = timestamp.parse().expect("unix-seconds timestamp");
        assert!(tumult_ingest::webhooks::verify_v2(
            "test-secret",
            body,
            ts,
            sig_v2,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .cast_signed(),
        ));
        let payload: Value = serde_json::from_str(body).unwrap();
        assert_eq!(payload["event"], "enqueued");
        assert_eq!(payload["run_id"], "run-1");
        assert_eq!(payload["actor"], "tester");
    }

    // The cursor advanced past the delivered event; a second dispatch is a
    // no-op.
    let reader = Store::at(&db_path).read_only().unwrap();
    let cursor = reader.webhook_cursor("w-1").unwrap().unwrap();
    assert!(cursor > 2, "{cursor}");
    let delivered = tumult_ingest::webhooks::dispatch_pending(&db_path, &ingest)
        .await
        .unwrap();
    assert_eq!(delivered, 0);
    assert_eq!(receiver.hits.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn dispatcher_honours_the_event_filter() {
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_INSECURE", "1");
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_LOCAL", "1");
    let receiver = spawn_receiver().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    // The hook only wants stop_requested; the due event is enqueued.
    seed(
        &ingest,
        &webhook(&receiver.url, vec!["stop_requested".into()]),
    )
    .await;

    let delivered = tumult_ingest::webhooks::dispatch_pending(&db_path, &ingest)
        .await
        .unwrap();
    assert_eq!(delivered, 0, "filtered events are not delivered");
    assert!(receiver.hits.lock().unwrap().is_empty());
    // …but the cursor still advances (filtered events are never candidates).
    let reader = Store::at(&db_path).read_only().unwrap();
    assert!(reader.webhook_cursor("w-1").unwrap().unwrap() > 2);
}

/// A receiver that always rejects with 500 (hits are counted).
async fn spawn_rejecting_receiver() -> Receiver {
    let hits = Arc::new(Mutex::new(Vec::new()));
    let app = {
        let hits = Arc::clone(&hits);
        axum::Router::new().route(
            "/hook",
            axum::routing::post(move |body: String| {
                let hits = Arc::clone(&hits);
                async move {
                    hits.lock()
                        .unwrap()
                        .push((String::new(), String::new(), String::new(), body));
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                }
            }),
        )
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Receiver { hits, url }
}

fn dispatcher(
    max_attempts: u32,
    budget: std::time::Duration,
) -> tumult_ingest::webhooks::Dispatcher {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    tumult_ingest::webhooks::Dispatcher::new(client, max_attempts, budget)
}

#[tokio::test]
async fn failing_endpoint_is_dead_lettered_and_the_cursor_advances_past_the_loss() {
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_INSECURE", "1");
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_LOCAL", "1");
    let receiver = spawn_rejecting_receiver().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    seed(&ingest, &webhook(&receiver.url, vec![])).await;

    let mut d = dispatcher(2, std::time::Duration::from_secs(30));
    // Tick 1: delivery rejected, cursor must NOT advance (retry next tick).
    assert_eq!(d.dispatch_pending(&db_path, &ingest).await.unwrap(), 0);
    let reader = Store::at(&db_path).read_only().unwrap();
    assert_eq!(reader.webhook_cursor("w-1").unwrap(), Some(1));
    assert!(reader
        .query_json_rows("SELECT * FROM webhook_dead_letters")
        .unwrap()
        .is_empty());

    // Tick 2: second consecutive failure reaches max_attempts — the event is
    // dead-lettered and the cursor advances past it.
    assert_eq!(d.dispatch_pending(&db_path, &ingest).await.unwrap(), 0);
    let reader = Store::at(&db_path).read_only().unwrap();
    assert!(reader.webhook_cursor("w-1").unwrap().unwrap() > 2);
    let letters = reader
        .query_json_rows(
            "SELECT webhook_id, run_id, event, error, attempts FROM webhook_dead_letters",
        )
        .unwrap();
    assert_eq!(letters.len(), 1, "{letters:?}");
    assert_eq!(letters[0]["webhook_id"], serde_json::json!("w-1"));
    assert_eq!(letters[0]["run_id"], serde_json::json!("run-1"));
    assert_eq!(letters[0]["event"], serde_json::json!("enqueued"));
    assert_eq!(letters[0]["attempts"], serde_json::json!(2));
    assert!(
        letters[0]["error"].as_str().unwrap().contains("500"),
        "{letters:?}"
    );

    // Nothing left to deliver: the receiver saw exactly two attempts.
    assert_eq!(d.dispatch_pending(&db_path, &ingest).await.unwrap(), 0);
    assert_eq!(receiver.hits.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn backoff_skips_ticks_between_attempts() {
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_INSECURE", "1");
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_LOCAL", "1");
    let receiver = spawn_rejecting_receiver().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    seed(&ingest, &webhook(&receiver.url, vec![])).await;

    let mut d = dispatcher(10, std::time::Duration::from_secs(30));
    d.dispatch_pending(&db_path, &ingest).await.unwrap(); // tick 1: attempt 1
    d.dispatch_pending(&db_path, &ingest).await.unwrap(); // tick 2: attempt 2, backoff 2 ticks
    d.dispatch_pending(&db_path, &ingest).await.unwrap(); // tick 3: skipped
    assert_eq!(
        receiver.hits.lock().unwrap().len(),
        2,
        "the backoff tick must not attempt a delivery"
    );
    d.dispatch_pending(&db_path, &ingest).await.unwrap(); // tick 4: attempt 3
    assert_eq!(receiver.hits.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn one_hung_endpoint_does_not_block_the_others() {
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_INSECURE", "1");
    std::env::set_var("TUMULTD_WEBHOOK_ALLOW_LOCAL", "1");
    // A receiver that hangs for 10s before answering.
    let hang_app = axum::Router::new().route(
        "/hook",
        axum::routing::post(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            "ok"
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hang_url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, hang_app).await.unwrap() });
    let good = spawn_receiver().await;

    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("kronika.duckdb");
    let store = Store::open(&db_path).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
    let mut hung = webhook(&hang_url, vec![]);
    hung.id = "w-hung".into();
    let mut healthy = webhook(&good.url, vec![]);
    healthy.id = "w-good".into();
    seed(&ingest, &hung).await;
    // The second hook shares the same run's audit events: register it and
    // its cursor without inserting a second run.
    {
        let healthy = healthy.clone();
        ingest
            .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
                writer.create_webhook(&healthy).map_err(|e| e.to_string())?;
                writer
                    .set_webhook_cursor(&healthy.id, 1)
                    .map_err(|e| e.to_string())
            })))
            .await
            .unwrap();
    }

    let started = std::time::Instant::now();
    let mut d = dispatcher(5, std::time::Duration::from_millis(500));
    let delivered = d.dispatch_pending(&db_path, &ingest).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(delivered, 1, "the healthy endpoint still got the event");
    assert_eq!(good.hits.lock().unwrap().len(), 1);
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the hung endpoint stalled the tick: {elapsed:?}"
    );
    // The hung endpoint's cursor did not advance; its event is retried.
    let reader = Store::at(&db_path).read_only().unwrap();
    assert_eq!(reader.webhook_cursor("w-hung").unwrap(), Some(1));
    assert!(reader.webhook_cursor("w-good").unwrap().unwrap() > 2);
}
