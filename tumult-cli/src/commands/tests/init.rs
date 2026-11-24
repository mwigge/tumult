//! Tests for `tumult init` scaffolding (`init_at`) and the dry-run renderer
//! (`print_dry_run`) via `cmd_run --dry-run`.

use super::super::*;
use tempfile::TempDir;
use tumult_core::execution::RollbackStrategy;
use tumult_core::types::{
    Activity, ActivityType, BaselineConfig, BaselineMethod, Estimate, ExpectedOutcome, Experiment,
    ExperimentTarget, Guard, Hypothesis, LoadConfig, LoadProfile, LoadTool, NfrCriticality,
    NfrKind, NfrObjective, NfrOperator, Provider, RegulatoryMapping, TestKind,
};

#[test]
fn init_at_writes_the_starter_template() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("experiment.toon");

    init_at(&path, None).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let experiment = tumult_core::engine::parse_experiment(&content).unwrap();
    assert_eq!(experiment, tumult_authoring::starter().unwrap());
}

#[test]
fn init_at_refuses_to_clobber() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("experiment.toon");
    std::fs::write(&path, "kept").unwrap();

    let err = init_at(&path, None).unwrap_err();

    assert!(err.to_string().contains("already exists"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept");
}

/// A dry run must render every optional section (estimate, baseline,
/// hypothesis probes, background marker, rollbacks, regulatory) and must not
/// produce a journal — it is a plan, not an execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_renders_all_optional_sections_without_executing() {
    let dir = TempDir::new().unwrap();
    let process = |args: Vec<String>| Provider::Process {
        path: "echo".into(),
        arguments: args,
        env: std::collections::HashMap::new(),
        timeout_s: Some(5.0),
    };
    let experiment = Experiment {
        version: "v1".into(),
        title: "full dry-run experiment".into(),
        description: Some("exercises every dry-run section".into()),
        tags: vec!["test".into()],
        configuration: indexmap::IndexMap::new(),
        secrets: indexmap::IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: Some(Hypothesis {
            title: "system healthy".into(),
            probes: vec![Activity {
                name: "health".into(),
                activity_type: ActivityType::Probe,
                provider: process(vec!["ok".into()]),
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            }],
        }),
        method: vec![
            Activity {
                name: "foreground-step".into(),
                activity_type: ActivityType::Action,
                provider: process(vec!["fg".into()]),
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            },
            Activity {
                name: "background-step".into(),
                activity_type: ActivityType::Action,
                provider: process(vec!["bg".into()]),
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: true,
                label_selector: None,
            },
        ],
        rollbacks: vec![Activity {
            name: "undo".into(),
            activity_type: ActivityType::Action,
            provider: process(vec!["undo".into()]),
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        estimate: Some(Estimate {
            expected_outcome: ExpectedOutcome::Recovered,
            expected_recovery_s: Some(30.0),
            expected_degradation: None,
            expected_data_loss: None,
            confidence: None,
            rationale: None,
            prior_runs: None,
        }),
        baseline: Some(BaselineConfig {
            duration_s: 60.0,
            warmup_s: None,
            interval_s: 5.0,
            method: BaselineMethod::Percentile,
            sigma: None,
            confidence: None,
        }),
        load: None,
        regulatory: Some(RegulatoryMapping {
            frameworks: vec!["dora".into()],
            requirements: vec![],
            tester: None,
        }),
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
        kind: tumult_core::types::TestKind::default(),
        target: None,
        nfr: vec![],
        rto_target_s: None,
        rpo_target_s: None,
        recovery: None,
    };
    let exp_path = dir.path().join("rich.toon");
    std::fs::write(&exp_path, toon_format::encode_default(&experiment).unwrap()).unwrap();
    let journal_path = dir.path().join("journal.toon");

    cmd_run(
        &exp_path,
        &journal_path,
        false,
        true, // dry_run
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap();

    // A dry run is a plan: no journal is written, nothing executed.
    assert!(!journal_path.exists());
}

fn dry_run_text(experiment: &Experiment) -> String {
    let ((), printed) = crate::output::testing::capture(|| print_dry_run(experiment));
    printed.stdout
}

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|printed| printed == line)
}

/// A recovery drill that names everything a run is judged by.
fn judged_experiment() -> Experiment {
    Experiment {
        title: "failover drill".into(),
        kind: TestKind::Dr,
        target: Some(ExperimentTarget {
            system: "checkout".into(),
            technology: Some("postgresql".into()),
            environment: Some("staging".into()),
            criticality: None,
            namespace: None,
            version: None,
        }),
        rto_target_s: Some(900.0),
        rpo_target_s: Some(60.0),
        nfr: vec![NfrObjective {
            id: "p95".into(),
            kind: NfrKind::Latency,
            metric: "load.latency_p95_ms".into(),
            operator: NfrOperator::Lte,
            threshold: 200.0,
            window_s: None,
            criticality: NfrCriticality::Major,
            description: None,
        }],
        load: Some(LoadConfig {
            tool: LoadTool::K6,
            script: "examples/k6/smoke-test.js".into(),
            vus: Some(10),
            duration_s: Some(30.0),
            thresholds: std::collections::HashMap::new(),
            profile: Some(LoadProfile::Ramp {
                start_vus: 1,
                ramp_up_s: 10.0,
            }),
        }),
        guards: vec![Guard {
            name: "error-budget".into(),
            probe: Activity {
                name: "errors".into(),
                activity_type: ActivityType::Probe,
                ..Default::default()
            },
            min_breaches: 2,
        }],
        method: vec![Activity {
            name: "stop-primary".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn dry_run_shows_what_the_run_is_judged_by() {
    let out = dry_run_text(&judged_experiment());

    for line in [
        "Kind: dr",
        "Target: checkout (technology: postgresql, environment: staging)",
        "Recovery targets: RTO 900s, RPO 60s",
        "Objectives (1):",
        "  - p95 (latency, major): load.latency_p95_ms lte 200",
        "Load: k6 examples/k6/smoke-test.js (10 users, 30s, ramp)",
        "Guards (1):",
        "  - error-budget: probe errors, halts after 2 breaches",
    ] {
        assert!(has_line(&out, line), "no line {line:?} in:\n{out}");
    }
}

#[test]
fn dry_run_of_a_plain_experiment_has_a_kind_and_no_empty_sections() {
    let plain = Experiment {
        title: "plain".into(),
        method: vec![Activity {
            name: "step".into(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let out = dry_run_text(&plain);

    assert!(has_line(&out, "Kind: chaos"), "{out}");
    for absent in [
        "Target:",
        "Recovery targets:",
        "Objectives",
        "Load:",
        "Guards",
    ] {
        assert!(!out.contains(absent), "{absent} in:\n{out}");
    }
}

#[test]
fn dry_run_shows_a_flat_load_and_one_recovery_target() {
    let mut experiment = judged_experiment();
    experiment.kind = TestKind::Performance;
    experiment.rpo_target_s = None;
    let load = experiment.load.as_mut().unwrap();
    load.profile = None;
    load.vus = None;

    let out = dry_run_text(&experiment);

    assert!(has_line(&out, "Recovery targets: RTO 900s"), "{out}");
    assert!(
        has_line(&out, "Load: k6 examples/k6/smoke-test.js (30s, flat)"),
        "{out}"
    );
}

#[test]
fn dry_run_says_that_a_run_does_not_use_the_baseline_block() {
    let mut experiment = judged_experiment();
    experiment.baseline = Some(BaselineConfig {
        duration_s: 60.0,
        warmup_s: None,
        interval_s: 5.0,
        method: BaselineMethod::Percentile,
        sigma: None,
        confidence: None,
    });

    let out = dry_run_text(&experiment);

    assert!(
        has_line(&out, "Baseline (declared, a run does not use it):"),
        "{out}"
    );
    assert!(!out.contains("Phase 1"), "{out}");
}
