//! Webhook dispatcher tests: due audit events are delivered to enabled
//! webhooks with a correct HMAC-SHA256 signature; the cursor advances
//! (fire-and-log) and event filters are honored.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tumult_ingest::IngestWriter;
use tumult_lake::{NewRun, Store, WebhookRow};

/// Run one dispatch against a local receiver; returns the captured
/// (signature, body) pairs and the delivery count.
struct Receiver {
    hits: Arc<Mutex<Vec<(String, String)>>>,
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
                    let sig = headers
                        .get("x-tumult-signature")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    hits.lock().unwrap().push((sig, body));
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
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.create_webhook(&hook).map_err(|e| e.to_string())?;
            writer
                .set_webhook_cursor("w-1", 1)
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
        let (sig, body) = &hits[0];
        // X-Tumult-Signature: sha256=<hmac-sha256(secret, body)>.
        let expected = tumult_ingest::webhooks::hmac_sha256_hex("test-secret", body);
        assert_eq!(sig, &format!("sha256={expected}"));
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
    // …but the cursor still advances (fire-and-log: no replays).
    let reader = Store::at(&db_path).read_only().unwrap();
    assert!(reader.webhook_cursor("w-1").unwrap().unwrap() > 2);
}
