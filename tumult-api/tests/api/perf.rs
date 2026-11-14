//! `GET /api/perf/{target}` — the performance regression verdict between
//! two `service.version`s of a target inside one environment, computed
//! from the `during` load samples of the runs that measured each version —
//! `POST /api/perf/{target}/verdicts`, which appends it to the history,
//! `GET /api/perf`, the targets with runs, and
//! `GET /api/perf/{target}/version`, the version that served a window.

use crate::common::*;
use serde_json::{json, Value};
use tumult_core::types::{
    ExperimentStatus, Journal, LoadExecution, LoadResult, LoadSample, LoadTool, RequestCounts,
    TargetVersion, VersionSource,
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
                counts: None,
                request_timeout_ms: None,
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
        shares: Vec::new(),
    });
    journal
}

/// `load_journal` with what became of the run's requests.
fn counted(mut journal: Journal, sent: u64, ok: u64, timed_out: u64) -> Journal {
    if let Some(load) = journal.load_result.as_mut() {
        load.execution.requests = Some(RequestCounts {
            sent,
            ok,
            timed_out,
            http_5xx: sent - ok - timed_out,
            ..RequestCounts::default()
        });
    }
    journal
}

/// `load_journal` whose requests were given `timeout_ms`; every `nth`
/// window's p95 fell among the requests that timed out.
fn timing_out(mut journal: Journal, timeout_ms: f64, nth: usize) -> Journal {
    if let Some(load) = journal.load_result.as_mut() {
        for (i, sample) in load.samples.iter_mut().enumerate() {
            sample.request_timeout_ms = Some(timeout_ms);
            if nth > 0 && i % nth == 0 {
                sample.latency_p95_ms = timeout_ms;
            }
        }
    }
    journal
}

