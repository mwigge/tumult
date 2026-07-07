//! `autopilot` subcommand: one pass of the policy-gated decision loop, the
//! approval queue, status readback, and the Parquet archive export —
//! mirroring the MCP `tumult_autopilot_*` tools over the same store.
//!
//! `once` prints the pass report and exits 0 even when every decision was
//! vetoed: a veto is the gate doing its job (a working outcome), not an
//! error. Errors are reserved for a missing store, an invalid or disabled
//! policy, and failed playbook runs.

use std::path::Path;

use anyhow::{anyhow, Result};

use super::chaosgraph::{emit, resolve_store};

/// `autopilot once`: run one decide-and-record pass; with `execute` the
/// enact verdicts also run their playbook experiments.
///
/// # Errors
///
/// Returns an error if the store or policy is missing, the policy is
/// invalid or disabled, or an executed playbook fails.
pub fn cmd_autopilot_once(
    store: Option<&Path>,
    policy: &Path,
    execute: bool,
    limit: Option<u32>,
) -> Result<()> {
    let store = resolve_store(store);
    let report = tumult_mcp::tools::autopilot_once(
        &store.to_string_lossy(),
        &policy.to_string_lossy(),
        execute,
        limit,
    )
    .map_err(|e| anyhow!(e.to_string()))?;
    println!("{}", report.text);
    Ok(())
}

/// `autopilot status`: recorded decisions with their latest lifecycle
/// event, optionally filtered by verdict. Opens the store read-only.
///
/// # Errors
///
/// Returns an error if the store is missing or cannot be read.
pub fn cmd_autopilot_status(
    store: Option<&Path>,
    verdict: Option<&str>,
    limit: Option<u32>,
    json: bool,
) -> Result<()> {
    let store = resolve_store(store);
    let report = tumult_mcp::tools::autopilot_status(&store.to_string_lossy(), verdict, limit)
        .map_err(|e| anyhow!(e.to_string()))?;
    if json {
        return emit(&report, true);
    }
    // The status text is newline-joined lines without a trailing newline;
    // println keeps the shell prompt off the last line.
    println!("{}", report.text);
    Ok(())
}

/// `autopilot approve` / `autopilot deny`: record the human response to a
/// proposed decision. Approval runs the playbook experiment; denial records
/// the veto feedback the autonomy ladder consumes.
///
/// # Errors
///
/// Returns an error if the store is missing, the decision does not exist or
/// was already resolved, or an approved playbook run fails.
pub fn cmd_autopilot_respond(
    store: Option<&Path>,
    id: &str,
    approve: bool,
    reason: Option<&str>,
) -> Result<()> {
    let store = resolve_store(store);
    let report =
        tumult_mcp::tools::autopilot_respond(&store.to_string_lossy(), id, approve, reason)
            .map_err(|e| anyhow!(e.to_string()))?;
    println!("{}", report.text);
    Ok(())
}

/// `autopilot export`: write the decision and event tables as Parquet
/// files under `dir`.
///
/// # Errors
///
/// Returns an error if the store is missing or the directory cannot be
/// written.
pub fn cmd_autopilot_export(store: Option<&Path>, dir: &Path) -> Result<()> {
    let store = resolve_store(store);
    let report =
        tumult_mcp::tools::autopilot_export(&store.to_string_lossy(), &dir.to_string_lossy())
            .map_err(|e| anyhow!(e.to_string()))?;
    println!("{}", report.text);
    Ok(())
}

/// Record a change event against a service.
pub fn cmd_autopilot_notify_change(
    store: Option<&Path>,
    service: &str,
    source: &str,
    detail: Option<&str>,
) -> Result<()> {
    let store = resolve_store(store);
    let report = tumult_mcp::tools::autopilot_notify_change(
        &store.to_string_lossy(),
        service,
        source,
        detail,
    )
    .map_err(|e| anyhow!(e.to_string()))?;
    println!("{}", report.text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded_store(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("analytics.duckdb");
        drop(tumult_analytics::AnalyticsStore::open(&db).unwrap());
        db
    }

    #[test]
    fn once_status_and_export_round_trip_on_empty_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seeded_store(dir.path());
        let policy = dir.path().join("autopilot.toml");
        std::fs::write(&policy, "[autopilot]\nenabled = true\n").unwrap();

        // No candidates on an empty store: still exit code 0 (a quiet pass
        // is a working outcome, like a pass full of vetoes).
        cmd_autopilot_once(Some(&db), &policy, false, None).unwrap();
        cmd_autopilot_status(Some(&db), None, None, false).unwrap();
        cmd_autopilot_status(Some(&db), Some("veto"), Some(5), true).unwrap();
        let out = dir.path().join("archive");
        cmd_autopilot_export(Some(&db), &out).unwrap();
        assert!(out.join("autopilot_decisions.parquet").exists());
    }

    #[test]
    fn once_with_disabled_policy_is_a_clean_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seeded_store(dir.path());
        let policy = dir.path().join("autopilot.toml");
        std::fs::write(&policy, "[autopilot]\nenabled = false\n").unwrap();
        let err = cmd_autopilot_once(Some(&db), &policy, false, None).unwrap_err();
        assert!(err.to_string().contains("disabled"), "{err}");
    }

    #[test]
    fn respond_on_unknown_decision_is_a_clean_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seeded_store(dir.path());
        let err = cmd_autopilot_respond(Some(&db), "no-such-id", false, None).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn missing_store_is_a_clean_error() {
        let missing = Path::new("/nonexistent/tumult-autopilot.duckdb");
        let err = cmd_autopilot_status(Some(missing), None, None, false).unwrap_err();
        assert!(err.to_string().contains("store not found"), "{err}");
    }
}
