//! End-to-end tests for the query API: a seeded store served on an ephemeral
//! port, every endpoint exercised over HTTP.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tumult_lake::{LogRow, MetricSumRow, SpanRow, Store};
use serde_json::{json, Value};

const NS: i64 = 1_000_000_000;

/// Org fixture: `pg-failover` maps to data/db-team (critical), everything
/// else lands in `(unassigned)`.
const ORG_YAML: &str = "
nodes:
  - {name: data, kind: domain}
  - {name: db-team, parent: data}
assignments:
  - team: db-team
    targets: [\"pg-*\"]
    criticality: {pg-failover: critical}
";

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
        // tumult tags the system under test on action spans.
        span_attrs: vec![
            (
                "resilience.target.name".to_string(),
                "postgres-1".to_string(),
            ),
            ("resilience.target.type".to_string(), "database".to_string()),
        ],
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
            // Extra rows so the logs explorer filters have something to bite
            // on: an error inside exp-fail's trace, and a warning from a
            // second service with no experiment linkage.
            LogRow {
                ts_ns: now - 590 * NS,
                severity_text: "ERROR".into(),
                body: "probe redis-latency failed: connection refused".into(),
                trace_id: Some("trace-exp-fail".into()),
                span_id: None,
                service_name: "tumult".into(),
                log_attrs: vec![("experiment_id".to_string(), "exp-fail".to_string())],
                resource_attrs: vec![],
            },
            LogRow {
                ts_ns: now - 580 * NS,
                severity_text: "WARN".into(),
                body: "target postgres-1 slow to recover".into(),
                trace_id: None,
                span_id: None,
                service_name: "chaos-agent".into(),
                log_attrs: vec![],
                resource_attrs: vec![],
            },
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
            // Raw-metric explorer fixtures: one sum split by an attr key,
            // exercising group_by without touching the KPI counters.
            MetricSumRow {
                ts_ns: now - 1700 * NS,
                metric_name: "demo.requests".into(),
                value: 10.0,
                attrs: vec![("route".to_string(), "/api".to_string())],
                ..MetricSumRow::default()
            },
            MetricSumRow {
                ts_ns: now - 1600 * NS,
                metric_name: "demo.requests".into(),
                value: 20.0,
                attrs: vec![("route".to_string(), "/web".to_string())],
                ..MetricSumRow::default()
            },
            MetricSumRow {
                ts_ns: now - 100 * NS,
                metric_name: "demo.requests".into(),
                value: 5.0,
                attrs: vec![("route".to_string(), "/api".to_string())],
                ..MetricSumRow::default()
            },
        ])
        .unwrap();

    writer
        .insert_metric_gauges(&[tumult_lake::MetricGaugeRow {
            ts_ns: now - 300 * NS,
            metric_name: "demo.cpu.usage".into(),
            value: 0.5,
            ..Default::default()
        }])
        .unwrap();
    // 4 observations: 1 below 100, 2 in [100,200), 1 at/above 200.
    writer
        .insert_metric_histograms(&[tumult_lake::MetricHistogramRow {
            ts_ns: now - 400 * NS,
            metric_name: "demo.latency".into(),
            count: 4,
            sum: 600.0,
            min: Some(50.0),
            max: Some(210.0),
            bucket_counts: vec![1, 2, 1],
            explicit_bounds: vec![100.0, 200.0],
            ..Default::default()
        }])
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
    let llm = Arc::new(tumult_intelligence::llm::OpenAiCompatClient::new(
        "http://127.0.0.1:1".into(),
        None,
        "test-model".into(),
    ));
    let state = tumult_api::ApiState::new(
        db_path.clone(),
        metrics_dir,
        reports_dir.clone(),
        llm,
        tumult_compliance::OrgTree::from_yaml(ORG_YAML).unwrap(),
        // A real single-writer channel so manual-evidence endpoints work.
        Some(tumult_ingest::IngestWriter::spawn(Store::at(&db_path).writer().unwrap(), 16).0),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, tumult_api::router(state))
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

