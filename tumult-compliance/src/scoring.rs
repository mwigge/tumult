//! Gremlin-style compliance scoring with freshness decay.
//!
//! Per experiment/target: a passed run scores 100, a passed run older than
//! 30 days decays to 75 (stale), a failed/deviated/halted run scores 50, an
//! aborted or interrupted run proves nothing and is left out, and never
//! run scores 0. Bands: > 70 good, 50–70 fair, < 50 poor. The portfolio
//! rollup is the mean of target scores with a period-over-period delta;
//! with nothing on record there is no portfolio score and its band is
//! `unknown`.
//!
//! Verified manual evidence scores exactly like automated telemetry
//! (passed 100 / partial 75 / failed 50; inconclusive outcomes are excluded
//! from scoring entirely). Draft/submitted manual records carry no score
//! weight — they surface via [`pending_manual_leaves`] for coverage.

use tumult_lake::{scope, Reader, RunVerdict};

/// Staleness threshold: a pass older than this decays from 100 to 75.
pub const STALE_NS: i64 = 30 * 86_400 * 1_000_000_000;

/// What the compliance score measures, in one line for a reader. The web
/// says it in the same words (`web/src/lib/score-names.ts`).
pub const COMPLIANCE_SCORE_LINE: &str = "Latest run of each experiment: pass 100, pass older \
     than 30 days 75, failure 50, never run 0. Averaged per target.";

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

impl RunState {
    /// An open weakness: the latest run failed, worked in part or never
    /// happened. A stale pass is not one: it asks for a re-run instead.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Failed | Self::Partial | Self::NeverRun)
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

/// The portfolio score of `targets`: the mean of their scores, each target
/// of equal weight. `None` with no target: no score is not a score of 0.
#[must_use]
pub fn portfolio_score(targets: &[TargetScore]) -> Option<f64> {
    if targets.is_empty() {
        return None;
    }
    Some(targets.iter().map(|t| t.score).sum::<f64>() / targets.len() as f64)
}

/// Band for a portfolio score; `unknown` with no score, as a DORA band
/// without a value ([`crate::dora::report::band`]).
#[must_use]
pub fn portfolio_band(portfolio: Option<f64>) -> &'static str {
    portfolio.map_or("unknown", band)
}

/// One scored experiment (latest run decides).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExperimentScore {
    pub name: String,
    pub target: Option<String>,
    /// Environment is part of the logical experiment identity.
    pub environment: Option<String>,
    /// Exact latest source record for provenance (manual id or automated run id).
    pub evidence_id: Option<String>,
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
    /// The latest run is in the lake as a journal only: it sent no trace.
    pub journal_only: bool,
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
    /// `None` (`null` in JSON) when nothing is on record ([`portfolio_score`]).
    pub portfolio: Option<f64>,
    /// [`portfolio_band`] of the portfolio score.
    pub band: String,
    pub delta: Option<f64>,
    pub as_of_ns: i64,
    pub targets: Vec<TargetScore>,
    pub experiments: Vec<ExperimentScore>,
}

