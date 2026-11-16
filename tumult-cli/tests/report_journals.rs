//! The HTML report `tumult report` writes for what a journal can hold and
//! a run in the lab does not produce: a load that fell short, a breached
//! objective, a step that timed out, several versions of a target.
//!
//! Each journal is written to a file as a run writes it, and the binary
//! reads it from there.
#![cfg(unix)]

mod lab;
mod report_page;

use lab::Lab;
use report_page::{
    card, card_labels, failed_rows, has_section, report, report_command, row, rows, section, words,
    written,
};
use tumult_core::journal::write_journal;
use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, BlastRadiusRecord, Confidence,
    DegradationLevel, DuringResult, Estimate, ExpectedOutcome, ExperimentStatus, HaltRecord,
    HypothesisResult, Journal, LoadExecution, LoadResult, LoadSample, LoadTool, NfrCriticality,
    NfrKind, NfrOperator, NfrState, NfrVerdict, ObjectiveOutcome, PostResult, ProbeDuring,
    ProbePost, RegulatoryMapping, RegulatoryRequirement, RequestCounts, SpanId, TargetVersion,
    TestKind, TraceId, Trend, VersionShare, VersionSource,
};

/// 2026-11-16 22:15:19 UTC.
const STARTED: i64 = 1_794_867_319_000_000_000;
const MILLISECOND: i64 = 1_000_000;

/// Every section a report can have but its summary.
const SECTIONS: [&str; 11] = [
    "halt",
    "steady-state",
    "steps",
    "objectives",
    "load",
    "measurements",
    "target-versions",
    "blast-radius",
    "estimate",
    "analysis",
    "regulatory",
];

/// The journal of a run of a minute that did nothing.
fn journal() -> Journal {
    Journal {
        experiment_title: "A run that was written down".into(),
        experiment_id: "run-1".into(),
        status: ExperimentStatus::Completed,
        started_at_ns: STARTED,
        ended_at_ns: STARTED + 60_000 * MILLISECOND,
        duration_ms: 60_000,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![],
        rollback_results: vec![],
        rollback_failures: 0,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: None,
        regulatory: None,
        halt: None,
        blast_radius: None,
        kind: TestKind::Chaos,
        objectives: None,
        probe_samples: vec![],
        target_version: None,
    }
}

/// A step that began `after_ms` into the run and left no trace and no
/// words.
fn step(name: &str, status: ActivityStatus, after_ms: i64, duration_ms: u64) -> ActivityResult {
    ActivityResult {
        name: name.into(),
        activity_type: ActivityType::Action,
        status,
        started_at_ns: STARTED + after_ms * MILLISECOND,
        duration_ms,
        output: None,
        error: None,
        trace_id: TraceId(String::new()),
        span_id: SpanId::empty(),
    }
}

/// The report the binary writes for `journal`, which it reads from a file.
fn report_for(journal: &Journal) -> String {
    let lab = Lab::with_state("ok");
    write_journal(journal, &lab.path("journal.toon")).expect("write the journal");
    report(&lab)
}

#[test]
fn a_run_that_did_nothing_has_a_summary_and_no_other_section() {
    let html = report_for(&journal());

    assert!(
        html.contains("<title>Tumult report: A run that was written down</title>"),
        "{html}"
    );
    assert!(
        html.contains("<h1>A run that was written down</h1>"),
        "{html}"
    );
    assert_eq!(
        card_labels(&html),
        [
            "Kind",
            "Started",
            "Duration",
            "Steady state before",
            "Steady state after",
            "Rollbacks"
        ]
    );
    assert_eq!(card(&html, "Kind"), "chaos");
    assert_eq!(card(&html, "Started"), "2026-11-16 22:15:19 UTC");
    assert_eq!(card(&html, "Duration"), "1 min 0 s");
    assert_eq!(card(&html, "Steady state before"), "not checked");
    assert_eq!(card(&html, "Steady state after"), "not checked");
    assert_eq!(card(&html, "Rollbacks"), "none run");
    for id in SECTIONS {
        assert!(!has_section(&html, id), "{id} in: {html}");
    }
}

#[test]
fn every_status_of_a_run_has_its_colour() {
    let colours = [
        (ExperimentStatus::Completed, "ok", "completed"),
        (ExperimentStatus::Deviated, "warn", "deviated"),
        (ExperimentStatus::Aborted, "fail", "aborted"),
        (ExperimentStatus::Failed, "fail", "failed"),
        (ExperimentStatus::Interrupted, "fail", "interrupted"),
        (ExperimentStatus::Halted, "fail", "halted"),
    ];
    for (status, class, word) in colours {
        let html = report_for(&Journal {
            status,
            ..journal()
        });

        let header = format!(
            r#"<p class="lede"><span class="badge {class}">{word}</span> Run <code>run-1</code></p>"#
        );
        assert!(html.contains(&header), "no {header} in: {html}");
    }
}

