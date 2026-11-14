//! Structured logging: the one place a `tracing` subscriber is installed.
//!
//! Every Tumult binary calls [`init`] once, first thing in `main`. It
//! installs a single global subscriber made of:
//!
//! * a level filter — `RUST_LOG` when set, otherwise the binary's default
//!   level with the chattiest dependencies capped at `warn`;
//! * an event formatter writing to **stderr** — JSON by default, text when
//!   stderr is a terminal, overridable with `TUMULT_LOG_FORMAT=json|text`;
//! * the `tracing-opentelemetry` layer, which keeps the OpenTelemetry
//!   context active across `tracing` spans;
//! * the OTLP log bridge, which starts empty and is filled in by
//!   [`TumultTelemetry::new`](crate::TumultTelemetry::new) once the logger
//!   provider exists, and which never exports the exporter's own events.
//!
//! stdout is never written to: it belongs to command output and to the MCP
//! JSON-RPC stream.
//!
//! See `docs/guides/logging.md` for the field vocabulary and event naming.

mod bridge;
mod capped;
mod error;
mod fields;
mod filter;
mod format;
mod limit;
mod sanitize;
mod selection;
pub mod testing;

use std::io::{IsTerminal, Write};
use std::sync::OnceLock;

use opentelemetry_sdk::logs::SdkLoggerProvider;
use serde_json::Value;
use tracing::{Level, Subscriber};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use bridge::LogBridgeSlot;
use format::DirectEvent;

pub use bridge::AttachError;
pub use error::{err, err_sources, error_type, ErrorChain, SourceChain};
pub use fields::{bounded, TypedFields, MAX_FIELD_BYTES};
pub use filter::{default_directives, is_exporter_internal};
pub use format::{EventFormatter, LogFormat, LogResource};
pub use sanitize::{redact_url, sanitize_url, server_address, ServerAddress, REDACTED_URL};
pub use selection::{choose_format, FormatChoice, FORMAT_ENV};

/// Environment variable naming the deployment environment (`prod`, `dev`,
/// ...) when `OTEL_RESOURCE_ATTRIBUTES` does not carry
/// `deployment.environment.name`.
pub const ENVIRONMENT_ENV: &str = "TUMULT_ENVIRONMENT";

/// Resource attribute, and JSON log key, for the deployment environment.
pub const ENVIRONMENT_ATTRIBUTE: &str = "deployment.environment.name";

const RUST_LOG_ENV: &str = "RUST_LOG";
const NO_COLOR_ENV: &str = "NO_COLOR";
const RESOURCE_ATTRIBUTES_ENV: &str = "OTEL_RESOURCE_ATTRIBUTES";
const BACKTRACE_ENV: &str = "RUST_BACKTRACE";

const FATAL_EVENT: &str = "process.fatal";
const FATAL_MESSAGE: &str = "exiting because of an unrecoverable error";
const PANIC_EVENT: &str = "process.panic";
const PANIC_MESSAGE: &str = "process panicked";

/// The version stamped on log lines and on the OpenTelemetry resource.
pub(crate) const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the [`init`] call that installed the global subscriber left behind.
/// Set exactly once.
static INSTALLED: OnceLock<Installed> = OnceLock::new();

struct Installed {
    /// The bridge slot of the installed subscriber.
    bridge: LogBridgeSlot,
    /// The identity stamped on lines written past the subscriber.
    resource: LogResource,
}

/// What a binary decides about its logging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogOptions {
    /// Emitted as `service.name` on every JSON line.
    pub service_name: String,
    /// Level used when `RUST_LOG` is unset. Dependencies known to be noisy
    /// are capped at `warn` regardless.
    pub default_level: Level,
    /// Forces a format. `None` (the default) honours `TUMULT_LOG_FORMAT`
    /// and otherwise picks text for a terminal and JSON for everything else.
    pub format: Option<LogFormat>,
    /// Modules (each with its submodules) whose events are written to
    /// stderr but never exported over OTLP. See
    /// [`with_local_only_targets`](Self::with_local_only_targets).
    pub local_only_targets: Vec<String>,
    /// Most bytes of events written to stderr over the life of the process;
    /// `None` (the default) for no limit. See
    /// [`with_output_limit`](Self::with_output_limit).
    pub output_limit: Option<u64>,
}

