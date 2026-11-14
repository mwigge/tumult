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

/// What was true at the moment the telemetry was flushed.
#[derive(Debug, PartialEq, Eq)]
struct Flushed {
    /// The listener that did not fail was still serving.
    survivor_serving: bool,
    /// The `process.fatal` event had been logged.
    fatal_logged: bool,
}

/// Stands in for the daemon's telemetry and records each flush.
#[derive(Default)]
struct FlushProbe {
    /// Resolves once the listener that did not fail is gone.
    survivor_gone: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// What the test thread has logged.
    log: tumult_otel::logging::testing::Sink,
    flushes: std::sync::Mutex<Vec<Flushed>>,
}

impl FlushProbe {
    fn watching(
        survivor_gone: tokio::sync::oneshot::Receiver<()>,
        capture: &tumult_otel::logging::testing::LogCapture,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            survivor_gone: std::sync::Mutex::new(Some(survivor_gone)),
            log: capture.sink(),
            flushes: std::sync::Mutex::default(),
        })
    }
}

impl super::Flush for FlushProbe {
    fn flush(&self) {
        let survivor_serving = self
            .survivor_gone
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|gone| {
                gone.try_recv() == Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            });
        let fatal_logged = self.log.text().contains("\"process.fatal\"");
        self.flushes.lock().unwrap().push(Flushed {
            survivor_serving,
            fatal_logged,
        });
    }
}

/// Telemetry nobody looks at.
fn unwatched() -> std::sync::Arc<FlushProbe> {
    std::sync::Arc::default()
}

#[tokio::test]
async fn a_failed_listener_is_reported_and_flushed_while_the_other_still_serves() {
    let capture = tumult_otel::logging::testing::LogCapture::start(tracing::Level::INFO);
    let (grpc, grpc_gone) = serving();
    let telemetry = FlushProbe::watching(grpc_gone, &capture);

    let outcome = until_both_stop(grpc, failing("address in use"), &telemetry).await;

    assert_eq!(
        *telemetry.flushes.lock().unwrap(),
        [Flushed {
            survivor_serving: true,
            fatal_logged: true,
        }]
    );
    // Stopped all the same, once the flush is done.
    let grpc_gone = telemetry.survivor_gone.lock().unwrap().take().unwrap();
    assert!(grpc_gone.await.is_err());
    let logs = capture.snapshot();
    let fatal = logs.named("process.fatal");
    assert_eq!(fatal.len(), 1, "{}", logs.raw);
    let message = fatal[0]["error.message"].as_str().unwrap();
    assert!(message.contains("HTTP server"), "{message}");
    assert!(message.contains("address in use"), "{message}");
    let error = outcome.expect_err("a failed listener is an error");
    assert!(crate::events::fatal_was_reported(&error));
}

#[tokio::test]
async fn a_clean_stop_leaves_the_flush_to_the_shutdown_signal() {
    let stopped = || -> Listener { tokio::spawn(async { Ok(()) }) };
    let telemetry = unwatched();

    until_both_stop(stopped(), stopped(), &telemetry)
        .await
        .expect("clean stop");

    assert!(telemetry.flushes.lock().unwrap().is_empty());
}

/// Telemetry whose exporter hangs until the test lets go of it.
struct StuckFlush(std::sync::Mutex<std::sync::mpsc::Receiver<()>>);

impl super::Flush for StuckFlush {
    fn flush(&self) {
        // Ends when the sender is dropped.
        let _ = self.0.lock().unwrap().recv();
    }
}

#[tokio::test]
async fn a_flush_that_hangs_is_given_up_on_and_reported() {
    let capture = tumult_otel::logging::testing::LogCapture::start(tracing::Level::INFO);
    let (release, held) = std::sync::mpsc::channel::<()>();
    let telemetry = std::sync::Arc::new(StuckFlush(std::sync::Mutex::new(held)));
    let limit = std::time::Duration::from_millis(50);

    let started = std::time::Instant::now();
    super::flush_within(telemetry, limit).await;

    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let logs = capture.snapshot();
    let timed_out = logs.named("daemon.telemetry.flush.timed_out");
    assert_eq!(timed_out.len(), 1, "{}", logs.raw);
    assert_eq!(timed_out[0]["level"], "WARN");
    assert_eq!(timed_out[0]["timeout_ms"], 50);
    drop(release);
}

