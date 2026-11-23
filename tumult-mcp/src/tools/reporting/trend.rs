//! Cross-run metric trend tool over journals.

use std::fmt::Write as _;

use tumult_core::journal::JournalScope;

use crate::error::ToolError;
use crate::tools::StructuredReport;

use super::for_each_journal;

/// Metrics supported by [`trend`], in the same set as `tumult trend`.
const TREND_METRICS: &[&str] = &[
    "resilience_score",
    "duration_ms",
    "estimate_accuracy",
    "method_step_count",
];

/// A single trend data point.
struct TrendPoint {
    ts: i64,
    value: f64,
}

/// The start of the window `last` asks for ("30d" or "30"), in epoch
/// nanoseconds. `None` when it asks for none.
fn window_start_ns(last: Option<&str>) -> Result<Option<i64>, ToolError> {
    let Some(window) = last else {
        return Ok(None);
    };
    let days: i64 = window.trim_end_matches('d').parse().map_err(|_| {
        ToolError::InvalidInput(format!(
            "last must be a number of days (e.g., 30d), got: {window}"
        ))
    })?;
    let now_ns = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    )
    .unwrap_or(i64::MAX);
    Ok(Some(now_ns.saturating_sub(
        days.saturating_mul(86_400 * 1_000_000_000),
    )))
}

/// Cross-run metric trend over journals (mirrors `tumult trend`): ingests
/// the journals of the window and of the target into an in-memory analytics
/// store and returns time-ordered `{ts, value}` points plus a direction
/// verdict. The target is the one a journal names (its system or its
/// technology); a journal that names none is left out of a trend by target
/// and counted.
///
/// # Errors
///
/// Returns a [`ToolError`] if the metric or `last` window is invalid, the
/// journals cannot be read, or the analytics query fails.
pub fn trend(
    journals_path: &str,
    metric: &str,
    last: Option<&str>,
    target: Option<&str>,
) -> Result<StructuredReport, ToolError> {
    if !TREND_METRICS.contains(&metric) {
        return Err(ToolError::InvalidInput(format!(
            "unknown metric '{metric}'; valid values: {}",
            TREND_METRICS.join(", ")
        )));
    }

    let mut scope = JournalScope::new(window_start_ns(last)?, target);

    let store =
        tumult_lake::AnalyticsStore::in_memory().map_err(|e| ToolError::Store(e.to_string()))?;
    let (loaded, skipped) = for_each_journal(journals_path, |journal| {
        if !scope.admit(&journal) {
            return;
        }
        // Ingest failures surface as missing points, not tool failures.
        if let Err(error) = store.ingest_journal(&journal) {
            tracing::warn!(
                name: "mcp.trend.ingest_failed",
                {
                    resilience.experiment.id = journal.experiment_id.as_str(),
                    error.message = %error,
                },
                "journal not ingested; the trend omits it"
            );
        }
    })?;

    // Pre-built queries keyed by metric — no interpolation of user input.
    // They read every run of the store: it holds the journals of the scope
    // only.
    let sql = match metric {
        "resilience_score" => "SELECT started_at_ns, resilience_score FROM experiments WHERE resilience_score IS NOT NULL ORDER BY started_at_ns",
        "duration_ms" => "SELECT started_at_ns, duration_ms FROM experiments WHERE duration_ms IS NOT NULL ORDER BY started_at_ns",
        "estimate_accuracy" => "SELECT started_at_ns, estimate_accuracy FROM experiments WHERE estimate_accuracy IS NOT NULL ORDER BY started_at_ns",
        "method_step_count" => "SELECT started_at_ns, method_step_count FROM experiments WHERE method_step_count IS NOT NULL ORDER BY started_at_ns",
        _ => unreachable!("validated above"),
    };
    let rows = store
        .query(sql)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let points: Vec<TrendPoint> = rows
        .iter()
        .filter_map(|row| {
            let ts = row.first()?.parse::<i64>().ok()?;
            let value = row.get(1)?.parse::<f64>().ok()?;
            Some(TrendPoint { ts, value })
        })
        .collect();

    let verdict = trend_verdict(metric, &points);

    let mut structured = serde_json::Map::new();
    structured.insert("metric".into(), serde_json::json!(metric));
    structured.insert(
        "points".into(),
        serde_json::Value::Array(
            points
                .iter()
                .map(|p| serde_json::json!({ "ts": p.ts, "value": p.value }))
                .collect(),
        ),
    );
    structured.insert("target".into(), serde_json::json!(target));
    structured.insert(
        "journals_without_target".into(),
        serde_json::json!(scope.without_target()),
    );
    structured.insert("verdict".into(), serde_json::json!(verdict));

    let mut text = String::new();
    writeln!(text, "Loaded {loaded} journal(s)").ok();
    if skipped > 0 {
        writeln!(text, "Skipped (unreadable): {skipped}").ok();
    }
    if let Some(note) = scope.left_out_note() {
        writeln!(text, "{note}").ok();
    }
    writeln!(text).ok();
    if points.is_empty() {
        writeln!(text, "No data points for metric: {metric}").ok();
    } else {
        writeln!(text, "Trend: {} ({} data points)", metric, points.len()).ok();
        for point in &points {
            writeln!(text, "  {}  {}", point.ts, point.value).ok();
        }
        let (min, max, sum) = points.iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY, 0.0_f64),
            |(min, max, sum), p| (min.min(p.value), max.max(p.value), sum + p.value),
        );
        #[allow(clippy::cast_precision_loss)]
        let avg = sum / points.len() as f64;
        writeln!(
            text,
            "\nSummary: {} runs, min={min}, max={max}, avg={avg}",
            points.len()
        )
        .ok();
    }
    writeln!(text, "Verdict: {verdict}").ok();

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "narrow with last/target to reduce points"),
        structured,
    })
}

