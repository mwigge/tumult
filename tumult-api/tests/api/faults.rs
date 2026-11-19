//! The fault pie of the home page: the fault injection steps of the runs.
//!
//! The runner marks the span of every step with a fault type: an action
//! is an "injection", a probe sample an "observation". It sets no subtype,
//! so the pie names the plugin of a step in its place. A step of the
//! rollback undoes a fault and is not one.

use crate::common::*;
use serde_json::{json, Value};
use tumult_lake::SpanRow;

/// The environment of the run of this file. The seed runs in "demo".
const LAB: &str = "lab";

/// A span of the trace of the run, the way the runner sends it.
fn span(id: &str, parent: Option<&str>, name: &str) -> SpanRow {
    SpanRow {
        ts_ns: now_ns() - 300 * NS,
        trace_id: "trace-faults".into(),
        span_id: format!("span-{id}"),
        parent_span_id: parent.map(|p| format!("span-{p}")),
        span_name: name.into(),
        span_kind: "Internal".into(),
        duration_ns: NS,
        status_code: "Ok".into(),
        service_name: "tumult".into(),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// A step of the run: its fault type follows its kind, and it names the
/// plugin that served it.
fn step(id: &str, parent: &str, kind: &str, plugin: &str) -> SpanRow {
    let (name, fault_type) = match kind {
        "action" => ("resilience.action", "injection"),
        _ => ("resilience.probe", "observation"),
    };
    SpanRow {
        fault_type: Some(fault_type.into()),
        plugin_name: Some(plugin.into()),
        ..span(id, Some(parent), name)
    }
}

/// One run in [`LAB`]: two actions of one plugin, one of another, ten
/// probe samples, and a rollback with one action.
fn run_with_faults() -> Vec<SpanRow> {
    let mut spans = vec![SpanRow {
        experiment_id: Some("exp-faults".into()),
        experiment_name: Some("network-partition".into()),
        target_system: Some("ledger".into()),
        target_environment: Some(LAB.into()),
        ..span("root", None, "resilience.experiment")
    }];
    spans.push(step("cut-1", "root", "action", "tumult-network"));
    spans.push(step("cut-2", "root", "action", "tumult-network"));
    spans.push(step("kill", "root", "action", "tumult-process"));
    for i in 0..10 {
        spans.push(step(&format!("sample-{i}"), "root", "probe", "tumult-http"));
    }
    spans.push(span("rollback", Some("root"), "resilience.rollback"));
    spans.push(step("heal", "rollback", "action", "tumult-network"));
    spans
}

async fn seed_run_with_faults(srv: &TestServer) {
    let rows = run_with_faults();
    exec_write(srv, move |w| {
        w.insert_spans(&rows).map_err(|e| e.to_string())
    })
    .await;
}

/// The slices of the pie, in a fixed order: the order of slices of one
/// size is not part of the answer.
fn slices(body: &Value) -> Vec<Value> {
    let mut rows = body["faults"]
        .as_array()
        .unwrap_or_else(|| panic!("no faults: {body}"))
        .clone();
    rows.sort_by_key(|row| {
        (
            row["fault_subtype"].to_string(),
            row["fault_type"].to_string(),
        )
    });
    rows
}

fn slice(fault_type: &str, fault_subtype: &str, count: u64) -> Value {
    json!({"fault_type": fault_type, "fault_subtype": fault_subtype, "count": count})
}

#[tokio::test]
async fn the_fault_pie_counts_the_fault_injection_steps_and_no_probe_sample() {
    let srv = spawn_server().await;
    seed_run_with_faults(&srv).await;

    let (status, body) = get(&srv.base, "/api/overview?range=24h").await;

    assert_eq!(status, 200, "{body}");
    // The seed has two actions with the subtype process-kill. The rollback
    // step and the probe samples are not there.
    assert_eq!(
        slices(&body),
        [
            slice("injection", "process-kill", 2),
            slice("injection", "tumult-network", 2),
            slice("injection", "tumult-process", 1),
        ],
        "{}",
        body["faults"]
    );
}

#[tokio::test]
async fn a_manual_import_with_a_fault_type_and_subtype_is_counted_as_before() {
    let srv = spawn_server().await;
    let at = now_ns() - 200 * NS;
    let csv = srv._tmp.path().join("faults.csv");
    std::fs::write(
        &csv,
        format!(
            "ts_ns,trace_id,span_id,fault_type,fault_subtype\n\
             {at},trace-csv,csv-1,network,latency\n\
             {at},trace-csv,csv-2,network,latency\n"
        ),
    )
    .unwrap();
    exec_write(&srv, move |w| {
        tumult_ingest::ManualImporter::new(w)
            .import_file(&csv, Some("faults".into()))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;

    let (status, body) = get(&srv.base, "/api/overview?range=24h").await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(
        slices(&body),
        [
            slice("network", "latency", 2),
            slice("injection", "process-kill", 2),
        ],
        "{}",
        body["faults"]
    );
}

#[tokio::test]
async fn the_fault_pie_of_a_scoped_reader_counts_the_steps_of_its_environments_only() {
    let srv = spawn_server().await;
    seed_run_with_faults(&srv).await;
    let lab = add_scoped_token(&srv, "lab-viewer", "viewer", &[LAB]).await;
    let demo = add_scoped_token(&srv, "demo-viewer", "viewer", &["demo"]).await;

    let (lab_status, in_lab) = get_auth(&srv.base, "/api/overview?range=24h", &lab).await;
    let (demo_status, in_demo) = get_auth(&srv.base, "/api/overview?range=24h", &demo).await;

    assert_eq!((lab_status, demo_status), (200, 200), "{in_lab}\n{in_demo}");
    assert_eq!(
        slices(&in_lab),
        [
            slice("injection", "tumult-network", 2),
            slice("injection", "tumult-process", 1),
        ],
        "{}",
        in_lab["faults"]
    );
    assert_eq!(
        slices(&in_demo),
        [slice("injection", "process-kill", 2)],
        "{}",
        in_demo["faults"]
    );
}
