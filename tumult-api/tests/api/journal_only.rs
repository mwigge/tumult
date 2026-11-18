//! A run that is in the lake only as a journal.
//!
//! A plain `tumult run` and a journal import leave no trace: the lake holds
//! the journal of the run and no span. Each test here empties the lake of
//! spans and logs, stores journals, and reads the runs back from the pages
//! that list runs.

use crate::common::*;
use serde_json::{json, Value};
use tumult_core::types::{Experiment, ExperimentStatus, ExperimentTarget, Journal};
use tumult_lake::{MetricSumRow, SpanRow};

const TITLE: &str = "payments-replica-loss";

/// The journal of a 30 second run that started `age_s` seconds ago.
fn journal(id: &str, status: ExperimentStatus, age_s: i64) -> Journal {
    let mut journal = tumult_lake::duckdb_store::sample_journal(id, status);
    journal.experiment_title = TITLE.into();
    journal.started_at_ns = now_ns() - age_s * NS;
    journal.ended_at_ns = journal.started_at_ns + 30 * NS;
    journal.duration_ms = 30_000;
    journal
}

/// The definition of the run: it tests `payments-api` in `environment`.
fn definition(environment: Option<&str>) -> Experiment {
    Experiment {
        title: TITLE.into(),
        target: Some(ExperimentTarget {
            system: "payments-api".into(),
            technology: Some("postgresql".into()),
            environment: environment.map(Into::into),
            criticality: None,
            namespace: None,
            version: None,
        }),
        ..Experiment::default()
    }
}

type StoredRun = (Journal, Option<Experiment>);

/// A run that completed ten minutes ago, with a definition that names
/// `environment`.
fn completed_run(id: &str, environment: Option<&str>) -> StoredRun {
    let stored = journal(id, ExperimentStatus::Completed, 600);
    (stored, Some(definition(environment)))
}

/// The value of the experiments card of an overview, and the runs its
/// sparkline and its chart of runs per day add up to.
fn overview_counts(body: &Value) -> (f64, f64, f64) {
    let card = body["kpis"]
        .as_array()
        .unwrap()
        .iter()
        .find(|kpi| kpi["name"] == "experiments")
        .unwrap_or_else(|| panic!("no experiments card: {body}"));
    let total = |points: &Value| -> f64 {
        let points = points.as_array().unwrap();
        points
            .iter()
            .map(|point| point["v"].as_f64().unwrap())
            .sum()
    };
    (
        card["value"].as_f64().unwrap(),
        total(&card["spark"]),
        total(&body["experiments_per_day"]),
    )
}

