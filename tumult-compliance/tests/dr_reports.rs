//! The DR reports of a lake: the DR test report of one target over a
//! period (R4), the DR exercise report of one manual record (R5), and the
//! DORA matrix and DR detail of the evidence pack (R2).
//!
//! Each test seeds one lake. `ledger` in `prod` has four DR runs stored as
//! their journals, each with an RTO of 10 s: one met it, one breached it,
//! one did not recover and one saw no outage. It also has a failover
//! drill entered by hand, with a file attached and two sign-offs, and a
//! function of the register that the target binds to. Then each test
//! builds a report from that lake as the API and the CLI build it.

use sha2::{Digest, Sha256};
use tumult_compliance::builders::{
    build_dr_exercise, build_dr_test, build_evidence_pack, DrTestRequest, House,
};
use tumult_compliance::dora::coverage::Matrix;
use tumult_compliance::dora::loader::{self, Request};
use tumult_compliance::dora::params::HouseParams;
use tumult_compliance::dora::report::{self, DoraBy};
use tumult_compliance::html::{fmt_date, render};
use tumult_compliance::typst_pdf::render_pdf;
use tumult_compliance::{Block, Cell, ReportDoc, TemplateKind};
use tumult_core::compliance::{EVIDENCE_DISCLAIMER, REQUIREMENTS};
use tumult_core::types::{
    Experiment, ExperimentStatus, ExperimentTarget, Journal, NfrCriticality, NfrKind, NfrOperator,
    NfrState, NfrVerdict, ObjectiveOutcome, PostResult, TestKind,
};
use tumult_lake::{
    attachments_dir, ActionItem, ExerciseDetail, ExerciseStep, ExerciseType, FileUpload,
    IctFunction, IctFunctionService, IctLegalEntity, IctRegister, ManualOutcome, NewExercisePlan,
    NewManualExperiment, Participant, PlannedTest, Severity, SignOff, SignOffDecision, SpanRow,
    StepResult, Store, Tester, TesterKind, Writer,
};

const S: i64 = 1_000_000_000;
const DAY: i64 = 86_400 * S;
/// The time the reports are built at.
const AS_OF: i64 = 1_800_000_000 * S;
/// The start of the period of the DR test report: 365 days before
/// [`AS_OF`].
const FROM: i64 = AS_OF - 365 * DAY;
/// The trace the run that met its RTO sent.
const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
/// The bytes of the file attached to the drill.
const RESTORE_LOG: &[u8] = b"standby promoted at 10:04\n";

/// What a DR run with an RTO of 10 s saw.
#[derive(Clone, Copy)]
enum Seen {
    /// Out, and back 4 s later.
    Met,
    /// Out, and back 14 s later.
    Late,
    /// Out, and never back while the post phase watched.
    NeverBack,
    /// No probe saw an outage.
    NoOutage,
}

/// The post phase of a run that saw `seen`, its method ending at `end_ns`.
fn post_phase(seen: Seen, end_ns: i64) -> PostResult {
    let (outage, back): (Option<i64>, Option<i64>) = match seen {
        Seen::Met => (Some(1), Some(3)),
        Seen::Late => (Some(1), Some(13)),
        Seen::NeverBack => (Some(1), None),
        Seen::NoOutage => (None, None),
    };
    let outage_started_at_ns = outage.map(|s| end_ns - s * S);
    let recovered_at_ns = back.map(|s| end_ns + s * S);
    #[allow(clippy::cast_precision_loss)]
    let mttr_s = outage_started_at_ns
        .zip(recovered_at_ns)
        .map(|(from, to)| (to - from) as f64 / 1e9);
    PostResult {
        started_at_ns: end_ns,
        ended_at_ns: end_ns + 30 * S,
        duration_s: 30.0,
        samples: 30,
        probes: vec![],
        recovery_time_s: 29.1,
        full_recovery: back.is_some() || outage.is_none(),
        mttr_s,
        impact_observed: Some(outage.is_some()),
        outage_started_at_ns,
        recovered_at_ns,
    }
}

/// The verdict of a run on its RTO of 10 s.
fn rto_verdict(seen: Seen, post: &PostResult) -> NfrVerdict {
    let (observed, state, recovered, reason) = match seen {
        Seen::Met => (post.mttr_s, NfrState::Met, Some(true), None),
        Seen::Late => (post.mttr_s, NfrState::Breached, Some(true), None),
        Seen::NeverBack => (Some(30.1), NfrState::Breached, Some(false), None),
        Seen::NoOutage => (
            None,
            NfrState::Inconclusive,
            None,
            Some(NfrVerdict::NO_OUTAGE_OBSERVED.to_string()),
        ),
    };
    NfrVerdict {
        nfr_id: "rto".into(),
        kind: NfrKind::Rto,
        metric: "recovery.mttr_s".into(),
        operator: NfrOperator::Lte,
        threshold: 10.0,
        observed,
        state,
        margin: None,
        criticality: NfrCriticality::Blocking,
        recovered,
        reason,
    }
}

/// The DR run `id`, titled `title`, against `target` in `prod`, that
/// started `days_ago` days before [`AS_OF`] and saw `seen`.
fn dr_run(id: &str, title: &str, target: &str, days_ago: i64, seen: Seen) -> (Journal, Experiment) {
    let status = match seen {
        Seen::Met | Seen::NoOutage => ExperimentStatus::Completed,
        Seen::Late | Seen::NeverBack => ExperimentStatus::Deviated,
    };
    let mut journal = tumult_lake::duckdb_store::sample_journal(id, status);
    journal.kind = TestKind::Dr;
    journal.experiment_title = title.into();
    journal.started_at_ns = AS_OF - days_ago * DAY;
    journal.ended_at_ns = journal.started_at_ns + 60 * S;
    let post = post_phase(seen, journal.started_at_ns + 20 * S);
    let rpo = matches!(seen, Seen::Met);
    journal.objectives = Some(ObjectiveOutcome {
        rto_target_s: Some(10.0),
        rto_measured_s: post.mttr_s,
        rpo_target_s: rpo.then_some(5.0),
        rpo_measured_s: rpo.then_some(2.0),
        verdicts: vec![rto_verdict(seen, &post)],
    });
    journal.post_result = Some(post);
    let definition = Experiment {
        title: title.into(),
        target: Some(ExperimentTarget {
            system: target.into(),
            technology: Some("postgresql".into()),
            environment: Some("prod".into()),
            criticality: None,
            namespace: None,
            version: None,
        }),
        ..Experiment::default()
    };
    (journal, definition)
}