/// Three whole runs `<prefix>-1` … `<prefix>-3` that declared `version`,
/// a hundred seconds apart from `first`: their root spans and journals.
fn declared_runs(
    prefix: &str,
    version: &str,
    first: i64,
    p95: f64,
) -> (Vec<SpanRow>, Vec<Journal>) {
    (0..3)
        .map(|i| {
            let id = format!("{prefix}-{}", i + 1);
            let started = first + i * 100 * NS;
            (
                root_span(&id, started),
                recorded(
                    load_journal(&id, started, p95, 40),
                    version,
                    VersionSource::Declared,
                ),
            )
        })
        .unzip()
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

/// Seed `checkout` under 1.4.0 (three runs, p95 ≈ 100 ms) then 1.5.0
/// (three runs, p95 ≈ 150 ms), and `search` under a single version with
/// no runs.
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
            root_span("perf-old-2", now - 2400 * NS),
            root_span("perf-old-3", now - 2300 * NS),
            root_span("perf-new", now - 1000 * NS),
            root_span("perf-new-2", now - 900 * NS),
            root_span("perf-new-3", now - 800 * NS),
            svc("search", Some("demo"), "s1", "0.9.0", now - 50 * NS),
        ],
        vec![
            ("perf-old", now - 2500 * NS, 100.0),
            ("perf-old-2", now - 2400 * NS, 100.0),
            ("perf-old-3", now - 2300 * NS, 100.0),
            ("perf-new", now - 1000 * NS, 150.0),
            ("perf-new-2", now - 900 * NS, 150.0),
            ("perf-new-3", now - 800 * NS, 150.0),
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
    // The answer says which rule chose the pair, in a code and in words.
    assert_eq!(body["selection"], "latest_deployment");
    let rule = body["selection_rule"].as_str().unwrap();
    assert!(rule.contains("1.5.0") && rule.contains("1.4.0"), "{rule}");
    assert!(rule.contains("deployed latest"), "{rule}");
    assert_eq!(body["backfill"], "done");
    assert_eq!(body["range"], "30d");
    assert_eq!(body["include_partial"], false);
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["reason"], "latency", "{body}");
    assert_eq!(body["baseline"]["version"], "1.4.0");
    assert_eq!(
        runs_of(&body["baseline"]),
        vec!["perf-old", "perf-old-2", "perf-old-3"]
    );
    assert_eq!(body["baseline"]["samples"], 120);
    assert_eq!(body["candidate"]["version"], "1.5.0");
    assert_eq!(body["candidate"]["runs"][0], "perf-new");
    // No run recorded its version, so both were read from the spans.
    assert_eq!(
        body["candidate"]["attribution"],
        json!({"declared": 0, "observed": 0, "inferred": 3})
    );
    // These runs counted no requests: the answer says the failure share
    // is unknown rather than zero.
    assert_eq!(
        body["candidate"]["requests"],
        json!({"sent": 0, "failed": 0, "failure_share": null, "runs_without_counts": 3})
    );
    assert!(body["comparison"]["failure_share"].is_null(), "{body}");
    assert!(
        body["reason_codes"]
            .as_array()
            .unwrap()
            .contains(&json!("failure_data_missing")),
        "{body}"
    );
    assert_eq!(body["comparison"]["min_runs"], 3);
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
    assert_eq!(versions[0]["redeployed"], false);
    assert_eq!(
        versions[0]["last_deployed_ns"],
        versions[0]["first_seen_ns"]
    );
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
        "min_runs=0",
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

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_samples=500").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive");
    assert_eq!(body["reason"], "insufficient_evidence");

    // Three runs a side is the least by default; asking for more than
    // there are is inconclusive, and the same seed gives the same answer.
    let (status, body) = get(&srv.base, "/api/perf/checkout?min_runs=4").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive");
    assert_eq!(body["comparison"]["min_runs"], 4);
    assert!(body["reasons"].to_string().contains("3 run(s)"), "{body}");
    let (_, first) = get(&srv.base, "/api/perf/checkout?seed=7").await;
    let (_, second) = get(&srv.base, "/api/perf/checkout?seed=7").await;
    assert_eq!(first["comparison"]["seed"], 7);
    assert_eq!(first["comparison"], second["comparison"]);
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

    for path in [
        "/api/perf/checkout?min_runs=1",
        "/api/perf/checkout?env=demo&min_runs=1",
    ] {
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

/// A run is read from the requests the target served inside its window:
/// one version has to have served at least 95 % of them. A window two
/// versions shared says nothing about either and the run is left out,
/// with the reason; the spans a request fans out into are not counted.
#[tokio::test]
async fn perf_leaves_a_run_two_versions_served_unattributed() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 1010 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1495 * NS, now - 5 * NS));
    // Twenty more requests of the new build inside the last run's window,
    // one stray request of the old build, and a hundred spans the old
    // build's stray request fanned out into.
    for i in 0..20 {
        let at = now - 500 * NS + i * NS + NS / 2;
        spans.push(span_in("demo", &format!("x-new-{i}"), "1.1.0", at));
    }
    spans.push(span_in("demo", "stray", "1.0.0", now - 480 * NS + NS / 2));
    for i in 0..100 {
        spans.push(SpanRow {
            parent_span_id: Some("span-svc-stray".into()),
            span_kind: "Internal".into(),
            ..span_in(
                "demo",
                &format!("stray-child-{i}"),
                "1.0.0",
                now - 480 * NS + NS / 2 + i,
            )
        });
    }
    spans.push(root_span("old", now - 2500 * NS));
    spans.push(root_span("canary", now - 1400 * NS));
    spans.push(root_span("new", now - 500 * NS));
    spans.push(root_span("frozen", now - 300 * NS));
    seed_journals(
        &srv,
        spans,
        vec![
            load_journal("old", now - 2500 * NS, 100.0, 40),
            load_journal("canary", now - 1400 * NS, 120.0, 40),
            load_journal("new", now - 500 * NS, 150.0, 40),
            // The runner saw two versions when the run ended and said so.
            recorded(
                load_journal("frozen", now - 300 * NS, 150.0, 40),
                "1.1.0",
                VersionSource::Mixed,
            ),
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_runs=1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["version"], "1.1.0", "{body}");
    assert_eq!(runs_of(&body["candidate"]), vec!["new"], "{body}");
    assert_eq!(runs_of(&body["baseline"]), vec!["old"], "{body}");
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(
        body["unattributed_runs"],
        json!({
            "count": 2,
            "reasons": {"mixed_versions": 2},
            "runs": [
                {"experiment_id": "canary", "environment": "demo", "reason": "mixed_versions"},
                {"experiment_id": "frozen", "environment": "demo", "reason": "mixed_versions"},
            ],
        }),
        "{body}"
    );
}

/// The clock-skew padding is read only for a run whose own window is
/// silent: one request of the build before, a second ahead of the run,
/// does not make the run mixed.
#[tokio::test]
async fn perf_does_not_mix_a_run_by_a_request_in_the_padding() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS);
    spans.push(span_in("demo", "last-old", "1.0.0", now - 1001 * NS));
    for i in 0..10 {
        let at = now - 990 * NS + i * NS;
        spans.push(span_in("demo", &format!("in-new-{i}"), "1.1.0", at));
    }
    // A run whose own window is silent is still read with the padding.
    spans.push(span_in("demo", "late-new", "1.1.0", now - 739 * NS));
    spans.push(root_span("old", now - 2500 * NS));
    spans.push(root_span("new", now - 1000 * NS));
    spans.push(root_span("quiet", now - 800 * NS));
    seed(
        &srv,
        spans,
        vec![
            ("old", now - 2500 * NS, 100.0),
            ("new", now - 1000 * NS, 150.0),
            ("quiet", now - 800 * NS, 150.0),
        ],
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_runs=1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["unattributed_runs"]["count"], 0, "{body}");
    assert_eq!(runs_of(&body["candidate"]), vec!["new", "quiet"], "{body}");
    assert_eq!(runs_of(&body["baseline"]), vec!["old"], "{body}");
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

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_runs=1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["version"], "1.1.0");
    assert_eq!(runs_of(&body["candidate"]), vec!["new"], "{body}");
    assert_eq!(runs_of(&body["baseline"]), vec!["old"], "{body}");
    assert_eq!(body["verdict"], "regressed", "{body}");
}

