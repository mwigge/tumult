//! A run that is in the lake only as a journal.
//!
//! A plain `tumult run` and a journal import leave no trace: the lake holds
//! the journal of the run and no span. Each test here empties the lake of
//! spans and logs, stores journals, and reads the runs back from the pages
//! that list runs.

use crate::common::*;
use serde_json::{json, Value};
use tumult_core::types::{Experiment, ExperimentStatus, ExperimentTarget, Journal};

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
    let run = |id: &str, environment: Option<&str>| {
        let stored = journal(id, ExperimentStatus::Completed, 600);
        (stored, Some(definition(environment)))
    };
    let nowhere = (journal("bare", ExperimentStatus::Completed, 600), None);
    let runs = vec![
        run("in-staging", Some("staging")),
        run("in-prod", Some("prod")),
        run("no-environment", None),
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
