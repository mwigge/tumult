//! The evidence of one run on its page: `GET /api/experiments/{id}`.
//!
//! The store is filled the way a run fills it: a journal goes through the
//! writer of the server. What the page says is then compared with what the
//! journal says, part by part.

use std::path::PathBuf;

use serde_json::{json, Value};
use tumult_core::types::{
    ActivityResult, ExperimentStatus, Journal, LoadResult, LoadSample, ProbeSample, SamplePhase,
};
use tumult_lake::{MetricGaugeRow, MetricSumRow};

use crate::common::*;

/// The journal of a recorded run: a steady state, a method, a load with
/// its windows, two objectives and the samples of a probe.
fn recorded_run() -> Journal {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/recorded/api-saturation.journal.toon");
    tumult_core::journal::read_journal(&path).expect("recorded journal")
}

async fn store(srv: &TestServer, journal: &Journal) {
    let journal = journal.clone();
    exec_write(srv, move |w| {
        w.ingest_journal(&journal, None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
}

/// The page of the run `id`.
async fn page_of(srv: &TestServer, id: &str) -> Value {
    let (status, page) = get(&srv.base, &format!("/api/experiments/{id}")).await;
    assert_eq!(status, 200, "{page}");
    page
}

fn step(phase: &str, result: &ActivityResult) -> Value {
    json!({
        "phase": phase,
        "name": result.name,
        "activity_type": result.activity_type,
        "status": result.status,
        "started_at_ns": result.started_at_ns,
        "duration_ms": result.duration_ms,
        "output": result.output,
        "error": result.error,
    })
}

/// The steps of a run in the order it took them.
fn steps(journal: &Journal) -> Vec<Value> {
    let probes = |hypothesis: &Option<tumult_core::types::HypothesisResult>| {
        hypothesis
            .as_ref()
            .map_or(Vec::new(), |h| h.probe_results.clone())
    };
    let phases = [
        ("hypothesis_before", probes(&journal.steady_state_before)),
        ("method", journal.method_results.clone()),
        ("hypothesis_after", probes(&journal.steady_state_after)),
        ("rollback", journal.rollback_results.clone()),
    ];
    phases
        .iter()
        .flat_map(|(phase, results)| results.iter().map(move |result| step(phase, result)))
        .collect()
}

/// One window of a load. A window says what became of its requests, and
/// which timeout they were held to, only when the tool told.
fn window(sample: &LoadSample) -> Value {
    let mut window = json!({
        "ts_ns": sample.ts_ns,
        "window_s": sample.duration_s,
        "throughput_rps": sample.throughput_rps,
        "latency_p50_ms": sample.latency_p50_ms,
        "latency_p95_ms": sample.latency_p95_ms,
        "latency_p99_ms": sample.latency_p99_ms,
        "error_rate": sample.error_rate,
    });
    if let Some(counts) = sample.counts {
        window["requests_sent"] = json!(counts.sent as f64);
        window["requests_ok"] = json!(counts.ok as f64);
        window["requests_timed_out"] = json!(counts.timed_out as f64);
    }
    if let Some(timeout_ms) = sample.request_timeout_ms {
        window["request_timeout_ms"] = json!(timeout_ms);
    }
    window
}

fn load(load: &LoadResult) -> Value {
    json!({
        "tool": load.tool,
        "started_at_ns": load.started_at_ns,
        "ended_at_ns": load.ended_at_ns,
        "duration_s": load.duration_s,
        "vus": load.vus,
        "throughput_rps": load.throughput_rps,
        "latency_p50_ms": load.latency_p50_ms,
        "latency_p95_ms": load.latency_p95_ms,
        "latency_p99_ms": load.latency_p99_ms,
        "error_rate": load.error_rate,
        "total_requests": load.total_requests,
        "thresholds_met": load.thresholds_met,
        "windows": load.samples.iter().map(window).collect::<Vec<Value>>(),
    })
}

fn sample(sample: &ProbeSample) -> Value {
    json!({
        "probe": sample.probe,
        "phase": sample.phase,
        "ts_ns": sample.ts_ns,
        "duration_ms": sample.duration_ms as f64,
        "ok": sample.ok,
    })
}

/// The objectives of a run as the page lists them: a verdict without an
/// observed value says so with a `null`.
fn objectives(journal: &Journal) -> Vec<Value> {
    let verdicts = journal.objectives.iter().flat_map(|o| &o.verdicts);
    verdicts
        .map(|verdict| {
            json!({
                "nfr_id": verdict.nfr_id,
                "kind": verdict.kind,
                "metric": verdict.metric,
                "operator": verdict.operator,
                "threshold": verdict.threshold,
                "observed": verdict.observed,
                "state": verdict.state,
                "margin": verdict.margin,
                "criticality": verdict.criticality,
            })
        })
        .collect()
}

/// What the page of a run has to say about it, read from its journal.
fn evidence(journal: &Journal) -> Value {
    let outcome = journal.objectives.as_ref();
    let mut samples: Vec<&ProbeSample> = journal.probe_samples.iter().collect();
    samples.sort_by_key(|sample| sample.ts_ns);
    json!({
        "kind": journal.kind,
        "started_at_ns": journal.started_at_ns,
        "ended_at_ns": journal.ended_at_ns,
        "hypothesis": {
            "before_met": journal.steady_state_before.as_ref().map(|h| h.met),
            "after_met": journal.steady_state_after.as_ref().map(|h| h.met),
        },
        "recovery": {
            "rto_target_s": outcome.and_then(|o| o.rto_target_s),
            "rto_measured_s": outcome.and_then(|o| o.rto_measured_s),
            "rpo_target_s": outcome.and_then(|o| o.rpo_target_s),
            "rpo_measured_s": outcome.and_then(|o| o.rpo_measured_s),
        },
        "steps": steps(journal),
        "objectives": objectives(journal),
        "load": journal.load_result.as_ref().map(load),
        "probe_samples": samples.into_iter().map(sample).collect::<Vec<Value>>(),
        "probe_samples_total": journal.probe_samples.len(),
    })
}

/// Compare the evidence of a page with the journal of its run, one part
/// after the other, so that a difference names its part.
fn assert_evidence(page: &Value, journal: &Journal) {
    let expected = evidence(journal);
    let parts = expected.as_object().expect("an object");
    for (part, value) in parts {
        assert_eq!(&page["evidence"][part], value, "{part}");
    }
    assert_eq!(page["evidence"], expected);
}

#[tokio::test]
async fn the_page_of_a_run_shows_what_its_journal_says() {
    let journal = recorded_run();
    // The recorded run has every part of the evidence, more than once
    // where the order of the rows matters.
    let recorded_load = journal.load_result.as_ref().expect("a load");
    assert!(recorded_load.samples.len() > 1, "one window or none");
    assert!(steps(&journal).len() > 3, "too few steps");
    assert!(objectives(&journal).len() > 1, "one objective or none");
    assert!(journal.probe_samples.len() > 100, "too few samples");
    let srv = spawn_server().await;
    store(&srv, &journal).await;

    let page = page_of(&srv, &journal.experiment_id).await;

    assert_eq!(page["experiment"]["name"], journal.experiment_title);
    assert_evidence(&page, &journal);
}

#[tokio::test]
async fn a_run_without_a_steady_state_a_load_or_an_objective_says_so() {
    let journal = tumult_lake::duckdb_store::sample_journal("plain", ExperimentStatus::Completed);
    let srv = spawn_server().await;
    store(&srv, &journal).await;

    let page = page_of(&srv, "plain").await;

    assert_evidence(&page, &journal);
    let evidence = &page["evidence"];
    assert_eq!(evidence["kind"], "chaos");
    assert_eq!(
        evidence["hypothesis"],
        json!({"before_met": null, "after_met": null})
    );
    assert_eq!(evidence["load"], Value::Null);
    assert_eq!(evidence["objectives"], json!([]));
    assert_eq!(evidence["probe_samples"], json!([]));
    assert_eq!(evidence["steps"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn a_run_that_left_no_journal_has_no_evidence() {
    let srv = spawn_server().await;

    let page = page_of(&srv, "exp-pass").await;

    assert_eq!(page.get("evidence"), Some(&Value::Null), "{page}");
}

/// A sample of `probe` taken `at_s` seconds into the run of `journal`.
fn sample_at(journal: &Journal, probe: &str, phase: SamplePhase, at_s: i64) -> ProbeSample {
    ProbeSample {
        probe: probe.into(),
        phase,
        ts_ns: journal.started_at_ns + at_s * NS,
        duration_ms: 7,
        ok: at_s % 2 == 0,
        reading: None,
    }
}

#[tokio::test]
async fn the_evidence_of_a_run_is_not_the_evidence_of_another() {
    let mut first = tumult_lake::duckdb_store::sample_journal("first", ExperimentStatus::Completed);
    first.probe_samples = vec![sample_at(&first, "health", SamplePhase::During, 1)];
    let mut second = recorded_run();
    second.experiment_id = "second".into();
    // A second load, whose windows are at other times with other values.
    let mut third = recorded_run();
    third.experiment_id = "third".into();
    for window in &mut third.load_result.as_mut().expect("a load").samples {
        window.ts_ns += NS;
        window.throughput_rps += 1.0;
    }
    let srv = spawn_server().await;
    store(&srv, &first).await;
    store(&srv, &second).await;
    store(&srv, &third).await;

    assert_evidence(&page_of(&srv, "first").await, &first);
    assert_evidence(&page_of(&srv, "second").await, &second);
    assert_evidence(&page_of(&srv, "third").await, &third);
}

#[tokio::test]
async fn a_run_that_lost_its_steady_state_says_which_check_it_failed() {
    let mut journal = recorded_run();
    journal.status = ExperimentStatus::Deviated;
    journal
        .steady_state_after
        .as_mut()
        .expect("a steady state after")
        .met = false;
    let srv = spawn_server().await;
    store(&srv, &journal).await;

    let page = page_of(&srv, &journal.experiment_id).await;

    assert_evidence(&page, &journal);
    assert_eq!(
        page["evidence"]["hypothesis"],
        json!({"before_met": true, "after_met": false})
    );
}

#[tokio::test]
async fn the_recovery_of_a_run_is_each_measure_next_to_its_own_target() {
    let mut journal = recorded_run();
    let outcome = journal.objectives.as_mut().expect("objectives");
    outcome.rto_target_s = Some(30.0);
    outcome.rto_measured_s = Some(12.5);
    outcome.rpo_target_s = Some(60.0);
    outcome.rpo_measured_s = Some(4.25);
    let srv = spawn_server().await;
    store(&srv, &journal).await;

    let page = page_of(&srv, &journal.experiment_id).await;

    assert_evidence(&page, &journal);
    assert_eq!(
        page["evidence"]["recovery"],
        json!({
            "rto_target_s": 30.0,
            "rto_measured_s": 12.5,
            "rpo_target_s": 60.0,
            "rpo_measured_s": 4.25,
        })
    );
}

#[tokio::test]
async fn the_samples_of_a_run_are_in_the_order_of_their_time() {
    let mut journal =
        tumult_lake::duckdb_store::sample_journal("order", ExperimentStatus::Completed);
    // Stored latest first, and a probe whose name starts like a load
    // series stays a probe.
    journal.probe_samples = vec![
        sample_at(&journal, "health", SamplePhase::Post, 9),
        sample_at(&journal, "load.balancer", SamplePhase::During, 4),
        sample_at(&journal, "health", SamplePhase::During, 2),
    ];
    let srv = spawn_server().await;
    store(&srv, &journal).await;

    let page = page_of(&srv, "order").await;

    assert_evidence(&page, &journal);
    let seconds: Vec<i64> = page["evidence"]["probe_samples"]
        .as_array()
        .expect("samples")
        .iter()
        .map(|sample| (sample["ts_ns"].as_i64().expect("ts_ns") - journal.started_at_ns) / NS)
        .collect();
    assert_eq!(seconds, [2, 4, 9]);
}

/// The most samples a page carries.
const MOST_SAMPLES: usize = 5000;

#[tokio::test]
async fn a_run_with_more_samples_than_a_page_carries_says_how_many_it_has() {
    let mut journal =
        tumult_lake::duckdb_store::sample_journal("long", ExperimentStatus::Completed);
    let count = i64::try_from(MOST_SAMPLES).expect("fits") + 3;
    // Stored latest first: the page has to carry the earliest ones.
    journal.probe_samples = (0..count)
        .rev()
        .map(|at_s| sample_at(&journal, "health", SamplePhase::During, at_s))
        .collect();
    let srv = spawn_server().await;
    store(&srv, &journal).await;

    let page = page_of(&srv, "long").await;

    let samples = page["evidence"]["probe_samples"]
        .as_array()
        .expect("samples");
    assert_eq!(samples.len(), MOST_SAMPLES);
    assert_eq!(page["evidence"]["probe_samples_total"], json!(count));
    let last_s =
        (samples[MOST_SAMPLES - 1]["ts_ns"].as_i64().expect("ts_ns") - journal.started_at_ns) / NS;
    assert_eq!(samples[0]["ts_ns"], json!(journal.started_at_ns));
    assert_eq!(last_s, count - 4);
}

/// The time the metrics of a run may take to arrive after its end: one
/// export interval of the metrics reader.
const EXPORT_GRACE_NS: i64 = 60 * NS;

fn sum(name: &str, ts_ns: i64) -> MetricSumRow {
    MetricSumRow {
        ts_ns,
        metric_name: name.into(),
        value: 1.0,
        experiment_name: Some("pg-failover".into()),
        ..MetricSumRow::default()
    }
}

fn gauge(name: &str, ts_ns: i64) -> MetricGaugeRow {
    MetricGaugeRow {
        ts_ns,
        metric_name: name.into(),
        value: 1.0,
        experiment_name: Some("pg-failover".into()),
        ..MetricGaugeRow::default()
    }
}

/// The names of the metric points of a page, in its order.
fn metric_names(page: &Value) -> Vec<&str> {
    let points = page["metrics"].as_array().expect("metrics");
    points
        .iter()
        .map(|point| point["metric_name"].as_str().expect("metric_name"))
        .collect()
}

#[tokio::test]
async fn the_metric_points_of_a_run_are_the_ones_of_its_time() {
    let srv = spawn_server().await;
    let before = page_of(&srv, "exp-pass").await;
    let started = before["experiment"]["started_ns"].as_i64().expect("start");
    let ended = started
        + before["experiment"]["duration_ns"]
            .as_i64()
            .expect("duration");
    // The seed has three points with the name of this run: one of its own
    // time, and two of a run of the same name twenty minutes later.
    assert_eq!(before["experiment"]["name"], "pg-failover");
    exec_write(&srv, move |w| {
        w.insert_metric_sums(&[
            sum("sum.just-before-the-start", started - 1),
            sum("sum.at-the-start", started),
            sum("sum.at-the-end-of-the-grace", ended + EXPORT_GRACE_NS),
            sum("sum.just-after-the-grace", ended + EXPORT_GRACE_NS + 1),
        ])
        .map_err(|e| e.to_string())?;
        w.insert_metric_gauges(&[
            gauge("gauge.just-before-the-start", started - 1),
            gauge("gauge.during", started + 1),
            gauge("gauge.just-after-the-grace", ended + EXPORT_GRACE_NS + 1),
        ])
        .map_err(|e| e.to_string())
    })
    .await;

    let page = page_of(&srv, "exp-pass").await;

    assert_eq!(
        metric_names(&page),
        [
            "sum.at-the-start",
            "gauge.during",
            "tumult.experiments.total",
            "sum.at-the-end-of-the-grace",
        ]
    );
}
