//! Tests for `tumult trend`: metric validation, the time window, the target
//! of the runs, and the empty/missing-data paths.

use super::super::*;
use crate::output::testing::capture;
use tempfile::TempDir;
use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, ExperimentStatus,
    ExperimentTarget, Journal, SpanId, TraceId,
};

fn journal(id: &str, title: &str, started_at_ns: i64) -> Journal {
    Journal {
        target_version: None,
        target: None,
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

/// `journal`, of a run on the target `system` of the technology
/// `technology`.
fn on(mut journal: Journal, system: &str, technology: &str) -> Journal {
    journal.target = Some(ExperimentTarget {
        system: system.into(),
        technology: Some(technology.into()),
        environment: None,
        criticality: None,
        namespace: None,
        version: None,
    });
    journal
}

/// What `tumult trend --metric duration_ms --target <target>` prints over
/// the journals in `dir`.
fn duration_trend_on(dir: &TempDir, last: Option<&str>, target: &str) -> String {
    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", last, Some(target)));
    result.unwrap();
    printed.stdout
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
    assert_eq!(
        err.to_string(),
        "unknown metric: nonsense. Valid: resilience_score, duration_ms, estimate_accuracy, \
         method_step_count"
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
fn the_summary_is_over_the_runs_of_the_window() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let mut recent = journal("new-1", "experiment", now_ns - 1_000_000_000);
    recent.duration_ms = 1_000;
    // The old run took 60000 ms.
    let dir = journal_dir(&[journal("old-1", "experiment", OLD_NS), recent]);

    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", Some("30d"), None));

    result.unwrap();
    let out = printed.stdout;
    assert!(
        out.starts_with("Loaded 2 journal(s)\n\nTrend: duration_ms (1 data points)\n"),
        "{out}"
    );
    assert!(
        out.contains("\nSummary: 1 runs, min=1000, max=1000, "),
        "{out}"
    );
}

#[test]
fn a_window_is_counted_in_days() {
    const DAY_NS: i64 = 86_400_000_000_000;
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[journal("t-1", "experiment", now_ns - 10 * DAY_NS)]);

    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", Some("30d"), None));
    result.unwrap();
    assert!(
        printed
            .stdout
            .contains("Trend: duration_ms (1 data points)"),
        "{}",
        printed.stdout
    );

    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", Some("5d"), None));
    result.unwrap();
    assert_eq!(
        printed.stdout,
        "Loaded 1 journal(s)\n\nNo data points for metric: duration_ms\n"
    );
}

#[test]
fn a_trend_by_target_is_over_the_runs_on_it_and_so_is_its_summary() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let mut short = journal("t-2", "second", now_ns - 1_000_000_000);
    short.duration_ms = 1_000;
    let dir = journal_dir(&[
        on(
            journal("t-1", "first", now_ns - 2_000_000_000),
            "orders",
            "mysql",
        ),
        on(short, "sessions", "redis"),
    ]);

    // By the system or by the technology, in any case.
    for wanted in ["orders", "ORDERS", " mysql "] {
        let out = duration_trend_on(&dir, None, wanted);
        assert!(
            out.starts_with("Loaded 2 journal(s)\n\nTrend: duration_ms (1 data points)\n"),
            "{wanted}: {out}"
        );
        assert!(out.contains("t-1 "), "{wanted}: {out}");
        assert!(!out.contains("t-2 "), "{wanted}: {out}");
        assert!(
            out.contains("\nSummary: 1 runs, min=60000, max=60000, "),
            "{wanted}: {out}"
        );
    }

    let out = duration_trend_on(&dir, None, "redis");
    assert!(
        out.contains("\nSummary: 1 runs, min=1000, max=1000, "),
        "{out}"
    );

    assert_eq!(
        duration_trend_on(&dir, None, "no-such-target"),
        "Loaded 2 journal(s)\n\nNo data points for metric: duration_ms\n"
    );
}

#[test]
fn a_target_is_not_looked_for_in_the_title_or_in_a_part_of_a_name() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[on(
        journal("t-1", "postgres failover", now_ns - 1_000_000_000),
        "orders",
        "mysql",
    )]);

    for wanted in ["postgres", "postgres failover", "order", "sql"] {
        assert_eq!(
            duration_trend_on(&dir, None, wanted),
            "Loaded 1 journal(s)\n\nNo data points for metric: duration_ms\n",
            "{wanted}"
        );
    }
}

#[test]
fn a_trend_by_target_says_how_many_journals_name_no_target() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[
        on(
            journal("t-1", "first", now_ns - 2_000_000_000),
            "orders",
            "mysql",
        ),
        journal("t-2", "second", now_ns - 1_000_000_000),
        // Before the window: this one is not left out for its target.
        journal("old-1", "third", OLD_NS),
    ]);

    let out = duration_trend_on(&dir, Some("30d"), "orders");

    assert!(
        out.starts_with(
            "Loaded 3 journal(s)\nLeft out: 1 journal(s) that name no target (a journal names \
             the target its experiment declares, since 3.0.2)\n\n\
             Trend: duration_ms (1 data points)\n"
        ),
        "{out}"
    );
    assert!(out.contains("\nSummary: 1 runs, "), "{out}");
}