#[tokio::test]
async fn a_flush_that_finishes_in_time_is_not_reported() {
    let capture = tumult_otel::logging::testing::LogCapture::start(tracing::Level::INFO);
    let telemetry = unwatched();

    super::flush_within(std::sync::Arc::clone(&telemetry), super::FLUSH_TIMEOUT).await;

    assert_eq!(telemetry.flushes.lock().unwrap().len(), 1);
    assert!(capture.snapshot().events.is_empty());
}

/// A batch the store refuses.
fn refused_batch() -> tumult_ingest::Batch {
    tumult_ingest::Batch::Exec(Box::new(|_| Err("disk full".to_string())))
}

/// How many records from the ingest path the daemon has exported.
fn exported_from_the_ingest_path(capture: &tumult_otel::logging::testing::ExportCapture) -> usize {
    capture
        .exported()
        .iter()
        .filter(|event| {
            crate::events::INGEST_PATH
                .iter()
                .any(|module| event.target.starts_with(module))
        })
        .count()
}

/// The daemon exports its own log events to its own ingest. With a store
/// that refuses every write, each record exported from the ingest path
/// would come back as one more refused write, and one more record.
#[tokio::test]
async fn a_failing_store_never_feeds_the_daemons_own_export() {
    let capture =
        tumult_otel::logging::testing::ExportCapture::start(&crate::events::log_options("tumultd"));
    let Daemon {
        _dir,
        ingest,
        workers: _workers,
        ..
    } = daemon();

    let mut refused_writes = 0;
    // The first refused write comes from outside; later ones are whatever
    // the previous flush exported.
    let mut arriving = 1;
    for _flush in 0..5 {
        let exported_before = exported_from_the_ingest_path(&capture);
        for _ in 0..arriving {
            ingest
                .write(refused_batch())
                .await
                .expect_err("the store refuses the batch");
            refused_writes += 1;
        }
        arriving = (exported_from_the_ingest_path(&capture) - exported_before).max(1);
    }

    let local = capture.local();
    let logged = local
        .events
        .iter()
        .filter(|event| event["message"] == "failed to persist ingest batch")
        .count();
    assert_eq!(refused_writes, 5, "one refused write per flush, no echo");
    assert_eq!(logged, refused_writes, "every refusal is on stderr");
    assert_eq!(exported_from_the_ingest_path(&capture), 0);
    // The daemon's other events are still exported.
    crate::events::server_stopped("grpc", &Ok::<(), std::io::Error>(()));
    let exported = capture.exported();
    assert_eq!(exported.len(), 1, "{exported:?}");
    assert_eq!(
        exported[0].event_name.as_deref(),
        Some("daemon.server.stopped")
    );
}

// The two tests below wait on tasks only, so they run on the paused
// clock: their bound costs no time, and a wait that never ends fails at
// once instead of after the bound.
#[tokio::test(start_paused = true)]
async fn a_failed_http_listener_stops_the_grpc_one() {
    let (grpc, grpc_gone) = serving();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        until_both_stop(grpc, failing("address in use"), &unwatched()),
    )
    .await
    .expect("the wait ends with the first failure");
    let error = format!("{:#}", outcome.expect_err("a failed listener is an error"));
    assert!(error.contains("HTTP server"), "{error}");
    assert!(error.contains("address in use"), "{error}");
    // The receiver errors because the task, and the sender in it, is gone.
    assert!(grpc_gone.await.is_err());
}