#[tokio::test]
async fn logs_lists_filters_and_searches() {
    let srv = spawn_server().await;

    let (status, body) = get(&srv.base, "/api/logs?range=24h").await;
    assert_eq!(status, 200, "{body}");
    let rows = body["logs"].as_array().unwrap();
    assert_eq!(rows.len(), 6, "{rows:?}");
    // Newest first: the WARN row (10 min ago) leads.
    assert_eq!(rows[0]["severity_text"], "WARN");
    assert_eq!(rows[0]["service_name"], "chaos-agent");
    // experiment_id is lifted out of log_attrs for UI linking.
    assert_eq!(rows[1]["experiment_id"], "exp-fail");

    // Severity is a case-insensitive exact match, not a substring search.
    let (_, body) = get(&srv.base, "/api/logs?severity=error").await;
    let rows = body["logs"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["severity_text"], "ERROR");
    let (_, body) = get(&srv.base, "/api/logs?severity=err").await;
    assert_eq!(body["logs"].as_array().unwrap().len(), 0);

    let (_, body) = get(&srv.base, "/api/logs?service=chaos-agent").await;
    let rows = body["logs"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["severity_text"], "WARN");

    // Free-text search matches the body, %/_ in the query stay literal.
    let (_, body) = get(&srv.base, "/api/logs?q=connection+refused").await;
    let rows = body["logs"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["body"].as_str().unwrap().contains("redis-latency"));
    let (_, body) = get(&srv.base, "/api/logs?q=%25").await; // literal "%"
    assert_eq!(body["logs"].as_array().unwrap().len(), 0);

    // Limit is honoured and capped.
    let (_, body) = get(&srv.base, "/api/logs?limit=2").await;
    assert_eq!(body["logs"].as_array().unwrap().len(), 2);
    let (_, body) = get(&srv.base, "/api/logs?limit=99999").await;
    assert_eq!(body["logs"].as_array().unwrap().len(), 6);

    let (status, body) = get(&srv.base, "/api/logs?range=1y").await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid range"));
    let (status, _) = get(&srv.base, "/api/logs?range=24h&limit=0").await;
    // limit clamps to >= 1 rather than erroring.
    assert_eq!(status, 200);
}

#[tokio::test]
async fn logs_volume_buckets_counts_per_severity() {
    let srv = spawn_server().await;

    let (status, body) = get(&srv.base, "/api/logs/volume?range=24h&interval=1h").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["bucket_s"], 3600);
    let rows = body["rows"].as_array().unwrap();
    // All seed logs land in one 1h bucket, split by severity.
    let total: u64 = rows.iter().map(|r| r["count"].as_u64().unwrap()).sum();
    assert_eq!(total, 6, "{rows:?}");
    let count_of = |sev: &str| {
        rows.iter()
            .filter(|r| r["severity"] == sev)
            .map(|r| r["count"].as_u64().unwrap())
            .sum::<u64>()
    };
    assert_eq!(count_of("INFO"), 4);
    assert_eq!(count_of("ERROR"), 1);
    assert_eq!(count_of("WARN"), 1);
    assert!(rows[0]["ts"].as_i64().unwrap() > 1_700_000_000);

    // Filters apply to the volume too.
    let (_, body) = get(&srv.base, "/api/logs/volume?severity=error").await;
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["count"], 1);

    let (status, body) = get(&srv.base, "/api/logs/volume?interval=10m").await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid interval"));
}

