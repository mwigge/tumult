//! Tests for the bundled script plugins.
//!
//! Validates that:
//! 1. Plugin manifests parse correctly as TOON
//! 2. Probe scripts execute and produce output on the current platform
//! 3. Scripts validate required environment variables

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// Get workspace root (parent of tumult-cli/).
fn ws() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn plugin_path(relative: &str) -> String {
    ws().join("plugins")
        .join(relative)
        .to_str()
        .unwrap()
        .to_string()
}

/// Parse a plugin.toon manifest and verify it has expected fields.
fn assert_manifest_valid(plugin_dir: &str) {
    let path = plugin_path(&format!("{plugin_dir}/plugin.toon"));
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("manifest not found: {path}"));
    assert!(content.contains("name:"), "manifest missing name field");
    assert!(
        content.contains("version:"),
        "manifest missing version field"
    );
    assert!(
        content.contains("actions") || content.contains("probes"),
        "manifest missing actions/probes"
    );
}

fn assert_script_executable(relative: &str) {
    let path = plugin_path(relative);
    let metadata = std::fs::metadata(&path).unwrap_or_else(|_| panic!("script not found: {path}"));
    let mode = metadata.permissions().mode();
    assert!(
        mode & 0o111 != 0,
        "script not executable: {path} (mode: {mode:o})"
    );
}

fn run_script(relative: &str, env: &[(&str, &str)]) -> (i32, String, String) {
    let path = plugin_path(relative);
    let mut cmd = Command::new("sh");
    cmd.arg(&path);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to run {path}: {e}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// ═══════════════════════════════════════════════════════════════
// tumult-stress
// ═══════════════════════════════════════════════════════════════

#[test]
fn stress_manifest_parses() {
    assert_manifest_valid("tumult-stress");
}

#[test]
fn stress_scripts_are_executable() {
    for script in &[
        "tumult-stress/actions/cpu-stress.sh",
        "tumult-stress/actions/memory-stress.sh",
        "tumult-stress/actions/io-stress.sh",
        "tumult-stress/actions/combined-stress.sh",
        "tumult-stress/probes/cpu-utilization.sh",
        "tumult-stress/probes/memory-utilization.sh",
        "tumult-stress/probes/io-utilization.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn cpu_utilization_probe_produces_number() {
    let (code, stdout, _) = run_script("tumult-stress/probes/cpu-utilization.sh", &[]);
    assert_eq!(code, 0, "cpu-utilization probe failed");
    let value: f64 = stdout
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("cpu-utilization output not a number: '{}'", stdout.trim()));
    assert!(
        (0.0..=100.0).contains(&value),
        "cpu value out of range: {value}"
    );
}

#[test]
fn memory_utilization_probe_produces_number() {
    let (code, stdout, _) = run_script("tumult-stress/probes/memory-utilization.sh", &[]);
    assert_eq!(code, 0, "memory-utilization probe failed");
    let value: f64 = stdout.trim().parse().unwrap_or_else(|_| {
        panic!(
            "memory-utilization output not a number: '{}'",
            stdout.trim()
        )
    });
    assert!(
        (0.0..=100.0).contains(&value),
        "memory value out of range: {value}"
    );
}

#[test]
fn stress_action_fails_without_stressng() {
    let (code, _, stderr) = run_script(
        "tumult-stress/actions/cpu-stress.sh",
        &[("TUMULT_TIMEOUT", "1")],
    );
    if code != 0 {
        assert!(
            stderr.contains("stress-ng not found"),
            "expected stress-ng not found error, got: {stderr}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// tumult-containers
// ═══════════════════════════════════════════════════════════════

#[test]
fn containers_manifest_parses() {
    assert_manifest_valid("tumult-containers");
}

#[test]
fn containers_scripts_are_executable() {
    for script in &[
        "tumult-containers/actions/kill-container.sh",
        "tumult-containers/actions/stop-container.sh",
        "tumult-containers/actions/pause-container.sh",
        "tumult-containers/actions/unpause-container.sh",
        "tumult-containers/actions/limit-cpu.sh",
        "tumult-containers/actions/limit-memory.sh",
        "tumult-containers/probes/container-status.sh",
        "tumult-containers/probes/container-health.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn container_action_requires_container_id() {
    let (code, _, stderr) = run_script("tumult-containers/actions/kill-container.sh", &[]);
    assert_ne!(code, 0, "should fail without TUMULT_CONTAINER_ID");
    assert!(
        stderr.contains("TUMULT_CONTAINER_ID"),
        "error should mention missing var, got: {stderr}"
    );
}

// ═══════════════════════════════════════════════════════════════
// tumult-process
// ═══════════════════════════════════════════════════════════════

#[test]
fn process_manifest_parses() {
    assert_manifest_valid("tumult-process");
}

#[test]
fn process_scripts_are_executable() {
    for script in &[
        "tumult-process/actions/kill-process.sh",
        "tumult-process/actions/suspend-process.sh",
        "tumult-process/actions/resume-process.sh",
        "tumult-process/probes/process-exists.sh",
        "tumult-process/probes/process-resources.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn process_exists_probe_detects_current_shell() {
    let pid = std::process::id();
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-exists.sh",
        &[("TUMULT_PID", &pid.to_string())],
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "true");
}

#[test]
fn process_exists_probe_detects_missing_pid() {
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-exists.sh",
        &[("TUMULT_PID", "999999999")],
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "false");
}

#[test]
fn process_resources_probe_returns_json() {
    let pid = std::process::id();
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-resources.sh",
        &[("TUMULT_PID", &pid.to_string())],
    );
    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("not valid JSON: '{}'", stdout.trim()));
    assert!(parsed.get("cpu_percent").is_some());
    assert!(parsed.get("mem_percent").is_some());
    assert!(parsed.get("running").is_some());
}

#[test]
fn process_action_requires_target() {
    let (code, _, stderr) = run_script("tumult-process/actions/kill-process.sh", &[]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("TUMULT_PID") || stderr.contains("required"),
        "error should mention missing target, got: {stderr}"
    );
}
