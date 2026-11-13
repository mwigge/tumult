use super::serve;
use super::tls::load_tls;
use super::ui::{ui_handler, UiAssets};
use super::until_both_stop;
use super::{finish, Workers};
use axum::http::StatusCode;
use axum::response::Response;
use std::path::PathBuf;
use tumult_ingest::Config;

/// Throwaway self-signed `localhost` certificate/key pair (generated with
/// `openssl req -x509`), used to exercise the TLS load path.
const CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIDJTCCAg2gAwIBAgIUHvcqpsPfQCAnuxXOJLnZKClsYrswDQYJKoZIhvcNAQEL
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDczMTEyNTYyOFoXDTI2MDgw
MTEyNTYyOFowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEAtMVXRp4+rZ/6CfN2uLYBdUuI/3STDSWr6piuVvCi7S1J
bszJTo9ILKvvljKkhhkV9u4g+6cPzD5rt/rd8/92EWC+8lVkYPoLPM9ydLa4VbqV
/z6P7xt2DQmTY8oqfI+GkiXBvLfMKW63bqfYAEp+ZzwZPKibmykCmUScGGOfeezd
YinTrvDnlrttw6Jf5H6eu/CJAmZA1iKqjCQcxv3guUgUN6PECwfc7kIvCnSvNSlo
38f5zzg4xiyUA/larv9HtL4WZVrXwXnIrYv+tbA2eJJxUKwr8Pck/XKUHn5KhtDa
hmEp3wMdjJQSbXclgsyG43jIQ25CldbfYHEComUpTQIDAQABo28wbTAdBgNVHQ4E
FgQU7hn+N0c9xOL25lD8KslytcX7U0IwHwYDVR0jBBgwFoAU7hn+N0c9xOL25lD8
KslytcX7U0IwDwYDVR0TAQH/BAUwAwEB/zAaBgNVHREEEzARgglsb2NhbGhvc3SH
BH8AAAEwDQYJKoZIhvcNAQELBQADggEBAAUvKDh7FLD/DtMlERVvPiu7rtgBfLcK
S2Wd858fZ/IhvR3mvXNQxcWVsfjb8/O5ZlY0/+YCZMTjuL9YpBPix3WY50ktjHDk
f1HBjXWNS8hLLKpM7f3jwFsuE/OaYdRTu7ob2JOI2lDIjajHpKp0XU/a9pTq+xNX
XNpP0Bj7ElEmQKtNeJUqoMXzT1pPQJNpDLAyaNSYbz4yuSy7tXNoMI4Dy2VB5nLB
ye+6Z1fl8Y9TdaxZBQolepjbBEQQ/dqeu3+WLn17FInao9yR81/7J0CxAtv1YXen
qPsqtfAE6Ug4+5TQUqwIxtvzYHFF+pG2Lx+fcDCTQN3GDwXpJJdUQo8=
-----END CERTIFICATE-----
";
const KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQC0xVdGnj6tn/oJ
83a4tgF1S4j/dJMNJavqmK5W8KLtLUluzMlOj0gsq++WMqSGGRX27iD7pw/MPmu3
+t3z/3YRYL7yVWRg+gs8z3J0trhVupX/Po/vG3YNCZNjyip8j4aSJcG8t8wpbrdu
p9gASn5nPBk8qJubKQKZRJwYY5957N1iKdOu8OeWu23Dol/kfp678IkCZkDWIqqM
JBzG/eC5SBQ3o8QLB9zuQi8KdK81KWjfx/nPODjGLJQD+Vqu/0e0vhZlWtfBecit
i/61sDZ4knFQrCvw9yT9cpQefkqG0NqGYSnfAx2MlBJtdyWCzIbjeMhDbkKV1t9g
cQKiZSlNAgMBAAECggEAOqSUShP2+GNf/Y9uUcC1m2QcNucN92NjsJDEae7ZpACf
hGLJ4YLo4pkKedrG9bu4nOkmaQ0KunL7he1LyKZ0mnGcsEfUbwNe1uTjWAqYpTMJ
Cws0LVjmxJb5KhPBEbSL7uhxv7OOd1h0CGFJ2NpRxFLCSyPVixHURn1z+BOFfks1
GVi+e5oGG0nsGMw7EJ7dYr6Qu1mmvRuulshJlbTRFrveKXW5qIZLGsnkmvhR8A69
N9hbvsALfo4yaeKEQTXWzCBmYYdDPqRVosGOuwOSG10NP3O5KH2WAsrGX5J2qTCZ
4d3RMTJ9jdET3CQ4r3mhLaG2cegiWB7LLZpRF0ulXwKBgQDwlp61p7KfRfCYj25f
AgcZwL/KjTVD866XRz2p7LVTTxG2kZIIjaHYOZcenACe3KJ75BnrN6d7m/1GeW3N
XgVdiW35s2JoR07hRH+fdF1dBx/DSMMOhB9YquOuNEzgq+LSgb017kUfUaOkQFEY
mMxs6HgNA8Yq0v0l8zdDoD+UHwKBgQDAWcfqN55NQr76z8nTnKKjeuMPoL7WSJ+J
CBgVbP/eZAefnKqhA7MZFZZOfKmsBtspeHqj78ZqTXysZOAXSWCNlFnmw5EZ9NXf
T8WO+SVFt7S3ykjYsSMfC9+aBYQpyFjjp4Sjp/gyYGdMTt/y/kMEomx6+lEiindh
iKlK6aV1EwKBgBRQw6oXNRAZ+c0IH4vKQgs8qXVTIzJPu2huzZgxssYMITTHagtq
2kXF5yrghXTksJvBkSa5llzruSFgU5NJ4y4Y0r6JFUA09UY0YIp4awHV/iqhVEc/
hN4Z4AvvwqYeHZMk/XM2YYPZgvX1sGNhU7HGl4yRywQGuPWhagM93uCFAoGAGy/V
aM5pqoPnmG2sGiPGfRLOaxQORR1Ip0akmMqqM5Wx2iZ7m3x5YO9DKl7GYJErguYL
d4ZZZgcDux4a6k+tvPUd69byeFe5rvGIe9fNI9h+S4fk2fPXgfjcptlmv70YizzP
K45/LyefEhMH5kF32XzXll4w/4/QpdF6FCOIBk8CgYAY84rMAeJIZ6Os7PUOVTpP
RtbpfPUQivnmWXg4j9FMktcGRwf+jd+gY+0zx154foHbKzfCIU+Knn64WpWdqJb9
xXwj/ADaeC8lQd9LqUwMRysvfyIE9z+/Xky/FGNpoql7xskwe76x34sO+VoPBW3c
xPXAffHX6Z04foGNwjzXeg==
-----END PRIVATE KEY-----
";