#[tokio::test]
async fn traces_groups_spans_and_filters() {
    let srv = spawn_server().await;

    let (status, body) = get(&srv.base, "/api/traces").await;
    assert_eq!(status, 200, "{body}");
    let rows = body["traces"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    // Newest first; root span gives the name/service, outcome joins from
    // the experiment.completed log.
    assert_eq!(rows[0]["trace_id"], "trace-exp-fail");
    assert_eq!(rows[0]["root_name"], "resilience.experiment");
    assert_eq!(rows[0]["service_name"], "tumult");
    assert_eq!(rows[0]["span_count"], 2);
    assert_eq!(rows[0]["error_count"], 0);
    assert_eq!(rows[0]["status"], "Deviated");
    assert_eq!(rows[0]["experiment_id"], "exp-fail");
    assert_eq!(rows[1]["status"], "Completed");
    // Trace duration spans the whole tree (5s), not just one span.
    assert_eq!(rows[0]["duration_ns"], serde_json::json!(5 * NS));

    let (_, body) = get(&srv.base, "/api/traces?outcome=completed").await;
    let rows = body["traces"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["trace_id"], "trace-exp-pass");
    let (_, body) = get(&srv.base, "/api/traces?outcome=incomplete").await;
    assert_eq!(body["traces"].as_array().unwrap().len(), 0);
    let (status, _) = get(&srv.base, "/api/traces?outcome=bogus").await;
    assert_eq!(status, 400);

    let (_, body) = get(&srv.base, "/api/traces?service=tumult").await;
    assert_eq!(body["traces"].as_array().unwrap().len(), 2);
    let (_, body) = get(&srv.base, "/api/traces?service=nope").await;
    assert_eq!(body["traces"].as_array().unwrap().len(), 0);

    // Duration filter is trace-level, in milliseconds.
    let (_, body) = get(&srv.base, "/api/traces?min_duration_ms=4000").await;
    assert_eq!(body["traces"].as_array().unwrap().len(), 2);
    let (_, body) = get(&srv.base, "/api/traces?min_duration_ms=6000").await;
    assert_eq!(body["traces"].as_array().unwrap().len(), 0);

    let (status, _) = get(&srv.base, "/api/traces?range=1y").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn trace_durations_points_and_percentiles() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/traces/durations").await;
    assert_eq!(status, 200, "{body}");
    let points = body["points"].as_array().unwrap();
    assert_eq!(points.len(), 2);
    assert!(points[0]["trace_id"]
        .as_str()
        .unwrap()
        .starts_with("trace-"));
    assert_eq!(points[0]["duration_ms"], 5000.0);
    // Both root spans run exactly 5s, so every percentile is 5000ms.
    assert_eq!(body["p50_ms"], 5000.0);
    assert_eq!(body["p95_ms"], 5000.0);
    assert_eq!(body["p99_ms"], 5000.0);
}

#[tokio::test]
async fn trace_detail_returns_spans_and_logs() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/traces/trace-exp-pass").await;
    assert_eq!(status, 200, "{body}");
    let spans = body["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 2);
    assert!(spans[0]["parent_span_id"].is_null());
    assert_eq!(spans[0]["experiment_id"], "exp-pass");
    // The two tumult logs on this trace (the unlinked WARN row stays out).
    assert_eq!(body["logs"].as_array().unwrap().len(), 2);

    let (status, _) = get(&srv.base, "/api/traces/no-such-trace").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn metrics_catalog_lists_names_types_and_dimensions() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/metrics/catalog").await;
    assert_eq!(status, 200, "{body}");
    let metrics = body["metrics"].as_array().unwrap();
    let find = |name: &str| metrics.iter().find(|m| m["name"] == name).cloned();

    let sums = find("tumult.experiments.total").unwrap();
    assert_eq!(sums["types"], serde_json::json!(["sum"]));
    let gauge = find("demo.cpu.usage").unwrap();
    assert_eq!(gauge["types"], serde_json::json!(["gauge"]));
    let hist = find("demo.latency").unwrap();
    assert_eq!(hist["types"], serde_json::json!(["histogram"]));
    let requests = find("demo.requests").unwrap();
    assert_eq!(requests["dimensions"], serde_json::json!(["route"]));
}

#[tokio::test]
async fn metrics_query_aggregates_by_type() {
    let srv = spawn_server().await;

    // Sum over a 1d bucket: both tumult counters land in one point.
    let (status, body) = get(
        &srv.base,
        "/api/metrics/query?name=tumult.experiments.total&interval=1d",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["type"], "sum");
    let series = body["series"].as_array().unwrap();
    assert_eq!(series.len(), 1);
    assert!(series[0]["group"].is_null());
    let points = series[0]["points"].as_array().unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0]["v"], 2.0);

    // Gauge averages instead of summing.
    let (_, body) = get(
        &srv.base,
        "/api/metrics/query?name=demo.cpu.usage&interval=1d",
    )
    .await;
    assert_eq!(body["series"][0]["points"][0]["v"], 0.5);

    // Histogram: avg = sum/count, p95 clamps into the overflow bucket.
    let (_, body) = get(
        &srv.base,
        "/api/metrics/query?name=demo.latency&interval=1d",
    )
    .await;
    assert_eq!(body["type"], "histogram");
    let point = &body["series"][0]["points"][0];
    assert_eq!(point["avg"], 150.0);
    assert_eq!(point["p95"], 200.0);

    let (status, _) = get(&srv.base, "/api/metrics/query?name=no.such.metric").await;
    assert_eq!(status, 404);
    let (status, _) = get(&srv.base, "/api/metrics/query").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn metrics_query_splits_by_attribute_key() {
    let srv = spawn_server().await;
    let (status, body) = get(
        &srv.base,
        "/api/metrics/query?name=demo.requests&group_by=route&interval=1d",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let series = body["series"].as_array().unwrap();
    assert_eq!(series.len(), 2, "{series:?}");
    // Groups sort by label: /api before /web; /api sums 10 + 5.
    assert_eq!(series[0]["group"], "/api");
    assert_eq!(series[0]["points"][0]["v"], 15.0);
    assert_eq!(series[1]["group"], "/web");
    assert_eq!(series[1]["points"][0]["v"], 20.0);

    // Attribute keys become SQL — the charset is strict.
    let (status, _) = get(
        &srv.base,
        "/api/metrics/query?name=demo.requests&group_by=x%27%3BDROP",
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn topology_builds_service_and_target_graph() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/topology").await;
    assert_eq!(status, 200, "{body}");

    let nodes = body["nodes"].as_array().unwrap();
    let svc = nodes.iter().find(|n| n["id"] == "svc:tumult").unwrap();
    assert_eq!(svc["type"], "service");
    assert_eq!(svc["runs"], 4);
    assert_eq!(svc["errors"], 0);
    let tgt = nodes.iter().find(|n| n["id"] == "tgt:postgres-1").unwrap();
    assert_eq!(tgt["type"], "target");
    assert_eq!(tgt["runs"], 2);

    let edges = body["edges"].as_array().unwrap();
    // Service → target calls from the action spans' target attribute.
    assert!(edges.iter().any(|e| e["from_id"] == "svc:tumult"
        && e["to_id"] == "tgt:postgres-1"
        && e["weight"] == 2));
    // Intra-service parent→child hops with differing span names survive as
    // a self-loop (root experiment span → action span).
    assert!(edges
        .iter()
        .any(|e| e["from_id"] == "svc:tumult" && e["to_id"] == "svc:tumult"));

    let (status, _) = get(&srv.base, "/api/topology?range=1y").await;
    assert_eq!(status, 400);
    // An empty window yields an empty graph, not an error.
    let (status, body) = get(&srv.base, "/api/topology?range=24h").await;
    assert_eq!(status, 200);
    assert_eq!(body["nodes"].as_array().unwrap().len(), 2);
}

// ---------------------------------------------------------------------------
// /api/scores + /api/reports/v2/*

#[tokio::test]
async fn scores_returns_freshness_decayed_card() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/scores").await;
    assert_eq!(status, 200, "{body}");

    let experiments = body["experiments"].as_array().unwrap();
    assert_eq!(experiments.len(), 2, "{body}");
    let pass = experiments
        .iter()
        .find(|e| e["name"] == "pg-failover")
        .unwrap();
    assert_eq!(pass["score"], 100);
    assert_eq!(pass["state"], "passed");
    assert_eq!(pass["band"], "good");
    let fail = experiments
        .iter()
        .find(|e| e["name"] == "cache-stampede")
        .unwrap();
    assert_eq!(fail["score"], 50);
    assert_eq!(fail["state"], "failed");

    // Both experiments target "database": one target at (100+50)/2.
    let targets = body["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["target"], "database");
    assert_eq!(targets[0]["score"], 75.0);
    assert_eq!(body["portfolio"], 75.0);
    assert_eq!(body["band"], "good");
    assert!(body["delta"].is_number(), "delta should compare windows");

    let (status, _) = get(&srv.base, "/api/scores?range=1y").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn reports_v2_executive_digest_roundtrip() {
    let srv = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "executive-digest", "period": "7d"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    let id = meta["doc_id"].as_str().unwrap();
    assert!(id.starts_with("KRK-R1-"), "{id}");
    assert_eq!(meta["type"], "executive-digest");
    assert_eq!(meta["sha256"].as_str().unwrap().len(), 64);
    assert!(meta["bytes"].as_u64().unwrap() > 10_000, "{meta}");

    // Listed newest first.
    let (_, list) = get(&srv.base, "/api/reports/v2").await;
    let ids: Vec<&str> = list["reports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["doc_id"].as_str())
        .collect();
    assert!(ids.contains(&id), "{ids:?}");

    // PDF artifact has the magic bytes.
    let resp = reqwest::get(format!("{}/api/reports/v2/{id}/pdf", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    assert!(bytes.starts_with(b"%PDF"), "missing pdf magic");

    // HTML preview carries the document id.
    let resp = reqwest::get(format!("{}/api/reports/v2/{id}/html", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains(id));
}

#[tokio::test]
async fn reports_v2_game_day_validates_and_roundtrips() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    // Missing experiment_id → 400.
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "game-day"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    // Unknown experiment_id → 404.
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "game-day", "experiment_id": "nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    // A real run renders.
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "game-day", "experiment_id": "exp-pass"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    let id = meta["doc_id"].as_str().unwrap();
    assert!(id.starts_with("KRK-R3-"), "{id}");
    let resp = reqwest::get(format!("{}/api/reports/v2/{id}/html", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("pg-failover"));
}

#[tokio::test]
async fn reports_v2_evidence_pack_validates_framework() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "evidence-pack", "framework": "hipaa"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "evidence-pack", "framework": "dora"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    assert!(meta["doc_id"].as_str().unwrap().starts_with("KRK-R2-"));
    // The mandatory clause-verification footnote is in the HTML.
    let id = meta["doc_id"].as_str().unwrap();
    let resp = reqwest::get(format!("{}/api/reports/v2/{id}/html", srv.base))
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("verified against the licensed framework text"),
        "{html}"
    );
}

#[tokio::test]
async fn reports_v2_rejects_bad_document_ids() {
    let srv = spawn_server().await;
    let resp = reqwest::get(format!("{}/api/reports/v2/evil..id/pdf", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = reqwest::get(format!(
        "{}/api/reports/v2/KRK-R1-20200101-000000/pdf",
        srv.base
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn scores_tree_rolls_up_org_hierarchy() {
    let srv = spawn_server().await;
    // Root: data subtree holds pg-failover (critical, 100); (unassigned)
    // holds cache-stampede (50). Root = (3*100 + 1*50) / 4 = 87.5.
    let (status, body) = get(&srv.base, "/api/scores/tree").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["path"], "");
    assert!(
        (body["score"].as_f64().unwrap() - 87.5).abs() < 1e-9,
        "{}",
        body["score"]
    );
    assert_eq!(body["expected"], 2);
    assert_eq!(body["scored"], 2);
    assert_eq!(body["sparkline"].as_array().unwrap().len(), 10);
    let children = body["children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    // Weakest first: (unassigned) 50 before data 100.
    assert_eq!(children[0]["name"], "(unassigned)");
    assert_eq!(children[0]["score"], 50.0);
    assert_eq!(children[1]["name"], "data");
    assert_eq!(children[1]["score"], 100.0);

    // Drill one level: node=data.
    let (status, child) = get(&srv.base, "/api/scores/tree?node=data").await;
    assert_eq!(status, 200, "{child}");
    assert_eq!(child["score"], 100.0);
    assert_eq!(child["weakest"], "pg-failover");
    assert_eq!(child["children"][0]["name"], "db-team");
    assert_eq!(child["children"][0]["path"], "data/db-team");

    // Unknown node → 400.
    let (status, _) = get(&srv.base, "/api/scores/tree?node=nope").await;
    assert_eq!(status, 400);
    // Bad range → 400.
    let (status, _) = get(&srv.base, "/api/scores/tree?range=99y").await;
    assert_eq!(status, 400);
}

/// A valid manual record body (partial outcome, entered by `by`).
fn manual_body(by: &str) -> Value {
    json!({
        "experiment_name": "pg-manual-gameday",
        "exercise_type": "gameday",
        "executed_at_ns": now_ns() - 3600 * NS,
        "hypothesis": "Failover keeps p95 under 800ms",
        "method": "Disabled the primary; observed failover",
        "outcome_status": "partial",
        "hypothesis_met": true,
        "findings": "Failover worked; warm-up took 40s",
        "action_items": ["Pre-warm the secondary"],
        "target_system": "database",
        "target_environment": "production",
        "recovery_time_s": 40.0,
        "duration_s": 3600.0,
        "entered_by": by,
        "attestation": "I attest this record reflects the exercise as executed.",
        "framework_refs": ["DORA Art. 24(7)"]
    })
}

#[tokio::test]
async fn manual_evidence_lifecycle_end_to_end() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    let base = srv.base.clone();

    // Create a draft.
    let resp = client
        .post(format!("{base}/api/manual/experiments"))
        .json(&manual_body("alice"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let id = resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Listed as draft; not yet scored.
    let (status, list) = get(&base, "/api/manual/experiments?status=draft").await;
    assert_eq!(status, 200);
    assert_eq!(list["records"].as_array().unwrap().len(), 1);
    let (_, scores) = get(&base, "/api/scores").await;
    assert!(scores["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["name"] != "pg-manual-gameday"));

    // Edit the draft (full replace).
    let mut edited = manual_body("alice");
    edited["findings"] = json!("updated findings after replay");
    let resp = client
        .put(format!("{base}/api/manual/experiments/{id}"))
        .json(&edited)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Attach evidence (url ok, file kind rejected — no file storage).
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/attachments"))
        .json(&json!({
            "kind": "url",
            "uri": "https://wiki.example.com/gameday",
            "label": "write-up",
            "added_by": "alice"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/attachments"))
        .json(&json!({"kind": "file", "uri": "/etc/passwd", "added_by": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Verify before submit → 409.
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/verify"))
        .json(&json!({"reviewer": "bob"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // Submit locks the record; edits are then rejected.
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/submit"))
        .json(&json!({"by": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = client
        .put(format!("{base}/api/manual/experiments/{id}"))
        .json(&edited)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // Same-user verify → 400 (segregation of duties).
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/verify"))
        .json(&json!({"reviewer": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // A second user verifies.
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/verify"))
        .json(&json!({"reviewer": "bob", "note": "evidence reviewed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Detail: verified, audit chain intact, one attachment.
    let (status, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(status, 200);
    assert_eq!(detail["experiment"]["status"], "verified");
    assert_eq!(detail["experiment"]["reviewed_by"], "bob");
    assert_eq!(detail["attachments"].as_array().unwrap().len(), 1);
    let actions: Vec<&str> = detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["create", "edit", "attach", "submit", "verify"]);
    let audit = detail["audit"].as_array().unwrap();
    for w in audit.windows(2) {
        assert_eq!(w[0]["new_hash"], w[1]["prev_hash"]);
    }

    // Scored as manual partial (75) with origin.
    let (_, scores) = get(&base, "/api/scores").await;
    let manual = scores["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "pg-manual-gameday")
        .expect("manual record in scores")
        .clone();
    assert_eq!(manual["origin"], "manual");
    assert_eq!(manual["score"], 75);
    assert_eq!(manual["state"], "partial");

    // The experiments list unions manual rows; origin filter works.
    let (status, list) = get(&base, "/api/experiments?origin=manual").await;
    assert_eq!(status, 200);
    let rows = list["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["origin"], "manual");
    assert_eq!(rows[0]["review_status"], "verified");
    let (_, all) = get(&base, "/api/experiments").await;
    assert!(all["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["origin"] == "manual"));

    // The org tree sees the manual record too (it matches pg-*).
    let (_, tree) = get(&base, "/api/scores/tree?node=data/db-team").await;
    assert_eq!(tree["expected"], 2);

    // R1 executive digest renders with the By-domain section.
    let resp = client
        .post(format!("{base}/api/reports/v2/generate"))
        .json(&json!({"type": "executive-digest", "period": "7d"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    let id2 = meta["doc_id"].as_str().unwrap();
    let resp = reqwest::get(format!("{base}/api/reports/v2/{id2}/html"))
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    assert!(html.contains("By domain"), "{html}");
    assert!(html.contains("Evidence mix"), "{html}");
}

#[tokio::test]
async fn manual_reject_flow_and_import() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    let base = srv.base.clone();

    // Reject requires a note and lands in status rejected.
    let resp = client
        .post(format!("{base}/api/manual/experiments"))
        .json(&manual_body("alice"))
        .send()
        .await
        .unwrap();
    let id = resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    client
        .post(format!("{base}/api/manual/experiments/{id}/submit"))
        .json(&json!({"by": "alice"}))
        .send()
        .await
        .unwrap();
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/reject"))
        .json(&json!({"reviewer": "bob", "note": "  "}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/reject"))
        .json(&json!({"reviewer": "bob", "note": "insufficient evidence"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let (_, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(detail["experiment"]["status"], "rejected");

    // Bulk import lands as drafts under one batch and does not score.
    let mut second = manual_body("dan");
    second["experiment_name"] = json!("vpn-tabletop");
    second["outcome_status"] = json!("passed");
    let resp = client
        .post(format!("{base}/api/manual/import"))
        .json(&json!({
            "label": "q3-backfill",
            "records": [manual_body("carol"), second]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ids"].as_array().unwrap().len(), 2);
    let (_, drafts) = get(&base, "/api/manual/experiments?status=draft").await;
    assert_eq!(drafts["records"].as_array().unwrap().len(), 2);
    assert!(drafts["records"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["batch_id"] == body["batch_id"]));
    let (_, scores) = get(&base, "/api/scores").await;
    assert!(scores["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["name"] != "vpn-tabletop"));

    // Unknown id → 404; bad enum → 400.
    let (status, _) = get(&base, "/api/manual/experiments/01JNONE").await;
    assert_eq!(status, 404);
    let mut bad = manual_body("alice");
    bad["exercise_type"] = json!("war-game");
    let resp = client
        .post(format!("{base}/api/manual/experiments"))
        .json(&bad)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn lake_status_and_manual_export_trigger() {
    let srv = spawn_server().await;

    // Nothing exported yet.
    let (status, body) = get(&srv.base, "/api/lake/status").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["files"], 0);
    assert_eq!(body["retention_days"], 0);
    assert!(body["last_export_ns"].is_null());

    // Trigger one export pass (retention off: no delete, but the endpoint
    // still reports `deleted`).
    let resp = reqwest::Client::new()
        .post(format!("{}/api/lake/export", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let report: Value = resp.json().await.unwrap();
    let spans = report["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "spans")
        .unwrap()
        .clone();
    assert!(spans["rows"].as_u64().unwrap() >= 4, "{spans}");
    assert!(!spans["files"].as_array().unwrap().is_empty());
    assert!(spans["watermark_ns"].as_i64().unwrap() > 0);
    assert_eq!(report["deleted"], serde_json::json!({}));

    // Status now reports files, bytes and watermarks.
    let (status, body) = get(&srv.base, "/api/lake/status").await;
    assert_eq!(status, 200, "{body}");
    assert!(body["files"].as_u64().unwrap() > 0);
    assert!(body["bytes"].as_u64().unwrap() > 0);
    assert!(body["last_export_ns"].as_i64().unwrap() > 0);
    assert!(body["watermarks"]["spans"].as_i64().unwrap() > 0);

    // Idempotent re-run: no new rows, no new files.
    let resp = reqwest::Client::new()
        .post(format!("{}/api/lake/export", srv.base))
        .send()
        .await
        .unwrap();
    let second: Value = resp.json().await.unwrap();
    assert!(
        second["tables"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["rows"] == 0),
        "{second}"
    );
}

#[tokio::test]
async fn experiments_windows_returns_overlapping_runs() {
    let srv = spawn_server().await;
    // Whole seeded window: both runs.
    let (status, body) = get(
        &srv.base,
        "/api/experiments/windows?from=0&to=9000000000000000000",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "{runs:?}");
    let ids: Vec<&str> = runs.iter().filter_map(|r| r["id"].as_str()).collect();
    assert!(ids.contains(&"exp-pass") && ids.contains(&"exp-fail"));
    assert!(runs[0]["start_ns"].as_i64().unwrap() > 0);
    assert!(runs[0]["end_ns"].as_i64().unwrap() > runs[0]["start_ns"].as_i64().unwrap());
    assert!(runs
        .iter()
        .all(|r| r["outcome"].is_string() || r["outcome"].is_null()));

    // Narrow window overlapping only exp-fail (started ~10 min ago).
    let now = now_ns();
    let from = now - 900 * NS;
    let (status, body) = get(
        &srv.base,
        &format!("/api/experiments/windows?from={from}&to={now}"),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["id"], "exp-fail");

    // Window before everything: nothing overlaps; bad params: 400.
    let (_, body) = get(&srv.base, "/api/experiments/windows?from=1&to=2").await;
    assert_eq!(body["runs"].as_array().unwrap().len(), 0);
    let (status, _) = get(&srv.base, "/api/experiments/windows?from=5").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn logs_attr_click_to_filter() {
    let srv = spawn_server().await;
    // filter-for: only logs carrying experiment_id=exp-pass.
    let (status, body) = get(&srv.base, "/api/logs?attr=experiment_id%3Dexp-pass").await;
    assert_eq!(status, 200, "{body}");
    let logs = body["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 2, "{logs:?}");
    assert!(logs
        .iter()
        .all(|l| l["log_attrs"]["experiment_id"] == "exp-pass"));

    // filter-out: drops exp-pass's two logs, keeps the rest.
    let (status, body) = get(&srv.base, "/api/logs?attr_not=experiment_id%3Dexp-pass").await;
    assert_eq!(status, 200, "{body}");
    let logs = body["logs"].as_array().unwrap();
    assert!(logs.len() >= 4, "{logs:?}");
    assert!(logs
        .iter()
        .all(|l| l["log_attrs"]["experiment_id"] != "exp-pass"));

    // malformed: 400.
    let (status, _) = get(&srv.base, "/api/logs?attr=noequals").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn traces_attr_click_to_filter() {
    let srv = spawn_server().await;
    // Seed spans carry no span_attrs; filter-for matches nothing…
    let (status, body) = get(&srv.base, "/api/traces?attr=resilience.suite%3Ddemo").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["traces"].as_array().unwrap().len(), 0);
    // …and filter-out keeps everything.
    let (status, body) = get(&srv.base, "/api/traces?attr_not=resilience.suite%3Ddemo").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["traces"].as_array().unwrap().len(), 2, "{body}");
}
