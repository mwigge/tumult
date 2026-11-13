//! Log events and logging identity shared by the daemon's modules.

use std::error::Error;

use tumult_ingest::Config;
use tumult_otel::logging::{err_sources, error_type};

/// The daemon's `service.name` unless `OTEL_SERVICE_NAME` overrides it.
pub(crate) const SERVICE_NAME: &str = "tumultd";

/// The `service.name` for both the log lines and the OpenTelemetry resource.
pub(crate) fn service_name() -> String {
    service_name_from(std::env::var("OTEL_SERVICE_NAME").ok())
}

fn service_name_from(configured: Option<String>) -> String {
    configured
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| SERVICE_NAME.to_string())
}

/// Announces the daemon's store and listeners before anything binds.
pub(crate) fn starting(config: &Config, tls_enabled: bool) {
    tracing::info!(
        name: "daemon.starting",
        {
            file.path = %config.db_path.display(),
            server.grpc.address = %config.otlp_grpc_addr.ip(),
            server.grpc.port = config.otlp_grpc_addr.port(),
            server.http.address = %config.otlp_http_addr.ip(),
            server.http.port = config.otlp_http_addr.port(),
            tls.enabled = tls_enabled,
        },
        "starting tumultd"
    );
}

/// Reports how a server future ended: a normal stop at info, a failure at
/// error with the cause. `task` is the server's short name (`grpc`, `http`).
pub(crate) fn server_stopped<E: Error>(task: &'static str, result: &Result<(), E>) {
    let Err(error) = result else {
        tracing::info!(
            name: "daemon.server.stopped",
            { resilience.task.name = task },
            "server stopped"
        );
        return;
    };
    tracing::error!(
        name: "daemon.server.failed",
        {
            resilience.task.name = task,
            error.message = %err_sources(error),
            "error.type" = error_type(error),
        },
        "server failed"
    );
}

#[cfg(test)]
mod tests {
    use tracing::Level;
    use tumult_otel::logging::testing::capture_json;

    use super::{server_stopped, service_name_from, starting, SERVICE_NAME};

    #[test]
    fn service_name_defaults_to_the_binary_name() {
        assert_eq!(service_name_from(None), SERVICE_NAME);
        assert_eq!(service_name_from(Some("  ".into())), SERVICE_NAME);
        assert_eq!(service_name_from(Some("tumultd-eu".into())), "tumultd-eu");
    }

    #[test]
    fn clean_server_stop_is_an_info_event_naming_the_task() {
        let logs = capture_json(Level::INFO, || {
            server_stopped("grpc", &Ok::<(), std::io::Error>(()));
        });
        assert_eq!(logs.events.len(), 1);
        let event = &logs.events[0];
        assert_eq!(event["event.name"], "daemon.server.stopped");
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["message"], "server stopped");
        assert_eq!(event["resilience.task.name"], "grpc");
        assert!(event.get("error.message").is_none());
    }

    #[test]
    fn failed_server_is_an_error_event_with_the_cause() {
        let failure = std::io::Error::new(std::io::ErrorKind::AddrInUse, "address in use");
        let logs = capture_json(Level::INFO, || {
            server_stopped("https", &Err::<(), _>(failure));
        });
        assert_eq!(logs.events.len(), 1);
        let event = &logs.events[0];
        assert_eq!(event["event.name"], "daemon.server.failed");
        assert_eq!(event["level"], "ERROR");
        assert_eq!(event["message"], "server failed");
        assert_eq!(event["resilience.task.name"], "https");
        assert_eq!(event["error.message"], "address in use");
        assert_eq!(event["error.type"], "Error");
    }

    #[test]
    fn failed_server_event_keeps_the_underlying_cause() {
        #[derive(Debug)]
        struct Transport(std::io::Error);
        impl std::fmt::Display for Transport {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("transport error")
            }
        }
        impl std::error::Error for Transport {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let failure = Transport(std::io::Error::other("address in use"));
        let logs = capture_json(Level::INFO, || {
            server_stopped("grpc", &Err::<(), _>(failure));
        });
        let event = &logs.events[0];
        assert_eq!(event["error.message"], "transport error: address in use");
        assert_eq!(event["error.type"], "Transport");
    }

    #[test]
    fn starting_event_splits_listener_addresses_into_host_and_port() {
        let config = tumult_ingest::Config {
            db_path: "/var/lib/tumult/lake.duckdb".into(),
            otlp_grpc_addr: "127.0.0.1:4317".parse().unwrap(),
            otlp_http_addr: "0.0.0.0:4318".parse().unwrap(),
            metrics_dir: "metrics".into(),
            ingest_token: Some("kro_secret-ingest-token".into()),
            tls_cert: None,
            tls_key: None,
        };
        let logs = capture_json(Level::INFO, || starting(&config, true));
        assert_eq!(logs.events.len(), 1);
        let event = &logs.events[0];
        assert_eq!(event["event.name"], "daemon.starting");
        assert_eq!(event["message"], "starting tumultd");
        assert_eq!(event["file.path"], "/var/lib/tumult/lake.duckdb");
        assert_eq!(event["server.grpc.address"], "127.0.0.1");
        assert_eq!(event["server.grpc.port"], 4317);
        assert_eq!(event["server.http.address"], "0.0.0.0");
        assert_eq!(event["server.http.port"], 4318);
        assert_eq!(event["tls.enabled"], true);
        assert!(!logs.raw.contains("kro_secret-ingest-token"));
    }
}