fn config_with_tls(cert: Option<&std::path::Path>, key: Option<&std::path::Path>) -> Config {
    Config {
        db_path: PathBuf::from("/tmp/db.duckdb"),
        otlp_grpc_addr: "127.0.0.1:4317".parse().unwrap(),
        otlp_http_addr: "127.0.0.1:4318".parse().unwrap(),
        metrics_dir: PathBuf::from("metrics"),
        ingest_token: None,
        tls_cert: cert.map(std::path::Path::to_path_buf),
        tls_key: key.map(std::path::Path::to_path_buf),
    }
}

#[tokio::test]
async fn tls_unset_stays_plaintext() {
    assert!(load_tls(&config_with_tls(None, None))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn tls_missing_files_fail_fast_with_clear_message() {
    let config = config_with_tls(
        Some(std::path::Path::new("/nonexistent/tls.crt")),
        Some(std::path::Path::new("/nonexistent/tls.key")),
    );
    let err = load_tls(&config).await.err().unwrap();
    let msg = format!("{err:#}");
    assert!(msg.contains("KRONIKA_TLS_CERT"), "{msg}");
}

#[tokio::test]
async fn tls_garbage_pem_fails_fast() {
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("tls.crt");
    let key = dir.path().join("tls.key");
    std::fs::write(&cert, "not a pem").unwrap();
    std::fs::write(&key, "not a pem").unwrap();
    assert!(load_tls(&config_with_tls(Some(&cert), Some(&key)))
        .await
        .is_err());
}

#[tokio::test]
async fn tls_mismatched_key_fails_fast() {
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("tls.crt");
    let key = dir.path().join("tls.key");
    std::fs::write(&cert, CERT_PEM).unwrap();
    // A valid PEM but the wrong half: cert as key.
    std::fs::write(&key, CERT_PEM).unwrap();
    assert!(load_tls(&config_with_tls(Some(&cert), Some(&key)))
        .await
        .is_err());
}

#[tokio::test]
async fn tls_valid_pair_loads() {
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("tls.crt");
    let key = dir.path().join("tls.key");
    std::fs::write(&cert, CERT_PEM).unwrap();
    std::fs::write(&key, KEY_PEM).unwrap();
    assert!(load_tls(&config_with_tls(Some(&cert), Some(&key)))
        .await
        .unwrap()
        .is_some());
}

// -- ui_handler (embedded SPA) --------------------------------------------

async fn body_bytes(resp: Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn ui_handler_serves_index_html_at_the_root() {
    let resp = ui_handler(axum::http::Uri::from_static("/")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()[axum::http::header::CONTENT_TYPE],
        "text/html"
    );
    // Non-fingerprinted files must revalidate.
    assert_eq!(
        resp.headers()[axum::http::header::CACHE_CONTROL],
        "no-cache"
    );
    let index = UiAssets::get("index.html").expect("index.html is embedded");
    assert_eq!(body_bytes(resp).await.as_slice(), &index.data[..]);
}

#[tokio::test]
async fn ui_handler_caches_fingerprinted_assets_forever() {
    let path = UiAssets::iter()
        .find(|p| p.starts_with("_app/immutable/"))
        .expect("fingerprinted assets are embedded")
        .into_owned();
    let uri: axum::http::Uri = format!("/{path}").parse().unwrap();
    let resp = ui_handler(uri).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()[axum::http::header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    let file = UiAssets::get(&path).unwrap();
    assert_eq!(body_bytes(resp).await.as_slice(), &file.data[..]);
}

#[tokio::test]
async fn ui_handler_falls_back_to_the_app_shell_for_client_routes() {
    let resp = ui_handler(axum::http::Uri::from_static("/runs/some/client/route")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()[axum::http::header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let shell = UiAssets::get("200.html").expect("200.html app shell is embedded");
    assert_eq!(body_bytes(resp).await.as_slice(), &shell.data[..]);
}

// -- serve() end to end ----------------------------------------------------

/// One metric definition so the live `/report` endpoint has something to
/// render.
const METRIC_YAML: &str = r#"
name: experiment_count
description: Count of experiment runs in the window, per experiment.
source_table: spans
time_col: ts_ns
measure:
  type: count
dimensions: [experiment_name]
condition: { column: span_name, equals: "resilience.experiment" }
"#;

/// A currently-free loopback port (the listener is dropped before the
/// server binds it).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Minimal HTTP/1.0 GET against a loopback server; returns the raw
/// response (status line, headers and body).
async fn http_get(port: u16, path: &str) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes())
        .await?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

type Listener = tokio::task::JoinHandle<std::io::Result<()>>;

/// A listener task that serves until it is stopped, and a receiver that
/// resolves once the task is gone.
fn serving() -> (Listener, tokio::sync::oneshot::Receiver<()>) {
    let (alive, gone) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let _alive = alive;
        std::future::pending::<std::io::Result<()>>().await
    });
    (task, gone)
}

fn failing(why: &'static str) -> Listener {
    tokio::spawn(async move { Err(std::io::Error::other(why)) })
}

#[tokio::test]
async fn a_failed_http_listener_stops_the_grpc_one() {
    let (grpc, grpc_gone) = serving();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        until_both_stop(grpc, failing("address in use")),
    )
    .await
    .expect("the wait ends with the first failure");
    let error = format!("{:#}", outcome.expect_err("a failed listener is an error"));
    assert!(error.contains("HTTP server"), "{error}");
    assert!(error.contains("address in use"), "{error}");
    // The receiver errors because the task, and the sender in it, is gone.
    assert!(grpc_gone.await.is_err());
}

#[tokio::test]
async fn a_failed_grpc_listener_stops_the_http_one() {
    let (http, http_gone) = serving();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        until_both_stop(failing("no certificate"), http),
    )
    .await
    .expect("the wait ends with the first failure");
    let error = format!("{:#}", outcome.expect_err("a failed listener is an error"));
    assert!(error.contains("gRPC server"), "{error}");
    assert!(http_gone.await.is_err());
}

#[tokio::test]
async fn two_listeners_that_stop_cleanly_are_a_clean_stop() {
    let stopped = || -> Listener { tokio::spawn(async { Ok(()) }) };
    until_both_stop(stopped(), stopped())
        .await
        .expect("clean stop");
}

/// The daemon's workers on a new lake, as `serve` starts them: the
/// writer, the run queue and one background task holding the writer
/// open until it is told to stop.
struct Daemon {
    _dir: tempfile::TempDir,
    store: tumult_lake::Store,
    ingest: tumult_ingest::IngestWriter,
    workers: Workers,
}

fn daemon() -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lake.duckdb");
    let store = tumult_lake::Store::open(&db).unwrap();
    let (ingest, writer) = tumult_ingest::IngestWriter::spawn(store.writer().unwrap(), 16);
    let factory: tumult_ingest::runs::ExecutorFactory = std::sync::Arc::new(|env| {
        std::sync::Arc::new(tumult_exec::ProviderExecutor::with_injected_env(env))
    });
    let run_queue = tumult_ingest::RunQueue::spawn(
        ingest.clone(),
        db,
        tumult_ingest::RunQueueConfig::from_env(),
        factory,
    );
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (held, stop) = (ingest.clone(), shutdown.clone());
    let background = tokio::spawn(async move {
        stop.cancelled().await;
        drop(held);
    });
    let workers = Workers {
        shutdown,
        tasks: vec![("background", background)],
        run_queue,
        writer,
    };
    Daemon {
        _dir: dir,
        store,
        ingest,
        workers,
    }
}

