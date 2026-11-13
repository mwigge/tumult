//! `GET /api/perf/{target}` — the performance regression verdict between a
//! target's latest two `service.version`s, computed from the `during`
//! load samples of the runs attributed to each version — and
//! `POST /api/perf/{target}/verdicts`, which appends it to the history.

use crate::common::*;
use tumult_core::types::{ExperimentStatus, LoadResult, LoadSample, LoadTool};
use tumult_lake::SpanRow;

/// A span the target service itself emitted under `version`.
fn service_span(id: &str, version: &str, ts: i64) -> SpanRow {
    SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-svc-{id}"),
        span_id: format!("span-svc-{id}"),
        parent_span_id: None,
        span_name: "GET /checkout".into(),
        span_kind: "Server".into(),
        duration_ns: 1_000_000,
        status_code: "Ok".into(),
        status_message: String::new(),
        service_name: "checkout".into(),
        service_version: Some(version.into()),
        experiment_id: None,
        experiment_name: None,
        outcome_status: None,
        fault_type: None,
        fault_subtype: None,
        fault_severity: None,
        blast_radius: None,
        target_system: None,
        target_technology: None,
        target_environment: Some("demo".into()),
        plugin_name: None,
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs: vec![],
        events: "[]".into(),
    }
}

/// POST with no body and no credentials; returns (status, body).
async fn post(base: &str, path: &str) -> (u16, serde_json::Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// A tumult experiment root span against the `checkout` target.
fn root_span(id: &str, ts: i64) -> SpanRow {
    SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        parent_span_id: None,
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 60 * NS,
        status_code: "Unset".into(),
        status_message: String::new(),
        service_name: "tumult".into(),
        service_version: Some("2.22.0".into()),
        experiment_id: Some(id.into()),
        experiment_name: Some("checkout-perf".into()),
        outcome_status: None,
        fault_type: None,
        fault_subtype: None,
        fault_severity: None,
        blast_radius: None,
        target_system: Some("checkout".into()),
        target_technology: Some("http".into()),
        target_environment: Some("demo".into()),
        plugin_name: None,
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs: vec![],
        events: "[]".into(),
    }
}

/// A journal whose load result streamed `windows` samples around `p95`.
fn load_journal(id: &str, started: i64, p95: f64, windows: usize) -> tumult_core::types::Journal {
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
    });
    journal
}

/// Seed `checkout` under 1.4.0 (one run, p95 ≈ 100 ms) then 1.5.0 (one
/// run, p95 ≈ 150 ms), and `search` under a single version with no runs.
async fn seed_perf(srv: &TestServer) {
    let now = now_ns();
    exec_write(srv, move |writer| {
        writer
            .insert_spans(&[
                service_span("a1", "1.4.0", now - 3000 * NS),
                service_span("a2", "1.4.0", now - 2000 * NS),
                service_span("b1", "1.5.0", now - 1500 * NS),
                service_span("b2", "1.5.0", now - 100 * NS),
                root_span("perf-old", now - 2500 * NS),
                root_span("perf-new", now - 1000 * NS),
                SpanRow {
                    service_name: "search".into(),
                    ..service_span("s1", "0.9.0", now - 50 * NS)
                },
            ])
            .map_err(|e| e.to_string())?;
        writer
            .ingest_journal(&load_journal("perf-old", now - 2500 * NS, 100.0, 40), None)
            .map_err(|e| e.to_string())?;
        writer
            .ingest_journal(&load_journal("perf-new", now - 1000 * NS, 150.0, 40), None)
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await;
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

    // The read has no storing variant any more: the old flag is ignored.
    let (status, body) = get(&srv.base, "/api/perf/checkout?store=true").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["stored"], false);
    let (_, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(body["history"].as_array().unwrap().len(), 0, "{body}");

    let (status, body) = post(&srv.base, "/api/perf/checkout/verdicts").await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["target"], "checkout");
    assert_eq!(body["probe"], "load.latency_p95_ms");
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["baseline"]["version"], "1.4.0");
    assert_eq!(body["baseline"]["runs"][0], "perf-old");
    assert_eq!(body["baseline"]["samples"], 40);
    assert_eq!(body["candidate"]["version"], "1.5.0");
    assert_eq!(body["candidate"]["runs"][0], "perf-new");
    let comparison = &body["comparison"];
    assert_eq!(comparison["quantile"], "p95");
    assert!(comparison["delta"].as_f64().unwrap() > 40.0, "{body}");
    assert!(comparison["ci_low"].as_f64().unwrap() > comparison["margin"].as_f64().unwrap());
    assert!(comparison["ci_high"].as_f64().unwrap() >= comparison["ci_low"].as_f64().unwrap());
    assert_eq!(comparison["confidence"], 0.95);
    assert_eq!(body["stored"], true);
    assert_eq!(body["history"].as_array().unwrap().len(), 0);
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 2);
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
    assert_eq!(again["comparison"]["ci_low"], body["comparison"]["ci_low"]);
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
    let (status, _) = get(&srv.base, "/api/perf/checkout?quantile=p42").await;
    assert_eq!(status, 400);
    let (status, _) = get(&srv.base, "/api/perf/checkout?confidence=1").await;
    assert_eq!(status, 400);
    // An empty environment is a malformed filter, not "every environment".
    let (status, body) = get(&srv.base, "/api/perf/checkout?env=").await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&srv.base, "/api/perf/checkout/verdicts?env=%20").await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&srv.base, "/api/perf/no-such-service/verdicts").await;
    assert_eq!(status, 404, "{body}");
}

