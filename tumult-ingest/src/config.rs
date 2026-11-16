//! Daemon configuration from environment variables.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Runtime configuration. Every knob is an env var with a sensible default:
///
/// * `KRONIKA_OTLP_GRPC_ADDR` — OTLP/gRPC listen addr (default `0.0.0.0:4317`)
/// * `KRONIKA_OTLP_HTTP_ADDR` — OTLP/HTTP listen addr (default `0.0.0.0:4318`)
/// * `TUMULT_LAKE_PATH` — unified DuckDB store path (default `~/.tumult/lake.duckdb`);
///   `KRONIKA_DB` is honored as a deprecated alias with a warning
/// * `KRONIKA_METRICS_DIR` — semantic metric definitions (default `metrics/`)
/// * `KRONIKA_INGEST_TOKEN` — bearer token required on OTLP ingest
///   (`/v1/*` HTTP routes and the gRPC export methods); required whenever
///   either OTLP listener binds a non-loopback address (fail-closed, see
///   [`Config::ensure_ingest_auth`]), unset/empty means unauthenticated
///   ingest (loopback dev mode)
/// * `KRONIKA_ENTITY_EVENTS` — `1` or `true` stores OpenTelemetry entity
///   events as entities instead of log records (off by default).
///   `KRONIKA_ENTITY_LABEL_ALLOW` lists the Kubernetes label names kept on
///   them, `KRONIKA_ENTITY_ATTRIBUTE_ALLOW` the description keys kept
///   whatever they are (a whole key, or the start of one followed by `*`;
///   an entry that names no namespace is honoured with a warning at
///   start), and `KRONIKA_ENTITY_MAX_PER_ENVIRONMENT` is the most entities
///   one environment may hold (default 50 000, the lake four times as
///   many; anything but a positive integer keeps the default with a
///   warning). All four are read once, at start, by
///   [`crate::EntityEvents::from_env`]
/// * `KRONIKA_TLS_CERT` / `KRONIKA_TLS_KEY` — PEM certificate chain and
///   private key enabling TLS on both the HTTP (axum) and gRPC (tonic)
///   servers; both must be set together, unset means plaintext
#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: PathBuf,
    pub otlp_grpc_addr: SocketAddr,
    pub otlp_http_addr: SocketAddr,
    pub metrics_dir: PathBuf,
    pub ingest_token: Option<String>,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
}

impl Config {
    /// Load configuration from the environment.
    ///
    /// # Panics / errors
    /// Returns an error string if an address fails to parse or the home
    /// directory cannot be determined for the default DB path.
    pub fn from_env() -> Result<Self, String> {
        for name in ["KRONIKA_RETENTION_DAYS", "TUMULTD_RUN_RETENTION_DAYS"] {
            crate::retention::validate_policy(name, std::env::var(name).ok().as_deref())?;
        }
        let db_path = match std::env::var("TUMULT_LAKE_PATH") {
            Ok(p) if !p.is_empty() => PathBuf::from(p),
            _ => match std::env::var("KRONIKA_DB") {
                Ok(p) if !p.is_empty() => {
                    tracing::warn!(
                        "KRONIKA_DB is deprecated; tumultd now uses the unified \
                         store at TUMULT_LAKE_PATH (default ~/.tumult/lake.duckdb). Migrate \
                         with `tumult store import-legacy` and unset KRONIKA_DB."
                    );
                    PathBuf::from(p)
                }
                _ => dirs_next::home_dir()
                    .ok_or("cannot determine home directory; set TUMULT_LAKE_PATH")?
                    .join(".tumult")
                    .join("lake.duckdb"),
            },
        };
        let otlp_grpc_addr = std::env::var("KRONIKA_OTLP_GRPC_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:4317".into())
            .parse()
            .map_err(|e| format!("invalid KRONIKA_OTLP_GRPC_ADDR: {e}"))?;
        let otlp_http_addr = std::env::var("KRONIKA_OTLP_HTTP_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:4318".into())
            .parse()
            .map_err(|e| format!("invalid KRONIKA_OTLP_HTTP_ADDR: {e}"))?;
        let metrics_dir = std::env::var("KRONIKA_METRICS_DIR")
            .map_or_else(|_| PathBuf::from("metrics"), PathBuf::from);
        let ingest_token = std::env::var("KRONIKA_INGEST_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());
        let (tls_cert, tls_key) = tls_from_env()?;
        Ok(Self {
            db_path,
            otlp_grpc_addr,
            otlp_http_addr,
            metrics_dir,
            ingest_token,
            tls_cert,
            tls_key,
        })
    }

    /// Fail-closed ingest bind guard: refuse to start when either OTLP
    /// listener (gRPC or HTTP) binds a non-loopback address without
    /// `KRONIKA_INGEST_TOKEN` — an unauthenticated network-exposed ingest
    /// accepts telemetry from anyone (spoofing, store pollution). Loopback
    /// binds stay open for local development.
    pub fn ensure_ingest_auth(&self) -> Result<(), String> {
        if self.ingest_token.is_some() {
            return Ok(());
        }
        for (env, addr) in [
            ("KRONIKA_OTLP_GRPC_ADDR", self.otlp_grpc_addr),
            ("KRONIKA_OTLP_HTTP_ADDR", self.otlp_http_addr),
        ] {
            if !addr.ip().is_loopback() {
                return Err(format!(
                    "refusing to serve unauthenticated OTLP ingest on non-loopback address \
                     {addr} ({env}): set KRONIKA_INGEST_TOKEN to require a bearer token, or \
                     bind {env} to 127.0.0.1 for local-only access"
                ));
            }
        }
        Ok(())
    }
}

/// Read the optional TLS certificate/key pair from the environment. Both
/// `KRONIKA_TLS_CERT` and `KRONIKA_TLS_KEY` must be set together — a lone
/// value is a configuration error, never a silent fallback to plaintext.
fn tls_from_env() -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
    let cert = std::env::var("KRONIKA_TLS_CERT")
        .ok()
        .filter(|p| !p.is_empty());
    let key = std::env::var("KRONIKA_TLS_KEY")
        .ok()
        .filter(|p| !p.is_empty());
    match (cert, key) {
        (Some(cert), Some(key)) => Ok((Some(PathBuf::from(cert)), Some(PathBuf::from(key)))),
        (None, None) => Ok((None, None)),
        _ => Err(
            "KRONIKA_TLS_CERT and KRONIKA_TLS_KEY must be set together (PEM certificate \
             chain and private key); refusing to start with only one of them"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use tumult_test_utils::EnvGuard;

    use super::*;

    /// The whole env surface in one pass, under the guard that keeps other
    /// tests of this binary out of the environment meanwhile.
    #[test]
    fn db_path_resolution_order() {
        let mut env = EnvGuard::lock();
        env.remove("KRONIKA_OTLP_GRPC_ADDR");
        env.remove("KRONIKA_OTLP_HTTP_ADDR");
        env.remove("TUMULT_LAKE_PATH");
        env.remove("KRONIKA_DB");
        env.remove("KRONIKA_INGEST_TOKEN");
        env.remove("KRONIKA_TLS_CERT");
        env.remove("KRONIKA_TLS_KEY");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.otlp_grpc_addr.port(), 4317);
        assert_eq!(cfg.otlp_http_addr.port(), 4318);
        assert!(cfg.db_path.ends_with("lake.duckdb"));
        assert_eq!(cfg.ingest_token, None);

        env.set("TUMULT_LAKE_PATH", "/tmp/unified.duckdb");
        env.set("KRONIKA_DB", "/tmp/legacy.duckdb");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.db_path, PathBuf::from("/tmp/unified.duckdb"));
        env.remove("TUMULT_LAKE_PATH");
        // The deprecated alias still resolves when TUMULT_LAKE_PATH is unset.
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.db_path, PathBuf::from("/tmp/legacy.duckdb"));
        env.remove("KRONIKA_DB");

