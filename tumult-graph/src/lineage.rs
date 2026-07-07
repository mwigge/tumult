//! Compliance lineage: per-(article, service) status computed purely from
//! edge rows.
//!
//! The question this module answers is the auditor's question: *for each
//! regulatory control and each service, does the latest chaos evidence say
//! the control is evidenced, broken, or simply untested?* It consumes raw
//! [`EdgeRecord`] rows (read back from storage by `tumult-analytics`) plus
//! parsed deviation attrs, and never touches a database — mirroring the
//! coverage/topology pattern so the computation is deterministic and unit
//! testable without `DuckDB`.
//!
//! # Semantics
//!
//! A **candidate** `(article, service)` pair exists when one run carries both
//! `experiment -[maps_to_compliance]-> article` and
//! `experiment -[targets]-> service`. Only the **latest** run per
//! `(experiment, article, service)` counts — later `ts` wins, ties broken by
//! lexicographically greater `run_id` so output never depends on row order.
//! When several experiments cover the same cell, the cell is *worst-of* their
//! latest runs: one currently-broken experiment marks the control broken on
//! that service even if another experiment still evidences it, because the
//! freshest bad news is what an operator must act on.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::model::{EdgeRecord, NodeSummary};

/// The status of one regulatory control on one service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlServiceStatus {
    /// The latest relevant run produced an `evidences` edge.
    Evidenced,
    /// The latest relevant run mapped to the control but produced no
    /// evidence (usually because the run exhibited a deviation).
    Broken,
    /// No run has ever mapped this control to this service.
    Untested,
}

/// Why a control is broken on a service: the deviation the latest run
/// exhibited, plus whatever attribution the graph carries for it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BreakCause {
    /// The deviation node id (`dev:<experiment_id>`). Empty when the run
    /// produced no evidence but exhibited no deviation either.
    pub deviation_id: String,
    /// The fault attributed via a `caused_by` edge, when unambiguous.
    pub fault_id: Option<String>,
    /// The halting guard's name from the deviation attrs, when present.
    pub guard_name: Option<String>,
    /// Names of failing actions from the deviation attrs.
    pub failing_actions: Vec<String>,
    /// The run whose outcome broke the cell.
    pub run_id: String,
}

/// One cell of the lineage matrix: a control × service verdict.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineageCell {
    /// Compliance article node id (`compliance:<FW>/<control>`).
    pub article_id: String,
    /// Service node id (`svc:<host>`).
    pub service_id: String,
    /// The verdict for this pair.
    pub status: ControlServiceStatus,
    /// Citation strength from the winning `evidences` edge attrs.
    pub evidence_strength: Option<String>,
    /// Break attribution, populated only for [`ControlServiceStatus::Broken`].
    pub cause: Option<BreakCause>,
    /// Experiment node ids contributing to this cell (sorted, deduped).
    pub experiments: Vec<String>,
}

/// Everything [`compute_lineage`] needs, read from storage by the caller.
pub struct LineageInput<'a> {
    /// All edge rows (any rel; irrelevant rels are ignored).
    pub edges: &'a [EdgeRecord],
    /// All service nodes.
    pub services: &'a [NodeSummary],
    /// All compliance-article nodes.
    pub articles: &'a [NodeSummary],
    /// Deviation node id → parsed node attrs (for `halt` / `failing_actions`).
    pub deviation_attrs: &'a HashMap<String, serde_json::Value>,
}

/// Per-run slice of the edges relevant to lineage.
#[derive(Default)]
struct RunEdges<'a> {
    /// Max `ts` across the run's edges — the run's timestamp for ordering.
    ts: i64,
    /// `(experiment, article)` pairs from `maps_to_compliance`.
    maps: Vec<(&'a str, &'a str)>,
    /// `(experiment, service)` pairs from `targets`.
    targets: Vec<(&'a str, &'a str)>,
    /// `(experiment, article)` → attrs JSON text from `evidences`.
    evidences: HashMap<(&'a str, &'a str), &'a str>,
    /// Deviation ids exhibited by this run's journal.
    deviations: Vec<&'a str>,
}

/// The latest `(experiment, ts, run_id)` entries contributing to one covered
/// `(article, service)` cell.
type CellEntries<'a> = BTreeMap<(&'a str, &'a str), Vec<(&'a str, i64, &'a str)>>;