/// Hand the writer one batch, as a request in flight does: the batch is
/// on its way and nothing has waited for it.
fn write_in_flight(ingest: tumult_ingest::IngestWriter) {
    let batch = tumult_ingest::Batch::Exec(Box::new(|writer| {
        writer
            .execute("CREATE TABLE drained AS SELECT 42 AS answer", ())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }));
    tokio::spawn(async move { ingest.write(batch).await });
}

/// What the batch of [`write_in_flight`] left in the lake, one row per
/// line.
fn persisted(store: &tumult_lake::Store) -> Result<String, String> {
    let reader = store.read_only().map_err(|e| e.to_string())?;
    let rows = reader
        .query_json_rows("SELECT answer FROM drained")
        .map_err(|e| e.to_string())?;
    Ok(rows.iter().map(|row| format!("{row}\n")).collect())
}

/// The one row [`write_in_flight`] writes.
const WRITTEN: &str = "{\"answer\":42}\n";

/// Longer than any drain here takes.
const AMPLE: std::time::Duration = std::time::Duration::from_secs(30);

fn listener_failed() -> anyhow::Result<()> {
    Err(anyhow::anyhow!("HTTP server: connection reset"))
}

#[tokio::test]
async fn a_failed_listener_still_has_the_writer_drained() {
    let Daemon {
        _dir,
        store,
        ingest,
        workers,
    } = daemon();
    write_in_flight(ingest);

    let outcome = finish(listener_failed(), None::<Listener>, workers, AMPLE).await;

    // Read at once: whatever `finish` did not wait for is lost with the
    // process.
    let rows = persisted(&store).expect("the batch in flight was written");
    assert_eq!(rows, WRITTEN);
    let error = format!(
        "{:#}",
        outcome.expect_err("the failure is still the outcome")
    );
    assert!(error.contains("connection reset"), "{error}");
}