#[test]
fn the_steps_of_a_run_say_how_each_ended_and_when() {
    let trace = "b".repeat(32);
    let html = report_for(&Journal {
        method_results: vec![
            ActivityResult {
                output: Some("partial output".into()),
                ..step("timed-out", ActivityStatus::Timeout, 1500, 30_000)
            },
            ActivityResult {
                activity_type: ActivityType::Probe,
                ..step("skipped-step", ActivityStatus::Skipped, 31_500, 0)
            },
            ActivityResult {
                trace_id: TraceId(trace),
                ..step("no-detail", ActivityStatus::Failed, 31_500, 250)
            },
            ActivityResult {
                output: Some("half done".into()),
                error: Some("then it broke".into()),
                ..step("both", ActivityStatus::Failed, 31_800, 9)
            },
            step("quiet", ActivityStatus::Succeeded, 31_809, 1),
        ],
        rollback_results: vec![ActivityResult {
            error: Some("no way back".into()),
            ..step("undo", ActivityStatus::Failed, 32_000, 65_000)
        }],
        rollback_failures: 1,
        ..journal()
    });

    let steps = section(&html, "steps");
    assert_eq!(
        rows(steps),
        [
            row(&["Phase", "Step", "Type", "Status", "Start", "Duration", "Trace", "Detail"]),
            row(&[
                "method",
                "timed-out",
                "action",
                "timeout",
                "+1.5 s",
                "30.0 s",
                "",
                "partial output"
            ]),
            row(&[
                "method",
                "skipped-step",
                "probe",
                "skipped",
                "+31.5 s",
                "0 ms",
                "",
                ""
            ]),
            row(&[
                "method",
                "no-detail",
                "action",
                "failed",
                "+31.5 s",
                "250 ms",
                "bbbbbbbbbbbbbbbb",
                "(no detail captured)"
            ]),
            row(&[
                "method",
                "both",
                "action",
                "failed",
                "+31.8 s",
                "9 ms",
                "",
                "half done then it broke"
            ]),
            row(&[
                "method",
                "quiet",
                "action",
                "succeeded",
                "+31.8 s",
                "1 ms",
                "",
                ""
            ]),
            row(&[
                "rollback",
                "undo",
                "action",
                "failed",
                "+32.0 s",
                "1 min 5 s",
                "",
                "no way back"
            ]),
        ]
    );
    assert_eq!(
        failed_rows(steps),
        ["method", "method", "method", "rollback"]
    );
    for badge in [
        r#"<span class="badge fail">timeout</span>"#,
        r#"<span class="badge muted">skipped</span>"#,
        r#"<span class="badge fail">failed</span>"#,
        r#"<span class="badge ok">succeeded</span>"#,
        r#"<span class="detail-output">half done</span> <span class="detail-error">then it broke</span>"#,
    ] {
        assert!(steps.contains(badge), "no {badge} in: {steps}");
    }
    assert_eq!(card(&html, "Rollbacks"), "1 run, 1 failed");
}

#[test]
fn a_steady_state_that_was_checked_once_is_shown_once() {
    let html = report_for(&Journal {
        status: ExperimentStatus::Aborted,
        steady_state_before: Some(HypothesisResult {
            title: "The target answers".into(),
            met: false,
            probe_results: vec![ActivityResult {
                activity_type: ActivityType::Probe,
                error: Some("connection refused".into()),
                ..step("answers", ActivityStatus::Failed, 0, 1300)
            }],
        }),
        ..journal()
    });

    let steady = section(&html, "steady-state");
    assert!(
        steady.contains(
            r#"<h3>Before the method: The target answers <span class="badge fail">not met</span></h3>"#
        ),
        "{steady}"
    );
    assert!(!steady.contains("After the method"), "{steady}");
    assert_eq!(
        rows(steady),
        [
            row(&["Probe", "Status", "Duration", "Trace", "Output"]),
            row(&["answers", "failed", "1.3 s", "", "connection refused"]),
        ]
    );
    assert_eq!(card(&html, "Steady state before"), "not met");
    assert_eq!(card(&html, "Steady state after"), "not checked");
}

