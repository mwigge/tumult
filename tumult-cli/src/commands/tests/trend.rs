//! Tests for `tumult trend`: metric validation, the time window, the target
//! of the runs, and the empty/missing-data paths.

use super::super::*;
use crate::output::testing::capture;
use tempfile::TempDir;
use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, ExperimentStatus,
    ExperimentTarget, Journal, NfrCriticality, NfrKind, NfrOperator, NfrState, NfrVerdict,
    ObjectiveOutcome, PostResult, SpanId, TraceId,
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
            estimate_data_loss_correct: None,
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
    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "duration_ms", last, Some(target), false));
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

    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", None, None, false));

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
        let (result, printed) = capture(|| cmd_trend(dir.path(), metric, None, None, false));

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

    let err = cmd_trend(dir.path(), "nonsense", None, None, false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown metric: nonsense. Valid: resilience_score, duration_ms, estimate_accuracy, \
         method_step_count, rto_measured_s, rto_margin_s, rpo_measured_s, rpo_margin_s"
    );
}

#[test]
fn trend_rejects_malformed_last_window() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[journal("t-1", "experiment", now_ns)]);

    let err = cmd_trend(dir.path(), "duration_ms", Some("soon"), None, false).unwrap_err();
    assert!(
        err.to_string().contains("--last must be a number of days"),
        "{err}"
    );
}

#[test]
fn trend_last_window_excludes_old_journals() {
    let dir = journal_dir(&[journal("old-1", "ancient experiment", OLD_NS)]);

    // A 30d window filters out the 2020 journal: valid run, no data points.
    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "duration_ms", Some("30d"), None, false));

    result.unwrap();
    assert_eq!(
        printed.stdout,
        "Loaded 1 journal(s)\n\nNo data points for metric: duration_ms\n"
    );

    // Without the window the same journal is a data point.
    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", None, None, false));
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

    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "duration_ms", Some("30d"), None, false));

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

    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "duration_ms", Some("30d"), None, false));
    result.unwrap();
    assert!(
        printed
            .stdout
            .contains("Trend: duration_ms (1 data points)"),
        "{}",
        printed.stdout
    );

    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "duration_ms", Some("5d"), None, false));
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
        let (result, printed) = capture(|| cmd_trend(&file, "duration_ms", last, target, false));
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

    let (result, printed) = capture(|| {
        cmd_trend(
            dir.path(),
            "duration_ms",
            Some("999999999999999d"),
            None,
            false,
        )
    });

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
        estimate_data_loss_correct: None,
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
    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", None, None, false));
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
        let (result, printed) = capture(|| cmd_trend(dir.path(), metric, None, None, false));

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

    let (result, printed) = capture(|| cmd_trend(&path, "duration_ms", None, None, false));

    result.unwrap();
    let out = printed.stdout;
    assert!(out.starts_with("Loaded 1 journal(s)\n"), "{out}");
    assert!(out.contains("Trend: duration_ms (1 data points)"), "{out}");
}

#[test]
fn trend_missing_path_errors() {
    let dir = TempDir::new().unwrap();
    let err = cmd_trend(&dir.path().join("nope"), "duration_ms", None, None, false).unwrap_err();
    assert!(err.to_string().contains("path does not exist"), "{err}");
}

#[test]
fn trend_skips_malformed_journals_with_warning() {
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let dir = journal_dir(&[journal("t-1", "experiment", now_ns)]);
    std::fs::write(dir.path().join("broken.toon"), "not a journal {{{").unwrap();

    // The good journal still loads; the broken one is skipped, not fatal.
    let (result, printed) = capture(|| cmd_trend(dir.path(), "duration_ms", None, None, false));

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

    let (result, printed) =
        capture(|| cmd_trend(dir.path(), "resilience_score", None, None, false));

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
    cmd_trend(dir.path(), "duration_ms", None, None, false).unwrap();
    let logs = capture.snapshot();

    let skipped = logs.named("cli.journal.skipped");
    assert_eq!(skipped.len(), 1, "{}", logs.raw);
    assert_eq!(skipped[0]["level"], "WARN");
    assert_eq!(skipped[0]["file.path"], corrupt.to_str().unwrap());
    assert!(skipped[0]["error.message"].is_string(), "{}", logs.raw);
}

// ── Recovery trends ──────────────────────────────────────────

/// The start of the fixed day of the DR runs.
const DAY_NS: i64 = 1_700_000_000_000_000_000;

