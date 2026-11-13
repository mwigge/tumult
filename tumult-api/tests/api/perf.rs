//! `GET /api/perf/{target}` — the performance regression verdict between
//! two `service.version`s of a target inside one environment, computed
//! from the `during` load samples of the runs that measured each version —
//! `POST /api/perf/{target}/verdicts`, which appends it to the history,
//! `GET /api/perf`, the targets with runs, and
//! `GET /api/perf/{target}/version`, the version that served a window.

use crate::common::*;
use serde_json::{json, Value};
use tumult_core::types::{
    ExperimentStatus, Journal, LoadExecution, LoadResult, LoadSample, LoadTool, TargetVersion,
    VersionSource,
};
use tumult_lake::SpanRow;

/// A span `service` itself emitted under `version` in `env`.
fn svc(service: &str, env: Option<&str>, id: &str, version: &str, ts: i64) -> SpanRow {
    SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-svc-{id}"),
        span_id: format!("span-svc-{id}"),
        span_name: "GET /checkout".into(),
        span_kind: "Server".into(),
        duration_ns: 1_000_000,
        status_code: "Ok".into(),
        service_name: service.into(),
        service_version: Some(version.into()),
        target_environment: env.map(str::to_string),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// A span the `checkout` service emitted under `version` in demo.
fn service_span(id: &str, version: &str, ts: i64) -> SpanRow {
    svc("checkout", Some("demo"), id, version, ts)
}

/// `service_span` in another environment.
fn span_in(env: &str, id: &str, version: &str, ts: i64) -> SpanRow {
    svc("checkout", Some(env), id, version, ts)
}

/// Spans of `version` every ten seconds over `[from, to]`, in `env`.
fn stream(env: &str, version: &str, from: i64, to: i64) -> Vec<SpanRow> {
    stream_of("checkout", Some(env), version, from, to)
}

fn stream_of(service: &str, env: Option<&str>, version: &str, from: i64, to: i64) -> Vec<SpanRow> {
    (0..)
        .map(|i| from + i * 10 * NS)
        .take_while(|ts| *ts <= to)
        .map(|ts| {
            svc(
                service,
                env,
                &format!("{service}-{env:?}-{version}-{ts}"),
                version,
                ts,
            )
        })
        .collect()
}

/// A tumult experiment root span against `target`, sixty seconds long.
fn root(target: &str, env: Option<&str>, id: &str, ts: i64) -> SpanRow {
    SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 60 * NS,
        status_code: "Unset".into(),
        service_name: "tumult".into(),
        service_version: Some("2.22.0".into()),
        experiment_id: Some(id.into()),
        experiment_name: Some("checkout-perf".into()),
        target_system: Some(target.into()),
        target_technology: Some("http".into()),
        target_environment: env.map(str::to_string),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// A root span against the `checkout` target in demo.
fn root_span(id: &str, ts: i64) -> SpanRow {
    root("checkout", Some("demo"), id, ts)
}

/// POST with no body and no credentials; returns (status, body).
async fn post(base: &str, path: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// A journal whose load result streamed `windows` samples around `p95`.
fn load_journal(id: &str, started: i64, p95: f64, windows: usize) -> Journal {
    let mut journal = tumult_lake::duckdb_store::sample_journal(id, ExperimentStatus::Completed);
    journal.started_at_ns = started;
    journal.ended_at_ns = started + 60 * NS;
    let samples: Vec<LoadSample> = (0..windows)
        .map(|i| {
            // Small deterministic jitter so the bootstrap has variance.
            #[allow(clippy::cast_precision_loss)]
            let jitter = ((i * 7) % 5) as f64 - 2.0;
            #[allow(clippy::cast_possible_wrap)]
            let offset = i as i64;
            LoadSample {
                ts_ns: started + offset * NS,
                duration_s: 1.0,
                vus: 10,
                throughput_rps: 100.0,
                latency_p50_ms: p95 / 2.0,
                latency_p95_ms: p95 + jitter,
                latency_p99_ms: p95 * 1.5,
                error_rate: 0.0,
                requests: 100,
            }
        })
        .collect();
    journal.load_result = Some(LoadResult {
        tool: LoadTool::Oha,
        started_at_ns: started,
        ended_at_ns: started + 60 * NS,
        duration_s: 60.0,
        vus: 10,
        throughput_rps: 100.0,
        latency_p50_ms: p95 / 2.0,
        latency_p95_ms: p95,
        latency_p99_ms: p95 * 1.5,
        error_rate: 0.0,
        total_requests: 100 * windows as u64,
        thresholds_met: true,
        samples,
        execution: LoadExecution::default(),
    });
    journal
}

/// `load_journal` with how its load was executed.
fn executed(mut journal: Journal, status: ExperimentStatus, execution: LoadExecution) -> Journal {
    journal.status = status;
    if let Some(load) = journal.load_result.as_mut() {
        load.execution = execution;
    }
    journal
}

/// `load_journal` with the version the run recorded.
fn recorded(mut journal: Journal, version: &str, version_source: VersionSource) -> Journal {
    journal.target_version = Some(TargetVersion {
        version: version.into(),
        version_source,
    });
    journal
}

/// Insert spans and journals.
async fn seed_journals(srv: &TestServer, spans: Vec<SpanRow>, journals: Vec<Journal>) {
    exec_write(srv, move |writer| {
        writer.insert_spans(&spans).map_err(|e| e.to_string())?;
        for journal in &journals {
            writer
                .ingest_journal(journal, None)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await;
}

/// Insert spans and one 40-window journal per `(run id, start, p95)`.
async fn seed(srv: &TestServer, spans: Vec<SpanRow>, runs: Vec<(&'static str, i64, f64)>) {
    let journals = runs
        .iter()
        .map(|(id, started, p95)| load_journal(id, *started, *p95, 40))
        .collect();
    seed_journals(srv, spans, journals).await;
}

/// Seed `checkout` under 1.4.0 (one run, p95 ≈ 100 ms) then 1.5.0 (one
/// run, p95 ≈ 150 ms), and `search` under a single version with no runs.
async fn seed_perf(srv: &TestServer) {
    let now = now_ns();
    seed(
        srv,
        vec![
            service_span("a1", "1.4.0", now - 3000 * NS),
            service_span("a2", "1.4.0", now - 2000 * NS),
            service_span("b1", "1.5.0", now - 1500 * NS),
            service_span("b2", "1.5.0", now - 100 * NS),
            root_span("perf-old", now - 2500 * NS),
            root_span("perf-new", now - 1000 * NS),
            svc("search", Some("demo"), "s1", "0.9.0", now - 50 * NS),
        ],
        vec![
            ("perf-old", now - 2500 * NS, 100.0),
            ("perf-new", now - 1000 * NS, 150.0),
        ],
    )
    .await;
}

fn runs_of(side: &Value) -> Vec<String> {
    side["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect()
}

fn error_of(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn perf_compares_the_latest_two_versions_and_stores_the_verdict() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;

    // The default is a pure read: nothing is stored, however often asked.
    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["stored"], false);
    let (_, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(body["history"].as_array().unwrap().len(), 0, "{body}");

    // Neither route takes a storing flag: it is refused, with the route
    // that does store, rather than silently ignored.
    for flag in ["store=true", "store=false", "store"] {
        let (status, body) = get(&srv.base, &format!("/api/perf/checkout?{flag}")).await;
        assert_eq!(status, 400, "{flag}: {body}");
        assert!(
            error_of(&body).contains("POST /api/perf/checkout/verdicts"),
            "{body}"
        );
        let (status, body) = post(&srv.base, &format!("/api/perf/checkout/verdicts?{flag}")).await;
        assert_eq!(status, 400, "POST {flag}: {body}");
        assert!(error_of(&body).contains("store"), "{body}");
    }
    let (_, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(body["history"].as_array().unwrap().len(), 0, "{body}");

    let (status, body) = post(&srv.base, "/api/perf/checkout/verdicts").await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["target"], "checkout");
    assert_eq!(body["probe"], "load.latency_p95_ms");
    assert_eq!(body["environment"], "demo");
    assert_eq!(body["selection"], "deployment_order");
    assert_eq!(body["range"], "30d");
    assert_eq!(body["include_partial"], false);
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["baseline"]["version"], "1.4.0");
    assert_eq!(body["baseline"]["runs"][0], "perf-old");
    assert_eq!(body["baseline"]["samples"], 40);
    assert_eq!(body["candidate"]["version"], "1.5.0");
    assert_eq!(body["candidate"]["runs"][0], "perf-new");
    // No run recorded its version, so both were read from the spans.
    assert_eq!(
        body["candidate"]["attribution"],
        json!({"declared": 0, "observed": 0, "inferred": 1})
    );
    assert_eq!(
        body["candidate"]["run_details"][0],
        json!({
            "experiment_id": "perf-new", "attribution": "inferred", "status": "completed",
            "truncated": false, "aborted": false, "included": true,
        })
    );
    let distribution = body["candidate"]["distribution"].as_array().unwrap();
    assert_eq!(distribution.len(), 21, "{body}");
    assert_eq!(distribution[0]["p"], 0);
    assert_eq!(distribution[20]["p"], 100);
    assert!(distribution[0]["value"].as_f64() <= distribution[20]["value"].as_f64());
    let comparison = &body["comparison"];
    assert_eq!(comparison["quantile"], "p95");
    assert!(comparison["delta"].as_f64().unwrap() > 40.0, "{body}");
    assert!(comparison["ci_low"].as_f64().unwrap() > comparison["margin"].as_f64().unwrap());
    assert!(comparison["ci_high"].as_f64().unwrap() >= comparison["ci_low"].as_f64().unwrap());
    assert_eq!(comparison["confidence"], 0.95);
    assert_eq!(body["stored"], true);
    assert_eq!(body["duplicate"], false);
    assert_eq!(body["history"].as_array().unwrap().len(), 0);
    let versions = body["versions_seen"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["version"], "1.5.0", "candidate first: {body}");
    assert!(versions[0].get("live").is_none(), "{body}");
    assert_eq!(body["unattributed_runs"]["count"], 0);
    assert!(body["reasons"][0]
        .as_str()
        .unwrap()
        .contains("above the margin"));

    // The verdict was appended: the next call sees it as history, and the
    // answer is deterministic.
    let (status, again) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["stored"], false);
    let history = again["history"].as_array().unwrap();
    assert_eq!(history.len(), 1, "{again}");
    assert_eq!(history[0]["verdict"], "regressed");
    assert_eq!(history[0]["candidate_version"], "1.5.0");
    assert_eq!(history[0]["envs"], json!(["demo"]));
    assert_eq!(again["comparison"], body["comparison"]);

    // A retry of the POST (the client timed out, the daemon had stored)
    // answers 200 and appends nothing; naming the environment the answer
    // was already for is the same comparison.
    for path in [
        "/api/perf/checkout/verdicts",
        "/api/perf/checkout/verdicts?env=demo",
    ] {
        let (status, retry) = post(&srv.base, path).await;
        assert_eq!(status, 200, "{path}: {retry}");
        assert_eq!(retry["stored"], true);
        assert_eq!(retry["duplicate"], true);
        assert_eq!(retry["history"].as_array().unwrap().len(), 1, "{retry}");
    }
    // Another question about the same evidence is another row.
    let (status, p50) = post(&srv.base, "/api/perf/checkout/verdicts?quantile=p50").await;
    assert_eq!(status, 201, "{p50}");
    let (_, after) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(after["history"].as_array().unwrap().len(), 2, "{after}");
}

#[tokio::test]
async fn perf_single_version_is_inconclusive_not_404() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    let (status, body) = get(&srv.base, "/api/perf/search").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive");
    assert!(body["baseline"].is_null());
    assert_eq!(body["candidate"]["version"], "0.9.0");
    assert!(body["reasons"][0]
        .as_str()
        .unwrap()
        .contains("only one service.version"));
    assert_eq!(body["stored"], false);

    // With nothing to compare there is no verdict to store.
    let (status, body) = post(&srv.base, "/api/perf/search/verdicts").await;
    assert_eq!(status, 409, "{body}");
    let (_, body) = get(&srv.base, "/api/perf/search").await;
    assert_eq!(body["history"].as_array().unwrap().len(), 0, "{body}");
}

#[tokio::test]
async fn perf_unknown_target_is_404_and_bad_options_are_400() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/perf/no-such-service").await;
    assert_eq!(status, 404, "{body}");
    assert!(body["error"].is_string());
    for bad in [
        "quantile=p42",
        "confidence=1",
        "range=forever",
        "range=0d",
        "include=everything",
        "baseline=%20",
        "candidate=",
        "baseline=1.0.0&candidate=1.0.0",
    ] {
        let (status, body) = get(&srv.base, &format!("/api/perf/checkout?{bad}")).await;
        assert_eq!(status, 400, "{bad}: {body}");
    }
    // An empty environment is a malformed filter, not "every environment".
    let (status, body) = get(&srv.base, "/api/perf/checkout?env=").await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&srv.base, "/api/perf/checkout/verdicts?env=%20").await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&srv.base, "/api/perf/no-such-service/verdicts").await;
    assert_eq!(status, 404, "{body}");
}

/// A service whose spans carry no environment is an environment of its
/// own: an unscoped read answers for it and says so, an environment
/// filter or a scoped principal does not see it — with a message that
/// says why.
#[tokio::test]
async fn perf_untagged_service_is_an_environment_of_its_own() {
    let srv = spawn_server().await;
    let now = now_ns();
    seed(
        &srv,
        vec![
            svc("ledger", None, "l1", "3.0.0", now - 300 * NS),
            svc("ledger", None, "l2", "3.1.0", now - 100 * NS),
        ],
        vec![],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/ledger").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 2);
    assert!(body["environment"].is_null(), "{body}");
    assert_eq!(body["environments_seen"], json!([null]));

    let (status, body) = get(&srv.base, "/api/perf/ledger?env=demo").await;
    assert_eq!(status, 404, "{body}");
    let error = error_of(&body);
    assert!(error.contains("in environment demo"), "{error}");
    assert!(error.contains("deployment.environment.name"), "{error}");

    // Fail closed for a scoped principal, with the same explanation.
    let scoped = add_scoped_token(&srv, "ledger-viewer", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/ledger", &scoped).await;
    assert_eq!(status, 404, "{body}");
    assert!(error_of(&body).contains("in environment demo"), "{body}");

    // A target nobody has seen at all says so plainly.
    let admin = add_scoped_token(&srv, "ledger-admin", "admin", &[]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/nothing", &admin).await;
    assert_eq!(status, 404, "{body}");
    assert!(!error_of(&body).contains("in environment"), "{body}");
}

#[tokio::test]
async fn perf_honours_quantile_probe_and_min_samples() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    let (status, body) = get(&srv.base, "/api/perf/checkout?quantile=p50").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["comparison"]["quantile"], "p50");
    assert_eq!(body["verdict"], "regressed", "{body}");

    // A series nobody wrote: both sides empty, so inconclusive.
    let (status, body) = get(&srv.base, "/api/perf/checkout?probe=nope").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive");
    assert_eq!(body["candidate"]["samples"], 0);
    assert_eq!(body["candidate"]["distribution"], json!([]));

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_samples=100").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive");
}

/// Scoped principals: `env` outside the scopes is a 403, and a prod-scoped
/// viewer sees neither the demo versions nor the demo runs — the target
/// does not exist for it — rather than leaking demo evidence.
#[tokio::test]
async fn perf_respects_environment_scopes() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    let token = add_scoped_token(&srv, "perf-viewer", "viewer", &["prod"]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout?env=demo", &token).await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout", &token).await;
    assert_eq!(status, 404, "{body}");
    let (status, body) = get_auth(&srv.base, "/api/perf", &token).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["targets"], json!([]), "{body}");

    let demo = add_scoped_token(&srv, "perf-demo", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout", &demo).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 2);
}

/// Storing is a mutation: a viewer may read the verdict but not append
/// it, and a stored row is visible only inside the environment it was
/// computed for.
#[tokio::test]
async fn perf_store_needs_operator_and_history_is_environment_scoped() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    let viewer = add_scoped_token(&srv, "perf-viewer", "viewer", &["demo"]).await;
    let none = Value::Null;
    let (status, body) = post_auth(
        &srv.base,
        "/api/perf/checkout/verdicts",
        &viewer,
        none.clone(),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    // The old storing read stores nothing for anyone: it is refused.
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout?store=true", &viewer).await;
    assert_eq!(status, 400, "{body}");

    let operator = add_scoped_token(&srv, "perf-operator", "operator", &["demo"]).await;
    let (status, body) = post_auth(
        &srv.base,
        "/api/perf/checkout/verdicts?env=prod",
        &operator,
        none.clone(),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_auth(&srv.base, "/api/perf/checkout/verdicts", &operator, none).await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["stored"], true);

    // The demo-scoped viewer sees the stored row, with its environment.
    let (_, body) = get_auth(&srv.base, "/api/perf/checkout", &viewer).await;
    let history = body["history"].as_array().unwrap();
    assert_eq!(history.len(), 1, "{body}");
    assert_eq!(history[0]["envs"], json!(["demo"]));

    // A principal scoped to demo and prod sees it too.
    let wide = add_scoped_token(&srv, "perf-wide", "viewer", &["demo", "prod"]).await;
    let (_, body) = get_auth(&srv.base, "/api/perf/checkout", &wide).await;
    assert_eq!(body["history"].as_array().unwrap().len(), 1, "{body}");
}

/// 1.0.0 is busy in prod while 1.1.0 is tried in demo. An unscoped read
/// answers for the one environment that has two versions to compare, and
/// neither side takes a run or a version from the other environment.
/// Once both environments can be compared the request must say which.
#[tokio::test]
async fn perf_compares_within_one_environment_and_never_across() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("prod", "1.0.0", now - 4000 * NS, now - NS);
    spans.extend(stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS));
    // Demo's 1.1.0 is quiet around its run: prod spoke last before it.
    spans.push(span_in("demo", "new-1", "1.1.0", now - 1500 * NS));
    spans.push(span_in("demo", "new-2", "1.1.0", now - 400 * NS));
    spans.push(root_span("demo-old", now - 2800 * NS));
    spans.push(root_span("demo-new", now - 1000 * NS));
    spans.push(root("checkout", Some("prod"), "prod-run", now - 700 * NS));
    seed(
        &srv,
        spans,
        vec![
            ("demo-old", now - 2800 * NS, 100.0),
            ("demo-new", now - 1000 * NS, 150.0),
            ("prod-run", now - 700 * NS, 100.0),
        ],
    )
    .await;

    for path in ["/api/perf/checkout", "/api/perf/checkout?env=demo"] {
        let (status, body) = get(&srv.base, path).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert_eq!(body["environment"], "demo", "{path}: {body}");
        assert_eq!(body["environments_seen"], json!(["demo", "prod"]));
        assert_eq!(body["candidate"]["version"], "1.1.0", "{body}");
        assert_eq!(runs_of(&body["candidate"]), vec!["demo-new"]);
        assert_eq!(body["baseline"]["version"], "1.0.0");
        assert_eq!(runs_of(&body["baseline"]), vec!["demo-old"], "{body}");
        assert_eq!(body["candidate"]["samples"], 40);
        assert_eq!(body["baseline"]["samples"], 40);
        assert_eq!(body["verdict"], "regressed", "{body}");
    }
    let (_, prod) = get(&srv.base, "/api/perf/checkout?env=prod").await;
    assert_eq!(prod["environment"], "prod");
    assert_eq!(prod["candidate"]["version"], "1.0.0");
    assert_eq!(runs_of(&prod["candidate"]), vec!["prod-run"]);
    assert!(prod["baseline"].is_null(), "{prod}");

    // Prod gets a second version: now two environments could answer.
    seed(
        &srv,
        vec![span_in("prod", "prod-new", "1.2.0", now - 50 * NS)],
        vec![],
    )
    .await;
    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 400, "{body}");
    let error = error_of(&body);
    assert!(error.contains("env="), "{error}");
    assert!(error.contains("demo") && error.contains("prod"), "{error}");
    let (status, body) = post(&srv.base, "/api/perf/checkout/verdicts").await;
    assert_eq!(status, 400, "{body}");
    // Versions only one of them has seen settle it.
    let (status, body) = get(
        &srv.base,
        "/api/perf/checkout?baseline=1.0.0&candidate=1.1.0",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["environment"], "demo");
    assert_eq!(body["selection"], "explicit");
    let (status, body) = get(&srv.base, "/api/perf/checkout?env=prod").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["version"], "1.2.0");
    assert_eq!(body["baseline"]["version"], "1.0.0");
    assert_eq!(runs_of(&body["baseline"]), vec!["prod-run"]);
}

