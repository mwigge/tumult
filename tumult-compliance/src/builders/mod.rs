//! Report builders: store queries → [`ReportDoc`] for each template.
//!
//! All SQL values that flow from parameters go through [`q`] (single-quote
//! doubling). Numbers (`i64` timestamps) are formatted directly.

mod evidence_pack;
mod executive;
mod game_day;

#[cfg(test)]
mod tests;

use sha2::{Digest, Sha256};
use tumult_lake::Reader;

use crate::html::fmt_date;
use crate::model::{DocMeta, TemplateKind};
use crate::scoring::{self, RunState, Scorecard};

pub use evidence_pack::{build_evidence_pack, CLAUSE_VERIFY_FOOTNOTE, FRAMEWORK_CLAUSES};
pub use executive::build_executive;
pub use game_day::build_game_day;

const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// Quote a string value for DuckDB SQL.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Document ID: `KRK-<code>-<yyyymmdd>-<hash6>` where hash6 is the first 6
/// hex chars of SHA-256 over template + params + timestamp.
fn doc_id(code: &str, seed: &str, generated_at_ns: i64) -> String {
    let ymd = fmt_date(generated_at_ns).replace('-', "");
    let digest = Sha256::digest(seed.as_bytes());
    let hash6: String = digest[..3].iter().map(|b| format!("{b:02x}")).collect();
    format!("KRK-{code}-{ymd}-{hash6}")
}

fn base_meta(
    template: TemplateKind,
    title: String,
    generated_at_ns: i64,
    data_as_of_ns: i64,
    seed: &str,
) -> DocMeta {
    DocMeta {
        doc_id: doc_id(template.code(), seed, generated_at_ns),
        title,
        template,
        version: env!("CARGO_PKG_VERSION").to_string(),
        classification: "Internal".into(),
        generated_at_ns,
        data_as_of_ns,
        period: None,
        framework: None,
        experiment_id: None,
    }
}

fn cell<'a>(row: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(serde_json::Value::as_str)
}

/// Round to one decimal for display.
fn f1(v: f64) -> String {
    format!("{v:.1}")
}

fn bluf(card: &Scorecard, open: &[&scoring::ExperimentScore]) -> String {
    let trend = card.delta.map_or("no prior-period baseline".into(), |d| {
        if d > 0.5 {
            format!("improving ({d:+.1} vs the previous period)")
        } else if d < -0.5 {
            format!("declining ({d:+.1} vs the previous period)")
        } else {
            format!("flat ({d:+.1} vs the previous period)")
        }
    });
    let weakest: Vec<String> = card
        .targets
        .iter()
        .take(3)
        .map(|t| format!("{} ({})", t.target, f1(t.score)))
        .collect();
    let mut s = format!(
        "Portfolio resilience is {} ({}/100) and {}. Weakest targets: {}. ",
        card.band,
        f1(card.portfolio),
        trend,
        if weakest.is_empty() {
            "none — no targets on record".into()
        } else {
            weakest.join(", ")
        },
    );
    if open.is_empty() {
        s.push_str("No open weaknesses; keep the current experiment cadence.");
    } else {
        s.push_str(&format!(
            "{} open weakness(es) need a decision: re-run, remediate, or accept.",
            open.len()
        ));
    }
    s
}

fn outlook(card: &Scorecard) -> String {
    let untested = card
        .experiments
        .iter()
        .filter(|e| e.state == RunState::NeverRun)
        .count();
    let stale = card
        .experiments
        .iter()
        .filter(|e| e.state == RunState::Stale)
        .count();
    let mut pts = Vec::new();
    if untested > 0 {
        pts.push(format!(
            "schedule first runs for {untested} untested experiment(s)"
        ));
    }
    if stale > 0 {
        pts.push(format!(
            "re-run {stale} experiment(s) whose last pass is over 30 days old"
        ));
    }
    if let Some(w) = card.targets.first() {
        pts.push(format!(
            "focus next game-day on {}, the weakest target",
            w.target
        ));
    }
    if pts.is_empty() {
        "Coverage is green across the board; maintain cadence and consider widening fault scenarios.".into()
    } else {
        format!("Next period priorities: {}.", pts.join("; "))
    }
}