/// The DR runs of the lake. The first four are of `ledger` in the period;
/// one more of `ledger` is older than the period, and one is of `cache`.
fn runs() -> Vec<(Journal, Experiment)> {
    vec![
        dr_run("run-met", "ledger promote standby", "ledger", 40, Seen::Met),
        dr_run(
            "run-late",
            "ledger restore backup",
            "ledger",
            30,
            Seen::Late,
        ),
        dr_run(
            "run-never",
            "ledger zone loss",
            "ledger",
            20,
            Seen::NeverBack,
        ),
        dr_run(
            "run-quiet",
            "ledger replica kill",
            "ledger",
            10,
            Seen::NoOutage,
        ),
        dr_run(
            "run-old",
            "ledger promote standby",
            "ledger",
            400,
            Seen::Met,
        ),
        dr_run("run-cache", "cache failover", "cache", 5, Seen::Met),
    ]
}

/// The root span of the run that met its RTO: it sent its trace.
fn root_span() -> SpanRow {
    SpanRow {
        ts_ns: AS_OF - 40 * DAY,
        trace_id: TRACE.into(),
        span_id: "span-met".into(),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 60 * S,
        service_name: "tumult".into(),
        experiment_id: Some("run-met".into()),
        experiment_name: Some("ledger promote standby".into()),
        outcome_status: Some("completed".into()),
        target_system: Some("ledger".into()),
        target_environment: Some("prod".into()),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// One critical function, `cif-0001`, that `ledger` binds to.
fn register() -> IctRegister {
    IctRegister {
        legal_entities: vec![IctLegalEntity {
            lei: "529900T8BM49AURSDO55".into(),
            name: "Demo Bank".into(),
            profile: "full".into(),
            ..IctLegalEntity::default()
        }],
        functions: vec![IctFunction {
            function_id: "cif-0001".into(),
            lei: "529900T8BM49AURSDO55".into(),
            name: "Ledger".into(),
            licensed_activity: "payment services".into(),
            tier: "critical".into(),
            rto_s: Some(900.0),
            rpo_s: Some(60.0),
            active: true,
            ..IctFunction::default()
        }],
        services: vec![IctFunctionService {
            function_id: "cif-0001".into(),
            kind: "service".into(),
            pattern: "ledger".into(),
        }],
    }
}

/// The house of the lake: a failover needs two approvals.
fn house_params() -> HouseParams {
    let mut params = HouseParams::default();
    params.manual_required_signoffs.insert("failover".into(), 2);
    params
}

/// The failover drill of `ledger`, 15 days before [`AS_OF`], fulfilling
/// the plan `plan_id`. The RPO was planned and not measured. An external
/// firm ran it; it cites DORA in the older form of a citation.
fn drill(plan_id: &str) -> NewManualExperiment {
    let start = AS_OF - 15 * DAY;
    NewManualExperiment {
        experiment_name: "ledger failover drill".into(),
        exercise_type: ExerciseType::Failover,
        executed_at_ns: start,
        hypothesis: "The standby takes the writes within 15 minutes".into(),
        method: "Stop the primary and promote the standby by hand".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: Some("The promote script waited on a stale lock for 4 minutes.".into()),
        action_items: vec![],
        target_system: Some("ledger".into()),
        target_environment: Some("prod".into()),
        blast_radius: None,
        recovery_time_s: Some(600.0),
        duration_s: Some(1800.0),
        entered_by: "alice".into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec!["DORA Art. 24(6)".into(), "ISO 27001 A.5.30".into()],
        exercise: ExerciseDetail {
            rto_target_s: Some(900.0),
            rpo_target_s: Some(60.0),
            rpo_measured_s: None,
            data_verified: Some(true),
            participants: vec![
                Participant {
                    name: "alice".into(),
                    role: Some("DR owner".into()),
                },
                Participant {
                    name: "dave".into(),
                    role: Some("database admin".into()),
                },
            ],
            steps: vec![
                ExerciseStep {
                    title: "Stop the primary".into(),
                    actor: Some("dave".into()),
                    started_at_ns: Some(start),
                    ended_at_ns: Some(start + 60 * S),
                    result: Some(StepResult::Done),
                    note: None,
                },
                ExerciseStep {
                    title: "Promote the standby".into(),
                    actor: Some("dave".into()),
                    started_at_ns: Some(start + 60 * S),
                    ended_at_ns: Some(start + 600 * S),
                    result: Some(StepResult::Done),
                    note: Some("waited on a stale lock".into()),
                },
            ],
            actions: vec![ActionItem {
                title: "Clear the stale lock before a promote".into(),
                owner: "dave".into(),
                due_at_ns: Some(AS_OF + 30 * DAY),
                ticket: Some("OPS-1234".into()),
                severity: Some(Severity::High),
                retest: None,
            }],
            links: vec![],
            plan_id: Some(plan_id.into()),
            findings_severity: Some(Severity::Medium),
            tester: Some(Tester {
                name: "Ridge Recovery Partners".into(),
                kind: TesterKind::External,
            }),
            tlpt: None,
        },
    }
}

/// Enter the drill with its plan, attach the log, submit it and have bob
/// and carol sign it off. Returns the id of the record and of the plan.
fn enter_drill(writer: &Writer, store: &Store, params: &HouseParams) -> (String, String) {
    let plan = writer
        .create_manual_plan(
            &NewExercisePlan {
                name: "Yearly ledger failover".into(),
                exercise_type: ExerciseType::Failover,
                target_system: Some("ledger".into()),
                target_environment: Some("prod".into()),
                function_id: None,
                scope: Some("Promote the standby of the ledger".into()),
                owner: "alice".into(),
                planned_at_ns: Some(AS_OF - 20 * DAY),
                due_at_ns: AS_OF - 10 * DAY,
                interval_days: None,
                rto_target_s: Some(900.0),
                rpo_target_s: Some(60.0),
            },
            "alice",
        )
        .unwrap();
    let id = writer.create_manual_draft(&drill(&plan)).unwrap();
    let mut upload = FileUpload::begin(&attachments_dir(store.path())).unwrap();
    upload.write(RESTORE_LOG).unwrap();
    let staged = upload.finish("restore.log").unwrap();
    writer.add_manual_file(&id, staged, "alice").unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    for by in ["bob", "carol"] {
        let sign_off = SignOff {
            by,
            note: Some("logs checked"),
            decision: SignOffDecision::Approved,
        };
        writer
            .sign_off_manual(&id, &sign_off, |t| params.required_signoffs(t))
            .unwrap();
    }
    (id, plan)
}

/// A game day of `ledger`, verified, and a failover of `ledger` that is
/// still a draft: neither is a DR test. Returns their ids.
fn enter_others(writer: &Writer) -> (String, String) {
    let game_day = writer
        .create_manual_draft(&NewManualExperiment {
            experiment_name: "ledger game day".into(),
            exercise_type: ExerciseType::GameDay,
            exercise: ExerciseDetail::default(),
            ..drill("unused")
        })
        .unwrap();
    writer.submit_manual(&game_day, None, "alice").unwrap();
    writer.verify_manual(&game_day, "bob", None).unwrap();
    let draft = writer
        .create_manual_draft(&NewManualExperiment {
            experiment_name: "ledger draft failover".into(),
            exercise: ExerciseDetail::default(),
            ..drill("unused")
        })
        .unwrap();
    (game_day, draft)
}

/// The seeded lake.
struct Lake {
    _dir: tempfile::TempDir,
    store: Store,
    /// The id of the failover drill.
    drill: String,
    /// The id of the plan the drill fulfils.
    plan: String,
    /// The id of the verified game day.
    game_day: String,
    /// The id of the failover that is still a draft.
    draft: String,
    params: HouseParams,
}

impl Lake {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("lake.duckdb")).unwrap();
        let params = house_params();
        let (drill, plan, game_day, draft) = {
            let writer = store.writer().unwrap();
            writer
                .sync_ict_register(&register(), AS_OF - 400 * DAY)
                .unwrap();
            for (journal, definition) in runs() {
                assert!(writer.ingest_journal(&journal, Some(&definition)).unwrap());
            }
            writer.insert_spans(&[root_span()]).unwrap();
            let (drill, plan) = enter_drill(&writer, &store, &params);
            let (game_day, draft) = enter_others(&writer);
            (drill, plan, game_day, draft)
        };
        Self {
            _dir: dir,
            store,
            drill,
            plan,
            game_day,
            draft,
            params,
        }
    }

    fn house(&self) -> House<'_> {
        House::of(&self.params)
    }

    /// The DR test report of `ledger` over the 365 days up to [`AS_OF`].
    fn dr_test_report(&self, envs: &[String]) -> ReportDoc {
        let reader = self.store.read_only().unwrap();
        let request = DrTestRequest {
            target: "ledger",
            from_ns: FROM,
            to_ns: AS_OF,
        };
        build_dr_test(&reader, &self.house(), &request, AS_OF, envs).unwrap()
    }

    /// The DR exercise report of record `id`, or none.
    fn exercise_report(&self, id: &str, envs: &[String]) -> Option<ReportDoc> {
        let reader = self.store.read_only().unwrap();
        build_dr_exercise(&reader, &self.house(), id, AS_OF, envs).unwrap()
    }

    /// The DORA evidence pack, of every run on record.
    fn dora_pack(&self) -> ReportDoc {
        let reader = self.store.read_only().unwrap();
        build_evidence_pack(&reader, "dora", None, AS_OF, &[], &self.house()).unwrap()
    }
}

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
}

