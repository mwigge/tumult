//! E2E tests that require Docker infrastructure.
//!
//! Run with: make e2e (or cargo test -- --ignored)
//! Requires: docker compose up -d in docker/

use std::process::Command;

fn docker_pg_available() -> bool {
    Command::new("pg_isready")
        .args(["-h", "localhost", "-p", "15432", "-U", "tumult"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn docker_redis_available() -> bool {
    Command::new("redis-cli")
        .args(["-h", "localhost", "-p", "16379", "ping"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("PONG"))
        .unwrap_or(false)
}

fn run_plugin_script(script: &str, env: &[(&str, &str)]) -> (i32, String, String) {
    let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let path = ws.join("plugins").join(script);
    let mut cmd = Command::new("sh");
    cmd.arg(&path);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("failed to run script");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// ═══════════════════════════════════════════════════════════════
// PostgreSQL E2E (against Docker container)
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires Docker: make infra-up"]
fn e2e_postgres_connection_count() {
    if !docker_pg_available() {
        eprintln!("SKIP: Docker PostgreSQL not available on port 15432");
        return;
    }

    let (code, stdout, stderr) = run_plugin_script(
        "tumult-db-postgres/probes/connection-count.sh",
        &[
            ("TUMULT_PG_HOST", "localhost"),
            ("TUMULT_PG_PORT", "15432"),
            ("TUMULT_PG_USER", "tumult"),
            ("TUMULT_PG_PASSWORD", "tumult_test"),
        ],
    );
    assert_eq!(code, 0, "probe failed: {}", stderr);
    let count: i64 = stdout.trim().parse().expect("not a number");
    assert!(count >= 1, "expected at least 1 connection, got {}", count);
}

#[test]
#[ignore = "requires Docker: make infra-up"]
fn e2e_postgres_pool_utilization() {
    if !docker_pg_available() {
        return;
    }

    let (code, stdout, stderr) = run_plugin_script(
        "tumult-db-postgres/probes/pool-utilization.sh",
        &[
            ("TUMULT_PG_HOST", "localhost"),
            ("TUMULT_PG_PORT", "15432"),
            ("TUMULT_PG_USER", "tumult"),
            ("TUMULT_PG_PASSWORD", "tumult_test"),
        ],
    );
    assert_eq!(code, 0, "probe failed: {}", stderr);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("not valid JSON");
    assert!(parsed.get("max_connections").is_some());
}

#[test]
#[ignore = "requires Docker: make infra-up"]
fn e2e_postgres_kill_connections() {
    if !docker_pg_available() {
        return;
    }

    let (code, _stdout, stderr) = run_plugin_script(
        "tumult-db-postgres/actions/kill-connections.sh",
        &[
            ("TUMULT_PG_HOST", "localhost"),
            ("TUMULT_PG_PORT", "15432"),
            ("TUMULT_PG_USER", "tumult"),
            ("TUMULT_PG_PASSWORD", "tumult_test"),
            ("TUMULT_PG_DATABASE", "tumult_test"),
        ],
    );
    assert_eq!(code, 0, "kill-connections failed: {}", stderr);
}

// ═══════════════════════════════════════════════════════════════
// Redis E2E (against Docker container)
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires Docker: make infra-up"]
fn e2e_redis_ping() {
    if !docker_redis_available() {
        eprintln!("SKIP: Docker Redis not available on port 16379");
        return;
    }

    let (code, stdout, _) = run_plugin_script(
        "tumult-db-redis/probes/redis-ping.sh",
        &[
            ("TUMULT_REDIS_HOST", "localhost"),
            ("TUMULT_REDIS_PORT", "16379"),
        ],
    );
    assert_eq!(code, 0);
    assert!(stdout.trim().contains("PONG"));
}

#[test]
#[ignore = "requires Docker: make infra-up"]
fn e2e_redis_info() {
    if !docker_redis_available() {
        return;
    }

    let (code, stdout, _) = run_plugin_script(
        "tumult-db-redis/probes/redis-info.sh",
        &[
            ("TUMULT_REDIS_HOST", "localhost"),
            ("TUMULT_REDIS_PORT", "16379"),
        ],
    );
    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("not valid JSON");
    assert!(parsed.get("connected_clients").is_some());
}

// ═══════════════════════════════════════════════════════════════
// Full pipeline E2E
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires Docker: make infra-up"]
fn e2e_full_pipeline_init_run_analyze_export() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let tumult = ws.join("target/release/tumult");

    // Skip if binary not built
    if !tumult.exists() {
        eprintln!("SKIP: release binary not found. Run: cargo build --release -p tumult-cli");
        return;
    }

    // Init
    let output = Command::new(&tumult)
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("init failed");
    assert!(output.status.success(), "init failed");

    // Run
    let output = Command::new(&tumult)
        .args(["run", "experiment.toon"])
        .current_dir(dir.path())
        .output()
        .expect("run failed");
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Analyze
    let output = Command::new(&tumult)
        .args(["analyze", "journal.toon"])
        .current_dir(dir.path())
        .output()
        .expect("analyze failed");
    assert!(output.status.success(), "analyze failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Experiment Summary"));

    // Export parquet
    let output = Command::new(&tumult)
        .args(["export", "journal.toon", "--format", "parquet"])
        .current_dir(dir.path())
        .output()
        .expect("export failed");
    assert!(output.status.success(), "export failed");
    assert!(dir.path().join("journal.parquet").exists());

    // Export CSV
    let output = Command::new(&tumult)
        .args(["export", "journal.toon", "--format", "csv"])
        .current_dir(dir.path())
        .output()
        .expect("csv export failed");
    assert!(output.status.success());

    // Trend
    let output = Command::new(&tumult)
        .args(["trend", ".", "--metric", "duration_ms"])
        .current_dir(dir.path())
        .output()
        .expect("trend failed");
    assert!(output.status.success());
}

// ═══════════════════════════════════════════════════════════════
// Stress probes E2E (against local machine — no Docker needed)
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore = "slow — runs cpu/memory utilization probes"]
fn e2e_stress_probes_return_valid_numbers() {
    let (code, stdout, _) = run_plugin_script("tumult-stress/probes/cpu-utilization.sh", &[]);
    assert_eq!(code, 0);
    let cpu: f64 = stdout.trim().parse().expect("cpu not a number");
    assert!((0.0..=100.0).contains(&cpu));

    let (code, stdout, _) = run_plugin_script("tumult-stress/probes/memory-utilization.sh", &[]);
    assert_eq!(code, 0);
    let mem: f64 = stdout.trim().parse().expect("memory not a number");
    assert!((0.0..=100.0).contains(&mem));
}
