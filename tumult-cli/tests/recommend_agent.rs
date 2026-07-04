//! Hermetic tests for `tumult recommend --agent` and `tumult agents`,
//! driving a fake `claude` shell script via `CLAUDE_CODE_BIN` — no real
//! agent CLI, no network. Mirrors the env-mutex + fake-binary pattern of
//! `tumult-agent-cli/tests/adapters.rs`.
#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tumult_cli::commands::{cmd_agents, cmd_recommend, AgentArgs};
use tumult_core::engine::{parse_experiment, validate_experiment};
use tumult_intelligence::{OutputFormat, RecommendOptions};

/// Serializes tests that mutate process-global env vars.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Restores an env var to its previous state on drop.
struct EnvGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = env::var_os(key);
        env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => env::set_var(self.key, prev),
            None => env::remove_var(self.key),
        }
    }
}

/// Write an executable `#!/bin/sh` script into `dir`.
fn fake_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake binary");
    let mut perms = fs::metadata(&path).expect("stat fake binary").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod fake binary");
    path
}

const VALID_TOON: &str = r#"title: Process retry validation
description: Verify process execution path stays healthy

method[1]:
  - name: exercise-process-provider
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: "chaos"
      timeout_s: 5.0

rollbacks[0]:
"#;

const INVALID_TOON: &str = "title: missing method\nmethod[0]:\nrollbacks[0]:\n";

/// Canned agent response: recommendations, one valid `toon` fence, one
/// invalid (empty-method) `toon` fence.
fn canned_response() -> String {
    format!(
        "## Recommendations\n1. Exercise the process provider first — it is untested.\n\n\
         ```toon\n{VALID_TOON}```\n\n```toon\n{INVALID_TOON}```\n"
    )
}

/// Install a fake `claude` that answers the version probe and emits a JSON
/// result envelope whose `result` is the canned agent response.
fn install_fake_claude(dir: &Path) -> (PathBuf, EnvGuard) {
    let envelope = serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": canned_response(),
        "session_id": "5b1e9a2f-0000-4000-8000-00000000abcd",
    })
    .to_string();
    let fixture = dir.join("envelope.json");
    fs::write(&fixture, envelope).expect("write fixture");
    let body = format!(
        r#"if [ "$1" = "--version" ]; then echo "2.0.13 (Claude Code)"; exit 0; fi
cat > /dev/null
cat "{fixture}""#,
        fixture = fixture.display(),
    );
    let bin = fake_bin(dir, "claude", &body);
    let guard = EnvGuard::set("CLAUDE_CODE_BIN", &bin);
    (bin, guard)
}

fn recommend_options(dir: &Path, format: OutputFormat) -> RecommendOptions {
    let mut options = RecommendOptions::new(dir.join("no-store-here.duckdb"));
    options.format = format;
    options
}

fn agent_args(generate_dir: Option<PathBuf>) -> AgentArgs {
    AgentArgs {
        agent: "claude-code".to_string(),
        model: None,
        timeout_secs: 10,
        generate_dir,
    }
}

#[test]
fn recommend_agent_text_end_to_end_writes_valid_and_rejects_invalid() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let (_bin, _guard) = install_fake_claude(dir.path());
    let out_dir = dir.path().join("generated");

    let output = cmd_recommend(
        &recommend_options(dir.path(), OutputFormat::Text),
        Some(&agent_args(Some(out_dir.clone()))),
    )
    .expect("agent-enhanced recommend succeeds");

    // Heuristic section, then the agent-enhanced section.
    assert!(output.contains("=== Recommendations ===") || output.contains("AI-Powered"));
    assert!(
        output.contains("=== Agent-enhanced recommendations (claude-code) ==="),
        "missing agent section: {output}"
    );
    assert!(output.contains("Exercise the process provider first"));

    // Valid experiment written and passes validation end to end.
    let written = out_dir.join("process-retry-validation.toon");
    assert!(
        output.contains(&format!("Wrote {}", written.display())),
        "missing written path: {output}"
    );
    let content = fs::read_to_string(&written).expect("written experiment readable");
    let experiment = parse_experiment(&content).expect("written experiment parses");
    validate_experiment(&experiment).expect("written experiment validates");

    // Invalid experiment rejected, not written, counted honestly.
    assert!(output.contains("Rejected experiment:"), "{output}");
    assert!(
        output.contains("1 experiment(s) written, 1 rejected (validation failed)"),
        "missing summary: {output}"
    );
    assert_eq!(
        fs::read_dir(&out_dir).expect("out dir listable").count(),
        1,
        "only the valid experiment may be written"
    );
}