fn verdict(id: &str, operator: NfrOperator, threshold: f64, state: NfrState) -> NfrVerdict {
    NfrVerdict {
        nfr_id: id.into(),
        kind: NfrKind::Latency,
        metric: "load.latency_p99_ms".into(),
        operator,
        threshold,
        observed: None,
        state,
        margin: None,
        criticality: NfrCriticality::Major,
    }
}

#[test]
fn the_objectives_of_a_run_say_what_was_breached_and_by_how_much() {
    let html = report_for(&Journal {
        objectives: Some(ObjectiveOutcome {
            rpo_measured_s: Some(2.5),
            verdicts: vec![
                NfrVerdict {
                    observed: Some(312.5),
                    margin: Some(-62.5),
                    criticality: NfrCriticality::Blocking,
                    ..verdict("p99", NfrOperator::Lte, 250.0, NfrState::Breached)
                },
                NfrVerdict {
                    kind: NfrKind::Capacity,
                    metric: "load.vus".into(),
                    observed: Some(3.0),
                    margin: Some(0.0),
                    criticality: NfrCriticality::Minor,
                    ..verdict("exact", NfrOperator::Eq, 3.0, NfrState::Met)
                },
                NfrVerdict {
                    kind: NfrKind::Availability,
                    metric: "during.availability".into(),
                    observed: Some(0.995),
                    margin: Some(0.005),
                    ..verdict("floor", NfrOperator::Gte, 0.99, NfrState::Met)
                },
                verdict("unknown", NfrOperator::Lte, 1.5, NfrState::Inconclusive),
            ],
            ..ObjectiveOutcome::default()
        }),
        ..journal()
    });

    let objectives = section(&html, "objectives");
    assert_eq!(
        rows(objectives),
        [
            row(&["Recovery", "Target", "Measured"]),
            row(&["RPO", "not set", "2.5 s"]),
            row(&[
                "Objective",
                "Kind",
                "Metric",
                "Must be",
                "Observed",
                "Margin",
                "State",
                "Criticality"
            ]),
            row(&[
                "p99",
                "latency",
                "load.latency_p99_ms",
                "at most 250",
                "312.5",
                "-62.5",
                "breached",
                "blocking"
            ]),
            row(&[
                "exact",
                "capacity",
                "load.vus",
                "equal to 3",
                "3",
                "0",
                "met",
                "minor"
            ]),
            row(&[
                "floor",
                "availability",
                "during.availability",
                "at least 0.99",
                "0.995",
                "0.005",
                "met",
                "major"
            ]),
            row(&[
                "unknown",
                "latency",
                "load.latency_p99_ms",
                "at most 1.5",
                "not measured",
                "",
                "inconclusive",
                "major"
            ]),
        ]
    );
    assert_eq!(failed_rows(objectives), ["p99"]);
    for badge in [
        r#"<span class="badge fail">breached</span>"#,
        r#"<span class="badge ok">met</span>"#,
        r#"<span class="badge warn">inconclusive</span>"#,
    ] {
        assert!(objectives.contains(badge), "no {badge} in: {objectives}");
    }
    assert_eq!(card(&html, "Objectives"), "2 of 4 met");
}

#[test]
fn a_recovery_target_without_a_verdict_is_shown_without_a_count() {
    let html = report_for(&Journal {
        objectives: Some(ObjectiveOutcome {
            rto_target_s: Some(30.0),
            ..ObjectiveOutcome::default()
        }),
        ..journal()
    });

    assert_eq!(
        rows(section(&html, "objectives")),
        [
            row(&["Recovery", "Target", "Measured"]),
            row(&["RTO", "30.0 s", "not measured"]),
        ]
    );
    assert!(!card_labels(&html).contains(&"Objectives".to_string()));
}

#[test]
fn objectives_that_say_nothing_are_no_section() {
    let html = report_for(&Journal {
        objectives: Some(ObjectiveOutcome::default()),
        ..journal()
    });

    assert!(!has_section(&html, "objectives"), "{html}");
    assert!(!card_labels(&html).contains(&"Objectives".to_string()));
}

/// A window of ten seconds that began `after_ms` into the run.
fn window(after_ms: i64, p50: f64, p95: f64, p99: f64) -> LoadSample {
    LoadSample {
        ts_ns: STARTED + after_ms * MILLISECOND,
        duration_s: 10.0,
        vus: 4,
        throughput_rps: 12.5,
        latency_p50_ms: p50,
        latency_p95_ms: p95,
        latency_p99_ms: p99,
        error_rate: 0.25,
        requests: 125,
        counts: None,
        request_timeout_ms: Some(1000.0),
    }
}