/// Count deviated/failed runs in `[from, to)` and how many later ran green;
/// plus MTTR from spans that report `recovery_time_s`.
/// The decision an open weakness asks of the reader, by run state.
fn decision_for(state: RunState) -> String {
    match state {
        RunState::Failed => "Re-run, remediate, or accept".into(),
        RunState::Partial => "Re-run to a full pass, or accept".into(),
        RunState::Stale => "Re-run to refresh evidence".into(),
        RunState::NeverRun => "Schedule first run".into(),
        RunState::Passed => "—".into(),
    }
}

/// Experiment names with at least one run in `[from_ns, to_ns)` — automated
/// runs plus verified manual executions in the window. Scoped to `envs`
/// (empty = unscoped).
fn runs_in_window(
    reader: &Reader,
    from_ns: i64,
    to_ns: i64,
    envs: &[String],
) -> Result<std::collections::BTreeSet<String>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT DISTINCT experiment_name AS name FROM spans \
             WHERE span_name = 'resilience.experiment' AND experiment_name IS NOT NULL \
               AND ts_ns >= {from_ns} AND ts_ns < {to_ns}{} \
             UNION \
             SELECT DISTINCT experiment_name FROM manual_experiments \
             WHERE status = 'verified' \
               AND executed_at_ns >= {from_ns} AND executed_at_ns < {to_ns}{}",
            scoring::and_env(scoring::env_in("target_environment", envs)),
            scoring::and_env(scoring::env_in("target_environment", envs)),
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| cell(r, "name").map(str::to_owned))
        .collect())
}

fn issue_stats(
    reader: &Reader,
    from_ns: i64,
    to_ns: i64,
    envs: &[String],
) -> Result<(u64, u64, Option<f64>), String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT s.experiment_name AS name, s.ts_ns AS ts, l.log_attrs['status'] AS status \
             FROM spans s LEFT JOIN logs l \
               ON l.log_attrs['experiment_id'] = s.experiment_id \
              AND l.body = 'experiment.completed' \
             WHERE s.span_name = 'resilience.experiment' AND s.experiment_name IS NOT NULL \
               AND s.ts_ns >= {from_ns} AND s.ts_ns < {to_ns}{} ORDER BY s.ts_ns",
            scoring::and_env(scoring::env_in("s.target_environment", envs)),
        ))
        .map_err(|e| e.to_string())?;

    let mut by_name: std::collections::BTreeMap<String, Vec<(i64, bool)>> =
        std::collections::BTreeMap::new();
    for row in &rows {
        let (Some(name), Some(ts)) = (
            cell(row, "name"),
            row.get("ts").and_then(serde_json::Value::as_i64),
        ) else {
            continue;
        };
        let ok = cell(row, "status") == Some("Completed");
        by_name.entry(name.to_string()).or_default().push((ts, ok));
    }
    let mut discovered = 0u64;
    let mut fixed = 0u64;
    for runs in by_name.values() {
        for (i, (_, ok)) in runs.iter().enumerate() {
            if !ok {
                discovered += 1;
                // Fixed = a completed run of the same experiment exists after
                // this failure (later in the period or up to now).
                if runs[i + 1..].iter().any(|(_, ok)| *ok) {
                    fixed += 1;
                }
            }
        }
    }

    let mttr = reader
        .query_json_rows(&format!(
            "SELECT AVG(recovery_time_s) AS v FROM spans \
             WHERE recovery_time_s IS NOT NULL AND ts_ns >= {from_ns} AND ts_ns < {to_ns}{}",
            scoring::and_env(scoring::env_in("target_environment", envs)),
        ))
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|r| r.get("v").and_then(serde_json::Value::as_f64));
    Ok((discovered, fixed, mttr))
}
