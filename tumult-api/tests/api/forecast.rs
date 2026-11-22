//! `GET /api/forecast` and `GET /api/runs/{id}/impact` over the seeded
//! store: the built-in provider answers (no sidecar is configured), every
//! response names its model, metric forecasts are cached per context
//! window, and the impact endpoint 404s for runs without probe samples.
//! With a sidecar configured, its answer carries its own provenance, and a
//! failing one opens the circuit so later requests fall back without
//! calling it; the reason they are given is a coarse code.
//!
//! Cache assertions go through [`get_in_one_window`], so none of them
//! depends on where in a minute the test happens to run.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::common::*;

#[tokio::test]
async fn forecast_returns_a_band_with_provenance_and_caches_it() {
    let srv = spawn_server().await;
    let (status, body) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&range=24h&horizon=6",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["metric"], "experiment_count");
    assert_eq!(body["horizon"], 6);
    assert_eq!(body["range"], "24h");
    assert_eq!(
        body["step_ns"], 60_000_000_000_i64,
        "a day forecasts at one minute"
    );
    assert_eq!(body["provenance"]["model"], "holt-winters");
    assert_eq!(body["provenance"]["weights_hash"], "none");
    assert!(body["provenance"]["fallback_reason"].is_null());
    assert_eq!(body["cached"], false);
    let series = &body["series"];
    let n = series["ts"].as_array().unwrap().len();
    assert_eq!(n, 24 * 60 + 6);
    assert_eq!(series["observed"].as_array().unwrap().len(), n);
    assert_eq!(series["median"].as_array().unwrap().len(), n);
    assert!(series["median"][n - 1].is_number(), "{body}");
    assert!(series["median"][0].is_null());
    assert_eq!(series["bands"][0]["level"], 0.8);
    assert_eq!(body["forecast"]["quantiles"].as_array().unwrap().len(), 6);
    assert_eq!(
        body["forecast"]["levels"],
        serde_json::json!([0.1, 0.5, 0.9])
    );
    // Two counters in a day are not much history; the response says so.
    assert_eq!(body["forecast"]["insufficient_history"], true);

    assert_eq!(body["forecast"]["degenerate_band"], true, "{body}");
    assert_eq!(
        body["from_ms"].as_i64().unwrap(),
        body["from_ns"].as_i64().unwrap() / 1_000_000
    );
    assert_eq!(
        body["to_ms"].as_i64().unwrap(),
        body["to_ns"].as_i64().unwrap() / 1_000_000
    );

    // Horizon 100 is fitted at another bucket than the 6 above.
    let path = "/api/forecast?metric=experiment_count&range=24h&horizon=100";
    let (first, again) = get_in_one_window(&srv, path, path).await;
    assert_eq!(first["cached"], false, "{first}");
    assert_eq!(again["cached"], true, "{again}");
    assert_eq!(
        again["forecast"]["quantiles"],
        first["forecast"]["quantiles"]
    );
    assert_eq!(again["forecast"]["degenerate_band"], true);
    assert_eq!(again["provenance"]["model"], "holt-winters");
}

/// A forecast is fitted at the smallest horizon bucket that holds the
/// asked horizon, so the horizons inside one bucket share one cache row
/// and each is served its own number of steps.
#[tokio::test]
async fn horizons_in_one_bucket_share_a_cache_entry() {
    let srv = spawn_server().await;
    let short = "/api/forecast?metric=experiment_count&range=24h&horizon=30";
    let long = "/api/forecast?metric=experiment_count&range=24h&horizon=90";
    let (a, b) = get_in_one_window(&srv, short, long).await;
    assert_eq!(a["cached"], false, "{a}");
    assert_eq!(b["cached"], true, "30 and 90 are one bucket: {b}");
    assert_eq!(a["horizon"], 30);
    assert_eq!(b["horizon"], 90);
    let first = a["forecast"]["quantiles"].as_array().unwrap();
    let second = b["forecast"]["quantiles"].as_array().unwrap();
    assert_eq!(first.len(), 30);
    assert_eq!(second.len(), 90);
    assert_eq!(first[..], second[..30], "the short one is the head");
    let observed = 24 * 60;
    assert_eq!(a["series"]["ts"].as_array().unwrap().len(), observed + 30);
    assert_eq!(b["series"]["ts"].as_array().unwrap().len(), observed + 90);
    // A try that crossed a window boundary left rows of its own window;
    // the pair that matched is in the newest one.
    assert_eq!(
        forecast_rows(&srv, &format!("horizon = 96 AND {NEWEST_WINDOW}")),
        1,
        "one row, fitted at the bucket"
    );
    assert_eq!(forecast_rows(&srv, NEWEST_WINDOW), 1);

    // Another bucket is another entry.
    let longer = "/api/forecast?metric=experiment_count&range=24h&horizon=97";
    let (_, c) = get_in_one_window(&srv, long, longer).await;
    assert_eq!(c["cached"], false, "{c}");
    assert_eq!(c["forecast"]["quantiles"].as_array().unwrap().len(), 97);
    assert_eq!(
        forecast_rows(&srv, &format!("horizon = 384 AND {NEWEST_WINDOW}")),
        1
    );
}

/// The rows of the newest window that any forecast was fitted on.
const NEWEST_WINDOW: &str = "context_end_ns = (SELECT max(context_end_ns) FROM forecasts)";

/// How many `forecasts` rows match `predicate`.
fn forecast_rows(srv: &TestServer, predicate: &str) -> u64 {
    let reader = tumult_lake::Store::at(&srv.db_path).read_only().unwrap();
    let sql = format!("SELECT count(*) AS c FROM forecasts WHERE {predicate}");
    reader.query_json_rows(&sql).unwrap()[0]["c"]
        .as_u64()
        .unwrap()
}

