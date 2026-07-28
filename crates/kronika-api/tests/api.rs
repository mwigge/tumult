//! End-to-end tests for the query API: a seeded store served on an ephemeral
//! port, every endpoint exercised over HTTP.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use kronika_store::{LogRow, MetricSumRow, SpanRow, Store};
use serde_json::Value;

const NS: i64 = 1_000_000_000;

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64
}

/// Two experiments: `exp-pass` (Completed, 30 min ago) and `exp-fail`
/// (Deviated, 10 min ago), with tumult-style outcome logs and counters.
/// Returns the timestamp of exp-pass's `experiment.started` log so tests can
/// pin its survival end-to-end.
fn seed(db_path: &std::path::Path) -> i64 {
    let store = Store::open(db_path).unwrap();
    let writer = store.writer().unwrap();
    let now = now_ns();

    let root = |id: &str, name: &str, ts: i64| SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        parent_span_id: None,
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 5 * NS,
        status_code: "Unset".into(),
        status_message: String::new(),
        service_name: "tumult".into(),
        service_version: Some("2.18.0".into()),
        experiment_id: Some(id.into()),
        experiment_name: Some(name.into()),
        outcome_status: None,
        fault_type: None,
        fault_subtype: None,
        fault_severity: None,
        blast_radius: None,
        target_system: Some("database".into()),
        target_technology: Some("postgresql".into()),
        target_environment: Some("demo".into()),
        plugin_name: None,
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs: vec![],
        events: "[]".into(),
    };
    // Tumult-realistic: only the root span carries experiment_id; children
    // correlate through trace_id.
    let action = |id: &str, ts: i64| SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-action"),
        parent_span_id: Some(format!("span-{id}-root")),
        span_name: "resilience.action".into(),
        span_kind: "Internal".into(),
        duration_ns: 2 * NS,
        status_code: "Ok".into(),
        status_message: String::new(),
        service_name: "tumult".into(),
        service_version: Some("2.18.0".into()),
        experiment_id: None,
        experiment_name: None,
        outcome_status: None,
        fault_type: Some("injection".into()),
        fault_subtype: Some("process-kill".into()),
        fault_severity: None,
        blast_radius: None,
        target_system: None,
        target_technology: None,
        target_environment: None,
        plugin_name: Some("process".into()),
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs: vec![],
        events: "[]".into(),
    };

    writer
        .insert_spans(&[
            root("exp-pass", "pg-failover", now - 1800 * NS),
            action("exp-pass", now - 1799 * NS),
            root("exp-fail", "cache-stampede", now - 600 * NS),
            action("exp-fail", now - 599 * NS),
        ])
        .unwrap();

    let log = |id: &str, body: &str, status: Option<&str>, ts: i64| LogRow {
        ts_ns: ts,
        severity_text: "INFO".into(),
        body: body.into(),
        trace_id: Some(format!("trace-{id}")),
        span_id: None,
        service_name: "tumult".into(),
        log_attrs: [
            Some(("experiment_id".to_string(), id.to_string())),
            Some(("experiment_title".to_string(), format!("title-{id}"))),
            status.map(|s| ("status".to_string(), s.to_string())),
            status.map(|_| ("duration_ms".to_string(), "4200".to_string())),
            status.map(|_| ("deviations".to_string(), "0".to_string())),
        ]
        .into_iter()
        .flatten()
        .collect(),
        resource_attrs: vec![],
    };
    writer
        .insert_logs(&[
            log("exp-pass", "experiment.started", None, now - 1800 * NS),
            log(
                "exp-pass",
                "experiment.completed",
                Some("Completed"),
                now - 1795 * NS,
            ),
            log("exp-fail", "experiment.started", None, now - 600 * NS),
            log(
                "exp-fail",
                "experiment.completed",
                Some("Deviated"),
                now - 595 * NS,
            ),
        ])
        .unwrap();

    let counter = |name: &str, outcome: Option<&str>, ts: i64| MetricSumRow {
        ts_ns: ts,
        metric_name: name.into(),
        value: 1.0,
        experiment_name: Some("pg-failover".into()),
        outcome_status: outcome.map(str::to_string),
        plugin_name: None,
        attrs: vec![],
        resource_attrs: vec![],
    };
    writer
        .insert_metric_sums(&[
            counter("tumult.experiments.total", Some("success"), now - 1795 * NS),
            counter("tumult.experiments.total", Some("failure"), now - 595 * NS),
            counter("tumult.hypothesis.deviations.total", None, now - 595 * NS),
        ])
        .unwrap();

    now - 1800 * NS
}