/// Two identical reads with no new runs give the same verdict, whatever
/// the telemetry did in between: here the old build never stopped
/// reporting beside the new one and goes on, minutes on end, after the
/// new one went quiet. That is not a deployment. (An old build that
/// speaks again after a silence in which the new one spoke is one: see
/// `perf_orders_versions_by_latest_deployment_whatever_the_range`.)
#[tokio::test]
async fn perf_verdict_does_not_move_when_only_the_old_version_keeps_speaking() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 900 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1500 * NS, now - 900 * NS));
    let (roots, old) = declared_runs("old", "1.0.0", now - 2500 * NS, 100.0);
    spans.extend(roots);
    let (roots, new) = declared_runs("new", "1.1.0", now - 1400 * NS, 150.0);
    spans.extend(roots);
    seed_journals(&srv, spans, [old, new].concat()).await;
    let (status, first) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["candidate"]["version"], "1.1.0", "{first}");
    assert_eq!(first["verdict"], "regressed", "{first}");

    // Only the old version's last sight advances, by far more than any
    // liveness window; no run is added.
    seed(
        &srv,
        stream("demo", "1.0.0", now - 890 * NS, now - NS),
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
    assert_eq!(second["selection"], "latest_deployment");
    assert_eq!(second["versions_seen"][1]["redeployed"], false, "{second}");
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

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_runs=1").await;
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
    assert_eq!(body["selection"], "latest_deployment");
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
        ("halted-2", 700),
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
                load_journal("halted-2", now - 700 * NS, 900.0, 10),
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

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_runs=1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["include_partial"], false);
    let candidate = &body["candidate"];
    assert_eq!(runs_of(candidate), vec!["clean"], "{body}");
    assert_eq!(candidate["samples"], 40);
    assert_eq!(
        candidate["partial"],
        json!({"truncated": 3, "aborted": 2, "halted": 2, "deviated": 1, "excluded": 3}),
        "{body}"
    );
    assert_eq!(
        candidate["windows"],
        json!({
            "planned": 40, "completed": 40, "unanswered": 0, "lost": 0,
            "unanswered_share": 0.0, "censored": 0,
        })
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

    let (status, body) = get(&srv.base, "/api/perf/checkout?include=partial&min_runs=1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["include_partial"], true);
    assert_eq!(
        runs_of(&body["candidate"]),
        vec!["clean", "halted", "halted-2", "short"]
    );
    assert_eq!(body["candidate"]["samples"], 80);
    assert_eq!(body["candidate"]["partial"]["excluded"], 0);
    assert_eq!(body["verdict"], "regressed", "{body}");
}

/// Windows in which nothing answered leave no latency behind, so the
/// latencies of a side that went silent flatter it. A candidate that
/// fails to answer is worse, not unknown: it is regressed. A baseline
/// that failed to answer is no yardstick: nothing can be said.
#[tokio::test]
async fn perf_does_not_call_a_blacked_out_side_unchanged() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS);
    spans.extend(stream("demo", "1.1.0", now - 1500 * NS, now - 600 * NS));
    spans.extend(stream("demo", "1.2.0", now - 500 * NS, now - NS));
    for (id, at) in [("old", 2500), ("blackout", 1000), ("blackout-2", 300)] {
        spans.push(root_span(id, now - at * NS));
    }
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
            executed(
                load_journal("blackout-2", now - 300 * NS, 100.0, 30),
                ExperimentStatus::Completed,
                execution(8, 0),
            ),
        ],
    )
    .await;

    // The candidate went silent, the baseline did not: regressed.
    let pair = "baseline=1.0.0&candidate=1.1.0";
    let (status, body) = get(&srv.base, &format!("/api/perf/checkout?{pair}")).await;
    assert_eq!(status, 200, "{body}");
    // The run is whole — no window was truncated — so it is included.
    assert_eq!(runs_of(&body["candidate"]), vec!["blackout"], "{body}");
    assert_eq!(
        body["candidate"]["windows"],
        json!({
            "planned": 40, "completed": 36, "unanswered": 10, "lost": 4,
            "unanswered_share": 0.25, "censored": 0,
        })
    );
    assert_eq!(body["baseline"]["windows"]["unanswered"], 1);
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["reason"], "candidate_unanswered", "{body}");
    let reason = body["reasons"][0].as_str().unwrap();
    assert!(reason.contains("10 of 40"), "{reason}");
    assert!(reason.contains("unanswered"), "{reason}");
    assert!(reason.contains("1.1.0"), "{reason}");
    // The verdict is stored as what it is.
    let (status, stored) = post(&srv.base, &format!("/api/perf/checkout/verdicts?{pair}")).await;
    assert_eq!(status, 201, "{stored}");
    assert_eq!(stored["verdict"], "regressed");
    let (_, after) = get(&srv.base, &format!("/api/perf/checkout?{pair}")).await;
    assert_eq!(after["history"][0]["verdict"], "regressed", "{after}");

    // The other way round the yardstick is the one that went silent.
    let (status, body) = get(
        &srv.base,
        "/api/perf/checkout?baseline=1.1.0&candidate=1.0.0",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive", "{body}");
    assert_eq!(body["reason"], "baseline_unanswered", "{body}");
    assert!(
        body["reasons"][0]
            .as_str()
            .unwrap()
            .contains("baseline 1.1.0"),
        "{body}"
    );

    // Both went silent — the default pair here: nothing can be said.
    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["version"], "1.2.0", "{body}");
    assert_eq!(body["baseline"]["version"], "1.1.0", "{body}");
    assert_eq!(body["verdict"], "inconclusive", "{body}");
    assert_eq!(body["reason"], "both_unanswered", "{body}");
    assert!(body["reasons"].as_array().unwrap().len() >= 2, "{body}");
}