/// A service whose spans carry no environment is visible to an unscoped
/// read, and invisible — with a message that says why — to an environment
/// filter or a scoped principal.
#[tokio::test]
async fn perf_untagged_service_is_outside_every_environment() {
    let srv = spawn_server().await;
    let now = now_ns();
    exec_write(&srv, move |writer| {
        let untagged = |id: &str, version: &str, ts: i64| SpanRow {
            service_name: "ledger".into(),
            target_environment: None,
            ..service_span(id, version, ts)
        };
        writer
            .insert_spans(&[
                untagged("l1", "3.0.0", now - 300 * NS),
                untagged("l2", "3.1.0", now - 100 * NS),
            ])
            .map_err(|e| e.to_string())
    })
    .await;

    let (status, body) = get(&srv.base, "/api/perf/ledger").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 2);

    let (status, body) = get(&srv.base, "/api/perf/ledger?env=demo").await;
    assert_eq!(status, 404, "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("in environment demo"), "{error}");
    assert!(error.contains("deployment.environment.name"), "{error}");
    assert!(!error.contains("no service.version seen for target ledger;"));

    // Fail closed for a scoped principal, with the same explanation.
    let scoped = add_scoped_token(&srv, "ledger-viewer", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/ledger", &scoped).await;
    assert_eq!(status, 404, "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("in environment demo"), "{error}");

    // A target nobody has seen at all says so plainly.
    let admin = add_scoped_token(&srv, "ledger-admin", "admin", &[]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/nothing", &admin).await;
    assert_eq!(status, 404, "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(!error.contains("in environment"), "{error}");
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

    let demo = add_scoped_token(&srv, "perf-demo", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout", &demo).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "regressed", "{body}");
    assert_eq!(body["versions_seen"].as_array().unwrap().len(), 2);
}

/// Storing is a mutation: a viewer may read the verdict but not append
/// it, and a stored row is visible only inside the environments it was
/// computed under.
#[tokio::test]
async fn perf_store_needs_operator_and_history_is_environment_scoped() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    let viewer = add_scoped_token(&srv, "perf-viewer", "viewer", &["demo"]).await;
    let none = serde_json::Value::Null;
    let (status, body) = post_auth(
        &srv.base,
        "/api/perf/checkout/verdicts",
        &viewer,
        none.clone(),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    // The old storing read stores nothing for anyone.
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout?store=true", &viewer).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["stored"], false);

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

    // The demo-scoped viewer sees the stored row, with its environments.
    let (_, body) = get_auth(&srv.base, "/api/perf/checkout", &viewer).await;
    let history = body["history"].as_array().unwrap();
    assert_eq!(history.len(), 1, "{body}");
    assert_eq!(history[0]["envs"], serde_json::json!(["demo"]));

    // A principal scoped to demo and prod sees it too; one scoped to
    // another environment sees neither the target nor the row.
    let wide = add_scoped_token(&srv, "perf-wide", "viewer", &["demo", "prod"]).await;
    let (_, body) = get_auth(&srv.base, "/api/perf/checkout", &wide).await;
    assert_eq!(body["history"].as_array().unwrap().len(), 1, "{body}");
}
