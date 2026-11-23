//! The reports of the recorded runs are the ones kept with the tests.
//!
//! The store holds what the `tumult` binary sent and wrote on five runs of
//! a lab (see `tumult_test_utils::recorded`) and one manual exercise that
//! went through the registry. Each report is built from that store at a
//! fixed time and compared with the file in `tests/golden`, as the HTML of
//! the preview and as the Typst markup of the PDF.
//!
//! When a report changes on purpose, read the new text and keep it:
//! `TUMULT_BLESS=1` writes the files.

use std::path::{Path, PathBuf};

use tumult_compliance::builders::{build_evidence_pack, build_executive, build_game_day};
use tumult_compliance::html::{fmt_date, render};
use tumult_compliance::markup::doc_to_typst;
use tumult_compliance::{Block, Cell, OrgTree, ReportDoc};
use tumult_core::types::{ExperimentStatus, Journal};
use tumult_lake::{ExerciseType, ManualOutcome, NewManualExperiment, Reader, Store, Writer};
use tumult_test_utils::{assert_kept, recorded};

const MINUTE_NS: i64 = 60_000_000_000;
const DAY_NS: i64 = 86_400_000_000_000;

/// The manual exercise next to the recorded runs.
const DRILL: &str = "Regional failover drill";

/// Who owns which experiment. The billing export has no owner.
const ORG: &str = "
nodes:
  - {name: commerce, kind: domain}
  - {name: platform, kind: domain}
  - {name: checkout-team, parent: commerce}
  - {name: payments-team, parent: commerce}
  - {name: search-team, parent: platform}