/// A canary: both versions report from one environment, span by span. A
/// run belongs to the version that served most of it, an even split to
/// the version deployed latest, and neither answer depends on who spoke
/// last.
#[tokio::test]
async fn perf_resolves_versions_interleaved_within_one_environment() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 10 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1495 * NS, now - 5 * NS));
    // Each ten seconds of a window hold one span of either version; the
    // extra ones give one run to the old version and one to the new.
    for i in 0..20 {
        let old = now - 1000 * NS + i * NS + NS / 2;
        spans.push(span_in("demo", &format!("x-old-{i}"), "1.0.0", old));
        let new = now - 600 * NS + i * NS + NS / 2;
        spans.push(span_in("demo", &format!("x-new-{i}"), "1.1.0", new));
    }
    spans.push(root_span("before-the-canary", now - 2500 * NS));
    spans.push(root_span("mostly-old", now - 1000 * NS));
    spans.push(root_span("mostly-new", now - 600 * NS));
    // Six spans each inside [−300 s, −241 s].
    spans.push(SpanRow {
        duration_ns: 59 * NS,
        ..root_span("even", now - 300 * NS)
    });
    seed(
        &srv,
        spans,
        vec![
            ("before-the-canary", now - 2500 * NS, 100.0),
            ("mostly-old", now - 1000 * NS, 100.0),
            ("mostly-new", now - 600 * NS, 150.0),
            ("even", now - 300 * NS, 150.0),
        ],
    )
    .await;

    let (status, first) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["candidate"]["version"], "1.1.0", "{first}");
    assert_eq!(runs_of(&first["candidate"]), vec!["mostly-new", "even"]);
    assert_eq!(first["baseline"]["version"], "1.0.0");
    assert_eq!(
        runs_of(&first["baseline"]),
        vec!["before-the-canary", "mostly-old"]
    );
    assert_eq!(first["verdict"], "regressed", "{first}");

    // Now the old version speaks last: nothing about the answer moves.
    seed(
        &srv,
        vec![span_in("demo", "old-last", "1.0.0", now - NS)],
        vec![],
    )
    .await;
    let (_, second) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(second["candidate"], first["candidate"], "{second}");
    assert_eq!(second["baseline"], first["baseline"], "{second}");
    assert_eq!(second["comparison"], first["comparison"]);
}

