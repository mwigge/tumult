//! The outcome data types the panel renders — one per `tools/call` result
//! shape the parsers project into.

use serde::Serialize;

/// A tool as reported by `tools/list`, reduced to the fields the panel needs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// `annotations.destructiveHint` — true means the tool performs a
    /// destructive/irreversible action (fault injection) and the UI should
    /// require an explicit confirmation before calling it.
    pub destructive: bool,
    /// `annotations.readOnlyHint`.
    pub read_only: bool,
}

/// Normalised outcome of a `tumult_run_experiment` call.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RunOutcome {
    /// Raw journal status: completed / deviated / aborted / failed / interrupted
    /// / halted (the auto-halt guard pulled the run mid-flight).
    pub status: String,
    /// UI-facing verdict derived from `status`:
    /// "passed" | "failed" | "deviated" | "halted".
    pub outcome: String,
    pub duration_ms: Option<u64>,
    pub journal_path: Option<String>,
    pub ingestion: Option<String>,
}

/// Result of a `tumult_discover` call: how many plugins and actions the server
/// can dispatch to. Parsed from the tool's text output (discover advertises no
/// structured schema).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoverOutcome {
    pub plugins: usize,
    pub actions: usize,
}

/// Result of a `tumult_validate` call. Parsed from the tool's text summary
/// (`Valid: '<title>' — N method steps, M rollbacks`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValidateOutcome {
    pub valid: bool,
    pub title: Option<String>,
    pub method_steps: usize,
    pub rollbacks: usize,
    /// The raw one-line summary the tool returned.
    pub summary: String,
}

/// A tabular result from `tumult_analyze_store` (tab-separated text output).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TableOutcome {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
}

/// One recommendation from `tumult_recommend`'s structured content.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Recommendation {
    pub rank: i64,
    pub title: String,
    pub rationale: String,
}

/// Result of a `tumult_recommend` call. Either a `message` (no analytics store
/// yet) or a ranked list of recommendations from the structured content.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecommendOutcome {
    pub message: Option<String>,
    pub recommendations: Vec<Recommendation>,
}

/// One sourced control citation from `tumult_compliance` (reduced to the
/// fields the panel renders).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComplianceCitation {
    pub control_id: String,
    pub title: String,
    /// Evidence-strength grade: `direct` / `supporting` / `indirect`.
    pub strength: String,
}

/// Result of a `tumult_compliance` call: an *evidence* summary toward a
/// regulatory framework's controls — pass rate, recovery-compliance proxy, an
/// evidence verdict, and the scope disclaimer. Read from the tool's
/// `structuredContent`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ComplianceOutcome {
    /// Canonical framework report id (e.g. `DORA`).
    pub framework: String,
    /// Fraction of journals that completed (0.0-1.0).
    pub pass_rate: f64,
    /// Recovery-compliance proxy (0.0-1.0); `None` for a pass-rate-only verdict.
    pub recovery_compliance: Option<f64>,
    /// Evidence-strength verdict (`COMPLIANT` / `PARTIAL` / `NON-COMPLIANT`,
    /// possibly with a `(pass-rate only)` suffix). NOT a compliance attestation.
    pub verdict: String,
    pub journals_evaluated: u64,
    /// Scope disclaimer the tool ships: evidence toward controls, not a
    /// compliance determination.
    pub disclaimer: String,
    pub source_url: Option<String>,
    /// A few representative control citations (capped for the panel).
    pub citations: Vec<ComplianceCitation>,
}

/// A `ChaosGraph` node summary (`{id, kind, label}`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub label: String,
}

/// A `ChaosGraph` directed edge (`(src)-[rel]->(dst)`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphEdge {
    pub src: String,
    pub rel: String,
    pub dst: String,
}

/// Result of `tumult_chaosgraph_query`: node ids + labels for one kind.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphNodesOutcome {
    pub kind: String,
    pub count: usize,
    pub nodes: Vec<GraphNode>,
}

/// Result of `tumult_chaosgraph_neighbors`: the ego sub-graph of a node.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphEgoOutcome {
    pub node_id: String,
    pub depth: u32,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// One untested plugin action from `tumult_chaosgraph_coverage_gaps`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CoverageGap {
    pub id: String,
    pub plugin: String,
    pub action: String,
    pub domain: String,
}

/// A framework article with no evidence edge yet (coverage-gaps, framework
/// filter set).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UnevidencedArticle {
    pub id: String,
    pub control_id: String,
    pub strength: String,
}

/// Result of `tumult_chaosgraph_coverage_gaps`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CoverageGapsOutcome {
    pub count: usize,
    pub gaps: Vec<CoverageGap>,
    /// Present only when a framework filter was given.
    pub framework: Option<String>,
    /// Framework articles still lacking any evidence edge (framework filter).
    pub unevidenced_articles: Vec<UnevidencedArticle>,
}

/// One declared argument of a catalog action (`{name, required, description}`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogArg {
    pub name: String,
    pub required: bool,
    pub description: String,
}

/// One fault action/probe in the catalog, reduced to the fields the picker
/// needs to render an action option and its argument inputs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogAction {
    pub plugin: String,
    pub name: String,
    pub description: String,
    /// `action` (a fault) or `probe`.
    pub kind: String,
    pub args: Vec<CatalogArg>,
}

/// One fault domain and the actions grouped under it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogDomain {
    /// Stable kebab-case domain id (e.g. `network`).
    pub domain: String,
    /// Human label (e.g. `Network`).
    pub label: String,
    pub actions: Vec<CatalogAction>,
}

/// Result of `tumult_fault_catalog`: the domains → actions → args tree the
/// "New experiment" picker populates itself from.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogOutcome {
    pub action_count: usize,
    pub domains: Vec<CatalogDomain>,
}

/// Result of a `tumult_scaffold_experiment` call: the generated TOON and
/// whether it validates. Read from `structuredContent`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScaffoldOutcome {
    /// The fully-qualified `plugin::action` that was scaffolded.
    pub action: String,
    /// The generated experiment as TOON text.
    pub toon: String,
    /// True when the generated experiment passes validation.
    pub valid: bool,
    /// The validation error message, present only when `valid` is false.
    pub validation_error: Option<String>,
}

/// Result of a `tumult_whoami` call: the caller's resolved RBAC role and
/// whether the request was authenticated. Read from `structuredContent`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WhoamiOutcome {
    /// Canonical role name: `viewer` (read-only tools) or `operator` (all tools).
    pub role: String,
    /// True when a configured bearer token validated the request; false in
    /// loopback open mode (no auth configured — full access without a token).
    pub authenticated: bool,
}