/// One series keeps its newest rows only, and a backlog of rows past the
/// retention shrinks by a batch with every write.
#[tokio::test]
async fn the_cache_is_capped_per_series_and_pruned_in_batches() {
    let srv = spawn_server().await;
    let (status, body) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&range=24h&horizon=6",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(forecast_rows(&srv, "true"), 1);
    // 300 rows of another series from long ago, and 200 older rows of
    // this one on windows of their own.
    exec_write(&srv, |w| {
        w.execute(
            "INSERT INTO forecasts SELECT f.* REPLACE ('expired' AS series_key, \
             i AS created_at_ns) FROM forecasts f, range(1, 301) r(i)",
            [],
        )
        .map_err(|e| e.to_string())?;
        w.execute(
            "INSERT INTO forecasts SELECT f.* REPLACE (f.created_at_ns - i AS created_at_ns, \
             f.context_end_ns - i AS context_end_ns) FROM forecasts f, range(1, 201) r(i) \
             WHERE f.series_key <> 'expired'",
            [],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
    .await;
    assert_eq!(forecast_rows(&srv, "series_key = 'expired'"), 300);
    assert_eq!(forecast_rows(&srv, "series_key <> 'expired'"), 201);

    // One more write: a horizon in another bucket misses the cache.
    let (status, body) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&range=24h&horizon=100",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["cached"], false);
    assert_eq!(
        forecast_rows(&srv, "series_key <> 'expired'"),
        tumult_lake::FORECAST_ROWS_PER_SERIES as u64,
        "the series is at its cap"
    );
    assert_eq!(
        forecast_rows(&srv, "series_key <> 'expired' AND horizon = 384"),
        1,
        "the row just written is kept"
    );
    assert_eq!(
        forecast_rows(&srv, "series_key = 'expired'"),
        300 - tumult_lake::FORECAST_PRUNE_BATCH as u64,
        "one batch of the backlog went"
    );
    assert_eq!(
        forecast_rows(&srv, "series_key = 'expired' AND created_at_ns <= 256"),
        0,
        "oldest first"
    );
}

/// Two requests that fall in the same aligned window (the same `to_ns`),
/// so a cache assertion never depends on the wall clock. When a window
/// boundary passes between them the pair is asked again in a window
/// neither has touched, so the first response of the pair is as fresh as
/// it would have been the first time.
async fn get_in_one_window(srv: &TestServer, first: &str, second: &str) -> (Value, Value) {
    for _ in 0..5 {
        let (status, a) = get(&srv.base, first).await;
        assert_eq!(status, 200, "{a}");
        let (status, b) = get(&srv.base, second).await;
        assert_eq!(status, 200, "{b}");
        if a["to_ns"] == b["to_ns"] {
            return (a, b);
        }
        next_window().await;
    }
    panic!("five window boundaries in a row");
}

/// Sleep into the next window. Every range the tests ask for moves its
/// window on the minute.
async fn next_window() {
    let minute = 60 * NS;
    let left = minute - now_ns().rem_euclid(minute);
    let left = Duration::from_nanos(u64::try_from(left).unwrap_or_default());
    // sleep: the window follows the wall clock, which a test cannot move
    tokio::time::sleep(left + Duration::from_millis(50)).await;
}

/// A day and a fortnight share the one-minute tier, so they share the
/// step, the horizon and the window end; only the context start differs.
/// The fortnight must not be served the forecast fitted on the day.
#[tokio::test]
async fn a_forecast_is_not_served_for_another_context_range() {
    let srv = spawn_server().await;
    let day = "/api/forecast?metric=experiment_count&range=24h&horizon=30";
    let fortnight = "/api/forecast?metric=experiment_count&range=14d&horizon=30";
    let (a, b) = get_in_one_window(&srv, day, fortnight).await;
    assert_eq!(a["step_ns"], b["step_ns"], "same tier");
    assert_eq!(a["horizon"], b["horizon"]);
    assert_eq!(a["cached"], false);
    assert_eq!(b["cached"], false, "another context is another forecast");
    assert!(b["from_ns"].as_i64().unwrap() < a["from_ns"].as_i64().unwrap());
    let observed = b["series"]["observed"].as_array().unwrap();
    let points = observed.iter().filter(|v| !v.is_null()).count();
    assert_eq!(
        b["forecast"]["context_points"].as_u64(),
        Some(points as u64),
        "the forecast was fitted on the fortnight's own points"
    );
    // Each range still hits its own entry.
    let (_, second) = get_in_one_window(&srv, fortnight, fortnight).await;
    assert_eq!(second["cached"], true, "{second}");
}

/// The cache is an optimisation. With the table gone, the lookup and the
/// write both fail; the forecast is served regardless.
#[tokio::test]
async fn a_broken_cache_does_not_fail_the_request() {
    let srv = spawn_server().await;
    exec_write(&srv, |w| {
        w.execute("DROP TABLE forecasts", [])
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
    let path = "/api/forecast?metric=experiment_count&range=24h&horizon=30";
    let (first, again) = get_in_one_window(&srv, path, path).await;
    for body in [first, again] {
        assert_eq!(body["cached"], false, "{body}");
        assert_eq!(body["provenance"]["model"], "holt-winters");
        assert!(body["forecast"]["quantiles"].is_array());
    }
}

/// An hour forecasts at one second, but its window moves once a minute:
/// a chart polling every second reuses one cache row.
#[tokio::test]
async fn the_one_second_tier_is_cached_by_the_minute() {
    let srv = spawn_server().await;
    let now = now_ns();
    exec_write(&srv, move |w| {
        w.execute(
            &format!(
                "UPDATE spans SET ts_ns = {} WHERE span_name = 'resilience.experiment'",
                now - 120 * NS
            ),
            [],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
    .await;
    let path = "/api/forecast?metric=experiment_count&range=1h&horizon=30";
    let (first, again) = get_in_one_window(&srv, path, path).await;
    assert_eq!(first["step_ns"], NS);
    assert_eq!(first["to_ns"].as_i64().unwrap() % (60 * NS), 0, "{first}");
    assert_eq!(first["cached"], false);
    assert_eq!(again["cached"], true, "{again}");
}

#[tokio::test]
async fn forecast_filters_by_target_and_validates_input() {
    let srv = spawn_server().await;
    let (status, body) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&target=database&range=24h&horizon=2",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["target"], "database");

    let (status, body) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&target=nothing-here&range=24h",
    )
    .await;
    assert_eq!(status, 404, "{body}");

    let (status, _) = get(&srv.base, "/api/forecast?range=24h").await;
    assert_eq!(status, 400);
    let (status, _) = get(&srv.base, "/api/forecast?metric=nope").await;
    assert_eq!(status, 404);
    let (status, _) = get(&srv.base, "/api/forecast?metric=experiment_count&range=2w").await;
    assert_eq!(status, 400);
    let (status, _) = get(&srv.base, "/api/forecast?metric=experiment_count&horizon=0").await;
    assert_eq!(status, 400);
    let (status, _) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&horizon=99999",
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn forecast_honours_environment_scopes() {
    let srv = spawn_server().await;
    let token = add_scoped_token(&srv, "fc-scoped", "viewer", &["staging"]).await;
    let (status, body) = get_auth(
        &srv.base,
        "/api/forecast?metric=experiment_count&env=demo",
        &token,
    )
    .await;
    assert_eq!(status, 403, "{body}");
    // In scope but no rows there: an honest 404, never another env's data.
    let (status, body) = get_auth(
        &srv.base,
        "/api/forecast?metric=experiment_count&env=staging",
        &token,
    )
    .await;
    assert_eq!(status, 404, "{body}");
}

/// A finished run with no probe samples of its own.
async fn seed_bare_run(srv: &TestServer, run_id: &str, experiment_id: &str) {
    let run = run_id.to_string();
    let exp = experiment_id.to_string();
    exec_write(srv, move |w| {
        w.insert_run(&tumult_lake::NewRun {
            id: run.clone(),
            registry_id: "reg-impact".into(),
            params_json: None,
            queued_at_ns: now_ns() - 600 * NS,
            actor: None,
        })
        .map_err(|e| e.to_string())?;
        w.finish_run(&run, "passed", Some(&exp), None, None)
            .map_err(|e| e.to_string())
    })
    .await;
}

/// A run whose `health` probe was sampled once a second: `pre` steps
/// before the fault at a flat 100 ms, 20 during-phase steps where latency
/// triples, then 40 post-phase steps back at exactly 100 ms after a
/// three-step 60 ms tail. The settled level is constant on purpose: a
/// probe that reports the same value every second is the common case, and
/// it leaves a fit without residuals.
async fn seed_run(srv: &TestServer, run_id: &str, experiment_id: &str, pre: i64) {
    let run = run_id.to_string();
    let exp = experiment_id.to_string();
    let base = now_ns() - 600 * NS;
    exec_write(srv, move |w| {
        w.insert_run(&tumult_lake::NewRun {
            id: run.clone(),
            registry_id: "reg-impact".into(),
            params_json: None,
            queued_at_ns: base,
            actor: None,
        })
        .map_err(|e| e.to_string())?;
        w.finish_run(&run, "passed", Some(&exp), None, None)
            .map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        for i in 0..pre + 60 {
            let since_fault = i - pre;
            let (phase, value) = if since_fault < 0 {
                ("before", 100.0)
            } else if since_fault < 20 {
                (
                    "during",
                    300.0 + f64::from(i32::try_from(since_fault % 3).unwrap_or(0)) * 5.0,
                )
            } else if since_fault < 23 {
                ("post", 160.0)
            } else {
                ("post", 100.0)
            };
            rows.push(format!(
                "('{exp}', 'health', '{phase}', {}, {value}, true)",
                base + i * NS
            ));
        }
        w.execute(
            &format!(
                "INSERT INTO probe_samples (experiment_id, probe_name, phase, ts_ns, value, ok) \
                 VALUES {}",
                rows.join(", ")
            ),
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await;
}

/// A run without pre-fault samples: the counterfactual is a backcast.
async fn seed_run_with_samples(srv: &TestServer, run_id: &str, experiment_id: &str) {
    seed_run(srv, run_id, experiment_id, 0).await;
}

#[tokio::test]
async fn impact_is_404_without_a_run_or_samples() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/runs/does-not-exist/impact").await;
    assert_eq!(status, 404, "{body}");

    let run = "run-no-samples".to_string();
    exec_write(&srv, move |w| {
        w.insert_run(&tumult_lake::NewRun {
            id: run.clone(),
            registry_id: "reg-impact".into(),
            params_json: None,
            queued_at_ns: now_ns(),
            actor: None,
        })
        .map_err(|e| e.to_string())?;
        w.finish_run(&run, "passed", Some("exp-without-samples"), None, None)
            .map_err(|e| e.to_string())
    })
    .await;
    let (status, body) = get(&srv.base, "/api/runs/run-no-samples/impact").await;
    assert_eq!(status, 404, "{body}");
    assert!(body["error"].as_str().unwrap().contains("no probe samples"));
}

#[tokio::test]
async fn impact_backcasts_from_the_post_phase_and_measures_recovery() {
    let srv = spawn_server().await;
    seed_run_with_samples(&srv, "run-impact", "exp-impact").await;
    let (status, body) = get(&srv.base, "/api/runs/run-impact/impact").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["run_id"], "run-impact");
    assert_eq!(body["experiment_id"], "exp-impact");
    assert_eq!(body["stable_steps"], 3);
    assert_eq!(body["provenance"]["model"], "holt-winters");
    let probes = body["probes"].as_array().unwrap();
    assert_eq!(probes.len(), 1);
    let p = &probes[0];
    assert_eq!(p["probe"], "health");
    assert_eq!(
        p["reference"], "post_backcast",
        "no pre-fault samples were recorded"
    );
    assert_eq!(p["step_ns"], NS);
    assert_eq!(p["series"]["ts"].as_array().unwrap().len(), 60);
    assert_eq!(p["series"]["observed"].as_array().unwrap().len(), 60);
    assert_eq!(p["series"]["median"].as_array().unwrap().len(), 60);
    assert_eq!(p["series"]["bands"][0]["level"], 0.8);
    let fault_len =
        (p["fault"]["end_ns"].as_i64().unwrap() - p["fault"]["start_ns"].as_i64().unwrap()) / NS;
    assert_eq!(fault_len, 20);
    let impact = &p["impact"];
    assert_eq!(impact["samples_during"], 20);
    assert_eq!(impact["samples_post"], 40);
    // The post phase is flat, so the band it is judged against is the
    // floor, and the response says so instead of reporting no peak (a
    // zero-width band) or one in the quadrillions (a float-noise band).
    assert_eq!(p["degenerate_band"], true, "{p}");
    assert_eq!(body["degenerate_band"], true);
    let peak = impact["during_peak"].as_f64().unwrap();
    assert!((1.0..1e6).contains(&peak), "{impact}");
    assert_eq!(body["impact_peak"].as_f64(), Some(peak));
    let band = &p["series"]["bands"][0];
    let width = band["upper"][5].as_f64().unwrap() - band["lower"][5].as_f64().unwrap();
    assert!(width > 0.05 && width <= 10.0, "floored, not zero: {width}");
    assert!(
        impact["during_outside_ratio"].as_f64().unwrap() > 0.9,
        "{impact}"
    );
    assert_eq!(impact["per_sample"].as_array().unwrap().len(), 60);
    let recovery = &p["recovery"];
    let seconds = recovery["recovery_time_s"].as_f64().unwrap();
    assert!(
        (1.0..=6.0).contains(&seconds),
        "the 60 ms tail lasts three steps: {recovery}"
    );
    assert!(recovery["recovered_at_ns"].is_number());
    assert_eq!(
        recovery["recovered_at_ms"].as_i64().unwrap(),
        recovery["recovered_at_ns"].as_i64().unwrap() / 1_000_000
    );
    assert_eq!(
        p["fault"]["start_ms"].as_i64().unwrap(),
        p["fault"]["start_ns"].as_i64().unwrap() / 1_000_000
    );
    assert_eq!(
        p["fault"]["end_ms"].as_i64().unwrap() - p["fault"]["start_ms"].as_i64().unwrap(),
        20_000
    );
    assert_eq!(
        body["as_of_ms"].as_i64().unwrap(),
        body["as_of_ns"].as_i64().unwrap() / 1_000_000
    );
    assert_eq!(body["recovery_time_s"], recovery["recovery_time_s"]);
    assert_eq!(body["unrecovered_probes"], 0);
    assert_eq!(body["evaluated_probes"], 1);
    assert!(p["forecast"]["context_points"].as_u64().unwrap() >= 30);
    assert_eq!(p["provenance"]["model"], "holt-winters");
}

/// A second probe that never comes back: 100 ± 2 before the fault, 300
/// during it and 200 ± 2 for as long as it was watched.
async fn seed_stuck_probe(srv: &TestServer, experiment_id: &str, pre: i64) {
    let exp = experiment_id.to_string();
    let base = now_ns() - 600 * NS;
    exec_write(srv, move |w| {
        let mut rows = Vec::new();
        for i in 0..pre + 60 {
            let jitter = f64::from(i32::try_from(i % 5).unwrap_or(0)) - 2.0;
            let (phase, level) = match i - pre {
                d if d < 0 => ("before", 100.0),
                d if d < 20 => ("during", 300.0),
                _ => ("post", 200.0),
            };
            rows.push(format!(
                "('{exp}', 'stuck', '{phase}', {}, {}, true)",
                base + i * NS,
                level + jitter
            ));
        }
        w.execute(
            &format!(
                "INSERT INTO probe_samples (experiment_id, probe_name, phase, ts_ns, value, ok) \
                 VALUES {}",
                rows.join(", ")
            ),
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await;
}

/// One probe recovered within seconds, the other never did. The run did
/// not recover, and the headline must not report the first one's time.
#[tokio::test]
async fn a_probe_that_never_recovered_makes_the_run_not_recovered() {
    let srv = spawn_server().await;
    seed_run(&srv, "run-stuck", "exp-stuck", 40).await;
    seed_stuck_probe(&srv, "exp-stuck", 40).await;
    let (status, body) = get(&srv.base, "/api/runs/run-stuck/impact").await;
    assert_eq!(status, 200, "{body}");
    let probes = body["probes"].as_array().unwrap();
    assert_eq!(probes.len(), 2);
    let by_name = |name: &str| probes.iter().find(|p| p["probe"] == name).unwrap();
    assert!(
        by_name("health")["recovery"]["recovery_time_s"].is_number(),
        "{body}"
    );
    assert!(by_name("stuck")["recovery"]["recovery_time_s"].is_null());
    assert!(
        body["recovery_time_s"].is_null(),
        "{}",
        body["recovery_time_s"]
    );
    assert_eq!(body["unrecovered_probes"], 1);
    assert_eq!(body["evaluated_probes"], 2);
    assert!(body["impact_peak"].is_number());
}

/// A probe sampled `post` times after the fault at its baseline level:
/// 100 ± 2 before, 300 during.
async fn seed_cut_off_probe(srv: &TestServer, experiment_id: &str, name: &str, post: i64) {
    let exp = experiment_id.to_string();
    let name = name.to_string();
    let base = now_ns() - 600 * NS;
    exec_write(srv, move |w| {
        let mut rows = Vec::new();
        for i in 0..60 + post {
            let jitter = f64::from(i32::try_from(i % 5).unwrap_or(0)) - 2.0;
            let (phase, level) = match i {
                i if i < 40 => ("before", 100.0),
                i if i < 60 => ("during", 300.0),
                _ => ("post", 100.0),
            };
            rows.push(format!(
                "('{exp}', '{name}', '{phase}', {}, {}, true)",
                base + i * NS,
                level + jitter
            ));
        }
        w.execute(
            &format!(
                "INSERT INTO probe_samples (experiment_id, probe_name, phase, ts_ns, value, ok) \
                 VALUES {}",
                rows.join(", ")
            ),
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await;
}

/// One probe sampled once a second at the given levels, 100 ± 2 style
/// jitter added.
async fn seed_probe(srv: &TestServer, experiment_id: &str, name: &str, plan: &[(&str, f64, i64)]) {
    let exp = experiment_id.to_string();
    let name = name.to_string();
    let base = now_ns() - 600 * NS;
    let plan: Vec<(String, f64, i64)> = plan
        .iter()
        .map(|(phase, level, count)| ((*phase).to_string(), *level, *count))
        .collect();
    exec_write(srv, move |w| {
        let mut rows = Vec::new();
        let mut i = 0_i64;
        for (phase, level, count) in &plan {
            for _ in 0..*count {
                let jitter = f64::from(i32::try_from(i % 5).unwrap_or(0)) - 2.0;
                rows.push(format!(
                    "('{exp}', '{name}', '{phase}', {}, {}, true)",
                    base + i * NS,
                    level + jitter
                ));
                i += 1;
            }
        }
        w.execute(
            &format!(
                "INSERT INTO probe_samples (experiment_id, probe_name, phase, ts_ns, value, ok) \
                 VALUES {}",
                rows.join(", ")
            ),
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await;
}

/// A run whose only probe was never sampled during a fault has nothing to
/// evaluate. That is its own state: not "not recovered", which is a
/// finding, and not "undetermined", which is about probes that were
/// evaluated.
#[tokio::test]
async fn a_run_with_no_evaluable_probe_is_not_measurable() {
    let srv = spawn_server().await;
    seed_bare_run(&srv, "run-idle", "exp-idle").await;
    seed_probe(&srv, "exp-idle", "idle", &[("before", 100.0, 30)]).await;
    let (status, body) = get(&srv.base, "/api/runs/run-idle/impact").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["recovery_status"], "not_measurable", "{body}");
    assert_eq!(body["evaluated_probes"], 0);
    assert_eq!(body["unrecovered_probes"], 0);
    assert_eq!(body["undetermined_probes"], 0);
    assert!(body["recovery_time_s"].is_null());
    assert!(body["impact_peak"].is_null());
    let probe = &body["probes"][0];
    assert_eq!(probe["error"], "no during-phase samples", "{probe}");
}

/// Still outside the band at the first sample after the fault and back at
/// the second, where the samples stop: on its way back, not unrecovered.
/// The same probe last seen outside is.
#[tokio::test]
async fn a_short_post_phase_is_judged_by_where_it_ends() {
    let srv = spawn_server().await;
    seed_bare_run(&srv, "run-short", "exp-short").await;
    let fault = [("before", 100.0, 40), ("during", 300.0, 20)];
    let returning = [&fault[..], &[("post", 300.0, 1), ("post", 100.0, 1)]].concat();
    seed_probe(&srv, "exp-short", "returning", &returning).await;
    let relapsed = [&fault[..], &[("post", 100.0, 1), ("post", 300.0, 1)]].concat();
    seed_probe(&srv, "exp-short", "relapsed", &relapsed).await;
    let (status, body) = get(&srv.base, "/api/runs/run-short/impact").await;
    assert_eq!(status, 200, "{body}");
    let probes = body["probes"].as_array().unwrap();
    let by_name = |name: &str| &probes.iter().find(|p| p["probe"] == name).unwrap()["recovery"];
    let returning = by_name("returning");
    assert_eq!(returning["outside_after_fault"], 1, "{returning}");
    assert_eq!(returning["ended_inside"], true, "{returning}");
    assert_eq!(returning["status"], "undetermined", "{returning}");
    let relapsed = by_name("relapsed");
    assert_eq!(relapsed["ended_inside"], false, "{relapsed}");
    assert_eq!(relapsed["status"], "unrecovered", "{relapsed}");
    assert_eq!(body["recovery_status"], "unrecovered", "{body}");
    assert_eq!(body["unrecovered_probes"], 1);
    assert_eq!(body["undetermined_probes"], 1);
}

/// A probe that was only sampled before and during the fault, and one
/// seen twice afterwards, were not watched long enough to be called
/// unrecovered. They are counted apart and leave the headline alone.
#[tokio::test]
async fn a_probe_without_enough_post_samples_is_undetermined_not_unrecovered() {
    let srv = spawn_server().await;
    seed_run(&srv, "run-cut", "exp-cut", 40).await;
    seed_cut_off_probe(&srv, "exp-cut", "cut-off", 0).await;
    seed_cut_off_probe(&srv, "exp-cut", "seen-twice", 2).await;
    seed_stuck_probe(&srv, "exp-other", 40).await;
    let (status, body) = get(&srv.base, "/api/runs/run-cut/impact").await;
    assert_eq!(status, 200, "{body}");
    let probes = body["probes"].as_array().unwrap();
    let by_name = |name: &str| probes.iter().find(|p| p["probe"] == name).unwrap();
    let health = &by_name("health")["recovery"];
    assert_eq!(health["status"], "recovered", "{health}");
    let cut = &by_name("cut-off")["recovery"];
    assert_eq!(cut["status"], "undetermined", "{}", by_name("cut-off"));
    assert_eq!(cut["observed_after_fault"], 0);
    assert!(cut["recovery_time_s"].is_null());
    let twice = &by_name("seen-twice")["recovery"];
    assert_eq!(twice["status"], "undetermined", "{twice}");
    assert_eq!(twice["observed_after_fault"], 2);
    assert_eq!(twice["outside_after_fault"], 0, "{twice}");
    assert_eq!(body["evaluated_probes"], 3);
    assert_eq!(body["unrecovered_probes"], 0);
    assert_eq!(body["undetermined_probes"], 2);
    assert_eq!(
        body["recovery_time_s"], health["recovery_time_s"],
        "the headline is the probe that was watched"
    );
    assert!(body["recovery_time_s"].is_number());
    assert_eq!(body["recovery_status"], "recovered");
    assert!(cut["ended_inside"].is_null(), "{cut}");
}

/// A run where no probe was watched after the fault has no recovery time,
/// and no probe is called unrecovered for it.
#[tokio::test]
async fn a_run_with_only_undetermined_probes_has_no_recovery_time() {
    let srv = spawn_server().await;
    seed_bare_run(&srv, "run-blind", "exp-blind").await;
    seed_cut_off_probe(&srv, "exp-blind", "cut-off", 0).await;
    let (status, body) = get(&srv.base, "/api/runs/run-blind/impact").await;
    assert_eq!(status, 200, "{body}");
    assert!(body["recovery_time_s"].is_null());
    assert_eq!(body["unrecovered_probes"], 0);
    assert_eq!(body["undetermined_probes"], 1);
    assert_eq!(body["evaluated_probes"], 1);
    assert_eq!(body["recovery_status"], "undetermined");
    assert!(body["impact_peak"].as_f64().unwrap() > 1.0, "{body}");
}

/// A probe that stayed outside its band is unrecovered however the others
/// were watched.
#[tokio::test]
async fn a_stuck_probe_is_unrecovered_next_to_an_undetermined_one() {
    let srv = spawn_server().await;
    seed_bare_run(&srv, "run-mixed", "exp-mixed").await;
    seed_cut_off_probe(&srv, "exp-mixed", "cut-off", 0).await;
    seed_stuck_probe(&srv, "exp-mixed", 40).await;
    let (status, body) = get(&srv.base, "/api/runs/run-mixed/impact").await;
    assert_eq!(status, 200, "{body}");
    let probes = body["probes"].as_array().unwrap();
    let stuck = probes.iter().find(|p| p["probe"] == "stuck").unwrap();
    assert_eq!(stuck["recovery"]["status"], "unrecovered", "{stuck}");
    assert!(body["recovery_time_s"].is_null());
    assert_eq!(body["unrecovered_probes"], 1);
    assert_eq!(body["undetermined_probes"], 1);
}

/// With samples from before the fault, the counterfactual is a forecast
/// forward from them rather than a backcast.
#[tokio::test]
async fn impact_forecasts_from_the_baseline_when_there_is_one() {
    let srv = spawn_server().await;
    seed_run(&srv, "run-baseline", "exp-baseline", 40).await;
    let (status, body) = get(&srv.base, "/api/runs/run-baseline/impact").await;
    assert_eq!(status, 200, "{body}");
    let p = &body["probes"][0];
    assert_eq!(p["reference"], "baseline", "{p}");
    assert_eq!(p["series"]["ts"].as_array().unwrap().len(), 100);
    assert_eq!(p["forecast"]["context_points"], 40);
    let fault_start = p["fault"]["start_ms"].as_i64().unwrap();
    assert_eq!(
        fault_start,
        p["series"]["ts"][40].as_i64().unwrap(),
        "the fault starts after the forty baseline samples"
    );
    // Nothing is compared before the fault.
    assert!(p["impact"]["per_sample"][39].is_null());
    assert!(p["impact"]["per_sample"][40].is_number());
    assert_eq!(p["impact"]["samples_during"], 20);
    assert!(p["impact"]["during_peak"].as_f64().unwrap() > 1.0, "{p}");
    assert_eq!(p["degenerate_band"], true, "a flat baseline: {p}");
    let seconds = p["recovery"]["recovery_time_s"].as_f64().unwrap();
    assert!((1.0..=6.0).contains(&seconds), "{p}");
}

#[tokio::test]
async fn impact_respects_run_scopes() {
    let srv = spawn_server().await;
    seed_run_with_samples(&srv, "run-scoped-impact", "exp-scoped-impact").await;
    let token = add_scoped_token(&srv, "impact-scoped", "viewer", &["staging"]).await;
    let (status, body): (u16, Value) =
        get_auth(&srv.base, "/api/runs/run-scoped-impact/impact", &token).await;
    assert_eq!(status, 404, "a run outside the scopes is invisible: {body}");
}

/// A stand-in sidecar: counts `POST /forecast` calls and answers 500 until
/// it is marked healthy, then a flat three-level forecast.
#[derive(Clone, Default)]
struct FakeSidecar {
    hits: Arc<AtomicUsize>,
    healthy: Arc<AtomicBool>,
    reject: Arc<AtomicBool>,
    /// Answer every request with this status when it is not zero.
    status: Arc<std::sync::atomic::AtomicU16>,
    /// Answer 200 with other quantile levels than the ones asked for.
    other_levels: Arc<AtomicBool>,
}

async fn fake_forecast(
    axum::extract::State(sidecar): axum::extract::State<FakeSidecar>,
    axum::Json(request): axum::Json<Value>,
) -> (axum::http::StatusCode, axum::Json<Value>) {
    sidecar.hits.fetch_add(1, Ordering::SeqCst);
    let forced = sidecar.status.load(Ordering::SeqCst);
    if forced != 0 {
        return (
            axum::http::StatusCode::from_u16(forced).unwrap(),
            axum::Json(serde_json::json!({"error": "forced"})),
        );
    }
    if sidecar.reject.load(Ordering::SeqCst) {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({"error": "context too short"})),
        );
    }
    if !sidecar.healthy.load(Ordering::SeqCst) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": "model not loaded"})),
        );
    }
    let horizon = request["horizon"].as_u64().unwrap_or(1);
    let steps: Vec<Value> = (0..horizon)
        .map(|_| serde_json::json!([90.0, 100.0, 110.0]))
        .collect();
    let levels = if sidecar.other_levels.load(Ordering::SeqCst) {
        serde_json::json!([0.2, 0.5, 0.8])
    } else {
        serde_json::json!([0.1, 0.5, 0.9])
    };
    let body = serde_json::json!({
        "levels": levels,
        "context_points": request["context"].as_array().map_or(0, Vec::len),
        "quantiles": [steps],
        "model": {"model": "timestral", "model_id": "mistralai/Timestral-5M",
                  "version": "1.0.0", "revision": "rev1", "weights_hash": "feed"},
    });
    (axum::http::StatusCode::OK, axum::Json(body))
}

async fn spawn_fake_sidecar() -> (String, FakeSidecar) {
    let sidecar = FakeSidecar::default();
    let app = axum::Router::new()
        .route("/forecast", axum::routing::post(fake_forecast))
        .with_state(sidecar.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), sidecar)
}

#[tokio::test]
async fn a_failing_sidecar_opens_the_circuit_and_is_retried_after_the_cooldown() {
    let (url, sidecar) = spawn_fake_sidecar().await;
    let srv = spawn_server_with_forecast_sidecar(&url, Duration::from_secs(3)).await;
    seed_run_with_samples(&srv, "run-breaker", "exp-breaker").await;

    let (status, first) = get(&srv.base, "/api/runs/run-breaker/impact").await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["provenance"]["model"], "holt-winters");
    // A coarse code: what the sidecar said stays in the daemon's log.
    assert_eq!(first["provenance"]["fallback_reason"], "bad_response");
    assert!(!first.to_string().contains("model not loaded"), "{first}");
    assert_eq!(sidecar.hits.load(Ordering::SeqCst), 1);

    // The circuit is open: the fallback answers without calling the sidecar.
    let (status, second) = get(&srv.base, "/api/runs/run-breaker/impact").await;
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["provenance"]["model"], "holt-winters");
    assert_eq!(second["provenance"]["fallback_reason"], "circuit_open");
    assert_eq!(sidecar.hits.load(Ordering::SeqCst), 1);

    // Once the cool-down has passed the sidecar is tried again. (The
    // cool-down is seconds, not milliseconds, so a loaded host cannot let it
    // lapse between the two requests above.)
    sidecar.healthy.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let recovered = loop {
        let (_, body) = get(&srv.base, "/api/runs/run-breaker/impact").await;
        if body["provenance"]["model"] == "timestral" {
            break body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never retried: {body}"
        );
        // sleep: poll interval until the cooldown has passed
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(recovered["provenance"]["fallback_reason"].is_null());
    assert_eq!(
        recovered["provenance"]["version"],
        "mistralai/Timestral-5M@rev1 (timestral 1.0.0)"
    );
    assert_eq!(recovered["provenance"]["weights_hash"], "feed");
    assert_eq!(
        sidecar.hits.load(Ordering::SeqCst),
        2,
        "one probe closed the circuit"
    );
    // The sidecar's own band (90..110) is wider than the floor and is kept.
    assert_eq!(recovered["probes"][0]["forecast"]["degenerate_band"], false);
}

#[tokio::test]
async fn forecast_caches_per_provider_and_serves_the_fallback_while_the_circuit_is_open() {
    let (url, sidecar) = spawn_fake_sidecar().await;
    sidecar.healthy.store(true, Ordering::SeqCst);
    let srv = spawn_server_with_forecast_sidecar(&url, Duration::from_secs(600)).await;
    let path = "/api/forecast?metric=experiment_count&range=24h&horizon=30";

    let (live, again) = get_in_one_window(&srv, path, path).await;
    assert_eq!(live["provenance"]["model"], "timestral");
    assert_eq!(live["cached"], false);
    assert_eq!(again["cached"], true, "the sidecar's row is a hit: {again}");
    assert_eq!(again["provenance"]["model"], "timestral");
    let calls = sidecar.hits.load(Ordering::SeqCst);
    assert!(calls >= 1);

    // A horizon in another bucket misses the cache; the sidecar now fails.
    sidecar.healthy.store(false, Ordering::SeqCst);
    let (status, down) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&range=24h&horizon=5",
    )
    .await;
    assert_eq!(status, 200, "{down}");
    assert_eq!(down["provenance"]["model"], "holt-winters");
    assert_eq!(down["provenance"]["fallback_reason"], "bad_response");
    assert_eq!(down["cached"], false);
    assert_eq!(sidecar.hits.load(Ordering::SeqCst), calls + 1);

    // While the circuit is open the built-in provider's rows are the ones
    // looked up, and the sidecar is left alone.
    let (fitted, open) = get_in_one_window(&srv, path, path).await;
    assert_eq!(fitted["cached"], false);
    assert_eq!(fitted["provenance"]["fallback_reason"], "circuit_open");
    assert_eq!(open["provenance"]["model"], "holt-winters");
    assert_eq!(open["cached"], true, "the fallback row is served: {open}");
    assert_eq!(open["provenance"]["fallback_reason"], "circuit_open");
    assert_eq!(sidecar.hits.load(Ordering::SeqCst), calls + 1);
}

#[tokio::test]
async fn a_rejected_request_falls_back_without_opening_the_circuit() {
    let (url, sidecar) = spawn_fake_sidecar().await;
    sidecar.reject.store(true, Ordering::SeqCst);
    let srv = spawn_server_with_forecast_sidecar(&url, Duration::from_secs(600)).await;
    seed_run_with_samples(&srv, "run-rejected", "exp-rejected").await;

    for expected_hits in 1..=2 {
        let (status, body) = get(&srv.base, "/api/runs/run-rejected/impact").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["provenance"]["model"], "holt-winters");
        assert_eq!(body["provenance"]["fallback_reason"], "rejected");
        assert!(!body.to_string().contains("context too short"), "{body}");
        assert_eq!(sidecar.hits.load(Ordering::SeqCst), expected_hits);
    }
}

/// A throttled request costs that request its model forecast and no
/// other: the reason is `busy`, and the next request is sent as well.
#[tokio::test]
async fn a_throttled_request_falls_back_without_opening_the_circuit() {
    let (url, sidecar) = spawn_fake_sidecar().await;
    sidecar.status.store(429, Ordering::SeqCst);
    let srv = spawn_server_with_forecast_sidecar(&url, Duration::from_secs(600)).await;
    seed_run_with_samples(&srv, "run-busy", "exp-busy").await;
    for expected_hits in 1..=2 {
        let (status, body) = get(&srv.base, "/api/runs/run-busy/impact").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["provenance"]["model"], "holt-winters");
        assert_eq!(body["provenance"]["fallback_reason"], "busy");
        assert_eq!(sidecar.hits.load(Ordering::SeqCst), expected_hits);
    }
    sidecar.status.store(0, Ordering::SeqCst);
    sidecar.healthy.store(true, Ordering::SeqCst);
    let (_, body) = get(&srv.base, "/api/runs/run-busy/impact").await;
    assert_eq!(body["provenance"]["model"], "timestral", "{body}");
}

/// A status that every request would get (a wrong URL, a proxy asking for
/// credentials, a sidecar that is down) opens the circuit: one request
/// pays for finding out, the rest fall back at once.
#[tokio::test]
async fn a_status_that_is_not_about_the_request_opens_the_circuit() {
    for (status, reason) in [
        (404, "bad_response"),
        (401, "bad_response"),
        (403, "bad_response"),
        (503, "bad_response"),
        (408, "timeout"),
    ] {
        let (url, sidecar) = spawn_fake_sidecar().await;
        sidecar.status.store(status, Ordering::SeqCst);
        let srv = spawn_server_with_forecast_sidecar(&url, Duration::from_secs(600)).await;
        seed_run_with_samples(&srv, "run-status", "exp-status").await;
        let (_, first) = get(&srv.base, "/api/runs/run-status/impact").await;
        assert_eq!(first["provenance"]["fallback_reason"], reason, "{status}");
        let (_, second) = get(&srv.base, "/api/runs/run-status/impact").await;
        assert_eq!(
            second["provenance"]["fallback_reason"], "circuit_open",
            "{status}"
        );
        assert_eq!(sidecar.hits.load(Ordering::SeqCst), 1, "{status}");
    }
}

/// A sidecar that answers 200 with other levels than the ones asked for
/// (a model snapping to its own grid) has not produced the band the
/// endpoints serve. Both fall back; neither is a 500.
#[tokio::test]
async fn mismatched_sidecar_levels_fall_back_and_never_fail_the_request() {
    let (url, sidecar) = spawn_fake_sidecar().await;
    sidecar.healthy.store(true, Ordering::SeqCst);
    sidecar.other_levels.store(true, Ordering::SeqCst);
    // No cool-down: every request reaches the sidecar.
    let srv = spawn_server_with_forecast_sidecar(&url, Duration::ZERO).await;
    seed_run_with_samples(&srv, "run-levels", "exp-levels").await;

    let (status, impact) = get(&srv.base, "/api/runs/run-levels/impact").await;
    assert_eq!(status, 200, "{impact}");
    assert_eq!(impact["provenance"]["model"], "holt-winters");
    assert_eq!(impact["provenance"]["fallback_reason"], "bad_response");
    assert!(impact["probes"][0]["error"].is_null(), "{impact}");
    assert!(impact["impact_peak"].is_number(), "{impact}");

    let (status, forecast) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&range=24h&horizon=4",
    )
    .await;
    assert_eq!(status, 200, "{forecast}");
    assert_eq!(forecast["provenance"]["model"], "holt-winters");
    assert_eq!(forecast["provenance"]["fallback_reason"], "bad_response");
    assert_eq!(
        forecast["forecast"]["levels"],
        serde_json::json!([0.1, 0.5, 0.9])
    );
    assert_eq!(sidecar.hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_unreachable_sidecar_costs_one_attempt_per_cooldown() {
    // Nothing listens on port 1: the connection is refused.
    let srv =
        spawn_server_with_forecast_sidecar("http://127.0.0.1:1", Duration::from_secs(600)).await;
    seed_run_with_samples(&srv, "run-refused", "exp-refused").await;
    let (_, first) = get(&srv.base, "/api/runs/run-refused/impact").await;
    assert_eq!(first["provenance"]["fallback_reason"], "unreachable");
    let started = std::time::Instant::now();
    let (status, second) = get(&srv.base, "/api/runs/run-refused/impact").await;
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["provenance"]["fallback_reason"], "circuit_open");
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// A sidecar URL the daemon could never call is refused when the state is
/// built, not discovered on the first forecast.
#[test]
fn a_sidecar_url_that_is_not_http_is_refused_at_start_up() {
    let state = || {
        tumult_api::ApiState::new(
            "unused.duckdb".into(),
            "unused".into(),
            "unused".into(),
            Arc::new(tumult_intelligence::llm::OpenAiCompatClient::from_env()),
            tumult_compliance::OrgTree::empty(),
            None,
            None,
            None,
            false,
        )
    };
    for bad in ["file:///etc/passwd", "forecastd:8700", "ftp://forecastd"] {
        let Err(message) = state().with_forecast_url(Some(bad.into())) else {
            panic!("{bad} was accepted");
        };
        assert!(!message.is_empty());
    }
    let none = state().with_forecast_url(None).unwrap();
    assert!(none.forecast_url().is_none());
    let blank = state().with_forecast_url(Some("  ".into())).unwrap();
    assert!(blank.forecast_url().is_none());
    let good = state()
        .with_forecast_url(Some("http://forecastd:8700/".into()))
        .unwrap();
    assert_eq!(good.forecast_url(), Some("http://forecastd:8700"));
}