/// The text of a cell.
fn text(cell: &Cell) -> &str {
    match cell {
        Cell::Text(t) | Cell::Status(t) => t,
    }
}

/// The blocks under the heading `title`, up to the next heading of the
/// same level or above.
fn section<'a>(doc: &'a ReportDoc, title: &str) -> &'a [Block] {
    let level = |b: &Block| match b {
        Block::H1(_) => Some(1),
        Block::H2(_) => Some(2),
        Block::H3(_) => Some(3),
        _ => None,
    };
    let start = doc
        .blocks
        .iter()
        .position(|b| matches!(b, Block::H1(t) | Block::H2(t) if t == title))
        .unwrap_or_else(|| panic!("no heading {title:?} in {:#?}", doc.blocks));
    let own = level(&doc.blocks[start]).unwrap_or(1);
    let rest = &doc.blocks[start + 1..];
    let end = rest
        .iter()
        .position(|b| level(b).is_some_and(|l| l <= own))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// The blocks under the heading `title`, up to the next heading of any
/// level.
fn lead<'a>(doc: &'a ReportDoc, title: &str) -> &'a [Block] {
    let rest = section(doc, title);
    let end = rest
        .iter()
        .position(|b| matches!(b, Block::H1(_) | Block::H2(_) | Block::H3(_)))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// The headers and the rows, as text, of the first table under `title`.