/// A load of a minute as its tool summed it up.
fn load(execution: LoadExecution, samples: Vec<LoadSample>) -> LoadResult {
    LoadResult {
        tool: LoadTool::K6,
        started_at_ns: STARTED + 2000 * MILLISECOND,
        ended_at_ns: STARTED + 62_000 * MILLISECOND,
        duration_s: 60.0,
        vus: 4,
        throughput_rps: 12.5,
        latency_p50_ms: 40.0,
        latency_p95_ms: 900.26,
        latency_p99_ms: 1000.0,
        error_rate: 0.4,
        total_requests: 750,
        thresholds_met: false,
        samples,
        execution,
    }
}

#[test]
fn a_load_whose_plan_fell_short_says_how() {
    let html = report_for(&Journal {
        load_result: Some(load(
            LoadExecution {
                planned_windows: Some(6),
                completed_windows: Some(3),
                unanswered_windows: 2,
                lost_to_unanswered: 1,
                aborted: false,
                reasons: vec![
                    "two windows got no answer".into(),
                    "one window never ran".into(),
                ],
                requests: Some(RequestCounts {
                    sent: 100,
                    ok: 60,
                    timed_out: 20,
                    transport: 5,
                    http_4xx: 4,
                    http_5xx: 3,
                    http_other: 2,
                    unclassified: 6,
                }),
                censored_windows: 1,
            },
            vec![
                window(2000, 20.0, 80.0, 999.9),
                window(12_000, 200.0, 1000.0, 1000.5),
            ],
        )),
        ..journal()
    });

    assert_eq!(
        rows(section(&html, "load")),
        [
            row(&["Tool", "k6"]),
            row(&["Users", "4"]),
            row(&["Duration", "1 min 0 s"]),
            row(&["Requests", "750"]),
            row(&["Throughput", "12.5 requests/s"]),
            row(&["Latency p50", "40.0 ms"]),
            row(&["Latency p95", "900.3 ms"]),
            row(&["Latency p99", "1000.0 ms"]),
            row(&["Error rate", "40.00 %"]),
            row(&["Thresholds", "not met"]),
            row(&["Windows", "3 of 6 completed"]),
            row(&["Windows without an answer", "2"]),
            row(&["Windows that never ran in a blackout", "1"]),
            row(&["Windows with a p99 among the timeouts", "1"]),
            row(&[
                "Why the plan fell short",
                "two windows got no answer; one window never ran"
            ]),
            row(&[
                "Requests by outcome",
                "sent 100, ok 60, timed out 20, transport errors 5, 4xx 4, 5xx 3, \
                 other status 2, unclassified 6"
            ]),
            row(&[
                "Start",
                "Length",
                "Users",
                "Requests",
                "Throughput",
                "p50",
                "p95",
                "p99",
                "Error rate"
            ]),
            row(&[
                "+2.0 s",
                "10.0 s",
                "4",
                "125",
                "12.5 requests/s",
                "20.0 ms",
                "80.0 ms",
                "999.9 ms",
                "25.00 %"
            ]),
            row(&[
                "+12.0 s",
                "10.0 s",
                "4",
                "125",
                "12.5 requests/s",
                "200.0 ms",
                "at least 1000.0 ms",
                "at least 1000.5 ms",
                "25.00 %"
            ]),
        ]
    );
}

#[test]
fn a_load_says_only_what_its_tool_told() {
    let html = report_for(&Journal {
        load_result: Some(LoadResult {
            thresholds_met: true,
            ..load(
                LoadExecution {
                    // A count of the plan alone says nothing of how it went.
                    planned_windows: Some(6),
                    requests: Some(RequestCounts {
                        sent: 750,
                        timed_out: 750,
                        ..RequestCounts::default()
                    }),
                    ..LoadExecution::default()
                },
                vec![LoadSample {
                    request_timeout_ms: None,
                    ..window(2000, 20.0, 80.0, 5000.0)
                }],
            )
        }),
        ..journal()
    });

    let load = rows(section(&html, "load"));
    assert_eq!(
        load[9..],
        [
            row(&["Thresholds", "met"]),
            row(&["Requests by outcome", "sent 750, timed out 750"]),
            row(&[
                "Start",
                "Length",
                "Users",
                "Requests",
                "Throughput",
                "p50",
                "p95",
                "p99",
                "Error rate"
            ]),
            // Without a request timeout no percentile is a lower bound.
            row(&[
                "+2.0 s",
                "10.0 s",
                "4",
                "125",
                "12.5 requests/s",
                "20.0 ms",
                "80.0 ms",
                "5000.0 ms",
                "25.00 %"
            ]),
        ]
    );
}

