use super::serve;
use super::tls::load_tls;
use super::ui::{ui_handler, UiAssets};
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
