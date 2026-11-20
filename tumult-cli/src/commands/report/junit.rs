//! JSON and `JUnit` renderers — thin wrappers over the shared
//! `tumult_core::report` implementations (one source of truth with the MCP
//! server's `tumult_report` tool).

use anyhow::Result;

use tumult_core::types::Journal;

/// FIX 1: raw journal serialized as JSON.
///
/// # Errors
///
/// Returns an error if the journal cannot be serialized.
pub(crate) fn generate_json_report(journal: &Journal) -> Result<String> {
    Ok(tumult_core::report::json_report(journal)?)
}

/// Minimal `JUnit` XML: one `<testcase>` per step of every phase and one per
/// check of the steady state.
pub(crate) fn generate_junit_report(journal: &Journal) -> String {
    tumult_core::report::junit_report(journal)
}