/// Three declared runs a side, the requests of each counted. With
/// `timeout`, the candidate's requests were given up on after that many
/// milliseconds and every nth window's p95 fell among those; the
/// baseline's were given two seconds.
async fn seed_counted(
    srv: &TestServer,
    old: (f64, [u64; 3]),
    new: (f64, [u64; 3]),
    timeout: Option<(f64, usize)>,
) {
    let now = now_ns();
    let mut spans = Vec::new();
    let mut journals = Vec::new();
    for (prefix, version, first, (p95, [sent, ok, timed_out])) in [
        ("old", "1.0.0", now - 2500 * NS, old),
        ("new", "1.1.0", now - 1000 * NS, new),
    ] {
        let (roots, runs) = declared_runs(prefix, version, first, p95);
        spans.extend(roots);
        for run in runs {
            let run = counted(run, sent, ok, timed_out);
            journals.push(match timeout {
                Some((ms, nth)) if prefix == "new" => timing_out(run, ms, nth),
                Some(_) => timing_out(run, 2000.0, 0),
                None => run,
            });
        }
    }
    seed_journals(srv, spans, journals).await;
}

/// The survivors of a build that hangs are fast. A candidate that left
/// four requests in ten unanswered and served the rest in 20 ms is not
/// improved: its failure share alone makes it regressed.
#[tokio::test]
async fn perf_calls_a_candidate_that_hangs_part_of_its_requests_regressed() {
    let srv = spawn_server().await;
    seed_counted(
        &srv,
        (100.0, [4000, 4000, 0]),
        (20.0, [4000, 2400, 1600]),
        None,
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["version"], "1.1.0", "{body}");
    // The latencies alone say the candidate is five times faster.
    assert!(
        body["comparison"]["delta"].as_f64().unwrap() < -70.0,
        "{body}"
    );
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["reason"], "failure_share", "{body}");
    assert_eq!(
        body["candidate"]["requests"],
        json!({"sent": 12000, "failed": 4800, "failure_share": 0.4, "runs_without_counts": 0})
    );
    assert_eq!(
        body["baseline"]["requests"],
        json!({"sent": 12000, "failed": 0, "failure_share": 0.0, "runs_without_counts": 0})
    );
    assert_eq!(
        body["comparison"]["failure_share"],
        json!({"baseline": 0.0, "candidate": 0.4, "delta": 0.4, "margin": 0.01})
    );
    let reason = body["reasons"][0].as_str().unwrap();
    assert!(reason.contains("4800 of 12000"), "{reason}");
    assert!(reason.contains("1.1.0"), "{reason}");
    assert!(
        !body["reason_codes"]
            .as_array()
            .unwrap()
            .contains(&json!("failure_data_missing")),
        "{body}"
    );
    let (status, stored) = post(&srv.base, "/api/perf/checkout/verdicts").await;
    assert_eq!(status, 201, "{stored}");
    assert_eq!(stored["verdict"], "regressed");
}