/// Compute the lineage matrix, optionally scoped to one framework and/or one
/// control. `framework` matches the `compliance:<FW>/` prefix
/// case-insensitively; `control` matches the trailing `/<control>` exactly.
/// Output is sorted by `(article_id, service_id)`.
#[must_use]
pub fn compute_lineage(
    input: &LineageInput<'_>,
    framework: Option<&str>,
    control: Option<&str>,
) -> Vec<LineageCell> {
    let runs = index_runs(input.edges);
    let caused_by = index_caused_by(input.edges);

    // Latest run per (experiment, article, service): later ts wins, ties by
    // greater run_id, so the result is independent of input row order.
    let mut latest: BTreeMap<(&str, &str, &str), (i64, &str)> = BTreeMap::new();
    for (run_id, run) in &runs {
        for &(exp, article) in &run.maps {
            if !article_in_scope(article, framework, control) {
                continue;
            }
            for &(t_exp, service) in &run.targets {
                if t_exp != exp {
                    continue;
                }
                let key = (exp, article, service);
                let candidate = (run.ts, *run_id);
                match latest.get(&key) {
                    Some(best) if *best >= candidate => {}
                    _ => {
                        latest.insert(key, candidate);
                    }
                }
            }
        }
    }

    // Group by (article, service); a BTreeMap keeps covered cells sorted.
    let mut cells: CellEntries<'_> = BTreeMap::new();
    for (&(exp, article, service), &(ts, run_id)) in &latest {
        cells
            .entry((article, service))
            .or_default()
            .push((exp, ts, run_id));
    }

    let mut out = Vec::new();
    for ((article, service), entries) in &cells {
        out.push(resolve_cell(
            article,
            service,
            entries,
            &runs,
            &caused_by,
            input.deviation_attrs,
        ));
    }

    // Fill Untested for every in-scope article × service not covered above.
    let covered: BTreeSet<(&str, &str)> = cells.keys().copied().collect();
    for article in input.articles {
        if !article_in_scope(&article.id, framework, control) {
            continue;
        }
        for service in input.services {
            if covered.contains(&(article.id.as_str(), service.id.as_str())) {
                continue;
            }
            out.push(LineageCell {
                article_id: article.id.clone(),
                service_id: service.id.clone(),
                status: ControlServiceStatus::Untested,
                evidence_strength: None,
                cause: None,
                experiments: Vec::new(),
            });
        }
    }

    out.sort_by(|a, b| {
        (a.article_id.as_str(), a.service_id.as_str())
            .cmp(&(b.article_id.as_str(), b.service_id.as_str()))
    });
    out
}

/// Resolve one covered cell from the latest run of each contributing
/// experiment: worst-of semantics (any broken experiment breaks the cell).
fn resolve_cell(
    article: &str,
    service: &str,
    entries: &[(&str, i64, &str)],
    runs: &BTreeMap<&str, RunEdges<'_>>,
    caused_by: &HashMap<&str, &str>,
    deviation_attrs: &HashMap<String, serde_json::Value>,
) -> LineageCell {
    let mut broken: Vec<(i64, &str)> = Vec::new();
    let mut evidenced: Vec<(i64, &str, Option<String>)> = Vec::new();
    for &(exp, ts, run_id) in entries {
        let evidence = runs
            .get(run_id)
            .and_then(|run| run.evidences.get(&(exp, article)))
            .map(|attrs| strength_of(attrs));
        match evidence {
            Some(strength) => evidenced.push((ts, run_id, strength)),
            None => broken.push((ts, run_id)),
        }
    }

    let mut experiments: Vec<String> = entries.iter().map(|&(exp, _, _)| exp.to_string()).collect();
    experiments.sort();
    experiments.dedup();

    if let Some(&(_, run_id)) = broken.iter().max() {
        // Deterministic deviation pick: lexicographically smallest id.
        let deviation = runs
            .get(run_id)
            .and_then(|run| run.deviations.iter().min())
            .map(|dev| (*dev).to_string());
        let cause = build_cause(deviation, run_id, caused_by, deviation_attrs);
        return LineageCell {
            article_id: article.to_string(),
            service_id: service.to_string(),
            status: ControlServiceStatus::Broken,
            evidence_strength: None,
            cause: Some(cause),
            experiments,
        };
    }

    let strength = evidenced
        .iter()
        .max_by_key(|(ts, run_id, _)| (*ts, *run_id))
        .and_then(|(_, _, strength)| strength.clone());
    LineageCell {
        article_id: article.to_string(),
        service_id: service.to_string(),
        status: ControlServiceStatus::Evidenced,
        evidence_strength: strength,
        cause: None,
        experiments,
    }
}

