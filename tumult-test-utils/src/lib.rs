//! Shared test helpers for the tumult workspace.
//!
//! This crate is a **test-only** utility used exclusively as a
//! `[dev-dependency]`. One definition per helper; see
//! `docs/guides/testing.md` for the rules the helpers support.
//!
//! | Helper | Feature | Purpose |
//! |---|---|---|
//! | [`EnvGuard`] | always | change environment variables under a process-wide lock, restored on drop |
//! | [`capture_spans`], [`capture_spans_async`] | `otel` | the spans a piece of code finished, isolated per test |
//! | [`MetricCapture`] | `otel` | a private meter provider whose counters and gauges can be read back |
//! | [`action`], [`background_action`], [`minimal_experiment`] | `builders` (default) | experiment fixtures for runner tests |
//!
//! `builders` depends on `tumult-core`. Crates below `tumult-core` in the
//! dependency graph take this crate with `default-features = false`.

#[cfg(feature = "builders")]
mod builders;
mod env;
#[cfg(feature = "otel")]
mod otel;

#[cfg(feature = "builders")]
pub use builders::{action, background_action, minimal_experiment};
pub use env::EnvGuard;
#[cfg(feature = "otel")]
pub use otel::{
    attribute, capture_spans, capture_spans_async, install_recording_tracer, span_attribute,
    MetricCapture, MetricPoint,
};
