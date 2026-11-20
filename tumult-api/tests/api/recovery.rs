//! The mean time to recover: one number from the root of every run.
//!
//! The card of the home page, its sparkline, the executive digest and the
//! metric `mttr` give the mean recovery time of the runs that report one.
//! A traced run reports it on its root span; a run stored only as a
//! journal reports the recovery time its journal measured.

use crate::common::*;
use serde_json::{json, Value};
use tumult_core::types::{
    Experiment, ExperimentStatus, ExperimentTarget, Journal, ObjectiveOutcome,
};
use tumult_lake::SpanRow;

const HOUR: i64 = 3_600 * NS;

/// The root span of a traced run of `ledger` in `environment` that started
/// at `ts_ns` and recovered in `recovery_s`.
fn traced_run(id: &str, environment: &str, ts_ns: i64, recovery_s: f64) -> SpanRow {
    SpanRow {
        ts_ns,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 30 * NS,
        status_code: "Ok".into(),
        service_name: "tumult".into(),
        experiment_id: Some(id.into()),
        experiment_name: Some(format!("name-{id}")),
        outcome_status: Some("completed".into()),
        target_system: Some("ledger".into()),
        target_environment: Some(environment.into()),
        recovery_time_s: Some(recovery_s),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// A run of `ledger` in `environment` stored only as a journal: it started
/// at `started_at_ns` and its journal measured a recovery time of `rto_s`,
/// or none.
fn journal_run(
    id: &str,
    environment: &str,
    started_at_ns: i64,
    rto_s: Option<f64>,
) -> (Journal, Experiment) {
    let mut journal = tumult_lake::duckdb_store::sample_journal(id, ExperimentStatus::Completed);
    journal.experiment_title = format!("name-{id}");
    journal.started_at_ns = started_at_ns;
    journal.ended_at_ns = started_at_ns + 30 * NS;
    journal.duration_ms = 30_000;
    journal.objectives = rto_s.map(|rto| ObjectiveOutcome {
        rto_target_s: Some(300.0),
        rto_measured_s: Some(rto),
        ..ObjectiveOutcome::default()
    });
    let definition = Experiment {
        title: journal.experiment_title.clone(),
        target: Some(ExperimentTarget {
            system: "ledger".into(),
            technology: None,
            environment: Some(environment.into()),
            criticality: None,
            namespace: None,
            version: None,
        }),
        ..Experiment::default()
    };
    (journal, definition)
}

/// Store the spans `spans` and the runs stored only as journals `journals`.
async fn store(srv: &TestServer, spans: Vec<SpanRow>, journals: Vec<(Journal, Experiment)>) {
    exec_write(srv, move |w| {
        w.insert_spans(&spans).map_err(|e| e.to_string())?;
        for (journal, definition) in &journals {
            w.ingest_journal(journal, Some(definition))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await;
}

/// A request to `path`, with the bearer `token` when there is one.
fn request(
    method: reqwest::Method,
    srv: &TestServer,
    path: &str,
    token: Option<&str>,
) -> reqwest::RequestBuilder {
    let request = reqwest::Client::new().request(method, format!("{}{path}", srv.base));
    match token {
        Some(token) => request.header("Authorization", format!("Bearer {token}")),
        None => request,
    }
}

/// The body of `GET path`, read with `token` when there is one.
async fn read(srv: &TestServer, path: &str, token: Option<&str>) -> Value {
    let resp = request(reqwest::Method::GET, srv, path, token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "{path}");
    resp.json().await.unwrap()
}

/// The mean time to recover card of the home page over the last day.
async fn card(srv: &TestServer, token: Option<&str>) -> Value {
    let body = read(srv, "/api/overview?range=24h", token).await;
    body["kpis"]
        .as_array()
        .unwrap()
        .iter()
        .find(|kpi| kpi["name"] == "mttr_s")
        .cloned()
        .unwrap_or_else(|| panic!("no mean time to recover card: {body}"))
}

/// The points of the sparkline of a card, as `(bucket start in seconds,
/// value)`.
fn sparkline(card: &Value) -> Vec<(i64, f64)> {
    card["spark"]
        .as_array()
        .unwrap_or_else(|| panic!("no sparkline: {card}"))
        .iter()
        .map(|point| (point["ts"].as_i64().unwrap(), point["v"].as_f64().unwrap()))
        .collect()
}

/// The text after `marker` in `html`, up to the next tag.
fn after(html: &str, marker: &str) -> Option<String> {
    let (_, rest) = html.split_once(marker)?;
    rest.split('<').next().map(str::to_string)
}

/// What an executive digest of the last week says of the mean time to
/// recover: the note under its card of issues, and the end of the sentence
/// on it in its section of issues.
async fn digest(srv: &TestServer, token: Option<&str>) -> (Option<String>, Option<String>) {
    let resp = request(
        reqwest::Method::POST,
        srv,
        "/api/reports/v2/generate",
        token,
    )
    .json(&json!({"type": "executive-digest", "period": "7d"}))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    let path = format!("/api/reports/v2/{}/html", meta["doc_id"].as_str().unwrap());
    let resp = request(reqwest::Method::GET, srv, &path, token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    (
        after(&html, "MTTR "),
        after(&html, "Mean time to recovery across runs reporting one: "),
    )
}

/// The points of the metric `mttr` per hour of the last day that have a
/// value, as `(bucket start in seconds, value)`. An hour with runs and no
/// recovery time has a point without a value.
async fn metric(srv: &TestServer, token: Option<&str>) -> Vec<(i64, f64)> {
    let path = "/api/timeseries?metric=mttr&interval=1h&range=24h";
    let body = read(srv, path, token).await;
    body["points"]
        .as_array()
        .unwrap_or_else(|| panic!("no points: {body}"))
        .iter()
        .filter_map(|point| Some((point["bucket_s"].as_i64()?, point["value"].as_f64()?)))
        .collect()
}

/// The value the report of the metric `mttr` heads with, as it shows it.
async fn metric_report(srv: &TestServer, token: Option<&str>) -> Option<String> {
    let resp = request(reqwest::Method::POST, srv, "/api/reports/generate", token)
        .json(&json!({"metric": "mttr"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let made: Value = resp.json().await.unwrap();
    let path = format!("/api/reports/{}", made["name"].as_str().unwrap());
    let resp = request(reqwest::Method::GET, srv, &path, token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    after(
        &html,
        "<div class=\"kpi-label\">mttr</div><div class=\"kpi-value\">",
    )
}

/// The start of the clock hour two hours ago.
fn two_hours_ago() -> i64 {
    (now_ns() - 2 * HOUR) / HOUR * HOUR
}

#[tokio::test]
async fn the_card_the_digest_and_the_metric_give_the_mean_of_a_traced_run_and_a_journal() {
    // The seed holds two traced runs that report no recovery time.
    let srv = spawn_server().await;
    let hour = two_hours_ago();
    let traced = traced_run("traced", "staging", hour + 60 * NS, 20.0);
    let journal = journal_run("journal", "staging", hour + 1_860 * NS, Some(40.0));
    store(&srv, vec![traced], vec![journal]).await;

    let card = card(&srv, None).await;
    let digest = digest(&srv, None).await;
    let metric = metric(&srv, None).await;
    let report = metric_report(&srv, None).await;

    // The card, the note and the sentence of the digest, the metric for
    // the hour of the runs and the report of the metric.
    assert_eq!(
        (&card["value"], digest, metric, report.as_deref()),
        (
            &json!(30.0),
            (Some("30.0s".to_string()), Some("30.0s.".to_string())),
            vec![(hour / NS, 30.0)],
            Some("30.0 s"),
        ),
        "{card}"
    );
}

#[tokio::test]
async fn a_span_that_is_no_root_does_not_move_the_mean_time_to_recover() {
    let srv = spawn_server().await;
    let hour = two_hours_ago();
    let root = traced_run("traced", "staging", hour + 60 * NS, 20.0);
    // A span inside the trace of the run that carries a recovery time.
    let inner = SpanRow {
        span_id: "span-traced-action".into(),
        parent_span_id: Some("span-traced-root".into()),
        span_name: "resilience.action".into(),
        experiment_id: None,
        experiment_name: None,
        recovery_time_s: Some(80.0),
        ..traced_run("traced", "staging", hour + 120 * NS, 20.0)
    };
    store(&srv, vec![root, inner], vec![]).await;

    let card = card(&srv, None).await;
    let digest = digest(&srv, None).await;
    let metric = metric(&srv, None).await;

    assert_eq!(
        (&card["value"], digest, metric),
        (
            &json!(20.0),
            (Some("20.0s".to_string()), Some("20.0s.".to_string())),
            vec![(hour / NS, 20.0)],
        ),
        "{card}"
    );
}

#[tokio::test]
async fn with_no_run_that_reports_a_recovery_time_there_is_no_mean_time_to_recover() {
    // The seed holds two traced runs that report no recovery time. The
    // journal of this run measured none.
    let srv = spawn_server().await;
    let journal = journal_run("journal", "staging", two_hours_ago(), None);
    store(&srv, vec![], vec![journal]).await;

    let card = card(&srv, None).await;
    let digest = digest(&srv, None).await;
    let metric = metric(&srv, None).await;

    assert!(card["value"].is_null(), "{card}");
    assert_eq!(card["spark"], json!([]), "{card}");
    assert_eq!(digest, (None, None));
    assert!(metric.is_empty(), "{metric:?}");
}

#[tokio::test]
async fn the_sparkline_has_the_mean_of_each_hour_at_the_start_of_the_hour() {
    let srv = spawn_server().await;
    let now = now_ns();
    // Two runs half an hour apart in one hour of the clock five hours ago,
    // one traced and one stored only as a journal, and a traced run five
    // minutes ago.
    let earlier = (now - 5 * HOUR) / HOUR * HOUR;
    let spans = vec![
        traced_run("earlier-traced", "staging", earlier + 60 * NS, 20.0),
        traced_run("now-traced", "staging", now - 300 * NS, 60.0),
    ];
    let journal = journal_run(
        "earlier-journal",
        "staging",
        earlier + 1_860 * NS,
        Some(40.0),
    );
    store(&srv, spans, vec![journal]).await;

    let card = card(&srv, None).await;

    let last = (now - 300 * NS) / HOUR * HOUR;
    assert_eq!(
        sparkline(&card),
        [(earlier / NS, 30.0), (last / NS, 60.0)],
        "{card}"
    );
    assert_eq!(card["value"], json!(40.0), "{card}");
}

#[tokio::test]
async fn a_scoped_reader_gets_the_mean_time_to_recover_of_its_environments() {
    let srv = spawn_server().await;
    let hour = two_hours_ago();
    let spans = vec![
        traced_run("staging-traced", "staging", hour + 60 * NS, 20.0),
        traced_run("prod-traced", "prod", hour + 120 * NS, 100.0),
    ];
    let journals = vec![
        journal_run("staging-journal", "staging", hour + 1_860 * NS, Some(40.0)),
        journal_run("prod-journal", "prod", hour + 1_920 * NS, Some(200.0)),
    ];
    store(&srv, spans, journals).await;
    let token = add_scoped_token(&srv, "staging-operator", "operator", &["staging"]).await;
    let token = Some(token.as_str());

    let card = card(&srv, token).await;
    let digest = digest(&srv, token).await;
    let metric = metric(&srv, token).await;
    let report = metric_report(&srv, token).await;

    assert_eq!(
        (
            &card["value"],
            sparkline(&card),
            digest,
            metric,
            report.as_deref()
        ),
        (
            &json!(30.0),
            vec![(hour / NS, 30.0)],
            (Some("30.0s".to_string()), Some("30.0s.".to_string())),
            vec![(hour / NS, 30.0)],
            Some("30.0 s"),
        ),
        "{card}"
    );
}

#[tokio::test]
async fn a_forecast_of_the_mean_time_to_recover_of_a_target_reads_the_runs_of_that_target() {
    let srv = spawn_server().await;
    let at = now_ns() - 2 * HOUR;
    // Three runs of one experiment, minutes apart: a traced run and a run
    // stored only as a journal on `ledger`, and a traced run on `vault`.
    let named = |span: SpanRow| SpanRow {
        experiment_name: Some("failover".into()),
        ..span
    };
    let on_ledger = named(traced_run("on-ledger", "staging", at, 20.0));
    let on_vault = SpanRow {
        target_system: Some("vault".into()),
        ..named(traced_run("on-vault", "staging", at + 300 * NS, 80.0))
    };
    let (mut journal, mut definition) =
        journal_run("journal", "staging", at + 600 * NS, Some(40.0));
    journal.experiment_title = "failover".into();
    definition.title = "failover".into();
    store(&srv, vec![on_ledger, on_vault], vec![(journal, definition)]).await;

    let body = read(
        &srv,
        "/api/forecast?metric=mttr&target=ledger&range=24h&horizon=1",
        None,
    )
    .await;

    let mut observed: Vec<f64> = body["series"]["observed"]
        .as_array()
        .unwrap_or_else(|| panic!("no series: {body}"))
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    observed.sort_by(f64::total_cmp);
    assert_eq!(observed, [20.0, 40.0]);
}
