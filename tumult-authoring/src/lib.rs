//! Authoring ergonomics for Tumult.
//!
//! This crate turns "pick a fault" into "a validated, ready-to-run experiment"
//! in three cooperating pieces, all reused by both the CLI (`tumult new`,
//! `tumult templates`) and the MCP tools (`tumult_fault_catalog`,
//! `tumult_scaffold_experiment`) so there is a single source of truth:
//!
//! - [`catalog`] — a fault catalog derived live from the shipped plugins via
//!   [`tumult_plugin::discovery`], grouped into fault [`Domain`]s.
//! - [`builder`] — the experiment builder: given an action, argument values, a
//!   target, and a steady-state probe, construct a validated
//!   [`Experiment`](tumult_core::types::Experiment) and serialize it to TOON.
//! - [`templates`] — ~10 curated, parameterized starter templates covering the
//!   main domains; every one validates.

pub mod builder;
pub mod catalog;
pub mod templates;

pub use builder::{
    build_experiment, build_experiment_toon, build_experiment_unvalidated, encode_experiment,
    rollback_action, AuthoringError, ProbeSpec, ScaffoldRequest,
};
pub use catalog::{
    build_catalog, build_catalog_with_config, documented_args, domain_for, ActionKind,
    CatalogAction, CatalogArg, CatalogDomain, Domain, FaultCatalog,
};
pub use templates::{all_templates, find_template, parse_overrides, Template, TemplateParam};