#[tokio::test]
async fn a_connection_left_open_does_not_hold_a_failed_daemon_up() {
    let Daemon {
        _dir,
        store,
        ingest,
        workers,
    } = daemon();
    write_in_flight(ingest.clone());
    // A connection that outlived its listener keeps a writer handle.
    let _connection = ingest;

    let brief = std::time::Duration::from_millis(300);
    let ended = finish(listener_failed(), None::<Listener>, workers, brief);
    let outcome = tokio::time::timeout(AMPLE, ended)
        .await
        .expect("the wait for the writer is bounded");

    let error = format!("{:#}", outcome.expect_err("the failure is the outcome"));
    assert!(error.contains("connection reset"), "{error}");
    let rows = persisted(&store).expect("what was queued is written all the same");
    assert_eq!(rows, WRITTEN);
}

#[tokio::test]
async fn a_failed_listener_stops_the_loopback_one() {
    let Daemon { _dir, workers, .. } = daemon();
    let (loopback, loopback_gone) = serving();

    let outcome = finish(listener_failed(), Some(loopback), workers, AMPLE).await;

    assert!(outcome.is_err());
    // The receiver errors because the task, and the sender in it, is gone.
    assert!(loopback_gone.await.is_err());
}

#[tokio::test]
async fn a_failed_loopback_listener_still_has_the_writer_drained() {
    let Daemon {
        _dir,
        store,
        ingest,
        workers,
    } = daemon();
    write_in_flight(ingest);

    let outcome = finish(Ok(()), Some(failing("broken pipe")), workers, AMPLE).await;

    let rows = persisted(&store).expect("the batch in flight was written");
    assert_eq!(rows, WRITTEN);
    let error = format!("{:#}", outcome.expect_err("the failure is the outcome"));
    assert!(error.contains("loopback"), "{error}");
    assert!(error.contains("broken pipe"), "{error}");
}