#[test]
fn recommend_agent_does_not_overwrite_on_slug_collision() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let (_bin, _guard) = install_fake_claude(dir.path());
    let out_dir = dir.path().join("generated");
    fs::create_dir_all(&out_dir).expect("create out dir");
    let existing = out_dir.join("process-retry-validation.toon");
    fs::write(&existing, "pre-existing content").expect("seed collision file");

    let output = cmd_recommend(
        &recommend_options(dir.path(), OutputFormat::Text),
        Some(&agent_args(Some(out_dir.clone()))),
    )
    .expect("recommend succeeds");

    assert_eq!(
        fs::read_to_string(&existing).expect("existing file intact"),
        "pre-existing content",
        "collision must not overwrite"
    );
    let renamed = out_dir.join("process-retry-validation-2.toon");
    assert!(renamed.exists(), "collision must append -2: {output}");
}

#[test]
fn recommend_agent_json_includes_agent_object() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let (_bin, _guard) = install_fake_claude(dir.path());
    let out_dir = dir.path().join("generated");

    let output = cmd_recommend(
        &recommend_options(dir.path(), OutputFormat::Json),
        Some(&agent_args(Some(out_dir.clone()))),
    )
    .expect("recommend succeeds");

    let value: serde_json::Value = serde_json::from_str(&output).expect("valid JSON output");
    assert_eq!(value["source"], "heuristic-fallback");
    let agent = &value["agent"];
    assert_eq!(agent["adapter"], "claude-code");
    assert!(agent["model"].is_null());
    assert!(
        agent["recommendations"]
            .as_str()
            .expect("recommendations string")
            .contains("Exercise the process provider first"),
        "agent recommendations missing"
    );
    let written = agent["experiments_written"]
        .as_array()
        .expect("written array");
    assert_eq!(written.len(), 1);
    assert!(written[0]
        .as_str()
        .expect("path string")
        .ends_with("process-retry-validation.toon"));
    let rejected = agent["experiments_rejected"]
        .as_array()
        .expect("rejected array");
    assert_eq!(rejected.len(), 1);
    assert!(
        !rejected[0]["error"]
            .as_str()
            .expect("error string")
            .is_empty(),
        "rejection must carry the validation error"
    );
}

#[test]
fn recommend_agent_without_generate_dir_writes_nothing() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let (_bin, _guard) = install_fake_claude(dir.path());

    let output = cmd_recommend(
        &recommend_options(dir.path(), OutputFormat::Text),
        Some(&agent_args(None)),
    )
    .expect("recommend succeeds");

    assert!(output.contains("=== Agent-enhanced recommendations (claude-code) ==="));
    assert!(
        !output.contains("experiment(s) written"),
        "no summary line without --generate-experiments: {output}"
    );
    assert!(!dir.path().join("generated").exists());
}

#[test]
fn recommend_unknown_agent_lists_available_adapters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut args = agent_args(None);
    args.agent = "cursor".to_string();

    let err = cmd_recommend(
        &recommend_options(dir.path(), OutputFormat::Text),
        Some(&args),
    )
    .expect_err("unknown adapter must error");

    let message = err.to_string();
    assert!(message.contains("unknown adapter 'cursor'"), "{message}");
    assert!(message.contains("claude-code, codex"), "{message}");
}

#[test]
fn agents_table_reports_installed_and_missing_adapters() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let claude = fake_bin(
        dir.path(),
        "claude",
        r#"if [ "$1" = "--version" ]; then echo "2.0.13 (Claude Code)"; exit 0; fi"#,
    );
    let _claude = EnvGuard::set("CLAUDE_CODE_BIN", &claude);
    let _codex = EnvGuard::set("CODEX_BIN", dir.path().join("does-not-exist"));
    let _path = EnvGuard::set("PATH", dir.path()); // PATH fallback finds nothing

    let output = cmd_agents();

    assert!(output.contains("ADAPTER"), "{output}");
    assert!(output.contains("INSTALLED"), "{output}");
    assert!(output.contains("VERSION"), "{output}");
    let claude_line = output
        .lines()
        .find(|line| line.starts_with("claude-code"))
        .expect("claude-code row");
    assert!(claude_line.contains("yes"), "{claude_line}");
    assert!(claude_line.contains("2.0.13"), "{claude_line}");
    let codex_line = output
        .lines()
        .find(|line| line.starts_with("codex"))
        .expect("codex row");
    assert!(codex_line.contains("no"), "{codex_line}");
    assert!(
        codex_line.contains("npm i -g @openai/codex"),
        "missing adapter must show install hint: {codex_line}"
    );
}