fn table(doc: &ReportDoc, title: &str) -> (Vec<String>, Vec<Vec<String>>) {
    section(doc, title)
        .iter()
        .find_map(|b| match b {
            Block::Table { headers, rows, .. } => Some((
                headers.clone(),
                rows.iter()
                    .map(|row| row.iter().map(|c| text(c).to_string()).collect())
                    .collect(),
            )),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no table under {title:?}: {:#?}", section(doc, title)))
}

/// The row of `rows` whose cell `column` is `key`.
fn row_of<'a>(rows: &'a [Vec<String>], column: usize, key: &str) -> &'a [String] {
    rows.iter()
        .find(|row| row[column] == key)
        .unwrap_or_else(|| panic!("no row {key:?} in {rows:#?}"))
}

/// Every key and value, as text, of the key-value blocks of `blocks`.
fn pairs(blocks: &[Block]) -> Vec<(String, String)> {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::KeyValues(kvs) => Some(kvs),
            _ => None,
        })
        .flatten()
        .map(|(k, v)| (k.clone(), text(v).to_string()))
        .collect()
}

/// The key-value pairs under the third-level heading that starts with
/// `title`, up to the next heading.
fn pairs_under_h3(doc: &ReportDoc, title: &str) -> Vec<(String, String)> {
    let start = doc
        .blocks
        .iter()
        .position(|b| matches!(b, Block::H3(t) if t.starts_with(title)))
        .unwrap_or_else(|| panic!("no H3 {title:?} in {:#?}", doc.blocks));
    let blocks: Vec<Block> = doc.blocks[start + 1..]
        .iter()
        .take_while(|b| !matches!(b, Block::H1(_) | Block::H2(_) | Block::H3(_)))
        .cloned()
        .collect();
    pairs(&blocks)
}

/// The values of `key` among `pairs`.
fn values<'a>(pairs: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    pairs
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .collect()
}

/// The one value of `key` among `pairs`.
fn value<'a>(pairs: &'a [(String, String)], key: &str) -> &'a str {
    match values(pairs, key).as_slice() {
        [one] => one,
        other => panic!("{key:?} is {other:?} in {pairs:#?}"),
    }
}

/// The sign-off block of a report.
fn signoff(doc: &ReportDoc) -> Vec<(String, String)> {
    doc.blocks
        .iter()
        .find_map(|b| match b {
            Block::Signoff(entries) => Some(entries.clone()),
            _ => None,
        })
        .expect("a sign-off block")
}

