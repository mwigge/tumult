//! Shared harness: seeded store on an ephemeral port, HTTP helpers.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tumult_lake::{LogRow, MetricSumRow, SpanRow, Store};

pub const NS: i64 = 1_000_000_000;

/// Org fixture: `pg-failover` maps to data/db-team (critical), everything
/// else lands in `(unassigned)`.
pub const ORG_YAML: &str = "
nodes:
  - {name: data, kind: domain}
  - {name: db-team, parent: data}
assignments:
  - team: db-team
    targets: [\"pg-*\"]
    criticality: {pg-failover: critical}
";

pub fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64
}

/// Every activity succeeds after a sleep: 200ms for ordinary steps, 1s for
/// the `hold-*` steps of STOP_TOON — slow enough that an HTTP test can catch
/// a run mid-method for the e-stop endpoint, even on a loaded CI runner.
pub struct SlowNoopExecutor;
impl tumult_core::runner::ActivityExecutor for SlowNoopExecutor {
    fn execute(
        &self,
        activity: &tumult_core::types::Activity,
    ) -> tumult_core::runner::ActivityOutcome {
        let ms: u64 = if activity.name.starts_with("hold-") {
            1_000
        } else {
            200
        };
        std::thread::sleep(std::time::Duration::from_millis(ms));
        tumult_core::runner::ActivityOutcome {
            success: true,
            output: Some(format!("ok: {}", activity.name)),
            error: None,
            duration_ms: ms,
        }
    }
}

/// Three method steps plus one rollback, native providers (the test executor
/// intercepts everything regardless of provider).
pub const RUN_TOON: &str = r#"
title: api run test experiment
description: exercises the run endpoints
method[3]:
  - name: action-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
  - name: action-2
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
  - name: action-3
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
rollbacks[1]:
  - name: rollback-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
"#;

/// Same shape as RUN_TOON, but the SlowNoopExecutor holds each `hold-*`
/// step for 1s: the run stays in `running` for ~3s instead of ~600ms, so
/// the e-stop test's stop request cannot race the method's end on a loaded
/// CI runner (it did exactly that — stop landed after `passed` → 409).
pub const STOP_TOON: &str = r#"
title: api run stop test experiment
description: exercises e-stop against a genuinely running run
method[3]:
  - name: hold-1
    activity_type: action
    pause_after_s: 5
    provider:
      type: native
      plugin: test
      function: noop
  - name: hold-2
    activity_type: action
    pause_after_s: 5
    provider:
      type: native
      plugin: test
      function: noop
  - name: hold-3
    activity_type: action
    pause_after_s: 5
    provider:
      type: native
      plugin: test
      function: noop
rollbacks[1]:
  - name: rollback-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
"#;