async fn store(srv: &TestServer, runs: Vec<StoredRun>) {
    exec_write(srv, move |w| {
        for (journal, experiment) in &runs {
            w.ingest_journal(journal, experiment.as_ref())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await;
}

/// A server whose lake holds `runs` as journals, and no span and no log.
async fn server_with_only_journals(runs: Vec<StoredRun>) -> TestServer {
    let srv = spawn_server().await;
    exec_write(&srv, |w| {
        for table in ["spans", "logs"] {
            w.execute(&format!("DELETE FROM {table}"), ())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await;
    store(&srv, runs).await;
    srv
}

/// One deviated run in staging, stored with its definition.
async fn one_deviated_run() -> (TestServer, Journal) {
    let run = journal("cli-1", ExperimentStatus::Deviated, 600);
    let stored = vec![(run.clone(), Some(definition(Some("staging"))))];
    (server_with_only_journals(stored).await, run)
}

fn listed(body: &Value) -> Vec<String> {
    body["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn the_experiments_list_shows_the_run_and_says_journal_only() {
    let (srv, run) = one_deviated_run().await;

    let (status, body) = get(&srv.base, "/api/experiments?range=24h").await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(listed(&body), ["cli-1"], "{body}");
    let row = &body["experiments"][0];
    assert_eq!(row["journal_only"], true, "{row}");
    assert_eq!(row["name"], TITLE);
    assert_eq!(row["status"], "deviated");
    assert_eq!(row["origin"], "automated");
    assert_eq!(row["started_ns"], json!(run.started_at_ns));
    assert_eq!(row["duration_ns"], json!(30 * NS));
    assert_eq!(row["target_system"], "payments-api");
    assert_eq!(row["target_technology"], "postgresql");
    assert_eq!(row["target_environment"], "staging");
}

#[tokio::test]
async fn the_filters_of_the_list_find_the_run() {
    let (srv, _) = one_deviated_run().await;

    for (filter, found) in [
        ("outcome=deviated", true),
        ("outcome=completed", false),
        ("target=payments-api", true),
        ("target=ledger", false),
        ("name=payments-replica-loss", true),
        ("q=cli-", true),
        ("origin=automated", true),
        ("origin=manual", false),
    ] {
        let (status, body) = get(&srv.base, &format!("/api/experiments?{filter}")).await;
        assert_eq!(status, 200, "{filter}: {body}");
        assert_eq!(listed(&body).len(), usize::from(found), "{filter}: {body}");
    }
}

#[tokio::test]
async fn the_page_of_the_run_opens_without_a_trace() {
    let (srv, _) = one_deviated_run().await;

    let (status, body) = get(&srv.base, "/api/experiments/cli-1").await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["experiment"]["journal_only"], true, "{body}");
    assert_eq!(body["experiment"]["status"], "deviated");
    assert_eq!(body["experiment"]["target_system"], "payments-api");
    assert_eq!(body["spans"], json!([]));
    assert_eq!(body["logs"], json!([]));
}

#[tokio::test]
async fn the_facets_name_the_outcome_the_experiment_and_the_target() {
    let (srv, _) = one_deviated_run().await;

    let (status, body) = get(&srv.base, "/api/dimensions").await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["outcomes"], json!(["deviated"]));
    assert_eq!(body["experiments"], json!([TITLE]));
    assert_eq!(body["targets"], json!(["payments-api"]));
}

#[tokio::test]
async fn the_run_marks_its_time_on_the_charts() {
    let (srv, run) = one_deviated_run().await;
    let (from, to) = (run.started_at_ns - NS, run.ended_at_ns + NS);

    let path = format!("/api/experiments/windows?from={from}&to={to}");
    let (status, body) = get(&srv.base, &path).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 1, "{body}");
    assert_eq!(body["runs"][0]["id"], "cli-1");
    assert_eq!(body["runs"][0]["outcome"], "deviated");
    assert_eq!(body["runs"][0]["end_ns"], json!(run.ended_at_ns));
}

#[tokio::test]
async fn the_scorecard_scores_the_experiment_by_its_latest_journal() {
    let earlier = journal("cli-0", ExperimentStatus::Completed, 7200);
    let latest = journal("cli-1", ExperimentStatus::Deviated, 600);
    let staging = || Some(definition(Some("staging")));
    let srv = server_with_only_journals(vec![(earlier, staging()), (latest, staging())]).await;

    let (status, body) = get(&srv.base, "/api/scores").await;

    assert_eq!(status, 200, "{body}");
    let scored = body["experiments"].as_array().unwrap();
    assert_eq!(scored.len(), 1, "{body}");
    assert_eq!(scored[0]["name"], TITLE);
    assert_eq!(scored[0]["journal_only"], true, "{body}");
    assert_eq!(scored[0]["evidence_id"], "cli-1");
    assert_eq!(scored[0]["last_outcome"], "deviated");
    assert_eq!(scored[0]["target"], "payments-api");
    assert_eq!(scored[0]["environment"], "staging");
    assert_eq!(scored[0]["runs"], 2);
    assert_eq!(scored[0]["state"], "failed");
    assert_eq!(scored[0]["score"], 50);
    assert_eq!(body["targets"][0]["target"], "payments-api");
}

#[tokio::test]
async fn the_star_map_counts_the_run_as_evidence_on_its_system() {
    let (srv, _) = one_deviated_run().await;

    let (status, body) = get(&srv.base, "/api/starmap").await;

    assert_eq!(status, 200, "{body}");
    let nodes = body["nodes"].as_array().unwrap();
    let system = nodes
        .iter()
        .find(|node| node["id"] == "sys:staging/payments-api")
        .unwrap_or_else(|| panic!("the system is on the map: {body}"));
    assert_eq!(system["evidence"]["runs"], 1, "{system}");
    assert_eq!(system["evidence"]["failed"], 1, "{system}");
    let tested = format!("sys:staging/payments-api|tested_by|def:name/{TITLE}");
    let edges = body["edges"].as_array().unwrap();
    assert!(
        edges.iter().any(|edge| edge["id"] == tested.as_str()),
        "{body}"
    );
}

#[tokio::test]
async fn the_overview_counts_the_run() {
    let (srv, _) = one_deviated_run().await;

    let (status, body) = get(&srv.base, "/api/overview?range=24h").await;

    assert_eq!(status, 200, "{body}");
    let kpis = body["kpis"].as_array().unwrap();
    let experiments = kpis
        .iter()
        .find(|kpi| kpi["name"] == "experiments")
        .expect("the experiments card");
    assert_eq!(experiments["value"], 1.0, "{experiments}");
    assert_eq!(body["targets"][0]["target"], "payments-api", "{body}");
    assert_eq!(body["targets"][0]["experiments"], 1, "{body}");
}

#[tokio::test]
async fn a_run_that_also_sent_its_trace_is_listed_once_and_not_journal_only() {
    let srv = spawn_server().await;
    let twice = journal("exp-pass", ExperimentStatus::Completed, 1800);
    store(&srv, vec![(twice, Some(definition(Some("staging"))))]).await;

    let (status, body) = get(&srv.base, "/api/experiments").await;

    assert_eq!(status, 200, "{body}");
    let rows = body["experiments"].as_array().unwrap();
    let of_the_run: Vec<&Value> = rows.iter().filter(|row| row["id"] == "exp-pass").collect();
    assert_eq!(of_the_run.len(), 1, "{body}");
    assert_eq!(of_the_run[0]["name"], "pg-failover");
    assert!(
        rows.iter().all(|row| row["journal_only"] == false),
        "{body}"
    );
}

#[tokio::test]
async fn a_scoped_reader_sees_only_the_journals_of_its_environments() {
    let nowhere = (journal("bare", ExperimentStatus::Completed, 600), None);
    let runs = vec![
        completed_run("in-staging", Some("staging")),
        completed_run("in-prod", Some("prod")),
        completed_run("no-environment", None),
        nowhere,
    ];
    let srv = server_with_only_journals(runs).await;
    let token = add_scoped_token(&srv, "staging-viewer", "viewer", &["staging"]).await;

    let (status, body) = get_auth(&srv.base, "/api/experiments", &token).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(listed(&body), ["in-staging"], "{body}");

    let (status, _) = get_auth(&srv.base, "/api/experiments/in-staging", &token).await;
    assert_eq!(status, 200);
    for hidden in ["in-prod", "no-environment", "bare"] {
        let path = format!("/api/experiments/{hidden}");
        let (status, body) = get_auth(&srv.base, &path, &token).await;
        assert_eq!(status, 404, "{hidden}: {body}");
    }

    let (status, body) = get_auth(&srv.base, "/api/dimensions", &token).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["experiments"], json!([TITLE]));
    assert_eq!(body["targets"], json!(["payments-api"]));
}

#[tokio::test]
async fn the_overview_of_a_scoped_reader_counts_only_its_environments() {
    let runs = vec![
        completed_run("in-staging", Some("staging")),
        completed_run("in-prod", Some("prod")),
        completed_run("no-environment", None),
    ];
    let srv = server_with_only_journals(runs).await;

    let (status, all) = get(&srv.base, "/api/overview?range=24h").await;
    assert_eq!(status, 200, "{all}");
    let token = add_scoped_token(&srv, "staging-viewer", "viewer", &["staging"]).await;
    let (status, scoped) = get_auth(&srv.base, "/api/overview?range=24h", &token).await;
    assert_eq!(status, 200, "{scoped}");

    assert_eq!(overview_counts(&all), (3.0, 3.0, 3.0), "{all}");
    assert_eq!(overview_counts(&scoped), (1.0, 1.0, 1.0), "{scoped}");
}

/// The answer of `/api/ask` to `question`, asked with `token` when there
/// is one.
async fn asked(srv: &TestServer, question: &str, token: Option<&str>) -> (u16, Value) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/api/ask", srv.base))
        .json(&json!({ "question": question }));
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let resp = request.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

#[tokio::test]
async fn asked_how_many_experiments_ran_the_answer_is_the_experiments_card() {
    // The seed holds two runs that sent their trace.
    let srv = spawn_server().await;
    store(&srv, vec![completed_run("cli-1", Some("staging"))]).await;

    let (status, overview) = get(&srv.base, "/api/overview?range=24h").await;
    let (asked_status, answer) = asked(&srv, "How many experiments ran?", None).await;

    assert_eq!((status, asked_status), (200, 200), "{overview}\n{answer}");
    assert_eq!(overview_counts(&overview).0, 3.0, "{overview}");
    assert_eq!(answer["rows"], json!([{"experiments": 3}]), "{answer}");
}

#[tokio::test]
async fn a_scoped_question_counts_only_the_runs_of_its_environments() {
    // The seed holds two runs in `demo` that sent their trace.
    let srv = spawn_server().await;
    let nowhere = (journal("bare", ExperimentStatus::Completed, 600), None);
    let runs = vec![
        completed_run("in-staging", Some("staging")),
        completed_run("in-prod", Some("prod")),
        completed_run("no-environment", None),
        nowhere,
    ];
    store(&srv, runs).await;
    let staging = add_scoped_token(&srv, "staging-viewer", "viewer", &["staging"]).await;
    let demo = add_scoped_token(&srv, "demo-viewer", "viewer", &["demo"]).await;

    let (_, in_staging) = asked(&srv, "How many experiments ran?", Some(&staging)).await;
    let (_, in_demo) = asked(&srv, "How many experiments ran?", Some(&demo)).await;

    assert_eq!(
        in_staging["rows"],
        json!([{"experiments": 1}]),
        "{in_staging}"
    );
    assert_eq!(in_demo["rows"], json!([{"experiments": 2}]), "{in_demo}");
}

#[tokio::test]
async fn a_scoped_question_about_the_pass_rate_is_not_answered_for_every_environment() {
    let srv = spawn_server().await;
    let demo = add_scoped_token(&srv, "demo-viewer", "viewer", &["demo"]).await;

    let (status, answer) = asked(&srv, "What is the pass rate?", Some(&demo)).await;

    // The completion log of a run names no environment, so the question
    // cannot be kept to the environments of the reader: it is refused.
    assert_eq!(status, 422, "{answer}");
    assert_eq!(answer.get("rows"), None, "{answer}");
}

#[tokio::test]
async fn the_deviation_rate_and_its_sparkline_leave_out_a_run_that_sent_no_counter() {
    let (srv, run) = one_deviated_run().await;
    // A run that sent its trace and its deviation counter, at the time the
    // run of the journal started. Only a trace brings the counter.
    let at = run.started_at_ns;
    let traced = SpanRow {
        ts_ns: at,
        trace_id: "trace-traced".into(),
        span_id: "span-traced".into(),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        service_name: "tumult".into(),
        experiment_id: Some("traced".into()),
        experiment_name: Some("traced-run".into()),
        outcome_status: Some("deviated".into()),
        target_system: Some("payments-api".into()),
        target_environment: Some("staging".into()),
        events: "[]".into(),
        ..SpanRow::default()
    };
    let deviation = MetricSumRow {
        ts_ns: at,
        metric_name: "tumult.hypothesis.deviations.total".into(),
        value: 1.0,
        experiment_name: Some("traced-run".into()),
        ..MetricSumRow::default()
    };
    exec_write(&srv, move |w| {
        w.execute("DELETE FROM metric_sums", ())
            .map_err(|e| e.to_string())?;
        w.insert_spans(&[traced]).map_err(|e| e.to_string())?;
        w.insert_metric_sums(&[deviation])
            .map_err(|e| e.to_string())
    })
    .await;

    let (status, body) = get(&srv.base, "/api/overview?range=24h").await;

    assert_eq!(status, 200, "{body}");
    let card = body["kpis"]
        .as_array()
        .unwrap()
        .iter()
        .find(|kpi| kpi["name"] == "deviation_rate")
        .unwrap_or_else(|| panic!("no deviation card: {body}"));
    // One deviation of one traced run, on the card and on its one point.
    assert_eq!(card["value"], 1.0, "{card}");
    let sparkline: Vec<&Value> = card["spark"]
        .as_array()
        .unwrap()
        .iter()
        .map(|point| &point["v"])
        .collect();
    assert_eq!(sparkline, [&json!(1.0)], "{card}");
}