/// The candidate build got no traffic until the load began, so its first
/// span is later than the run's start, where the old build spoke last.
#[tokio::test]
async fn perf_gives_a_run_to_a_candidate_that_was_idle_until_the_load() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS);
    spans.extend(stream("demo", "1.1.0", now - 990 * NS, now - 945 * NS));
    spans.push(root_span("old", now - 2500 * NS));
    spans.push(root_span("new", now - 1000 * NS));
    seed(
        &srv,
        spans,
        vec![
            ("old", now - 2500 * NS, 100.0),
            ("new", now - 1000 * NS, 150.0),
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["version"], "1.1.0");
    assert_eq!(runs_of(&body["candidate"]), vec!["new"], "{body}");
    assert_eq!(runs_of(&body["baseline"]), vec!["old"], "{body}");
    assert_eq!(body["verdict"], "regressed", "{body}");
}

/// Two identical reads with no new runs give the same verdict, whatever
/// the telemetry did in between: here the new build went quiet after its
/// run and only the old one kept reporting, minutes on end.
#[tokio::test]
async fn perf_verdict_does_not_move_when_only_the_old_version_keeps_speaking() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1500 * NS, now - 900 * NS));
    spans.push(root_span("old", now - 2500 * NS));
    spans.push(root_span("new", now - 1000 * NS));
    seed(
        &srv,
        spans,
        vec![
            ("old", now - 2500 * NS, 100.0),
            ("new", now - 1000 * NS, 150.0),
        ],
    )
    .await;
    let (status, first) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["candidate"]["version"], "1.1.0", "{first}");
    assert_eq!(first["verdict"], "regressed", "{first}");

    // Only the old version's last sight advances, by far more than any
    // liveness window; no run is added.
    seed(
        &srv,
        stream("demo", "1.0.0", now - 800 * NS, now - NS),
        vec![],
    )
    .await;
    let (status, second) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["verdict"], first["verdict"], "{second}");
    assert_eq!(second["reasons"], first["reasons"]);
    assert_eq!(second["candidate"], first["candidate"], "{second}");
    assert_eq!(second["baseline"], first["baseline"], "{second}");
    assert_eq!(second["comparison"], first["comparison"]);
    assert_eq!(second["selection"], "deployment_order");
}

