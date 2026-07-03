//! Integration tests for the experiment runner.
//!
//! These tests exercise the full experiment lifecycle using mock plugins,
//! validating the five-phase execution, hypothesis evaluation, rollbacks,
//! background activities, and estimate accuracy.
//!
//! The suite is split into cohesive submodules under
//! `experiment_integration/`. Because each file under `tests/` is compiled as
//! its own crate root (which cannot use directory-based module resolution),
//! the submodules are wired in explicitly with `#[path]`.

#[path = "experiment_integration/common.rs"]
mod common;

#[path = "experiment_integration/journal_tests.rs"]
mod journal_tests;

#[path = "experiment_integration/hypothesis_rollback_tests.rs"]
mod hypothesis_rollback_tests;

#[path = "experiment_integration/background_tests.rs"]
mod background_tests;

#[path = "experiment_integration/estimate_tests.rs"]
mod estimate_tests;

#[path = "experiment_integration/tolerance_tests.rs"]
mod tolerance_tests;
