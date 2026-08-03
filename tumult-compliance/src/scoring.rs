//! Gremlin-style resilience scoring with freshness decay.
//!
//! Per experiment/target: a passed run scores 100, a passed run older than
//! 30 days decays to 75 (stale), a failed/deviated run scores 50, and never
//! run scores 0. Bands: > 70 good, 50–70 fair, < 50 poor. The portfolio
//! rollup is the mean of target scores with a period-over-period delta.
//!
//! Verified manual evidence scores exactly like automated telemetry
//! (passed 100 / partial 75 / failed 50; inconclusive outcomes are excluded
//! from scoring entirely). Draft/submitted manual records carry no score
//! weight — they surface via [`pending_manual_leaves`] for coverage.

use tumult_lake::Reader;

/// Staleness threshold: a pass older than this decays from 100 to 75.
pub const STALE_NS: i64 = 30 * 86_400 * 1_000_000_000;

/// `col IN ('a', 'b')` predicate confining a query to a principal's
/// environment scopes, or `None` when the set is empty (all environments —
/// the API layer's unscoped-principal case). Values are single-quote
/// doubled, matching the builders' SQL quoting.
pub(crate) fn env_in(col: &str, envs: &[String]) -> Option<String> {
    if envs.is_empty() {
        return None;
    }
    let list = envs
        .iter()
        .map(|e| format!("'{}'", e.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("{col} IN ({list})"))
}

/// `" AND <predicate>"` suffix for a WHERE clause, or empty when the
/// predicate is `None` (unscoped).
pub(crate) fn and_env(pred: Option<String>) -> String {
    pred.map_or_else(String::new, |p| format!(" AND {p}"))
}

/// Run states driving the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Passed,
    Stale,
    /// A verified manual run with a `partial` outcome: always 75.
    Partial,
    Failed,
    NeverRun,
}

/// Score one entity from its latest run (`ts_ns`, passed). Pure.
#[must_use]
pub fn score_run(latest: Option<(i64, bool)>, as_of_ns: i64) -> (u32, RunState) {
    match latest {
        None => (0, RunState::NeverRun),
        Some((_, false)) => (50, RunState::Failed),
        Some((ts, true)) if as_of_ns - ts > STALE_NS => (75, RunState::Stale),
        Some((_, true)) => (100, RunState::Passed),
    }
}

/// Band for a score: > 70 good, 50–70 fair, < 50 poor.
#[must_use]
pub fn band(score: f64) -> &'static str {
    if score > 70.0 {
        "good"
    } else if score >= 50.0 {
        "fair"
    } else {
        "poor"
    }
}

/// One scored experiment (latest run decides).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExperimentScore {
    pub name: String,
    pub target: Option<String>,
    pub score: u32,
    pub state: RunState,
    pub band: String,
    pub last_run_ns: Option<i64>,
    pub last_outcome: Option<String>,
    /// Fault severity of the latest run (`fault_severity` on the root span).
    pub severity: Option<String>,
    pub runs: u64,
    /// `automated` (OTLP telemetry) or `manual` (verified manual evidence).
    pub origin: String,
}

/// One scored target (mean of its experiment scores).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TargetScore {
    pub target: String,
    pub score: f64,
    pub band: String,
    pub runs: u64,
    pub last_run_ns: Option<i64>,
}

/// Full scorecard for `GET /api/scores` and the executive digest.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Scorecard {
    pub portfolio: f64,
    pub band: String,
    pub delta: Option<f64>,
    pub as_of_ns: i64,
    pub targets: Vec<TargetScore>,
    pub experiments: Vec<ExperimentScore>,
}

