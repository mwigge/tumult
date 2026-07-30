//! Daemon configuration from environment variables.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Runtime configuration. Every knob is an env var with a sensible default:
///
/// * `KRONIKA_OTLP_GRPC_ADDR` — OTLP/gRPC listen addr (default `0.0.0.0:4317`)
/// * `KRONIKA_OTLP_HTTP_ADDR` — OTLP/HTTP listen addr (default `0.0.0.0:4318`)
/// * `KRONIKA_DB` — DuckDB store path (default `~/.kronika/kronika.duckdb`)
/// * `KRONIKA_METRICS_DIR` — semantic metric definitions (default `metrics/`)
#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: PathBuf,
    pub otlp_grpc_addr: SocketAddr,
    pub otlp_http_addr: SocketAddr,
    pub metrics_dir: PathBuf,
}

impl Config {
    /// Load configuration from the environment.
    ///
    /// # Panics / errors
    /// Returns an error string if an address fails to parse or the home
    /// directory cannot be determined for the default DB path.
    pub fn from_env() -> Result<Self, String> {
        let db_path = match std::env::var("KRONIKA_DB") {
            Ok(p) if !p.is_empty() => PathBuf::from(p),
            _ => dirs_next::home_dir()
                .ok_or("cannot determine home directory; set KRONIKA_DB")?
                .join(".kronika")
                .join("kronika.duckdb"),
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
        Ok(Self {
            db_path,
            otlp_grpc_addr,
            otlp_http_addr,
            metrics_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply() {
        // Env isolation: only safe because the test binary is single-purpose
        // here; assert on a Config built from explicit values instead.
        std::env::remove_var("KRONIKA_OTLP_GRPC_ADDR");
        std::env::remove_var("KRONIKA_OTLP_HTTP_ADDR");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.otlp_grpc_addr.port(), 4317);
        assert_eq!(cfg.otlp_http_addr.port(), 4318);
        assert!(cfg.db_path.ends_with("kronika.duckdb"));
    }
}