struct TestServer {
    base: String,
    _tmp: tempfile::TempDir,
    reports_dir: PathBuf,
    pass_log_ts: i64,
}

async fn spawn_server() -> TestServer {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("k.duckdb");
    let reports_dir = tmp.path().join("reports");
    std::fs::create_dir_all(&reports_dir).unwrap();
    std::fs::write(
        reports_dir.join("2026-01-01T00-00_digest.html"),
        "<html>digest</html>",
    )
    .unwrap();
    let pass_log_ts = seed(&db_path);

    let metrics_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../metrics")
        .canonicalize()
        .unwrap();
    // An LLM client pointing at a closed port: connection refused, which the
    // ask endpoint must surface as `{configured: false}`.
    let llm = Arc::new(kronika_ai::OpenAiCompatClient::new(
        "http://127.0.0.1:1".into(),
        None,
        "test-model".into(),
    ));
    let state = kronika_api::ApiState::new(db_path, metrics_dir, reports_dir.clone(), llm);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, kronika_api::router(state))
            .await
            .unwrap();
    });
    TestServer {
        base: format!("http://{addr}"),
        _tmp: tmp,
        reports_dir,
        pass_log_ts,
    }
}

async fn get(base: &str, path: &str) -> (u16, Value) {
    let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    (status, body)
}

#[tokio::test]
async fn overview_returns_kpis_deltas_and_breakdowns() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/overview?range=24h").await;
    assert_eq!(status, 200, "{body}");
    let kpis = body["kpis"].as_array().unwrap();
    assert_eq!(kpis.len(), 5);

    let kpi = |name: &str| kpis.iter().find(|k| k["name"] == name).unwrap().clone();
    assert_eq!(kpi("experiments")["value"].as_f64().unwrap(), 2.0);
    assert_eq!(kpi("pass_rate")["value"], 0.5);
    assert_eq!(kpi("deviation_rate")["value"], 0.5);
    // No recovery_time_s in the seed → honest null, and coverage = 1 target.
    assert!(kpi("mttr_s")["value"].is_null());
    assert_eq!(kpi("coverage")["value"].as_f64().unwrap(), 1.0);
    // Previous window was empty → delta vs 0/None is computed, not missing.
    assert_eq!(kpi("experiments")["delta"].as_f64().unwrap(), 2.0);
    assert!(!kpi("experiments")["spark"].as_array().unwrap().is_empty());

    assert_eq!(body["experiments_per_day"].as_array().unwrap().len(), 1);
    let targets = body["targets"].as_array().unwrap();
    assert_eq!(targets[0]["target"], "database");
    assert_eq!(targets[0]["experiments"], 2);
    assert_eq!(targets[0]["pass_rate"], 0.5);
    let faults = body["faults"].as_array().unwrap();
    assert_eq!(faults[0]["fault_type"], "injection");
    assert_eq!(faults[0]["count"], 2);
}

#[tokio::test]
async fn overview_rejects_bad_range() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/overview?range=1y").await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid range"));
}

#[tokio::test]
async fn timeseries_buckets_a_semantic_metric() {
    let srv = spawn_server().await;
    let (status, body) = get(
        &srv.base,
        "/api/timeseries?metric=hypothesis_pass_rate&interval=1h&range=24h",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let points = body["points"].as_array().unwrap();
    assert!(!points.is_empty());
    assert!(points[0]["bucket_s"].as_i64().unwrap() > 0);
    assert!(points[0]["value"].is_number());
}

#[tokio::test]
async fn timeseries_rejects_unknown_metric() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/timeseries?metric=nope").await;
    assert_eq!(status, 404);
    assert!(body["error"].as_str().unwrap().contains("available:"));
}