/// A DR run of a failover on `system` that started `second` seconds into
/// the day, with `objectives`. Its probes saw an outage when `outage`, and
/// the target back `mttr_s` after it when that is set.
fn dr_run(
    id: &str,
    second: i64,
    system: &str,
    objectives: ObjectiveOutcome,
    outage: bool,
    mttr_s: Option<f64>,
) -> Journal {
    let started_at_ns = DAY_NS + second * 1_000_000_000;
    let mut run = on(journal(id, "failover", started_at_ns), system, "postgres");
    run.objectives = Some(objectives);
    run.post_result = Some(PostResult {
        started_at_ns,
        ended_at_ns: started_at_ns + 30_000_000_000,
        duration_s: 30.0,
        samples: 30,
        probes: vec![],
        recovery_time_s: 29.1,
        full_recovery: mttr_s.is_some() || !outage,
        mttr_s,
        impact_observed: Some(outage),
        outage_started_at_ns: outage.then_some(started_at_ns),
        recovered_at_ns: None,
    });
    run
}

/// A verdict of `kind` named `id` on a target of `threshold`.
fn verdict(id: &str, kind: NfrKind, threshold: f64, observed: Option<f64>) -> NfrVerdict {
    NfrVerdict {
        nfr_id: id.into(),
        kind,
        metric: "recovery.mttr_s".into(),
        operator: NfrOperator::Lte,
        threshold,
        observed,
        state: NfrState::Inconclusive,
        margin: None,
        criticality: NfrCriticality::Blocking,
        recovered: None,
        reason: None,
    }
}

/// A run with an RTO of 10 s and the verdict `rto` on it.
fn with_rto(rto: NfrVerdict, measured_s: Option<f64>) -> ObjectiveOutcome {
    ObjectiveOutcome {
        rto_target_s: Some(10.0),
        rto_measured_s: measured_s,
        verdicts: vec![rto],
        ..ObjectiveOutcome::default()
    }
}

/// A run with an RTO of 10 s that recovered in `mttr_s`.
fn recovered(id: &str, second: i64, system: &str, mttr_s: f64) -> Journal {
    let rto = NfrVerdict {
        state: NfrState::Met,
        margin: Some(10.0 - mttr_s),
        recovered: Some(true),
        ..verdict("rto", NfrKind::Rto, 10.0, Some(mttr_s))
    };
    let objectives = with_rto(rto, Some(mttr_s));
    dr_run(id, second, system, objectives, true, Some(mttr_s))
}

/// A run with an RTO of 10 s that was not back in the 30.1 s it watched.
fn never_back(id: &str, second: i64, system: &str) -> Journal {
    let rto = NfrVerdict {
        state: NfrState::Breached,
        recovered: Some(false),
        ..verdict("rto", NfrKind::Rto, 10.0, Some(30.1))
    };
    let mut run = dr_run(id, second, system, with_rto(rto, None), true, None);
    run.status = ExperimentStatus::Deviated;
    run
}

/// A run with an RTO of 10 s whose probes saw no outage.
fn no_outage(id: &str, second: i64, system: &str) -> Journal {
    let rto = NfrVerdict {
        reason: Some(NfrVerdict::NO_OUTAGE_OBSERVED.into()),
        ..verdict("rto", NfrKind::Rto, 10.0, None)
    };
    dr_run(id, second, system, with_rto(rto, None), false, None)
}

/// A run with an RPO of 5 s that lost `lost_s`, or read none back.
fn with_rpo(id: &str, second: i64, system: &str, lost_s: Option<f64>) -> Journal {
    let objectives = ObjectiveOutcome {
        rpo_target_s: Some(5.0),
        rpo_measured_s: lost_s,
        verdicts: vec![verdict("data-kept", NfrKind::Rpo, 5.0, lost_s)],
        ..ObjectiveOutcome::default()
    };
    dr_run(id, second, system, objectives, true, Some(1.0))
}

/// DR runs on two targets, not in the order they ran in.
fn dr_runs() -> TempDir {
    journal_dir(&[
        never_back("never-back", 3, "orders"),
        recovered("fast", 1, "orders", 4.0),
        no_outage("no-outage", 2, "orders"),
        recovered("slow", 5, "orders", 7.5),
        recovered("other", 4, "sessions", 2.0),
        with_rpo("lost-little", 6, "orders", Some(2.0)),
        with_rpo("lost-more", 7, "orders", Some(4.5)),
        with_rpo("none-read", 8, "orders", None),
        with_rpo("elsewhere", 9, "sessions", Some(1.0)),
    ])
}

/// What `tumult trend --metric <metric> --target <target>` prints over
/// the DR runs.
fn recovery_trend_on(metric: &str, target: &str) -> String {
    let dir = dr_runs();
    let (result, printed) = capture(|| cmd_trend(dir.path(), metric, None, Some(target), false));
    result.unwrap();
    printed.stdout
}

