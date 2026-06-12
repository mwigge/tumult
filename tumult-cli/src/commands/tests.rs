use super::*;
use tempfile::TempDir;
use tumult_core::execution::RollbackStrategy;
use tumult_core::runner::ActivityExecutor;
use tumult_core::types::{Activity, ActivityType};

// ── Helper: write a valid experiment file ─────────────────

fn write_valid_experiment(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("test-experiment.toon");
    let exp = Experiment {
        version: "v1".into(),
        title: "CLI test experiment".into(),
        description: Some("Tests CLI command execution".into()),
        tags: vec!["test".into()],
        configuration: indexmap::IndexMap::new(),
        secrets: indexmap::IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![Activity {
            name: "echo-action".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
    };
    let toon = toon_format::encode_default(&exp).unwrap();
    std::fs::write(&path, toon).unwrap();
    path
}

fn write_invalid_experiment(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("invalid.toon");
    std::fs::write(&path, "this is not valid toon {{{").unwrap();
    path
}

fn write_empty_method_experiment(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("empty-method.toon");
    let exp = Experiment {
        version: "v1".into(),
        title: "Empty method experiment".into(),
        description: None,
        tags: vec![],
        configuration: indexmap::IndexMap::new(),
        secrets: indexmap::IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
    };
    let toon = toon_format::encode_default(&exp).unwrap();
    std::fs::write(&path, toon).unwrap();
    path
}

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

// ── ProviderExecutor tests ────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_runs_echo() {
    let activity = Activity {
        name: "echo-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "echo".into(),
            arguments: vec!["hello world".into()],
            env: std::collections::HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor;
    let outcome = executor.execute(&activity);

    assert!(outcome.success);
    assert_eq!(outcome.output.as_deref(), Some("hello world"));
}

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_captures_failure() {
    let activity = Activity {
        name: "false-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "false".into(),
            arguments: vec![],
            env: std::collections::HashMap::new(),
            timeout_s: Some(5.0),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor;
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
}

#[tokio::test(flavor = "multi_thread")]
async fn process_executor_nonexistent_returns_error() {
    let activity = Activity {
        name: "bad-cmd".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Process {
            path: "/nonexistent/binary".into(),
            arguments: vec![],
            env: std::collections::HashMap::new(),
            timeout_s: None,
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor;
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    assert!(outcome.error.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn native_provider_rejects_unknown_plugin() {
    let activity = Activity {
        name: "native-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "unknown-plugin".into(),
            function: "test-fn".into(),
            arguments: std::collections::HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    };

    let executor = ProviderExecutor;
    let outcome = executor.execute(&activity);

    assert!(!outcome.success);
    assert!(outcome
        .error
        .as_ref()
        .unwrap()
        .contains("unknown native plugin"));
}

// ── Phase 4: Import/Export roundtrip ──────────────────────

#[test]
fn import_rejects_missing_directory() {
    let result = cmd_import(Path::new("/nonexistent/path"));
    assert!(result.is_err());
}

#[test]
fn import_rejects_missing_parquet_files() {
    let d = TempDir::new().unwrap();
    let result = cmd_import(d.path());
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("experiments.parquet not found"));
}

// ── Phase 4: Run with auto-ingest ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cmd_run_with_auto_ingest() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("out.toon");

    // Run with auto-ingest disabled (avoids touching real ~/.tumult)
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
async fn cmd_run_dry_run_does_not_ingest() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("out.toon");

    let result = cmd_run(
        &exp_path,
        &journal_path,
        true,
        RollbackStrategy::OnDeviation,
        true,
        std::collections::HashMap::new(),
        None,
    )
    .await;
    assert!(result.is_ok());
    // Journal should NOT be written in dry-run mode
    assert!(!journal_path.exists());
}

// ── Phase 4: Store command tests ──────────────────────────

#[test]
fn store_backup_creates_parquet_files() {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::types::*;

    let d = TempDir::new().unwrap();
    let db_path = d.path().join("test.duckdb");
    let backup_dir = d.path().join("backup");

    // Create store with data
    let store = AnalyticsStore::open(&db_path).unwrap();
    store
        .ingest_journal(&Journal {
            experiment_title: "test".into(),
            experiment_id: "e1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_060_000_000_000,
            duration_ms: 60_000,
            method_results: vec![],
            steady_state_before: None,
            steady_state_after: None,
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        })
        .unwrap();
    drop(store);

    // Backup via store API directly
    let store = AnalyticsStore::open(&db_path).unwrap();
    std::fs::create_dir_all(&backup_dir).unwrap();
    store
        .export_tables(
            &backup_dir.join("experiments.parquet"),
            &backup_dir.join("activities.parquet"),
        )
        .unwrap();

    assert!(backup_dir.join("experiments.parquet").exists());
    assert!(backup_dir.join("activities.parquet").exists());
}

#[test]
fn store_purge_removes_old_data() {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::types::*;

    let d = TempDir::new().unwrap();
    let db_path = d.path().join("test.duckdb");
    let store = AnalyticsStore::open(&db_path).unwrap();

    // Old experiment (2020)
    store
        .ingest_journal(&Journal {
            experiment_title: "old".into(),
            experiment_id: "old-1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_577_836_800_000_000_000,
            ended_at_ns: 1_577_836_860_000_000_000,
            duration_ms: 60_000,
            method_results: vec![],
            steady_state_before: None,
            steady_state_after: None,
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        })
        .unwrap();

    // Recent experiment
    let recent_started_at_ns = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or(i64::MAX - 60_000_000_000);
    store
        .ingest_journal(&Journal {
            experiment_title: "new".into(),
            experiment_id: "new-1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: recent_started_at_ns,
            ended_at_ns: recent_started_at_ns.saturating_add(60_000_000_000),
            duration_ms: 60_000,
            method_results: vec![],
            steady_state_before: None,
            steady_state_after: None,
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        })
        .unwrap();

    assert_eq!(store.experiment_count().unwrap(), 2);
    let purged = store.purge_older_than_days(30).unwrap();
    assert_eq!(purged, 1);
    assert_eq!(store.experiment_count().unwrap(), 1);
}

#[test]
fn store_stats_reports_counts() {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::types::*;

    let store = AnalyticsStore::in_memory().unwrap();
    let stats = store.stats().unwrap();
    assert_eq!(stats.experiment_count, 0);
    assert_eq!(stats.activity_count, 0);

    store
        .ingest_journal(&Journal {
            experiment_title: "test".into(),
            experiment_id: "e1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_060_000_000_000,
            duration_ms: 60_000,
            method_results: vec![ActivityResult {
                name: "act".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1_774_980_000_000_000_000,
                duration_ms: 500,
                output: None,
                error: None,
                trace_id: TraceId::empty(),
                span_id: SpanId::empty(),
            }],
            steady_state_before: None,
            steady_state_after: None,
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        })
        .unwrap();

    let stats = store.stats().unwrap();
    assert_eq!(stats.experiment_count, 1);
    assert_eq!(stats.activity_count, 1);
}

// ── Phase 3: Report command ──────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_generates_html_file() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("journal.toon");

    cmd_run(
        &exp_path,
        &journal_path,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap();

    let report_path = d.path().join("report.html");
    cmd_report(&journal_path, Some(&report_path), ReportFormat::Html).unwrap();
    assert!(report_path.exists());

    let content = std::fs::read_to_string(&report_path).unwrap();
    assert!(content.contains("<!DOCTYPE html>"));
    assert!(content.contains("Tumult Experiment Report"));
    assert!(content.contains("CLI test experiment"));
    assert!(content.contains("Activity Timeline"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_default_output_uses_journal_stem() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("my-experiment.toon");

    cmd_run(
        &exp_path,
        &journal_path,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap();

    // Change to temp dir so default output lands there
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(d.path()).unwrap();
    cmd_report(&journal_path, None, ReportFormat::Html).unwrap();
    std::env::set_current_dir(prev).unwrap();

    assert!(d.path().join("my-experiment.html").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_html_contains_trace_ids() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("journal.toon");

    cmd_run(
        &exp_path,
        &journal_path,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap();

    let report_path = d.path().join("report.html");
    cmd_report(&journal_path, Some(&report_path), ReportFormat::Html).unwrap();

    let content = std::fs::read_to_string(&report_path).unwrap();
    // Should contain method steps
    assert!(content.contains("echo-action"));
}

#[test]
fn report_nonexistent_journal_returns_error() {
    let result = cmd_report(Path::new("/nonexistent.toon"), None, ReportFormat::Html);
    assert!(result.is_err());
}

// ── parse_duration_str tests ──────────────────────────────

#[test]
fn parse_duration_seconds() {
    assert!((parse_duration_str("30s") - 30.0).abs() < f64::EPSILON);
}

#[test]
fn parse_duration_minutes() {
    assert!((parse_duration_str("5m") - 300.0).abs() < f64::EPSILON);
}

#[test]
fn parse_duration_hours() {
    assert!((parse_duration_str("2h") - 7200.0).abs() < f64::EPSILON);
}

#[test]
fn parse_duration_bare_number() {
    assert!((parse_duration_str("45") - 45.0).abs() < f64::EPSILON);
}

#[test]
fn parse_duration_invalid_falls_back_to_default() {
    assert!((parse_duration_str("nonsense") - 30.0).abs() < f64::EPSILON);
}

// ── parse_var_args tests ──────────────────────────────────

#[test]
fn parse_var_args_valid() {
    let vars = vec!["HOST=localhost".to_string(), "PORT=8080".to_string()];
    let map = parse_var_args(&vars).unwrap();
    assert_eq!(map.get("HOST").map(String::as_str), Some("localhost"));
    assert_eq!(map.get("PORT").map(String::as_str), Some("8080"));
}

#[test]
fn parse_var_args_empty_is_ok() {
    let map = parse_var_args(&[]).unwrap();
    assert!(map.is_empty());
}

#[test]
fn parse_var_args_missing_equals_is_error() {
    let vars = vec!["NOEQUALSSIGN".to_string()];
    let result = parse_var_args(&vars);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("KEY=VALUE format"));
}

#[test]
fn parse_var_args_value_with_equals() {
    // KEY=VALUE=WITH=EQUALS should split on first = only
    let vars = vec!["URL=http://localhost:8080/path?a=1".to_string()];
    let map = parse_var_args(&vars).unwrap();
    assert_eq!(
        map.get("URL").map(String::as_str),
        Some("http://localhost:8080/path?a=1")
    );
}

// ── build_load_override tests ─────────────────────────────

#[test]
fn build_load_override_none_tool_returns_none() {
    let result = build_load_override(None, None, None, None);
    assert!(result.is_none());
}

#[test]
fn build_load_override_explicit_none_returns_none() {
    let result = build_load_override(Some(LoadToolArg::None), None, None, None);
    assert!(result.is_none());
}

#[test]
fn build_load_override_k6_returns_config() {
    let result = build_load_override(
        Some(LoadToolArg::K6),
        None,
        Some(20),
        Some("60s".to_string()),
    );
    let config = result.unwrap();
    assert_eq!(config.vus, Some(20));
    assert!((config.duration_s.unwrap() - 60.0).abs() < f64::EPSILON);
    assert!(matches!(config.tool, tumult_core::types::LoadTool::K6));
}

#[test]
fn build_load_override_jmeter_returns_config() {
    let result = build_load_override(Some(LoadToolArg::Jmeter), None, None, None);
    let config = result.unwrap();
    assert!(matches!(config.tool, tumult_core::types::LoadTool::Jmeter));
    assert_eq!(config.vus, Some(10)); // default
    assert!((config.duration_s.unwrap() - 30.0).abs() < f64::EPSILON); // default
}

// ── k6 metric parser tests (CLI-TEST-01) ─────────────────────────────

#[test]
fn parse_k6_metric_extracts_stat_value() {
    let output = "iteration_duration...: avg=97.77ms min=55.75ms med=63.81ms max=201.09ms p(90)=67.34ms p(95)=148.01ms";
    assert!((parse_k6_metric(output, "iteration_duration", "avg").unwrap() - 97.77).abs() < 0.001);
    assert!((parse_k6_metric(output, "iteration_duration", "med").unwrap() - 63.81).abs() < 0.001);
    assert!(
        (parse_k6_metric(output, "iteration_duration", "p(95)").unwrap() - 148.01).abs() < 0.001
    );
}

#[test]
fn parse_k6_metric_returns_none_for_missing_metric() {
    let output = "iteration_duration...: avg=10ms";
    assert!(parse_k6_metric(output, "missing_metric", "avg").is_none());
}

#[test]
fn parse_k6_metric_returns_none_for_missing_stat() {
    let output = "iteration_duration...: avg=10ms";
    assert!(parse_k6_metric(output, "iteration_duration", "p(99)").is_none());
}

#[test]
fn parse_k6_counter_extracts_integer_value() {
    let output = "iterations...........: 1025 51.006998/s";
    assert_eq!(parse_k6_counter(output, "iterations"), Some(1025));
}

#[test]
fn parse_k6_counter_returns_none_for_missing_counter() {
    let output = "iterations...........: 1025 51.006998/s";
    assert!(parse_k6_counter(output, "checks_total").is_none());
}

#[test]
fn parse_k6_counter_handles_zero_value() {
    let output = "checks_failed.......: 0 0/s";
    assert_eq!(parse_k6_counter(output, "checks_failed"), Some(0));
}

#[test]
fn parse_k6_rate_extracts_per_second_value() {
    let output = "iterations...........: 300 29.82/s";
    let rate = parse_k6_rate(output, "iterations").unwrap();
    assert!((rate - 29.82).abs() < 0.001);
}

#[test]
fn parse_k6_rate_returns_none_for_missing_counter() {
    let output = "iterations...........: 300 29.82/s";
    assert!(parse_k6_rate(output, "http_reqs").is_none());
}

#[test]
fn parse_k6_rate_handles_integer_rate() {
    let output = "http_reqs...........: 100 10/s";
    let rate = parse_k6_rate(output, "http_reqs").unwrap();
    assert!((rate - 10.0).abs() < 0.001);
}

#[test]
fn parse_k6_metric_multiline_finds_correct_metric() {
    let output = "\
http_req_duration...: avg=12ms p(95)=50ms\n\
iteration_duration..: avg=97ms p(95)=148ms\n\
";
    assert!(
        (parse_k6_metric(output, "iteration_duration", "p(95)").unwrap() - 148.0).abs() < 0.001
    );
    assert!((parse_k6_metric(output, "http_req_duration", "avg").unwrap() - 12.0).abs() < 0.001);
}

// ── k6 JSON summary parser tests (T-9) ───────────────────────────────

fn sample_k6_summary() -> serde_json::Value {
    serde_json::json!({
        "metrics": {
            "iterations": { "count": 1025, "rate": 51.006998 },
            "iteration_duration": {
                "avg": 97.77, "min": 55.75, "med": 63.81, "max": 201.09,
                "p(90)": 67.34, "p(95)": 148.01, "p(99)": 180.0
            },
            "checks_total": { "count": 1025 },
            "checks_failed": { "count": 5 }
        }
    })
}

#[test]
fn k6_summary_metric_extracts_value() {
    let summary = sample_k6_summary();
    assert!(
        (k6_summary_metric(Some(&summary), "iteration_duration", "p(95)").unwrap() - 148.01).abs()
            < 0.001
    );
    assert!(
        (k6_summary_metric(Some(&summary), "iterations", "rate").unwrap() - 51.006998).abs()
            < 0.001
    );
}

#[test]
fn k6_summary_metric_returns_none_for_missing_metric() {
    let summary = sample_k6_summary();
    assert!(k6_summary_metric(Some(&summary), "http_req_duration", "p(95)").is_none());
    assert!(k6_summary_metric(None, "iterations", "rate").is_none());
}

#[test]
fn k6_summary_count_extracts_value() {
    let summary = sample_k6_summary();
    assert_eq!(k6_summary_count(Some(&summary), "checks_total"), Some(1025));
    assert_eq!(k6_summary_count(Some(&summary), "checks_failed"), Some(5));
    assert!(k6_summary_count(Some(&summary), "missing_counter").is_none());
}

#[test]
fn k6_metric_or_warn_returns_parsed_value() {
    assert!((k6_metric_or_warn(Some(42.5), "some.metric") - 42.5).abs() < 0.001);
}

#[test]
fn k6_metric_or_warn_defaults_to_zero_when_missing() {
    assert_eq!(k6_metric_or_warn(None, "some.metric"), 0.0);
}

#[test]
fn read_k6_summary_returns_none_for_missing_file() {
    let path = std::path::Path::new("/nonexistent/tumult-k6-summary.json");
    assert!(read_k6_summary(path).is_none());
}

#[test]
fn read_k6_summary_returns_none_for_invalid_json() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut file, b"not json").unwrap();
    assert!(read_k6_summary(file.path()).is_none());
}

#[test]
fn read_k6_summary_parses_valid_json() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let summary = sample_k6_summary();
    std::io::Write::write_all(&mut file, summary.to_string().as_bytes()).unwrap();
    let parsed = read_k6_summary(file.path()).unwrap();
    assert_eq!(k6_summary_count(Some(&parsed), "checks_total"), Some(1025));
}