#[tokio::test]
async fn a_clean_stop_waits_for_the_loopback_listener_and_the_writer() {
    let Daemon {
        _dir,
        store,
        ingest,
        workers,
    } = daemon();
    write_in_flight(ingest);
    let loopback: Listener = tokio::spawn(async { Ok(()) });

    finish(Ok(()), Some(loopback), workers, AMPLE)
        .await
        .expect("clean stop");
    assert_eq!(persisted(&store).expect("written"), WRITTEN);
}

/// Point `serve` at a lake of its own in `dir` and at these loopback
/// ports. The caller holds the env lock.
fn daemon_env(dir: &std::path::Path, grpc_port: u16, http_port: u16) {
    std::env::set_var("TUMULT_LAKE_PATH", dir.join("lake.duckdb"));
    std::env::set_var("KRONIKA_METRICS_DIR", dir.join("metrics"));
    std::env::set_var("KRONIKA_OTLP_GRPC_ADDR", format!("127.0.0.1:{grpc_port}"));
    std::env::set_var("KRONIKA_OTLP_HTTP_ADDR", format!("127.0.0.1:{http_port}"));
    std::env::set_var("KRONIKA_LAKE_INTERVAL", "off");
    std::env::remove_var("KRONIKA_INGEST_TOKEN");
    std::env::remove_var("KRONIKA_REPORT_INTERVAL");
    std::env::remove_var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD");
    std::env::remove_var("KRONIKA_BOOTSTRAP_TOKEN");
}

/// A run an earlier daemon left queued. Reconciling it is the first work
/// a starting daemon does, before any background task exists.
fn leave_a_queued_run(db: &std::path::Path) {
    let store = tumult_lake::Store::open(db).unwrap();
    let writer = store.writer().unwrap();
    writer
        .register_definition(&tumult_lake::RegisteredDefinition {
            id: "def-1".into(),
            name: "left behind".into(),
            definition_toon: String::new(),
            content_hash: "hash-1".into(),
            registered_at_ns: 1,
            registered_by: None,
        })
        .unwrap();
    writer
        .insert_run(&tumult_lake::NewRun {
            id: "run-1".into(),
            registry_id: "def-1".into(),
            params_json: None,
            queued_at_ns: 1,
            actor: None,
        })
        .unwrap();
}