/// The footnotes of a report.
fn footnotes(doc: &ReportDoc) -> Vec<&str> {
    doc.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Footnote(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_dr_test_report_puts_each_dr_test_of_the_target_against_its_rto_and_rpo() {
    let lake = Lake::new();

    let doc = lake.dr_test_report(&[]);

    assert_eq!(doc.meta.template, TemplateKind::DrTest);
    assert!(
        doc.meta.doc_id.starts_with("KRK-R4-"),
        "{}",
        doc.meta.doc_id
    );
    assert_eq!(doc.meta.title, "DR test report: ledger");
    assert_eq!(doc.meta.period, Some((FROM, AS_OF)));
    assert_eq!(doc.meta.generated_at_ns, AS_OF);
    let (headers, rows) = table(&doc, "DR tests");
    assert_eq!(
        headers,
        [
            "Date",
            "Test",
            "RTO planned",
            "RTO actual",
            "RPO planned",
            "RPO actual",
            "Verdict"
        ]
    );
    // The tests of the period, oldest first: not the older run, and not
    // the run of the other target. Each says where it comes from.
    let tests: Vec<&str> = rows.iter().map(|row| row[1].as_str()).collect();
    assert_eq!(
        tests,
        [
            "ledger promote standby (automated)",
            "ledger restore backup (automated)",
            "ledger zone loss (automated)",
            "ledger failover drill (manual failover)",
            "ledger replica kill (automated)",
        ]
    );
    assert_eq!(rows[0][0], fmt_date(AS_OF - 40 * DAY));
    let planned_and_actual = |test: &str| -> Vec<String> { row_of(&rows, 1, test)[2..].to_vec() };
    assert_eq!(
        planned_and_actual("ledger promote standby (automated)"),
        ["10.0 s", "4.0 s", "5.0 s", "2.0 s", "met"]
    );
    assert_eq!(
        planned_and_actual("ledger restore backup (automated)"),
        ["10.0 s", "14.0 s", "not set", "not measured", "breached"]
    );
    assert_eq!(
        planned_and_actual("ledger zone loss (automated)"),
        [
            "10.0 s",
            "not recovered",
            "not set",
            "not measured",
            "not recovered"
        ]
    );
    assert_eq!(
        planned_and_actual("ledger replica kill (automated)"),
        [
            "10.0 s",
            "no outage observed",
            "not set",
            "not measured",
            "no outage observed"
        ]
    );
    // The RPO of the drill was planned and not measured: it is not met.
    assert_eq!(
        planned_and_actual("ledger failover drill (manual failover)"),
        [
            "900.0 s",
            "600.0 s",
            "60.0 s",
            "not measured",
            "not measured"
        ]
    );
}

#[test]
fn the_dr_test_report_counts_the_tests_and_says_when_the_next_is_due() {
    let lake = Lake::new();

    let doc = lake.dr_test_report(&[]);

    let kpis = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Kpis(kpis) => Some(kpis.clone()),
            _ => None,
        })
        .expect("the counts");
    let counts: Vec<(&str, &str)> = kpis
        .iter()
        .map(|(label, value, _)| (label.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        counts,
        [
            ("DR tests", "5"),
            ("Met", "1"),
            ("Breached", "1"),
            ("Not recovered", "1"),
            ("Not measured", "2"),
        ]
    );
    // The last test that proves something is the drill: the run that saw
    // no outage proves no recovery. The next one is due a year later.
    let (headers, rows) = table(&doc, "Due state");
    assert_eq!(
        headers,
        [
            "Environment",
            "State",
            "Last DR test",
            "Next due",
            "Interval"
        ]
    );
    assert_eq!(
        rows,
        [[
            "prod".to_string(),
            "ok".into(),
            format!("{} (manual, not measured)", fmt_date(AS_OF - 15 * DAY)),
            fmt_date(AS_OF - 15 * DAY + 365 * DAY),
            "365 days (house default)".into(),
        ]]
    );
    // The plan the drill fulfilled covers the target no longer.
    assert!(
        !section(&doc, "Due state")
            .iter()
            .any(|b| matches!(b, Block::Paragraph(p) if p.starts_with("A planned DR exercise"))),
        "{:#?}",
        section(&doc, "Due state")
    );
}

#[test]
fn a_planned_dr_exercise_that_covers_the_target_is_named_with_its_due_date() {
    let lake = Lake::new();
    let plan = lake
        .store
        .writer()
        .unwrap()
        .create_manual_plan(
            &NewExercisePlan {
                name: "Half-yearly ledger restore".into(),
                exercise_type: ExerciseType::Restore,
                target_system: Some("ledger".into()),
                target_environment: Some("prod".into()),
                function_id: None,
                scope: None,
                owner: "alice".into(),
                planned_at_ns: None,
                due_at_ns: AS_OF + 20 * DAY,
                interval_days: Some(180),
                rto_target_s: None,
                rpo_target_s: None,
            },
            "alice",
        )
        .unwrap();

    let doc = lake.dr_test_report(&[]);

    let (_, rows) = table(&doc, "Due state");
    assert_eq!(rows[0][4], "180 days (the plan)");
    let said = format!(
        "A planned DR exercise covers ledger in prod: plan {plan}, due {}.",
        fmt_date(AS_OF + 20 * DAY)
    );
    assert!(
        section(&doc, "Due state")
            .iter()
            .any(|b| matches!(b, Block::Paragraph(p) if *p == said)),
        "{:#?}",
        section(&doc, "Due state")
    );
}

#[test]
fn the_interval_of_a_test_plan_of_the_function_is_named_as_its_source() {
    let lake = Lake::new();
    let mut with_plan = register();
    with_plan.functions[0].test_plan = vec![PlannedTest {
        kind: "dr".into(),
        interval_days: 90,
    }];
    lake.store
        .writer()
        .unwrap()
        .sync_ict_register(&with_plan, AS_OF - DAY)
        .unwrap();

    let doc = lake.dr_test_report(&[]);

    let (_, rows) = table(&doc, "Due state");
    assert_eq!(rows[0][4], "90 days (the test plan of its function)");
    assert_eq!(rows[0][3], fmt_date(AS_OF - 15 * DAY + 90 * DAY));
}

#[test]
fn the_dr_test_report_links_the_evidence_of_each_test() {
    let lake = Lake::new();

    let doc = lake.dr_test_report(&[]);

    let met = pairs_under_h3(
        &doc,
        &format!("{} ledger promote standby", fmt_date(AS_OF - 40 * DAY)),
    );
    assert_eq!(value(&met, "Run id"), "run-met");
    assert_eq!(value(&met, "Outcome"), "completed");
    assert_eq!(value(&met, "Trace id"), TRACE);
    assert_eq!(
        value(&met, "Journal SHA-256"),
        "not kept in the lake: tumult run prints it when it writes the journal"
    );
    let journal_only = pairs_under_h3(
        &doc,
        &format!("{} ledger restore backup", fmt_date(AS_OF - 30 * DAY)),
    );
    assert_eq!(
        value(&journal_only, "Trace id"),
        "none: the run sent no trace"
    );

    let drill = pairs_under_h3(
        &doc,
        &format!("{} ledger failover drill", fmt_date(AS_OF - 15 * DAY)),
    );
    assert_eq!(value(&drill, "Record id"), lake.drill);
    assert_eq!(value(&drill, "Status"), "verified");
    assert_eq!(value(&drill, "Reviewer"), "carol");
    assert_eq!(
        value(&drill, "Attachment"),
        "restore.log, text/plain, 26 bytes"
    );
    assert_eq!(value(&drill, "SHA-256"), sha256(RESTORE_LOG));
    let signed = values(&drill, "Sign-off");
    assert_eq!(signed.len(), 2, "{signed:?}");
    assert!(
        signed[0].starts_with("bob, reviewer, approved on "),
        "{signed:?}"
    );
    assert!(
        signed[1].starts_with("carol, reviewer, approved on "),
        "{signed:?}"
    );
}

#[test]
fn the_dr_test_report_is_evidence_with_a_block_left_for_signature() {
    let lake = Lake::new();

    let doc = lake.dr_test_report(&[]);

    assert_eq!(
        signoff(&doc),
        [
            ("DR owner".to_string(), String::new()),
            ("Reviewer".to_string(), String::new())
        ]
    );
    assert!(
        footnotes(&doc).contains(&EVIDENCE_DISCLAIMER),
        "{:?}",
        footnotes(&doc)
    );
    assert_eq!(
        doc.meta.subject,
        [
            ("Target".to_string(), "ledger".to_string()),
            ("Environments".to_string(), "every environment".to_string())
        ]
    );
    let scope = format!(
        "This report lists the DR tests of ledger in every environment from {} to {}: the runs \
         of kind dr, and the manual exercises of a type the house counts as DR (backup, drill, \
         failover, restore) that are submitted or verified. Each test is set against the RTO \
         and the RPO it planned. A test that did not recover, and a run whose probes saw no \
         outage, say so. The report holds evidence only. It is not a compliance verdict.",
        fmt_date(FROM),
        fmt_date(AS_OF)
    );
    assert!(
        lead(&doc, "Scope")
            .iter()
            .any(|b| matches!(b, Block::Paragraph(p) if *p == scope)),
        "{:#?}",
        lead(&doc, "Scope")
    );
    let html = render(&doc);
    assert!(html.contains("DR test report: ledger"), "{html}");
    assert!(html.contains("<th>Target</th><td>ledger</td>"), "{html}");
    for verdict in ["COMPLIANT", "Compliant", "compliant with"] {
        assert!(!html.contains(verdict), "a compliance verdict: {verdict}");
    }
    let pdf = render_pdf(&doc).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn the_dr_test_report_reads_only_the_environments_it_is_given() {
    let lake = Lake::new();

    let doc = lake.dr_test_report(&["staging".to_string()]);

    assert!(
        section(&doc, "DR tests").iter().any(
            |b| matches!(b, Block::Paragraph(p) if p == "No DR test of ledger in this period.")
        ),
        "{:#?}",
        section(&doc, "DR tests")
    );
    assert!(
        !format!("{:?}", doc.blocks).contains("run-met"),
        "a run of another environment"
    );
    assert_eq!(doc.meta.subject[1].1, "staging");
}

#[test]
fn the_dr_exercise_report_holds_the_whole_record() {
    let lake = Lake::new();

    let doc = lake.exercise_report(&lake.drill, &[]).expect("the drill");

    assert_eq!(doc.meta.template, TemplateKind::DrExercise);
    assert!(
        doc.meta.doc_id.starts_with("KRK-R5-"),
        "{}",
        doc.meta.doc_id
    );
    assert_eq!(doc.meta.title, "DR exercise report: ledger failover drill");
    let summary = pairs(lead(&doc, "Exercise"));
    assert_eq!(value(&summary, "Record id"), lake.drill);
    assert_eq!(value(&summary, "Type"), "failover");
    assert_eq!(value(&summary, "Target"), "ledger");
    assert_eq!(value(&summary, "Status"), "verified");
    assert!(
        value(&summary, "Entered by").starts_with("alice on "),
        "{summary:?}"
    );
    assert!(
        value(&summary, "Reviewed by").starts_with("carol on "),
        "{summary:?}"
    );
    let plan = pairs(section(&doc, "Plan"));
    assert_eq!(value(&plan, "Plan id"), lake.plan);
    assert_eq!(value(&plan, "Name"), "Yearly ledger failover");
    assert_eq!(
        table(&doc, "Participants").1,
        [["alice", "DR owner"], ["dave", "database admin"]]
    );
    let (headers, steps) = table(&doc, "Steps");
    assert_eq!(
        headers,
        ["#", "Step", "Actor", "Started", "Ended", "Result", "Note"]
    );
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1][1], "Promote the standby");
    assert!(steps[1][3].ends_with(" UTC"), "{:?}", steps[1]);
    assert_eq!(steps[1][6], "waited on a stale lock");
    // The RTO was met. The RPO was planned and never measured: not met.
    assert_eq!(
        table(&doc, "Recovery objectives").1,
        [
            ["RTO", "900.0 s", "600.0 s", "met"],
            ["RPO", "60.0 s", "not measured", "not measured"]
        ]
    );
    assert_eq!(
        value(
            &pairs(section(&doc, "Recovery objectives")),
            "Data verified"
        ),
        "yes, complete"
    );
    assert!(section(&doc, "Findings").iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "The promote script waited on a stale lock for 4 minutes."
    )));
    let (_, actions) = table(&doc, "Actions");
    assert_eq!(
        actions,
        [[
            "1".to_string(),
            "Clear the stale lock before a promote".into(),
            "high".into(),
            "dave".into(),
            fmt_date(AS_OF + 30 * DAY),
            "OPS-1234".into(),
            "open".into(),
        ]]
    );
}