/// Requests that hang make a plan fall short, and a run whose plan fell
/// short is left out of the latencies. The requests it failed are not
/// left out with it: a candidate none of whose runs finished its plan
/// because a third of its requests hung is regressed, not unknown.
#[tokio::test]
async fn perf_counts_the_failed_requests_of_runs_whose_plan_fell_short() {
    let srv = spawn_server().await;
    let now = now_ns();
    let (mut spans, old) = declared_runs("old", "1.0.0", now - 2500 * NS, 100.0);
    let (roots, new) = declared_runs("new", "1.1.0", now - 1000 * NS, 20.0);
    spans.extend(roots);
    let short = || LoadExecution {
        planned_windows: Some(40),
        completed_windows: Some(20),
        ..LoadExecution::default()
    };
    let journals = old
        .into_iter()
        .map(|run| counted(run, 4000, 4000, 0))
        .chain(new.into_iter().map(|run| {
            let run = executed(run, ExperimentStatus::Deviated, short());
            counted(run, 2000, 1300, 700)
        }))
        .collect();
    seed_journals(&srv, spans, journals).await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    let candidate = &body["candidate"];
    assert_eq!(candidate["version"], "1.1.0", "{body}");
    assert!(runs_of(candidate).is_empty(), "{body}");
    assert_eq!(candidate["samples"], 0, "{body}");
    assert_eq!(candidate["partial"]["excluded"], 3, "{body}");
    assert_eq!(
        candidate["requests"],
        json!({"sent": 6000, "failed": 2100, "failure_share": 0.35, "runs_without_counts": 0})
    );
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["reason"], "failure_share", "{body}");
}

/// Both builds fail one request in twenty: the failure share says
/// nothing against the candidate, and the latencies decide.
#[tokio::test]
async fn perf_does_not_regress_on_equal_failure_shares() {
    let srv = spawn_server().await;
    seed_counted(
        &srv,
        (100.0, [4000, 3800, 50]),
        (100.0, [4000, 3800, 50]),
        None,
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "unchanged", "{body}");
    assert_eq!(body["reason"], "latency", "{body}");
    assert_eq!(body["comparison"]["failure_share"]["delta"], 0.0);
    assert_eq!(body["candidate"]["requests"]["failure_share"], 0.05);
}

/// A window whose percentile fell among the requests that timed out
/// reports the timeout: the true value is that or more. The window is
/// flagged, and a verdict that would rest on it being no more is not
/// given.
#[tokio::test]
async fn perf_flags_censored_windows() {
    let srv = spawn_server().await;
    // The candidate answers in 20 ms, except that in one window in ten
    // its p95 is a request that was given up on after 50 ms.
    seed_counted(
        &srv,
        (100.0, [4000, 4000, 0]),
        (20.0, [4000, 3980, 20]),
        Some((50.0, 10)),
    )
    .await;

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["windows"]["censored"], 12, "{body}");
    assert_eq!(body["baseline"]["windows"]["censored"], 0, "{body}");
    assert_eq!(body["candidate"]["p95"], 50.0, "{body}");
    // 50 ms against 100 ms would read as improved.
    assert!(
        body["comparison"]["ci_high"].as_f64().unwrap() < 0.0,
        "{body}"
    );
    assert_eq!(body["verdict"], "inconclusive", "{body}");
    assert_eq!(body["reason"], "censored", "{body}");
    let reason = body["reasons"][0].as_str().unwrap();
    assert!(reason.contains("12 of 120"), "{reason}");
    assert!(reason.contains("at least"), "{reason}");

    // With the censored side as the yardstick, "slower than at least
    // 50 ms" is not known to be slower either.
    let (status, body) = get(
        &srv.base,
        "/api/perf/checkout?baseline=1.1.0&candidate=1.0.0",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["baseline"]["windows"]["censored"], 12, "{body}");
    assert_eq!(body["verdict"], "inconclusive", "{body}");
    assert_eq!(body["reason"], "censored", "{body}");
    // Another series than a latency has no timeout to be censored at.
    let (_, body) = get(&srv.base, "/api/perf/checkout?probe=load.throughput_rps").await;
    assert_eq!(body["candidate"]["windows"]["censored"], 0, "{body}");
}