/// Assemble a [`BreakCause`], tolerating missing deviation/attribution: a run
/// that mapped a control but yielded neither evidence nor deviation is still
/// broken — "no data" must never be mistaken for "evidenced".
fn build_cause(
    deviation: Option<String>,
    run_id: &str,
    caused_by: &HashMap<&str, &str>,
    deviation_attrs: &HashMap<String, serde_json::Value>,
) -> BreakCause {
    let deviation_id = deviation.unwrap_or_default();
    let fault_id = caused_by
        .get(deviation_id.as_str())
        .map(|fault| (*fault).to_string());
    let attrs = deviation_attrs.get(&deviation_id);
    let guard_name = attrs
        .and_then(|a| a.pointer("/halt/guard_name"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let failing_actions = attrs
        .and_then(|a| a.get("failing_actions"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    BreakCause {
        deviation_id,
        fault_id,
        guard_name,
        failing_actions,
        run_id: run_id.to_string(),
    }
}

/// Group relevant edges by `run_id`. The run's `ts` is the max over its
/// edges so a run's ordering never depends on which edge we happen to read.
fn index_runs(edges: &[EdgeRecord]) -> BTreeMap<&str, RunEdges<'_>> {
    let mut runs: BTreeMap<&str, RunEdges<'_>> = BTreeMap::new();
    for edge in edges {
        let run = runs.entry(edge.run_id.as_str()).or_default();
        run.ts = run.ts.max(edge.ts);
        match edge.rel.as_str() {
            "maps_to_compliance" => run.maps.push((&edge.src, &edge.dst)),
            "targets" => run.targets.push((&edge.src, &edge.dst)),
            "evidences" => {
                run.evidences.insert((&edge.src, &edge.dst), &edge.attrs);
            }
            "exhibited" => run.deviations.push(&edge.dst),
            _ => {}
        }
    }
    runs
}

/// Deviation → fault attribution from `caused_by` edges. Deterministic when
/// duplicated: the lexicographically smallest fault id wins.
fn index_caused_by(edges: &[EdgeRecord]) -> HashMap<&str, &str> {
    let mut map: HashMap<&str, &str> = HashMap::new();
    for edge in edges {
        if edge.rel == "caused_by" {
            match map.get(edge.src.as_str()) {
                Some(existing) if *existing <= edge.dst.as_str() => {}
                _ => {
                    map.insert(&edge.src, &edge.dst);
                }
            }
        }
    }
    map
}

/// Parse the `strength` attr from an `evidences` edge's JSON text.
fn strength_of(attrs: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(attrs)
        .ok()?
        .get("strength")?
        .as_str()
        .map(str::to_string)
}

/// Does an article id fall inside the requested framework/control scope?
/// Framework matches the `compliance:<FW>/` prefix case-insensitively;
/// control matches the trailing `/<control>` exactly.
fn article_in_scope(article_id: &str, framework: Option<&str>, control: Option<&str>) -> bool {
    let Some(rest) = article_id.strip_prefix("compliance:") else {
        return false;
    };
    let Some((fw, ctrl)) = rest.split_once('/') else {
        return false;
    };
    if let Some(want_fw) = framework {
        if !fw.eq_ignore_ascii_case(want_fw) {
            return false;
        }
    }
    if let Some(want_ctrl) = control {
        if ctrl != want_ctrl {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::article_in_scope;

    // Fixture-driven behaviour tests live in `tests/lineage.rs`; this module
    // covers only the private scope predicate.

    #[test]
    fn scope_matches_framework_case_insensitively_and_control_exactly() {
        let id = "compliance:DORA/Art.25";
        assert!(article_in_scope(id, None, None));
        assert!(article_in_scope(id, Some("dora"), None));
        assert!(article_in_scope(id, Some("DORA"), Some("Art.25")));
        assert!(!article_in_scope(id, Some("nis2"), None));
        assert!(!article_in_scope(id, None, Some("art.25"))); // control is exact
        assert!(!article_in_scope("svc:db", None, None));
        assert!(!article_in_scope("compliance:no-slash", None, None));
    }
}
