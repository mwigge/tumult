//! Gremlin-style resilience scoring with freshness decay.
//!
//! Per experiment/target: a passed run scores 100, a passed run older than
//! 30 days decays to 75 (stale), a failed/deviated run scores 50, and never
//! run scores 0. Bands: > 70 good, 50–70 fair, < 50 poor. The portfolio
//! rollup is the mean of target scores with a period-over-period delta.

use kronika_store::Reader;

/// Staleness threshold: a pass older than this decays from 100 to 75.
pub const STALE_NS: i64 = 30 * 86_400 * 1_000_000_000;

/// Run states driving the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Passed,
    Stale,
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
    pub runs: u64,
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
const LATEST_SQL: &str = "SELECT s.experiment_name AS name, s.target_system AS target, \
     s.ts_ns AS ts, l.log_attrs['status'] AS status, \
     (SELECT COUNT(*) FROM spans r WHERE r.span_name = 'resilience.experiment' \
      AND r.experiment_name = s.experiment_name AND r.ts_ns <= {AS_OF}) AS runs \
     FROM spans s LEFT JOIN logs l \
       ON l.log_attrs['experiment_id'] = s.experiment_id \
      AND l.body = 'experiment.completed' \
     WHERE s.span_name = 'resilience.experiment' AND s.experiment_name IS NOT NULL \
       AND s.ts_ns <= {AS_OF} \
     QUALIFY ROW_NUMBER() OVER (PARTITION BY s.experiment_name ORDER BY s.ts_ns DESC) = 1";

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
        let runs = row.get("runs").and_then(serde_json::Value::as_u64).unwrap_or(0);
        // tumult outcomes: Completed passes; Deviated/Failed fail; a missing
        // outcome (incomplete run) counts as a failed attempt — conservative.
        let passed = outcome.as_deref() == Some("Completed");
        let (score, state) = score_run(ts.map(|t| (t, passed)), as_of_ns);
        experiments.push(ExperimentScore {
            name: name.to_string(),
            target,
            score,
            state,
            band: band(f64::from(score)).to_string(),
            last_run_ns: ts,
            last_outcome: outcome,
            runs,
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
            let score =
                exps.iter().map(|e| f64::from(e.score)).sum::<f64>() / exps.len() as f64;
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