/// The header and the rule of the table of a recovery trend of `metric`.
fn recovery_table(metric: &str) -> String {
    format!(
        "experiment_id       title               status              {metric:<20}started_at_ns       note                \n{}\n",
        "-".repeat(120)
    )
}

#[test]
fn an_rto_trend_on_a_target_marks_the_run_that_did_not_recover_and_counts_the_one_that_saw_no_outage(
) {
    let out = recovery_trend_on("rto_measured_s", "orders");

    assert_eq!(
        out,
        format!(
            "Loaded 9 journal(s)\n\nTrend: rto_measured_s (3 data points)\n\n{}\
             fast                failover            completed           4.0000              1700000001000000000                     \n\
             never-back          failover            deviated            at least 30.1000    1700000003000000000 not recovered       \n\
             slow                failover            completed           7.5000              1700000005000000000                     \n\
             \n\
             Summary: 2 runs, min=4.0000, max=7.5000, avg=5.7500\n\
             Not recovered: 1 run(s), each shown as a bound and left out of the summary\n\
             1 run(s) saw no outage\n",
            recovery_table("rto_measured_s")
        )
    );
}

#[test]
fn an_rto_margin_is_the_target_minus_the_rto_and_at_most_that_for_a_run_that_did_not_recover() {
    let out = recovery_trend_on("rto_margin_s", "orders");

    assert_eq!(
        out,
        format!(
            "Loaded 9 journal(s)\n\nTrend: rto_margin_s (3 data points)\n\n{}\
             fast                failover            completed           6.0000              1700000001000000000                     \n\
             never-back          failover            deviated            at most -20.1000    1700000003000000000 not recovered       \n\
             slow                failover            completed           2.5000              1700000005000000000                     \n\
             \n\
             Summary: 2 runs, min=2.5000, max=6.0000, avg=4.2500\n\
             Not recovered: 1 run(s), each shown as a bound and left out of the summary\n\
             1 run(s) saw no outage\n",
            recovery_table("rto_margin_s")
        )
    );
}

#[test]
fn an_rpo_trend_on_a_target_counts_the_run_that_measured_none() {
    for (metric, little, more, summary) in [
        (
            "rpo_measured_s",
            "2.0000",
            "4.5000",
            "Summary: 2 runs, min=2.0000, max=4.5000, avg=3.2500",
        ),
        (
            "rpo_margin_s",
            "3.0000",
            "0.5000",
            "Summary: 2 runs, min=0.5000, max=3.0000, avg=1.7500",
        ),
    ] {
        let out = recovery_trend_on(metric, "orders");

        assert_eq!(
            out,
            format!(
                "Loaded 9 journal(s)\n\nTrend: {metric} (2 data points)\n\n{}\
                 lost-little         failover            completed           {little:<20}1700000006000000000                     \n\
                 lost-more           failover            completed           {more:<20}1700000007000000000                     \n\
                 \n\
                 {summary}\n\
                 1 run(s) measured no RPO\n",
                recovery_table(metric)
            ),
            "{metric}"
        );
    }
}

#[test]
fn a_recovery_trend_is_of_the_runs_on_the_target_only() {
    let out = recovery_trend_on("rto_measured_s", "sessions");

    assert!(
        out.starts_with("Loaded 9 journal(s)\n\nTrend: rto_measured_s (1 data points)\n"),
        "{out}"
    );
    assert!(out.contains("\nother  "), "{out}");
    assert!(
        out.ends_with("\nSummary: 1 runs, min=2.0000, max=2.0000, avg=2.0000\n"),
        "{out}"
    );
}

#[test]
fn a_recovery_trend_without_points_still_counts_the_runs_it_left_out() {
    let dir = journal_dir(&[no_outage("no-outage", 2, "orders")]);

    let (result, printed) = capture(|| cmd_trend(dir.path(), "rto_measured_s", None, None, false));

    result.unwrap();
    assert_eq!(
        printed.stdout,
        "Loaded 1 journal(s)\n\nNo data points for metric: rto_measured_s\n1 run(s) saw no outage\n"
    );
}

#[test]
fn a_recovery_trend_of_runs_that_all_did_not_recover_has_no_summary() {
    let dir = journal_dir(&[never_back("never-back", 3, "orders")]);

    let (result, printed) = capture(|| cmd_trend(dir.path(), "rto_measured_s", None, None, false));

    result.unwrap();
    let out = printed.stdout;
    assert!(!out.contains("Summary:"), "{out}");
    assert!(
        out.ends_with(
            "\n\nNot recovered: 1 run(s), each shown as a bound and left out of the summary\n"
        ),
        "{out}"
    );
}

