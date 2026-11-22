//! The game-day report (R3) of a run the runner made.
//!
//! The spans come from a real run: the runner drives the executor of
//! `tumult run`, every step is an `sh` command on a state file, the OTLP
//! conversion turns the spans into store rows, and the report is built from
//! the store. Nothing here writes a span or a log by hand, so the report
//! shows what a user of `tumult run` gets.
#![cfg(unix)]

use std::collections::HashMap;
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
    run_experiment_with_sampling, ActivityExecutor, RunConfig, SamplingConfig,
};
use tumult_core::types::*;
use tumult_exec::ProviderExecutor;
use tumult_test_utils::capture_spans;

/// A target that is one file. It holds `ok` until the fault writes to it.
struct Lab(tempfile::TempDir);

impl Lab {
    fn new() -> Self {
        let lab = Self(tempfile::TempDir::new().expect("lab directory"));
        std::fs::write(lab.0.path().join("state"), "ok\n").expect("state file");
        lab
    }

    fn has(&self, name: &str) -> bool {
        self.0.path().join(name).exists()
    }

    /// A step that runs `command` in `sh`, in the directory of the lab.
    fn step(&self, name: &str, activity_type: ActivityType, command: &str) -> Activity {
        let lab = self.0.path().display();
        Activity {
            name: name.into(),
            activity_type,
            provider: Provider::Process {
                path: "sh".into(),
                arguments: vec!["-c".into(), format!("cd {lab} || exit 1; {command}")],
                env: HashMap::new(),
                timeout_s: Some(10.0),
            },
            ..Default::default()
        }
    }

    /// One fault that writes `after` to the state file, a steady state that
    /// holds while the file says `ok`, one rollback and a declared blast
    /// radius.
    fn experiment(&self, title: &str, after: &str) -> Experiment {
        let mut probe = self.step("steady", ActivityType::Probe, "cat state");
        probe.tolerance = Some(Tolerance::Regex {
            pattern: "^ok$".into(),
        });
        let fault = format!("echo {after} > state");
        Experiment {
            title: title.into(),
            steady_state_hypothesis: Some(Hypothesis {
                title: "the target answers ok".into(),
                probes: vec![probe],
            }),
            blast_radius: Some("one replica".into()),
            method: vec![self.step("kill-replica", ActivityType::Action, &fault)],
            rollbacks: vec![self.step("restart-replica", ActivityType::Action, "touch restarted")],
            ..Experiment::default()
        }
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

/// Run an experiment whose target says `after` once the fault is in, and
/// build the report of that run from a store that holds only its spans.
fn report_of_a_run(title: &str, after: &str) -> (Journal, ReportDoc) {
    let lab = Lab::new();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(ProviderExecutor::new());
    let sampling = SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: 50,
        recovery_timeout: Duration::from_millis(500),
        recovery_interval: Duration::from_millis(10),
    };
    let (journal, spans) = capture_spans(|| {
        run_experiment_with_sampling(
            &lab.experiment(title, after),
            &executor,
            &Arc::new(ControlRegistry::new()),
            &RunConfig::default(),
            &sampling,
        )
        .expect("run")
    });

    assert_eq!(
        lab.has("restarted"),
        !journal.rollback_results.is_empty(),
        "the journal and the target disagree on the rollback"
    );

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

fn bullets(doc: &ReportDoc) -> Vec<&str> {
    doc.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Bullets(items) => Some(items),
            _ => None,
        })
        .flatten()
        .map(String::as_str)
        .collect()
}

fn timeline(doc: &ReportDoc) -> &[Vec<Cell>] {
    doc.blocks
        .iter()
        .find_map(|block| match block {
            Block::Table { rows, .. } => Some(rows.as_slice()),
            _ => None,
        })
        .expect("timeline table")
}

/// The status cell of the timeline row of span `name`.
fn timeline_status(doc: &ReportDoc, name: &str) -> String {
    let row = timeline(doc)
        .iter()
        .find(|row| cell_text(&row[1]) == name)
        .unwrap_or_else(|| panic!("no timeline row {name}"));
    cell_text(&row[4])
}

/// The step cells of the timeline rows of span `name`, in the order of the
/// run.
fn timeline_steps(doc: &ReportDoc, name: &str) -> Vec<String> {
    timeline(doc)
        .iter()
        .filter(|row| cell_text(&row[1]) == name)
        .map(|row| cell_text(&row[2]))
        .collect()
}

/// How many spans the timeline rows of span `name` that say `status` stand
/// for: a row of a step that ran several times in a row says how many.
fn spans_in_timeline(doc: &ReportDoc, name: &str, status: &str) -> usize {
    timeline(doc)
        .iter()
        .filter(|row| cell_text(&row[1]) == name && cell_text(&row[4]) == status)
        .map(|row| {
            cell_text(&row[2])
                .split_once(" × ")
                .map_or(1, |(_, times)| times.parse().expect("a count"))
        })
        .sum()
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
fn the_report_of_a_completed_run_names_its_steps_and_finds_nothing_wrong() {
    let (_, doc) = report_of_a_run("replica loss", "ok");

    assert_eq!(value(&doc, "Scenario"), "kill-replica");
    assert_eq!(timeline_steps(&doc, "resilience.action"), ["kill-replica"]);
    assert_eq!(bullets(&doc), [""; 0]);
    let said = paragraphs(&doc);
    assert!(
        said.contains(&"No step failed and no warnings or errors were logged during this run."),
        "{said:?}"
    );
    assert!(
        said.contains(&"No rollback action was exercised in this run."),
        "{said:?}"
    );
}

#[test]
fn the_report_of_a_deviated_run_names_the_probe_that_failed_and_the_rollback_step() {
    let (journal, doc) = report_of_a_run("replica loss, no recovery", "down");

    // The fault is the scenario. The step that undid it is not.
    assert_eq!(value(&doc, "Scenario"), "kill-replica");
    assert_eq!(
        timeline_steps(&doc, "resilience.action"),
        ["kill-replica", "restart-replica"]
    );
    // The probe failed in every sample that read the target down, and in
    // the check after the method: one finding that says how many times.
    let failed_samples = journal.probe_samples.iter().filter(|s| !s.ok).count();
    assert!(failed_samples >= 1, "{:?}", journal.probe_samples);
    let failed = failed_samples + 1;
    assert_eq!(
        bullets(&doc),
        [format!(
            "[ERROR] steady (resilience.probe): the output is outside the tolerance (× {failed})"
        )]
    );
    assert_eq!(spans_in_timeline(&doc, "resilience.probe", "error"), failed);
    let said = paragraphs(&doc);
    assert!(
        said.contains(&"The rollback ran 1 step (restart-replica); status: all clean."),
        "{said:?}"
    );
    // The probe that watched for recovery polled many times: one line.
    let probes = timeline_steps(&doc, "resilience.probe");
    assert!(
        probes.iter().any(|step| step.starts_with("steady × ")),
        "{probes:?}"
    );
    assert!(probes.len() < 8, "{probes:?}");
}

#[test]
fn the_report_of_a_deviated_run_says_deviated_and_shows_the_rollback() {
    let (journal, doc) = report_of_a_run("replica loss, no recovery", "down");
    assert_eq!(journal.status, ExperimentStatus::Deviated);

    assert_eq!(value(&doc, "Outcome"), "deviated");
    assert_eq!(value(&doc, "Hypothesis"), "Not met");
    assert_eq!(value(&doc, "Recovery time"), "not recorded");
    assert_eq!(journal.rollback_results.len(), 1);
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
