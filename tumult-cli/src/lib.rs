//! Tumult CLI — command implementations exposed for integration tests.
//!
//! Command results are printed to stdout: that output is the product.
//! Diagnostics are structured log events on stderr (`docs/guides/logging.md`).
//! Results are printed through [`output`], which does not panic when the
//! reader has gone away.

#![allow(clippy::print_stdout)] // analyze.rs and perf.rs still print with println!: docs/guides/logging.md

pub mod commands;
pub mod output;
