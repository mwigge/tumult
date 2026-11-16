//! Tests for `tumult trend`: metric validation, time-window parsing, target
//! filtering, and the empty/missing-data paths.

use super::super::*;
use crate::output::testing::capture;
use tempfile::TempDir;
use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, ExperimentStatus, Journal,
    SpanId, TraceId,
};

fn journal(id: &str, title: &str, started_at_ns: i64) -> Journal {
    Journal {
        target_version: None,
        experiment_title: title.into(),
        experiment_id: id.into(),
        status: ExperimentStatus::Completed,
        started_at_ns,
        ended_at_ns: started_at_ns + 60_000_000_000,
        duration_ms: 60_000,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![ActivityResult {
            name: "step".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns,
            duration_ms: 500,
            output: None,
            error: None,
            trace_id: TraceId::empty(),
            span_id: SpanId::empty(),
        }],
        rollback_results: vec![],
        rollback_failures: 0,
        kind: tumult_core::types::TestKind::default(),
        objectives: None,
        probe_samples: vec![],
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: Some(AnalysisResult {
            estimate_accuracy: Some(0.9),
            estimate_recovery_delta_s: None,
            trend: None,
            resilience_score: Some(0.95),
        }),
        regulatory: None,
        halt: None,
        blast_radius: None,
    }
}

/// Write each journal as `<id>.toon` into a fresh tempdir.
fn journal_dir(journals: &[Journal]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for j in journals {
        tumult_core::journal::write_journal(
            j,
            &dir.path().join(format!("{}.toon", j.experiment_id)),
        )
        .unwrap();
    }
    dir
}

const OLD_NS: i64 = 1_577_836_800_000_000_000; // 2020-01-07

#[test]
fn trend_reports_metric_over_journal_dir() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[
        journal("t-1", "latency experiment", now_ns - 2_000_000_000),
        journal("t-2", "latency experiment rerun", now_ns - 1_000_000_000),
    ]);

    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", None, None));

    result.unwrap();
    let out = printed.stdout;
    assert!(out.starts_with("Loaded 2 journal(s)\n"), "{out}");
    assert!(out.contains("Trend: duration_ms (2 data points)"), "{out}");
    // Rows come oldest first.
    let first = out.find("latency experiment ").expect("first run listed");
    let second = out.find("latency experiment rerun").expect("rerun listed");
    assert!(first < second, "{out}");
    assert!(
        out.contains("Summary: 2 runs, min=60000, max=60000"),
        "{out}"
    );
}

#[test]
fn trend_supports_every_documented_metric() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[journal("t-1", "experiment", now_ns)]);

    // The value each metric has in the one journal.
    for (metric, value) in [
        ("resilience_score", "0.9500"),
        ("duration_ms", "60000"),
        ("estimate_accuracy", "0.9000"),
        ("method_step_count", "1"),
    ] {
        let (result, printed) = capture(|| cmd_trend(dir.path(), metric, None, None));

        result.unwrap();
        let out = printed.stdout;
        assert!(
            out.contains(&format!("Trend: {metric} (1 data points)")),
            "{out}"
        );
        assert!(
            out.contains(&format!("Summary: 1 runs, min={value}, max={value}")),
            "{metric}: {out}"
        );
    }
}

#[test]
fn trend_rejects_unknown_metric() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[journal("t-1", "experiment", now_ns)]);

    let err = cmd_trend(dir.path(), "nonsense", None, None).unwrap_err();
    assert!(
        err.to_string().contains("unknown metric: nonsense"),
        "{err}"
    );
}

#[test]
fn trend_rejects_malformed_last_window() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[journal("t-1", "experiment", now_ns)]);

    let err = cmd_trend(dir.path(), "duration_ms", Some("soon"), None).unwrap_err();
    assert!(
        err.to_string().contains("--last must be a number of days"),
        "{err}"
    );
}