/// Latest run per experiment: (ts_ns, outcome) for the most recent root
/// span, plus run counts. Outcome joins tumult's `experiment.completed` log.
/// Verified manual records (excluding inconclusive outcomes) are UNIONed in
/// with `origin = 'manual'`; the latest row is picked per (name, origin), so
/// an experiment name present in both worlds yields one row per origin.
/// `{ENV_*}` placeholders take the environment-scope predicates (empty when
/// unscoped): automated rows bind `target_environment` directly on the root
/// span, manual rows on the manual record.
const LATEST_SQL: &str = "SELECT * FROM ( \
     SELECT s.experiment_name AS name, s.target_system AS target, \
     s.ts_ns AS ts, s.fault_severity AS severity, l.log_attrs['status'] AS status, \
     'automated' AS origin, \
     (SELECT COUNT(*) FROM spans r WHERE r.span_name = 'resilience.experiment' \
      AND r.experiment_name = s.experiment_name AND r.ts_ns <= {AS_OF}{ENV_R}) AS runs \
     FROM spans s LEFT JOIN logs l \
       ON l.log_attrs['experiment_id'] = s.experiment_id \
      AND l.body = 'experiment.completed' \
     WHERE s.span_name = 'resilience.experiment' AND s.experiment_name IS NOT NULL \
       AND s.ts_ns <= {AS_OF}{ENV_S} \
     UNION ALL \
     SELECT m.experiment_name, m.target_system, m.executed_at_ns, NULL, \
     m.outcome_status, 'manual', 1 \
     FROM manual_experiments m \
     WHERE m.status = 'verified' AND m.outcome_status != 'inconclusive' \
       AND m.executed_at_ns <= {AS_OF}{ENV_M} \
     ) QUALIFY ROW_NUMBER() OVER (PARTITION BY name, origin ORDER BY ts DESC) = 1";

/// Compute the scorecard as of `as_of_ns`, unscoped (all environments);
/// `delta` compares against the portfolio as of `as_of_ns - period_ns` when
/// a period is given.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn compute(
    reader: &Reader,
    as_of_ns: i64,
    period_ns: Option<i64>,
) -> Result<Scorecard, String> {
    compute_scoped(reader, as_of_ns, period_ns, &[])
}

/// Scoped variant of [`compute`]: `envs` confines every aggregate to the
/// given environments (empty = unscoped). Scoped principals get a scorecard
/// of their own environments only.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn compute_scoped(
    reader: &Reader,
    as_of_ns: i64,
    period_ns: Option<i64>,
    envs: &[String],
) -> Result<Scorecard, String> {
    let mut card = compute_as_of(reader, as_of_ns, envs)?;
    if let Some(period) = period_ns {
        let prev = compute_as_of(reader, as_of_ns - period, envs)?;
        card.delta = Some(card.portfolio - prev.portfolio);
    }
    Ok(card)
}

/// Portfolio score sampled at `points` evenly spaced instants across
/// `(as_of_ns - period_ns, as_of_ns]` — the R1 trend chart. X values are
/// bucket indices `1..=points`.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn portfolio_series(
    reader: &Reader,
    as_of_ns: i64,
    period_ns: i64,
    points: usize,
) -> Result<Vec<(f64, f64)>, String> {
    portfolio_series_scoped(reader, as_of_ns, period_ns, points, &[])
}

/// Scoped variant of [`portfolio_series`] (empty `envs` = unscoped).
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn portfolio_series_scoped(
    reader: &Reader,
    as_of_ns: i64,
    period_ns: i64,
    points: usize,
    envs: &[String],
) -> Result<Vec<(f64, f64)>, String> {
    let points = points.max(2) as i64;
    let step = period_ns / points;
    let mut out = Vec::with_capacity(points as usize);
    for i in 1..=points {
        let t = as_of_ns - period_ns + step * i;
        out.push((i as f64, compute_as_of(reader, t, envs)?.portfolio));
    }
    Ok(out)
}