impl LogOptions {
    /// Options for `service_name`: `info` level, automatic format.
    #[must_use]
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            default_level: Level::INFO,
            format: None,
            local_only_targets: Vec::new(),
            output_limit: None,
        }
    }

    /// Options for a command-line tool (`tumult`, `tumult-mcp`): named after
    /// `config.service_name`, and quiet by default. Without an OTLP endpoint
    /// a tool's `info` events have no collector to go to and would only
    /// clutter the terminal, so the default level is `warn` until an
    /// endpoint is configured. `RUST_LOG` overrides either default.
    #[must_use]
    pub fn for_tool(config: &crate::config::TelemetryConfig) -> Self {
        let level = if config.otlp_endpoint.is_some() {
            Level::INFO
        } else {
            Level::WARN
        };
        Self::new(config.service_name.clone()).with_default_level(level)
    }

    /// Sets the level used when `RUST_LOG` is unset.
    #[must_use]
    pub fn with_default_level(mut self, level: Level) -> Self {
        self.default_level = level;
        self
    }

    /// Forces the format, ignoring `TUMULT_LOG_FORMAT` and the terminal check.
    #[must_use]
    pub fn with_format(mut self, format: LogFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Keeps the events of `targets` (module paths, each covering its
    /// submodules) out of the OTLP export. They are still written to stderr.
    ///
    /// A process that receives its own export needs this for the modules on
    /// the receiving path: an error logged there while storing a record
    /// would be exported, received, fail the same way and be logged again,
    /// without end.
    #[must_use]
    pub fn with_local_only_targets<I, T>(mut self, targets: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        self.local_only_targets = targets.into_iter().map(Into::into).collect();
        self
    }

    /// Stops writing events to stderr once `bytes` of them have been
    /// written. Events are kept or dropped whole, and once one has been
    /// dropped none follows, so what was written is a prefix of the log.
    /// The OTLP export is not affected.
    ///
    /// For a detached process whose stderr is a file nobody rotates: without
    /// a limit that file grows for as long as the process runs.
    #[must_use]
    pub fn with_output_limit(mut self, bytes: u64) -> Self {
        self.output_limit = Some(bytes);
        self
    }
}

/// Why [`init`] did not install the subscriber.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    /// [`init`] already succeeded in this process.
    #[error("logging is already initialised")]
    AlreadyInitialized,
    /// Another global subscriber (or `log` logger) is installed.
    #[error("could not install the global tracing subscriber: {0}")]
    Install(String),
}

/// Installs the process-global `tracing` subscriber. Call it once, first
/// thing in `main`; see the [module documentation](self) for what it
/// installs.
///
/// Also installs a panic hook that reports panics as a `process.panic`
/// error event instead of the default unstructured message. The hook that
/// was installed before runs after it when `RUST_BACKTRACE` asks for a
/// backtrace, so the backtrace follows the event.
///
/// # Errors
///
/// [`InitError::AlreadyInitialized`] on a second call, and
/// [`InitError::Install`] when something else already installed a global
/// subscriber. Nothing is installed or replaced in either case.
pub fn init(options: LogOptions) -> Result<(), InitError> {
    if is_initialized() {
        return Err(InitError::AlreadyInitialized);
    }
    let stderr_is_terminal = std::io::stderr().is_terminal();
    let choice = match options.format {
        Some(format) => FormatChoice {
            format,
            rejected: None,
        },
        None => choose_format(env_value(FORMAT_ENV).as_deref(), stderr_is_terminal),
    };
    let (filter, rejected_filter) =
        filter::env_filter(env_value(RUST_LOG_ENV).as_deref(), options.default_level);
    let resource = process_resource(options.service_name);
    let pipeline = Pipeline {
        formatter: EventFormatter::new(choice.format, resource.clone()),
        filter,
        writer: std::io::stderr,
        ansi: stderr_is_terminal && std::env::var_os(NO_COLOR_ENV).is_none(),
        local_only: options.local_only_targets,
        output_limit: options.output_limit,
    };
    let (subscriber, bridge) = pipeline.build();
    subscriber
        .try_init()
        .map_err(|e| InitError::Install(e.to_string()))?;
    INSTALLED
        .set(Installed { bridge, resource })
        .map_err(|_| InitError::AlreadyInitialized)?;
    install_panic_hook();
    report_rejected_settings(choice.rejected.as_deref(), rejected_filter.as_deref());
    Ok(())
}