/// Latest run per experiment: (ts_ns, outcome) for the most recent run
/// root, plus run counts. A run root is the root span of a run, or a row
/// built from its journal when the run sent no trace (`journal_only`).
/// `{STATUS}` takes the outcome of the run, by the rule of
/// [`tumult_lake::run_outcome_sql`], and `{VERDICT}` its
/// [`tumult_lake::run_verdict_sql`]: a run that proves nothing is neither
/// the latest run nor counted.
/// Verified manual records (excluding inconclusive outcomes) are UNIONed in
/// with `origin = 'manual'`; the latest row is picked per (name, target, environment, origin).
/// Display names cannot merge distinct deployment targets or environments.
/// `{ENV_*}` placeholders take the environment-scope predicates (empty when
/// unscoped): automated rows bind `target_environment` directly on the root
/// span, manual rows on the manual record.
const LATEST_SQL: &str = "SELECT * FROM ( \
     SELECT s.experiment_name AS name, s.target_system AS target, \
     s.ts_ns AS ts, s.fault_severity AS severity, {STATUS} AS status, \
     'automated' AS origin, s.target_environment AS environment, s.experiment_id AS evidence_id, \
     COUNT(*) OVER (PARTITION BY s.experiment_name, s.target_system, s.target_environment) \
     AS runs, \
     s.journal_only \
     FROM run_roots s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id \
     WHERE s.experiment_name IS NOT NULL \
       AND s.ts_ns <= {AS_OF}{ENV_S} \
       AND {VERDICT} IS DISTINCT FROM '{NO_EVIDENCE}' \
     UNION ALL \
     SELECT m.experiment_name, m.target_system, m.executed_at_ns, NULL, \
     m.outcome_status, 'manual', m.target_environment, m.id, 1, false \
     FROM manual_experiments m \
     WHERE m.status = 'verified' AND m.outcome_status != 'inconclusive' \
       AND m.executed_at_ns <= {AS_OF}{ENV_M} \
     ) QUALIFY ROW_NUMBER() OVER (PARTITION BY name, target, environment, origin ORDER BY ts DESC, evidence_id DESC) = 1";

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
        let before = compute_as_of(reader, as_of_ns - period, envs)?;
        card.delta = change_since(card.portfolio, before.portfolio);
    }
    Ok(card)
}

/// How far a score `now` is from the score `before` of an earlier instant:
/// of a portfolio, or of a node of the org tree. `None` when either is
/// missing: a first score is not a change.
#[must_use]
pub fn change_since(now: Option<f64>, before: Option<f64>) -> Option<f64> {
    Some(now? - before?)
}

/// Portfolio score sampled at `points` evenly spaced instants across
/// `(as_of_ns - period_ns, as_of_ns]` — the R1 trend chart. Each point is
/// the time of its instant and the score at that time. An instant before
/// anything was on record has no point: no score is not a score of 0.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn portfolio_series(
    reader: &Reader,
    as_of_ns: i64,
    period_ns: i64,
    points: usize,
) -> Result<Vec<(i64, f64)>, String> {
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
) -> Result<Vec<(i64, f64)>, String> {
    let points = points.max(2) as i64;
    let step = period_ns / points;
    let mut out = Vec::with_capacity(points as usize);
    for i in 1..=points {
        let t = as_of_ns - period_ns + step * i;
        if let Some(score) = compute_as_of(reader, t, envs)?.portfolio {
            out.push((t, score));
        }
    }
    Ok(out)
}

