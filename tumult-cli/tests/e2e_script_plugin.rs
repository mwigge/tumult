//! End-to-end test: tumult run with a script plugin → verify TOON journal.
//!
//! This test creates a temporary script plugin and experiment, runs it
//! through the CLI command layer, and verifies the journal output.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use indexmap::IndexMap;
use tempfile::TempDir;
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{run_experiment, ActivityExecutor, RunConfig};
use tumult_core::types::*;

/// Create a script plugin in a temp directory that:
/// - Has an action that echoes "injected"
/// - Has a probe that echoes "200"
fn setup_script_plugin(dir: &std::path::Path) {
    let plugin_dir = dir.join("plugins").join("test-chaos");
    std::fs::create_dir_all(plugin_dir.join("actions")).unwrap();
    std::fs::create_dir_all(plugin_dir.join("probes")).unwrap();

    // Plugin manifest
    let manifest = r#"{
        "name": "test-chaos",
        "version": "0.1.0",
        "description": "Test chaos plugin",
        "actions": [{"name": "inject", "script": "actions/inject.sh", "description": "Inject fault"}],
        "probes": [{"name": "check", "script": "probes/check.sh", "description": "Check health"}]
    }"#;
    std::fs::write(plugin_dir.join("plugin.toon"), manifest).unwrap();

    // Action script
    let action_script = "#!/bin/sh\necho injected\nexit 0\n";
    let action_path = plugin_dir.join("actions/inject.sh");
    std::fs::write(&action_path, action_script).unwrap();
    std::fs::set_permissions(&action_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Probe script
    let probe_script = "#!/bin/sh\necho 200\nexit 0\n";
    let probe_path = plugin_dir.join("probes/check.sh");
    std::fs::write(&probe_path, probe_script).unwrap();
    std::fs::set_permissions(&probe_path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Create an experiment that uses process providers (shell scripts).
fn create_experiment(action_script: &str, probe_script: &str) -> Experiment {
    Experiment {
        version: "v1".into(),
        title: "E2E script plugin test".into(),
        description: Some("Validates full lifecycle with script-based activities".into()),
        tags: vec!["e2e".into(), "script".into()],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: Some(Hypothesis {
            title: "Probe returns 200".into(),
            probes: vec![Activity {
                name: "health-probe".into(),
                activity_type: ActivityType::Probe,
                provider: Provider::Process {
                    path: probe_script.into(),
                    arguments: vec![],
                    env: HashMap::new(),
                    timeout_s: Some(5.0),
                },
                tolerance: Some(Tolerance::Exact {
                    value: serde_json::Value::Number(200.into()),
                }),
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            }],
        }),
        method: vec![Activity {
            name: "inject-fault".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: action_script.into(),
                arguments: vec![],
                env: HashMap::new(),
                timeout_s: Some(10.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        rollbacks: vec![Activity {
            name: "cleanup".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec!["cleaned up".into()],
                env: HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        estimate: Some(Estimate {
            expected_outcome: ExpectedOutcome::Recovered,
            expected_recovery_s: Some(5.0),
            expected_degradation: Some(DegradationLevel::Minor),
            expected_data_loss: Some(false),
            confidence: Some(Confidence::High),
            rationale: Some("Script-based test".into()),
            prior_runs: Some(1),
        }),
        baseline: None,
        load: None,
        regulatory: None,
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_script_plugin_produces_complete_journal() {
    let dir = TempDir::new().unwrap();
    setup_script_plugin(dir.path());

    let action_script = dir
        .path()
        .join("plugins/test-chaos/actions/inject.sh")
        .to_str()
        .unwrap()
        .to_string();
    let probe_script = dir
        .path()
        .join("plugins/test-chaos/probes/check.sh")
        .to_str()
        .unwrap()
        .to_string();

    let experiment = create_experiment(&action_script, &probe_script);

    // Write experiment to TOON file
    let exp_path = dir.path().join("experiment.toon");
    let toon = toon_format::encode_default(&experiment).unwrap();
    std::fs::write(&exp_path, &toon).unwrap();

    // Run through the engine directly (not CLI binary, but same code path)
    let journal_path = dir.path().join("journal.toon");

    // Use the CLI's ProviderExecutor
    let executor: Arc<dyn ActivityExecutor> =
        Arc::new(tumult_cli::commands::ProviderExecutor::new());
    let controls = Arc::new(ControlRegistry::new());
    let config = RunConfig::default();

    let journal = run_experiment(&experiment, &executor, &controls, &config).unwrap();

    // Write journal
    tumult_core::journal::write_journal(&journal, &journal_path).unwrap();

    // ── Verify journal completeness ───────────────────────────

    // Status: scripts return "200" which matches tolerance → completed
    assert_eq!(journal.status, ExperimentStatus::Completed);

    // Experiment metadata
    assert_eq!(journal.experiment_title, "E2E script plugin test");
    assert!(!journal.experiment_id.is_empty());
    assert!(journal.started_at_ns > 0);
    assert!(journal.ended_at_ns >= journal.started_at_ns);

    // Hypothesis before: probe returns "200", tolerance matches
    assert!(journal.steady_state_before.is_some());
    let hyp_before = journal.steady_state_before.as_ref().unwrap();
    assert!(hyp_before.met);
    assert_eq!(hyp_before.probe_results.len(), 1);
    assert_eq!(
        hyp_before.probe_results[0].status,
        ActivityStatus::Succeeded
    );

    // Method: action script ran successfully
    assert_eq!(journal.method_results.len(), 1);
    assert_eq!(journal.method_results[0].name, "inject-fault");
    assert_eq!(journal.method_results[0].status, ActivityStatus::Succeeded);
    assert_eq!(
        journal.method_results[0].output.as_deref(),
        Some("injected")
    );

    // Hypothesis after: still passes
    assert!(journal.steady_state_after.is_some());
    assert!(journal.steady_state_after.as_ref().unwrap().met);

    // Estimate preserved
    assert!(journal.estimate.is_some());
    assert_eq!(
        journal.estimate.as_ref().unwrap().expected_outcome,
        ExpectedOutcome::Recovered
    );

    // Analysis computed (estimate present → analysis present)
    assert!(journal.analysis.is_some());
    assert_eq!(
        journal.analysis.as_ref().unwrap().estimate_accuracy,
        Some(1.0)
    );

    // ── Verify journal file round-trips ───────────────────────

    assert!(journal_path.exists());
    let loaded = tumult_core::journal::read_journal(&journal_path).unwrap();
    assert_eq!(loaded.experiment_title, journal.experiment_title);
    assert_eq!(loaded.status, journal.status);
    assert_eq!(loaded.method_results.len(), journal.method_results.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_failing_script_marks_failed() {
    let dir = TempDir::new().unwrap();

    // Create a script that fails
    let fail_script = dir.path().join("fail.sh");
    std::fs::write(&fail_script, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&fail_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let probe_script = dir.path().join("probe.sh");
    std::fs::write(&probe_script, "#!/bin/sh\necho 200\nexit 0\n").unwrap();
    std::fs::set_permissions(&probe_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let experiment = create_experiment(
        fail_script.to_str().unwrap(),
        probe_script.to_str().unwrap(),
    );

    let executor: Arc<dyn ActivityExecutor> =
        Arc::new(tumult_cli::commands::ProviderExecutor::new());
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&experiment, &executor, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert_eq!(journal.method_results[0].status, ActivityStatus::Failed);
}

// ── type: script provider dispatch ────────────────────────────

/// Serializes the env-mutating tests in this binary: `TUMULT_PLUGIN_PATH`
/// is process-global, and tests run on parallel threads.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Restore an env var to its prior value on drop (test env hygiene).
struct EnvGuard(&'static str, Option<String>);

impl EnvGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let prior = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self(key, prior)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.1 {
            Some(value) => std::env::set_var(self.0, value),
            None => std::env::remove_var(self.0),
        }
    }
}

/// Script plugin fixture: an action that echoes its `TUMULT_BROKER_ID`
/// argument (proving args → TUMULT_* env propagation), a cleanup action,
/// and a probe that echoes "200".
fn setup_script_provider_plugin(dir: &std::path::Path) {
    let plugin_dir = dir.join("plugins").join("test-chaos");
    std::fs::create_dir_all(plugin_dir.join("actions")).unwrap();
    std::fs::create_dir_all(plugin_dir.join("probes")).unwrap();

    let manifest = "\
name: test-chaos
version: 0.1.0
description: Test chaos plugin

actions[2]:
  - name: inject
    script: actions/inject.sh
    description: Inject fault
  - name: cleanup
    script: actions/cleanup.sh
    description: Clean up

probes[1]:
  - name: check
    script: probes/check.sh
    description: Check health
";
    std::fs::write(plugin_dir.join("plugin.toon"), manifest).unwrap();

    let scripts = [
        (
            "actions/inject.sh",
            "#!/bin/sh\necho \"injected ${TUMULT_BROKER_ID}\"\n",
        ),
        ("actions/cleanup.sh", "#!/bin/sh\necho cleaned\n"),
        ("probes/check.sh", "#!/bin/sh\necho 200\n"),
    ];
    for (name, content) in scripts {
        let path = plugin_dir.join(name);
        std::fs::write(&path, content).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// An experiment driven entirely by `type: script` providers: hypothesis
/// probe, method action (with an argument), and rollback.
fn script_provider_experiment() -> Experiment {
    let script_provider = |function: &str, arguments| Provider::Script {
        plugin: "test-chaos".into(),
        function: function.into(),
        arguments,
        timeout_s: Some(5.0),
    };
    Experiment {
        version: "v1".into(),
        title: "E2E script provider dispatch".into(),
        steady_state_hypothesis: Some(Hypothesis {
            title: "Probe returns 200".into(),
            probes: vec![Activity {
                name: "health-probe".into(),
                activity_type: ActivityType::Probe,
                provider: script_provider("check", HashMap::new()),
                tolerance: Some(Tolerance::Exact {
                    value: serde_json::Value::Number(200.into()),
                }),
                ..Default::default()
            }],
        }),
        method: vec![Activity {
            name: "inject-fault".into(),
            activity_type: ActivityType::Action,
            provider: script_provider(
                "inject",
                HashMap::from([(
                    "broker_id".to_string(),
                    serde_json::Value::String("42".into()),
                )]),
            ),
            ..Default::default()
        }],
        rollbacks: vec![Activity {
            name: "cleanup".into(),
            activity_type: ActivityType::Action,
            provider: script_provider("cleanup", HashMap::new()),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_script_provider_dispatches_through_discovery() {
    let _lock = ENV_LOCK.lock().await;
    let dir = TempDir::new().unwrap();
    setup_script_provider_plugin(dir.path());
    // Point discovery at the fixture plugin dir (TUMULT_PLUGIN_PATH is one of
    // the default search paths).
    let _guard = EnvGuard::set("TUMULT_PLUGIN_PATH", &dir.path().join("plugins"));

    let experiment = script_provider_experiment();
    tumult_core::engine::validate_experiment(&experiment).unwrap();

    let executor: Arc<dyn ActivityExecutor> =
        Arc::new(tumult_cli::commands::ProviderExecutor::new());
    let controls = Arc::new(ControlRegistry::new());
    // RollbackStrategy::Always: a clean run still exercises rollback dispatch.
    let config = RunConfig {
        rollback_strategy: tumult_core::execution::RollbackStrategy::Always,
        ..RunConfig::default()
    };

    let journal = run_experiment(&experiment, &executor, &controls, &config).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    // The action ran the manifest-declared script, with the experiment
    // argument exported as TUMULT_BROKER_ID.
    assert_eq!(journal.method_results.len(), 1);
    assert_eq!(journal.method_results[0].status, ActivityStatus::Succeeded);
    assert_eq!(
        journal.method_results[0].output.as_deref(),
        Some("injected 42")
    );
    // The script probe satisfied its tolerance.
    assert!(journal.steady_state_before.as_ref().unwrap().met);
    // Rollback executed through the same dispatch.
    assert_eq!(journal.rollback_results.len(), 1);
    assert_eq!(
        journal.rollback_results[0].status,
        ActivityStatus::Succeeded
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_script_provider_unknown_plugin_and_action_name_available() {
    let _lock = ENV_LOCK.lock().await;
    let dir = TempDir::new().unwrap();
    setup_script_provider_plugin(dir.path());
    let _guard = EnvGuard::set("TUMULT_PLUGIN_PATH", &dir.path().join("plugins"));

    let executor = tumult_cli::commands::ProviderExecutor::new();

    let mut activity = Activity {
        name: "inject".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Script {
            plugin: "no-such-plugin".into(),
            function: "inject".into(),
            arguments: HashMap::new(),
            timeout_s: None,
        },
        ..Default::default()
    };
    let outcome = executor.execute(&activity);
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("unknown script plugin: no-such-plugin"),
        "{error}"
    );
    assert!(
        error.contains("test-chaos"),
        "error should list available: {error}"
    );

    activity.provider = Provider::Script {
        plugin: "test-chaos".into(),
        function: "no-such-action".into(),
        arguments: HashMap::new(),
        timeout_s: None,
    };
    let outcome = executor.execute(&activity);
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("unknown test-chaos action: no-such-action"),
        "{error}"
    );
    assert!(
        error.contains("inject"),
        "error should list available: {error}"
    );
}