/// Reports that [`init`] failed.
///
/// With no subscriber there is nothing to log through, so this writes one
/// JSON line — shaped like every other log line — straight to stderr.
pub fn report_init_failure(service_name: &str, error: &InitError) {
    let resource = process_resource(service_name.to_string());
    write_to_stderr(&init_failure_line(&resource, error, chrono::Utc::now()));
}

fn init_failure_line(
    resource: &LogResource,
    error: &InitError,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let event = DirectEvent {
        name: "logging.init.failed",
        message: "logging could not be initialised",
        fields: vec![
            ("error.type", Value::from(error_type(error))),
            ("error.message", capped_value(&error.to_string())),
        ],
    };
    format::error_line(resource, now, module_path!(), event)
}

/// Logs the error that is about to end the process as a `process.fatal`
/// error event. Binaries call this instead of letting `main` return the
/// error, which would print it unstructured.
///
/// The event is never lost to the log filter: when the filter would drop
/// it (`RUST_LOG=off`, or a filter that names other crates only), the same
/// line is written to stderr directly.
pub fn log_fatal<E: std::fmt::Display + ?Sized>(error: &E) {
    let message = err(error).to_string();
    if emit_fatal(&message) {
        return;
    }
    write_past_the_subscriber(DirectEvent {
        name: FATAL_EVENT,
        message: FATAL_MESSAGE,
        fields: vec![("error.message", capped_value(&message))],
    });
}

/// Emits the `process.fatal` event through the subscriber. Returns whether
/// the subscriber takes error events from here; `false` means nothing was
/// written.
fn emit_fatal(message: &str) -> bool {
    if !tracing::enabled!(Level::ERROR) {
        return false;
    }
    tracing::error!(
        name: "process.fatal",
        { error.message = message },
        "exiting because of an unrecoverable error"
    );
    true
}

/// Writes `event` as one JSON line straight to stderr, for an event the
/// subscriber would filter away.
fn write_past_the_subscriber(event: DirectEvent) {
    let now = chrono::Utc::now();
    let Some(installed) = INSTALLED.get() else {
        let unknown = process_resource("unknown_service".to_string());
        write_to_stderr(&format::error_line(&unknown, now, module_path!(), event));
        return;
    };
    write_to_stderr(&format::error_line(
        &installed.resource,
        now,
        module_path!(),
        event,
    ));
}

fn write_to_stderr(line: &str) {
    // A failed write means stderr itself is gone: there is nowhere left to
    // report anything to.
    let _ = writeln!(std::io::stderr().lock(), "{line}");
}

/// `text` as a JSON string, capped like every formatted field.
fn capped_value(text: &str) -> Value {
    Value::from(bounded(text).as_ref())
}

/// Whether [`init`] has installed the subscriber in this process.
#[must_use]
pub fn is_initialized() -> bool {
    INSTALLED.get().is_some()
}

/// Starts mirroring log events to `provider` over OTLP.
///
/// # Errors
///
/// [`AttachError::NotInitialized`] before [`init`], and
/// [`AttachError::AlreadyAttached`] when a provider is already attached.
pub(crate) fn attach_log_bridge(provider: &SdkLoggerProvider) -> Result<(), AttachError> {
    INSTALLED
        .get()
        .ok_or(AttachError::NotInitialized)?
        .bridge
        .attach(provider)
}

/// Stops mirroring log events over OTLP, before the logger provider is shut
/// down. A no-op when [`init`] has not installed the subscriber.
pub(crate) fn detach_log_bridge() {
    let Some(installed) = INSTALLED.get() else {
        return;
    };
    installed.bridge.detach();
}