#[test]
fn the_dr_exercise_report_gives_the_severity_of_the_findings_and_of_each_action() {
    let lake = Lake::new();

    let doc = lake.exercise_report(&lake.drill, &[]).expect("the drill");

    let findings = section(&doc, "Findings");
    assert_eq!(value(&pairs(findings), "Severity"), "medium");
    assert!(findings.iter().any(|b| matches!(
        b,
        Block::KeyValues(kvs) if kvs.iter().any(|(k, v)| k == "Severity" && *v == Cell::status("medium"))
    )), "the severity is a status, a glyph and a word: {findings:#?}");
    let (headers, actions) = table(&doc, "Actions");
    let column = headers
        .iter()
        .position(|h| h == "Severity")
        .unwrap_or_else(|| panic!("no Severity column in {headers:?}"));
    assert_eq!(actions[0][column], "high");
}

#[test]
fn a_finding_or_an_action_with_no_severity_says_so() {
    let lake = Lake::new();
    let id = {
        let writer = lake.store.writer().unwrap();
        let id = writer
            .create_manual_draft(&NewManualExperiment {
                experiment_name: "ledger second drill".into(),
                exercise: ExerciseDetail {
                    actions: vec![ActionItem {
                        title: "Write the promote runbook down".into(),
                        owner: "dave".into(),
                        due_at_ns: None,
                        ticket: None,
                        severity: None,
                        retest: None,
                    }],
                    ..ExerciseDetail::default()
                },
                ..drill("unused")
            })
            .unwrap();
        writer.submit_manual(&id, None, "alice").unwrap();
        id
    };

    let doc = lake.exercise_report(&id, &[]).expect("the second drill");

    assert_eq!(
        value(&pairs(section(&doc, "Findings")), "Severity"),
        "not set"
    );
    let (headers, actions) = table(&doc, "Actions");
    let column = headers.iter().position(|h| h == "Severity").unwrap();
    assert_eq!(actions[0][column], "not set");
}

#[test]
fn the_dr_exercise_report_shows_the_files_the_sign_offs_and_the_audit_verify() {
    let lake = Lake::new();

    let doc = lake.exercise_report(&lake.drill, &[]).expect("the drill");

    let file = pairs(section(&doc, "Attachments"));
    assert_eq!(value(&file, "Name"), "restore.log");
    assert_eq!(value(&file, "SHA-256"), sha256(RESTORE_LOG));
    assert_eq!(value(&file, "Size"), "26 bytes");
    let (headers, signed) = table(&doc, "Sign-offs");
    assert_eq!(headers, ["Role", "Signed by", "Signed", "Decision", "Note"]);
    let by: Vec<&str> = signed.iter().map(|row| row[1].as_str()).collect();
    assert_eq!(by, ["bob", "carol"]);
    assert!(section(&doc, "Sign-offs").iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "2 approvals of the 2 that a failover needs."
    )));
    assert!(section(&doc, "Audit verify").iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p.starts_with("The audit verify found no problem")
    )));
    assert!(
        !section(&doc, "Audit verify").iter().any(|b| matches!(
            b,
            Block::Paragraph(p) if p.contains("came before the link hash")
        )),
        "no row of the trail came before the link hash"
    );
    assert_eq!(
        signoff(&doc),
        [
            ("DR owner".to_string(), String::new()),
            ("Reviewer".to_string(), String::new())
        ]
    );
    assert!(footnotes(&doc).contains(&EVIDENCE_DISCLAIMER));
    assert!(render_pdf(&doc).unwrap().starts_with(b"%PDF"));
}