/// The version a run recorded — declared, or observed when it ended — is
/// what the run measured, whatever the spans around it say now. Only a
/// run that recorded none is read from the spans, and is marked so.
#[tokio::test]
async fn perf_uses_the_version_a_run_recorded_before_any_span() {
    let srv = spawn_server().await;
    let now = now_ns();
    // The telemetry says 1.0.0 served everything.
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - NS);
    spans.push(span_in("demo", "new-seen", "1.1.0", now - 1900 * NS));
    spans.push(root_span("legacy", now - 2500 * NS));
    spans.push(root_span("declared", now - 1500 * NS));
    // A journal that reached the lake without its version: the root span
    // still carries what the runner froze.
    spans.push(SpanRow {
        span_attrs: vec![
            ("resilience.target.version".into(), "1.1.0".into()),
            ("resilience.target.version_source".into(), "observed".into()),
        ],
        ..root_span("on-the-span", now - 1000 * NS)
    });
    seed_journals(
        &srv,
        spans,
        vec![
            load_journal("legacy", now - 2500 * NS, 100.0, 40),
            recorded(
                load_journal("declared", now - 1500 * NS, 150.0, 40),
                "1.1.0",
                VersionSource::Declared,
            ),
            load_journal("on-the-span", now - 1000 * NS, 150.0, 40),
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["version"], "1.1.0", "{body}");
    assert_eq!(
        runs_of(&body["candidate"]),
        vec!["declared", "on-the-span"],
        "{body}"
    );
    assert_eq!(
        body["candidate"]["attribution"],
        json!({"declared": 1, "observed": 1, "inferred": 0})
    );
    assert_eq!(runs_of(&body["baseline"]), vec!["legacy"], "{body}");
    assert_eq!(
        body["baseline"]["attribution"],
        json!({"declared": 0, "observed": 0, "inferred": 1})
    );
    assert_eq!(
        body["baseline"]["run_details"][0]["attribution"],
        "inferred"
    );

    // A version only runs ever declared is still a version of the target.
    seed_journals(
        &srv,
        vec![root_span("declared-only", now - 500 * NS)],
        vec![recorded(
            load_journal("declared-only", now - 500 * NS, 150.0, 40),
            "2.0.0-rc.1",
            VersionSource::Declared,
        )],
    )
    .await;
    let (_, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(body["candidate"]["version"], "2.0.0-rc.1", "{body}");
    assert_eq!(body["baseline"]["version"], "1.1.0", "{body}");
    assert_eq!(body["runs_on_other_versions"], 1, "{body}");
}

/// A run is attributed from the target's spans of exactly its own
/// environment — a run that names none, from spans that name none — and
/// a run that cannot be attributed is counted, with the reason.
#[tokio::test]
async fn perf_counts_runs_it_cannot_attribute_and_says_why() {
    let srv = spawn_server().await;
    let now = now_ns();
    let hour = 3600 * NS;
    // `ledger` reports without an environment.
    let mut spans = stream_of("ledger", None, "3.0.0", now - 3000 * NS, now - 2000 * NS);
    spans.extend(stream_of(
        "ledger",
        None,
        "3.1.0",
        now - 1500 * NS,
        now - 100 * NS,
    ));
    spans.push(root("ledger", None, "untagged-old", now - 2500 * NS));
    spans.push(root("ledger", None, "untagged-new", now - 1000 * NS));
    spans.push(root(
        "ledger",
        Some("demo"),
        "ledger-in-demo",
        now - 800 * NS,
    ));
    // `cart` reports from demo only, and was silent for hours before one
    // of its runs.
    spans.extend(stream_of(
        "cart",
        Some("demo"),
        "1.0.0",
        now - 5 * hour,
        now - 4 * hour,
    ));
    spans.extend(stream_of(
        "cart",
        Some("demo"),
        "1.1.0",
        now - 500 * NS,
        now - 400 * NS,
    ));
    spans.push(root("cart", None, "cart-nowhere", now - 450 * NS));
    spans.push(root("cart", Some("demo"), "cart-stale", now - 2 * hour));
    spans.push(root(
        "cart",
        Some("demo"),
        "cart-near",
        now - 4 * hour + 600 * NS,
    ));
    seed(
        &srv,
        spans,
        vec![
            ("untagged-old", now - 2500 * NS, 100.0),
            ("untagged-new", now - 1000 * NS, 150.0),
            ("ledger-in-demo", now - 800 * NS, 150.0),
            ("cart-nowhere", now - 450 * NS, 150.0),
            ("cart-stale", now - 2 * hour, 100.0),
            ("cart-near", now - 4 * hour + 600 * NS, 100.0),
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/ledger").await;
    assert_eq!(status, 200, "{body}");
    assert!(body["environment"].is_null(), "{body}");
    assert_eq!(runs_of(&body["candidate"]), vec!["untagged-new"], "{body}");
    assert_eq!(runs_of(&body["baseline"]), vec!["untagged-old"], "{body}");
    assert_eq!(
        body["unattributed_runs"],
        json!({
            "count": 1,
            "reasons": {"no_target_telemetry_in_environment": 1},
            "runs": [{
                "experiment_id": "ledger-in-demo", "environment": "demo",
                "reason": "no_target_telemetry_in_environment",
            }],
        }),
        "{body}"
    );

    // The run that names no environment gets nothing from demo's spans,
    // though they cover its window; the last span before `cart-stale` is
    // two hours old, which says nothing about what served it; the one
    // ten minutes after the last span is still read from it.
    let (status, body) = get(&srv.base, "/api/perf/cart").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["environment"], "demo");
    assert!(runs_of(&body["candidate"]).is_empty(), "{body}");
    assert_eq!(runs_of(&body["baseline"]), vec!["cart-near"], "{body}");
    assert_eq!(body["unattributed_runs"]["count"], 2, "{body}");
    assert_eq!(
        body["unattributed_runs"]["reasons"],
        json!({"no_target_telemetry_in_environment": 1, "no_target_spans_near_run": 1}),
        "{body}"
    );
    // Asked for demo alone, the other environment's run is not in scope.
    let (_, body) = get(&srv.base, "/api/perf/cart?env=demo").await;
    assert_eq!(
        body["unattributed_runs"]["reasons"],
        json!({"no_target_spans_near_run": 1}),
        "{body}"
    );
}

/// Deployment order by first sight, ties by version number, and explicit
/// versions when the caller knows better.
#[tokio::test]
async fn perf_takes_explicit_versions_and_orders_ties_by_version_number() {
    let srv = spawn_server().await;
    let now = now_ns();
    seed(
        &srv,
        vec![
            service_span("a", "1.8.0", now - 3000 * NS),
            // Two builds first seen at the same instant.
            service_span("b", "1.9.0", now - 2000 * NS),
            service_span("c", "1.10.0", now - 2000 * NS),
        ],
        vec![],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["selection"], "deployment_order");
    assert_eq!(body["candidate"]["version"], "1.10.0", "{body}");
    assert_eq!(body["baseline"]["version"], "1.9.0", "{body}");
    let order: Vec<&str> = body["versions_seen"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["version"].as_str().unwrap())
        .collect();
    assert_eq!(order, vec!["1.10.0", "1.9.0", "1.8.0"]);

    let (status, body) = get(
        &srv.base,
        "/api/perf/checkout?baseline=1.10.0&candidate=1.8.0",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["selection"], "explicit");
    assert_eq!(body["candidate"]["version"], "1.8.0");
    assert_eq!(body["baseline"]["version"], "1.10.0");
    // One named: the other follows the deployment order around it.
    let (_, body) = get(&srv.base, "/api/perf/checkout?candidate=1.9.0").await;
    assert_eq!(body["selection"], "explicit");
    assert_eq!(body["baseline"]["version"], "1.8.0", "{body}");
    let (_, body) = get(&srv.base, "/api/perf/checkout?baseline=1.8.0").await;
    assert_eq!(body["candidate"]["version"], "1.10.0", "{body}");
    let (_, body) = get(&srv.base, "/api/perf/checkout?candidate=1.8.0").await;
    assert!(body["baseline"].is_null(), "{body}");

    let (status, body) = get(&srv.base, "/api/perf/checkout?candidate=9.9.9").await;
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("9.9.9"), "{body}");
    assert!(error_of(&body).contains("1.10.0"), "{body}");
}

/// A load that was cut short measured something else than the plan: its
/// samples stay out unless asked for, and the answer counts what it left
/// out.
#[tokio::test]
async fn perf_leaves_aborted_and_truncated_runs_out_unless_asked() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1500 * NS, now - NS));
    for (id, at) in [
        ("old", 2500),
        ("clean", 1000),
        ("halted", 800),
        ("short", 600),
    ] {
        spans.push(root_span(id, now - at * NS));
    }
    let windows = |planned: u32, completed: u32| LoadExecution {
        planned_windows: Some(planned),
        completed_windows: Some(completed),
        ..LoadExecution::default()
    };
    seed_journals(
        &srv,
        spans,
        vec![
            load_journal("old", now - 2500 * NS, 100.0, 40),
            executed(
                load_journal("clean", now - 1000 * NS, 101.0, 40),
                ExperimentStatus::Completed,
                windows(40, 40),
            ),
            executed(
                load_journal("halted", now - 800 * NS, 900.0, 10),
                ExperimentStatus::Halted,
                LoadExecution {
                    aborted: true,
                    ..windows(40, 10)
                },
            ),
            executed(
                load_journal("short", now - 600 * NS, 900.0, 20),
                ExperimentStatus::Deviated,
                windows(40, 20),
            ),
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["include_partial"], false);
    let candidate = &body["candidate"];
    assert_eq!(runs_of(candidate), vec!["clean"], "{body}");
    assert_eq!(candidate["samples"], 40);
    assert_eq!(
        candidate["partial"],
        json!({"truncated": 2, "aborted": 1, "halted": 1, "deviated": 1, "excluded": 2}),
        "{body}"
    );
    assert_eq!(
        candidate["windows"],
        json!({"planned": 40, "completed": 40, "unanswered": 0, "unanswered_share": 0.0})
    );
    let halted = &candidate["run_details"][1];
    assert_eq!(halted["experiment_id"], "halted", "{body}");
    assert_eq!(halted["included"], false);
    assert_eq!(halted["aborted"], true);
    assert_eq!(halted["status"], "halted");
    assert_eq!(
        body["baseline"]["partial"],
        json!({"truncated": 0, "aborted": 0, "halted": 0, "deviated": 0, "excluded": 0})
    );
    assert_eq!(body["verdict"], "unchanged", "{body}");

    let (status, body) = get(&srv.base, "/api/perf/checkout?include=partial").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["include_partial"], true);
    assert_eq!(
        runs_of(&body["candidate"]),
        vec!["clean", "halted", "short"]
    );
    assert_eq!(body["candidate"]["samples"], 70);
    assert_eq!(body["candidate"]["partial"]["excluded"], 0);
    assert_eq!(body["verdict"], "regressed", "{body}");
}

