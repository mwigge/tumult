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
        execution: tumult_core::types::LoadExecution::default(),
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

    // The read has no storing variant: the old flag is refused, with the
    // route that does store, rather than silently ignored.
    for flag in ["store=true", "store=false", "store"] {
        let (status, body) = get(&srv.base, &format!("/api/perf/checkout?{flag}")).await;
        assert_eq!(status, 400, "{flag}: {body}");
        let error = body["error"].as_str().unwrap();
        assert!(
            error.contains("POST /api/perf/checkout/verdicts"),
            "{error}"
        );
    }
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
    assert_eq!(body["duplicate"], false);
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

    // A retry of the POST (the client timed out, the daemon had stored)
    // answers 200 and appends nothing.
    let (status, retry) = post(&srv.base, "/api/perf/checkout/verdicts").await;
    assert_eq!(status, 200, "{retry}");
    assert_eq!(retry["stored"], true);
    assert_eq!(retry["duplicate"], true);
    assert_eq!(retry["history"].as_array().unwrap().len(), 1, "{retry}");
    let (_, after) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(after["history"].as_array().unwrap().len(), 1, "{after}");
    // Another question about the same evidence is another row.
    let (status, p50) = post(&srv.base, "/api/perf/checkout/verdicts?quantile=p50").await;
    assert_eq!(status, 201, "{p50}");
    let (status, scoped) = post(&srv.base, "/api/perf/checkout/verdicts?env=demo").await;
    assert_eq!(status, 201, "{scoped}");
    let (_, after) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(after["history"].as_array().unwrap().len(), 3, "{after}");
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

/// `service_span` in another environment.
fn span_in(env: &str, id: &str, version: &str, ts: i64) -> SpanRow {
    SpanRow {
        target_environment: Some(env.into()),
        ..service_span(id, version, ts)
    }
}

/// Spans of `version` every ten seconds over `[from, to]`, in `env`.
fn stream(env: &str, version: &str, from: i64, to: i64) -> Vec<SpanRow> {
    (0..)
        .map(|i| from + i * 10 * NS)
        .take_while(|ts| *ts <= to)
        .map(|ts| span_in(env, &format!("{env}-{version}-{ts}"), version, ts))
        .collect()
}

/// Insert spans and one 40-window journal per `(run id, start, p95)`.
async fn seed(srv: &TestServer, spans: Vec<SpanRow>, runs: Vec<(&'static str, i64, f64)>) {
    exec_write(srv, move |writer| {
        writer.insert_spans(&spans).map_err(|e| e.to_string())?;
        for (id, started, p95) in &runs {
            writer
                .ingest_journal(&load_journal(id, *started, *p95, 40), None)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await;
}

fn runs_of(side: &serde_json::Value) -> Vec<String> {
    side["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect()
}

/// 1.0.0 is busy in prod while 1.1.0 is tried in demo. An unscoped read
/// must give the demo runs to the version demo ran, however often prod
/// spoke in between, and must not let prod's chatter pick the candidate.
#[tokio::test]
async fn perf_attributes_a_run_from_the_spans_of_its_own_environment() {
    let srv = spawn_server().await;
    let now = now_ns();
    let mut spans = stream("prod", "1.0.0", now - 4000 * NS, now - NS);
    spans.extend(stream("demo", "1.0.0", now - 3000 * NS, now - 2000 * NS));
    // Demo's 1.1.0 is quiet around its run: prod spoke last before it.
    spans.push(span_in("demo", "new-1", "1.1.0", now - 1500 * NS));
    spans.push(span_in("demo", "new-2", "1.1.0", now - 400 * NS));
    spans.push(root_span("demo-old", now - 2800 * NS));
    spans.push(root_span("demo-new", now - 1000 * NS));
    spans.push(SpanRow {
        target_environment: Some("prod".into()),
        ..root_span("prod-run", now - 700 * NS)
    });
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

    let (status, body) = get(&srv.base, "/api/perf/checkout").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate"]["version"], "1.1.0", "{body}");
    assert_eq!(runs_of(&body["candidate"]), vec!["demo-new"]);
    assert_eq!(body["baseline"]["version"], "1.0.0");
    assert_eq!(runs_of(&body["baseline"]), vec!["demo-old", "prod-run"]);
    assert_eq!(body["candidate"]["samples"], 40);
    assert_eq!(body["baseline"]["samples"], 80);
    assert_eq!(body["verdict"], "regressed", "{body}");
    let live: Vec<bool> = body["versions_seen"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["live"].as_bool().unwrap())
        .collect();
    assert_eq!(live, vec![true, true], "{body}");

    // Each environment on its own: demo compares its two versions, prod
    // has one.
    let (_, demo) = get(&srv.base, "/api/perf/checkout?env=demo").await;
    assert_eq!(runs_of(&demo["candidate"]), vec!["demo-new"]);
    assert_eq!(runs_of(&demo["baseline"]), vec!["demo-old"]);
    let (_, prod) = get(&srv.base, "/api/perf/checkout?env=prod").await;
    assert_eq!(prod["candidate"]["version"], "1.0.0");
    assert_eq!(runs_of(&prod["candidate"]), vec!["prod-run"]);
    assert!(prod["baseline"].is_null(), "{prod}");
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
    exec_write(&srv, move |writer| {
        writer
            .insert_spans(&[span_in("demo", "old-last", "1.0.0", now - NS)])
            .map_err(|e| e.to_string())
    })
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
