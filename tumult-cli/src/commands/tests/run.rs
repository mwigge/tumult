//! Tests for `cmd_run`, `cmd_validate`, `cmd_discover`, `cmd_init` and templates.

use super::super::*;
use super::helpers::*;
use tempfile::TempDir;
use tumult_core::execution::RollbackStrategy;

// ── cmd_run tests ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_valid_experiment_produces_journal() {
    let dir = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(dir.path());
    let journal_path = dir.path().join("journal.toon");

    let result = cmd_run(
        &exp_path,
        &journal_path,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await;

    assert!(result.is_ok());
    assert!(journal_path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_dry_run_does_not_create_journal() {
    let dir = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(dir.path());
    let journal_path = dir.path().join("journal.toon");

    let result = cmd_run(
        &exp_path,
        &journal_path,
        true,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await;

    assert!(result.is_ok());
    assert!(!journal_path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_nonexistent_file_returns_error() {
    let result = cmd_run(
        Path::new("/nonexistent/experiment.toon"),
        Path::new("journal.toon"),
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_invalid_toon_returns_error() {
    let dir = TempDir::new().unwrap();
    let exp_path = write_invalid_experiment(dir.path());

    let result = cmd_run(
        &exp_path,
        &dir.path().join("journal.toon"),
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_empty_method_returns_error() {
    let dir = TempDir::new().unwrap();
    let exp_path = write_empty_method_experiment(dir.path());

    let result = cmd_run(
        &exp_path,
        &dir.path().join("journal.toon"),
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await;
    assert!(result.is_err());
}

// ── cmd_validate tests ────────────────────────────────────

#[test]
fn validate_valid_experiment_succeeds() {
    let dir = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(dir.path());

    let result = cmd_validate(&exp_path);
    assert!(result.is_ok());
}

#[test]
fn validate_nonexistent_file_returns_error() {
    let result = cmd_validate(Path::new("/nonexistent/experiment.toon"));
    assert!(result.is_err());
}

#[test]
fn validate_invalid_toon_returns_error() {
    let dir = TempDir::new().unwrap();
    let exp_path = write_invalid_experiment(dir.path());

    let result = cmd_validate(&exp_path);
    assert!(result.is_err());
}

/// The `http` provider variant was removed; experiment files that still use
/// it must fail parsing with a comprehensible serde unknown-variant error
/// that names the offending provider type and lists the supported ones.
#[test]
fn validate_http_provider_returns_unknown_variant_error() {
    let dir = TempDir::new().unwrap();
    let exp_path = write_http_provider_experiment(dir.path());

    let err = cmd_validate(&exp_path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unknown variant `http`"),
        "error should name the unknown provider type: {msg}"
    );
    assert!(
        msg.contains("expected"),
        "error should list the supported provider types: {msg}"
    );
}

#[test]
fn validate_empty_method_returns_error() {
    let dir = TempDir::new().unwrap();
    let exp_path = write_empty_method_experiment(dir.path());

    let result = cmd_validate(&exp_path);
    assert!(result.is_err());
}

// ── cmd_discover tests ────────────────────────────────────

#[test]
fn discover_without_plugins_shows_empty() {
    // No plugins in default search paths during tests
    let result = cmd_discover(None);
    assert!(result.is_ok());
}

#[test]
fn discover_nonexistent_plugin_returns_error() {
    let result = cmd_discover(Some("nonexistent-plugin"));
    assert!(result.is_err());
}

#[test]
fn discover_lists_native_plugins_with_kind_and_functions() {
    let registry = tumult_plugin::registry::PluginRegistry::new();
    let output = render_discover(None, &registry, native_registry()).unwrap();

    assert!(output.contains("tumult-kubernetes (native)"));
    assert!(output.contains("tumult-net (native)"));
    assert!(output.contains("tumult-ssh (native)"));
    assert!(output.contains("tumult-ssh::execute"));
    assert!(output.contains("tumult-kubernetes::delete_pod"));
}

#[test]
fn discover_labels_script_and_native_kinds() {
    let mut registry = tumult_plugin::registry::PluginRegistry::new();
    registry.register_script(tumult_plugin::manifest::ScriptPluginManifest {
        name: "tumult-kafka".into(),
        version: "1.0.0".into(),
        description: "Kafka chaos".into(),
        actions: vec![tumult_plugin::manifest::ScriptAction {
            name: "broker-kill".into(),
            script: "actions/broker-kill.sh".into(),
            description: "Kill a broker".into(),
        }],
        probes: vec![],
    });

    let output = render_discover(None, &registry, native_registry()).unwrap();

    assert!(output.contains("(1 script, 5 native)"));
    assert!(output.contains("tumult-kafka (script)"));
    assert!(output.contains("tumult-kafka::broker-kill"));
}

#[test]
fn discover_filter_matches_native_plugin() {
    let registry = tumult_plugin::registry::PluginRegistry::new();
    let output = render_discover(Some("tumult-ssh"), &registry, native_registry()).unwrap();

    assert!(output.contains("Plugin: tumult-ssh (native)"));
    assert!(output.contains("- execute"));
}

// ── cmd_init tests ────────────────────────────────────────

#[test]
fn init_creates_experiment_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("experiment.toon");

    let result = init_at(&path, None);

    assert!(result.is_ok());
    assert!(path.exists());
}

#[test]
fn init_with_plugin_includes_plugin_name() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("experiment.toon");

    let result = init_at(&path, Some("tumult-kafka"));

    assert!(result.is_ok());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("tumult-kafka"));
}

#[test]
fn init_fails_if_file_exists() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("experiment.toon");
    std::fs::write(&path, "existing").unwrap();

    let result = init_at(&path, None);
    assert!(result.is_err());
}

// ── generate_template tests ───────────────────────────────

#[test]
fn template_contains_required_sections() {
    let template = generate_template(None);
    assert!(template.contains("title:"));
    assert!(template.contains("steady_state_hypothesis:"));
    assert!(template.contains("method"));
    assert!(template.contains("rollbacks"));
}

#[test]
fn template_uses_plugin_name() {
    let template = generate_template(Some("tumult-db"));
    assert!(template.contains("tumult-db"));
}