#[test]
fn a_trend_of_one_journal_file_is_in_the_same_scope() {
    let dir = journal_dir(&[on(journal("old-1", "first", OLD_NS), "orders", "mysql")]);
    let file = dir.path().join("old-1.toon");
    let trend = |last: Option<&str>, target: Option<&str>| {
        let (result, printed) = capture(|| cmd_trend(&file, "duration_ms", last, target));
        result.unwrap();
        printed.stdout
    };

    assert!(
        trend(None, Some("orders")).contains("Trend: duration_ms (1 data points)"),
        "the run is on the target"
    );
    for (last, target) in [(Some("30d"), None), (None, Some("sessions"))] {
        assert_eq!(
            trend(last, target),
            "Loaded 1 journal(s)\n\nNo data points for metric: duration_ms\n",
            "{last:?} {target:?}"
        );
    }
}

#[test]
fn a_window_longer_than_time_takes_every_journal() {
    let dir = journal_dir(&[journal("old-1", "first", OLD_NS)]);

    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "duration_ms", Some("999999999999999d"), None));

    result.unwrap();
    assert!(
        printed
            .stdout
            .contains("Trend: duration_ms (1 data points)"),
        "{}",
        printed.stdout
    );
}

/// A run that started `second` seconds into a fixed day, with a value of
/// its own for each metric a trend can be of.
fn measured(id: &str, second: i64, duration_ms: u64, steps: usize, score: f64) -> Journal {
    let mut run = journal(
        id,
        "experiment",
        1_700_000_000_000_000_000 + second * 1_000_000_000,
    );
    run.duration_ms = duration_ms;
    run.method_results = vec![run.method_results[0].clone(); steps];
    run.analysis = Some(AnalysisResult {
        estimate_accuracy: Some(score / 2.0),
        estimate_recovery_delta_s: None,
        trend: None,
        resilience_score: Some(score),
    });
    run
}

#[test]
fn a_trend_lists_the_runs_oldest_first_and_sums_them_up() {
    // Not written in the order they ran in, and no value is at the same
    // place in two orders.
    let dir = journal_dir(&[
        measured("run-a", 3, 3_000, 3, 0.5),
        measured("run-b", 1, 1_000, 1, 0.75),
        measured("run-c", 2, 5_000, 2, 1.0),
    ]);

    // The whole of one trend: the columns are 20 wide.
    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", None, None));
    result.unwrap();
    assert_eq!(
        printed.stdout,
        "\
         Loaded 3 journal(s)\n\
         \n\
         Trend: duration_ms (3 data points)\n\
         \n\
         experiment_id       title               status              duration_ms         started_at_ns       \n\
         ----------------------------------------------------------------------------------------------------\n\
         run-b               experiment          completed           1000                1700000001000000000 \n\
         run-c               experiment          completed           5000                1700000002000000000 \n\
         run-a               experiment          completed           3000                1700000003000000000 \n\
         \n\
         Summary: 3 runs, min=1000, max=5000, avg=3000.0000\n"
    );

    // Each metric: its value in each run, oldest first, and their summary.
    for (metric, values, summary) in [
        (
            "duration_ms",
            ["1000", "5000", "3000"],
            "Summary: 3 runs, min=1000, max=5000, avg=3000.0000",
        ),
        (
            "method_step_count",
            ["1", "2", "3"],
            "Summary: 3 runs, min=1, max=3, avg=2.0000",
        ),
        (
            "resilience_score",
            ["0.7500", "1.0000", "0.5000"],
            "Summary: 3 runs, min=0.5000, max=1.0000, avg=0.7500",
        ),
        (
            "estimate_accuracy",
            ["0.3750", "0.5000", "0.2500"],
            "Summary: 3 runs, min=0.2500, max=0.5000, avg=0.3750",
        ),
    ] {
        let (result, printed) = capture(|| cmd_trend(dir.path(), metric, None, None));

        result.unwrap();
        let out = printed.stdout;
        let runs: Vec<Vec<&str>> = out
            .lines()
            .filter(|line| line.starts_with("run-"))
            .map(|line| line.split_whitespace().collect())
            .collect();
        let listed: Vec<(&str, &str)> = runs.iter().map(|cells| (cells[0], cells[3])).collect();
        assert_eq!(
            listed,
            [
                ("run-b", values[0]),
                ("run-c", values[1]),
                ("run-a", values[2])
            ],
            "{metric}: {out}"
        );
        assert_eq!(out.lines().last(), Some(summary), "{metric}: {out}");
    }
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
