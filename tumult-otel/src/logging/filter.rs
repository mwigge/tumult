//! Level filtering: the default directives and the OTLP-export guard.

use tracing::Level;
use tracing_subscriber::EnvFilter;

/// Crates whose events must never be exported over OTLP: they are the
/// exporter's own stack, so exporting them would make every export produce
/// more events to export.
const EXPORTER_INTERNAL_CRATES: [&str; 6] =
    ["opentelemetry", "tonic", "hyper", "h2", "tower", "reqwest"];

/// Dependencies that are capped at `warn` by the default filter. They log
/// per connection or per request at `info` and below.
const NOISY_CRATES: [&str; 7] = [
    "opentelemetry",
    "tonic",
    "hyper",
    "h2",
    "tower",
    "reqwest",
    "rustls",
];

/// Whether `target` belongs to the OTLP exporter's own stack
/// (`opentelemetry*`, `tonic`, `hyper*`, `h2`, `tower*`, `reqwest`).
///
/// Matches on the crate segment of the target, so `hyper_util::client` and
/// `opentelemetry-otlp` match while `h2o::server` does not.
#[must_use]
pub fn is_exporter_internal(target: &str) -> bool {
    let crate_name = target.split("::").next().unwrap_or(target);
    EXPORTER_INTERNAL_CRATES
        .iter()
        .any(|internal| is_crate_or_family(crate_name, internal))
}

/// `name` is `family` itself or a member such as `family_util`/`family-sdk`.
fn is_crate_or_family(name: &str, family: &str) -> bool {
    let Some(rest) = name.strip_prefix(family) else {
        return false;
    };
    rest.is_empty() || rest.starts_with(['_', '-'])
}

/// The filter directives used when `RUST_LOG` is unset: everything at
/// `level`, with the noisy dependencies capped at `warn` (or at `level`
/// when that is stricter).
#[must_use]
pub fn default_directives(level: Level) -> String {
    let level_name = level.as_str().to_ascii_lowercase();
    let cap = level.min(Level::WARN).as_str().to_ascii_lowercase();
    std::iter::once(level_name)
        .chain(NOISY_CRATES.iter().map(|noisy| format!("{noisy}={cap}")))
        .collect::<Vec<_>>()
        .join(",")
}

/// Builds the level filter: `rust_log` when it is set and parses, otherwise
/// [`default_directives`]. An unparseable value is returned as the second
/// element so the caller can report it; it is never silently ignored.
pub(crate) fn env_filter(
    rust_log: Option<&str>,
    default_level: Level,
) -> (EnvFilter, Option<String>) {
    let defaults = || EnvFilter::new(default_directives(default_level));
    let requested = rust_log.map(str::trim).filter(|value| !value.is_empty());
    let Some(value) = requested else {
        return (defaults(), None);
    };
    match EnvFilter::builder().parse(value) {
        Ok(filter) => (filter, None),
        Err(_) => (defaults(), Some(value.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{default_directives, env_filter, is_exporter_internal};
    use crate::logging::format::event_layer;
    use crate::logging::format::test_support::{formatter, Capture};
    use crate::logging::LogFormat;

    /// The messages that pass the filter built from `rust_log`.
    fn passed(rust_log: Option<&str>, emit: impl FnOnce()) -> Vec<String> {
        let capture = Capture::default();
        let (filter, _) = env_filter(rust_log, Level::INFO);
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(event_layer(
                formatter(LogFormat::Json),
                capture.clone(),
                false,
            ));
        tracing::subscriber::with_default(subscriber, emit);
        capture
            .text()
            .lines()
            .map(|line| {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                event["message"].as_str().unwrap().to_string()
            })
            .collect()
    }

    #[test]
    fn exporter_stack_targets_are_internal() {
        for target in [
            "opentelemetry",
            "opentelemetry_sdk",
            "opentelemetry-otlp",
            "opentelemetry_sdk::logs::batch_log_processor",
            "tonic::transport::channel",
            "hyper::client::conn",
            "hyper_util::client::legacy",
            "h2::codec::framed_write",
            "tower::buffer::worker",
            "tower_http::trace",
            "reqwest::connect",
        ] {
            assert!(is_exporter_internal(target), "{target}");
        }
    }

    #[test]
    fn workspace_and_lookalike_targets_are_not_internal() {
        for target in [
            "tumult_otel::telemetry",
            "tumult_api::auth::session",
            "tumultd",
            "h2o::server",
            "towering",
            "hyperion::core",
            "",
        ] {
            assert!(!is_exporter_internal(target), "{target}");
        }
    }

    #[test]
    fn default_directives_cap_noisy_dependencies_at_warn() {
        assert_eq!(
            default_directives(Level::INFO),
            "info,opentelemetry=warn,tonic=warn,hyper=warn,h2=warn,tower=warn,reqwest=warn,rustls=warn"
        );
    }

    #[test]
    fn a_stricter_default_level_is_never_loosened_for_dependencies() {
        assert_eq!(
            default_directives(Level::ERROR),
            "error,opentelemetry=error,tonic=error,hyper=error,h2=error,tower=error,reqwest=error,rustls=error"
        );
        assert!(default_directives(Level::DEBUG).starts_with("debug,opentelemetry=warn,"));
    }

    #[test]
    fn default_filter_passes_workspace_info_and_drops_debug() {
        let messages = passed(None, || {
            tracing::info!(target: "tumult_api::auth::session", "library info");
            tracing::debug!(target: "tumult_api::auth::session", "library debug");
            tracing::warn!(target: "tumult_core::runner", "library warn");
        });
        assert_eq!(messages, ["library info", "library warn"]);
    }

    #[test]
    fn default_filter_caps_noisy_dependencies_at_warn() {
        let messages = passed(None, || {
            tracing::info!(target: "hyper::proto::h1", "hyper info");
            tracing::info!(target: "opentelemetry_sdk", "sdk info");
            tracing::info!(target: "h2::codec", "h2 info");
            tracing::warn!(target: "tonic::transport", "tonic warn");
        });
        assert_eq!(messages, ["tonic warn"]);
    }

    #[test]
    fn rust_log_overrides_the_default() {
        let messages = passed(Some("warn,tumult_core=debug"), || {
            tracing::info!(target: "tumult_api", "api info");
            tracing::debug!(target: "tumult_core::runner", "core debug");
        });
        assert_eq!(messages, ["core debug"]);
    }

    #[test]
    fn empty_rust_log_counts_as_unset() {
        let (_, rejected) = env_filter(Some("  "), Level::INFO);
        assert_eq!(rejected, None);
        let messages = passed(
            Some(""),
            || tracing::info!(target: "tumult_api", "api info"),
        );
        assert_eq!(messages, ["api info"]);
    }

    #[test]
    fn invalid_rust_log_falls_back_to_the_default_and_is_reported() {
        let (_, rejected) = env_filter(Some("tumult_api=shouting"), Level::INFO);
        assert_eq!(rejected.as_deref(), Some("tumult_api=shouting"));
        let messages = passed(Some("tumult_api=shouting"), || {
            tracing::info!(target: "tumult_api", "api info");
            tracing::debug!(target: "tumult_api", "api debug");
        });
        assert_eq!(messages, ["api info"]);
    }
}
