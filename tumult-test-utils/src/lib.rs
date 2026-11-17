//! Shared test helpers for the tumult workspace.
//!
//! This crate is a **test-only** utility used exclusively as a
//! `[dev-dependency]`. One definition per helper; see
//! `docs/guides/testing.md` for the rules the helpers support.
//!
//! | Helper | Feature | Purpose |
//! |---|---|---|
//! | [`EnvGuard`] | always | change environment variables under a process-wide lock, restored on drop |
//! | [`assert_kept`] | always | compare a text with the file kept in the repository; `TUMULT_BLESS=1` keeps the new text |
//! | [`fake_bin`], [`write_executable`] | always, Unix | an executable file that can be spawned at once, without the "Text file busy" race |
//! | [`capture_spans`], [`capture_spans_async`] | `otel` | the spans a piece of code finished, isolated per test |
//! | [`MetricCapture`] | `otel` | a private meter provider whose counters, gauges and histograms can be read back |
//! | [`action`], [`background_action`], [`minimal_experiment`] | `builders` (default) | experiment fixtures for runner tests |
//! | [`recorded::load`] | `recorded` | the telemetry and journals of real runs, put into a store the way the daemon does |
//!
//! `builders` depends on `tumult-core`. Crates below `tumult-core` in the
//! dependency graph take this crate with `default-features = false`.

#[cfg(feature = "builders")]
mod builders;
mod env;
#[cfg(unix)]
mod exec;
mod golden;
#[cfg(feature = "otel")]
mod otel;
#[cfg(feature = "recorded")]
pub mod recorded;

#[cfg(feature = "builders")]
pub use builders::{action, background_action, minimal_experiment};
pub use env::EnvGuard;
#[cfg(unix)]
pub use exec::{fake_bin, write_executable};
pub use golden::{assert_kept, BLESS};
#[cfg(feature = "otel")]
pub use otel::{
    attribute, capture_spans, capture_spans_async, install_recording_tracer, span_attribute,
    HistogramPoint, MetricCapture, MetricPoint,
};
