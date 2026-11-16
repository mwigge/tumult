//! The game-day report (R3) of a run the runner made.
//!
//! The spans come from a real run: the runner writes them, the OTLP
//! conversion turns them into store rows, and the report is built from the
//! store. Nothing here writes a span or a log by hand, so the report shows
//! what a user of `tumult run` gets.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::transform::common::tonic::ResourceAttributesWithSchema;
use opentelemetry_proto::transform::trace::tonic::group_spans_by_resource_and_scope;
use opentelemetry_sdk::trace::SpanData;
use opentelemetry_sdk::Resource;
use tumult_compliance::builders::build_game_day;
use tumult_compliance::{Block, Cell, ReportDoc};
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;
use tumult_test_utils::capture_spans;

/// A target whose probes print `ok` until an action runs, and `after` from
/// then on.
struct Target {
    after: &'static str,
    acted: AtomicBool,
}

impl ActivityExecutor for Target {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let output = match activity.activity_type {
            ActivityType::Probe if self.acted.load(Ordering::SeqCst) => self.after,
            ActivityType::Probe => "ok",
            ActivityType::Action => {
                self.acted.store(true, Ordering::SeqCst);
                "done"
            }
        };
        ActivityOutcome {
            success: true,
            output: Some(output.into()),
            error: None,
            duration_ms: 1,
        }
    }
}

fn activity(name: &str, activity_type: ActivityType) -> Activity {
    Activity {
        name: name.into(),
        activity_type,
        provider: Provider::Native {
            plugin: "test".into(),
            function: "noop".into(),
            arguments: std::collections::HashMap::new(),
        },
        ..Default::default()
    }
}

/// One fault, a steady state that holds while the target prints `ok`, one
/// rollback and a declared blast radius.
fn experiment(title: &str) -> Experiment {
    let mut probe = activity("steady", ActivityType::Probe);
    probe.tolerance = Some(Tolerance::Regex {
        pattern: "ok".into(),
    });
    Experiment {
        title: title.into(),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target answers ok".into(),
            probes: vec![probe],
        }),
        blast_radius: Some("one replica".into()),
        method: vec![activity("kill-replica", ActivityType::Action)],
        rollbacks: vec![activity("restart-replica", ActivityType::Action)],
        ..Experiment::default()
    }
}

fn rows(spans: Vec<SpanData>) -> Vec<tumult_lake::SpanRow> {
    let resource = Resource::builder().with_service_name("tumult").build();
    let request = ExportTraceServiceRequest {
        resource_spans: group_spans_by_resource_and_scope(
            spans,
            &ResourceAttributesWithSchema::from(&resource),
        ),
    };
    tumult_otlp::trace_request_to_spans(&request)
}

/// Run an experiment whose probes print `after` once the fault is in, and
/// build the report of that run from a store that holds only its spans.
fn report_of_a_run(title: &str, after: &'static str) -> (Journal, ReportDoc) {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(Target {
        after,
        acted: AtomicBool::new(false),
    });
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(80),
    };
    let (journal, spans) = capture_spans(|| {
        run_experiment_with_sampling(
            &experiment(title),
            &executor,
            &Arc::new(ControlRegistry::new()),
            &RunConfig::default(),
            &sampling,
        )
        .expect("run")
    });

    let dir = tempfile::TempDir::new().expect("temp dir");
    let store = tumult_lake::Store::open(&dir.path().join("lake.duckdb")).expect("store");
    store
        .writer()
        .expect("writer")
        .insert_spans(&rows(spans))
        .expect("insert spans");
    let reader = store.read_only().expect("reader");
    let doc = build_game_day(&reader, &journal.experiment_id, journal.ended_at_ns, &[])
        .expect("build")
        .expect("the run is in the store");
    (journal, doc)
}

fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Text(text) | Cell::Status(text) => text.clone(),
    }
}

/// The value of `key` in the first key and value block that has it.
fn value(doc: &ReportDoc, key: &str) -> String {
    doc.blocks
        .iter()
        .filter_map(|block| match block {
            Block::KeyValues(pairs) => Some(pairs),
            _ => None,
        })
        .flatten()
        .find(|(name, _)| name == key)
        .map_or_else(
            || panic!("the report has no {key}"),
            |(_, cell)| cell_text(cell),
        )
}

fn paragraphs(doc: &ReportDoc) -> Vec<&str> {
    doc.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The status cell of the timeline row of span `name`.
fn timeline_status(doc: &ReportDoc, name: &str) -> String {
    let rows = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Table { rows, .. } => Some(rows),
            _ => None,
        })
        .expect("timeline table");
    let row = rows
        .iter()
        .find(|row| cell_text(&row[1]) == name)
        .unwrap_or_else(|| panic!("no timeline row {name}"));
    cell_text(&row[4])
}

#[test]
fn the_report_of_a_completed_run_shows_what_the_run_measured() {
    let (journal, doc) = report_of_a_run("replica loss", "ok");
    assert_eq!(journal.status, ExperimentStatus::Completed);

    assert_eq!(value(&doc, "Experiment"), "replica loss");
    assert_eq!(value(&doc, "Outcome"), "completed");
    assert_eq!(value(&doc, "Hypothesis"), "Met");
    assert_eq!(value(&doc, "Blast radius"), "one replica");
    let recovery = journal.recovery_time_s().expect("the run saw recovery");
    assert_eq!(value(&doc, "Recovery time"), format!("{recovery:.1}s"));
    assert_eq!(timeline_status(&doc, "resilience.experiment"), "ok");
}

#[test]
fn the_report_of_a_deviated_run_says_deviated_and_shows_the_rollback() {
    let (journal, doc) = report_of_a_run("replica loss, no recovery", "down");
    assert_eq!(journal.status, ExperimentStatus::Deviated);

    assert_eq!(value(&doc, "Outcome"), "deviated");
    assert_eq!(value(&doc, "Hypothesis"), "Not met");
    assert_eq!(value(&doc, "Recovery time"), "not recorded");
    assert_eq!(value(&doc, "Rollback exercised"), "yes");
    assert_eq!(timeline_status(&doc, "resilience.experiment"), "error");
    assert!(
        paragraphs(&doc).contains(&"Outcome: deviated. Steady-state hypothesis: not met."),
        "{:?}",
        paragraphs(&doc)
    );
}

#[test]
fn the_report_does_not_invent_a_severity_the_run_did_not_record() {
    let (_, doc) = report_of_a_run("no severity", "ok");

    assert_eq!(value(&doc, "Severity"), "not recorded");
}
