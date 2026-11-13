//! `GET /api/perf/{target}` — the performance regression verdict between a
//! target's latest two `service.version`s, computed from the `during`
//! load samples of the runs attributed to each version.

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
        target_environment: None,
        plugin_name: None,
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs: vec![],
        events: "[]".into(),
    }
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

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
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
    let (status, again) = get(&srv.base, "/api/perf/checkout?store=false").await;
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
}

#[tokio::test]
async fn perf_honours_quantile_probe_and_min_samples() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    let (status, body) = get(&srv.base, "/api/perf/checkout?quantile=p50&store=false").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["comparison"]["quantile"], "p50");
    assert_eq!(body["verdict"], "regressed", "{body}");

    // A series nobody wrote: both sides empty, so inconclusive.
    let (status, body) = get(&srv.base, "/api/perf/checkout?probe=nope&store=false").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive");
    assert_eq!(body["candidate"]["samples"], 0);

    let (status, body) = get(&srv.base, "/api/perf/checkout?min_samples=100&store=false").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive");
}

/// Scoped principals: `env` outside the scopes is a 403; a prod-scoped
/// viewer sees the versions but none of the demo runs, so the verdict
/// is inconclusive rather than leaking demo samples.
#[tokio::test]
async fn perf_respects_environment_scopes() {
    let srv = spawn_server().await;
    seed_perf(&srv).await;
    let token = add_scoped_token(&srv, "perf-viewer", "viewer", &["prod"]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout?env=demo", &token).await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout?store=false", &token).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "inconclusive", "{body}");
    assert_eq!(body["candidate"]["samples"], 0);

    let demo = add_scoped_token(&srv, "perf-demo", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/perf/checkout?store=false", &demo).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "regressed", "{body}");
}
