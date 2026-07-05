//! Response mapping: the outcome data types the panel renders and the pure
//! parsers that project each `tools/call` result into them. Every parser is
//! unit-tested against canned JSON — no live server required.

use serde::Serialize;
use serde_json::Value;

use super::protocol::McpError;

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

/// Parse a `tools/list` result into [`ToolInfo`]s, reading each tool's
/// `annotations` for the destructive/read-only hints.
#[must_use]
pub fn parse_tools_list(result: &Value) -> Vec<ToolInfo> {
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(|t| {
                    let ann = t.get("annotations");
                    ToolInfo {
                        name: t
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        destructive: ann
                            .and_then(|a| a.get("destructiveHint"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        read_only: ann
                            .and_then(|a| a.get("readOnlyHint"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `tumult_run_experiment` `tools/call` result into a [`RunOutcome`].
///
/// Reads `structuredContent.journal.{status,duration_ms}`,
/// `structuredContent.journal_path`, and `structuredContent.ingestion`.
///
/// # Errors
/// Returns [`McpError::Protocol`] when the tool reported `isError: true` (the
/// error text is lifted from the `content` array) or when no journal status can
/// be found.
pub fn parse_run_result(result: &Value) -> Result<RunOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "experiment tool reported an error".to_string(),
        )));
    }

    let sc = result
        .get("structuredContent")
        .ok_or_else(|| McpError::Protocol("run result missing structuredContent".to_string()))?;
    let journal = sc.get("journal").unwrap_or(sc);

    let status = journal
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("journal missing status".to_string()))?
        .to_string();

    let duration_ms = journal.get("duration_ms").and_then(Value::as_u64);
    let journal_path = sc
        .get("journal_path")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let ingestion = sc
        .get("ingestion")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(RunOutcome {
        outcome: verdict_for(&status).to_string(),
        status,
        duration_ms,
        journal_path,
        ingestion,
    })
}

/// Map a raw journal status to the panel's verdict. `halted` (auto-halt guard)
/// gets its own verdict so the UI can badge it distinctly from an outright
/// failure.
#[must_use]
pub fn verdict_for(status: &str) -> &'static str {
    match status {
        "completed" => "passed",
        "deviated" => "deviated",
        "halted" => "halted",
        _ => "failed",
    }
}

// ── Discover / validate / analyze_store / recommend parsers ────

/// Parse a `tumult_discover` `tools/call` result into a [`DiscoverOutcome`].
///
/// Discover advertises no structured schema, so we read the text content and
/// pull the `Plugins: N` / `Actions: M` header counts.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or when the counts
/// cannot be located.
pub fn parse_discover_result(result: &Value) -> Result<DiscoverOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "discover tool reported an error".to_string()),
        ));
    }
    let text = content_text(result)
        .ok_or_else(|| McpError::Protocol("discover result had no text content".to_string()))?;
    let plugins = labeled_count(&text, "Plugins:")
        .ok_or_else(|| McpError::Protocol("discover output missing plugin count".to_string()))?;
    let actions = labeled_count(&text, "Actions:")
        .ok_or_else(|| McpError::Protocol("discover output missing action count".to_string()))?;
    Ok(DiscoverOutcome { plugins, actions })
}

/// Parse a `tumult_validate` `tools/call` result into a [`ValidateOutcome`].
///
/// A failed validation surfaces as `isError: true`; we lift its text into
/// [`McpError::Protocol`] so the loop marks the step failed.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing text.
pub fn parse_validate_result(result: &Value) -> Result<ValidateOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "experiment failed validation".to_string()),
        ));
    }
    let text = content_text(result)
        .ok_or_else(|| McpError::Protocol("validate result had no text content".to_string()))?;
    let trimmed = text.trim();
    let valid = trimmed.starts_with("Valid");
    let title = trimmed
        .split_once('\'')
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(t, _)| t.to_string());
    let method_steps = number_before(trimmed, "method step").unwrap_or(0);
    let rollbacks = number_before(trimmed, "rollback").unwrap_or(0);
    Ok(ValidateOutcome {
        valid,
        title,
        method_steps,
        rollbacks,
        summary: trimmed.to_string(),
    })
}

/// Parse a `tumult_analyze_store` `tools/call` result into a [`TableOutcome`].
///
/// The tool returns tab-separated text: a header row, one row per record, then
/// a trailing `N row(s)` line.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing text.
pub fn parse_analyze_store_result(result: &Value) -> Result<TableOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "analyze_store tool reported an error".to_string(),
        )));
    }
    let text = content_text(result).ok_or_else(|| {
        McpError::Protocol("analyze_store result had no text content".to_string())
    })?;

    let mut lines = text.lines();
    let columns: Vec<String> = lines
        .next()
        .map(|h| h.split('\t').map(str::to_string).collect())
        .unwrap_or_default();
    let mut rows = Vec::new();
    for line in lines {
        // The trailing "N row(s)" summary line is not a data row.
        if line.trim_end().ends_with("row(s)") {
            continue;
        }
        if line.is_empty() {
            continue;
        }
        rows.push(line.split('\t').map(str::to_string).collect::<Vec<_>>());
    }
    let row_count = rows.len();
    Ok(TableOutcome {
        columns,
        rows,
        row_count,
    })
}

