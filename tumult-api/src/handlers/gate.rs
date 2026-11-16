//! `GET /api/gate/{target}` — the CI gate.
//!
//! One target's resilience score, turned into a `pass` / `warn` / `fail`
//! verdict a pipeline can act on, with the evidence behind it. The score is
//! the same freshness-decayed model as `/api/scores` (so the gate and the
//! scorecard never disagree); the verdict adds a floor (`min_score`) and a
//! freshness window (`max_age_days`). Failed experiments always surface as
//! at least a `warn`, whatever the aggregate score says.

use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tumult_compliance::scoring::{ExperimentScore, RunState, Scorecard};

use crate::auth::Principal;
use crate::error::{bad_request, forbidden, not_found, ApiError};
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

const DEFAULT_MIN_SCORE: f64 = 70.0;
const DEFAULT_MAX_AGE_DAYS: u32 = 30;
const DAY_NS: i64 = 86_400 * 1_000_000_000;

#[derive(Deserialize)]
pub(crate) struct GateQuery {
    min_score: Option<f64>,
    max_age_days: Option<u32>,
    env: Option<String>,
}

/// Gate thresholds after validation.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub(crate) struct Thresholds {
    pub min_score: f64,
    pub max_age_days: u32,
}

impl Thresholds {
    fn from_query(q: &GateQuery) -> Result<Self, ApiError> {
        let min_score = q.min_score.unwrap_or(DEFAULT_MIN_SCORE);
        if !(0.0..=100.0).contains(&min_score) || min_score.is_nan() {
            return Err(bad_request("min_score must be between 0 and 100"));
        }
        Ok(Self {
            min_score,
            max_age_days: q.max_age_days.unwrap_or(DEFAULT_MAX_AGE_DAYS),
        })
    }
}

/// The gate's answer. `pass` only when the score clears the floor, the
/// latest evidence is inside the window and no experiment is failing.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Verdict {
    Pass,
    Warn,
    Fail,
}

/// Pure verdict: `fail` below the floor; `warn` on stale evidence or any
/// failed experiment; `pass` otherwise. Reasons explain every non-pass.
pub(crate) fn verdict(
    score: f64,
    experiments: &[&ExperimentScore],
    as_of_ns: i64,
    t: Thresholds,
) -> (Verdict, Vec<String>) {
    let mut reasons = Vec::new();
    let mut v = Verdict::Pass;
    if score < t.min_score {
        v = Verdict::Fail;
        reasons.push(format!(
            "score {score:.1} is below the minimum {:.1}",
            t.min_score
        ));
    }
    let last_run = experiments.iter().filter_map(|e| e.last_run_ns).max();
    match last_run {
        None => {
            v = Verdict::Fail;
            reasons.push("no run evidence for this target".into());
        }
        Some(ts) if as_of_ns - ts > i64::from(t.max_age_days) * DAY_NS => {
            if v == Verdict::Pass {
                v = Verdict::Warn;
            }
            reasons.push(format!(
                "evidence is stale: latest run is older than {} days",
                t.max_age_days
            ));
        }
        Some(_) => {}
    }
    let failed: Vec<&str> = experiments
        .iter()
        .filter(|e| e.state == RunState::Failed)
        .map(|e| e.name.as_str())
        .collect();
    if !failed.is_empty() {
        if v == Verdict::Pass {
            v = Verdict::Warn;
        }
        reasons.push(format!(
            "{} experiment(s) failed on the latest run: {}",
            failed.len(),
            failed.join(", ")
        ));
    }
    (v, reasons)
}