/// A version in a query string is held to the same bounds as one on a
/// span or a command line.
#[tokio::test]
async fn perf_refuses_a_version_outside_the_bounds() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    let long = "9".repeat(129);
    for bad in [
        format!("candidate={long}"),
        format!("baseline={long}"),
        "candidate=1.5.0%0A".to_string(),
        "baseline=1.4.0%1B%5B31m".to_string(),
    ] {
        let (status, body) = get(&srv.base, &format!("/api/perf/checkout?{bad}")).await;
        assert_eq!(status, 400, "{bad}: {body}");
        let error = error_of(&body);
        assert!(
            error.contains("128 bytes") || error.contains("control characters"),
            "{error}"
        );
        // What was sent is not echoed back.
        assert!(!error.contains(&long), "{error}");
        let (status, body) = post(&srv.base, &format!("/api/perf/checkout/verdicts?{bad}")).await;
        assert_eq!(status, 400, "POST {bad}: {body}");
    }
    let most = "9".repeat(128);
    let (status, body) = get(&srv.base, &format!("/api/perf/checkout?candidate={most}")).await;
    // Within the bounds: refused only because nobody has seen it.
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("was not seen"), "{body}");
}

/// A rollback: 1.4.0, then 1.5.0, then 1.4.0 again. The version deployed
/// latest is the candidate — 1.4.0, deployed again five days ago — and
/// neither that nor the order depends on how far back the samples are
/// read.
#[tokio::test]
async fn perf_orders_versions_by_latest_deployment_whatever_the_range() {
    let srv = spawn_server().await;
    let now = now_ns();
    let day = 86_400 * NS;
    let mut spans = vec![
        span_in("demo", "a1", "1.4.0", now - 40 * day),
        span_in("demo", "a2", "1.4.0", now - 21 * day),
        span_in("demo", "b1", "1.5.0", now - 20 * day),
        span_in("demo", "b2", "1.5.0", now - 6 * day),
        span_in("demo", "a3", "1.4.0", now - 5 * day),
        span_in("demo", "a4", "1.4.0", now - 3600 * NS),
    ];
    // What makes it a rollback and no stray request: it serves again.
    spans.extend(stream(
        "demo",
        "1.4.0",
        now - 5 * day,
        now - 5 * day + 300 * NS,
    ));
    let mut journals = Vec::new();
    for (prefix, version, first, p95) in [
        ("first", "1.4.0", now - 35 * day, 100.0),
        ("newer", "1.5.0", now - 10 * day, 150.0),
        ("again", "1.4.0", now - 2 * day, 100.0),
    ] {
        let (roots, runs) = declared_runs(prefix, version, first, p95);
        spans.extend(roots);
        journals.extend(runs);
    }
    seed_journals(&srv, spans, journals).await;

    let read = |range: &'static str| {
        let base = srv.base.clone();
        async move {
            let (status, body) = get(&base, &format!("/api/perf/checkout?range={range}")).await;
            assert_eq!(status, 200, "{range}: {body}");
            body
        }
    };
    let month = read("30d").await;
    let two = read("60d").await;
    for body in [&month, &two] {
        assert_eq!(body["selection"], "latest_deployment", "{body}");
        assert_eq!(body["candidate"]["version"], "1.4.0", "{body}");
        assert_eq!(body["baseline"]["version"], "1.5.0", "{body}");
        assert_eq!(body["verdict"], "improved", "{body}");
        let rule = body["selection_rule"].as_str().unwrap();
        assert!(rule.contains("deployed again"), "{rule}");
        let seen = body["versions_seen"].as_array().unwrap();
        assert_eq!(seen.len(), 2, "{body}");
        assert_eq!(seen[0]["version"], "1.4.0");
        assert_eq!(seen[0]["redeployed"], true, "{body}");
        assert_eq!(seen[0]["first_seen_ns"], now - 40 * day);
        assert_eq!(seen[0]["last_deployed_ns"], now - 5 * day);
        assert_eq!(seen[1]["version"], "1.5.0");
        assert_eq!(seen[1]["redeployed"], false, "{body}");
        assert_eq!(seen[1]["first_seen_ns"], now - 20 * day);
    }
    assert_eq!(month["versions_seen"], two["versions_seen"]);
    // The range bounds the samples, and only them.
    assert_eq!(
        runs_of(&month["candidate"]),
        vec!["again-1", "again-2", "again-3"]
    );
    assert_eq!(runs_of(&two["candidate"]).len(), 6, "{two}");
    assert_eq!(runs_of(&month["baseline"]), runs_of(&two["baseline"]));
}

/// 1.4.0 measured at 100 ms, then 1.5.0 at 150 ms and live, then 1.4.0
/// heard from once more at `straggler`.
async fn seed_straggler(srv: &TestServer, now: i64, new_until: i64, straggler: i64) {
    let hour = 3600 * NS;
    let mut spans = stream("demo", "1.4.0", now - 10 * hour, now - 9 * hour);
    spans.extend(stream("demo", "1.5.0", now - 8 * hour, new_until));
    spans.push(span_in("demo", "straggler", "1.4.0", straggler));
    let mut journals = Vec::new();
    for (prefix, version, first, p95) in [
        ("old", "1.4.0", now - 10 * hour, 100.0),
        ("new", "1.5.0", now - 8 * hour, 150.0),
    ] {
        let (roots, runs) = declared_runs(prefix, version, first, p95);
        spans.extend(roots);
        journals.extend(runs);
    }
    seed_journals(srv, spans, journals).await;
}