/// Two experiments: `exp-pass` (Completed, 30 min ago) and `exp-fail`
/// (Deviated, 10 min ago), with tumult-style outcome logs and counters.
/// Returns the timestamp of exp-pass's `experiment.started` log so tests can
/// pin its survival end-to-end.
pub fn seed(db_path: &std::path::Path) -> i64 {
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

pub struct TestServer {
    pub base: String,
    pub _tmp: tempfile::TempDir,
    pub reports_dir: PathBuf,
    pub pass_log_ts: i64,
    pub db_path: PathBuf,
    pub ingest: tumult_ingest::IngestWriter,
}

/// Run one write on the test store through the harness's single-writer
/// channel (a second direct writer would lose the DuckDB single-writer lock).
pub async fn exec_write(
    srv: &TestServer,
    f: impl FnOnce(&tumult_lake::Writer) -> Result<(), String> + Send + 'static,
) {
    srv.ingest
        .write(tumult_ingest::Batch::Exec(Box::new(f)))
        .await
        .unwrap();
}

pub async fn spawn_server() -> TestServer {
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
        .join("../metrics")
        .canonicalize()
        .unwrap();
    // An LLM client pointing at a closed port: connection refused, which the
    // ask endpoint must surface as `{configured: false}`.
    let llm = Arc::new(tumult_intelligence::llm::OpenAiCompatClient::new(
        "http://127.0.0.1:1".into(),
        None,
        "test-model".into(),
    ));
    // A real single-writer channel so manual-evidence and run endpoints work,
    // plus a real bounded run queue over a slow noop executor (HTTP tests can
    // catch a run mid-method for the e-stop endpoint).
    let ingest = tumult_ingest::IngestWriter::spawn(Store::at(&db_path).writer().unwrap(), 16).0;
    let factory: tumult_ingest::runs::ExecutorFactory = Arc::new(|_env| Arc::new(SlowNoopExecutor));
    let run_queue = tumult_ingest::RunQueue::spawn(
        ingest.clone(),
        db_path.clone(),
        tumult_ingest::RunQueueConfig {
            concurrency: 1,
            queue_depth: 4,
            sweep_interval: std::time::Duration::from_secs(3600),
        },
        factory,
    );
    let state = tumult_api::ApiState::new(
        db_path.clone(),
        metrics_dir,
        reports_dir.clone(),
        llm,
        tumult_compliance::OrgTree::from_yaml(ORG_YAML).unwrap(),
        Some(ingest.clone()),
        Some(run_queue),
        None,
        false,
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
        db_path,
        ingest,
    }
}

pub async fn get(base: &str, path: &str) -> (u16, Value) {
    let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    (status, body)
}

/// Create a user directly through the writer channel; returns the user id.
/// (Fixtures go straight to the store; the API's own user endpoints are
/// exercised separately.)
pub async fn add_user(
    srv: &TestServer,
    username: &str,
    password: &str,
    role: &str,
    must_change: bool,
) -> String {
    let hash = tumult_auth::hash_password(password).unwrap();
    let row = tumult_lake::UserRow {
        id: format!("u-{username}"),
        username: username.into(),
        password_hash: hash,
        role: role.into(),
        must_change,
        disabled: false,
        created_at_ns: now_ns(),
    };
    let id = row.id.clone();
    exec_write(srv, move |w| w.create_user(&row).map_err(|e| e.to_string())).await;
    id
}

/// Mint a `kro_` token for a user directly in the store; returns the
/// plaintext token.
pub async fn add_token(srv: &TestServer, user_id: &str, name: &str) -> (String, String) {
    let token = tumult_auth::new_token();
    let row = tumult_lake::TokenRow {
        id: format!("t-{name}"),
        user_id: user_id.into(),
        name: name.into(),
        token_hash: tumult_auth::sha256_hex(&token),
        created_at_ns: now_ns(),
        last_used_at_ns: None,
        revoked: false,
        expires_at_ns: None,
    };
    let hash = row.token_hash.clone();
    exec_write(srv, move |w| {
        w.create_token(&row).map_err(|e| e.to_string())
    })
    .await;
    (token, hash)
}

/// POST a JSON body with a `kro_` bearer token; returns (status, body).
pub async fn post_auth(base: &str, path: &str, token: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// GET with a `kro_` bearer token; returns (status, body).
pub async fn get_auth(base: &str, path: &str, token: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// Create a user with the given role and env scopes and mint a `kro_` token
/// for it; returns the plaintext token (fixtures go straight to the store).
pub async fn add_scoped_token(
    srv: &TestServer,
    username: &str,
    role: &str,
    scopes: &[&str],
) -> String {
    let user_id = add_user(srv, username, &format!("{username}-password"), role, false).await;
    let owned: Vec<String> = scopes.iter().map(|s| (*s).to_string()).collect();
    let uid = user_id.clone();
    exec_write(srv, move |w| {
        w.set_user_env_scopes(&uid, &owned)
            .map_err(|e| e.to_string())
    })
    .await;
    let (token, _) = add_token(srv, &user_id, username).await;
    token
}
