//! Agentic telemetry schema.
//!
//! The canonical schema now lives in [`tumult_otel::agentic`] so observability
//! is defined in exactly one place. This module re-exports it for backward
//! compatibility — existing `tumult_agentic::telemetry::*` paths keep working.

pub use tumult_otel::agentic::*;
