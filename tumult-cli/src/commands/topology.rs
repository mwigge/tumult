//! `topology` subcommand: declared service topology and compliance lineage
//! over the analytics store, mirroring the MCP `tumult_topology_*` /
//! `tumult_compliance_lineage` / `tumult_recommend_injection` tools.
//!
//! `import` loads a reviewed topology TOML (services + `depends_on` edges)
//! into the store; `map`, `lineage`, and `recommend` are read-only views
//! over the same store the MCP server exposes, so operator and agent always
//! see the same picture. `discover-k8s` drafts a *proposed* topology TOML
//! from a live cluster's Services — it never writes the store or graph;
//! its output is input to human review and then `import`.

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
    let store = resolve_store(store)?;
    let report = tumult_mcp::tools::topology_import(
        &store.to_string_lossy(),
        None,
        Some(&path.to_string_lossy()),
    )
    .map_err(|e| anyhow!(e.to_string()))?;
    emit(&report, json)
}

/// `topology discover-k8s`: list Services from the live cluster and render a
/// PROPOSED topology TOML for human review — to stdout or `--output`.
///
/// Never touches the store or the graph: Tumult topology is declared, not
/// guessed, so discovery only drafts the file a human reviews, fills
/// `depends_on` into (Kubernetes does not know dependencies), and then feeds
/// to `topology import`.
///
/// Not runnable in the docker demo — it needs reachable cluster credentials.
/// The discovery and rendering logic is unit-tested in `tumult-kubernetes`
/// against constructed objects and a fake apiserver instead.
///
/// # Errors
///
/// Returns an error if no kubernetes credentials are found, the cluster is
/// unreachable, or the `--output` file cannot be written.
pub async fn cmd_topology_discover_k8s(namespaces: &[String], output: Option<&Path>) -> Result<()> {
    let client = tumult_kubernetes::discovery::default_client()
        .await
        .map_err(|e| {
            anyhow!(
                "no kubernetes credentials found — this command needs a reachable cluster ({e})"
            )
        })?;
    let services = tumult_kubernetes::discovery::discover_services(client, namespaces)
        .await
        .map_err(|e| anyhow!("kubernetes service discovery failed: {e}"))?;
    let toml = tumult_kubernetes::discovery::proposed_topology_toml(&services);
    emit_proposed(&toml, services.len(), output)
}

/// Write the proposed TOML to `output`, or print it to stdout. Split from
/// [`cmd_topology_discover_k8s`] so the output path is testable without a
/// cluster.
fn emit_proposed(toml: &str, service_count: usize, output: Option<&Path>) -> Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, toml).map_err(|e| {
                anyhow!("cannot write proposed topology to {}: {e}", path.display())
            })?;
            println!(
                "proposed topology with {service_count} service(s) written to {} — review, fill in depends_on, then `tumult topology import`",
                path.display()
            );
        }
        None => print!("{toml}"),
    }
    Ok(())
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
    let store = resolve_store(store)?;
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
    let store = resolve_store(store)?;
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
    let store = resolve_store(store)?;
    let report =
        tumult_mcp::tools::recommend_injection(&store.to_string_lossy(), framework, Some(limit))
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
    fn emit_proposed_writes_output_file_verbatim() {
        let dir = tempfile::TempDir::new().unwrap();
        let out = dir.path().join("proposed-topology.toml");
        let toml = "# proposed by tumult topology discover-k8s — REVIEW before import\n\n[[service]]\nname = \"api\"\ndepends_on = []\n";

        emit_proposed(toml, 1, Some(&out)).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), toml);
    }

    #[test]
    fn emit_proposed_unwritable_output_is_a_clean_error() {
        let err = emit_proposed(
            "# x\n",
            0,
            Some(Path::new("/nonexistent/dir/topology.toml")),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("cannot write proposed topology"),
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