/// Parse a `tumult_recommend` `tools/call` result into a [`RecommendOutcome`].
///
/// Reads `structuredContent`: either a `message` (no store yet) or a
/// `recommendations` array of `{rank, title, rationale}`. Falls back to the
/// text content when no structured content is present.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error.
pub fn parse_recommend_result(result: &Value) -> Result<RecommendOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "recommend tool reported an error".to_string()),
        ));
    }
    if let Some(sc) = result.get("structuredContent") {
        if let Some(msg) = sc.get("message").and_then(Value::as_str) {
            return Ok(RecommendOutcome {
                message: Some(msg.to_string()),
                recommendations: Vec::new(),
            });
        }
        let recommendations = sc
            .get("recommendations")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|r| Recommendation {
                        rank: r.get("rank").and_then(Value::as_i64).unwrap_or_default(),
                        title: r
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        rationale: r
                            .get("rationale")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(RecommendOutcome {
            message: None,
            recommendations,
        });
    }
    // No structured content — fall back to the raw text summary.
    Ok(RecommendOutcome {
        message: content_text(result),
        recommendations: Vec::new(),
    })
}

/// Parse a `tumult_compliance` `tools/call` result into a [`ComplianceOutcome`]
/// from its `structuredContent`.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_compliance_result(result: &Value) -> Result<ComplianceOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "compliance tool reported an error".to_string(),
        )));
    }
    let sc = result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol("compliance result missing structuredContent".to_string())
    })?;
    let citations = sc
        .get("citations")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(4)
                .map(|c| ComplianceCitation {
                    control_id: str_field(c, "control_id"),
                    title: str_field(c, "title"),
                    strength: str_field(c, "strength"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ComplianceOutcome {
        framework: str_field(sc, "framework"),
        pass_rate: sc.get("pass_rate").and_then(Value::as_f64).unwrap_or(0.0),
        recovery_compliance: sc.get("recovery_compliance").and_then(Value::as_f64),
        verdict: str_field(sc, "verdict"),
        journals_evaluated: sc
            .get("journals_evaluated")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        disclaimer: str_field(sc, "disclaimer"),
        source_url: sc
            .get("source_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        citations,
    })
}

/// Parse a `tumult_chaosgraph_query` result into a [`GraphNodesOutcome`].
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_graph_query_result(result: &Value) -> Result<GraphNodesOutcome, McpError> {
    let sc = graph_structured(result, "query")?;
    Ok(GraphNodesOutcome {
        kind: str_field(sc, "kind"),
        count: sc
            .get("count")
            .and_then(Value::as_u64)
            .map_or_else(|| 0, |c| usize::try_from(c).unwrap_or(usize::MAX)),
        nodes: parse_graph_nodes(sc.get("nodes")),
    })
}

/// Parse a `tumult_chaosgraph_neighbors` result into a [`GraphEgoOutcome`].
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_graph_neighbors_result(result: &Value) -> Result<GraphEgoOutcome, McpError> {
    let sc = graph_structured(result, "neighbors")?;
    let edges = sc
        .get("edges")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|e| GraphEdge {
                    src: str_field(e, "src"),
                    rel: str_field(e, "rel"),
                    dst: str_field(e, "dst"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(GraphEgoOutcome {
        node_id: str_field(sc, "node_id"),
        depth: sc
            .get("depth")
            .and_then(Value::as_u64)
            .and_then(|d| u32::try_from(d).ok())
            .unwrap_or(1),
        nodes: parse_graph_nodes(sc.get("nodes")),
        edges,
    })
}

/// Parse a `tumult_chaosgraph_coverage_gaps` result into a
/// [`CoverageGapsOutcome`].
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_coverage_gaps_result(result: &Value) -> Result<CoverageGapsOutcome, McpError> {
    let sc = graph_structured(result, "coverage_gaps")?;
    let gaps = sc
        .get("gaps")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|g| CoverageGap {
                    id: str_field(g, "id"),
                    plugin: str_field(g, "plugin"),
                    action: str_field(g, "action"),
                    domain: str_field(g, "domain"),
                })
                .collect()
        })
        .unwrap_or_default();
    let unevidenced_articles = sc
        .get("unevidenced_articles")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|a| UnevidencedArticle {
                    id: str_field(a, "id"),
                    control_id: str_field(a, "control_id"),
                    strength: str_field(a, "strength"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(CoverageGapsOutcome {
        count: sc
            .get("count")
            .and_then(Value::as_u64)
            .map_or_else(|| 0, |c| usize::try_from(c).unwrap_or(usize::MAX)),
        gaps,
        framework: sc
            .get("framework")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        unevidenced_articles,
    })
}

// ── Authoring parsers (fault catalog / scaffold) ──────────────

/// Parse a `tumult_fault_catalog` result into a [`CatalogOutcome`] from its
/// `structuredContent` (`{action_count, domains:[{domain,label,actions:[…]}]}`).
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_catalog_result(result: &Value) -> Result<CatalogOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "fault_catalog tool reported an error".to_string(),
        )));
    }
    let sc = result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol("fault_catalog result missing structuredContent".to_string())
    })?;
    let domains = sc
        .get("domains")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_catalog_domain).collect())
        .unwrap_or_default();
    let action_count = sc
        .get("action_count")
        .and_then(Value::as_u64)
        .map_or_else(|| 0, |c| usize::try_from(c).unwrap_or(usize::MAX));
    Ok(CatalogOutcome {
        action_count,
        domains,
    })
}