/// One request that still reached the old build does not make the old
/// build the latest deployment: the order is refused, whether the
/// request came a moment ago or the new build has been idle since, and
/// naming the candidate answers.
#[tokio::test]
async fn perf_does_not_order_versions_by_a_straggler_of_the_old_one() {
    let now = now_ns();
    let hour = 3600 * NS;
    for (what, new_until, straggler) in [
        ("a moment ago", now - 300 * NS, now - 100 * NS),
        ("an idle new build", now - 4 * hour, now - 2 * hour),
    ] {
        let srv = spawn_server().await;
        seed_straggler(&srv, now, new_until, straggler).await;
        let (status, body) = get(&srv.base, "/api/perf/checkout").await;
        assert_eq!(status, 400, "{what}: {body}");
        assert_eq!(body["reason"], "unorderable", "{what}: {body}");
        let error = error_of(&body);
        assert!(
            error.contains("1.4.0") && error.contains("1.5.0"),
            "{what}: {error}"
        );
        assert!(error.contains("candidate="), "{what}: {error}");
        let (status, body) = post(&srv.base, "/api/perf/checkout/verdicts").await;
        assert_eq!(status, 400, "{what}: {body}");

        // The gate names what it tests: 1.5.0, against the version it
        // replaced.
        let (status, body) = get(&srv.base, "/api/perf/checkout?candidate=1.5.0").await;
        assert_eq!(status, 200, "{what}: {body}");
        assert_eq!(body["candidate"]["version"], "1.5.0", "{what}: {body}");
        assert_eq!(body["baseline"]["version"], "1.4.0", "{what}: {body}");
        assert_eq!(body["verdict"], "regressed", "{what}: {body}");
    }
}

/// `range`, thirty days unless asked otherwise, bounds the runs whose
/// samples are compared. It does not bound the versions: which version
/// was deployed before which is the same however short the lookback.
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

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_runs=1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["range"], "30d");
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 3, "{body}");
    let since = body["since_ns"].as_i64().unwrap();
    let as_of = body["as_of_ns"].as_i64().unwrap();
    assert_eq!(as_of - since, 30 * day);

    // Six hours hold the candidate's run only. The baseline is still the
    // version deployed before it; it just has nothing to show.
    let (status, body) = get(&srv.base, "/api/perf/checkout?range=6h&min_runs=1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["range"], "6h");
    assert_eq!(body["candidate"]["version"], "1.1.0");
    assert_eq!(body["baseline"]["version"], "1.0.0", "{body}");
    assert_eq!(body["baseline"]["samples"], 0, "{body}");
    assert_eq!(body["verdict"], "inconclusive");
    assert_eq!(body["reason"], "insufficient_evidence");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 3, "{body}");

    let (status, body) = get(&srv.base, "/api/perf/checkout?range=8w").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 3, "{body}");
}

/// The 400 that asks which environment is meant names the environments
/// the principal may see, and no other.
#[tokio::test]
async fn perf_ambiguity_names_only_the_environments_in_scope() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = Vec::new();
    for env in ["demo", "staging", "prod-eu-secret"] {
        spans.push(span_in(env, &format!("{env}-1"), "1.0.0", now - 300 * NS));
        spans.push(span_in(env, &format!("{env}-2"), "1.1.0", now - 100 * NS));
    }
    seed(&srv, spans, vec![]).await;

    let scoped = add_scoped_token(&srv, "two-envs", "viewer", &["demo", "staging"]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout", &scoped).await;
    assert_eq!(status, 400, "{body}");
    let error = error_of(&body);
    assert!(
        error.contains("demo") && error.contains("staging"),
        "{error}"
    );
    assert!(!error.contains("prod-eu-secret"), "{error}");
    assert!(!body.to_string().contains("prod-eu-secret"), "{body}");
    // Nor does an answer for one of them list it.
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout?env=demo", &scoped).await;
    assert_eq!(status, 200, "{body}");
    assert!(!body.to_string().contains("prod-eu-secret"), "{body}");
    // An unscoped principal is told about all three.
    let admin = add_scoped_token(&srv, "every-env", "admin", &[]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout", &admin).await;
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("prod-eu-secret"), "{body}");
}