#[test]
fn a_load_without_windows_has_no_table_of_windows() {
    let html = report_for(&Journal {
        load_result: Some(load(
            LoadExecution {
                aborted: true,
                ..LoadExecution::default()
            },
            vec![],
        )),
        ..journal()
    });

    let load = section(&html, "load");
    assert_eq!(
        rows(load)[10..],
        [row(&["Cut", "the load was cut before its planned end"])]
    );
    assert!(!load.contains("<thead>"), "{load}");
}

#[test]
fn the_versions_the_telemetry_of_the_target_named_are_in_the_report() {
    let share = |version: &str, requests| VersionShare {
        version: version.into(),
        requests,
    };
    let html = report_for(&Journal {
        target_version: Some(TargetVersion {
            version: "1.4.2".into(),
            version_source: VersionSource::Mixed,
            shares: vec![share("1.4.2", 900), share("1.4.3", 100)],
        }),
        ..journal()
    });

    assert_eq!(
        card(&html, "Target version"),
        "1.4.2 (mixed, 90.00 % of the requests)"
    );
    assert_eq!(
        rows(section(&html, "target-versions")),
        [
            row(&["Version", "Requests"]),
            row(&["1.4.2", "900"]),
            row(&["1.4.3", "100"]),
        ]
    );
}

#[test]
fn a_version_one_build_served_alone_has_no_shares() {
    let html = report_for(&Journal {
        target_version: Some(TargetVersion {
            version: "1.4.2".into(),
            version_source: VersionSource::Observed,
            shares: vec![],
        }),
        ..journal()
    });

    assert_eq!(card(&html, "Target version"), "1.4.2 (observed)");
    assert!(!has_section(&html, "target-versions"), "{html}");
}

fn during() -> DuringResult {
    let fault_started = STARTED + 2000 * MILLISECOND;
    DuringResult {
        started_at_ns: fault_started,
        ended_at_ns: fault_started + 12_500 * MILLISECOND,
        fault_active_s: 12.5,
        sample_interval_s: 0.5,
        probes: vec![
            ProbeDuring {
                name: "api".into(),
                samples: 25,
                mean: 12.34,
                max: 80.0,
                min: 2.0,
                error_rate: 0.2,
                breached_at_ns: Some(fault_started + 1500 * MILLISECOND),
                breach_count: 5,
            },
            ProbeDuring {
                name: "db".into(),
                samples: 24,
                mean: 3.0,
                max: 4.0,
                min: 1.0,
                error_rate: 0.0,
                breached_at_ns: None,
                breach_count: 0,
            },
        ],
        degradation_onset_s: None,
        degradation_peak_s: None,
        degradation_magnitude: None,
        graceful_degradation: None,
    }
}

fn post() -> PostResult {
    PostResult {
        started_at_ns: STARTED + 15_000 * MILLISECOND,
        ended_at_ns: STARTED + 45_000 * MILLISECOND,
        duration_s: 30.0,
        samples: 30,
        probes: vec![
            ProbePost {
                name: "api".into(),
                mean: 10.0,
                p95: 20.5,
                error_rate: 0.1,
                returned_to_baseline: true,
                recovery_time_s: 4.2,
            },
            ProbePost {
                name: "db".into(),
                mean: 3.0,
                p95: 4.0,
                error_rate: 1.0,
                returned_to_baseline: false,
                recovery_time_s: 30.0,
            },
        ],
        recovery_time_s: 30.0,
        full_recovery: false,
        residual_degradation: None,
        data_integrity_verified: None,
        data_loss_detected: None,
        mttr_s: None,
    }
}