fn compute_as_of(reader: &Reader, as_of_ns: i64, envs: &[String]) -> Result<Scorecard, String> {
    let sql = LATEST_SQL
        .replace("{AS_OF}", &as_of_ns.to_string())
        .replace("{ENV_S}", &and_env(env_in("s.target_environment", envs)))
        .replace("{ENV_R}", &and_env(env_in("r.target_environment", envs)))
        .replace("{ENV_M}", &and_env(env_in("m.target_environment", envs)));
    let rows = reader.query_json_rows(&sql).map_err(|e| e.to_string())?;

    let mut experiments = Vec::new();
    for row in rows {
        let Some(name) = row.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let target = row
            .get("target")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let ts = row.get("ts").and_then(serde_json::Value::as_i64);
        let outcome = row
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let runs = row
            .get("runs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let severity = row
            .get("severity")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        // tumult outcomes: Completed passes; Deviated/Failed fail; a missing
        // outcome (incomplete run) counts as a failed attempt — conservative.
        // Manual outcomes (lowercase): passed passes, partial scores 75,
        // failed fails (inconclusive rows never reach the query).
        let (score, state) = match outcome.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("completed") | Some("passed") => score_run(ts.map(|t| (t, true)), as_of_ns),
            Some("partial") => (75, RunState::Partial),
            _ => score_run(ts.map(|t| (t, false)), as_of_ns),
        };
        let origin = row
            .get("origin")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("automated")
            .to_string();
        experiments.push(ExperimentScore {
            name: name.to_string(),
            target,
            score,
            state,
            band: band(f64::from(score)).to_string(),
            last_run_ns: ts,
            last_outcome: outcome,
            severity,
            runs,
            origin,
        });
    }
    experiments.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.name.cmp(&b.name)));

    // Targets: equal-weight mean of their experiment scores.
    let mut by_target: std::collections::BTreeMap<String, Vec<&ExperimentScore>> =
        std::collections::BTreeMap::new();
    for e in &experiments {
        let key = e.target.clone().unwrap_or_else(|| "(untargeted)".into());
        by_target.entry(key).or_default().push(e);
    }
    let mut targets: Vec<TargetScore> = by_target
        .into_iter()
        .map(|(target, exps)| {
            let score = exps.iter().map(|e| f64::from(e.score)).sum::<f64>() / exps.len() as f64;
            TargetScore {
                target,
                score,
                band: band(score).to_string(),
                runs: exps.iter().map(|e| e.runs).sum(),
                last_run_ns: exps.iter().filter_map(|e| e.last_run_ns).max(),
            }
        })
        .collect();
    targets.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let portfolio = if targets.is_empty() {
        0.0
    } else {
        targets.iter().map(|t| t.score).sum::<f64>() / targets.len() as f64
    };
    Ok(Scorecard {
        portfolio,
        band: band(portfolio).to_string(),
        delta: None,
        as_of_ns,
        targets,
        experiments,
    })
}

/// Names of manual records still pending verification (`draft`/`submitted`).
/// They count toward org coverage as expected-but-unscored leaves.
///
/// # Errors
/// Returns the store error string when the query fails.
pub fn pending_manual_leaves(reader: &Reader) -> Result<Vec<String>, String> {
    pending_manual_leaves_scoped(reader, &[])
}

