// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! End-to-end tests for the query API: a seeded store served on an ephemeral
//! port, every endpoint exercised over HTTP.

mod ask_llm;
mod auth_approvals;
mod authoring;
mod common;
mod dora;
mod endpoints;
mod entities;
mod events;
mod evidence;
mod faults;
mod forecast;
mod gamedays;
mod gate;
mod journal_only;
mod manual;
mod manual_files;
mod manual_plans;
mod manual_signoffs;
mod perf;
mod query;
#[cfg(unix)]
mod real_runs;
mod recovery;
mod reports;
mod risks;
mod runs;
mod schedules;
mod scopes;
mod starmap;
mod webhooks;