/// Parse one `{domain, label, actions}` object.
fn parse_catalog_domain(v: &Value) -> CatalogDomain {
    let actions = v
        .get("actions")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_catalog_action).collect())
        .unwrap_or_default();
    CatalogDomain {
        domain: str_field(v, "domain"),
        label: str_field(v, "label"),
        actions,
    }
}

/// Parse one `{plugin, name, description, kind, args}` action object.
fn parse_catalog_action(v: &Value) -> CatalogAction {
    let args = v
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|a| CatalogArg {
                    name: str_field(a, "name"),
                    required: a.get("required").and_then(Value::as_bool).unwrap_or(false),
                    description: str_field(a, "description"),
                })
                .collect()
        })
        .unwrap_or_default();
    CatalogAction {
        plugin: str_field(v, "plugin"),
        name: str_field(v, "name"),
        description: str_field(v, "description"),
        kind: str_field(v, "kind"),
        args,
    }
}

/// Parse a `tumult_scaffold_experiment` result into a [`ScaffoldOutcome`] from
/// its `structuredContent` (`{action, toon, valid, validation_error?}`).
///
/// A scaffold that produces an *invalid* experiment is NOT a tool error — the
/// tool returns `valid: false` with a `validation_error`, which the UI badges.
/// Only a true tool-level failure (`isError: true`) maps to an error here.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_scaffold_result(result: &Value) -> Result<ScaffoldOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "scaffold_experiment tool reported an error".to_string(),
        )));
    }
    let sc = result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol("scaffold_experiment result missing structuredContent".to_string())
    })?;
    Ok(ScaffoldOutcome {
        action: str_field(sc, "action"),
        toon: str_field(sc, "toon"),
        valid: sc.get("valid").and_then(Value::as_bool).unwrap_or(false),
        validation_error: sc
            .get("validation_error")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

/// Parse a `tumult_whoami` result into a [`WhoamiOutcome`] from its
/// `structuredContent` (`{role, authenticated}`).
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing structured
/// content.
pub fn parse_whoami_result(result: &Value) -> Result<WhoamiOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "whoami tool reported an error".to_string()),
        ));
    }
    let sc = result
        .get("structuredContent")
        .ok_or_else(|| McpError::Protocol("whoami result missing structuredContent".to_string()))?;
    let role = sc
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("whoami result missing role".to_string()))?
        .to_string();
    let authenticated = sc
        .get("authenticated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(WhoamiOutcome {
        role,
        authenticated,
    })
}

/// Pull the `structuredContent` object from a ChaosGraph tool result, mapping a
/// tool-level error into [`McpError::Protocol`].
fn graph_structured<'a>(result: &'a Value, tool: &str) -> Result<&'a Value, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || format!("chaosgraph {tool} tool reported an error"),
        )));
    }
    result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol(format!(
            "chaosgraph {tool} result missing structuredContent"
        ))
    })
}

/// Parse a `nodes` array of `{id, kind, label}` objects.
fn parse_graph_nodes(nodes: Option<&Value>) -> Vec<GraphNode> {
    nodes
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|n| GraphNode {
                    id: str_field(n, "id"),
                    kind: str_field(n, "kind"),
                    label: str_field(n, "label"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read a string field from a JSON object, defaulting to empty.
fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Find the first line beginning with `label` and parse the remainder as a
/// count (e.g. `Plugins: 12` with label `Plugins:` → `12`).
fn labeled_count(text: &str, label: &str) -> Option<usize> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(label)
            .and_then(|rest| rest.trim().parse::<usize>().ok())
    })
}

/// Parse the whitespace-delimited number immediately preceding `suffix`
/// (e.g. `… 3 method steps …` with suffix `method step` → `3`).
fn number_before(text: &str, suffix: &str) -> Option<usize> {
    let idx = text.find(suffix)?;
    text[..idx]
        .split_whitespace()
        .next_back()
        .and_then(|tok| tok.parse::<usize>().ok())
}

/// First text block from a `content` array, if any.
fn content_text(result: &Value) -> Option<String> {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find_map(|b| b.get("text").and_then(Value::as_str)))
        .map(ToString::to_string)
}
