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
const LATEST_SQL: &str = "SELECT * FROM ( \
     SELECT s.experiment_name AS name, s.target_system AS target, \
     s.ts_ns AS ts, s.fault_severity AS severity, l.log_attrs['status'] AS status, \
     'automated' AS origin, \
     (SELECT COUNT(*) FROM spans r WHERE r.span_name = 'resilience.experiment' \
      AND r.experiment_name = s.experiment_name AND r.ts_ns <= {AS_OF}) AS runs \
     FROM spans s LEFT JOIN logs l \
       ON l.log_attrs['experiment_id'] = s.experiment_id \
      AND l.body = 'experiment.completed' \
     WHERE s.span_name = 'resilience.experiment' AND s.experiment_name IS NOT NULL \
       AND s.ts_ns <= {AS_OF} \
     UNION ALL \
     SELECT m.experiment_name, m.target_system, m.executed_at_ns, NULL, \
     m.outcome_status, 'manual', 1 \
     FROM manual_experiments m \
     WHERE m.status = 'verified' AND m.outcome_status != 'inconclusive' \
       AND m.executed_at_ns <= {AS_OF} \
     ) QUALIFY ROW_NUMBER() OVER (PARTITION BY name, origin ORDER BY ts DESC) = 1";

/// Compute the scorecard as of `as_of_ns`; `delta` compares against the
/// portfolio as of `as_of_ns - period_ns` when a period is given.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn compute(
    reader: &Reader,
    as_of_ns: i64,
    period_ns: Option<i64>,
) -> Result<Scorecard, String> {
    let mut card = compute_as_of(reader, as_of_ns)?;
    if let Some(period) = period_ns {
        let prev = compute_as_of(reader, as_of_ns - period)?;
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
    let points = points.max(2) as i64;
    let step = period_ns / points;
    let mut out = Vec::with_capacity(points as usize);
    for i in 1..=points {
        let t = as_of_ns - period_ns + step * i;
        out.push((i as f64, compute_as_of(reader, t)?.portfolio));
    }
    Ok(out)
}

fn compute_as_of(reader: &Reader, as_of_ns: i64) -> Result<Scorecard, String> {
    let sql = LATEST_SQL.replace("{AS_OF}", &as_of_ns.to_string());
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
    let rows = reader
        .query_json_rows(
            "SELECT experiment_name AS name FROM manual_experiments \
             WHERE status IN ('draft', 'submitted') ORDER BY experiment_name",
        )
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
}