/// Direction verdict comparing the mean of the later half of the series to
/// the earlier half (5% threshold).
///
/// `resilience_score` / `estimate_accuracy` are higher-is-better and
/// `duration_ms` is lower-is-better, mapping to `improving` / `declining` /
/// `stable`; `method_step_count` has no quality direction and maps to
/// `increasing` / `decreasing` / `stable`. Fewer than two points yields
/// `insufficient-data`.
fn trend_verdict(metric: &str, points: &[TrendPoint]) -> &'static str {
    /// Relative change below which the series counts as stable.
    const THRESHOLD: f64 = 0.05;

    if points.len() < 2 {
        return "insufficient-data";
    }
    let mid = points.len() / 2;
    #[allow(clippy::cast_precision_loss)]
    let mean =
        |slice: &[TrendPoint]| slice.iter().map(|p| p.value).sum::<f64>() / slice.len() as f64;
    let early = mean(&points[..mid]);
    let late = mean(&points[mid..]);
    let change = if early.abs() < f64::EPSILON {
        if late.abs() < f64::EPSILON {
            0.0
        } else {
            1.0
        }
    } else {
        (late - early) / early.abs()
    };

    if change.abs() <= THRESHOLD {
        return "stable";
    }
    let rising = change > 0.0;
    match metric {
        // No quality direction — report the raw movement.
        "method_step_count" => {
            if rising {
                "increasing"
            } else {
                "decreasing"
            }
        }
        // duration_ms: lower is better; everything else: higher is better.
        "duration_ms" => {
            if rising {
                "declining"
            } else {
                "improving"
            }
        }
        _ => {
            if rising {
                "improving"
            } else {
                "declining"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::{write_run_journal, write_run_journal_on};
    use tempfile::TempDir;

    const DAY_NS: i64 = 86_400_000_000_000;

    /// Make the run of the journal at `journal_path` one that started
    /// `age_ns` ago.
    fn start_ago(journal_path: &std::path::Path, age_ns: i64) {
        let mut journal = tumult_core::journal::read_journal(journal_path).unwrap();
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
        journal.started_at_ns = now_ns - age_ns;
        tumult_core::journal::write_journal(&journal, journal_path).unwrap();
    }

    /// The number of points of the trend of `duration_ms` over
    /// `journals_path`.
    fn points_of(
        journals_path: &std::path::Path,
        last: Option<&str>,
        target: Option<&str>,
    ) -> usize {
        let result = trend(journals_path.to_str().unwrap(), "duration_ms", last, target).unwrap();
        result.structured["points"].as_array().unwrap().len()
    }

    /// Make the run of the journal at `journal_path` one that started at
    /// `started_at_ns`, took `duration_ms`, ran `steps` steps, and was
    /// analysed to `score` and `accuracy`.
    fn measured(
        journal_path: &std::path::Path,
        started_at_ns: i64,
        duration_ms: u64,
        steps: usize,
        score: f64,
        accuracy: f64,
    ) {
        let mut journal = tumult_core::journal::read_journal(journal_path).unwrap();
        journal.started_at_ns = started_at_ns;
        journal.duration_ms = duration_ms;
        journal.method_results = vec![journal.method_results[0].clone(); steps];
        journal.analysis = Some(tumult_core::types::AnalysisResult {
            estimate_accuracy: Some(accuracy),
            estimate_recovery_delta_s: None,
            estimate_data_loss_correct: None,
            trend: None,
            resilience_score: Some(score),
        });
        tumult_core::journal::write_journal(&journal, journal_path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_points_of_a_trend_are_the_values_of_its_metric_oldest_first() {
        let journals = TempDir::new().unwrap();
        let later = write_run_journal_on(journals.path(), "later", "orders", "mysql");
        let earlier = write_run_journal_on(journals.path(), "earlier", "sessions", "redis");
        measured(&later, 2_000, 700, 1, 0.9, 0.4);
        measured(&earlier, 1_000, 300, 3, 0.5, 0.8);

        for (metric, first, second, verdict) in [
            ("resilience_score", 0.5, 0.9, "improving"),
            ("duration_ms", 300.0, 700.0, "declining"),
            ("estimate_accuracy", 0.8, 0.4, "declining"),
            ("method_step_count", 3.0, 1.0, "decreasing"),
        ] {
            let result = trend(journals.path().to_str().unwrap(), metric, None, None).unwrap();

            assert_eq!(
                result.structured["points"],
                serde_json::json!([
                    { "ts": 1_000, "value": first },
                    { "ts": 2_000, "value": second },
                ]),
                "{metric}"
            );
            assert_eq!(result.structured["verdict"], verdict, "{metric}");
        }
    }

    #[test]
    fn trend_rejects_unknown_metric_listing_values() {
        let err = trend(".", "latency", None, None).expect_err("unknown metric");
        let msg = err.to_string();
        assert!(msg.contains("latency"), "must name the bad value: {msg}");
        for metric in TREND_METRICS {
            assert!(msg.contains(metric), "must list '{metric}': {msg}");
        }
    }

    #[test]
    fn trend_rejects_malformed_last_window() {
        let err = trend(".", "duration_ms", Some("soon"), None).expect_err("bad window");
        assert!(err.to_string().contains("number of days"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trend_duration_over_journal_returns_points() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = trend(journal_path.to_str().unwrap(), "duration_ms", None, None).unwrap();
        assert_eq!(result.structured["metric"], "duration_ms");
        let points = result.structured["points"].as_array().unwrap();
        assert_eq!(points.len(), 1);
        assert!(points[0]["ts"].as_i64().is_some());
        assert!(points[0]["value"].as_f64().is_some());
        assert_eq!(result.structured["verdict"], "insufficient-data");
        assert_eq!(result.structured["target"], serde_json::Value::Null);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trend_by_target_matches_the_system_or_the_technology_and_not_the_title() {
        let journals = TempDir::new().unwrap();
        // The title names another technology than the one the run was on.
        write_run_journal_on(journals.path(), "postgres failover", "orders", "mysql");

        for (wanted, points) in [
            ("orders", 1),
            ("Orders", 1),
            ("MySQL", 1),
            ("postgres", 0),
            ("postgres failover", 0),
            ("order", 0),
        ] {
            let result = trend(
                journals.path().to_str().unwrap(),
                "duration_ms",
                None,
                Some(wanted),
            )
            .unwrap();
            assert_eq!(
                result.structured["points"].as_array().unwrap().len(),
                points,
                "{wanted}"
            );
            assert_eq!(result.structured["target"], wanted);
            assert_eq!(result.structured["journals_without_target"], 0);
            assert!(!result.text.contains("Left out"), "{}", result.text);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_trend_by_target_is_over_the_runs_on_it_and_so_is_its_summary() {
        let journals = TempDir::new().unwrap();
        write_run_journal_on(journals.path(), "first", "orders", "mysql");
        write_run_journal_on(journals.path(), "second", "sessions", "redis");

        let all = trend(
            journals.path().to_str().unwrap(),
            "method_step_count",
            None,
            None,
        )
        .unwrap();
        assert!(all.text.contains("\nSummary: 2 runs, "), "{}", all.text);

        let one = trend(
            journals.path().to_str().unwrap(),
            "method_step_count",
            None,
            Some("redis"),
        )
        .unwrap();
        assert!(
            one.text
                .starts_with("Loaded 2 journal(s)\n\nTrend: method_step_count (1 data points)\n"),
            "{}",
            one.text
        );
        assert!(one.text.contains("\nSummary: 1 runs, "), "{}", one.text);
        assert_eq!(one.structured["verdict"], "insufficient-data");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_trend_by_target_leaves_out_the_journals_that_name_no_target_and_says_so() {
        let dir = TempDir::new().unwrap();
        let journals = dir.path().join("journals");
        let run_dir = dir.path().join("run");
        std::fs::create_dir_all(&journals).unwrap();
        std::fs::create_dir_all(&run_dir).unwrap();
        write_run_journal_on(&journals, "first", "orders", "mysql");
        // The run of an experiment that declares no target.
        std::fs::rename(write_run_journal(&run_dir), journals.join("plain.toon")).unwrap();

        let result = trend(
            journals.to_str().unwrap(),
            "duration_ms",
            None,
            Some("orders"),
        )
        .unwrap();

        assert_eq!(result.structured["points"].as_array().unwrap().len(), 1);
        assert_eq!(result.structured["journals_without_target"], 1);
        assert!(
            result.text.starts_with(
                "Loaded 2 journal(s)\nLeft out: 1 journal(s) that name no target (a journal \
                 names the target its experiment declares, since 3.0.2)\n\n\
                 Trend: duration_ms (1 data points)\n"
            ),
            "{}",
            result.text
        );

        // Without a target nothing is left out.
        let result = trend(journals.to_str().unwrap(), "duration_ms", None, None).unwrap();
        assert_eq!(result.structured["points"].as_array().unwrap().len(), 2);
        assert_eq!(result.structured["journals_without_target"], 0);
        assert!(!result.text.contains("Left out"), "{}", result.text);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_window_is_counted_in_days_from_now() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());
        start_ago(&journal_path, 10 * DAY_NS);

        assert_eq!(points_of(&journal_path, Some("30d"), None), 1);
        assert_eq!(points_of(&journal_path, Some("11"), None), 1);
        assert_eq!(points_of(&journal_path, Some("9d"), None), 0);
        assert_eq!(points_of(&journal_path, None, None), 1);
        // A window longer than time itself takes every journal.
        assert_eq!(points_of(&journal_path, Some("999999999999999d"), None), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_journal_before_the_window_is_not_left_out_for_its_target() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());
        start_ago(&journal_path, 10 * DAY_NS);

        let result = trend(
            journal_path.to_str().unwrap(),
            "duration_ms",
            Some("5d"),
            Some("orders"),
        )
        .unwrap();

        assert!(result.structured["points"].as_array().unwrap().is_empty());
        assert_eq!(result.structured["journals_without_target"], 0);
    }

    fn pts(values: &[f64]) -> Vec<TrendPoint> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| TrendPoint {
                ts: i64::try_from(i).unwrap(),
                value: *v,
            })
            .collect()
    }

    #[test]
    fn trend_verdict_directions() {
        assert_eq!(
            trend_verdict("resilience_score", &pts(&[])),
            "insufficient-data"
        );
        assert_eq!(
            trend_verdict("resilience_score", &pts(&[0.5, 0.9])),
            "improving"
        );
        assert_eq!(
            trend_verdict("resilience_score", &pts(&[0.9, 0.5])),
            "declining"
        );
        assert_eq!(
            trend_verdict("resilience_score", &pts(&[0.8, 0.8])),
            "stable"
        );
        // Lower duration is better.
        assert_eq!(
            trend_verdict("duration_ms", &pts(&[100.0, 50.0])),
            "improving"
        );
        assert_eq!(
            trend_verdict("duration_ms", &pts(&[50.0, 100.0])),
            "declining"
        );
        // Step count is directionless.
        assert_eq!(
            trend_verdict("method_step_count", &pts(&[2.0, 4.0])),
            "increasing"
        );
        assert_eq!(
            trend_verdict("method_step_count", &pts(&[4.0, 2.0])),
            "decreasing"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trend_last_window_includes_recent_journals() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        // A 30-day window parses and keeps a journal written just now.
        let result = trend(
            journal_path.to_str().unwrap(),
            "duration_ms",
            Some("30d"),
            None,
        )
        .unwrap();
        assert_eq!(result.structured["points"].as_array().unwrap().len(), 1);

        // A zero-day window (with or without the 'd' suffix) excludes it:
        // the cutoff is the current instant.
        let result = trend(
            journal_path.to_str().unwrap(),
            "duration_ms",
            Some("0"),
            None,
        )
        .unwrap();
        assert!(result.structured["points"].as_array().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trend_over_a_directory_skips_unreadable_files() {
        let dir = TempDir::new().unwrap();
        // Run in a subdirectory, then move the journal next to the broken
        // file so the directory under test holds exactly one valid journal.
        let run_dir = dir.path().join("run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let journal_path = write_run_journal(&run_dir);
        std::fs::rename(&journal_path, dir.path().join("journal.toon")).unwrap();
        std::fs::write(dir.path().join("broken.toon"), "title: [unterminated").unwrap();

        let result = trend(dir.path().to_str().unwrap(), "duration_ms", None, None).unwrap();
        assert!(
            result.text.contains("Loaded 1 journal(s)"),
            "{}",
            result.text
        );
        assert!(
            result.text.contains("Skipped (unreadable): 1"),
            "{}",
            result.text
        );
        assert_eq!(result.structured["points"].as_array().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trend_method_step_count_summarizes_multiple_runs() {
        let dir = TempDir::new().unwrap();
        let experiment = crate::tools::test_support::write_valid_experiment(dir.path());
        // Two real runs of the same one-step experiment (distinct run ids).
        for name in ["journal-1.toon", "journal-2.toon"] {
            crate::tools::run_experiment(crate::tools::RunExperimentRequest {
                experiment_path: &experiment,
                rollback_strategy: "always",
                journal_path: &dir.path().join(name),
                force: false,
                store_path: "unused.duckdb",
                no_ingest: true,
                format: "toon",
                parent_context: None,
            })
            .unwrap();
        }

        let result = trend(
            dir.path().to_str().unwrap(),
            "method_step_count",
            None,
            None,
        )
        .unwrap();
        let points = result.structured["points"].as_array().unwrap();
        assert_eq!(points.len(), 2, "both runs are data points: {points:?}");
        for point in points {
            assert_eq!(point["value"], 1.0, "one method step per run");
        }
        // A flat series is stable, and the summary line reports min/max/avg.
        assert_eq!(result.structured["verdict"], "stable");
        assert!(
            result.text.contains("Summary: 2 runs, min=1, max=1, avg=1"),
            "{}",
            result.text
        );
    }
}