/// What `tumult trend --format json` prints, as JSON.
fn json_trend(dir: &std::path::Path, metric: &str, target: Option<&str>) -> serde_json::Value {
    let (result, printed) = capture(|| cmd_trend(dir, metric, None, target, true));
    result.unwrap();
    serde_json::from_str(&printed.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", printed.stdout))
}

#[test]
fn a_recovery_trend_as_json_has_the_points_with_their_flags_and_the_counts() {
    let dir = dr_runs();

    let trend = json_trend(dir.path(), "rto_measured_s", Some("orders"));

    let point = |id: &str, second: i64, status: &str, value: f64| {
        serde_json::json!({
            "experiment_id": id,
            "title": "failover",
            "status": status,
            "started_at_ns": DAY_NS + second * 1_000_000_000,
            "value": value,
            "recovered": true,
        })
    };
    let mut never_back = point("never-back", 3, "deviated", 30.1);
    never_back["recovered"] = false.into();
    never_back["bound"] = "at least".into();
    assert_eq!(
        trend,
        serde_json::json!({
            "metric": "rto_measured_s",
            "target": "orders",
            "journals": 9,
            "journals_without_target": 0,
            "points": [
                point("fast", 1, "completed", 4.0),
                never_back,
                point("slow", 5, "completed", 7.5),
            ],
            "summary": { "runs": 2, "min": 4.0, "max": 7.5, "avg": 5.75 },
            "runs_without_outage": 1,
            "runs_not_measured": 0,
        })
    );
}

#[test]
fn an_rpo_trend_as_json_has_no_recovery_flags_and_counts_the_run_that_measured_none() {
    let dir = dr_runs();

    let trend = json_trend(dir.path(), "rpo_margin_s", Some("orders"));

    assert_eq!(
        trend["points"],
        serde_json::json!([
            {
                "experiment_id": "lost-little",
                "title": "failover",
                "status": "completed",
                "started_at_ns": DAY_NS + 6_000_000_000,
                "value": 3.0,
            },
            {
                "experiment_id": "lost-more",
                "title": "failover",
                "status": "completed",
                "started_at_ns": DAY_NS + 7_000_000_000,
                "value": 0.5,
            },
        ])
    );
    assert_eq!(trend["runs_not_measured"], 1, "{trend}");
    assert_eq!(trend["runs_without_outage"], 0, "{trend}");
}

#[test]
fn a_trend_of_another_metric_as_json_has_the_points_of_its_table() {
    let dir = journal_dir(&[
        measured("run-a", 3, 3_000, 3, 0.5),
        on(measured("run-b", 1, 1_000, 1, 0.75), "orders", "mysql"),
    ]);

    let trend = json_trend(dir.path(), "duration_ms", None);

    assert_eq!(
        trend,
        serde_json::json!({
            "metric": "duration_ms",
            "target": null,
            "journals": 2,
            "journals_without_target": 0,
            "points": [
                {
                    "experiment_id": "run-b",
                    "title": "experiment",
                    "status": "completed",
                    "started_at_ns": 1_700_000_001_000_000_000_i64,
                    "value": 1000.0,
                },
                {
                    "experiment_id": "run-a",
                    "title": "experiment",
                    "status": "completed",
                    "started_at_ns": 1_700_000_003_000_000_000_i64,
                    "value": 3000.0,
                },
            ],
            "summary": { "runs": 2, "min": 1000.0, "max": 3000.0, "avg": 2000.0 },
        })
    );
}

#[test]
fn a_trend_as_json_by_target_counts_the_journals_that_name_no_target() {
    let dir = journal_dir(&[
        measured("run-a", 3, 3_000, 3, 0.5),
        on(measured("run-b", 1, 1_000, 1, 0.75), "orders", "mysql"),
    ]);

    let trend = json_trend(dir.path(), "duration_ms", Some("orders"));

    assert_eq!(trend["journals_without_target"], 1, "{trend}");
    assert_eq!(trend["target"], "orders", "{trend}");
    assert_eq!(trend["points"].as_array().unwrap().len(), 1, "{trend}");
}

#[test]
fn a_trend_as_json_without_points_has_no_summary() {
    let dir = journal_dir(&[no_outage("no-outage", 2, "orders")]);

    let trend = json_trend(dir.path(), "rto_margin_s", None);

    assert_eq!(trend["points"], serde_json::json!([]), "{trend}");
    assert_eq!(trend["summary"], serde_json::Value::Null, "{trend}");
    assert_eq!(trend["runs_without_outage"], 1, "{trend}");
}
