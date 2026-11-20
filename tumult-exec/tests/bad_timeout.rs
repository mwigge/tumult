//! A step whose timeout is not a number of seconds, in a run the runner
//! drives with the real executor. The step is a real script plugin in a
//! temp dir. The run does not validate the experiment first, as a caller
//! of the runner may not: the step must fail with its reason, the run must
//! end with a journal, and a guard must not be switched off.
#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, RunConfig, SamplingConfig,
};
use tumult_core::types::{
    Activity, ActivityStatus, ActivityType, Experiment, ExperimentStatus, Guard, Hypothesis,
    Journal, Provider, Tolerance,
};
use tumult_exec::ProviderExecutor;

const PLUGIN: &str = "timeout-plugin";

/// A plugin in a temp dir whose probe answers `ok`. The plugin search path
/// and the home directory point at the temp dir while the guard lives.
struct Plugin {
    _dir: tempfile::TempDir,
    _env: tumult_test_utils::EnvGuard,
}

impl Plugin {
    fn new() -> Self {
        let mut env = tumult_test_utils::EnvGuard::lock();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let root = dir.path().join(PLUGIN);
        std::fs::create_dir_all(root.join("probes")).expect("plugin dir");
        std::fs::write(
            root.join("plugin.toon"),
            "name: timeout-plugin\n\
             version: 0.1.0\n\
             description: Plugin whose probe answers ok\n\
             actions[0]:\n\
             probes[1]:\n\
             \x20 - name: health\n\
             \x20   script: probes/health.sh\n\
             \x20   description: Answer ok\n",
        )
        .expect("manifest");
        std::fs::write(root.join("probes/health.sh"), "echo ok\n").expect("script");
        env.set("TUMULT_PLUGIN_PATH", dir.path());
        env.set("HOME", dir.path());
        Self {
            _dir: dir,
            _env: env,
        }
    }
}

/// A step that runs the probe of the plugin with `timeout_s` as its
/// timeout. As a probe it holds when the script answers `ok`.
fn script_step(name: &str, activity_type: ActivityType, timeout_s: f64) -> Activity {
    Activity {
        name: name.into(),
        activity_type,
        provider: Provider::Script {
            plugin: PLUGIN.into(),
            function: "health".into(),
            arguments: HashMap::new(),
            timeout_s: Some(timeout_s),
        },
        tolerance: Some(Tolerance::Regex {
            pattern: "^ok$".into(),
        }),
        ..Activity::default()
    }
}

/// A method step that does nothing and succeeds.
fn quiet_step() -> Activity {
    Activity {
        name: "quiet".into(),
        provider: Provider::Process {
            path: "true".into(),
            arguments: vec![],
            env: HashMap::new(),
            timeout_s: Some(10.0),
        },
        ..Activity::default()
    }
}

/// Run `experiment` with the real executor, without validating it first.
fn run(experiment: &Experiment) -> Journal {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(ProviderExecutor::new());
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(200),
    };
    run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
        &sampling,
    )
    .expect("run")
}

#[test]
fn a_method_step_with_a_negative_timeout_fails_with_its_reason_and_does_not_panic() {
    let _plugin = Plugin::new();
    let experiment = Experiment {
        title: "bad timeout on a method step".into(),
        method: vec![script_step("inject", ActivityType::Action, -1.0)],
        ..Experiment::default()
    };

    let journal = run(&experiment);

    let step = &journal.method_results[0];
    assert_eq!(step.status, ActivityStatus::Failed);
    let error = step.error.as_deref().unwrap_or_default();
    assert!(error.starts_with("invalid timeout"), "{error}");
}

#[test]
fn a_steady_state_probe_with_a_negative_timeout_fails_the_hypothesis_and_the_run_has_a_journal() {
    let _plugin = Plugin::new();
    let experiment = Experiment {
        title: "bad timeout on a steady state probe".into(),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target answers ok".into(),
            probes: vec![script_step("steady", ActivityType::Probe, -1.0)],
        }),
        method: vec![quiet_step()],
        ..Experiment::default()
    };

    let journal = run(&experiment);

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    let before = journal.steady_state_before.expect("the hypothesis ran");
    assert!(!before.met);
    let error = before.probe_results[0].error.as_deref().unwrap_or_default();
    assert!(error.starts_with("invalid timeout"), "{error}");
}

#[test]
fn a_guard_whose_probe_has_a_negative_timeout_halts_the_run() {
    let _plugin = Plugin::new();
    let experiment = Experiment {
        title: "bad timeout on a guard probe".into(),
        guards: vec![Guard {
            name: "safety".into(),
            probe: script_step("guard-probe", ActivityType::Probe, -1.0),
            min_breaches: 1,
        }],
        method: vec![quiet_step()],
        ..Experiment::default()
    };

    let journal = run(&experiment);

    assert_eq!(
        journal.status,
        ExperimentStatus::Halted,
        "a guard that cannot run its probe let the run go on"
    );
    assert_eq!(journal.halt.expect("a halt record").guard_name, "safety");
}