fn compute_as_of(reader: &Reader, as_of_ns: i64, envs: &[String]) -> Result<Scorecard, String> {
    let status = tumult_lake::run_outcome_sql("s", "l");
    let sql = LATEST_SQL
        .replace("{VERDICT}", &tumult_lake::run_verdict_sql(&status))
        .replace("{NO_EVIDENCE}", RunVerdict::NoEvidence.as_str())
        .replace("{STATUS}", &status)
        .replace("{AS_OF}", &as_of_ns.to_string())
        .replace(
            "{ENV_S}",
            &scope::and(scope::in_list("s.target_environment", envs)),
        )
        .replace(
            "{ENV_M}",
            &scope::and(scope::in_list("m.target_environment", envs)),
        );
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
        // tumult outcomes pass by their verdict (a run that proves nothing
        // never reaches here); a missing outcome (incomplete run) counts as
        // a failed attempt — conservative. Manual outcomes: passed passes,
        // partial scores 75, failed fails (inconclusive rows never reach
        // the query).
        let (score, state) = match outcome.as_deref() {
            Some("passed") => score_run(ts.map(|t| (t, true)), as_of_ns),
            Some("partial") => (75, RunState::Partial),
            status => {
                let passed = status.and_then(tumult_lake::run_verdict) == Some(RunVerdict::Pass);
                score_run(ts.map(|t| (t, passed)), as_of_ns)
            }
        };
        let origin = row
            .get("origin")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("automated")
            .to_string();
        experiments.push(ExperimentScore {
            name: name.to_string(),
            target,
            environment: row
                .get("environment")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            evidence_id: row
                .get("evidence_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            score,
            state,
            band: band(f64::from(score)).to_string(),
            last_run_ns: ts,
            last_outcome: outcome,
            severity,
            runs,
            origin,
            journal_only: row
                .get("journal_only")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
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
    let portfolio = portfolio_score(&targets);
    Ok(Scorecard {
        portfolio,
        band: portfolio_band(portfolio).to_string(),
        delta: None,
        as_of_ns,
        targets,
        experiments,
    })
}

/// Names of manual records still pending verification (`draft`/`submitted`)
/// in the environments `envs` (empty = unscoped). They count toward org
/// coverage as expected-but-unscored leaves.
///
/// # Errors
/// Returns the store error string when the query fails.
pub fn pending_manual_leaves(reader: &Reader, envs: &[String]) -> Result<Vec<String>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT experiment_name AS name FROM manual_experiments \
             WHERE status IN ('draft', 'submitted'){} ORDER BY experiment_name",
            scope::and(scope::in_list("target_environment", envs))
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
    fn the_line_of_the_compliance_score_says_the_numbers_it_is_scored_with() {
        let now = 100 * DAY;
        let (pass, _) = score_run(Some((now, true)), now);
        let (stale, _) = score_run(Some((now - STALE_NS - 1, true)), now);
        let (failure, _) = score_run(Some((now, false)), now);
        let (never, _) = score_run(None, now);
        let days = STALE_NS / DAY;

        assert_eq!(
            COMPLIANCE_SCORE_LINE,
            format!(
                "Latest run of each experiment: pass {pass}, pass older than {days} days \
                 {stale}, failure {failure}, never run {never}. Averaged per target."
            )
        );
    }

    #[test]
    fn a_failed_partial_or_never_run_experiment_is_open_and_a_pass_is_not() {
        for (state, open) in [
            (RunState::Passed, false),
            // Stale evidence is listed apart: it asks for a re-run.
            (RunState::Stale, false),
            (RunState::Partial, true),
            (RunState::Failed, true),
            (RunState::NeverRun, true),
        ] {
            assert_eq!(state.is_open(), open, "{state:?}");
        }
    }

    #[test]
    fn a_card_with_nothing_on_record_has_no_score_and_no_band() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let now = 100 * DAY;
        store
            .writer()
            .unwrap()
            .insert_spans(&[tumult_lake::SpanRow {
                ts_ns: now - DAY,
                trace_id: "trace-one".into(),
                span_id: "span-one".into(),
                span_name: "resilience.experiment".into(),
                experiment_id: Some("one".into()),
                experiment_name: Some("one".into()),
                outcome_status: Some("completed".into()),
                target_system: Some("db".into()),
                target_environment: Some("prod".into()),
                events: "[]".into(),
                ..Default::default()
            }])
            .unwrap();
        let reader = store.read_only().unwrap();

        // Before the run nothing is on record, and nothing ever is in an
        // environment the run did not touch.
        for card in [
            compute(&reader, now - 2 * DAY, Some(7 * DAY)).unwrap(),
            compute_scoped(&reader, now, Some(7 * DAY), &["staging".to_string()]).unwrap(),
        ] {
            let json = serde_json::to_value(&card).unwrap();
            assert!(json["portfolio"].is_null(), "{json}");
            assert_eq!(card.band, "unknown");
            assert_eq!(card.delta, None);
        }
    }

    #[test]
    fn same_name_preserves_distinct_target_and_environment_results() {
        let dir = tempfile::tempdir().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("db")).unwrap();
        let writer = store.writer().unwrap();
        for (id, target, env, ts) in [
            ("one", "payments", "prod", 1),
            ("two", "catalog", "prod", 2),
            ("three", "payments", "staging", 3),
        ] {
            writer
                .insert_spans(&[tumult_lake::SpanRow {
                    span_name: "resilience.experiment".into(),
                    experiment_name: Some("same".into()),
                    experiment_id: Some(id.into()),
                    target_system: Some(target.into()),
                    target_environment: Some(env.into()),
                    ts_ns: ts,
                    trace_id: id.into(),
                    span_id: id.into(),
                    ..Default::default()
                }])
                .unwrap();
        }
        let card = compute(&store.read_only().unwrap(), 10, None).unwrap();
        assert_eq!(card.experiments.len(), 3);
        assert!(card.experiments.iter().all(|e| e.runs == 1));
    }

    #[test]
    fn a_run_is_scored_by_its_status_in_either_spelling() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let now = 100 * DAY;
        let root = |id: &str, status: Option<&str>| tumult_lake::SpanRow {
            ts_ns: now - DAY,
            trace_id: format!("trace-{id}"),
            span_id: format!("span-{id}"),
            span_name: "resilience.experiment".into(),
            experiment_id: Some(id.into()),
            experiment_name: Some(id.into()),
            outcome_status: status.map(str::to_string),
            events: "[]".into(),
            ..Default::default()
        };
        let writer = store.writer().unwrap();
        // Stored before 3.0.2: the status is on the completed log, with a
        // capital. Stored since: it is on the root span.
        writer
            .insert_spans(&[root("before", None), root("since", Some("completed"))])
            .unwrap();
        writer
            .insert_logs(&[tumult_lake::LogRow {
                ts_ns: now - DAY,
                body: "experiment.completed".into(),
                log_attrs: vec![
                    ("experiment_id".to_string(), "before".to_string()),
                    ("status".to_string(), "Completed".to_string()),
                ],
                ..tumult_lake::LogRow::default()
            }])
            .unwrap();

        let card = compute(&store.read_only().unwrap(), now, None).unwrap();

        let mut seen: Vec<(&str, RunState, Option<&str>)> = card
            .experiments
            .iter()
            .map(|e| (e.name.as_str(), e.state, e.last_outcome.as_deref()))
            .collect();
        seen.sort_by_key(|(name, _, _)| *name);
        assert_eq!(
            seen,
            [
                ("before", RunState::Passed, Some("completed")),
                ("since", RunState::Passed, Some("completed")),
            ]
        );
    }

    #[test]
    fn a_run_that_was_aborted_or_interrupted_is_left_out_of_the_score() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let now = 100 * DAY;
        let root = |id: &str, name: &str, status: &str, ts: i64| tumult_lake::SpanRow {
            ts_ns: ts,
            trace_id: format!("trace-{id}"),
            span_id: format!("span-{id}"),
            span_name: "resilience.experiment".into(),
            experiment_id: Some(id.into()),
            experiment_name: Some(name.into()),
            outcome_status: Some(status.into()),
            target_system: Some("db".into()),
            events: "[]".into(),
            ..Default::default()
        };
        // The drill passed, then was aborted and interrupted. The other
        // experiment never got past its first check.
        store
            .writer()
            .unwrap()
            .insert_spans(&[
                root("drill-1", "drill", "completed", now - 3 * DAY),
                root("drill-2", "drill", "aborted", now - 2 * DAY),
                root("drill-3", "drill", "interrupted", now - DAY),
                root("check-1", "check", "aborted", now - DAY),
            ])
            .unwrap();

        let card = compute(&store.read_only().unwrap(), now, None).unwrap();

        let seen: Vec<(&str, RunState, Option<&str>, u64)> = card
            .experiments
            .iter()
            .map(|e| (e.name.as_str(), e.state, e.last_outcome.as_deref(), e.runs))
            .collect();
        assert_eq!(
            seen,
            [("drill", RunState::Passed, Some("completed"), 1)],
            "neither a pass nor a failure"
        );
        assert_eq!(card.portfolio, Some(100.0));
    }

    #[test]
    fn a_run_with_two_completion_logs_is_one_run_that_ended_as_the_later_says() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let now = 100 * DAY;
        let done = |status: &str, ts: i64| tumult_lake::LogRow {
            ts_ns: ts,
            body: "experiment.completed".into(),
            log_attrs: vec![
                ("experiment_id".to_string(), "twice".to_string()),
                ("status".to_string(), status.to_string()),
            ],
            ..tumult_lake::LogRow::default()
        };
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[tumult_lake::SpanRow {
                ts_ns: now - DAY,
                trace_id: "trace-twice".into(),
                span_id: "span-twice".into(),
                span_name: "resilience.experiment".into(),
                experiment_id: Some("twice".into()),
                experiment_name: Some("twice".into()),
                events: "[]".into(),
                ..Default::default()
            }])
            .unwrap();
        // The later log is stored first.
        writer
            .insert_logs(&[
                done("Completed", now - DAY + 2),
                done("Deviated", now - DAY + 1),
            ])
            .unwrap();

        let card = compute(&store.read_only().unwrap(), now, None).unwrap();

        let seen: Vec<(RunState, Option<&str>, u64)> = card
            .experiments
            .iter()
            .map(|e| (e.state, e.last_outcome.as_deref(), e.runs))
            .collect();
        assert_eq!(seen, [(RunState::Passed, Some("completed"), 1)]);
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
            ..tumult_lake::LogRow::default()
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
        assert_eq!(card.portfolio, Some(50.0)); // latest run deviated
                                                // A week ago the latest run was the pass: portfolio 100 → delta -50.
        assert_eq!(card.delta, Some(-50.0));
        // Without a period there is no comparison point.
        assert_eq!(compute(&reader, now, None).unwrap().delta, None);
        // Nor with a period that reaches back before the first run: a
        // first score is not a change.
        assert_eq!(compute(&reader, now, Some(30 * DAY)).unwrap().delta, None);
    }

    #[test]
    fn the_portfolio_series_starts_when_something_is_on_record() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let now = 100 * DAY;
        store
            .writer()
            .unwrap()
            .insert_spans(&[tumult_lake::SpanRow {
                ts_ns: now - 4 * DAY,
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
        // The instants end at `now` and are a fifth of the week apart. The
        // only run is from four days ago, so the two instants before it
        // have no point: nothing was on record, which is not a score of 0.
        // The run has no outcome log (failed → 50).
        let step = 7 * DAY / 5;
        assert_eq!(
            series,
            [(now - 2 * step, 50.0), (now - step, 50.0), (now, 50.0)]
        );

        // Scoped to an environment with no spans, there is no point at all.
        let scoped =
            portfolio_series_scoped(&reader, now, 7 * DAY, 3, &["elsewhere".to_string()]).unwrap();
        assert!(scoped.is_empty(), "{scoped:?}");
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
        assert_eq!(pending_manual_leaves(&reader, &[]).unwrap(), ["draft-exp"]);
        // …confined to scope when scopes are given.
        assert_eq!(
            pending_manual_leaves(&reader, &["prod".to_string()]).unwrap(),
            ["draft-exp"]
        );
        assert!(pending_manual_leaves(&reader, &["staging".to_string()])
            .unwrap()
            .is_empty());

        // A verified record is in the card of its own environment only.
        let scoped =
            |env: &str| compute_scoped(&reader, 100 * DAY, None, &[env.to_string()]).unwrap();
        assert_eq!(scoped("prod").experiments.len(), 1);
        assert!(scoped("staging").experiments.is_empty());
        assert_eq!(scoped("staging").portfolio, None);
    }
}
