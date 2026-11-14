//! Log events and logging identity shared by the daemon's modules.

use std::error::Error;

use tumult_ingest::Config;
use tumult_otel::logging::{self, err_sources, error_type, LogOptions};

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

/// The modules that receive and store OTLP records. The daemon exports its
/// own log events to its own ingest, so an error logged here while a record
/// is being stored would be exported, received, fail the same way and be
/// logged again, without end. Their events stay on stderr. The listener
/// that admits the connections, and the guard that rate-limits what these
/// modules log, are on the same path: a refused connection of the daemon's
/// own exporter would otherwise be exported over the next one.
pub(crate) const INGEST_PATH: [&str; 7] = [
    "tumult_ingest::writer",
    "tumult_ingest::grpc",
    "tumult_ingest::http",
    "tumult_ingest::otap",
    "tumult_ingest::listener",
    "tumult_ingest::log_guard",
    "tumult_lake::writer",
];

/// How the daemon logs: at `info`, with the [`INGEST_PATH`] kept out of
/// its own export.
pub(crate) fn log_options(service_name: &str) -> LogOptions {
    LogOptions::new(service_name).with_local_only_targets(INGEST_PATH)
}

/// A fatal error that has already been logged as `process.fatal`. It reads
/// exactly like the error it wraps.
#[derive(Debug)]
struct Reported(anyhow::Error);

impl std::fmt::Display for Reported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for Reported {
    // The wrapped error's causes stay this error's causes, so `{:#}` and
    // `chain()` on the wrapper walk the same chain.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}

/// Logs `error` as the `process.fatal` event now, and marks it so that
/// [`fatal`] does not log it a second time.
///
/// For the failure that is detected while the daemon still exports its
/// telemetry: logged here, before the telemetry is flushed and shut down,
/// the event reaches the store as well as stderr.
pub(crate) fn report_fatal(error: anyhow::Error) -> anyhow::Error {
    logging::log_fatal(&error);
    anyhow::Error::new(Reported(error))
}

/// Whether `error` went through [`report_fatal`].
pub(crate) fn fatal_was_reported(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Reported>().is_some()
}

/// Logs the error that ends the process as `process.fatal`, unless
/// [`report_fatal`] already did.
pub(crate) fn fatal(error: &anyhow::Error) {
    if fatal_was_reported(error) {
        return;
    }
    logging::log_fatal(error);
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

    use super::{
        fatal, fatal_was_reported, log_options, report_fatal, server_stopped, service_name_from,
        starting, INGEST_PATH, SERVICE_NAME,
    };

    fn store_locked() -> anyhow::Error {
        anyhow::anyhow!("locked by pid 7").context("open store")
    }

    #[test]
    fn the_daemon_logs_at_info_and_names_itself() {
        let options = log_options("tumultd-eu");
        assert_eq!(options.service_name, "tumultd-eu");
        assert_eq!(options.default_level, Level::INFO);
        assert_eq!(options.format, None);
    }

    #[test]
    fn the_ingest_path_is_kept_out_of_the_daemons_own_export() {
        assert_eq!(log_options("tumultd").local_only_targets, INGEST_PATH);
        for module in ["writer", "grpc", "http", "otap", "listener", "log_guard"] {
            let target = format!("tumult_ingest::{module}");
            assert!(INGEST_PATH.contains(&target.as_str()), "{target}");
        }
        assert!(INGEST_PATH.contains(&"tumult_lake::writer"));
    }

    #[test]
    fn a_fatal_error_is_logged_once_with_its_cause_chain() {
        let logs = capture_json(Level::INFO, || fatal(&store_locked()));
        let events = logs.named("process.fatal");
        assert_eq!(events.len(), 1, "{}", logs.raw);
        assert_eq!(events[0]["error.message"], "open store: locked by pid 7");
    }

    #[test]
    fn a_fatal_error_reported_early_is_not_logged_again() {
        let logs = capture_json(Level::INFO, || {
            let reported = report_fatal(store_locked());
            assert!(fatal_was_reported(&reported));
            fatal(&reported);
        });
        let events = logs.named("process.fatal");
        assert_eq!(events.len(), 1, "{}", logs.raw);
        assert_eq!(events[0]["error.message"], "open store: locked by pid 7");
    }

    #[test]
    fn a_reported_fatal_error_reads_like_the_error_it_wraps() {
        let reported = capture_json(Level::INFO, || {
            let reported = report_fatal(store_locked());
            assert_eq!(format!("{reported:#}"), "open store: locked by pid 7");
            assert_eq!(reported.to_string(), "open store");
        });
        assert_eq!(reported.events.len(), 1);
        assert!(!fatal_was_reported(&store_locked()));
    }

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
