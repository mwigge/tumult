//! `GET /api/forecast` and `GET /api/runs/{id}/impact` over the seeded
//! store: the built-in provider answers (no sidecar is configured), every
//! response names its model, metric forecasts are cached, and the impact
//! endpoint 404s for runs without probe samples.

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

    let (status, again) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&range=24h&horizon=6",
    )
    .await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["cached"], true, "{again}");
    assert_eq!(
        again["forecast"]["quantiles"],
        body["forecast"]["quantiles"]
    );
    assert_eq!(again["provenance"]["model"], "holt-winters");

    // A different horizon is a different cache entry.
    let (_, other) = get(
        &srv.base,
        "/api/forecast?metric=experiment_count&range=24h&horizon=3",
    )
    .await;
    assert_eq!(other["cached"], false);
    assert_eq!(other["forecast"]["quantiles"].as_array().unwrap().len(), 3);
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

async fn seed_run_with_samples(srv: &TestServer, run_id: &str, experiment_id: &str) {
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
        // 1 s samples: 20 during-phase steps where latency triples, then 40
        // post-phase steps settling back around 100 ms.
        let mut rows = Vec::new();
        for i in 0..60_i64 {
            let (phase, value) = if i < 20 {
                (
                    "during",
                    300.0 + f64::from(i32::try_from(i % 3).unwrap_or(0)) * 5.0,
                )
            } else {
                (
                    "post",
                    100.0
                        + f64::from(i32::try_from(i % 4).unwrap_or(0)) * 2.0
                        + if i < 23 { 60.0 } else { 0.0 },
                )
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
    assert!(impact["during_peak"].as_f64().unwrap() > 1.0, "{impact}");
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
    assert_eq!(body["recovery_time_s"], recovery["recovery_time_s"]);
    assert!(p["forecast"]["context_points"].as_u64().unwrap() >= 30);
    assert_eq!(p["provenance"]["model"], "holt-winters");
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