#[tokio::test(start_paused = true)]
async fn a_failed_grpc_listener_stops_the_http_one() {
    let (http, http_gone) = serving();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        until_both_stop(failing("no certificate"), http, &unwatched()),
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
    until_both_stop(stopped(), stopped(), &unwatched())
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

/// Longer than any drain here takes. The drain tests stay on the wall
/// clock: the writer works on its own thread, which a paused clock does
/// not wait for, so a bound would fire before the write it bounds.
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
///
/// The tests from here on run the whole daemon on real sockets and a
/// multi-threaded runtime, where the clock cannot be paused and nothing
/// signals "serving" or "all tasks gone": they poll, and their 30 s
/// bounds are only there to turn a hang into a failure.
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

/// Whether the daemon closes `client` without sending anything.
async fn closed_by_the_daemon(client: &mut tokio::net::TcpStream) -> bool {
    use tokio::io::AsyncReadExt;
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut byte));
    matches!(read.await, Ok(Ok(0) | Err(_)))
}

/// The gRPC listener is bound early and handed to the server; the
/// connection limits must still apply to it. With room for one connection
/// the second one is closed, and its place comes back when the first ends.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pre_bound_grpc_listener_enforces_the_connection_limits() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (grpc_port, http_port) = (free_port(), free_port());
    daemon_env(dir.path(), grpc_port, http_port);
    std::env::set_var("KRONIKA_GRPC_MAX_CONNECTIONS", "1");

    let mut daemon = tokio::spawn(serve());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while http_get(http_port, "/healthz").await.is_err() {
        assert!(!daemon.is_finished(), "daemon exited before serving");
        assert!(std::time::Instant::now() < deadline, "daemon not serving");
        tokio::task::yield_now().await;
    }

    let grpc = ("127.0.0.1", grpc_port);
    let first = tokio::net::TcpStream::connect(grpc).await.unwrap();
    let mut second = tokio::net::TcpStream::connect(grpc).await.unwrap();
    let second_closed = closed_by_the_daemon(&mut second).await;
    drop((first, second));
    // The freed place is taken by a connection that speaks HTTP/2: the
    // server answers its preface with a SETTINGS frame (type 4).
    let reused = async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let mut next = tokio::net::TcpStream::connect(grpc).await.unwrap();
            let preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\0\0\0\x04\0\0\0\0\0";
            let mut frame = [0u8; 9];
            if next.write_all(preface).await.is_ok() && next.read_exact(&mut frame).await.is_ok() {
                return frame[3];
            }
            tokio::task::yield_now().await;
        }
    };
    let settings = tokio::time::timeout(std::time::Duration::from_secs(10), reused).await;

    let status = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(std::process::id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), &mut daemon)
        .await
        .expect("daemon did not stop within 30s of SIGTERM")
        .expect("daemon task panicked");
    for name in [
        "KRONIKA_GRPC_MAX_CONNECTIONS",
        "TUMULT_LAKE_PATH",
        "KRONIKA_METRICS_DIR",
        "KRONIKA_OTLP_GRPC_ADDR",
        "KRONIKA_OTLP_HTTP_ADDR",
        "KRONIKA_LAKE_INTERVAL",
    ] {
        std::env::remove_var(name);
    }
    result.expect("daemon returned an error");
    assert!(second_closed, "a connection over the limit stayed open");
    assert_eq!(settings.ok(), Some(4), "the freed place was not reused");
}

/// The daemon stores entity events as entities only when the operator
/// sets the flag; the label allow-list comes from the environment too.
#[test]
fn entity_events_follow_the_environment_flag() {
    let config = config_with_tls(None, None);
    let mut env = tumult_test_utils::EnvGuard::lock();
    env.remove("KRONIKA_ENTITY_EVENTS")
        .remove("KRONIKA_ENTITY_LABEL_ALLOW");
    assert!(super::entity_events(&config).is_none(), "off by default");
    env.set("KRONIKA_ENTITY_EVENTS", "0");
    assert!(super::entity_events(&config).is_none());
    env.set("KRONIKA_ENTITY_EVENTS", "1")
        .set("KRONIKA_ENTITY_LABEL_ALLOW", "team, app.kubernetes.io/name");
    let on = super::entity_events(&config).expect("the flag switches entity events on");
    assert_eq!(
        on.options().label_allow,
        vec!["team", "app.kubernetes.io/name"]
    );
    assert_eq!(on.options().observer, "anonymous");
}
