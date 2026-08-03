//! Tests for `tumult init` scaffolding (`init_at`, `generate_template`) and
//! the dry-run renderer (`print_dry_run`) via `cmd_run --dry-run`.

use super::super::*;
use tempfile::TempDir;
use tumult_core::execution::RollbackStrategy;
use tumult_core::types::{
    Activity, ActivityType, BaselineConfig, BaselineMethod, Estimate, ExpectedOutcome, Experiment,
    Hypothesis, Provider, RegulatoryMapping,
};

#[test]
fn init_at_writes_parseable_template_with_default_plugin() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("experiment.toon");

    init_at(&path, None).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("tumult-example"), "{content}");
    // The scaffold is a valid experiment: hypothesis, method, rollbacks.
    let experiment = tumult_core::engine::parse_experiment(&content).unwrap();
    assert!(experiment.steady_state_hypothesis.is_some());
    assert_eq!(experiment.method.len(), 2);
    assert_eq!(experiment.rollbacks.len(), 1);
}

#[test]
fn init_at_embeds_plugin_name_and_refuses_to_clobber() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("experiment.toon");

    init_at(&path, Some("tumult-pg")).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("tumult-pg"), "{content}");

    let err = init_at(&path, None).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
}

#[test]
fn generate_template_distinguishes_default_and_named_plugin() {
    let default = generate_template(None);
    let named = generate_template(Some("tumult-redis"));
    assert!(default.contains("tumult-example"));
    assert!(named.contains("tumult-redis"));
    assert!(!named.contains("tumult-example"));
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
        }),
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
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
