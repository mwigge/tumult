//! Tumult CLI — command implementations exposed for integration tests.
//!
//! Command results are printed to stdout: that output is the product.
//! Diagnostics are structured log events on stderr (`docs/guides/logging.md`).

#![allow(clippy::print_stdout)] // command results are this crate's output; diagnostics go through tracing

pub mod commands;
