use crate::common::*;
use serde_json::{json, Value};
use tumult_lake::{SpanRow, Store};

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

    // The seed is two runs 20 minutes apart, 30 and 10 minutes ago: one
    // UTC day, or two when midnight falls between them.
    let per_day = body["experiments_per_day"].as_array().unwrap();
    assert!((1..=2).contains(&per_day.len()), "{per_day:?}");
    let runs: f64 = per_day.iter().map(|day| day["v"].as_f64().unwrap()).sum();
    assert!((runs - 2.0).abs() < f64::EPSILON, "{per_day:?}");
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
    // Newest first. The seed is two runs stored before 3.0.2: the outcome
    // is on the completed log, spelled with a capital.
    assert_eq!(rows[0]["id"], "exp-fail");
    assert_eq!(rows[0]["status"], "deviated");
    assert_eq!(rows[1]["status"], "completed");
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

    // `name` is the whole name, not a part of it.
    let (status, body) = get(&srv.base, "/api/experiments?name=cache-stampede").await;
    assert_eq!(status, 200, "{body}");
    let rows = body["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "cache-stampede");
    let (_, body) = get(&srv.base, "/api/experiments?name=cache").await;
    assert_eq!(body["experiments"].as_array().unwrap().len(), 0);
    let (_, body) = get(&srv.base, "/api/experiments?name=CACHE-STAMPEDE").await;
    assert_eq!(body["experiments"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn experiment_detail_returns_spans_logs_metrics() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/experiments/exp-pass").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["experiment"]["name"], "pg-failover");
    assert_eq!(body["experiment"]["status"], "completed");
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
    assert_eq!(outcomes, ["completed", "deviated"]);
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
    assert_eq!(rows[0]["status"], "deviated");
    assert_eq!(rows[0]["experiment_id"], "exp-fail");
    assert_eq!(rows[1]["status"], "completed");
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

    // Sum over 1d buckets: both tumult counters total 2 regardless of how
    // the day-aligned bucket boundary falls relative to the fixture rows
    // (a run within ~30 min of UTC midnight splits them across buckets).
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
    let total: f64 = points.iter().map(|p| p["v"].as_f64().unwrap()).sum();
    assert_eq!(total, 2.0, "{points:?}");

    // Gauge averages instead of summing (one fixture row, so every bucket
    // carries the same average).
    let (_, body) = get(
        &srv.base,
        "/api/metrics/query?name=demo.cpu.usage&interval=1d",
    )
    .await;
    for p in body["series"][0]["points"].as_array().unwrap() {
        assert_eq!(p["v"], 0.5, "{p:?}");
    }

    // Histogram: avg = sum/count, p95 clamps into the overflow bucket.
    let (_, body) = get(
        &srv.base,
        "/api/metrics/query?name=demo.latency&interval=1d",
    )
    .await;
    assert_eq!(body["type"], "histogram");
    for point in body["series"][0]["points"].as_array().unwrap() {
        assert_eq!(point["avg"], 150.0, "{point:?}");
        assert_eq!(point["p95"], 200.0, "{point:?}");
    }

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
    // Groups sort by label: /api before /web; /api sums 10 + 5. Sum across
    // points: near UTC midnight the fixture rows straddle two day-aligned
    // buckets, so no single point carries the total.
    assert_eq!(series[0]["group"], "/api");
    let api_total: f64 = series[0]["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["v"].as_f64().unwrap())
        .sum();
    assert_eq!(api_total, 15.0, "{series:?}");
    assert_eq!(series[1]["group"], "/web");
    let web_total: f64 = series[1]["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["v"].as_f64().unwrap())
        .sum();
    assert_eq!(web_total, 20.0, "{series:?}");

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
    // Targets are the systems the naming contract names, under the id the
    // Star Map uses.
    let tgt = nodes
        .iter()
        .find(|n| n["id"] == "sys:demo/database")
        .unwrap();
    assert_eq!(tgt["type"], "target");
    assert_eq!(tgt["name"], "database");
    assert_eq!(tgt["runs"], 2);
    // A span that names its target only through `resilience.target.name`
    // (the runner's action spans) keeps its target, as a system without
    // an environment.
    let named = nodes
        .iter()
        .find(|n| n["id"] == "sys:-/postgres-1")
        .unwrap_or_else(|| panic!("{nodes:?}"));
    assert_eq!(named["type"], "target");
    assert_eq!(named["name"], "postgres-1");
    assert_eq!(named["runs"], 2);
    assert!(
        nodes
            .iter()
            .all(|n| !n["id"].as_str().unwrap().starts_with("tgt:")),
        "{nodes:?}"
    );

    let edges = body["edges"].as_array().unwrap();
    // Service → target edges from the spans that name the system.
    assert!(edges.iter().any(|e| e["from_id"] == "svc:tumult"
        && e["to_id"] == "sys:demo/database"
        && e["weight"] == 2));
    // Intra-service parent→child hops with differing span names survive as
    // a self-loop (root experiment span → action span).
    assert!(edges
        .iter()
        .any(|e| e["from_id"] == "svc:tumult" && e["to_id"] == "svc:tumult"));

    assert!(edges.iter().any(|e| e["from_id"] == "svc:tumult"
        && e["to_id"] == "sys:-/postgres-1"
        && e["weight"] == 2));

    let (status, body) = get(&srv.base, "/api/topology?range=1y").await;
    assert_eq!(status, 400);
    assert_eq!(
        body["error"], "invalid range \"1y\"; expected 24h|7d|14d|30d",
        "one range table for every endpoint"
    );
    let (status, body) = get(&srv.base, "/api/topology?range=24h").await;
    assert_eq!(status, 200);
    assert_eq!(body["nodes"].as_array().unwrap().len(), 3);
}

/// Without a `range` the topology reads the last seven days, never the
/// whole lake.
#[tokio::test]
async fn topology_is_bounded_without_a_range() {
    let srv = spawn_server().await;
    let old = |service: &str, age_days: i64| {
        let mut span = tumult_lake::SpanRow {
            ts_ns: now_ns() - age_days * 86_400 * NS,
            trace_id: format!("trace-{service}"),
            span_id: format!("span-{service}"),
            parent_span_id: None,
            span_name: "GET /".into(),
            span_kind: "Server".into(),
            duration_ns: NS / 100,
            status_code: "Ok".into(),
            status_message: String::new(),
            service_name: service.into(),
            service_version: None,
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
        };
        span.target_system = Some(format!("{service}-system"));
        span
    };
    let rows = vec![old("archive", 10), old("vault", 20), old("fossil", 40)];
    exec_write(&srv, move |w| {
        w.insert_spans(&rows).map_err(|e| e.to_string())
    })
    .await;
    let has = |body: &Value, id: &str| {
        body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == id)
    };

    let (status, body) = get(&srv.base, "/api/topology").await;
    assert_eq!(status, 200, "{body}");
    assert!(has(&body, "svc:tumult"));
    assert!(!has(&body, "svc:archive"), "the default is seven days");
    let (_, body) = get(&srv.base, "/api/topology?range=14d").await;
    assert!(has(&body, "svc:archive"));
    assert!(!has(&body, "svc:vault"));
    let (status, body) = get(&srv.base, "/api/topology?range=30d").await;
    assert_eq!(status, 200, "{body}");
    assert!(has(&body, "svc:vault"));
    assert!(!has(&body, "svc:fossil"));
}

/// Every endpoint that takes a `range` reads the same table.
#[tokio::test]
async fn every_ranged_endpoint_accepts_thirty_days() {
    let srv = spawn_server().await;
    for path in [
        "/api/experiments",
        "/api/overview",
        "/api/traces",
        "/api/logs",
        "/api/scores",
        "/api/starmap",
        "/api/topology",
    ] {
        let (status, body) = get(&srv.base, &format!("{path}?range=30d")).await;
        assert_eq!(status, 200, "{path}: {body}");
        let (status, body) = get(&srv.base, &format!("{path}?range=31d")).await;
        assert_eq!(status, 400, "{path}: {body}");
        let error = body["error"].as_str().unwrap_or_default();
        assert!(error.contains("24h|7d|14d|30d"), "{path}: {body}");
    }
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

/// `POST /api/import/journal` ingests through the daemon's single-writer
/// channel; the rows are then visible to a read-only connection, and a
/// repeat POST dedups on `experiment_id`.
#[tokio::test]
async fn import_journal_roundtrip_and_dedup() {
    let srv = spawn_server().await;
    let journal = json!({
        "experiment_title": "imported via api",
        "experiment_id": "exp-imported",
        "status": "completed",
        "started_at_ns": 1_774_980_000_000_000_000_i64,
        "ended_at_ns": 1_774_980_300_000_000_000_i64,
        "duration_ms": 300_000,
        "steady_state_before": null,
        "steady_state_after": null,
        "method_results": [],
        "rollback_results": [],
        "estimate": null,
        "baseline_result": null,
        "during_result": null,
        "post_result": null,
        "load_result": null,
        "analysis": null,
        "regulatory": null,
    });
    let client = reqwest::Client::new();
    let post = |body: &Value| {
        client
            .post(format!("{}/api/import/journal", srv.base))
            .json(body)
            .send()
    };

    let resp = post(&json!({"journal": journal, "experiment": null}))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ingested"], true, "{body}");
    assert_eq!(body["experiment_id"], "exp-imported");

    // Duplicate: skipped, not an error, not a second row.
    let resp = post(&json!({"journal": journal, "experiment": null}))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ingested"], false, "{body}");

    // Readable through a read-only connection on the same store file.
    let reader = Store::at(&srv._tmp.path().join("k.duckdb"))
        .read_only()
        .unwrap();
    let rows = reader
        .query_json_rows("SELECT experiment_id, status FROM experiments")
        .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["experiment_id"], json!("exp-imported"));
    assert_eq!(rows[0]["status"], json!("completed"));
}

#[tokio::test]
async fn a_journal_import_drops_the_cached_star_maps() {
    let srv = spawn_server().await;
    let journal = json!({
        "experiment_title": "imported via api",
        "experiment_id": "exp-star-import",
        "status": "completed",
        "started_at_ns": now_ns() - 600 * NS,
        "ended_at_ns": now_ns() - 300 * NS,
        "duration_ms": 300_000,
        "steady_state_before": null,
        "steady_state_after": null,
        "method_results": [],
        "rollback_results": [],
        "estimate": null,
        "baseline_result": null,
        "during_result": null,
        "post_result": null,
        "load_result": null,
        "analysis": null,
        "regulatory": null,
    });
    let import = || async {
        let resp = reqwest::Client::new()
            .post(format!("{}/api/import/journal", srv.base))
            .json(&json!({"journal": journal, "experiment": null}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        resp.json::<Value>().await.unwrap()["ingested"].clone()
    };
    let cached = starmap_as_of(&srv.base).await;
    assert_eq!(
        starmap_as_of(&srv.base).await,
        cached,
        "served from the cache"
    );

    assert_eq!(import().await, true);
    let after = starmap_as_of(&srv.base).await;
    assert_ne!(after, cached, "the imported run shows at once");

    // A duplicate stores nothing, so the cached map still stands.
    assert_eq!(import().await, false);
    assert_eq!(starmap_as_of(&srv.base).await, after);
}

/// The endings of a run other than `completed` and `deviated`, which the
/// seed has.
const OTHER_ENDINGS: [&str; 4] = ["failed", "aborted", "interrupted", "halted"];

/// The root span of run `id` against the system `queue`, as 3.0.2 stores
/// it: the span says how the run ended. `None` is a run that has not ended.
fn root_span(id: &str, status: Option<&str>) -> SpanRow {
    SpanRow {
        ts_ns: now_ns() - 300 * NS,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 5 * NS,
        status_code: "Error".into(),
        service_name: "tumult".into(),
        experiment_id: Some(id.into()),
        experiment_name: Some(format!("name-{id}")),
        outcome_status: status.map(str::to_string),
        target_system: Some("queue".into()),
        target_environment: Some("demo".into()),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// One run for each of [`OTHER_ENDINGS`], named `run-<ending>`. None of
/// them has a completed log.
async fn seed_other_endings(srv: &TestServer) {
    let rows: Vec<SpanRow> = OTHER_ENDINGS
        .iter()
        .map(|status| root_span(&format!("run-{status}"), Some(status)))
        .collect();
    exec_write(srv, move |w| {
        w.insert_spans(&rows).map_err(|e| e.to_string())
    })
    .await;
}

fn rows_of<'a>(body: &'a Value, key: &str) -> &'a Vec<Value> {
    body[key]
        .as_array()
        .unwrap_or_else(|| panic!("no {key}: {body}"))
}

#[tokio::test]
async fn a_run_is_listed_and_filtered_by_the_status_on_its_root_span() {
    let srv = spawn_server().await;
    seed_other_endings(&srv).await;

    for status in OTHER_ENDINGS {
        let (code, body) = get(&srv.base, &format!("/api/experiments?outcome={status}")).await;
        assert_eq!(code, 200, "{body}");
        let rows = rows_of(&body, "experiments");
        assert_eq!(rows.len(), 1, "{status}: {rows:?}");
        assert_eq!(rows[0]["id"], format!("run-{status}"));
        assert_eq!(rows[0]["status"], status);

        let (code, body) = get(&srv.base, &format!("/api/traces?outcome={status}")).await;
        assert_eq!(code, 200, "{body}");
        let rows = rows_of(&body, "traces");
        assert_eq!(rows.len(), 1, "{status}: {rows:?}");
        assert_eq!(rows[0]["trace_id"], format!("trace-run-{status}"));
        assert_eq!(rows[0]["status"], status);
    }

    let (_, body) = get(&srv.base, "/api/experiments/run-halted").await;
    assert_eq!(body["experiment"]["status"], "halted");
    let (_, body) = get(&srv.base, "/api/experiments?outcome=incomplete").await;
    assert_eq!(rows_of(&body, "experiments").len(), 0);
    let (_, body) = get(&srv.base, "/api/traces?outcome=incomplete").await;
    assert_eq!(rows_of(&body, "traces").len(), 0);
}

#[tokio::test]
async fn the_outcome_filter_takes_either_spelling() {
    let srv = spawn_server().await;

    for outcome in ["deviated", "Deviated", "DEVIATED"] {
        let (code, body) = get(&srv.base, &format!("/api/experiments?outcome={outcome}")).await;
        assert_eq!(code, 200, "{body}");
        assert_eq!(rows_of(&body, "experiments")[0]["id"], "exp-fail");
        let (code, body) = get(&srv.base, &format!("/api/traces?outcome={outcome}")).await;
        assert_eq!(code, 200, "{body}");
        assert_eq!(rows_of(&body, "traces")[0]["trace_id"], "trace-exp-fail");
    }

    let (code, body) = get(&srv.base, "/api/experiments?outcome=bogus").await;
    assert_eq!(code, 400);
    assert_eq!(
        body["error"],
        "invalid outcome \"bogus\"; expected \
         completed|deviated|failed|aborted|interrupted|halted|incomplete"
    );
}

#[tokio::test]
async fn a_run_that_has_not_ended_is_incomplete() {
    let srv = spawn_server().await;
    let row = root_span("run-open", None);
    exec_write(&srv, move |w| {
        w.insert_spans(&[row]).map_err(|e| e.to_string())
    })
    .await;

    let (_, body) = get(&srv.base, "/api/experiments?outcome=incomplete").await;
    let rows = rows_of(&body, "experiments");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["id"], "run-open");
    assert!(rows[0]["status"].is_null(), "{}", rows[0]);
    let (_, body) = get(&srv.base, "/api/traces?outcome=incomplete").await;
    assert_eq!(rows_of(&body, "traces")[0]["trace_id"], "trace-run-open");
}

#[tokio::test]
async fn the_outcome_facet_lists_each_status_once_in_one_spelling() {
    let srv = spawn_server().await;
    seed_other_endings(&srv).await;
    // A completed run stored by 3.0.2 next to the one of the seed, which is
    // spelled with a capital.
    let row = root_span("run-completed", Some("completed"));
    exec_write(&srv, move |w| {
        w.insert_spans(&[row]).map_err(|e| e.to_string())
    })
    .await;

    let (status, body) = get(&srv.base, "/api/dimensions").await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["outcomes"],
        json!([
            "aborted",
            "completed",
            "deviated",
            "failed",
            "halted",
            "interrupted"
        ])
    );
}

#[tokio::test]
async fn the_pass_rate_of_a_target_counts_runs_in_either_spelling() {
    let srv = spawn_server().await;
    seed_other_endings(&srv).await;
    let row = root_span("run-completed", Some("completed"));
    exec_write(&srv, move |w| {
        w.insert_spans(&[row]).map_err(|e| e.to_string())
    })
    .await;

    let (status, body) = get(&srv.base, "/api/overview?range=24h").await;

    assert_eq!(status, 200, "{body}");
    let pass_rate = |target: &str| {
        rows_of(&body, "targets")
            .iter()
            .find(|row| row["target"] == target)
            .unwrap_or_else(|| panic!("no target {target}: {body}"))["pass_rate"]
            .clone()
    };
    // The seed: one of two runs against the database completed.
    assert_eq!(pass_rate("database"), 0.5);
    // One of the five runs against the queue completed.
    assert_eq!(pass_rate("queue"), 0.2);
}

#[tokio::test]
async fn ask_names_the_runs_that_did_not_complete_in_either_spelling() {
    let srv = spawn_server().await;
    let row = root_span("run-halted", Some("halted"));
    exec_write(&srv, move |w| {
        w.insert_spans(&[row]).map_err(|e| e.to_string())
    })
    .await;

    let resp = reqwest::Client::new()
        .post(format!("{}/api/ask", srv.base))
        .json(&json!({"question": "Which experiments failed?"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let found: Vec<(&str, &str)> = rows_of(&body, "rows")
        .iter()
        .map(|row| {
            (
                row["experiment_id"].as_str().unwrap(),
                row["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [("run-halted", "halted"), ("exp-fail", "deviated")],
        "{body}"
    );
}

#[tokio::test]
async fn ask_lists_the_recent_runs_with_how_each_ended_in_one_spelling() {
    let srv = spawn_server().await;
    // The runs of the seed have their status on the log, with a capital.
    // This one has it on its root span.
    let row = root_span("run-halted", Some("halted"));
    exec_write(&srv, move |w| {
        w.insert_spans(&[row]).map_err(|e| e.to_string())
    })
    .await;

    let resp = reqwest::Client::new()
        .post(format!("{}/api/ask", srv.base))
        .json(&json!({"question": "Show recent experiments"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let found: Vec<(&str, &str)> = rows_of(&body, "rows")
        .iter()
        .map(|row| {
            (
                row["experiment_id"].as_str().unwrap(),
                row["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("run-halted", "halted"),
            ("exp-fail", "deviated"),
            ("exp-pass", "completed")
        ],
        "{body}"
    );
}
