//! Integration tests for tumult-db-postgres plugin scripts.
//!
//! Requires a local PostgreSQL instance accessible without password.
//! Skips gracefully if psql is not available or PostgreSQL is not running.

use std::process::Command;

fn ws() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn plugin_path(relative: &str) -> String {
    ws().join("plugins")
        .join("tumult-db-postgres")
        .join(relative)
        .to_str()
        .unwrap()
        .to_string()
}

fn pg_available() -> bool {
    Command::new("pg_isready")
        .arg("-h")
        .arg("localhost")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn pg_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "postgres".into())
}

fn run_script(relative: &str, extra_env: &[(&str, &str)]) -> (i32, String, String) {
    let path = plugin_path(relative);
    let mut cmd = Command::new("sh");
    cmd.arg(&path);
    cmd.env("TUMULT_PG_HOST", "localhost");
    cmd.env("TUMULT_PG_PORT", "5432");
    cmd.env("TUMULT_PG_USER", pg_user());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {}", path, e));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// ── Probe tests ───────────────────────────────────────────────

#[test]
fn connection_count_returns_integer() {
    if !pg_available() {
        eprintln!("SKIP: PostgreSQL not available");
        return;
    }

    let (code, stdout, stderr) = run_script("probes/connection-count.sh", &[]);
    assert_eq!(code, 0, "probe failed: {}", stderr);
    let count: i64 = stdout
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("not an integer: '{}'", stdout.trim()));
    assert!(count >= 1, "expected at least 1 connection, got {}", count);
}

#[test]
fn replication_lag_returns_number() {
    if !pg_available() {
        eprintln!("SKIP: PostgreSQL not available");
        return;
    }

    let (code, stdout, stderr) = run_script("probes/replication-lag.sh", &[]);
    assert_eq!(code, 0, "probe failed: {}", stderr);
    let lag: f64 = stdout
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("not a number: '{}'", stdout.trim()));
    // On a non-replica, lag should be 0
    assert!(lag >= 0.0, "lag should be >= 0, got {}", lag);
}

#[test]
fn pool_utilization_returns_json() {
    if !pg_available() {
        eprintln!("SKIP: PostgreSQL not available");
        return;
    }

    let (code, stdout, stderr) = run_script("probes/pool-utilization.sh", &[]);
    assert_eq!(code, 0, "probe failed: {}", stderr);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("not valid JSON: '{}'", stdout.trim()));
    assert!(parsed.get("current_connections").is_some());
    assert!(parsed.get("max_connections").is_some());
    assert!(parsed.get("utilization_pct").is_some());

    let max = parsed["max_connections"].as_i64().unwrap();
    assert!(max > 0, "max_connections should be > 0");
}

// ── Action validation tests (no destructive actions) ──────────

#[test]
fn kill_connections_requires_database() {
    let (code, _, stderr) = run_script("actions/kill-connections.sh", &[]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("TUMULT_PG_DATABASE"),
        "should require TUMULT_PG_DATABASE: {}",
        stderr
    );
}

#[test]
fn lock_table_requires_database_and_table() {
    let (code, _, stderr) = run_script("actions/lock-table.sh", &[]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("TUMULT_PG_DATABASE") || stderr.contains("TUMULT_PG_TABLE"),
        "should require vars: {}",
        stderr
    );
}

#[test]
fn exhaust_connections_requires_database() {
    let (code, _, stderr) = run_script("actions/exhaust-connections.sh", &[]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("TUMULT_PG_DATABASE"),
        "should require TUMULT_PG_DATABASE: {}",
        stderr
    );
}

// ── Manifest test ─────────────────────────────────────────────

#[test]
fn postgres_manifest_parses() {
    let path = plugin_path("plugin.toon");
    let content = std::fs::read_to_string(&path).expect("manifest not found");
    assert!(content.contains("name: tumult-db-postgres"));
    assert!(content.contains("actions"));
    assert!(content.contains("probes"));
}