#[test]
fn the_measurements_of_a_run_are_those_of_each_probe_during_and_after_the_fault() {
    let html = report_for(&Journal {
        during_result: Some(during()),
        post_result: Some(post()),
        ..journal()
    });

    let measured = section(&html, "measurements");
    let titles: Vec<String> = measured
        .split("<h3>")
        .skip(1)
        .map(|part| words(part.split("</h3>").next().unwrap_or(part)))
        .collect();
    assert_eq!(titles, ["During the fault", "After the fault"]);
    assert_eq!(
        rows(measured),
        [
            row(&["Fault active", "12.5 s"]),
            row(&["Sample interval", "500 ms"]),
            row(&[
                "Probe",
                "Samples",
                "Mean",
                "Min",
                "Max",
                "Error rate",
                "Breaches",
                "First breach"
            ]),
            row(&["api", "25", "12.3 ms", "2.0 ms", "80.0 ms", "20.00 %", "5", "+1.5 s"]),
            row(&["db", "24", "3.0 ms", "1.0 ms", "4.0 ms", "0.00 %", "0", ""]),
            row(&["Watched for", "30.0 s"]),
            row(&["Samples", "30"]),
            row(&["Recovery time", "30.0 s"]),
            row(&["Full recovery", "no"]),
            row(&[
                "Probe",
                "Mean",
                "p95",
                "Error rate",
                "Back in tolerance",
                "Recovery time"
            ]),
            row(&["api", "10.0 ms", "20.5 ms", "10.00 %", "yes", "4.2 s"]),
            row(&["db", "3.0 ms", "4.0 ms", "100.00 %", "no", "30.0 s"]),
        ]
    );
}

#[test]
fn what_a_run_measured_of_its_degradation_and_its_data_is_in_the_report() {
    let html = report_for(&Journal {
        during_result: Some(DuringResult {
            probes: vec![],
            degradation_onset_s: Some(1.2),
            degradation_peak_s: Some(4.0),
            degradation_magnitude: Some(0.375),
            graceful_degradation: Some(true),
            ..during()
        }),
        post_result: Some(PostResult {
            probes: vec![],
            recovery_time_s: 4.2,
            full_recovery: true,
            residual_degradation: Some(0.125),
            data_integrity_verified: Some(true),
            data_loss_detected: Some(false),
            mttr_s: Some(5.5),
            ..post()
        }),
        ..journal()
    });

    assert_eq!(
        rows(section(&html, "measurements")),
        [
            row(&["Fault active", "12.5 s"]),
            row(&["Sample interval", "500 ms"]),
            row(&["Degradation first seen after", "1.2 s"]),
            row(&["Degradation at its peak after", "4.0 s"]),
            row(&["Worst deviation from the baseline", "0.375"]),
            row(&["Degraded gradually", "yes"]),
            row(&["Watched for", "30.0 s"]),
            row(&["Samples", "30"]),
            row(&["Recovery time", "4.2 s"]),
            row(&["Full recovery", "yes"]),
            row(&["MTTR", "5.5 s"]),
            row(&["Deviation left at the end", "0.125"]),
            row(&["Data integrity verified", "yes"]),
            row(&["Data loss detected", "no"]),
        ]
    );
}

#[test]
fn a_run_that_never_watched_the_recovery_has_no_part_for_it() {
    let html = report_for(&Journal {
        during_result: Some(during()),
        ..journal()
    });

    let measured = section(&html, "measurements");
    assert!(measured.contains("<h3>During the fault</h3>"), "{measured}");
    assert!(!measured.contains("After the fault"), "{measured}");
}

#[test]
fn a_halt_says_what_the_guard_saw_and_how_long_the_halt_took() {
    let halt = HaltRecord {
        guard_name: "error-budget".into(),
        observed: Some("0.31".into()),
        safe_condition: "range [0, 0.05]".into(),
        breach_count: 3,
        breached_at_ns: STARTED + 2500 * MILLISECOND,
        time_to_halt_ms: 2600,
        rollback_ms: 40,
    };
    let halted = Journal {
        status: ExperimentStatus::Halted,
        halt: Some(halt.clone()),
        ..journal()
    };

    let html = report_for(&halted);

    assert!(
        html.contains("<section id=\"halt\" class=\"callout\">\n<h2>Halted by a guard</h2>"),
        "{html}"
    );
    assert_eq!(
        rows(section(&html, "halt")),
        [
            row(&["Guard", "error-budget"]),
            row(&["Safe condition", "range [0, 0.05]"]),
            row(&["Observed", "0.31"]),
            row(&["Breaches", "3"]),
            row(&["Breached at", "+2.5 s"]),
            row(&["Time to halt", "2.6 s"]),
            row(&["Rollback time", "40 ms"]),
        ]
    );
    // The halt is what a reader must see first.
    let (before_halt, _) = html.split_once("<section id=\"halt\"").expect("the halt");
    assert!(!before_halt.contains("<section"), "{before_halt}");

    let silent = report_for(&Journal {
        halt: Some(HaltRecord {
            observed: None,
            ..halt
        }),
        ..halted
    });
    assert_eq!(
        rows(section(&silent, "halt"))[2],
        row(&["Observed", "no output"])
    );
}

