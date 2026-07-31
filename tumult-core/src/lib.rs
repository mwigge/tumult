//! Tumult Core — Chaos engineering experiment engine.
//!
//! This crate provides the experiment runner, data model types,
//! hypothesis evaluation, and journal output for the Tumult platform.
//!
//! # Overview
//!
//! `tumult-core` is the central crate in the Tumult workspace. It owns the
//! five-phase experiment lifecycle:
//!
//! 1. **Estimate** (Phase 0) — record the operator's predicted outcome.
//! 2. **Baseline** (Phase 1) — acquire steady-state metrics before injection.
//! 3. **Method / During** (Phase 2) — execute chaos actions while sampling probes.
//! 4. **Post** (Phase 3) — measure recovery after actions complete.
//! 5. **Analysis** (Phase 4) — compare estimate vs. actual, compute resilience score.
//!
//! # Key modules
//!
//! | Module | Purpose |
//! |-------------|------------------------------------------------------|
//! | [`types`] | Core data model (`Experiment`, `Journal`, `Activity`) |
//! | [`engine`] | Validation, config/secret resolution, tolerance eval |
//! | [`runner`] | Five-phase orchestration via `run_experiment` |
//! | [`controls`]| Lifecycle hooks for logging, tracing, safeguards |
//! | [`execution`]| Result helpers and rollback strategy |
//! | [`journal`] | TOON-encoded journal writer and reader |
//!
//! # Getting started
//!
//! See the [data lifecycle guide](https://github.com/tumult-rs/tumult/blob/main/docs/data-lifecycle.md)
//! for an end-to-end walkthrough of experiment authoring through journal analysis.

/// Regulatory compliance domain logic shared by the CLI and MCP server.
pub mod compliance;
/// Controls — lifecycle hooks for cross-cutting concerns (logging, tracing, safeguards).
pub mod controls;
/// Experiment engine — validation, config/secret resolution, tolerance evaluation.
pub mod engine;
/// Method and rollback execution logic.
pub mod execution;
/// Journal writer — serializes experiment results to TOON format.
pub mod journal;
/// Journal report renderers (JSON, `JUnit` XML) shared by the CLI and MCP server.
pub mod report;
/// Experiment runner — orchestrates the five-phase execution lifecycle.
pub mod runner;
/// Bridge for driving async futures from synchronous code.
pub mod sync_bridge;
/// Core data model types for Tumult experiments.
pub mod types;
