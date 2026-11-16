//! Tests for `tumult validate` beyond the GameDay-hint cases: the happy-path
//! summary rendering, the file-size guard, unknown plugin references, the
//! symlink guard, and `tumult discover` filtering.

use super::super::*;
use super::helpers::*;
use crate::output::testing::capture;
use tempfile::TempDir;
use tumult_core::types::{Activity, ActivityType, Control, Experiment, Guard, Provider, Tolerance};

#[test]
fn validate_rejects_oversized_experiment_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("huge.toon");
    // Just over the 10MB limit enforced before parsing.
    std::fs::write(&path, "x".repeat(11 * 1024 * 1024)).unwrap();

    let err = cmd_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("experiment file too large"),
        "{err}"
    );
}

#[test]
fn validate_renders_summary_for_full_featured_experiment() {
    // Without the secret's variable the secrets line is the warning.
    let mut env = tumult_test_utils::EnvGuard::lock();
    env.remove(SECRET_ENV_VAR);
    let dir = TempDir::new().unwrap();
    let path = write_config_secrets_experiment(dir.path());

    let (result, printed) = capture(|| cmd_validate(&path));

    result.unwrap();
    let out = printed.stdout;
    assert!(
        out.starts_with("Experiment: config/secrets ${config.greeting}\n"),
        "{out}"
    );
    assert!(out.contains("\nTags: test\n"), "{out}");
    assert!(out.contains("\nMethod steps: 3\n"), "{out}");
    assert!(out.contains("\nRollback steps: 0\n"), "{out}");
    assert!(
        out.contains("\nConfiguration: all values resolved\n"),
        "{out}"
    );
    assert!(out.contains("\nSecrets: WARNING — "), "{out}");
    assert!(out.ends_with("\n\nValidation passed.\n"), "{out}");
}

fn native(plugin: &str, function: &str) -> Provider {
    Provider::Native {
        plugin: plugin.into(),
        function: function.into(),
        arguments: std::collections::HashMap::new(),
    }
}

fn step(name: &str, provider: Provider) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Action,
        provider,
        ..Default::default()
    }
}

/// An experiment whose only method step, `mystery`, runs through `provider`.
fn with_step(provider: Provider) -> Experiment {
    Experiment {
        title: "names something this binary may not have".into(),
        method: vec![step("mystery", provider)],
        ..Default::default()
    }
}

/// Validate `experiment` from a file: the outcome and what went to stdout.
fn validated(experiment: &Experiment) -> (anyhow::Result<()>, String) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("exp.toon");
    std::fs::write(&path, toon_format::encode_default(experiment).unwrap()).unwrap();
    let (result, printed) = capture(|| cmd_validate(&path));
    (result, printed.stdout)
}

/// The error of an experiment that must not validate.
fn refusal(experiment: &Experiment) -> String {
    let (result, stdout) = validated(experiment);
    assert!(!stdout.contains("Validation passed."), "{stdout}");
    format!("{:#}", result.unwrap_err())
}

fn a_native_plugin() -> String {
    native_registry()
        .plugin_names()
        .first()
        .expect("at least one native plugin compiled in")
        .to_string()
}

#[test]
fn validate_refuses_an_unknown_native_plugin() {
    let error = refusal(&with_step(native("no-such-native-plugin", "do_thing")));

    assert!(
        error.contains(
            "activity 'mystery' uses unknown native plugin 'no-such-native-plugin' (available: "
        ),
        "{error}"
    );
}

#[test]
fn validate_refuses_an_unknown_function_of_a_known_native_plugin() {
    let plugin = a_native_plugin();

    let error = refusal(&with_step(native(&plugin, "no_such_function")));

    let expected = format!(
        "activity 'mystery' uses unknown function '{plugin}::no_such_function' (available: "
    );
    assert!(error.contains(&expected), "{error}");
    assert!(!error.contains("unknown native plugin"), "{error}");
}

#[test]
fn validate_accepts_a_function_the_binary_has() {
    let plugin = a_native_plugin();
    let function = native_registry().get(&plugin).unwrap().functions()[0].to_string();

    let (result, stdout) = validated(&with_step(native(&plugin, &function)));

    result.unwrap();
    assert!(stdout.ends_with("Validation passed.\n"), "{stdout}");
}

#[test]
fn validate_refuses_an_unknown_script_plugin() {
    let error = refusal(&with_step(Provider::Script {
        plugin: "no-such-script-plugin".into(),
        function: "do_thing".into(),
        arguments: std::collections::HashMap::new(),
        timeout_s: None,
    }));

    assert!(
        error.contains(
            "activity 'mystery' uses unknown script plugin 'no-such-script-plugin' (available: "
        ),
        "{error}"
    );
}

#[test]
fn validate_checks_the_probe_of_a_guard() {
    let mut probe = step("errors", native("no-such-native-plugin", "count"));
    probe.activity_type = ActivityType::Probe;
    probe.tolerance = Some(Tolerance::Exact {
        value: serde_json::json!("ok"),
    });
    let mut experiment = with_step(Provider::Process {
        path: "echo".into(),
        arguments: vec![],
        env: std::collections::HashMap::new(),
        timeout_s: None,
    });
    experiment.guards = vec![Guard {
        name: "watch".into(),
        probe,
        min_breaches: 1,
    }];

    let error = refusal(&experiment);

    assert!(
        error.contains("guard 'watch' uses unknown native plugin 'no-such-native-plugin'"),
        "{error}"
    );
}

#[test]
fn validate_checks_the_provider_of_a_control() {
    let mut experiment = with_step(Provider::Process {
        path: "echo".into(),
        arguments: vec![],
        env: std::collections::HashMap::new(),
        timeout_s: None,
    });
    experiment.controls = vec![Control {
        name: "audit".into(),
        provider: native("no-such-native-plugin", "record"),
    }];

    let error = refusal(&experiment);

    assert!(
        error.contains("control 'audit' uses unknown native plugin 'no-such-native-plugin'"),
        "{error}"
    );
}

#[test]
fn validate_names_every_unknown_reference_in_one_error() {
    let mut experiment = with_step(native("no-such-native-plugin", "do_thing"));
    experiment
        .rollbacks
        .push(step("undo", native("another-missing-plugin", "undo_thing")));

    let error = refusal(&experiment);

    assert!(error.contains("2 steps"), "{error}");
    assert!(error.contains("activity 'mystery'"), "{error}");
    assert!(error.contains("activity 'undo'"), "{error}");
}

#[test]
fn validate_path_no_symlink_rejects_symlinks() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("real.toon");
    std::fs::write(&target, "title: x\n").unwrap();
    let link = dir.path().join("link.toon");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    validate_path_no_symlink(&target).unwrap();
    let err = validate_path_no_symlink(&link).unwrap_err();
    assert!(err.to_string().contains("symlink not allowed"), "{err}");
}

#[test]
fn discover_unknown_plugin_filter_errors() {
    let err = cmd_discover(Some("no-such-plugin")).unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}
