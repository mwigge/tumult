//! Tests for `tumult validate` beyond the GameDay-hint cases: the happy-path
//! summary rendering, the file-size guard, unknown-plugin warnings, the
//! symlink guard, and `tumult discover` filtering.

use super::super::*;
use super::helpers::*;
use tempfile::TempDir;
use tumult_core::types::{Activity, ActivityType, Experiment, Provider};

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
    // Env vars are process-global: pin the secret's env var to unset so the
    // "Secrets: WARNING" branch deterministically renders.
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(SECRET_ENV_VAR);
    let dir = TempDir::new().unwrap();
    let path = write_config_secrets_experiment(dir.path());

    cmd_validate(&path).unwrap();
}

#[test]
fn validate_warns_but_passes_on_unknown_native_plugin() {
    let dir = TempDir::new().unwrap();
    let experiment = Experiment {
        title: "unknown native plugin".into(),
        method: vec![Activity {
            name: "mystery".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "no-such-native-plugin".into(),
                function: "do_thing".into(),
                arguments: std::collections::HashMap::new(),
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let path = dir.path().join("exp.toon");
    std::fs::write(&path, toon_format::encode_default(&experiment).unwrap()).unwrap();

    // Unknown plugin refs are warnings on stderr, not validation failures.
    cmd_validate(&path).unwrap();
}

#[test]
fn validate_warns_on_unknown_function_of_known_native_plugin() {
    let Some(plugin) = native_registry()
        .plugin_names()
        .first()
        .map(|s| (*s).to_string())
    else {
        // No native plugins registered in this build — nothing to exercise.
        return;
    };
    let dir = TempDir::new().unwrap();
    let experiment = Experiment {
        title: "unknown native function".into(),
        method: vec![Activity {
            name: "mystery".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin,
                function: "no_such_function".into(),
                arguments: std::collections::HashMap::new(),
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let path = dir.path().join("exp.toon");
    std::fs::write(&path, toon_format::encode_default(&experiment).unwrap()).unwrap();

    cmd_validate(&path).unwrap();
}

#[test]
fn validate_warns_but_passes_on_unknown_script_plugin() {
    let dir = TempDir::new().unwrap();
    let experiment = Experiment {
        title: "unknown script plugin".into(),
        method: vec![Activity {
            name: "mystery".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Script {
                plugin: "no-such-script-plugin".into(),
                function: "do_thing".into(),
                arguments: std::collections::HashMap::new(),
                timeout_s: None,
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let path = dir.path().join("exp.toon");
    std::fs::write(&path, toon_format::encode_default(&experiment).unwrap()).unwrap();

    cmd_validate(&path).unwrap();
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