#[test]
fn the_dr_exercise_report_says_what_the_audit_verify_found_wrong() {
    let lake = Lake::new();
    // The stored file is gone behind the API.
    let hash = sha256(RESTORE_LOG);
    let stored = attachments_dir(lake.store.path())
        .join(&hash[..2])
        .join(&hash);
    std::fs::remove_file(stored).unwrap();

    let doc = lake.exercise_report(&lake.drill, &[]).expect("the drill");

    let verify = section(&doc, "Audit verify");
    assert!(
        verify.iter().any(|b| matches!(
            b,
            Block::Paragraph(p) if p.starts_with("The audit verify found 1 problem")
        )),
        "{verify:#?}"
    );
    let (headers, problems) = table(&doc, "Audit verify");
    assert_eq!(headers, ["Table", "Row", "Field", "Problem"]);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0][0], "evidence_attachments");
}

#[test]
fn a_dr_exercise_report_is_only_of_a_dr_record_in_the_environments_given() {
    let lake = Lake::new();

    assert!(lake.exercise_report("no-such-record", &[]).is_none());
    assert!(
        lake.exercise_report(&lake.drill, &["staging".to_string()])
            .is_none(),
        "a record of another environment"
    );
    assert!(lake
        .exercise_report(&lake.drill, &["prod".to_string()])
        .is_some());
    assert!(
        lake.exercise_report(&lake.game_day, &[]).is_none(),
        "a game day is no DR exercise"
    );
    assert!(
        lake.exercise_report(&lake.draft, &[]).is_none(),
        "a draft is no DR test"
    );
}

/// The status and the evidence ids `GET /api/dora` gives each row of the
/// requirement registry, for the same lake at the same time.
fn dora_rows(lake: &Lake) -> Vec<(String, String, Vec<String>)> {
    let reader = lake.store.read_only().unwrap();
    let request = Request {
        envs: &[],
        lei: None,
        profile: None,
        as_of_ns: AS_OF,
        workloads: &[],
        proof_days: false,
    };
    let loaded = loader::load(&reader, request, &lake.params).unwrap();
    let matrix = Matrix::build(&loaded.inputs, &lake.params);
    let doc = report::dora(&loaded, &matrix, DoraBy::Entity);
    doc["articles"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|article| article["requirements"].as_array().unwrap())
        .map(|row| {
            let mut ids: Vec<String> = Vec::new();
            for cell in row["entities"].as_array().unwrap() {
                let (Some(source), Some(id)) =
                    (cell["source"].as_str(), cell["evidence_id"].as_str())
                else {
                    continue;
                };
                let named = format!("{source} {id}");
                if !ids.contains(&named) {
                    ids.push(named);
                }
            }
            (
                row["cite"].as_str().unwrap().to_string(),
                row["status"].as_str().unwrap().to_string(),
                ids,
            )
        })
        .collect()
}

#[test]
fn the_dora_matrix_of_the_evidence_pack_is_the_requirement_registry_as_the_dora_score_reads_it() {
    let lake = Lake::new();

    let doc = lake.dora_pack();

    let (headers, rows) = table(&doc, "Traceability matrix");
    assert_eq!(
        headers,
        ["Article and paragraph", "Requirement", "Status", "Evidence"]
    );
    let cites: Vec<&str> = rows.iter().map(|row| row[0].as_str()).collect();
    let registry: Vec<&str> = REQUIREMENTS.iter().map(|r| r.cite).collect();
    assert_eq!(cites, registry, "every row of the registry, in its order");
    assert!(!format!("{rows:?}").contains("Unmapped"), "{rows:#?}");
    let scored = dora_rows(&lake);
    assert!(!scored.is_empty());
    for (cite, status, ids) in &scored {
        let row = row_of(&rows, 0, cite);
        let words = if status == "na" {
            "not applicable".to_string()
        } else {
            status.replace('_', " ")
        };
        assert_eq!(row[2], words, "{cite}: {row:?}");
        if !ids.is_empty() {
            assert_eq!(row[3], ids.join(", "), "{cite}: {row:?}");
        }
    }
    // A row the DR runs cover shows them as its evidence.
    let covered = scored
        .iter()
        .find(|(_, _, ids)| ids.iter().any(|id| id.starts_with("run run-")))
        .expect("a row a DR run covers");
    assert!(
        row_of(&rows, 0, &covered.0)[3].starts_with("run run-"),
        "{covered:?}"
    );
    // A row the profile of the entity excludes says so.
    let csd = row_of(&rows, 0, "Art. 11(9)");
    assert_eq!(
        csd[2..],
        [
            "not applicable",
            "the profile of the legal entity excludes this row"
        ]
    );
}