/// `GET /api/gate/{target}?min_score=&max_age_days=&env=`.
pub(crate) async fn gate(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(target): Path<String>,
    Query(q): Query<GateQuery>,
) -> Result<Json<Value>, ApiError> {
    let thresholds = Thresholds::from_query(&q)?;
    let envs: Vec<String> = match q.env {
        Some(env) => {
            if !principal.env_allowed(&env) {
                return Err(forbidden(format!(
                    "environment {env} is outside the principal's scopes"
                )));
            }
            vec![env]
        }
        None => principal.env_scopes.clone(),
    };
    let as_of_ns = now_ns();
    let card: Scorecard = with_reader(&state.db_path, move |reader| {
        tumult_compliance::scoring::compute_scoped(reader, as_of_ns, None, &envs)
    })
    .await?;
    let Some(t) = card.targets.iter().find(|t| t.target == target) else {
        return Err(not_found(format!("no evidence for target {target}")));
    };
    let experiments: Vec<&ExperimentScore> = card
        .experiments
        .iter()
        .filter(|e| e.target.as_deref() == Some(target.as_str()))
        .collect();
    let (verdict, reasons) = verdict(t.score, &experiments, as_of_ns, thresholds);
    let latest = experiments
        .iter()
        .filter(|e| e.last_run_ns.is_some())
        .max_by_key(|e| e.last_run_ns);
    let body = json!({
        "target": target,
        "score": t.score,
        "band": t.band,
        "verdict": verdict,
        "reasons": reasons,
        "thresholds": thresholds,
        "as_of_ns": as_of_ns,
        "evidence": {
            "runs": t.runs,
            "last_run_id": latest.and_then(|e| e.evidence_id.clone()),
            "last_run_at_ns": t.last_run_ns,
            "last_outcome": latest.and_then(|e| e.last_outcome.clone()),
            "experiments": serde_json::to_value(&experiments).map_err(|e| internal(e.to_string()))?,
        },
    });
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp(name: &str, state: RunState, score: u32, last_run_ns: Option<i64>) -> ExperimentScore {
        ExperimentScore {
            name: name.into(),
            target: Some("db".into()),
            environment: Some("demo".into()),
            evidence_id: Some(format!("run-{name}")),
            score,
            state,
            band: "good".into(),
            last_run_ns,
            last_outcome: None,
            severity: None,
            runs: 1,
            origin: "automated".into(),
            journal_only: false,
        }
    }

    const T: Thresholds = Thresholds {
        min_score: 70.0,
        max_age_days: 30,
    };

    #[test]
    fn passes_when_fresh_and_clean() {
        let e = exp("a", RunState::Passed, 100, Some(1_000));
        let (v, reasons) = verdict(100.0, &[&e], 2_000, T);
        assert_eq!(v, Verdict::Pass);
        assert!(reasons.is_empty());
    }

    #[test]
    fn fails_below_floor_and_on_no_evidence() {
        let e = exp("a", RunState::Passed, 100, Some(1_000));
        let (v, reasons) = verdict(60.0, &[&e], 2_000, T);
        assert_eq!(v, Verdict::Fail);
        assert!(reasons[0].contains("below"));

        let (v, reasons) = verdict(0.0, &[], 2_000, T);
        assert_eq!(v, Verdict::Fail);
        assert!(reasons.iter().any(|r| r.contains("no run evidence")));
    }

    #[test]
    fn warns_on_failed_experiment_and_stale_evidence() {
        let ok = exp("a", RunState::Passed, 100, Some(1_000));
        let bad = exp("b", RunState::Failed, 50, Some(1_500));
        let (v, reasons) = verdict(75.0, &[&ok, &bad], 2_000, T);
        assert_eq!(v, Verdict::Warn);
        assert!(reasons[0].contains("b"));

        let old = exp("a", RunState::Stale, 75, Some(0));
        let (v, reasons) = verdict(75.0, &[&old], 31 * DAY_NS, T);
        assert_eq!(v, Verdict::Warn);
        assert!(reasons[0].contains("stale"));
    }

    #[test]
    fn fail_outranks_warn() {
        let bad = exp("b", RunState::Failed, 50, Some(0));
        let (v, reasons) = verdict(50.0, &[&bad], 31 * DAY_NS, T);
        assert_eq!(v, Verdict::Fail);
        assert_eq!(reasons.len(), 3);
    }
}