#[tokio::test]
async fn experiments_lists_filters_and_searches() {
    let srv = spawn_server().await;

    let (status, body) = get(&srv.base, "/api/experiments?range=24h").await;
    assert_eq!(status, 200, "{body}");
    let rows = body["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    // Newest first; outcome joined from the completed log.
    assert_eq!(rows[0]["id"], "exp-fail");
    assert_eq!(rows[0]["status"], "Deviated");
    assert_eq!(rows[1]["status"], "Completed");
    assert_eq!(rows[0]["faults"], "injection");

    let (_, body) = get(&srv.base, "/api/experiments?outcome=deviated").await;
    let rows = body["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "exp-fail");

    let (_, body) = get(&srv.base, "/api/experiments?target=database").await;
    assert_eq!(body["experiments"].as_array().unwrap().len(), 2);
    let (_, body) = get(&srv.base, "/api/experiments?target=nope").await;
    assert_eq!(body["experiments"].as_array().unwrap().len(), 0);

    let (_, body) = get(&srv.base, "/api/experiments?fault=injection").await;
    assert_eq!(body["experiments"].as_array().unwrap().len(), 2);

    let (_, body) = get(&srv.base, "/api/experiments?q=cache").await;
    let rows = body["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "cache-stampede");
}

#[tokio::test]
async fn experiment_detail_returns_spans_logs_metrics() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/experiments/exp-pass").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["experiment"]["name"], "pg-failover");
    assert_eq!(body["experiment"]["status"], "Completed");
    assert_eq!(body["spans"].as_array().unwrap().len(), 2);
    assert_eq!(body["logs"].as_array().unwrap().len(), 2);
    // Regression: a log's real timestamp must survive ingest → store → API
    // verbatim (the epoch-0 bug rendered every log as 1970 in the UI).
    assert_eq!(body["logs"][0]["ts_ns"], serde_json::json!(srv.pass_log_ts));
    assert!(body["logs"][0]["ts_ns"].as_i64().unwrap() > 1_700_000_000_000_000_000);
    assert!(
        !body["metrics"].as_array().unwrap().is_empty(),
        "metric_sums rows join on experiment_name"
    );

    let (status, _) = get(&srv.base, "/api/experiments/no-such-id").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn dimensions_lists_distinct_filter_values() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/dimensions").await;
    assert_eq!(status, 200, "{body}");
    let outcomes: Vec<&str> = body["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .unwrap();
    assert_eq!(outcomes, ["Completed", "Deviated"]);
    assert_eq!(body["targets"], serde_json::json!(["database"]));
    assert_eq!(body["faults"], serde_json::json!(["injection"]));
    assert_eq!(
        body["experiments"],
        serde_json::json!(["cache-stampede", "pg-failover"])
    );
}

#[tokio::test]
async fn metrics_lists_semantic_definitions() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/metrics").await;
    assert_eq!(status, 200);
    let names: Vec<&str> = body["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert!(names.contains(&"hypothesis_pass_rate"), "{names:?}");
}

#[tokio::test]
async fn ask_answers_golden_questions_without_llm() {
    let srv = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/api/ask", srv.base))
        .json(&serde_json::json!({"question": "How many experiments ran?"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["configured"], true);
    assert_eq!(body["source"], "golden");
    assert_eq!(body["rows"][0]["experiments"], 2);
}

#[tokio::test]
async fn ask_degrades_gracefully_without_llm() {
    let srv = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/api/ask", srv.base))
        .json(&serde_json::json!({"question": "some question with no golden answer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body, serde_json::json!({"configured": false}));
}

#[tokio::test]
async fn generate_report_renders_stores_and_lists() {
    let srv = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/api/reports/generate", srv.base))
        .json(&serde_json::json!({"metric": "experiment_count"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let name = body["name"].as_str().unwrap();
    assert!(name.starts_with("manual_experiment_count_"), "{name}");
    assert!(body["bytes"].as_u64().unwrap() > 0);

    // It lands in the list…
    let (_, list) = get(&srv.base, "/api/reports").await;
    let names: Vec<&str> = list["reports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert!(names.contains(&name), "{names:?}");
    // …and is served as HTML.
    let resp = reqwest::get(format!("{}/api/reports/{name}", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("experiment_count"));

    // Unknown metric → 404.
    let resp = reqwest::Client::new()
        .post(format!("{}/api/reports/generate", srv.base))
        .json(&serde_json::json!({"metric": "no_such_metric"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn reports_lists_and_serves_digest_files() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/reports").await;
    assert_eq!(status, 200);
    let reports = body["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["name"], "2026-01-01T00-00_digest.html");

    let resp = reqwest::get(format!(
        "{}/api/reports/2026-01-01T00-00_digest.html",
        srv.base
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "<html>digest</html>");

    // Path traversal is rejected.
    let name = "..%2F..%2FCargo.toml";
    let resp = reqwest::get(format!("{}/api/reports/{name}", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // The reports dir is where ApiState was told it is.
    assert!(srv
        .reports_dir
        .join("2026-01-01T00-00_digest.html")
        .exists());
}