/// The deployment environment for this process, if configured.
#[must_use]
pub fn deployment_environment() -> Option<String> {
    environment_from(
        env_value(RESOURCE_ATTRIBUTES_ENV).as_deref(),
        env_value(ENVIRONMENT_ENV).as_deref(),
    )
}

/// Resolves the deployment environment: `deployment.environment.name` in
/// `OTEL_RESOURCE_ATTRIBUTES` (`k=v,k2=v2`) wins, then `TUMULT_ENVIRONMENT`.
fn environment_from(
    resource_attributes: Option<&str>,
    tumult_environment: Option<&str>,
) -> Option<String> {
    let from_resource = resource_attributes.and_then(|attributes| {
        attributes
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| key.trim() == ENVIRONMENT_ATTRIBUTE)
            .map(|(_, value)| value)
    });
    from_resource
        .into_iter()
        .chain(tumult_environment)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn env_value(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn process_resource(service_name: String) -> LogResource {
    LogResource {
        service_name,
        service_version: SERVICE_VERSION.to_string(),
        environment: deployment_environment(),
    }
}

/// The parts of the subscriber that differ between production and tests.
struct Pipeline<W> {
    formatter: EventFormatter,
    filter: EnvFilter,
    writer: W,
    ansi: bool,
    /// Modules whose events are not exported; see
    /// [`LogOptions::with_local_only_targets`].
    local_only: Vec<String>,
    /// Byte budget of the writer; see [`LogOptions::with_output_limit`].
    output_limit: Option<u64>,
}

impl<W> Pipeline<W>
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    /// Composes the subscriber and returns it with its (empty) bridge slot.
    fn build(self) -> (impl Subscriber + Send + Sync + 'static, LogBridgeSlot) {
        let slot = LogBridgeSlot::default();
        let subscriber = tracing_subscriber::registry()
            .with(self.filter)
            .with(format::event_layer(
                self.formatter,
                limit::Limited::new(self.writer, self.output_limit),
                self.ansi,
            ))
            .with(tracing_opentelemetry::layer())
            .with(slot.layer_keeping_local(self.local_only));
        (subscriber, slot)
    }
}

/// Logs one warning for each setting that was rejected during [`init`].
/// Runs after the subscriber is installed, so the warnings are structured.
fn report_rejected_settings(format: Option<&str>, filter: Option<&str>) {
    if let Some(value) = format {
        tracing::warn!(
            name: "logging.format.invalid",
            { config.key = FORMAT_ENV, config.value = value },
            "unknown log format; expected json or text, using json"
        );
    }
    if let Some(value) = filter {
        tracing::warn!(
            name: "logging.filter.invalid",
            { config.key = RUST_LOG_ENV, config.value = value },
            "unparseable log filter; using the default filter"
        );
    }
}

/// Replaces the panic hook with one that reports the panic as a
/// `process.panic` event, then hands over to the hook it replaced when a
/// backtrace was asked for.
///
/// The previous hook (normally the standard one) prints the unstructured
/// `thread ... panicked at` message. That is noise next to the event, so it
/// only runs when `RUST_BACKTRACE` is set, where it is the one thing able to
/// print the backtrace.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report_panic(info);
        if backtrace_requested(env_value(BACKTRACE_ENV).as_deref()) {
            previous(info);
        }
    }));
}

/// Whether a `RUST_BACKTRACE` value asks for a backtrace: any value but `0`.
fn backtrace_requested(rust_backtrace: Option<&str>) -> bool {
    rust_backtrace.is_some_and(|value| value != "0")
}

/// Reports a panic as the `process.panic` event; past the subscriber when
/// its filter would drop the event, so a panic is never silent.
fn report_panic(info: &std::panic::PanicHookInfo<'_>) {
    let message = info
        .payload_as_str()
        .unwrap_or("panic with a non-string payload");
    let location = info.location().map(|at| (at.file(), at.line()));
    if emit_panic(message, location) {
        return;
    }
    write_past_the_subscriber(panic_event(message, location));
}