#[test]
fn the_test_register_of_the_evidence_pack_gives_the_kind_rto_and_rpo() {
    let lake = Lake::new();

    let doc = lake.dora_pack();

    // The register keeps its columns; the table after it gives the kind,
    // the RTO and the RPO of each test, in the order of the register.
    let (_, register) = table(&doc, "Test register");
    let (headers, rows) = section(&doc, "Test register")
        .iter()
        .filter_map(|b| match b {
            Block::Table { headers, rows, .. } => Some((headers.clone(), rows.clone())),
            _ => None,
        })
        .nth(1)
        .expect("the table of the kind, the RTO and the RPO");
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|c| text(c).to_string()).collect())
        .collect();
    assert_eq!(
        headers,
        [
            "Experiment",
            "Kind",
            "RTO planned",
            "RTO actual",
            "RPO planned",
            "RPO actual"
        ]
    );
    let names =
        |rows: &[Vec<String>]| -> Vec<String> { rows.iter().map(|r| r[0].clone()).collect() };
    assert_eq!(names(&rows), names(&register));
    assert_eq!(
        row_of(&rows, 0, "ledger zone loss")[1..],
        ["dr", "10.0 s", "not recovered", "not set", "not measured"]
    );
    assert_eq!(
        row_of(&rows, 0, "ledger replica kill")[3],
        "no outage observed"
    );
    assert_eq!(
        row_of(&rows, 0, "ledger promote standby")[1..],
        ["dr", "10.0 s", "4.0 s", "5.0 s", "2.0 s"]
    );
    assert_eq!(
        row_of(&rows, 0, "ledger failover drill")[1..],
        ["failover", "900.0 s", "600.0 s", "60.0 s", "not measured"]
    );
}

#[test]
fn the_attestation_appendix_of_the_evidence_pack_gives_the_dr_detail_of_a_record() {
    let lake = Lake::new();

    let doc = lake.dora_pack();

    let drill = pairs_under_h3(&doc, "ledger failover drill");
    assert_eq!(value(&drill, "RTO planned / actual"), "900.0 s / 600.0 s");
    assert_eq!(
        value(&drill, "RPO planned / actual"),
        "60.0 s / not measured"
    );
    assert_eq!(value(&drill, "Data verified"), "yes, complete");
    assert_eq!(value(&drill, "Plan"), lake.plan);
    assert_eq!(
        value(&drill, "Attachment"),
        "restore.log, text/plain, 26 bytes"
    );
    assert_eq!(value(&drill, "SHA-256"), sha256(RESTORE_LOG));
    assert_eq!(values(&drill, "Sign-off").len(), 2);
    let game_day = pairs_under_h3(&doc, "ledger game day");
    assert!(
        values(&game_day, "RTO planned / actual").is_empty(),
        "a game day is no DR record: {game_day:?}"
    );
}

#[test]
fn the_attestation_appendix_names_each_dora_citation_by_its_registry_id() {
    let lake = Lake::new();

    let doc = lake.dora_pack();

    // The drill was stored citing `DORA Art. 24(6)`: the appendix names
    // the registry row, as the DORA score and the Star Map do. A citation
    // of another framework stays as written.
    let drill = pairs_under_h3(&doc, "ledger failover drill");
    assert_eq!(
        value(&drill, "Frameworks"),
        "dora.art24.6, ISO 27001 A.5.30"
    );
}

/// The paragraphs of `blocks`.
fn paragraphs(blocks: &[Block]) -> Vec<&str> {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(p) => Some(p.as_str()),
            _ => None,
        })
        .collect()
}

/// What the evidence pack says under Independence, and how many tests
/// its register holds.
fn independence(doc: &ReportDoc) -> (Vec<String>, usize) {
    let (_, register) = table(doc, "Test register");
    assert!(
        register.iter().any(|row| row[1] != "manual"),
        "the register holds runs: {register:?}"
    );
    let said = paragraphs(lead(doc, "Independence"))
        .into_iter()
        .map(str::to_owned)
        .collect();
    (said, register.len())
}

#[test]
fn the_evidence_pack_counts_the_tests_that_name_an_external_tester() {
    let lake = Lake::new();

    // The drill names an external tester; the game day names none.
    let (said, tests) = independence(&lake.dora_pack());
    assert_eq!(
        said[0],
        format!("Independence verified: 1 of {tests} tests name an external tester."),
        "{said:#?}"
    );
    assert_eq!(
        said[1],
        "The count reads the tester each manual record names. A run names its tester only \
         in its journal, which the lake does not keep, so no run counts as naming one."
    );
    assert!(
        !said.iter().any(|p| p.contains("has not been verified")),
        "{said:#?}"
    );

    // A tabletop the ledger team ran itself is one more test, and no
    // external one.
    {
        let writer = lake.store.writer().unwrap();
        let id = writer
            .create_manual_draft(&NewManualExperiment {
                experiment_name: "ledger tabletop".into(),
                exercise_type: ExerciseType::Tabletop,
                exercise: ExerciseDetail {
                    tester: Some(Tester {
                        name: "the ledger team".into(),
                        kind: TesterKind::Internal,
                    }),
                    ..ExerciseDetail::default()
                },
                ..drill("unused")
            })
            .unwrap();
        writer.submit_manual(&id, None, "alice").unwrap();
        writer.verify_manual(&id, "bob", None).unwrap();
    }
    let (said, more) = independence(&lake.dora_pack());
    assert_eq!(more, tests + 1);
    assert_eq!(
        said[0],
        format!("Independence verified: 1 of {more} tests name an external tester."),
        "{said:#?}"
    );
}

#[test]
fn a_report_of_an_empty_lake_says_there_is_nothing_and_still_renders() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("lake.duckdb")).unwrap();
    let reader = store.read_only().unwrap();
    let params = HouseParams::default();
    let request = DrTestRequest {
        target: "ledger",
        from_ns: FROM,
        to_ns: AS_OF,
    };

    let doc = build_dr_test(&reader, &House::of(&params), &request, AS_OF, &[]).unwrap();

    assert!(section(&doc, "Due state").iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p.starts_with("The DR test register holds no entry for ledger")
    )));
    assert!(render_pdf(&doc).unwrap().starts_with(b"%PDF"));
    let pack = build_evidence_pack(&reader, "dora", None, AS_OF, &[], &House::of(&params)).unwrap();
    let (_, rows) = table(&pack, "Traceability matrix");
    assert_eq!(rows.len(), REQUIREMENTS.len());
    assert_eq!(
        paragraphs(lead(&pack, "Independence"))[0],
        "Independence verified: 0 of 0 tests name an external tester."
    );
    assert!(render_pdf(&pack).unwrap().starts_with(b"%PDF"));
}