/// Windows in which nothing answered leave no latency behind. A side that
/// lost a material share of its windows that way cannot be called
/// unchanged on the strength of the ones that did answer.
#[tokio::test]
async fn perf_does_not_call_a_blacked_out_side_unchanged() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1500 * NS, now - NS));
    spans.push(root_span("old", now - 2500 * NS));
    spans.push(root_span("blackout", now - 1000 * NS));
    let execution = |unanswered: u32, lost: u32| LoadExecution {
        planned_windows: Some(40),
        completed_windows: Some(40 - lost),
        unanswered_windows: unanswered,
        lost_to_unanswered: lost,
        ..LoadExecution::default()
    };
    seed_journals(
        &srv,
        spans,
        vec![
            executed(
                load_journal("old", now - 2500 * NS, 100.0, 40),
                ExperimentStatus::Completed,
                execution(1, 0),
            ),
            executed(
                load_journal("blackout", now - 1000 * NS, 100.0, 30),
                ExperimentStatus::Completed,
                execution(10, 4),
            ),
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    // The run is whole — no window was truncated — so it is included.
    assert_eq!(runs_of(&body["candidate"]), vec!["blackout"], "{body}");
    assert_eq!(
        body["candidate"]["windows"],
        json!({"planned": 40, "completed": 36, "unanswered": 10, "unanswered_share": 0.25})
    );
    assert_eq!(body["baseline"]["windows"]["unanswered"], 1);
    assert_eq!(body["verdict"], "inconclusive", "{body}");
    let reason = body["reasons"][0].as_str().unwrap();
    assert!(reason.contains("10 of 40"), "{reason}");
    assert!(reason.contains("unanswered"), "{reason}");
    assert!(reason.contains("1.1.0"), "{reason}");
    // A verdict that is not comparable is stored as what it is.
    let (status, stored) = post(&srv.base, "/api/perf/checkout/verdicts").await;
    assert_eq!(status, 201, "{stored}");
    assert_eq!(stored["verdict"], "inconclusive");
}

/// Runs and versions are read inside a lookback, `range`, thirty days
/// unless asked otherwise.
#[tokio::test]
async fn perf_reads_only_the_requested_range() {
    let srv = spawn_server().await;
    let now = now_ns();
    let day = 86_400 * NS;
    let mut spans = stream("demo", "1.0.0", now - 3 * day, now - 3 * day + 3000 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1500 * NS, now - NS));
    spans.push(root_span("old", now - 3 * day + 1000 * NS));
    spans.push(root_span("new", now - 1000 * NS));
    // Forty days back: outside the default.
    spans.push(span_in("demo", "ancient", "0.9.0", now - 40 * day));
    seed(
        &srv,
        spans,
        vec![
            ("old", now - 3 * day + 1000 * NS, 100.0),
            ("new", now - 1000 * NS, 150.0),
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["range"], "30d");
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 2, "{body}");
    let since = body["since_ns"].as_i64().unwrap();
    let as_of = body["as_of_ns"].as_i64().unwrap();
    assert_eq!(as_of - since, 30 * day);

    let (status, body) = get(&srv.base, "/api/perf/checkout?range=6h").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["range"], "6h");
    assert_eq!(body["candidate"]["version"], "1.1.0");
    assert!(body["baseline"].is_null(), "{body}");
    assert_eq!(body["verdict"], "inconclusive");

    let (status, body) = get(&srv.base, "/api/perf/checkout?range=8w").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 3, "{body}");
}