fn state_of_the_queued_run(db: &std::path::Path) -> String {
    let reader = tumult_lake::Store::at(db).read_only().unwrap();
    let run = reader.run_get("run-1").unwrap().expect("the run is there");
    run["state"].to_string()
}

fn alive_tasks() -> usize {
    tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks()
}

/// The number of alive tasks once it is back at `expected`, or what it
/// still is after a few seconds.
async fn alive_tasks_settled(expected: usize) -> usize {
    for _ in 0..100 {
        if alive_tasks() == expected {
            return expected;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    alive_tasks()
}

/// A daemon that cannot have both its addresses says so before it starts
/// anything: no run is reconciled, no queue, scheduler or listener runs.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_daemon_that_cannot_bind_starts_no_work() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for taken_address in ["gRPC", "HTTP"] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("lake.duckdb");
        leave_a_queued_run(&db);
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken_port = taken.local_addr().unwrap().port();
        let (grpc_port, http_port) = match taken_address {
            "gRPC" => (taken_port, free_port()),
            _ => (free_port(), taken_port),
        };
        daemon_env(dir.path(), grpc_port, http_port);
        let before = alive_tasks();

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), serve())
            .await
            .expect("the daemon exits");

        let error = format!("{:#}", outcome.expect_err("a taken address is an error"));
        let expected = format!("{taken_address} server: bind 127.0.0.1:{taken_port}");
        assert!(error.contains(&expected), "{error}");
        assert_eq!(
            state_of_the_queued_run(&db),
            "\"queued\"",
            "{taken_address} address taken: no run was touched"
        );
        assert_eq!(
            alive_tasks_settled(before).await,
            before,
            "{taken_address} address taken: no task is left running"
        );
        drop(taken);
    }
}

/// A daemon whose HTTP address is taken must not stay up serving gRPC
/// only: whoever holds the port would answer its health checks.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serve_exits_when_the_http_address_is_taken() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = taken.local_addr().unwrap().port();
    let grpc_port = free_port();
    daemon_env(dir.path(), grpc_port, http_port);

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), serve())
        .await
        .expect("the daemon exits instead of serving gRPC alone");
    let error = format!("{:#}", outcome.expect_err("a taken address is an error"));
    assert!(error.contains("HTTP server"), "{error}");
    // Nothing of the daemon is left listening.
    let grpc = tokio::net::TcpStream::connect(("127.0.0.1", grpc_port)).await;
    assert!(grpc.is_err(), "the gRPC listener is closed");
    drop(taken);
}

