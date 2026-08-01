//! Tumult `OTel` — OpenTelemetry instrumentation for the Tumult platform.
//!
//! Provides always-on tracing, metrics, and logging via the
//! `tracing` + `tracing-opentelemetry` bridge with OTLP export.

/// Canonical agentic telemetry schema shared with tumult-agentic.
pub mod agentic;
/// Experiment-side agentic instrumentation: span constructors and run recording.
pub mod agentic_span;
/// Standard span attribute names in the `resilience.*` namespace.
pub mod attributes;
/// Telemetry configuration.
pub mod config;
/// Instrumentation wrappers for chaos actions and probes.
pub mod instrument;
/// Standard metrics for Tumult experiments.
pub mod metrics;
/// W3C trace-context (`traceparent`/`tracestate`) propagation helpers.
pub mod propagation;
/// Telemetry initialization and lifecycle management.
pub mod telemetry;

pub use config::TelemetryConfig;
pub use instrument::{client_span, SpanGuard};
pub use metrics::TumultMetrics;
pub use telemetry::TumultTelemetry;

#[cfg(test)]
mod tests;
