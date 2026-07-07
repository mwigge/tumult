//! `topology` subcommand: declared service topology and compliance lineage
//! over the analytics store, mirroring the MCP `tumult_topology_*` /
//! `tumult_compliance_lineage` / `tumult_recommend_injection` tools.
//!
//! `import` loads a reviewed topology TOML (services + `depends_on` edges)
//! into the store; `map`, `lineage`, and `recommend` are read-only views
//! over the same store the MCP server exposes, so operator and agent always
//! see the same picture.

use std::path::Path;

use anyhow::{anyhow, Result};

use super::chaosgraph::{emit, resolve_store};

/// `topology import`: parse and persist a declared topology TOML file,
/// replacing the previous declared topology (idempotent).
///
/// # Errors
///
/// Returns an error if the file cannot be read, the TOML is invalid, or the
/// store is missing or cannot be written.
pub fn cmd_topology_import(store: Option<&Path>, path: &Path, json: bool) -> Result<()> {
    let store = resolve_store(store);
    let report = tumult_mcp::tools::topology_import(
        &store.to_string_lossy(),
        None,
        Some(&path.to_string_lossy()),
    )
    .map_err(|e| anyhow!(e.to_string()))?;
    emit(&report, json)
}

/// `topology map`: the compliance-aware service map as text, Mermaid, or
/// JSON, optionally scoped to a framework/control, with ranked injection
/// recommendations unless `--no-recommend`.
///
/// # Errors
///
/// Returns an error if the store is missing, cannot be read, or an unknown
/// framework/format is given.
pub fn cmd_topology_map(
    store: Option<&Path>,
    framework: Option<&str>,
    control: Option<&str>,
    format: &str,
    recommend: bool,
    limit: u32,
) -> Result<()> {
    let store = resolve_store(store);
    let report = tumult_mcp::tools::topology_map(
        &store.to_string_lossy(),
        framework,
        control,
        Some(format),
        Some(recommend),
        Some(limit),
    )
    .map_err(|e| anyhow!(e.to_string()))?;
    // For --format json the report text is only a summary note; print the
    // full view from the structured content instead.
    emit(&report, format == "json")
}

/// `topology lineage`: the (article × service) compliance lineage matrix,
/// optionally scoped by framework, control, and service.
///
/// # Errors
///
/// Returns an error if the store is missing, cannot be read, or an unknown
/// framework is given.
pub fn cmd_topology_lineage(
    store: Option<&Path>,
    framework: Option<&str>,
    control: Option<&str>,
    service: Option<&str>,
    json: bool,
) -> Result<()> {
    let store = resolve_store(store);
    let report = tumult_mcp::tools::compliance_lineage(
        &store.to_string_lossy(),
        framework,
        control,
        service,
    )
    .map_err(|e| anyhow!(e.to_string()))?;
    emit(&report, json)
}

/// `topology recommend`: ranked, explained injection recommendations.
///
/// # Errors
///
/// Returns an error if the store is missing, cannot be read, or an unknown
/// framework is given.
pub fn cmd_topology_recommend(
    store: Option<&Path>,
    framework: Option<&str>,
    limit: u32,
    json: bool,
) -> Result<()> {
    let store = resolve_store(store);
    let report = tumult_mcp::tools::recommend_injection(
        &store.to_string_lossy(),
        framework,
        Some(limit),
    )
    .map_err(|e| anyhow!(e.to_string()))?;
    emit(&report, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPOLOGY_TOML: &str = "[[service]]\nname = \"api\"\ndepends_on = [\"db\"]\n\n[[service]]\nname = \"db\"\ntier = \"data\"\n";

    fn seeded_store(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("analytics.duckdb");
        drop(tumult_analytics::AnalyticsStore::open(&db).unwrap());
        db
    }

    #[test]
    fn import_map_lineage_and_recommend_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seeded_store(dir.path());
        let toml_path = dir.path().join("topology.toml");
        std::fs::write(&toml_path, TOPOLOGY_TOML).unwrap();

        cmd_topology_import(Some(&db), &toml_path, false).unwrap();
        cmd_topology_map(Some(&db), None, None, "text", true, 3).unwrap();
        cmd_topology_map(Some(&db), Some("dora"), None, "mermaid", false, 3).unwrap();
        cmd_topology_map(Some(&db), None, None, "json", true, 3).unwrap();
        cmd_topology_lineage(Some(&db), Some("nis2"), None, None, true).unwrap();
        cmd_topology_recommend(Some(&db), Some("dora"), 3, false).unwrap();
    }

    #[test]
    fn import_missing_file_is_a_clean_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = seeded_store(dir.path());
        let err = cmd_topology_import(Some(&db), Path::new("/nonexistent/topology.toml"), false)
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot read topology file"),
            "{err}"
        );
    }

    #[test]
    fn missing_store_is_a_clean_error() {
        let missing = Path::new("/nonexistent/tumult-topology.duckdb");
        let err = cmd_topology_map(Some(missing), None, None, "text", true, 3).unwrap_err();
        assert!(err.to_string().contains("store not found"), "{err}");
    }
}