/// Scoped variant of [`pending_manual_leaves`] (empty `envs` = unscoped).
///
/// # Errors
/// Returns the store error string when the query fails.
pub fn pending_manual_leaves_scoped(
    reader: &Reader,
    envs: &[String],
) -> Result<Vec<String>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT experiment_name AS name FROM manual_experiments \
             WHERE status IN ('draft', 'submitted'){} ORDER BY experiment_name",
            and_env(env_in("target_environment", envs))
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            r.get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400 * 1_000_000_000;

    #[test]
    fn score_run_covers_all_states() {
        let now = 100 * DAY;
        assert_eq!(score_run(None, now), (0, RunState::NeverRun));
        assert_eq!(
            score_run(Some((now - DAY, true)), now),
            (100, RunState::Passed)
        );
        // Exactly 30d is still fresh; 31d is stale.
        assert_eq!(
            score_run(Some((now - 30 * DAY, true)), now),
            (100, RunState::Passed)
        );
        assert_eq!(
            score_run(Some((now - 31 * DAY, true)), now),
            (75, RunState::Stale)
        );
        assert_eq!(
            score_run(Some((now - DAY, false)), now),
            (50, RunState::Failed)
        );
    }

    #[test]
    fn bands_match_thresholds() {
        assert_eq!(band(100.0), "good");
        assert_eq!(band(70.1), "good");
        assert_eq!(band(70.0), "fair");
        assert_eq!(band(50.0), "fair");
        assert_eq!(band(49.9), "poor");
        assert_eq!(band(0.0), "poor");
    }

    #[test]
    fn compute_scoped_confines_to_in_scope_environments() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let root = |id: &str, name: &str, env: &str, ts: i64| tumult_lake::SpanRow {
            ts_ns: ts,
            trace_id: format!("trace-{id}"),
            span_id: format!("span-{id}-root"),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: DAY,
            service_name: "tumult".into(),
            experiment_id: Some(id.into()),
            experiment_name: Some(name.into()),
            target_system: Some("database".into()),
            target_environment: Some(env.into()),
            events: "[]".into(),
            ..Default::default()
        };
        let now = 100 * DAY;
        store
            .writer()
            .unwrap()
            .insert_spans(&[
                root("exp-stg", "stg-exp", "staging", now - DAY),
                root("exp-prd", "prd-exp", "prod", now - DAY),
                // A second run of the staging experiment: run counts stay
                // confined too.
                root("exp-stg-2", "stg-exp", "staging", now - 2 * DAY),
            ])
            .unwrap();
        let reader = store.read_only().unwrap();

        let global = compute(&reader, now, None).unwrap();
        assert_eq!(global.experiments.len(), 2);

        let scoped = compute_scoped(&reader, now, None, &["staging".to_string()]).unwrap();
        assert_eq!(scoped.experiments.len(), 1);
        assert_eq!(scoped.experiments[0].name, "stg-exp");
        assert_eq!(scoped.experiments[0].runs, 2);
        assert_eq!(scoped.targets.len(), 1);
        assert_eq!(scoped.targets[0].runs, 2);
    }

    #[test]
    fn delta_compares_against_the_previous_period() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let root = |id: &str, ts: i64| tumult_lake::SpanRow {
            ts_ns: ts,
            trace_id: format!("trace-{id}"),
            span_id: format!("span-{id}"),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: DAY,
            service_name: "tumult".into(),
            experiment_id: Some(id.into()),
            experiment_name: Some("exp".into()),
            target_system: Some("db".into()),
            events: "[]".into(),
            ..Default::default()
        };
        let done = |id: &str, status: &str, ts: i64| tumult_lake::LogRow {
            ts_ns: ts,
            severity_text: "INFO".into(),
            body: "experiment.completed".into(),
            trace_id: Some(format!("trace-{id}")),
            span_id: None,
            service_name: "tumult".into(),
            log_attrs: vec![
                ("experiment_id".to_string(), id.to_string()),
                ("status".to_string(), status.to_string()),
            ],
            resource_attrs: vec![],
        };
        let now = 100 * DAY;
        store
            .writer()
            .unwrap()
            .insert_spans(&[root("exp-old", now - 10 * DAY), root("exp-new", now - DAY)])
            .unwrap();
        store
            .writer()
            .unwrap()
            .insert_logs(&[
                done("exp-old", "Completed", now - 10 * DAY),
                done("exp-new", "Deviated", now - DAY),
            ])
            .unwrap();
        let reader = store.read_only().unwrap();

        let card = compute(&reader, now, Some(7 * DAY)).unwrap();
        assert_eq!(card.portfolio, 50.0); // latest run deviated
                                          // A week ago the latest run was the pass: portfolio 100 → delta -50.
        assert_eq!(card.delta, Some(-50.0));
        // Without a period there is no comparison point.
        assert_eq!(compute(&reader, now, None).unwrap().delta, None);
    }

    #[test]
    fn portfolio_series_samples_evenly_spaced_instants() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let now = 100 * DAY;
        store
            .writer()
            .unwrap()
            .insert_spans(&[tumult_lake::SpanRow {
                ts_ns: now - DAY,
                trace_id: "trace-s".into(),
                span_id: "span-s".into(),
                span_name: "resilience.experiment".into(),
                span_kind: "Internal".into(),
                duration_ns: DAY,
                service_name: "tumult".into(),
                experiment_id: Some("exp-s".into()),
                experiment_name: Some("exp".into()),
                events: "[]".into(),
                ..Default::default()
            }])
            .unwrap();
        let reader = store.read_only().unwrap();

        let series = portfolio_series(&reader, now, 7 * DAY, 5).unwrap();
        assert_eq!(series.len(), 5);
        // Bucket indices run 1..=points. The only run sits at now-1d, so
        // samples before it see an empty store (0) and the last sample sees
        // the run with no outcome log (failed → 50).
        assert_eq!(series[0].0, 1.0);
        assert!(series[..4].iter().all(|(_, v)| *v == 0.0));
        assert_eq!(series[4].1, 50.0);

        // Scoped to an environment with no spans, every sample is zero.
        let scoped =
            portfolio_series_scoped(&reader, now, 7 * DAY, 3, &["elsewhere".to_string()]).unwrap();
        assert_eq!(scoped.len(), 3);
        assert!(scoped.iter().all(|(_, v)| *v == 0.0));
    }

    #[test]
    fn manual_outcomes_flow_into_the_scorecard() {
        use tumult_lake::{ExerciseType, ManualOutcome, NewManualExperiment};

        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let record = |name: &str, outcome: ManualOutcome| NewManualExperiment {
            experiment_name: name.into(),
            exercise_type: ExerciseType::Drill,
            executed_at_ns: 90 * DAY,
            hypothesis: "h".into(),
            method: "m".into(),
            outcome,
            hypothesis_met: None,
            findings: None,
            action_items: vec![],
            target_system: Some("svc".into()),
            target_environment: Some("prod".into()),
            blast_radius: None,
            recovery_time_s: None,
            duration_s: None,
            entered_by: "alice".into(),
            attestation: "attested".into(),
            renewal_due_ns: None,
            framework_refs: vec![],
        };
        let verify = |name: &str, outcome: ManualOutcome| {
            let id = writer.create_manual_draft(&record(name, outcome)).unwrap();
            writer.submit_manual(&id, None, "alice").unwrap();
            writer.verify_manual(&id, "bob", None).unwrap();
        };
        verify("partial-exp", ManualOutcome::Partial);
        verify("inconclusive-exp", ManualOutcome::Inconclusive);
        // A draft stays pending: coverage leaf, no score.
        writer
            .create_manual_draft(&record("draft-exp", ManualOutcome::Passed))
            .unwrap();

        let reader = store.read_only().unwrap();
        let card = compute(&reader, 100 * DAY, None).unwrap();
        // The inconclusive outcome is excluded from scoring entirely.
        assert_eq!(card.experiments.len(), 1);
        let exp = &card.experiments[0];
        assert_eq!(exp.name, "partial-exp");
        assert_eq!(exp.score, 75);
        assert_eq!(exp.state, RunState::Partial);
        assert_eq!(exp.origin, "manual");

        // Pending records surface as coverage leaves, draft included.
        assert_eq!(pending_manual_leaves(&reader).unwrap(), ["draft-exp"]);
        // …confined to scope when scopes are given.
        assert_eq!(
            pending_manual_leaves_scoped(&reader, &["prod".to_string()]).unwrap(),
            ["draft-exp"]
        );
        assert!(
            pending_manual_leaves_scoped(&reader, &["staging".to_string()])
                .unwrap()
                .is_empty()
        );
    }
}