#[test]
fn trend_last_window_excludes_old_journals() {
    let dir = journal_dir(&[journal("old-1", "ancient experiment", OLD_NS)]);

    // A 30d window filters out the 2020 journal: valid run, no data points.
    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", Some("30d"), None));

    result.unwrap();
    assert_eq!(
        printed.stdout,
        "Loaded 1 journal(s)\n\nNo data points for metric: duration_ms\n"
    );

    // Without the window the same journal is a data point.
    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", None, None));
    result.unwrap();
    assert!(
        printed
            .stdout
            .contains("Trend: duration_ms (1 data points)"),
        "{}",
        printed.stdout
    );
}

#[test]
fn trend_target_filter_selects_matching_titles() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[
        journal("t-1", "postgres failover", now_ns - 2_000_000_000),
        journal("t-2", "redis eviction", now_ns - 1_000_000_000),
    ]);

    // The filter is case-insensitive and matches a part of the title.
    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "duration_ms", None, Some("Postgres")));
    result.unwrap();
    let out = printed.stdout;
    assert!(out.contains("Trend: duration_ms (1 data points)"), "{out}");
    assert!(out.contains("postgres failover"), "{out}");
    assert!(!out.contains("redis eviction"), "{out}");

    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "duration_ms", None, Some("no-such-target")));
    result.unwrap();
    assert!(
        printed
            .stdout
            .ends_with("No data points for metric: duration_ms\n"),
        "{}",
        printed.stdout
    );
}

#[test]
fn trend_accepts_a_single_journal_file() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("single.toon");
    tumult_core::journal::write_journal(&journal("t-1", "experiment", now_ns), &path).unwrap();

    let (result, printed) = capture(|| cmd_trend(&path, "duration_ms", None, None));

    result.unwrap();
    let out = printed.stdout;
    assert!(out.starts_with("Loaded 1 journal(s)\n"), "{out}");
    assert!(out.contains("Trend: duration_ms (1 data points)"), "{out}");
}

#[test]
fn trend_missing_path_errors() {
    let dir = TempDir::new().unwrap();
    let err = cmd_trend(&dir.path().join("nope"), "duration_ms", None, None).unwrap_err();
    assert!(err.to_string().contains("path does not exist"), "{err}");
}

#[test]
fn trend_skips_malformed_journals_with_warning() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[journal("t-1", "experiment", now_ns)]);
    std::fs::write(dir.path().join("broken.toon"), "not a journal {{{").unwrap();

    // The good journal still loads; the broken one is skipped, not fatal.
    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", None, None));

    result.unwrap();
    let out = printed.stdout;
    assert!(out.starts_with("Loaded 1 journal(s)\n"), "{out}");
    assert!(out.contains("Trend: duration_ms (1 data points)"), "{out}");
}

#[test]
fn trend_without_data_points_reports_absence() {
    // Journals without an analysis block have no resilience_score values.
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let mut j = journal("t-1", "experiment", now_ns);
    j.analysis = None;
    let dir = journal_dir(&[j]);

    let (result, printed) = capture(|| cmd_trend(dir.path(), "resilience_score", None, None));

    result.unwrap();
    assert_eq!(
        printed.stdout,
        "Loaded 1 journal(s)\n\nNo data points for metric: resilience_score\n"
    );
}

#[test]
fn trend_reports_a_journal_it_could_not_read_as_a_log_event() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[journal("t-1", "experiment", now_ns)]);
    let corrupt = dir.path().join("corrupt.toon");
    std::fs::write(&corrupt, "{{{{ not a journal").unwrap();

    let capture = tumult_otel::logging::testing::LogCapture::start(tracing::Level::WARN);
    cmd_trend(dir.path(), "duration_ms", None, None).unwrap();
    let logs = capture.snapshot();

    let skipped = logs.named("cli.journal.skipped");
    assert_eq!(skipped.len(), 1, "{}", logs.raw);
    assert_eq!(skipped[0]["level"], "WARN");
    assert_eq!(skipped[0]["file.path"], corrupt.to_str().unwrap());
    assert!(skipped[0]["error.message"].is_string(), "{}", logs.raw);
}