        // KRONIKA_INGEST_TOKEN: empty string behaves as unset.
        env.set("KRONIKA_INGEST_TOKEN", "");
        assert_eq!(Config::from_env().unwrap().ingest_token, None);
        env.set("KRONIKA_INGEST_TOKEN", "kro_secret");
        assert_eq!(
            Config::from_env().unwrap().ingest_token.as_deref(),
            Some("kro_secret")
        );
        env.remove("KRONIKA_INGEST_TOKEN");

        // KRONIKA_TLS_CERT / KRONIKA_TLS_KEY: both or neither.
        assert_eq!(Config::from_env().unwrap().tls_cert, None);
        env.set("KRONIKA_TLS_CERT", "/etc/tumult/tls.crt");
        assert!(Config::from_env().unwrap_err().contains("TLS_CERT"));
        env.set("KRONIKA_TLS_KEY", "/etc/tumult/tls.key");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.tls_cert, Some(PathBuf::from("/etc/tumult/tls.crt")));
        assert_eq!(cfg.tls_key, Some(PathBuf::from("/etc/tumult/tls.key")));
        env.remove("KRONIKA_TLS_CERT");
        assert!(Config::from_env().unwrap_err().contains("TLS_CERT"));
    }

    /// Build a config directly (no env) for the bind-guard tests.
    fn config(addrs: [&str; 2], token: Option<&str>) -> Config {
        Config {
            db_path: PathBuf::from("/tmp/db.duckdb"),
            otlp_grpc_addr: addrs[0].parse().unwrap(),
            otlp_http_addr: addrs[1].parse().unwrap(),
            metrics_dir: PathBuf::from("metrics"),
            ingest_token: token.map(str::to_string),
            tls_cert: None,
            tls_key: None,
        }
    }

    #[test]
    fn ingest_auth_guard_fails_closed_on_non_loopback() {
        // Unspecified (0.0.0.0) and routable binds without a token: refused.
        let err = config(["0.0.0.0:4317", "0.0.0.0:4318"], None)
            .ensure_ingest_auth()
            .unwrap_err();
        assert!(err.contains("KRONIKA_INGEST_TOKEN"), "{err}");
        assert!(config(["127.0.0.1:4317", "0.0.0.0:4318"], None)
            .ensure_ingest_auth()
            .is_err());
        assert!(config(["0.0.0.0:4317", "127.0.0.1:4318"], None)
            .ensure_ingest_auth()
            .is_err());

        // A token opens the guard on any bind.
        assert!(config(["0.0.0.0:4317", "0.0.0.0:4318"], Some("kro_secret"))
            .ensure_ingest_auth()
            .is_ok());

        // Loopback binds stay open for dev, with or without a token.
        assert!(config(["127.0.0.1:4317", "127.0.0.1:4318"], None)
            .ensure_ingest_auth()
            .is_ok());
        assert!(
            config(["127.0.0.1:4317", "127.0.0.1:4318"], Some("kro_secret"))
                .ensure_ingest_auth()
                .is_ok()
        );
    }
}
