//! Behavior tests for the oha load adapter: the start/stop lifecycle
//! against a fake `oha` binary, including windowed sampling for a soak
//! profile and early stop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tumult_core::runner::oha::OhaLoadExecutor;
use tumult_core::runner::{load_executor_for, LoadExecutor};
use tumult_core::types::{LoadConfig, LoadProfile, LoadTool};

// The executor resolves the binary through `TUMULT_OHA_BINARY`; tests that
// point it at a fake binary must not run concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn config(profile: Option<LoadProfile>, duration_s: f64) -> LoadConfig {
    LoadConfig {
        tool: LoadTool::Oha,
        script: PathBuf::from("http://127.0.0.1:1/health"),
        vus: Some(4),
        duration_s: Some(duration_s),
        thresholds: HashMap::new(),
        profile,
    }
}

/// Write an executable fake-oha script and point `TUMULT_OHA_BINARY` at it.
fn install_fake_oha(dir: &tempfile::TempDir, body: &str) {
    let path = dir.path().join("fake-oha");
    std::fs::write(&path, body).expect("write fake oha");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake oha");
    }
    std::env::set_var("TUMULT_OHA_BINARY", &path);
}

/// A fake oha that prints a JSON summary whose request count is the
/// concurrency it was asked for, so windows are distinguishable.
const FAKE_OHA_JSON: &str = r#"#!/bin/sh
conc=0
while [ $# -gt 0 ]; do
  if [ "$1" = "-c" ]; then shift; conc="$1"; fi
  if [ "$1" = "--version" ]; then exit 0; fi
  shift
done
printf '%s' "{\"summary\":{\"successRate\":0.95,\"total\":1.0,\"slowest\":0.2,\"fastest\":0.01,\"average\":0.05,\"requestsPerSec\":50.0,\"totalData\":1,\"sizePerRequest\":1,\"sizePerSec\":1},\"responseTimeHistogram\":{},\"latencyPercentiles\":{\"p50\":0.02,\"p95\":0.08,\"p99\":0.1},\"rps\":{},\"details\":{},\"statusCodeDistribution\":{\"200\":$conc},\"errorDistribution\":{}}"
exit 0
"#;

#[test]
fn flat_config_is_one_window_and_one_sample() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_JSON);

    let executor = OhaLoadExecutor;
    let handle = executor.start(&config(None, 1.0)).expect("fake oha starts");
    // The fake exits immediately; give the worker a moment to finish.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let result = executor.stop(handle).expect("fake oha stops");

    assert_eq!(result.tool, LoadTool::Oha);
    assert_eq!(result.vus, 4);
    assert_eq!(result.samples.len(), 1);
    assert_eq!(result.total_requests, 4);
    assert!((result.throughput_rps - 50.0).abs() < f64::EPSILON);
    assert!((result.latency_p50_ms - 20.0).abs() < 1e-9);
    assert!((result.latency_p95_ms - 80.0).abs() < 1e-9);
    assert!((result.latency_p99_ms - 100.0).abs() < 1e-9);
    assert!((result.error_rate - 0.05).abs() < 1e-9);
    assert!(result.thresholds_met);
    assert!(result.ended_at_ns >= result.started_at_ns);
}

#[test]
fn soak_profile_streams_one_sample_per_interval() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_JSON);

    let soak = LoadProfile::Soak {
        sample_interval_s: 0.1,
    };
    let executor = OhaLoadExecutor;
    let handle = executor.start(&config(Some(soak), 0.3)).expect("starts");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let result = executor.stop(handle).expect("stops");

    assert_eq!(result.samples.len(), 3, "{:?}", result.samples);
    assert!(result.samples.iter().all(|s| s.vus == 4 && s.requests == 4));
    assert!(result
        .samples
        .windows(2)
        .all(|pair| pair[0].ts_ns <= pair[1].ts_ns));
    assert_eq!(result.total_requests, 12);
}

#[test]
fn ramp_profile_steps_the_concurrency_per_window() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&dir, FAKE_OHA_JSON);

    let ramp = LoadProfile::Ramp {
        start_vus: 0,
        ramp_up_s: 0.2,
    };
    let mut cfg = config(Some(ramp), 0.3);
    cfg.vus = Some(4);
    let executor = OhaLoadExecutor;
    let handle = executor.start(&cfg).expect("starts");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let result = executor.stop(handle).expect("stops");

    // One window per stage: the ramp at its midpoint level, then the hold.
    let vus: Vec<u32> = result.samples.iter().map(|s| s.vus).collect();
    assert_eq!(vus, vec![2, 4]);
}

#[test]
fn stop_skips_the_windows_not_yet_started() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    // Each window takes ~300 ms regardless of `-z`.
    let slow = FAKE_OHA_JSON.replacen("conc=0\n", "conc=0\nsleep 0.3\n", 1);
    install_fake_oha(&dir, &slow);

    let soak = LoadProfile::Soak {
        sample_interval_s: 1.0,
    };
    let executor = OhaLoadExecutor;
    let handle = executor.start(&config(Some(soak), 60.0)).expect("starts");
    let result = executor.stop(handle).expect("stops");

    // The in-flight window finishes (bounded wait), nothing else starts.
    assert!(result.samples.len() <= 2, "{}", result.samples.len());
}

#[test]
fn failed_window_is_reported_not_hidden() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(
        &dir,
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] && exit 0\necho broken >&2\nexit 2\n",
    );

    let executor = OhaLoadExecutor;
    let handle = executor.start(&config(None, 1.0)).expect("starts");
    std::thread::sleep(std::time::Duration::from_millis(200));
    let result = executor.stop(handle).expect("stops");

    assert!(result.samples.is_empty());
    assert!(!result.thresholds_met);
    assert_eq!(result.total_requests, 0);
}

#[test]
fn start_errors_when_the_oha_binary_cannot_be_spawned() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    std::env::set_var("TUMULT_OHA_BINARY", "/nonexistent/tumult-oha-test-binary");

    let executor = OhaLoadExecutor;
    let Err(error) = executor.start(&config(None, 1.0)) else {
        panic!("a missing binary must fail to start");
    };
    assert!(error.contains("failed to start oha"), "{error}");
}

#[test]
fn adapter_selector_covers_k6_and_oha_but_not_jmeter() {
    assert!(load_executor_for(&LoadTool::K6).is_ok());
    assert!(load_executor_for(&LoadTool::Oha).is_ok());
    let Err(err) = load_executor_for(&LoadTool::Jmeter) else {
        panic!("jmeter must have no adapter");
    };
    assert!(err.contains("jmeter"), "{err}");
}