#[test]
fn what_a_run_was_planned_with_is_shown_as_far_as_it_was_said() {
    let html = report_for(&Journal {
        estimate: Some(Estimate {
            expected_outcome: ExpectedOutcome::Deviated,
            expected_recovery_s: None,
            expected_degradation: None,
            expected_data_loss: None,
            confidence: None,
            rationale: None,
            prior_runs: None,
        }),
        blast_radius: Some(BlastRadiusRecord {
            note: None,
            max_concurrent_faults: None,
            peak_concurrent_faults: 3,
        }),
        analysis: Some(AnalysisResult {
            estimate_accuracy: None,
            estimate_recovery_delta_s: Some(-1.5),
            trend: Some(Trend::Improving),
            resilience_score: None,
        }),
        regulatory: Some(RegulatoryMapping {
            frameworks: vec!["dora".into()],
            requirements: vec![],
        }),
        ..journal()
    });

    assert_eq!(
        rows(section(&html, "estimate")),
        [row(&["Expected outcome", "deviated"])]
    );
    assert_eq!(
        rows(section(&html, "blast-radius")),
        [row(&["Peak concurrent faults", "3"])]
    );
    assert_eq!(
        rows(section(&html, "analysis")),
        [
            row(&["Recovery against the estimate", "-1.5 s"]),
            row(&["Trend", "improving"]),
        ]
    );
    assert_eq!(
        words(section(&html, "regulatory")),
        "Regulatory mapping Frameworks: dora"
    );
}

#[test]
fn an_estimate_and_an_analysis_say_all_that_they_hold() {
    let html = report_for(&Journal {
        estimate: Some(Estimate {
            expected_outcome: ExpectedOutcome::Unaffected,
            expected_recovery_s: Some(90.0),
            expected_degradation: Some(DegradationLevel::Severe),
            expected_data_loss: Some(true),
            confidence: Some(Confidence::Low),
            rationale: Some("The replica is a region away".into()),
            prior_runs: Some(0),
        }),
        analysis: Some(AnalysisResult {
            estimate_accuracy: Some(0.625),
            estimate_recovery_delta_s: Some(12.25),
            trend: Some(Trend::Degrading),
            resilience_score: Some(0.75),
        }),
        ..journal()
    });

    assert_eq!(
        rows(section(&html, "estimate")),
        [
            row(&["Expected outcome", "unaffected"]),
            row(&["Expected recovery", "1 min 30 s"]),
            row(&["Expected degradation", "severe"]),
            row(&["Expected data loss", "yes"]),
            row(&["Confidence", "low"]),
            row(&["Rationale", "The replica is a region away"]),
            row(&["Prior runs", "0"]),
        ]
    );
    assert_eq!(
        rows(section(&html, "analysis")),
        [
            row(&["Estimate accuracy", "62.50 %"]),
            row(&["Recovery against the estimate", "12.25 s"]),
            row(&["Trend", "degrading"]),
            row(&["Resilience score", "0.75"]),
        ]
    );
}

#[test]
fn an_analysis_and_a_mapping_that_say_nothing_are_no_sections() {
    let html = report_for(&Journal {
        analysis: Some(AnalysisResult {
            estimate_accuracy: None,
            estimate_recovery_delta_s: None,
            trend: None,
            resilience_score: None,
        }),
        regulatory: Some(RegulatoryMapping {
            frameworks: vec![],
            requirements: vec![],
        }),
        ..journal()
    });

    assert!(!has_section(&html, "analysis"), "{html}");
    assert!(!has_section(&html, "regulatory"), "{html}");
}

#[test]
fn requirements_without_a_framework_are_a_table_alone() {
    let html = report_for(&Journal {
        regulatory: Some(RegulatoryMapping {
            frameworks: vec![],
            requirements: vec![RegulatoryRequirement {
                id: "A.17.1".into(),
                description: "Continuity is tested".into(),
                evidence: "This run".into(),
            }],
        }),
        ..journal()
    });

    let regulatory = section(&html, "regulatory");
    assert!(!regulatory.contains("Frameworks"), "{regulatory}");
    assert_eq!(
        rows(regulatory),
        [
            row(&["Requirement", "Description", "Evidence"]),
            row(&["A.17.1", "Continuity is tested", "This run"]),
        ]
    );
}

/// A text no report may take for markup, with a number to find it by.
fn markup(number: &mut u32) -> String {
    *number += 1;
    format!("<x-{number}> & \"co\"")
}