/// The `process.panic` event as a line of its own, with the fields
/// [`emit_panic`] logs.
fn panic_event(message: &str, location: Option<(&str, u32)>) -> DirectEvent {
    let mut fields = vec![
        ("exception.type", Value::from("panic")),
        ("exception.message", capped_value(message)),
    ];
    if let Some((file, line)) = location {
        fields.push(("code.file.path", capped_value(file)));
        fields.push(("code.line.number", Value::from(line)));
    }
    DirectEvent {
        name: PANIC_EVENT,
        message: PANIC_MESSAGE,
        fields,
    }
}

/// Emits the `process.panic` event, for a panic with `message` raised at
/// `location` (`(file, line)`), through the subscriber. Returns whether the
/// subscriber takes error events from here; `false` means nothing was
/// written.
fn emit_panic(message: &str, location: Option<(&str, u32)>) -> bool {
    if !tracing::enabled!(Level::ERROR) {
        return false;
    }
    let Some((file, line)) = location else {
        tracing::error!(
            name: "process.panic",
            { "exception.type" = "panic", exception.message = message },
            "process panicked"
        );
        return true;
    };
    tracing::error!(
        name: "process.panic",
        {
            "exception.type" = "panic",
            exception.message = message,
            code.file.path = file,
            code.line.number = line,
        },
        "process panicked"
    );
    true
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::{TraceContextExt, Tracer, TracerProvider};
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use serde_json::Value;
    use tracing::Level;

    use super::format::test_support::{fixed_clock, resource};
    use super::format::test_support::{formatter, Capture};
    use super::{
        backtrace_requested, emit_fatal, emit_panic, environment_from, filter, format, panic_event,
        report_rejected_settings, LogFormat, LogOptions, Pipeline,
    };

    fn pipeline(capture: &Capture) -> Pipeline<Capture> {
        filtered_pipeline(capture, None)
    }

    /// A pipeline filtered as `RUST_LOG=<rust_log>` would filter it.
    fn filtered_pipeline(capture: &Capture, rust_log: Option<&str>) -> Pipeline<Capture> {
        Pipeline {
            formatter: formatter(LogFormat::Json),
            filter: filter::env_filter(rust_log, Level::INFO).0,
            writer: capture.clone(),
            ansi: false,
            local_only: Vec::new(),
            output_limit: None,
        }
    }

    #[test]
    fn output_stops_at_the_limit_with_whole_lines_only() {
        let unlimited = Capture::default();
        let (subscriber, _slot) = pipeline(&unlimited).build();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(name: "test.limit.first", "first");
        });
        let one_line = u64::try_from(unlimited.text().len()).unwrap();

        let capture = Capture::default();
        let limited = Pipeline {
            // Room for two lines and most of a third.
            output_limit: Some(one_line * 3 - 1),
            ..pipeline(&capture)
        };
        let (subscriber, _slot) = limited.build();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(name: "test.limit.first", "first");
            tracing::info!(name: "test.limit.second", "secnd");
            tracing::info!(name: "test.limit.third", "third");
            // Shorter than what is left, but after a dropped line: a log
            // with a hole in it would mislead.
            tracing::info!(name: "t", "x");
        });

        let names: Vec<Value> = json_lines(&capture)
            .iter()
            .map(|line| line["event.name"].clone())
            .collect();
        assert_eq!(names, ["test.limit.first", "test.limit.second"]);
    }

    #[test]
    fn no_limit_is_the_default() {
        assert_eq!(LogOptions::new("svc").output_limit, None);
        assert_eq!(
            LogOptions::new("svc").with_output_limit(4096).output_limit,
            Some(4096)
        );
    }

    /// Runs `emit` under a subscriber filtered by `rust_log`; returns what
    /// `emit` returned and the lines it logged.
    fn under_filter<T>(rust_log: &str, emit: impl FnOnce() -> T) -> (T, Vec<Value>) {
        let capture = Capture::default();
        let (subscriber, _slot) = filtered_pipeline(&capture, Some(rust_log)).build();
        let returned = tracing::subscriber::with_default(subscriber, emit);
        (returned, json_lines(&capture))
    }

    #[test]
    fn a_fatal_event_the_filter_drops_is_reported_as_not_emitted() {
        for rust_log in ["off", "tumult_core=debug"] {
            let (emitted, lines) = under_filter(rust_log, || emit_fatal("locked"));
            assert!(!emitted, "RUST_LOG={rust_log}");
            assert!(lines.is_empty(), "RUST_LOG={rust_log}: {lines:?}");
        }
    }

    #[test]
    fn a_fatal_event_the_filter_keeps_is_emitted_once() {
        for rust_log in ["error", "info", "tumult_otel=error"] {
            let (emitted, lines) = under_filter(rust_log, || emit_fatal("locked"));
            assert!(emitted, "RUST_LOG={rust_log}");
            assert_eq!(lines.len(), 1, "RUST_LOG={rust_log}");
            assert_eq!(lines[0]["event.name"], "process.fatal");
            assert_eq!(lines[0]["error.message"], "locked");
        }
    }

    #[test]
    fn a_panic_event_the_filter_drops_is_reported_as_not_emitted() {
        for rust_log in ["off", "tumult_core=debug"] {
            let (emitted, lines) = under_filter(rust_log, || emit_panic("boom", None));
            assert!(!emitted, "RUST_LOG={rust_log}");
            assert!(lines.is_empty(), "RUST_LOG={rust_log}: {lines:?}");
        }
    }

    #[test]
    fn the_direct_panic_line_carries_the_fields_of_the_emitted_event() {
        let (emitted, lines) = under_filter("info", || {
            emit_panic("boom\nbang", Some(("src/lib.rs", 42)))
        });
        assert!(emitted);
        let direct = format::error_line(
            &resource(),
            fixed_clock(),
            module_path!().trim_end_matches("::tests"),
            panic_event("boom\nbang", Some(("src/lib.rs", 42))),
        );
        let mut direct: Value = serde_json::from_str(&direct).unwrap();
        let mut emitted = lines[0].clone();
        for line in [&mut direct, &mut emitted] {
            line.as_object_mut().unwrap().remove("timestamp");
        }
        assert_eq!(direct, emitted);
    }

    #[test]
    fn a_direct_panic_line_without_a_location_omits_the_code_fields() {
        let event = panic_event("boom", None);
        let keys: Vec<_> = event.fields.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, ["exception.type", "exception.message"]);
    }

    #[test]
    fn a_backtrace_is_requested_by_any_value_but_zero() {
        assert!(!backtrace_requested(None));
        assert!(!backtrace_requested(Some("0")));
        assert!(backtrace_requested(Some("1")));
        assert!(backtrace_requested(Some("full")));
    }

    #[test]
    fn local_only_targets_are_logged_but_not_exported() {
        let capture = Capture::default();
        let pipeline = Pipeline {
            local_only: vec!["tumult_ingest::writer".to_string()],
            ..pipeline(&capture)
        };
        let (subscriber, slot) = pipeline.build();
        let exporter = InMemoryLogExporter::default();
        let logger_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        slot.attach(&logger_provider).unwrap();

        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(target: "tumult_ingest::writer", "failed to persist ingest batch");
            tracing::error!(target: "tumult_ingest::runs", "run failed");
        });

        assert_eq!(json_lines(&capture).len(), 2, "both logged locally");
        let records = exporter.get_emitted_logs().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].record.target().map(AsRef::as_ref),
            Some("tumult_ingest::runs")
        );
    }

    #[test]
    fn local_only_targets_are_set_through_the_builder() {
        let options = LogOptions::new("tumultd").with_local_only_targets(["a::b", "c"]);
        assert_eq!(options.local_only_targets, ["a::b", "c"]);
        assert!(LogOptions::new("tumultd").local_only_targets.is_empty());
    }

    fn json_lines(capture: &Capture) -> Vec<Value> {
        capture
            .text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn options_default_to_info_and_automatic_format() {
        let options = LogOptions::new("tumultd");
        assert_eq!(options.service_name, "tumultd");
        assert_eq!(options.default_level, Level::INFO);
        assert_eq!(options.format, None);
        let quiet = LogOptions::new("tumult")
            .with_default_level(Level::WARN)
            .with_format(LogFormat::Text);
        assert_eq!(quiet.default_level, Level::WARN);
        assert_eq!(quiet.format, Some(LogFormat::Text));
    }

    #[test]
    fn local_line_and_exported_record_share_the_trace_id() {
        let capture = Capture::default();
        let (subscriber, slot) = pipeline(&capture).build();
        let exporter = InMemoryLogExporter::default();
        let logger_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        slot.attach(&logger_provider).unwrap();
        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let tracer = tracer_provider.tracer("pipeline-tests");

        tracing::subscriber::with_default(subscriber, || {
            let cx = opentelemetry::Context::current_with_span(tracer.start("request"));
            let _guard = cx.attach();
            tracing::warn!(name: "auth.login.failed", { user.name = "alice" }, "login failed");
        });

        let lines = json_lines(&capture);
        assert_eq!(lines.len(), 1);
        let local_trace_id = lines[0]["trace_id"].as_str().expect("trace_id");
        let logs = exporter.get_emitted_logs().unwrap();
        assert_eq!(logs.len(), 1);
        let record_context = logs[0].record.trace_context().expect("trace context");
        assert_eq!(record_context.trace_id.to_string(), local_trace_id);
        assert_eq!(logs[0].record.event_name(), Some("auth.login.failed"));
    }

    #[test]
    fn trace_id_survives_a_tracing_span_nested_in_an_otel_span() {
        let capture = Capture::default();
        let (subscriber, _slot) = pipeline(&capture).build();
        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let tracer = tracer_provider.tracer("pipeline-tests");

        let trace_id = tracing::subscriber::with_default(subscriber, || {
            let cx = opentelemetry::Context::current_with_span(tracer.start("experiment"));
            let trace_id = cx.span().span_context().trace_id().to_string();
            let _guard = cx.attach();
            let span = tracing::info_span!("ssh.exec", host = "db-1");
            let _entered = span.enter();
            tracing::info!("command sent");
            trace_id
        });

        let lines = json_lines(&capture);
        assert_eq!(lines[0]["trace_id"], Value::String(trace_id));
        assert_eq!(lines[0]["spans"][0]["name"], "ssh.exec");
    }

    #[test]
    fn exporter_internal_events_stay_local() {
        let capture = Capture::default();
        let (subscriber, slot) = pipeline(&capture).build();
        let exporter = InMemoryLogExporter::default();
        let logger_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        slot.attach(&logger_provider).unwrap();

        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(target: "opentelemetry_sdk", "export failed");
        });

        assert_eq!(json_lines(&capture).len(), 1, "still logged locally");
        assert!(exporter.get_emitted_logs().unwrap().is_empty());
    }

    #[test]
    fn environment_prefers_the_otel_resource_attribute() {
        assert_eq!(
            environment_from(
                Some("service.namespace=chaos, deployment.environment.name = prod"),
                Some("dev")
            )
            .as_deref(),
            Some("prod")
        );
    }

    #[test]
    fn environment_falls_back_to_the_tumult_variable() {
        assert_eq!(
            environment_from(Some("service.namespace=chaos"), Some(" staging ")).as_deref(),
            Some("staging")
        );
        assert_eq!(environment_from(None, Some("dev")).as_deref(), Some("dev"));
    }

    #[test]
    fn environment_is_absent_when_nothing_is_set() {
        assert_eq!(environment_from(None, None), None);
        assert_eq!(environment_from(Some(""), Some("  ")), None);
        assert_eq!(
            environment_from(Some("deployment.environment.name="), None),
            None
        );
    }

    #[test]
    fn panic_is_logged_as_one_structured_error_event() {
        let capture = Capture::default();
        let (subscriber, _slot) = pipeline(&capture).build();
        tracing::subscriber::with_default(subscriber, || {
            emit_panic("index out of bounds\nsecond line", Some(("src/lib.rs", 42)));
        });
        let lines = json_lines(&capture);
        assert_eq!(lines.len(), 1);
        let event = &lines[0];
        assert_eq!(event["level"], "ERROR");
        assert_eq!(event["event.name"], "process.panic");
        assert_eq!(event["exception.type"], "panic");
        assert_eq!(
            event["exception.message"],
            "index out of bounds\nsecond line"
        );
        assert_eq!(event["code.file.path"], "src/lib.rs");
        assert_eq!(event["code.line.number"], 42);
    }

    #[test]
    fn panic_without_a_location_omits_the_code_fields() {
        let capture = Capture::default();
        let (subscriber, _slot) = pipeline(&capture).build();
        tracing::subscriber::with_default(subscriber, || {
            emit_panic("boom", None);
        });
        let event = &json_lines(&capture)[0];
        assert!(event.get("code.file.path").is_none());
        assert!(event.get("code.line.number").is_none());
    }

    #[test]
    fn rejected_settings_are_each_reported_once() {
        let capture = Capture::default();
        let (subscriber, _slot) = pipeline(&capture).build();
        tracing::subscriber::with_default(subscriber, || {
            report_rejected_settings(Some("yaml"), Some("a=shouting"));
        });
        let lines = json_lines(&capture);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["level"], "WARN");
        assert_eq!(lines[0]["event.name"], "logging.format.invalid");
        assert_eq!(lines[0]["config.key"], "TUMULT_LOG_FORMAT");
        assert_eq!(lines[0]["config.value"], "yaml");
        assert_eq!(lines[1]["event.name"], "logging.filter.invalid");
        assert_eq!(lines[1]["config.key"], "RUST_LOG");
        assert_eq!(lines[1]["config.value"], "a=shouting");
    }

    #[test]
    fn nothing_is_reported_when_every_setting_was_accepted() {
        let capture = Capture::default();
        let (subscriber, _slot) = pipeline(&capture).build();
        tracing::subscriber::with_default(subscriber, || report_rejected_settings(None, None));
        assert!(capture.text().is_empty());
    }

    #[test]
    fn init_failure_is_one_json_line_shaped_like_a_log_event() {
        let resource = super::LogResource {
            service_name: "tumultd".to_string(),
            ..resource()
        };
        let error = super::InitError::Install("a global subscriber\nis already set".into());
        let line = super::init_failure_line(&resource, &error, fixed_clock());
        assert!(!line.contains('\n'), "{line}");
        let event: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(event["timestamp"], "2026-01-01T00:00:00.000000Z");
        assert_eq!(event["level"], "ERROR");
        assert_eq!(event["target"], "tumult_otel::logging");
        assert_eq!(event["event.name"], "logging.init.failed");
        assert_eq!(event["message"], "logging could not be initialised");
        assert_eq!(event["service.name"], "tumultd");
        assert_eq!(event["error.type"], "InitError");
        assert!(event["error.message"]
            .as_str()
            .unwrap()
            .contains("a global subscriber\nis already set"));
    }

    #[test]
    fn fatal_error_is_logged_with_its_whole_cause_chain() {
        struct Chained;
        impl std::fmt::Display for Chained {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(if f.alternate() {
                    "open store: locked"
                } else {
                    "open store"
                })
            }
        }
        let capture = Capture::default();
        let (subscriber, _slot) = pipeline(&capture).build();
        tracing::subscriber::with_default(subscriber, || super::log_fatal(&Chained));
        let lines = json_lines(&capture);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["level"], "ERROR");
        assert_eq!(lines[0]["event.name"], "process.fatal");
        assert_eq!(lines[0]["error.message"], "open store: locked");
    }

    #[test]
    fn tool_options_are_quiet_until_an_otlp_endpoint_is_configured() {
        let mut config = crate::config::TelemetryConfig {
            service_name: "tumult-mcp".to_string(),
            ..Default::default()
        };
        let quiet = LogOptions::for_tool(&config);
        assert_eq!(quiet.service_name, "tumult-mcp");
        assert_eq!(quiet.default_level, Level::WARN);
        assert_eq!(quiet.format, None);

        config.otlp_endpoint = Some("http://collector:4317".to_string());
        assert_eq!(LogOptions::for_tool(&config).default_level, Level::INFO);
    }
}
