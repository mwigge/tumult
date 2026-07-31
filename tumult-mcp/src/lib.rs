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

/// Error types returned by tool implementations.
pub mod error;
/// MCP handler: routes tool calls to their implementations.
pub mod handler;
/// Composition root registering the native plugins visible to `tumult_discover`.
pub(crate) mod native;
/// Programmatic entry point for running the MCP server over stdio or HTTP.
pub mod server;
/// `OTel` instrumentation for MCP tool dispatch.
pub(crate) mod telemetry;
/// Tool implementations behind the MCP tool surface.
pub mod tools;
