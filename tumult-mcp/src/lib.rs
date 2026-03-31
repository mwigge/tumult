//! Tumult MCP Server — Library crate for tool definitions and handlers.
//!
//! Exposes Tumult chaos engineering as MCP tools:
//! - `tumult_run_experiment` — execute a chaos experiment
//! - `tumult_validate` — validate experiment syntax
//! - `tumult_discover` — list plugins and capabilities
//! - `tumult_analyze` — SQL query over journals (`DuckDB`)
//! - `tumult_read_journal` — read a TOON journal file
//! - `tumult_list_journals` — list journal files
//! - `tumult_create_experiment` — scaffold from template

pub mod handler;
pub(crate) mod telemetry;
pub mod tools;