/// A journal in which every text is markup; the journal and how many texts
/// it holds.
fn journal_of_markup() -> (Journal, u32) {
    let mut n = 0;
    let mut said = |status: ActivityStatus| ActivityResult {
        name: markup(&mut n),
        output: Some(markup(&mut n)),
        error: Some(markup(&mut n)),
        trace_id: TraceId(markup(&mut n)),
        ..step("", status, 0, 1)
    };
    let before = said(ActivityStatus::Succeeded);
    let after = said(ActivityStatus::Failed);
    let method = said(ActivityStatus::Failed);
    let rollback = said(ActivityStatus::Succeeded);
    let journal = Journal {
        experiment_title: markup(&mut n),
        experiment_id: markup(&mut n),
        status: ExperimentStatus::Halted,
        steady_state_before: Some(HypothesisResult {
            title: markup(&mut n),
            met: true,
            probe_results: vec![before],
        }),
        steady_state_after: Some(HypothesisResult {
            title: markup(&mut n),
            met: false,
            probe_results: vec![after],
        }),
        method_results: vec![method],
        rollback_results: vec![rollback],
        halt: Some(HaltRecord {
            guard_name: markup(&mut n),
            observed: Some(markup(&mut n)),
            safe_condition: markup(&mut n),
            breach_count: 1,
            breached_at_ns: STARTED,
            time_to_halt_ms: 1,
            rollback_ms: 1,
        }),
        target_version: Some(TargetVersion {
            version: "<x-version>".into(),
            version_source: VersionSource::Mixed,
            shares: vec![
                VersionShare {
                    version: "<x-version>".into(),
                    requests: 1,
                },
                VersionShare {
                    version: markup(&mut n),
                    requests: 1,
                },
            ],
        }),
        objectives: Some(ObjectiveOutcome {
            verdicts: vec![NfrVerdict {
                nfr_id: markup(&mut n),
                metric: markup(&mut n),
                ..verdict("", NfrOperator::Lte, 1.0, NfrState::Inconclusive)
            }],
            ..ObjectiveOutcome::default()
        }),
        load_result: Some(load(
            LoadExecution {
                reasons: vec![markup(&mut n), markup(&mut n)],
                ..LoadExecution::default()
            },
            vec![],
        )),
        during_result: Some(DuringResult {
            probes: vec![ProbeDuring {
                name: markup(&mut n),
                ..during().probes.remove(0)
            }],
            ..during()
        }),
        post_result: Some(PostResult {
            probes: vec![ProbePost {
                name: markup(&mut n),
                ..post().probes.remove(0)
            }],
            ..post()
        }),
        blast_radius: Some(BlastRadiusRecord {
            note: Some(markup(&mut n)),
            max_concurrent_faults: None,
            peak_concurrent_faults: 0,
        }),
        estimate: Some(Estimate {
            expected_outcome: ExpectedOutcome::Recovered,
            expected_recovery_s: None,
            expected_degradation: None,
            expected_data_loss: None,
            confidence: None,
            rationale: Some(markup(&mut n)),
            prior_runs: None,
        }),
        regulatory: Some(RegulatoryMapping {
            frameworks: vec![markup(&mut n), markup(&mut n)],
            requirements: vec![RegulatoryRequirement {
                id: markup(&mut n),
                description: markup(&mut n),
                evidence: markup(&mut n),
            }],
        }),
        ..journal()
    };
    (journal, n)
}

#[test]
fn no_text_of_a_journal_is_markup_in_its_report() {
    let (journal, texts) = journal_of_markup();
    let lab = Lab::with_state("ok");
    write_journal(&journal, &lab.path("journal.toon")).expect("write the journal");

    let output = report_command(&lab, &lab.path("journal.toon"))
        .args(["--trace-ui-base", "https://traces.example/\"><x-base>"])
        .output()
        .expect("run tumult");
    let html = written(&lab, &output);

    assert!(!html.contains("<x-"), "{html}");
    assert!(!html.contains("\"co\""), "{html}");
    for number in 1..=texts {
        let escaped = format!("&lt;x-{number}&gt; &amp; &quot;co&quot;");
        assert!(html.contains(&escaped), "no {escaped} in: {html}");
    }
    assert!(html.contains("&lt;x-version&gt;"), "{html}");
    assert!(
        html.contains(r#"href="https://traces.example/&quot;&gt;&lt;x-base&gt;/trace/&lt;x-"#),
        "{html}"
    );
}