assignments:
  - team: checkout-team
    targets: [\"Checkout *\"]
  - team: payments-team
    targets: [\"Payments *\"]
  - team: search-team
    targets: [\"Search *\"]
";

/// The recorded runs and the manual exercise, in a store of their own.
struct Recording {
    _dir: tempfile::TempDir,
    store: Store,
    journals: Vec<Journal>,
}

impl Recording {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a directory");
        let store = Store::open(&dir.path().join("lake.duckdb")).expect("a store");
        let journals = {
            let writer = store.writer().expect("a writer");
            let journals = recorded::load(&writer);
            enter_drill(&writer, drill_at(&journals));
            journals
        };
        Self {
            _dir: dir,
            store,
            journals,
        }
    }

    fn reader(&self) -> Reader {
        self.store.read_only().expect("a reader")
    }

    /// When the reports are made.
    fn made_at(&self) -> i64 {
        recorded::day_after(&self.journals)
    }

    /// The first recorded run of the experiment `title` that ended `status`.
    fn run(&self, title: &str, status: &ExperimentStatus) -> &Journal {
        self.journals
            .iter()
            .find(|journal| journal.experiment_title == title && journal.status == *status)
            .unwrap_or_else(|| panic!("no run of {title} ended {status}"))
    }
}

/// The time of the manual exercise: a minute before the first recorded run.
fn drill_at(journals: &[Journal]) -> i64 {
    let first = journals.iter().map(|journal| journal.started_at_ns).min();
    first.expect("a recorded run") - MINUTE_NS
}

/// Enter a manual exercise the way the registry takes one: a draft, the
/// attestation of the person who entered it, the review of another person.
fn enter_drill(writer: &Writer, executed_at_ns: i64) {
    let id = writer
        .create_manual_draft(&NewManualExperiment {
            experiment_name: DRILL.into(),
            exercise_type: ExerciseType::Failover,
            executed_at_ns,
            hypothesis: "The second region takes the traffic within 15 minutes".into(),
            method: "The operators switched the traffic to the second region by hand".into(),
            outcome: ManualOutcome::Partial,
            hypothesis_met: Some(true),
            findings: Some("The runbook names a dashboard that no longer exists".into()),
            action_items: vec!["Update the runbook".into()],
            target_system: Some("checkout".into()),
            target_environment: Some("prod".into()),
            blast_radius: Some("one region".into()),
            recovery_time_s: Some(540.0),
            duration_s: Some(3600.0),
            entered_by: "alice".into(),
            attestation: "I attest that this record reflects the exercise.".into(),
            renewal_due_ns: None,
            framework_refs: vec!["DORA Art. 24(6)".into(), "ISO 27001 A.5.29".into()],
            exercise: tumult_lake::ExerciseDetail::default(),
        })
        .expect("a draft");
    writer.submit_manual(&id, None, "alice").expect("submitted");
    writer
        .verify_manual(&id, "bob", Some("Checked against the exercise notes"))
        .expect("verified");
}

/// The table of `doc` under the heading `heading`.
fn table_under<'a>(doc: &'a mut ReportDoc, heading: &str) -> (&'a [String], &'a mut [Vec<Cell>]) {
    let at = doc
        .blocks
        .iter()
        .position(|block| matches!(block, Block::H2(text) if text == heading))
        .unwrap_or_else(|| panic!("no heading {heading}"));
    match &mut doc.blocks[at + 1] {
        Block::Table { headers, rows, .. } => (headers, rows),
        other => panic!("{heading} is followed by {other:?}"),
    }
}

/// Replace the text `shown` of `cell` by `name`; the cell must show it.
fn name_cell(cell: &mut Cell, shown: &str, name: &str) {
    assert_eq!(*cell, Cell::text(shown));
    *cell = Cell::text(name);
}

/// The registry stamps a manual record with the time of the clock when it
/// is entered and when it is verified, so those two dates differ from run
/// to run. Each place that shows one must show the stored date; the kept
/// text has `{entered}` and `{verified}` there.
fn name_the_dates_of_the_drill(doc: &mut ReportDoc, reader: &Reader) {
    let stored = reader
        .query_json_rows("SELECT entered_at_ns, reviewed_at_ns FROM manual_experiments")
        .expect("the manual record");
    let date_of = |column: &str| fmt_date(stored[0][column].as_i64().expect("a time"));
    let (entered, verified) = (date_of("entered_at_ns"), date_of("reviewed_at_ns"));

    let (headers, rows) = table_under(doc, "Test register");
    let column = headers
        .iter()
        .position(|header| header == "Entered")
        .expect("the column Entered");
    let row = rows
        .iter_mut()
        .find(|row| row[0] == Cell::text(DRILL))
        .expect("the row of the drill");
    name_cell(&mut row[column], &entered, "{entered}");

    let mut named = 0;
    for block in &mut doc.blocks {
        let Block::KeyValues(pairs) = block else {
            continue;
        };
        for (key, cell) in pairs {
            match key.as_str() {
                "Entered by" => {
                    name_cell(cell, &format!("alice on {entered}"), "alice on {entered}");
                }
                "Verified by" => {
                    name_cell(cell, &format!("bob on {verified}"), "bob on {verified}");
                }
                _ => continue,
            }
            named += 1;
        }
    }
    assert_eq!(named, 2, "the attestation of the drill");
}

/// A report says which Tumult made it, and the next release is another one.
/// The document must carry the version of this build, in its head and in
/// `footnotes` of its footnotes; the kept text has `{version}` there.
fn name_the_version(doc: &mut ReportDoc, footnotes: usize) {
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(doc.meta.version, version);
    doc.meta.version = "{version}".into();
    let mut named = 0;
    for block in &mut doc.blocks {
        let Block::Footnote(text) = block else {
            continue;
        };
        let made_by = format!("Tumult v{version}");
        if text.contains(&made_by) {
            *text = text.replace(&made_by, "Tumult v{version}");
            named += 1;
        }
    }
    assert_eq!(named, footnotes, "the footnotes that name the version");
}

fn kept_at(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

/// Compare both outputs of `doc` with the kept files `name.html` and
/// `name.typ`. `footnotes` of its footnotes name the version of Tumult.
fn assert_report_kept(name: &str, doc: &mut ReportDoc, footnotes: usize) {
    name_the_version(doc, footnotes);
    assert_kept(&kept_at(&format!("{name}.html")), &render(doc));
    assert_kept(&kept_at(&format!("{name}.typ")), &doc_to_typst(doc).0);
}

#[test]
fn the_executive_digest_of_the_recorded_runs_is_the_one_kept_with_the_tests() {
    let recording = Recording::new();
    let org = OrgTree::from_yaml(ORG).expect("an org tree");
    let made_at = recording.made_at();

    let mut doc = build_executive(
        &recording.reader(),
        &org,
        made_at,
        30 * DAY_NS,
        made_at,
        &[],
    )
    .expect("the executive digest");

    assert_report_kept("executive", &mut doc, 1);
}

#[test]
fn the_executive_digest_without_an_org_structure_is_the_one_kept_with_the_tests() {
    let recording = Recording::new();
    let made_at = recording.made_at();

    let mut doc = build_executive(
        &recording.reader(),
        &OrgTree::empty(),
        made_at,
        30 * DAY_NS,
        made_at,
        &[],
    )
    .expect("the executive digest");

    assert_report_kept("executive-no-org", &mut doc, 1);
}

/// Two months on, no run is inside the 30 days the digest looks at.
#[test]
fn the_executive_digest_made_two_months_later_is_the_one_kept_with_the_tests() {
    let recording = Recording::new();
    let org = OrgTree::from_yaml(ORG).expect("an org tree");
    let made_at = recording.made_at() + 60 * DAY_NS;

    let mut doc = build_executive(
        &recording.reader(),
        &org,
        made_at,
        30 * DAY_NS,
        made_at,
        &[],
    )
    .expect("the executive digest");

    assert_report_kept("executive-two-months-later", &mut doc, 1);
}

/// The evidence pack of `framework`, for the 30 days before it is made.
fn assert_evidence_pack_kept(framework: &str) {
    let recording = Recording::new();
    let reader = recording.reader();

    let mut doc = build_evidence_pack(
        &reader,
        framework,
        Some(30 * DAY_NS),
        recording.made_at(),
        &[],
    )
    .expect("the evidence pack");

    name_the_dates_of_the_drill(&mut doc, &reader);
    assert_report_kept(&format!("evidence-pack-{framework}"), &mut doc, 0);
}

#[test]
fn the_dora_evidence_pack_is_the_one_kept_with_the_tests() {
    assert_evidence_pack_kept("dora");
}

#[test]
fn the_nis2_evidence_pack_is_the_one_kept_with_the_tests() {
    assert_evidence_pack_kept("nis2");
}

#[test]
fn the_iso27001_evidence_pack_is_the_one_kept_with_the_tests() {
    assert_evidence_pack_kept("iso27001");
}

#[test]
fn the_soc2_evidence_pack_is_the_one_kept_with_the_tests() {
    assert_evidence_pack_kept("soc2");
}

/// The run report of the first recorded run of `title` that ended `status`.
fn assert_run_report_kept(name: &str, title: &str, status: &ExperimentStatus) {
    let recording = Recording::new();
    let run = recording.run(title, status);

    let mut doc = build_game_day(
        &recording.reader(),
        &run.experiment_id,
        recording.made_at(),
        &[],
    )
    .expect("the run report")
    .expect("a report for a run that sent its trace");

    assert_report_kept(name, &mut doc, 1);
}

#[test]
fn the_report_of_a_run_that_deviated_is_the_one_kept_with_the_tests() {
    assert_run_report_kept(
        "run-deviated",
        "Payments tolerate a slow ledger",
        &ExperimentStatus::Deviated,
    );
}

#[test]
fn the_report_of_a_run_that_completed_is_the_one_kept_with_the_tests() {
    assert_run_report_kept(
        "run-completed",
        "Checkout survives a cache restart",
        &ExperimentStatus::Completed,
    );
}

#[test]
fn a_run_that_sent_no_trace_has_no_run_report() {
    let recording = Recording::new();
    let run = recording.run(
        "Billing export resumes after a restart",
        &ExperimentStatus::Completed,
    );

    let doc = build_game_day(
        &recording.reader(),
        &run.experiment_id,
        recording.made_at(),
        &[],
    )
    .expect("the query of the run");

    assert!(doc.is_none(), "{doc:?}");
}