/// What a request supplies is a value to the lake, never part of a
/// statement: a quote in a target or a series changes nothing but the
/// name looked for.
#[tokio::test]
async fn perf_takes_a_target_and_a_probe_with_quotes_as_plain_names() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    for target in [
        "chec%27kout",
        "checkout%27%20OR%20%271%27%3D%271",
        "%27%3B--",
    ] {
        let (status, body) = get(&srv.base, &format!("/api/perf/{target}")).await;
        assert_eq!(status, 404, "{target}: {body}");
    }
    let probe = "load.latency_p95_ms%27%20OR%20%271%27%3D%271";
    let (status, body) = get(&srv.base, &format!("/api/perf/checkout?probe={probe}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["samples"], 0, "{body}");
    assert_eq!(body["baseline"]["samples"], 0, "{body}");
    let (status, body) = get(&srv.base, "/api/perf/checkout?env=demo%27%20OR%20%271").await;
    assert_eq!(status, 404, "{body}");
}

/// The question a runner asks when its run ends is answered with every
/// version that served the window and the requests each served, so the
/// runner can tell one version from a mix; the window is bounded.
#[tokio::test]
async fn perf_version_route_gives_shares_and_bounds_the_window() {
    let srv = spawn_server().await;
    let now = now_ns();
    let named = |id: &str, version: &str, namespace: &str, ts: i64| SpanRow {
        resource_attrs: vec![("service.namespace".into(), namespace.into())],
        ..span_in("demo", id, version, ts)
    };
    let mut spans = Vec::new();
    // [−1000 s, −940 s]: 1.0.0 serves 19 requests in 20.
    for i in 0..19 {
        spans.push(named(
            &format!("old-{i}"),
            "1.0.0",
            "shop",
            now - 1000 * NS + i * NS,
        ));
    }
    spans.push(named("new-0", "1.1.0", "shop", now - 950 * NS));
    // [−500 s, −440 s]: an even split, and a namesake in another
    // namespace that serves 2.0.0.
    for i in 0..5 {
        spans.push(named(
            &format!("mix-old-{i}"),
            "1.0.0",
            "shop",
            now - 500 * NS + i * NS,
        ));
        spans.push(named(
            &format!("mix-new-{i}"),
            "1.1.0",
            "shop",
            now - 490 * NS + i * NS,
        ));
        spans.push(named(
            &format!("other-{i}"),
            "2.0.0",
            "billing",
            now - 480 * NS + i * NS,
        ));
    }
    seed(&srv, spans, vec![]).await;

    let window =
        |from: i64, to: i64| format!("from_ns={}&to_ns={}", now - from * NS, now - to * NS);
    let path = format!("/api/perf/checkout/version?env=demo&{}", window(1000, 940));
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["version"], "1.0.0", "{body}");
    assert_eq!(body["version_source"], "observed", "{body}");
    assert_eq!(
        body["versions"],
        json!([
            {"version": "1.0.0", "requests": 19, "share": 0.95},
            {"version": "1.1.0", "requests": 1, "share": 0.05},
        ])
    );

    let path = format!(
        "/api/perf/checkout/version?env=demo&namespace=shop&{}",
        window(500, 440)
    );
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["namespace"], "shop");
    assert!(
        body["version"].is_null(),
        "no one version served it: {body}"
    );
    assert_eq!(body["version_source"], "mixed", "{body}");
    assert_eq!(body["versions"].as_array().unwrap().len(), 2, "{body}");
    let path = format!(
        "/api/perf/checkout/version?env=demo&namespace=billing&{}",
        window(500, 440)
    );
    let (_, body) = get(&srv.base, &path).await;
    assert_eq!(body["version"], "2.0.0", "{body}");
    // Nothing spoke: no version and no source.
    let path = format!("/api/perf/checkout/version?env=demo&{}", window(300, 240));
    let (_, body) = get(&srv.base, &path).await;
    assert!(
        body["version"].is_null() && body["version_source"].is_null(),
        "{body}"
    );
    assert_eq!(body["versions"], json!([]));

    // A day is the widest window; a namespace is a bounded name.
    let hour = 3600;
    let path = format!(
        "/api/perf/checkout/version?env=demo&{}",
        window(24 * hour, 0)
    );
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 200, "{body}");
    let path = format!(
        "/api/perf/checkout/version?env=demo&{}",
        window(24 * hour + 1, 0)
    );
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("24 h"), "{body}");
    let path = format!(
        "/api/perf/checkout/version?env=demo&from_ns={}&to_ns={}",
        i64::MIN,
        i64::MAX
    );
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 400, "{body}");
    let path = format!(
        "/api/perf/checkout/version?env=demo&namespace={}&{}",
        "n".repeat(300),
        window(500, 440)
    );
    let (status, body) = get(&srv.base, &path).await;
    assert_eq!(status, 400, "{body}");
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