/// The picker's list, and the question a runner asks when its run ends.
#[tokio::test]
async fn perf_lists_targets_and_resolves_the_version_that_served_a_window() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1500 * NS, now - NS));
    spans.extend(stream("prod", "0.9.0", now - 3000 * NS, now - NS));
    spans.push(root_span("old", now - 2500 * NS));
    spans.push(root_span("new", now - 1000 * NS));
    spans.push(root("checkout", Some("prod"), "prod-run", now - 700 * NS));
    spans.push(root("ledger", None, "ledger-run", now - 600 * NS));
    seed(&srv, spans, vec![]).await;

    let (status, body) = get(&srv.base, "/api/perf").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["range"], "30d");
    assert!(body["as_of_ns"].is_i64());
    // The shared fixture brings a `database` target of its own.
    let names: Vec<&str> = body["targets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["target"].as_str())
        .collect();
    assert_eq!(names, ["checkout", "database", "ledger"], "{body}");
    let targets = &body["targets"];
    assert_eq!(targets[0]["environments"], json!(["demo", "prod"]));
    assert_eq!(targets[0]["runs"], 3);
    assert_eq!(targets[0]["last_run_ns"], now - 700 * NS);
    assert_eq!(targets[2]["environments"], json!([null]));
    let (status, body) = get(&srv.base, "/api/perf?range=nope").await;
    assert_eq!(status, 400, "{body}");

    let window =
        |from: i64, to: i64| format!("from_ns={}&to_ns={}", now - from * NS, now - to * NS);
    let path = format!("/api/perf/checkout/version?env=demo&{}", window(2500, 2440));
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["target"], "checkout");
    assert_eq!(body["environment"], "demo");
    assert_eq!(body["version"], "1.0.0");
    let path = format!("/api/perf/checkout/version?env=prod&{}", window(1000, 940));
    let (_, body) = get(&srv.base, &path).await;
    assert_eq!(body["version"], "0.9.0");
    // Nothing spoke inside the window: nothing was observed.
    let path = format!("/api/perf/checkout/version?env=demo&{}", window(1900, 1600));
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["version"].is_null(), "{body}");
    // No environment: the spans that name none.
    let path = format!("/api/perf/checkout/version?{}", window(1000, 940));
    let (_, body) = get(&srv.base, &path).await;
    assert!(body["version"].is_null(), "{body}");
    assert!(body["environment"].is_null(), "{body}");

    let (status, body) = get(&srv.base, "/api/perf/checkout/version?env=demo").await;
    assert_eq!(status, 400, "{body}");
    let path = format!("/api/perf/checkout/version?env=demo&{}", window(940, 1000));
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 400, "{body}");

    // A scoped principal sees its environments only.
    let demo = add_scoped_token(&srv, "list-demo", "viewer", &["demo"]).await;
    let (_, body) = get_auth(&srv.base, "/api/perf", &demo).await;
    assert_eq!(body["targets"].as_array().unwrap().len(), 2, "{body}");
    assert_eq!(body["targets"][0]["target"], "checkout");
    assert_eq!(body["targets"][0]["environments"], json!(["demo"]));
    assert_eq!(body["targets"][0]["runs"], 2);
    assert_eq!(body["targets"][1]["target"], "database");
    let path = format!("/api/perf/checkout/version?env=prod&{}", window(1000, 940));
    let (status, body) = get_auth(&srv.base, &path, &demo).await;
    assert_eq!(status, 403, "{body}");
    let path = format!("/api/perf/checkout/version?{}", window(1000, 940));
    let (status, body) = get_auth(&srv.base, &path, &demo).await;
    assert_eq!(status, 403, "{body}");
}
