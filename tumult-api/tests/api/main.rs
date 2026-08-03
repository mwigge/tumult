// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! End-to-end tests for the query API: a seeded store served on an ephemeral
//! port, every endpoint exercised over HTTP.

mod ask_llm;
mod auth_approvals;
mod authoring;
mod common;
mod endpoints;
mod manual;
mod query;
mod reports;
mod runs;