/// The whole daemon in-process: bind both listeners on loopback, serve
/// the health and live-report endpoints, then stop cleanly on SIGTERM.
/// Holds the env lock for its whole lifetime — the configuration is
/// process-global. Holding a std mutex guard across awaits is deliberate
/// here: the guard only ever blocks other env-mutating tests, and this
/// test's own progress never depends on them.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serve_binds_serves_and_shuts_down_cleanly_on_sigterm() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let metrics_dir = dir.path().join("metrics");
    std::fs::create_dir_all(&metrics_dir).unwrap();
    std::fs::write(metrics_dir.join("experiment_count.yaml"), METRIC_YAML).unwrap();
    let grpc_port = free_port();
    let http_port = free_port();
    std::env::set_var("TUMULT_LAKE_PATH", dir.path().join("lake.duckdb"));
    std::env::set_var("KRONIKA_METRICS_DIR", &metrics_dir);
    std::env::set_var("KRONIKA_OTLP_GRPC_ADDR", format!("127.0.0.1:{grpc_port}"));
    std::env::set_var("KRONIKA_OTLP_HTTP_ADDR", format!("127.0.0.1:{http_port}"));
    std::env::set_var("KRONIKA_LAKE_INTERVAL", "off");
    std::env::remove_var("KRONIKA_INGEST_TOKEN");
    std::env::remove_var("KRONIKA_REPORT_INTERVAL");
    std::env::remove_var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD");
    std::env::remove_var("KRONIKA_BOOTSTRAP_TOKEN");

    let mut daemon = tokio::spawn(serve());

    // Wait for the HTTP listener, then probe the health and live report
    // endpoints while the daemon holds the store.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let health = loop {
        if let Ok(Ok(resp)) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            http_get(http_port, "/healthz"),
        )
        .await
        {
            break resp;
        }
        if daemon.is_finished() {
            let outcome = (&mut daemon).await.expect("daemon task panicked");
            panic!("daemon exited before serving: {outcome:?}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "daemon did not start serving within 30s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert!(health.contains(" 200 OK"), "{health}");

    // Readiness (migrations applied, supervisor ticking) and the daemon
    // metrics exposition answer on the same listener.
    let ready = http_get(http_port, "/readyz")
        .await
        .expect("readiness request failed");
    assert!(ready.contains(" 200 OK"), "{ready}");
    let metrics = http_get(http_port, "/metrics")
        .await
        .expect("metrics request failed");
    assert!(metrics.contains(" 200 OK"), "{metrics}");
    assert!(
        metrics.contains("tumultd_runs_started_total"),
        "metrics endpoint did not expose daemon SLIs"
    );

    let report = http_get(http_port, "/report?metric=experiment_count")
        .await
        .expect("live report request failed");
    assert!(report.contains(" 200 OK"), "{report}");
    assert!(
        report.contains("Tumult — experiment_count"),
        "live report did not render the metric"
    );

    // SIGTERM to ourselves: the daemon's shutdown handler must drive a
    // clean stop (servers, lake task, writer drain) and return Ok.
    let status = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(std::process::id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), daemon)
        .await
        .expect("daemon did not stop within 30s of SIGTERM")
        .expect("daemon task panicked");
    result.expect("daemon returned an error");

    std::env::remove_var("TUMULT_LAKE_PATH");
    std::env::remove_var("KRONIKA_METRICS_DIR");
    std::env::remove_var("KRONIKA_OTLP_GRPC_ADDR");
    std::env::remove_var("KRONIKA_OTLP_HTTP_ADDR");
    std::env::remove_var("KRONIKA_LAKE_INTERVAL");
}

#[test]
fn plaintext_network_listeners_are_reported_with_address_and_port() {
    let mut config = config_with_tls(None, None);
    config.otlp_http_addr = "0.0.0.0:4318".parse().unwrap();
    config.otlp_grpc_addr = "192.0.2.7:4317".parse().unwrap();

    let logs = tumult_otel::logging::testing::capture_json(tracing::Level::INFO, || {
        super::tls::warn_if_plaintext_on_network(&config, false);
    });

    let http = logs.named("daemon.tls.http_plaintext");
    assert_eq!(http.len(), 1, "{}", logs.raw);
    assert_eq!(http[0]["level"], "WARN");
    assert_eq!(http[0]["server.address"], "0.0.0.0");
    assert_eq!(http[0]["server.port"], 4318);
    let grpc = logs.named("daemon.tls.grpc_plaintext");
    assert_eq!(grpc.len(), 1, "{}", logs.raw);
    assert_eq!(grpc[0]["server.address"], "192.0.2.7");
    assert_eq!(grpc[0]["server.port"], 4317);
}

#[test]
fn tls_or_loopback_listeners_raise_no_plaintext_warning() {
    let mut exposed = config_with_tls(None, None);
    exposed.otlp_http_addr = "0.0.0.0:4318".parse().unwrap();
    let loopback = config_with_tls(None, None);

    let logs = tumult_otel::logging::testing::capture_json(tracing::Level::INFO, || {
        super::tls::warn_if_plaintext_on_network(&exposed, true);
        super::tls::warn_if_plaintext_on_network(&loopback, false);
    });

    assert!(logs.events.is_empty(), "{}", logs.raw);
}
