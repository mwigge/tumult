//! Builder tests (moved out of `builders.rs`): doc-id format, framework
//! clause coverage, the R2 approval-chain table, and environment scoping
//! across R1/R2/R3.

use super::*;
use crate::model::{Block, Cell, ChartSpec, ReportDoc};
use crate::org::OrgTree;

#[test]
fn doc_id_format() {
    let id = doc_id("R1", "seed", 1_785_273_590_000_000_000);
    assert!(id.starts_with("KRK-R1-20260728-"), "got {id}");
    assert_eq!(id.len(), "KRK-R1-20260728-".len() + 6);
    assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
}

#[test]
fn every_framework_but_dora_lists_its_clauses_and_dora_reads_the_registry() {
    assert_eq!(FRAMEWORK_CLAUSES.len(), 4);
    for (name, clauses) in FRAMEWORK_CLAUSES {
        assert_eq!(clauses.is_empty(), *name == "dora", "{name}");
    }
}

const BASE_NS: i64 = 1_785_000_000_000_000_000;
const HOUR_NS: i64 = 3_600_000_000_000;

/// A store with two gated runs: `run-a` (T2, quorum 2, approved×2,
/// consumed) and `run-b` (T1, overridden by break-glass).
fn gated_fixture() -> (tempfile::TempDir, tumult_lake::Store) {
    use tumult_lake::approvals::{decision, ApprovalDecision, ApprovalRequest};
    use tumult_lake::{NewRun, RegisteredDefinition};

    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    writer
        .register_definition(&RegisteredDefinition {
            id: "reg-1".into(),
            name: "cpu-burn".into(),
            definition_toon: "title: cpu-burn".into(),
            content_hash: "h".into(),
            registered_at_ns: 1,
            registered_by: None,
        })
        .unwrap();
    let gated_run = |id: &str, actor: &str, queued_at_ns: i64| NewRun {
        id: id.into(),
        registry_id: "reg-1".into(),
        params_json: None,
        queued_at_ns,
        actor: Some(actor.into()),
    };
    writer
        .insert_gated_run(
            &gated_run("run-a", "carol", BASE_NS),
            &ApprovalRequest {
                run_id: "run-a".into(),
                tier: "T2".into(),
                pin_hash: "deadbeef".into(),
                env: "prod".into(),
                target: Some("svc-a".into()),
                quorum_required: 2,
                requested_by: "carol".into(),
                requested_at_ns: BASE_NS,
                expires_at_ns: BASE_NS + HOUR_NS,
            },
            Some("tier T2 quorum 2"),
        )
        .unwrap();
    for approver in ["dave", "erin"] {
        writer
            .insert_approval_decision(&ApprovalDecision {
                run_id: "run-a".into(),
                approver: approver.into(),
                decision: decision::APPROVED.into(),
                note: None,
                decided_at_ns: BASE_NS + 60_000_000_000,
            })
            .unwrap();
    }
    writer
        .consume_approval("run-a", BASE_NS + 120_000_000_000)
        .unwrap();

    writer
        .insert_gated_run(
            &gated_run("run-b", "frank", BASE_NS + DAY_NS),
            &ApprovalRequest {
                run_id: "run-b".into(),
                tier: "T1".into(),
                pin_hash: "cafe".into(),
                env: "staging".into(),
                target: None,
                quorum_required: 1,
                requested_by: "frank".into(),
                requested_at_ns: BASE_NS + DAY_NS,
                expires_at_ns: BASE_NS + DAY_NS + HOUR_NS,
            },
            Some("tier T1 quorum 1"),
        )
        .unwrap();
    writer
        .mark_break_glass("run-b", "admin", "prod down")
        .unwrap();
    (d, store)
}

/// The approval-chain table after its H2 heading.
fn approval_table(doc: &ReportDoc) -> (&Vec<String>, &Vec<Vec<Cell>>) {
    let pos = doc
        .blocks
        .iter()
        .position(|b| matches!(b, Block::H2(t) if t.contains("Approval chain")))
        .expect("approval chain H2");
    doc.blocks[pos..]
        .iter()
        .find_map(|b| match b {
            Block::Table { headers, rows, .. } => Some((headers, rows)),
            _ => None,
        })
        .expect("approval chain table")
}

fn row_text(row: &[Cell]) -> String {
    row.iter()
        .map(|c| match c {
            Cell::Text(s) | Cell::Status(s) => s.as_str(),
        })
        .collect::<Vec<_>>()
        .join("|")
}

#[test]
fn evidence_pack_lists_approval_chain() {
    let (_d, store) = gated_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + DAY_NS + 12 * HOUR_NS;
    let doc = build_evidence_pack(
        &reader,
        "soc2",
        None,
        now,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();

    let (headers, rows) = approval_table(&doc);
    assert!(headers.iter().any(|h| h == "Tier"));
    assert_eq!(rows.len(), 2, "rows: {rows:?}");
    let rows: Vec<String> = rows.iter().map(|r| row_text(r)).collect();
    let t2 = rows.iter().find(|r| r.contains("T2")).expect("T2 row");
    assert!(t2.contains("carol"), "{t2}");
    assert!(t2.contains("2/2"), "{t2}");
    assert!(t2.contains("yes"), "{t2}"); // consumed
    let t1 = rows.iter().find(|r| r.contains("T1")).expect("T1 row");
    assert!(t1.contains("frank"), "{t1}");
    assert!(t1.contains("yes — admin"), "{t1}"); // break-glass

    let pdf = crate::typst_pdf::render_pdf(&doc).expect("pdf render");
    assert!(pdf.starts_with(b"%PDF"), "missing magic bytes");
}

#[test]
fn evidence_pack_approval_chain_empty_message() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let reader = store.read_only().unwrap();
    let doc = build_evidence_pack(
        &reader,
        "soc2",
        None,
        BASE_NS,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "No approval-gated runs in the selected sample for this scope and period."
    )));
}

#[test]
fn evidence_pack_approval_chain_respects_period() {
    let (_d, store) = gated_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + DAY_NS + 12 * HOUR_NS;

    // A one-hour period excludes both requests.
    let doc = build_evidence_pack(
        &reader,
        "soc2",
        Some(HOUR_NS),
        now,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "No approval-gated runs in the selected sample for this scope and period."
    )));

    // A period reaching back past run-b but not run-a only keeps run-b.
    let doc = build_evidence_pack(
        &reader,
        "soc2",
        Some(DAY_NS + 12 * HOUR_NS - 1),
        now,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();
    let (_, rows) = approval_table(&doc);
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert!(row_text(&rows[0]).contains("T1"));
}

/// A store with one root span per environment: `stg-exp` in staging,
/// `prd-exp` in prod (plus the gated runs of [`gated_fixture`]).
fn env_fixture() -> (tempfile::TempDir, tumult_lake::Store) {
    let (d, store) = gated_fixture();
    let root = |id: &str, name: &str, env: &str, ts: i64| tumult_lake::SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: HOUR_NS,
        service_name: "tumult".into(),
        experiment_id: Some(id.into()),
        experiment_name: Some(name.into()),
        target_system: Some("database".into()),
        target_environment: Some(env.into()),
        events: "[]".into(),
        ..Default::default()
    };
    store
        .writer()
        .unwrap()
        .insert_spans(&[
            root("exp-stg", "stg-exp", "staging", BASE_NS),
            root("exp-prd", "prd-exp", "prod", BASE_NS),
        ])
        .unwrap();
    (d, store)
}

/// Experiment names from the R1 "Experiment scores" bar chart.
fn bar_names(doc: &ReportDoc) -> Vec<String> {
    doc.blocks
        .iter()
        .find_map(|b| match b {
            Block::Chart(ChartSpec::Bars(bars)) => {
                Some(bars.iter().map(|(n, _)| n.clone()).collect())
            }
            _ => None,
        })
        .expect("experiment scores bar chart")
}

#[test]
fn executive_digest_scoped_contains_only_in_scope_experiments() {
    let (_d, store) = env_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + 2 * DAY_NS;
    let org = OrgTree::empty();

    let global = build_executive(&reader, &org, now, DAY_NS, now, &[]).unwrap();
    assert_eq!(bar_names(&global), ["prd-exp", "stg-exp"]);

    let scoped =
        build_executive(&reader, &org, now, DAY_NS, now, &["staging".to_string()]).unwrap();
    assert_eq!(bar_names(&scoped), ["stg-exp"]);
    // The scope set is part of the document identity.
    assert_ne!(global.meta.doc_id, scoped.meta.doc_id);
}

#[test]
fn evidence_pack_scoped_confines_register_and_approvals() {
    let (_d, store) = env_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + DAY_NS + 12 * HOUR_NS;

    let scoped = build_evidence_pack(
        &reader,
        "soc2",
        None,
        now,
        &["staging".to_string()],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();
    // Test register: only the staging experiment.
    let register = scoped
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Test register"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("test register table");
    assert_eq!(register.len(), 1, "rows: {register:?}");
    assert!(row_text(&register[0]).contains("stg-exp"));
    // Approval chain: only the staging run (run-b, T1).
    let (_, rows) = approval_table(&scoped);
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert!(row_text(&rows[0]).contains("T1"));
}

#[test]
fn evidence_pack_rejects_unknown_framework() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let reader = store.read_only().unwrap();
    let err = build_evidence_pack(
        &reader,
        "pci-dss",
        None,
        BASE_NS,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap_err();
    assert!(err.contains("unknown framework"), "{err}");
    assert!(err.contains("dora"), "{err}");
}

/// A store with:
/// * `flaky-exp` — an automated experiment that deviated, then recovered
///   (completed), then deviated again: the latest run decides (Failed), and
///   the earlier deviation counts as discovered *and* fixed.
/// * `drill-exp` — a verified manual gameday record with a `partial`
///   outcome, entered by alice and verified by bob.
fn findings_fixture() -> (tempfile::TempDir, tumult_lake::Store) {
    use tumult_lake::{ExerciseType, ManualOutcome, NewManualExperiment};

    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let root = |id: &str, name: &str, ts: i64, recovery: Option<f64>| tumult_lake::SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: HOUR_NS,
        service_name: "tumult".into(),
        experiment_id: Some(id.into()),
        experiment_name: Some(name.into()),
        target_system: Some("database".into()),
        target_environment: Some("prod".into()),
        recovery_time_s: recovery,
        events: "[]".into(),
        ..Default::default()
    };
    writer
        .insert_spans(&[
            // An old pass: the previous period's latest run (score 100), so
            // the current period's deviation reads as a decline.
            root("exp-0", "flaky-exp", BASE_NS - 8 * DAY_NS, None),
            root("exp-1a", "flaky-exp", BASE_NS, Some(12.0)),
            root("exp-1b", "flaky-exp", BASE_NS + HOUR_NS, None),
            root("exp-1c", "flaky-exp", BASE_NS + 2 * HOUR_NS, Some(30.0)),
        ])
        .unwrap();
    let done = |id: &str, status: &str, ts: i64| tumult_lake::LogRow {
        ts_ns: ts,
        severity_text: "INFO".into(),
        body: "experiment.completed".into(),
        trace_id: Some(format!("trace-{id}")),
        span_id: None,
        service_name: "tumult".into(),
        log_attrs: vec![
            ("experiment_id".to_string(), id.to_string()),
            ("status".to_string(), status.to_string()),
        ],
        resource_attrs: vec![],
        ..tumult_lake::LogRow::default()
    };
    writer
        .insert_logs(&[
            done("exp-0", "Completed", BASE_NS - 8 * DAY_NS + 60_000_000_000),
            done("exp-1a", "Deviated", BASE_NS + 60_000_000_000),
            done("exp-1b", "Completed", BASE_NS + HOUR_NS + 60_000_000_000),
            done("exp-1c", "Deviated", BASE_NS + 2 * HOUR_NS + 60_000_000_000),
        ])
        .unwrap();

    let id = writer
        .create_manual_draft(&NewManualExperiment {
            experiment_name: "drill-exp".into(),
            exercise_type: ExerciseType::GameDay,
            executed_at_ns: BASE_NS + 3 * HOUR_NS,
            hypothesis: "failover keeps p95 under 800ms".into(),
            method: "disabled the primary PoP".into(),
            outcome: ManualOutcome::Partial,
            hypothesis_met: Some(true),
            findings: None,
            action_items: vec![],
            target_system: Some("cdn".into()),
            target_environment: Some("prod".into()),
            blast_radius: None,
            recovery_time_s: None,
            duration_s: None,
            entered_by: "alice".into(),
            attestation: "I attest this record reflects the exercise.".into(),
            renewal_due_ns: None,
            framework_refs: vec!["DORA Art. 24(6)".into(), "ISO 27001 A.5.29".into()],
            exercise: tumult_lake::ExerciseDetail::default(),
        })
        .unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer.verify_manual(&id, "bob", Some("reviewed")).unwrap();
    (d, store)
}

/// The test-register table after its H2 heading.
fn register_table(doc: &ReportDoc) -> Vec<Vec<Cell>> {
    doc.blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Test register"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("test register table")
}

#[test]
fn evidence_pack_renders_register_provenance_attestation_and_findings() {
    let (_d, store) = findings_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + 6 * HOUR_NS;
    let doc = build_evidence_pack(
        &reader,
        "dora",
        Some(12 * HOUR_NS),
        now,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();

    assert_eq!(doc.meta.framework.as_deref(), Some("DORA"));
    assert_eq!(doc.meta.period, Some((now - 12 * HOUR_NS, now)));
    // The scope paragraph names the framework and the period.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p.contains("DORA") && p.contains('–') && p.contains("experiments are on record")
    )));
    // The DORA matrix is every row of the requirement registry. No function
    // of a register binds these targets, so no row names their evidence.
    let matrix = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Traceability matrix"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("traceability matrix");
    assert_eq!(matrix.len(), tumult_core::compliance::REQUIREMENTS.len());
    assert!(matrix.iter().all(|row| !row_text(row).contains("Unmapped")));
    assert!(matrix
        .iter()
        .all(|row| !row_text(row).contains("flaky-exp") && !row_text(row).contains("drill-exp")));

    // Register: the manual record carries entered/verifier provenance.
    let register = register_table(&doc);
    assert_eq!(register.len(), 2, "rows: {register:?}");
    let manual_row = register
        .iter()
        .find(|r| row_text(r).contains("drill-exp"))
        .expect("manual row");
    let text = row_text(manual_row);
    assert!(text.contains("manual"), "{text}");
    // Where the evidence comes from is a word, and no status with a mark.
    assert_eq!(manual_row[1], Cell::text("manual"));
    assert!(
        register.iter().all(|row| matches!(row[1], Cell::Text(_))),
        "{register:?}"
    );
    assert!(text.contains("bob"), "{text}");
    assert!(text.contains("partial"), "{text}");
    assert!(text.contains("|75"), "{text}");

    // Attestation appendix: one H3 per verified record, frameworks joined.
    let appendix = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Manual attestation appendix"));
    let blocks: Vec<&Block> = appendix.collect();
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, Block::H3(t) if t.starts_with("drill-exp — "))),
        "{blocks:?}"
    );
    assert!(blocks.iter().any(|b| matches!(
        b,
        Block::KeyValues(kvs) if kvs.iter().any(|(k, v)|
            k == "Frameworks" && row_text(std::slice::from_ref(v)).contains("ISO 27001 A.5.29"))
    )));

    // Findings log: the failed automated run is listed with its outcome.
    let findings = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Findings log"))
        .find_map(|b| match b {
            Block::Bullets(items) => Some(items.clone()),
            _ => None,
        })
        .expect("findings bullets");
    // The failed run and the partial drill are both open.
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(findings[0].contains("flaky-exp"), "{findings:?}");
    // The seed spells the status with a capital, as a run stored before
    // 3.0.2 does; the report gives it in the spelling of the journal.
    assert!(findings[0].contains("deviated"), "{findings:?}");
    assert!(findings[1].contains("drill-exp"), "{findings:?}");
}

#[test]
fn a_verified_manual_partial_is_open_in_the_digest_and_in_the_evidence_pack() {
    let (_d, store) = findings_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + 6 * HOUR_NS;

    let digest = build_executive(&reader, &OrgTree::empty(), now, 7 * DAY_NS, now, &[]).unwrap();
    let pack = build_evidence_pack(
        &reader,
        "dora",
        Some(12 * HOUR_NS),
        now,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();

    let open = rows_under(&digest, "Open weaknesses and decisions needed");
    assert!(
        open.iter().any(|row| row.contains("|drill-exp|")),
        "{open:?}"
    );
    let findings = bullets_under(&pack, "Findings log");
    assert!(
        findings
            .iter()
            .any(|finding| finding.starts_with("drill-exp (cdn): partial — last run ")),
        "{findings:?}"
    );
}

#[test]
fn an_open_weakness_with_no_severity_on_record_says_not_recorded() {
    let (_d, store) = findings_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + 6 * HOUR_NS;

    let doc = build_executive(&reader, &OrgTree::empty(), now, 7 * DAY_NS, now, &[]).unwrap();

    // Neither the manual drill nor the run recorded a severity.
    let severities: Vec<Cell> = blocks_under(&doc, "Open weaknesses and decisions needed")
        .into_iter()
        .find_map(|block| match block {
            Block::Table { rows, .. } => Some(rows.iter().map(|row| row[0].clone()).collect()),
            _ => None,
        })
        .expect("open weaknesses table");
    assert_eq!(
        severities,
        [Cell::text("not recorded"), Cell::text("not recorded")]
    );
    let html = crate::html::render_html(&doc);
    assert!(!html.contains("medium"), "a severity nobody recorded");
}

#[test]
fn the_evidence_pack_says_where_an_outcome_comes_from() {
    let (_d, store) = findings_fixture();
    let reader = store.read_only().unwrap();
    let doc = build_evidence_pack(
        &reader,
        "dora",
        None,
        BASE_NS + 6 * HOUR_NS,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();

    let scope = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Paragraph(p) => Some(p.as_str()),
            _ => None,
        })
        .expect("scope paragraph");

    assert!(
        scope.ends_with(
            "; 2 experiments are on record. An automated experiment is a chaos or continuity \
             test that Tumult ran. The outcome of a run is the status that run recorded; a \
             manual exercise has the outcome of its verified record."
        ),
        "{scope}"
    );
}

#[test]
fn evidence_pack_does_not_infer_clause_coverage_from_many_experiments() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let root = |n: usize| tumult_lake::SpanRow {
        ts_ns: BASE_NS,
        trace_id: format!("trace-{n}"),
        span_id: format!("span-{n}"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: HOUR_NS,
        service_name: "tumult".into(),
        experiment_id: Some(format!("exp-{n}")),
        experiment_name: Some(format!("exp-{n}")),
        events: "[]".into(),
        ..Default::default()
    };
    writer
        .insert_spans(&(0..4).map(root).collect::<Vec<_>>())
        .unwrap();
    let reader = store.read_only().unwrap();
    let doc = build_evidence_pack(
        &reader,
        "nis2",
        None,
        BASE_NS + HOUR_NS,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();
    let matrix = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Traceability matrix"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("traceability matrix");
    assert!(matrix.iter().all(|row| row_text(row).contains("Unmapped")));
    assert_eq!(
        register_table(&doc).len(),
        4,
        "tests remain visible as evidence candidates"
    );
}

/// The paragraph under the "Bottom line" heading.
fn bottom_line(doc: &ReportDoc) -> &str {
    doc.blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H1(t) if t == "Bottom line"))
        .find_map(|b| match b {
            Block::Paragraph(p) => Some(p.as_str()),
            _ => None,
        })
        .expect("bottom line")
}

/// The paragraph right under the KPI cards.
fn under_the_kpis(doc: &ReportDoc) -> &str {
    let cards = doc
        .blocks
        .iter()
        .position(|b| matches!(b, Block::Kpis(_)))
        .expect("KPI cards");
    match &doc.blocks[cards + 1] {
        Block::Paragraph(p) => p,
        other => panic!("{other:?} under the KPI cards"),
    }
}

/// What the digest says under its portfolio score.
fn compliance_score_line() -> String {
    format!(
        "The portfolio score is the compliance score. {}",
        scoring::COMPLIANCE_SCORE_LINE
    )
}

/// The first paragraph under the heading `title`.
fn paragraph_under<'a>(doc: &'a ReportDoc, title: &str) -> &'a str {
    doc.blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == title))
        .find_map(|b| match b {
            Block::Paragraph(p) => Some(p.as_str()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no paragraph under {title}"))
}

/// The value of the KPI card `label` and the line under it.
fn kpi(doc: &ReportDoc, label: &str) -> (String, Option<String>) {
    doc.blocks
        .iter()
        .find_map(|b| match b {
            Block::Kpis(cards) => cards.iter().find(|card| card.0 == label),
            _ => None,
        })
        .map(|card| (card.1.clone(), card.2.clone()))
        .unwrap_or_else(|| panic!("no KPI card {label}"))
}

/// A store with one run of `name` on the target `queue`, which ended with
/// `status` at `BASE_NS`.
fn one_run(name: &str, status: &str) -> (tempfile::TempDir, tumult_lake::Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    store
        .writer()
        .unwrap()
        .insert_spans(&[tumult_lake::SpanRow {
            ts_ns: BASE_NS,
            trace_id: "trace-one".into(),
            span_id: "span-one".into(),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: HOUR_NS,
            service_name: "tumult".into(),
            experiment_id: Some("exp-one".into()),
            experiment_name: Some(name.into()),
            outcome_status: Some(status.into()),
            target_system: Some("queue".into()),
            events: "[]".into(),
            ..Default::default()
        }])
        .unwrap();
    (d, store)
}

/// The digest of the day up to an hour after `BASE_NS`.
fn digest_of_the_day(store: &tumult_lake::Store) -> ReportDoc {
    let reader = store.read_only().unwrap();
    let as_of = BASE_NS + HOUR_NS;
    build_executive(&reader, &OrgTree::empty(), as_of, DAY_NS, as_of, &[]).unwrap()
}

#[test]
fn a_change_of_half_a_point_or_less_is_flat() {
    for (delta, text) in [
        (Some(12.0), " and improving (+12.0 vs the previous period)."),
        (Some(0.6), " and improving (+0.6 vs the previous period)."),
        (Some(0.5), " and flat (+0.5 vs the previous period)."),
        (Some(-0.5), " and flat (-0.5 vs the previous period)."),
        (Some(-0.6), " and declining (-0.6 vs the previous period)."),
        (None, ". There is no earlier period to compare it with."),
    ] {
        assert_eq!(direction(delta), text);
    }
}

#[test]
fn the_bottom_line_names_three_weakest_targets_at_most() {
    let target = |name: &str, score: f64| scoring::TargetScore {
        target: name.into(),
        score,
        band: scoring::band(score).into(),
        runs: 1,
        last_run_ns: None,
    };
    let card = Scorecard {
        portfolio: Some(60.0),
        band: "fair".into(),
        delta: None,
        as_of_ns: BASE_NS,
        targets: vec![
            target("a", 0.0),
            target("b", 50.0),
            target("c", 75.0),
            target("d", 75.0),
            target("e", 100.0),
        ],
        experiments: vec![],
    };

    assert_eq!(
        weakest_targets(&card),
        "Weakest targets: a (0.0), b (50.0), c (75.0)."
    );
}

#[test]
fn a_count_of_one_takes_the_name_for_one() {
    assert_eq!(counted(0, "run", "runs"), "0 runs");
    assert_eq!(counted(1, "run", "runs"), "1 run");
    assert_eq!(counted(2, "run", "runs"), "2 runs");
}

#[test]
fn a_digest_of_one_deviation_says_it_in_the_singular() {
    let (_d, store) = one_run("lone-exp", "deviated");

    let doc = digest_of_the_day(&store);

    assert_eq!(
        bottom_line(&doc),
        "Portfolio compliance score is fair (50.0/100). There is no earlier period to compare \
         it with. Weakest targets: queue (50.0). 1 open weakness needs a decision: re-run, \
         remediate, or accept."
    );
    let issues = paragraph_under(&doc, "Issues discovered and fixed");
    assert!(
        issues.contains(", 1 experiment run deviated or failed; 0 were later fixed (a later"),
        "{issues}"
    );
}

#[test]
fn a_digest_of_a_green_day_names_no_weakest_target_and_no_deviation() {
    let (_d, store) = one_run("green-exp", "completed");

    let doc = digest_of_the_day(&store);

    assert_eq!(
        bottom_line(&doc),
        "Portfolio compliance score is good (100.0/100). There is no earlier period to compare \
         it with. Every target scores 100. No open weaknesses; keep the current experiment \
         cadence."
    );
    assert_eq!(under_the_kpis(&doc), compliance_score_line());
    assert_eq!(kpi(&doc, "Δ vs previous period").0, "—");
    let issues = paragraph_under(&doc, "Issues discovered and fixed");
    assert!(
        issues.ends_with(", no experiment run deviated or failed."),
        "{issues}"
    );
    assert_eq!(
        paragraph_under(&doc, "Outlook"),
        "Coverage is green across the board; maintain cadence and consider widening fault \
         scenarios."
    );
}

#[test]
fn a_digest_of_an_empty_store_claims_no_score() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();

    let doc = digest_of_the_day(&store);

    assert_eq!(
        bottom_line(&doc),
        "No experiment is on record, so there is no compliance score yet."
    );
    assert_eq!(kpi(&doc, "Portfolio score"), ("—".to_string(), None));
    assert_eq!(under_the_kpis(&doc), compliance_score_line());
    assert_eq!(kpi(&doc, "Δ vs previous period").0, "—");
    assert_eq!(
        paragraph_under(&doc, "Score trend"),
        "No score on record in the period."
    );
    assert!(
        !doc.blocks
            .iter()
            .any(|b| matches!(b, Block::Chart(ChartSpec::Trend { .. }))),
        "a trend with no point is not drawn"
    );
    assert_eq!(
        paragraph_under(&doc, "Outlook"),
        "Next period priorities: run a first experiment to put a score on record."
    );
    // No score is not a score of 0: in no block, and so in neither output.
    let blocks = format!("{:?}", doc.blocks);
    assert!(!blocks.contains("0.0"), "{blocks}");
    assert!(!crate::html::render_html(&doc).contains("0.0"));
}

#[test]
fn the_digest_counts_the_experiments_tested_in_its_period() {
    let (_d, store) = findings_fixture();
    let reader = store.read_only().unwrap();
    let tested = |as_of: i64| {
        let doc =
            build_executive(&reader, &OrgTree::empty(), as_of, 7 * DAY_NS, as_of, &[]).unwrap();
        kpi(&doc, "Experiments tested")
    };
    let in_the_period = Some("in the period".to_string());

    assert_eq!(
        tested(BASE_NS + 6 * HOUR_NS),
        ("2 / 2".to_string(), in_the_period.clone())
    );
    // A month on both are still on record, and neither ran in the week.
    assert_eq!(
        tested(BASE_NS + 6 * HOUR_NS + 30 * DAY_NS),
        ("0 / 2".to_string(), in_the_period)
    );
}

#[test]
fn executive_digest_reports_declining_trend_open_weaknesses_and_mttr() {
    let (_d, store) = findings_fixture();
    let reader = store.read_only().unwrap();
    let org = OrgTree::empty();

    // Previous period (a week back): flaky-exp's latest run is the pass at
    // BASE_NS + 1h (score 100). Now: the deviation at +2h (score 50) —
    // a declining delta, one open weakness, one discovered-and-fixed issue.
    let doc = build_executive(
        &reader,
        &org,
        BASE_NS + 6 * HOUR_NS,
        7 * DAY_NS,
        BASE_NS + 6 * HOUR_NS,
        &[],
    )
    .unwrap();

    let bluf = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Paragraph(p) if p.starts_with("Portfolio compliance score") => Some(p.clone()),
            _ => None,
        })
        .expect("bluf paragraph");
    assert_eq!(
        bluf,
        "Portfolio compliance score is fair (62.5/100) and declining (-37.5 vs the previous \
         period). Weakest targets: database (50.0), cdn (75.0). 2 open weaknesses need a \
         decision: re-run, remediate, or accept."
    );
    let issues = paragraph_under(&doc, "Issues discovered and fixed");
    assert!(
        issues.contains(", 2 experiment runs deviated or failed; 1 was later fixed (a later"),
        "{issues}"
    );
    let footnote = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Footnote(text) => Some(text.as_str()),
            _ => None,
        })
        .expect("footnote");
    assert!(
        footnote.contains("Evidence mix: 1 automated, 1 verified manual experiment (manual"),
        "{footnote}"
    );

    // KPIs: 1 of 2 issues fixed, MTTR from the two recovery_time_s spans.
    let kpis = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Kpis(k) => Some(k.clone()),
            _ => None,
        })
        .expect("kpi cards");
    let fixed = kpis
        .iter()
        .find(|(label, _, _)| label == "Issues fixed")
        .unwrap();
    assert_eq!(fixed.1, "1 / 2");
    assert_eq!(fixed.2.as_deref(), Some("MTTR 21.0s"));

    // Open weaknesses table carries the decision per run state.
    let open = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Open weaknesses and decisions needed"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("open weaknesses table");
    let texts: Vec<String> = open.iter().map(|r| row_text(r)).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("flaky-exp") && t.contains("Re-run, remediate, or accept")),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("drill-exp") && t.contains("Re-run to a full pass, or accept")),
        "{texts:?}"
    );

    // Outlook focuses the next game-day on the weakest target.
    let outlook = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Paragraph(p) if p.starts_with("Next period priorities") => Some(p.clone()),
            _ => None,
        })
        .expect("outlook paragraph");
    assert!(outlook.contains("weakest target"), "{outlook}");

    // The HTML preview renders the whole document (charts included).
    let html = crate::html::render_html(&doc);
    assert!(html.contains("Executive resilience digest"));
    assert!(html.contains("<svg"));
}

#[test]
fn executive_digest_all_green_has_no_weaknesses() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    store
        .writer()
        .unwrap()
        .insert_spans(&[tumult_lake::SpanRow {
            ts_ns: BASE_NS,
            trace_id: "trace-green".into(),
            span_id: "span-green".into(),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: HOUR_NS,
            service_name: "tumult".into(),
            experiment_id: Some("exp-green".into()),
            experiment_name: Some("green-exp".into()),
            target_system: Some("queue".into()),
            events: "[]".into(),
            ..Default::default()
        }])
        .unwrap();
    store
        .writer()
        .unwrap()
        .insert_logs(&[tumult_lake::LogRow {
            ts_ns: BASE_NS + 60_000_000_000,
            severity_text: "INFO".into(),
            body: "experiment.completed".into(),
            trace_id: Some("trace-green".into()),
            span_id: None,
            service_name: "tumult".into(),
            log_attrs: vec![
                ("experiment_id".to_string(), "exp-green".to_string()),
                ("status".to_string(), "Completed".to_string()),
            ],
            resource_attrs: vec![],
            ..tumult_lake::LogRow::default()
        }])
        .unwrap();
    let reader = store.read_only().unwrap();
    let doc = build_executive(
        &reader,
        &OrgTree::empty(),
        BASE_NS + HOUR_NS,
        DAY_NS,
        BASE_NS + HOUR_NS,
        &[],
    )
    .unwrap();
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "No open weaknesses: every known experiment last ran green."
    )));
    let bluf = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Paragraph(p) if p.starts_with("Portfolio compliance score") => Some(p.clone()),
            _ => None,
        })
        .expect("bluf paragraph");
    assert!(bluf.contains("No open weaknesses"), "{bluf}");
}

/// A full game-day run: fault context on the root span, a rollback span in
/// the trace, WARN/ERROR findings logs, and started/completed logs.
fn game_day_fixture() -> (tempfile::TempDir, tumult_lake::Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[
            tumult_lake::SpanRow {
                ts_ns: BASE_NS,
                trace_id: "trace-gd".into(),
                span_id: "span-gd-root".into(),
                span_name: "resilience.experiment".into(),
                span_kind: "Internal".into(),
                duration_ns: 10 * 60 * NS_MIN,
                service_name: "tumult".into(),
                experiment_id: Some("exp-gd".into()),
                experiment_name: Some("pg-failover-drill".into()),
                fault_type: Some("injection".into()),
                fault_subtype: Some("process-kill".into()),
                fault_severity: Some("high".into()),
                blast_radius: Some("single-node".into()),
                target_system: Some("postgres".into()),
                target_environment: Some("staging".into()),
                hypothesis_met: Some(true),
                recovery_time_s: Some(42.5),
                span_attrs: vec![("fault.args.signal".into(), "9".into())],
                events: "[]".into(),
                ..Default::default()
            },
            tumult_lake::SpanRow {
                ts_ns: BASE_NS + NS_MIN,
                trace_id: "trace-gd".into(),
                span_id: "span-gd-rb".into(),
                parent_span_id: Some("span-gd-root".into()),
                span_name: "resilience.rollback".into(),
                span_kind: "Internal".into(),
                duration_ns: NS_MIN,
                status_code: "Ok".into(),
                service_name: "tumult".into(),
                events: "[]".into(),
                ..Default::default()
            },
        ])
        .unwrap();
    writer
        .insert_logs(&[
            tumult_lake::LogRow {
                ts_ns: BASE_NS,
                severity_text: "INFO".into(),
                body: "experiment.started".into(),
                trace_id: Some("trace-gd".into()),
                span_id: None,
                service_name: "tumult".into(),
                log_attrs: vec![
                    ("experiment_id".to_string(), "exp-gd".to_string()),
                    ("title".to_string(), "pg-failover-drill".to_string()),
                ],
                resource_attrs: vec![],
                ..tumult_lake::LogRow::default()
            },
            tumult_lake::LogRow {
                ts_ns: BASE_NS + 2 * NS_MIN,
                severity_text: "WARN".into(),
                body: "replica lag climbing".into(),
                trace_id: Some("trace-gd".into()),
                span_id: None,
                service_name: "tumult".into(),
                log_attrs: vec![],
                resource_attrs: vec![],
                ..tumult_lake::LogRow::default()
            },
            tumult_lake::LogRow {
                ts_ns: BASE_NS + 3 * NS_MIN,
                severity_text: "ERROR".into(),
                body: "primary unreachable".into(),
                trace_id: Some("trace-gd".into()),
                span_id: None,
                service_name: "tumult".into(),
                log_attrs: vec![],
                resource_attrs: vec![],
                ..tumult_lake::LogRow::default()
            },
            tumult_lake::LogRow {
                ts_ns: BASE_NS + 9 * NS_MIN,
                severity_text: "INFO".into(),
                body: "experiment.completed".into(),
                trace_id: Some("trace-gd".into()),
                span_id: None,
                service_name: "tumult".into(),
                log_attrs: vec![
                    ("experiment_id".to_string(), "exp-gd".to_string()),
                    ("status".to_string(), "Completed".to_string()),
                ],
                resource_attrs: vec![],
                ..tumult_lake::LogRow::default()
            },
        ])
        .unwrap();
    (d, store)
}

const NS_MIN: i64 = 60 * 1_000_000_000;

#[test]
fn game_day_renders_full_run_context() {
    let (_d, store) = game_day_fixture();
    let reader = store.read_only().unwrap();
    let doc = build_game_day(&reader, "exp-gd", BASE_NS + 10 * NS_MIN, &[])
        .unwrap()
        .expect("run exists");
    assert_eq!(doc.meta.experiment_id.as_deref(), Some("exp-gd"));

    let summary = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::KeyValues(kvs) => Some(kvs.clone()),
            _ => None,
        })
        .expect("run summary");
    let get = |key: &str| {
        summary
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| row_text(std::slice::from_ref(v)))
            .unwrap()
    };
    assert_eq!(get("Scenario"), "injection / process-kill");
    assert_eq!(get("Severity"), "high");
    assert_eq!(get("Hypothesis"), "Met");
    // The completed log of a stored run has the old spelling, `Completed`.
    assert_eq!(get("Outcome"), "completed");

    // Blast radius section: rollback exercised, recovery recorded.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::KeyValues(kvs) if kvs.iter().any(|(k, v)|
            k == "Rollback exercised" && row_text(std::slice::from_ref(v)) == "yes")
            && kvs.iter().any(|(k, v)|
            k == "Recovery time" && row_text(std::slice::from_ref(v)) == "42.5s")
    )));

    // Timeline: both spans, the rollback row rendering its Ok status.
    let timeline = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Timeline"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("timeline table");
    assert_eq!(timeline.len(), 2);
    assert!(row_text(&timeline[1]).contains("ok"));

    // Findings: WARN and ERROR rows become bullets.
    let findings = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Findings"))
        .find_map(|b| match b {
            Block::Bullets(items) => Some(items.clone()),
            _ => None,
        })
        .expect("findings bullets");
    assert_eq!(findings.len(), 2);
    assert!(findings[0].contains("[WARN] replica lag climbing"));

    // The rollback of this run has no step under it.
    assert_eq!(
        paragraph_under(&doc, "Rollback"),
        "The rollback ran; status: all clean."
    );

    // Config appendix merges root span attrs and started-log attrs.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::KeyValues(kvs) if kvs.iter().any(|(k, _)| k == "fault.args.signal")
            && kvs.iter().any(|(k, _)| k == "start.title")
    )));

    // Verdict mentions the recovery time.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p.contains("Service recovered in 42.5s.")
    )));

    // Both renderers accept the full document.
    let html = crate::html::render_html(&doc);
    assert!(html.contains("pg-failover-drill"));
    // The cover names the id as what it is, and its title line has no id.
    assert!(
        html.contains("<tr><th>Run id</th><td>exp-gd</td></tr>"),
        "{html}"
    );
    assert!(
        html.contains("<div class='cover-sub'>Game-Day Experiment Report</div>"),
        "{html}"
    );
    let (typst, _) = crate::markup::doc_to_typst(&doc);
    assert!(
        typst.contains("[#smallcaps[Run id]]], [#text(size: 10pt)[exp\\-gd]],"),
        "{typst}"
    );
    let pdf = crate::typst_pdf::render_pdf(&doc).expect("pdf render");
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn the_run_report_says_how_a_run_ended_in_the_spelling_of_the_journal() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    store
        .writer()
        .unwrap()
        .insert_spans(&[tumult_lake::SpanRow {
            ts_ns: BASE_NS,
            trace_id: "trace-h".into(),
            span_id: "span-h-root".into(),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: NS_MIN,
            service_name: "tumult".into(),
            experiment_id: Some("exp-h".into()),
            experiment_name: Some("halted-run".into()),
            outcome_status: Some("Halted".into()),
            events: "[]".into(),
            ..Default::default()
        }])
        .unwrap();

    let doc = build_game_day(&store.read_only().unwrap(), "exp-h", BASE_NS + NS_MIN, &[])
        .unwrap()
        .expect("run exists");

    assert_eq!(value_of(&doc, "Outcome"), "halted");
}

#[test]
fn game_day_handles_missing_outcome_and_failed_rollback() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    store
        .writer()
        .unwrap()
        .insert_spans(&[
            tumult_lake::SpanRow {
                ts_ns: BASE_NS,
                trace_id: "trace-x".into(),
                span_id: "span-x-root".into(),
                span_name: "resilience.experiment".into(),
                span_kind: "Internal".into(),
                duration_ns: NS_MIN,
                service_name: "tumult".into(),
                experiment_id: Some("exp-x".into()),
                experiment_name: Some("half-run".into()),
                fault_type: Some("network".into()),
                events: "[]".into(),
                ..Default::default()
            },
            tumult_lake::SpanRow {
                ts_ns: BASE_NS + 1_000_000_000,
                trace_id: "trace-x".into(),
                span_id: "span-x-rb".into(),
                span_name: "resilience.rollback".into(),
                span_kind: "Client".into(),
                duration_ns: 1_000_000_000,
                status_code: "Error".into(),
                status_message: "boom".into(),
                service_name: "tumult".into(),
                events: "[]".into(),
                ..Default::default()
            },
        ])
        .unwrap();
    let reader = store.read_only().unwrap();
    let doc = build_game_day(&reader, "exp-x", BASE_NS + NS_MIN, &[])
        .unwrap()
        .expect("run exists");

    let summary = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::KeyValues(kvs) => Some(kvs.clone()),
            _ => None,
        })
        .expect("run summary");
    let get = |key: &str| {
        summary
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| row_text(std::slice::from_ref(v)))
            .unwrap()
    };
    // No outcome on the root span and no completion log: incomplete
    // outcome; no hypothesis and no severity recorded; fault type without a
    // subtype stands alone.
    assert_eq!(get("Outcome"), "incomplete");
    assert_eq!(get("Hypothesis"), "Not recorded");
    assert_eq!(get("Scenario"), "network");
    assert_eq!(get("Severity"), "not recorded");

    // A failed rollback is flagged for investigation.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p.contains("errors present — investigate")
    )));
    // The failed rollback is a finding, though the run logged nothing.
    assert_eq!(
        bullets_under(&doc, "Findings"),
        ["[ERROR] resilience.rollback: boom"]
    );
    // The error span's timeline row renders the readable status.
    let timeline = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Timeline"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("timeline table");
    assert!(row_text(&timeline[1]).contains("error"));
}

#[test]
fn game_day_scoped_hides_out_of_scope_run() {
    let (_d, store) = env_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + DAY_NS;

    // Out-of-scope run: hidden (Ok(None) → the API maps it to 404).
    let hidden = build_game_day(&reader, "exp-prd", now, &["staging".to_string()]).unwrap();
    assert!(hidden.is_none());
    // In-scope and unscoped render normally.
    assert!(
        build_game_day(&reader, "exp-prd", now, &["prod".to_string()])
            .unwrap()
            .is_some()
    );
    assert!(build_game_day(&reader, "exp-prd", now, &[])
        .unwrap()
        .is_some());
}

#[test]
fn evidence_pack_does_not_assert_unverified_independence_or_clause_coverage() {
    let (_dir, store) = gated_fixture();
    let doc = build_evidence_pack(
        &store.read_only().unwrap(),
        "dora",
        None,
        BASE_NS + 2 * HOUR_NS,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();
    let text = format!("{doc:?}");
    assert!(!text.contains("Testing performed in line with"));
    // No record of this lake names a tester: none counts as external.
    assert!(text.contains("Independence verified: 0 of "), "{text}");
    assert!(text.contains("do not prove organizational independence"));
    // No function binds the runs of this lake: no row of the DORA matrix
    // claims evidence.
    let matrix = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Traceability matrix"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("traceability matrix");
    assert!(
        matrix
            .iter()
            .all(|row| row[2] == Cell::text("not applicable")),
        "{matrix:?}"
    );
}

#[test]
fn a_fixed_issue_is_counted_from_runs_stored_in_either_spelling() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let root = |id: &str, status: Option<&str>, ts: i64| tumult_lake::SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}"),
        span_name: "resilience.experiment".into(),
        experiment_id: Some(id.into()),
        experiment_name: Some("failover".into()),
        outcome_status: status.map(str::to_string),
        events: "[]".into(),
        ..Default::default()
    };
    let done = |id: &str, status: &str| tumult_lake::LogRow {
        ts_ns: BASE_NS,
        body: "experiment.completed".into(),
        log_attrs: vec![
            ("experiment_id".to_string(), id.to_string()),
            ("status".to_string(), status.to_string()),
        ],
        ..tumult_lake::LogRow::default()
    };
    let writer = store.writer().unwrap();
    // Two runs stored before 3.0.2, with the status on the completed log,
    // and two stored since, with the status on the root span.
    writer
        .insert_spans(&[
            root("old-deviated", None, BASE_NS),
            root("old-completed", None, BASE_NS + HOUR_NS),
            root("new-deviated", Some("deviated"), BASE_NS + 2 * HOUR_NS),
            root("new-completed", Some("completed"), BASE_NS + 3 * HOUR_NS),
        ])
        .unwrap();
    writer
        .insert_logs(&[
            done("old-deviated", "Deviated"),
            done("old-completed", "Completed"),
        ])
        .unwrap();

    let (discovered, fixed, _) =
        issue_stats(&store.read_only().unwrap(), BASE_NS, BASE_NS + DAY_NS, &[]).unwrap();

    assert_eq!((discovered, fixed), (2, 2));
}

#[test]
fn a_run_that_proves_nothing_is_no_issue_and_a_run_logged_twice_is_one() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let root = |id: &str, ts: i64| tumult_lake::SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}"),
        span_name: "resilience.experiment".into(),
        experiment_id: Some(id.into()),
        experiment_name: Some("failover".into()),
        events: "[]".into(),
        ..Default::default()
    };
    let done = |id: &str, status: &str, ts: i64| tumult_lake::LogRow {
        ts_ns: ts,
        body: "experiment.completed".into(),
        log_attrs: vec![
            ("experiment_id".to_string(), id.to_string()),
            ("status".to_string(), status.to_string()),
        ],
        ..tumult_lake::LogRow::default()
    };
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[
            root("aborted", BASE_NS),
            root("interrupted", BASE_NS + HOUR_NS),
            root("deviated", BASE_NS + 2 * HOUR_NS),
        ])
        .unwrap();
    // The deviated run left its completion log twice.
    writer
        .insert_logs(&[
            done("aborted", "Aborted", BASE_NS),
            done("interrupted", "Interrupted", BASE_NS + HOUR_NS),
            done("deviated", "Deviated", BASE_NS + 2 * HOUR_NS),
            done("deviated", "Deviated", BASE_NS + 2 * HOUR_NS + 1),
        ])
        .unwrap();

    let (discovered, fixed, _) =
        issue_stats(&store.read_only().unwrap(), BASE_NS, BASE_NS + DAY_NS, &[]).unwrap();

    assert_eq!((discovered, fixed), (1, 0));
}

#[test]
fn a_run_that_sent_no_trace_is_an_issue_like_a_run_that_did() {
    use tumult_core::types::ExperimentStatus;
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[tumult_lake::SpanRow {
            ts_ns: BASE_NS,
            trace_id: "trace-traced".into(),
            span_id: "span-traced".into(),
            span_name: "resilience.experiment".into(),
            experiment_id: Some("traced".into()),
            experiment_name: Some("failover".into()),
            outcome_status: Some("deviated".into()),
            events: "[]".into(),
            ..Default::default()
        }])
        .unwrap();
    // A plain `tumult run` of the same experiment an hour later: the lake
    // holds its journal and no span.
    let journal = tumult_core::types::Journal {
        experiment_title: "failover".into(),
        started_at_ns: BASE_NS + HOUR_NS,
        ended_at_ns: BASE_NS + HOUR_NS + 300_000_000_000,
        ..tumult_lake::duckdb_store::sample_journal("journal-only", ExperimentStatus::Deviated)
    };
    assert!(writer.ingest_journal(&journal, None).unwrap());

    let (discovered, fixed, _) =
        issue_stats(&store.read_only().unwrap(), BASE_NS, BASE_NS + DAY_NS, &[]).unwrap();

    assert_eq!((discovered, fixed), (2, 0));
}

#[test]
fn the_mean_time_to_recover_is_of_the_runs_with_or_without_a_trace_and_not_of_inner_spans() {
    use tumult_core::types::{ExperimentStatus, ObjectiveOutcome};
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    // A traced run that recovered in 20 s, and a span inside its trace
    // that carries a recovery time of its own.
    let root = tumult_lake::SpanRow {
        ts_ns: BASE_NS,
        trace_id: "trace-traced".into(),
        span_id: "span-traced".into(),
        span_name: "resilience.experiment".into(),
        experiment_id: Some("traced".into()),
        experiment_name: Some("failover".into()),
        outcome_status: Some("completed".into()),
        recovery_time_s: Some(20.0),
        events: "[]".into(),
        ..Default::default()
    };
    let inner = tumult_lake::SpanRow {
        ts_ns: BASE_NS + 1,
        span_id: "span-inner".into(),
        parent_span_id: Some("span-traced".into()),
        span_name: "resilience.action".into(),
        experiment_id: None,
        experiment_name: None,
        recovery_time_s: Some(80.0),
        ..root.clone()
    };
    // Two runs outside the window: one that started a moment before it,
    // and one that started as it ended.
    let outside = |id: &str, ts_ns: i64| tumult_lake::SpanRow {
        ts_ns,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}"),
        experiment_id: Some(id.into()),
        recovery_time_s: Some(500.0),
        ..root.clone()
    };
    let before = outside("before", BASE_NS - 1);
    let after = outside("after", BASE_NS + DAY_NS);
    writer.insert_spans(&[root, inner, before, after]).unwrap();
    // A plain `tumult run` an hour later: the lake holds its journal, which
    // measured a recovery time of 40 s, and no span.
    let journal = tumult_core::types::Journal {
        experiment_title: "failover".into(),
        started_at_ns: BASE_NS + HOUR_NS,
        ended_at_ns: BASE_NS + HOUR_NS + 300_000_000_000,
        objectives: Some(ObjectiveOutcome {
            rto_target_s: Some(300.0),
            rto_measured_s: Some(40.0),
            ..ObjectiveOutcome::default()
        }),
        ..tumult_lake::duckdb_store::sample_journal("journal-only", ExperimentStatus::Completed)
    };
    assert!(writer.ingest_journal(&journal, None).unwrap());

    let (_, _, mttr) =
        issue_stats(&store.read_only().unwrap(), BASE_NS, BASE_NS + DAY_NS, &[]).unwrap();

    assert_eq!(mttr, Some(30.0));
}

/// The bullets under the heading `title`.
fn bullets_under<'a>(doc: &'a ReportDoc, title: &str) -> &'a [String] {
    doc.blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == title))
        .find_map(|b| match b {
            Block::Bullets(items) => Some(items.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no bullets under {title}"))
}

/// The value of `key` in the first block of keys and values that has it.
fn value_of(doc: &ReportDoc, key: &str) -> String {
    doc.blocks
        .iter()
        .filter_map(|b| match b {
            Block::KeyValues(pairs) => Some(pairs),
            _ => None,
        })
        .flatten()
        .find(|(name, _)| name == key)
        .map_or_else(
            || panic!("the report has no {key}"),
            |(_, value)| row_text(std::slice::from_ref(value)),
        )
}

/// The headers of the timeline of a run report, and its rows as text.
fn timeline_of(doc: &ReportDoc) -> (Vec<String>, Vec<String>) {
    doc.blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Timeline"))
        .find_map(|b| match b {
            Block::Table { headers, rows, .. } => Some((
                headers.clone(),
                rows.iter().map(|row| row_text(row)).collect(),
            )),
            _ => None,
        })
        .expect("timeline table")
}

const OUTSIDE: &str = "the output is outside the tolerance";

/// One span of the run `exp-steps`: it started `at_ms` after the run did,
/// took `took_ms`, and ended in `error` when one is given. A span without a
/// parent is the root span of the run.
fn span_of(
    id: &str,
    parent: Option<&str>,
    name: &str,
    step: Option<&str>,
    at_ms: i64,
    took_ms: i64,
    error: Option<&str>,
) -> tumult_lake::SpanRow {
    tumult_lake::SpanRow {
        ts_ns: BASE_NS + at_ms * 1_000_000,
        trace_id: "trace-steps".into(),
        span_id: id.into(),
        parent_span_id: parent.map(Into::into),
        span_name: name.into(),
        span_kind: "Internal".into(),
        duration_ns: took_ms * 1_000_000,
        status_code: if error.is_some() { "Error" } else { "Unset" }.into(),
        status_message: error.unwrap_or_default().into(),
        service_name: "tumult".into(),
        experiment_id: parent.is_none().then(|| "exp-steps".into()),
        experiment_name: parent.is_none().then(|| "ledger-slowdown".into()),
        span_attrs: step
            .map(|step| ("resilience.action.name".to_string(), step.to_string()))
            .into_iter()
            .collect(),
        events: "[]".into(),
        ..Default::default()
    }
}

/// The root span of the run `exp-steps`, which ended in `error` when one is
/// given.
fn run_span(error: Option<&str>) -> tumult_lake::SpanRow {
    span_of(
        "root",
        None,
        "resilience.experiment",
        None,
        0,
        40_000,
        error,
    )
}

/// A step of the method of the run `exp-steps` that acted on the target.
fn action(id: &str, step: &str, at_ms: i64) -> tumult_lake::SpanRow {
    span_of(
        id,
        Some("root"),
        "resilience.action",
        Some(step),
        at_ms,
        250,
        None,
    )
}

/// A line the run `exp-steps` logged.
fn logged(severity: &str, body: &str, attrs: &[(&str, &str)]) -> tumult_lake::LogRow {
    tumult_lake::LogRow {
        ts_ns: BASE_NS,
        severity_text: severity.into(),
        body: body.into(),
        trace_id: Some("trace-steps".into()),
        service_name: "tumult".into(),
        log_attrs: attrs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
        ..tumult_lake::LogRow::default()
    }
}

/// The report of the run `exp-steps`, from a store that holds `spans` and
/// `logs`.
fn report_of_steps(spans: &[tumult_lake::SpanRow], logs: &[tumult_lake::LogRow]) -> ReportDoc {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    writer.insert_spans(spans).unwrap();
    if !logs.is_empty() {
        writer.insert_logs(logs).unwrap();
    }
    report_in(&store)
}

/// The report of the run `exp-steps` as `store` has it now.
fn report_in(store: &tumult_lake::Store) -> ReportDoc {
    let reader = store.read_only().unwrap();
    build_game_day(&reader, "exp-steps", BASE_NS + HOUR_NS, &[])
        .unwrap()
        .expect("run exists")
}

/// A run that slowed a ledger, polled a probe three times while it was
/// slow, saw the probe fail twice after it, and rolled back in two steps.
fn slowed_ledger() -> Vec<tumult_lake::SpanRow> {
    let poll = |id: &str, at_ms: i64, took_ms: i64| {
        let step = Some("payments-answer");
        span_of(
            id,
            Some("root"),
            "resilience.probe",
            step,
            at_ms,
            took_ms,
            None,
        )
    };
    let failed = |id: &str, at_ms: i64| {
        let step = Some("payments-answer");
        span_of(
            id,
            Some("after"),
            "resilience.probe",
            step,
            at_ms,
            5,
            Some(OUTSIDE),
        )
    };
    let undo = |id: &str, step: &str, at_ms: i64| {
        span_of(
            id,
            Some("undo"),
            "resilience.action",
            Some(step),
            at_ms,
            100,
            None,
        )
    };
    vec![
        run_span(Some("deviated")),
        action("fault", "slow-ledger", 1_000),
        poll("poll-1", 2_000, 10),
        poll("poll-2", 3_000, 30),
        poll("poll-3", 4_000, 20),
        span_of(
            "after",
            Some("root"),
            "resilience.hypothesis.after",
            None,
            4_900,
            1_200,
            None,
        ),
        failed("after-1", 5_000),
        failed("after-2", 6_000),
        span_of(
            "undo",
            Some("root"),
            "resilience.rollback",
            None,
            7_000,
            900,
            None,
        ),
        undo("undo-1", "restore-ledger", 7_100),
        undo("undo-2", "flush-cache", 7_500),
    ]
}

#[test]
fn a_step_that_ran_several_times_in_a_row_is_one_line_of_the_timeline() {
    let doc = report_of_steps(&slowed_ledger(), &[]);

    let (headers, rows) = timeline_of(&doc);

    assert_eq!(
        headers,
        ["t+ (s)", "Span", "Step", "Duration (ms)", "Status"]
    );
    assert_eq!(
        rows,
        [
            "0.0|resilience.experiment|—|40000.0|error",
            "1.0|resilience.action|slow-ledger|250.0|—",
            "2.0 – 4.0|resilience.probe|payments-answer × 3|10.0 – 30.0|—",
            "4.9|resilience.hypothesis.after|—|1200.0|—",
            "5.0 – 6.0|resilience.probe|payments-answer × 2|5.0|error",
            "7.0|resilience.rollback|—|900.0|—",
            "7.1|resilience.action|restore-ledger|100.0|—",
            "7.5|resilience.action|flush-cache|100.0|—",
        ]
    );
}

#[test]
fn spans_that_name_no_step_are_a_line_each() {
    let phase = |id: &str, at_ms: i64| {
        span_of(
            id,
            Some("root"),
            "resilience.hypothesis.before",
            None,
            at_ms,
            10,
            None,
        )
    };
    let doc = report_of_steps(&[run_span(None), phase("one", 100), phase("two", 200)], &[]);

    let (_, rows) = timeline_of(&doc);

    assert_eq!(
        rows,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "0.1|resilience.hypothesis.before|—|10.0|—",
            "0.2|resilience.hypothesis.before|—|10.0|—",
        ]
    );
}

#[test]
fn the_findings_name_the_steps_that_failed_and_then_what_the_run_logged() {
    let logs = [
        logged("INFO", "experiment.started", &[]),
        logged("WARN", "the ledger is slow", &[]),
    ];

    let doc = report_of_steps(&slowed_ledger(), &logs);

    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[ERROR] payments-answer (resilience.probe): the output is outside the tolerance \
             (× 2)",
            "[WARN] the ledger is slow",
        ]
    );
}

#[test]
fn a_span_that_failed_without_a_word_is_still_a_finding() {
    let silent = span_of(
        "undo",
        Some("root"),
        "resilience.rollback",
        None,
        1_000,
        10,
        Some(""),
    );

    let doc = report_of_steps(&[run_span(None), silent], &[]);

    assert_eq!(
        bullets_under(&doc, "Findings"),
        ["[ERROR] resilience.rollback"]
    );
}

#[test]
fn how_the_run_ended_is_the_verdict_and_not_a_finding() {
    for ended in [None, Some("deviated")] {
        let doc = report_of_steps(
            &[run_span(ended), action("fault", "slow-ledger", 1_000)],
            &[],
        );

        assert_eq!(
            paragraph_under(&doc, "Findings"),
            "No step failed and no warnings or errors were logged during this run.",
            "{ended:?}"
        );
    }
}

#[test]
fn the_scenario_of_a_run_that_names_no_fault_is_what_its_method_did() {
    let mut spans = slowed_ledger();
    spans.push(action("fault-2", "drop-cache", 1_300));
    spans.push(action("fault-3", "slow-ledger", 1_600));

    let doc = report_of_steps(&spans, &[]);

    // Each fault once, in the order of the run. A probe is no fault, and
    // neither is a step of the rollback.
    assert_eq!(value_of(&doc, "Scenario"), "slow-ledger, drop-cache");
}

#[test]
fn a_run_that_did_nothing_to_its_target_has_no_scenario() {
    let probe = span_of(
        "probe",
        Some("root"),
        "resilience.probe",
        Some("ping"),
        1_000,
        10,
        None,
    );

    let doc = report_of_steps(&[run_span(None), probe], &[]);

    assert_eq!(value_of(&doc, "Scenario"), "—");
}

#[test]
fn the_rollback_names_the_steps_it_ran() {
    let rollback = |error| {
        span_of(
            "undo",
            Some("root"),
            "resilience.rollback",
            None,
            7_000,
            900,
            error,
        )
    };
    let restore = |error| {
        let step = Some("restore-ledger");
        span_of(
            "undo-1",
            Some("undo"),
            "resilience.action",
            step,
            7_100,
            100,
            error,
        )
    };

    let two_steps = report_of_steps(&slowed_ledger(), &[]);
    assert_eq!(
        paragraph_under(&two_steps, "Rollback"),
        "The rollback ran 2 steps (restore-ledger, flush-cache); status: all clean."
    );

    for (spans, said) in [
        (
            vec![run_span(None), rollback(None), restore(None)],
            "The rollback ran 1 step (restore-ledger); status: all clean.",
        ),
        (
            vec![run_span(None), rollback(None), restore(Some("exit 1"))],
            "The rollback ran 1 step (restore-ledger); status: errors present — investigate.",
        ),
        (
            vec![run_span(None), rollback(Some("timed out")), restore(None)],
            "The rollback ran 1 step (restore-ledger); status: errors present — investigate.",
        ),
        (
            vec![run_span(None), action("fault", "slow-ledger", 1_000)],
            "No rollback action was exercised in this run.",
        ),
    ] {
        let doc = report_of_steps(&spans, &[]);

        assert_eq!(paragraph_under(&doc, "Rollback"), said);
    }
}

#[test]
fn a_step_of_the_rollback_that_failed_is_a_finding_too() {
    let spans = [
        run_span(None),
        span_of(
            "undo",
            Some("root"),
            "resilience.rollback",
            None,
            7_000,
            900,
            None,
        ),
        span_of(
            "undo-1",
            Some("undo"),
            "resilience.action",
            Some("restore-ledger"),
            7_100,
            100,
            Some("exit 1"),
        ),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        bullets_under(&doc, "Findings"),
        ["[ERROR] restore-ledger (resilience.action): exit 1"]
    );
}

/// The run `exp-steps`, alone in its trace, slowed the ledger with a step
/// whose script put a span of the database of the ledger into the trace:
/// the database rolled back a transaction, and ended in `error` when one is
/// given.
fn rolled_back_by_the_database(error: Option<&str>) -> Vec<tumult_lake::SpanRow> {
    let database = tumult_lake::SpanRow {
        service_name: "ledger-db".into(),
        ..span_of(
            "db-rollback",
            Some("fault"),
            "ROLLBACK",
            None,
            1_100,
            2,
            error,
        )
    };
    vec![
        run_span(None),
        action("fault", "slow-ledger", 1_000),
        database,
    ]
}

#[test]
fn a_rollback_of_another_service_in_the_trace_is_no_rollback_of_the_run() {
    let doc = report_of_steps(&rolled_back_by_the_database(None), &[]);

    assert_eq!(value_of(&doc, "Rollback exercised"), "no");
    assert_eq!(
        paragraph_under(&doc, "Rollback"),
        "No rollback action was exercised in this run."
    );
    assert_eq!(value_of(&doc, "Scenario"), "slow-ledger");
}

#[test]
fn a_rollback_of_another_service_that_failed_is_no_rollback_with_errors() {
    let doc = report_of_steps(&rolled_back_by_the_database(Some("deadlock")), &[]);

    let said = paragraph_under(&doc, "Rollback");
    assert!(!said.contains("errors present"), "{said}");
    assert_eq!(said, "No rollback action was exercised in this run.");
    assert_eq!(value_of(&doc, "Rollback exercised"), "no");
}

#[test]
fn a_time_in_the_appendix_is_a_date_and_a_time_of_day() {
    let started_at = BASE_NS.to_string();
    let started = logged(
        "INFO",
        "experiment.started",
        &[
            ("experiment_id", "exp-steps"),
            ("started_at_ns", &started_at),
            ("planned_at_ns", "soon"),
            ("retries", "3"),
        ],
    );

    let doc = report_of_steps(&[run_span(None)], &[started]);

    assert_eq!(
        value_of(&doc, "start.started_at_ns"),
        "2026-07-25 17:20 UTC"
    );
    // What is not a time stays as the run wrote it.
    assert_eq!(value_of(&doc, "start.planned_at_ns"), "soon");
    assert_eq!(value_of(&doc, "start.retries"), "3");
}

/// The run `exp-steps` as one of the two runs of a game day. Both hang
/// under the span of the game day and share its trace. The other run
/// failed a step, logged an error and rolled back.
fn game_day_of_two() -> (Vec<tumult_lake::SpanRow>, Vec<tumult_lake::LogRow>) {
    let under_the_day = |span: tumult_lake::SpanRow| tumult_lake::SpanRow {
        parent_span_id: Some("day".into()),
        ..span
    };
    let day = tumult_lake::SpanRow {
        experiment_id: None,
        experiment_name: None,
        ..span_of("day", None, "resilience.gameday", None, 0, 90_000, None)
    };
    let other = tumult_lake::SpanRow {
        experiment_id: Some("exp-other".into()),
        experiment_name: Some("cache-flush".into()),
        ..span_of(
            "other",
            None,
            "resilience.experiment",
            None,
            50_000,
            30_000,
            Some("deviated"),
        )
    };
    let flush = Some("flush-cache");
    let warm = Some("warm-cache");
    let spans = vec![
        day,
        under_the_day(run_span(None)),
        action("fault", "slow-ledger", 1_000),
        under_the_day(other),
        span_of(
            "other-1",
            Some("other"),
            "resilience.action",
            flush,
            51_000,
            100,
            Some("exit 1"),
        ),
        span_of(
            "other-undo",
            Some("other"),
            "resilience.rollback",
            None,
            60_000,
            500,
            None,
        ),
        span_of(
            "other-undo-1",
            Some("other-undo"),
            "resilience.action",
            warm,
            60_100,
            100,
            None,
        ),
        // A span that lost its parent on the way: no run can claim it.
        span_of(
            "stray",
            Some("lost"),
            "resilience.action",
            Some("stray"),
            70_000,
            100,
            Some("exit 9"),
        ),
    ];
    let logs = vec![
        logged_on("fault", 1, "WARN", "the ledger is slow"),
        logged_on("other-1", 2, "ERROR", "the cache did not flush"),
        logged_on("lost", 3, "ERROR", "a line of a span that never arrived"),
    ];
    (spans, logs)
}

/// A line logged on the span `span` of the trace of `exp-steps`, `at_ms`
/// after the run started.
fn logged_on(span: &str, at_ms: i64, severity: &str, body: &str) -> tumult_lake::LogRow {
    tumult_lake::LogRow {
        ts_ns: BASE_NS + at_ms * 1_000_000,
        span_id: Some(span.into()),
        ..logged(severity, body, &[])
    }
}

#[test]
fn the_report_of_a_run_of_a_game_day_leaves_the_other_runs_out() {
    let (spans, logs) = game_day_of_two();

    let doc = report_of_steps(&spans, &logs);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "1.0|resilience.action|slow-ledger|250.0|—",
        ]
    );
    assert_eq!(value_of(&doc, "Scenario"), "slow-ledger");
    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[WARN] 1 span of this trace names a parent that is not on record. It and the spans \
             under it are not in this report.",
            "[WARN] the ledger is slow",
        ]
    );
    assert_eq!(paragraph_under(&doc, "Rollback"), NOT_KNOWN);
}

#[test]
fn a_run_of_a_game_day_is_not_alone_while_the_root_spans_of_the_others_are_on_their_way() {
    // A span is sent when it ends. While the second run of a game day runs,
    // its steps are in the store, and its root span and the span of the game
    // day are not.
    let (spans, logs) = game_day_of_two();
    let arrived = |span: &tumult_lake::SpanRow| span.span_id != "day" && span.span_id != "other";
    let spans: Vec<_> = spans.into_iter().filter(arrived).collect();

    let doc = report_of_steps(&spans, &logs);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "1.0|resilience.action|slow-ledger|250.0|—",
        ]
    );
    assert_eq!(value_of(&doc, "Scenario"), "slow-ledger");
    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[WARN] 3 spans of this trace name a parent that is not on record. They and the \
             spans under them are not in this report.",
            "[WARN] the ledger is slow",
        ]
    );
    assert_eq!(paragraph_under(&doc, "Rollback"), NOT_KNOWN);
    assert_eq!(value_of(&doc, "Rollback exercised"), "not known");
}

/// What a report says of the rollback of a run when it read no rollback
/// span and left spans of the trace out.
const NOT_KNOWN: &str = "This report left out spans of this trace, and the rollback may be \
                         among them, so it cannot say whether a rollback ran.";

#[test]
fn a_report_that_left_out_spans_of_the_trace_does_not_know_that_no_rollback_ran() {
    let (spans, logs) = game_day_of_two();

    let left_out = report_of_steps(&spans, &logs);

    assert_eq!(value_of(&left_out, "Rollback exercised"), "not known");
    assert_eq!(paragraph_under(&left_out, "Rollback"), NOT_KNOWN);

    let (mut spans, logs) = game_day_of_two();
    spans.extend([
        span_of(
            "undo",
            Some("root"),
            "resilience.rollback",
            None,
            7_000,
            900,
            None,
        ),
        span_of(
            "undo-1",
            Some("undo"),
            "resilience.action",
            Some("restore-ledger"),
            7_100,
            100,
            None,
        ),
    ]);

    let rolled_back = report_of_steps(&spans, &logs);

    assert_eq!(value_of(&rolled_back, "Rollback exercised"), "yes");
    assert_eq!(
        paragraph_under(&rolled_back, "Rollback"),
        "The rollback ran 1 step (restore-ledger); status: all clean."
    );

    let (mut spans, logs) = game_day_of_two();
    spans.retain(|span| span.span_id != "stray");

    let nothing_left_out = report_of_steps(&spans, &logs);

    assert_eq!(value_of(&nothing_left_out, "Rollback exercised"), "no");
    assert_eq!(
        paragraph_under(&nothing_left_out, "Rollback"),
        "No rollback action was exercised in this run."
    );
}

#[test]
fn a_run_alone_in_its_trace_keeps_what_lost_its_parent() {
    let mut spans = slowed_ledger();
    spans.push(span_of(
        "late",
        Some("lost"),
        "resilience.action",
        Some("restore-ledger"),
        8_000,
        100,
        Some("exit 1"),
    ));
    let logs = [logged_on("lost", 8_050, "WARN", "the ledger is still slow")];

    let doc = report_of_steps(&spans, &logs);

    assert_eq!(
        timeline_of(&doc).1.last().map(String::as_str),
        Some("8.0|resilience.action|restore-ledger|100.0|error")
    );
    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[ERROR] payments-answer (resilience.probe): the output is outside the tolerance \
             (× 2)",
            "[ERROR] restore-ledger (resilience.action): exit 1",
            "[WARN] the ledger is still slow",
        ]
    );
}

#[test]
fn a_step_that_carries_the_id_of_its_run_does_not_make_the_run_alone() {
    // A sender can put the id of the run on the resource. Then every span
    // of the run carries it, not only the span of the run.
    let (mut spans, logs) = game_day_of_two();
    let fault = spans.iter_mut().find(|span| span.span_id == "fault");
    fault.unwrap().experiment_id = Some("exp-steps".into());

    let doc = report_of_steps(&spans, &logs);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "1.0|resilience.action|slow-ledger|250.0|—",
        ]
    );
}

#[test]
fn a_run_under_the_span_of_a_tool_call_is_alone_in_its_trace() {
    // A run that a tool call started is under the span of the call. That
    // span is stored when the call is over.
    let call = tumult_lake::SpanRow {
        experiment_id: None,
        experiment_name: None,
        ..span_of("call", None, "mcp.tool.call", None, 0, 1, None)
    };
    let root = tumult_lake::SpanRow {
        parent_span_id: Some("call".into()),
        ..run_span(None)
    };
    let late = span_of(
        "late",
        Some("lost"),
        "resilience.action",
        Some("restore-ledger"),
        8_000,
        100,
        Some("exit 1"),
    );
    let logs = [logged_on("lost", 8_050, "WARN", "the ledger is still slow")];

    let doc = report_of_steps(&[call, root, late], &logs);

    assert_eq!(
        timeline_of(&doc).1.last().map(String::as_str),
        Some("8.0|resilience.action|restore-ledger|100.0|error")
    );
    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[ERROR] restore-ledger (resilience.action): exit 1",
            "[WARN] the ledger is still slow",
        ]
    );
}

#[test]
fn a_line_logged_anywhere_in_a_run_of_a_game_day_is_a_finding() {
    let (mut spans, mut logs) = game_day_of_two();
    let restore = Some("restore-ledger");
    spans.push(span_of(
        "undo",
        Some("root"),
        "resilience.rollback",
        None,
        7_000,
        900,
        None,
    ));
    spans.push(span_of(
        "undo-1",
        Some("undo"),
        "resilience.action",
        restore,
        7_100,
        100,
        None,
    ));
    logs.extend([
        // On a step of the rollback, two spans below the root span.
        logged_on("undo-1", 7_200, "WARN", "the ledger took long to restore"),
        // After the run, outside its trace, with the id of the run.
        tumult_lake::LogRow {
            ts_ns: BASE_NS + 41_000_000_000,
            trace_id: None,
            span_id: None,
            ..logged(
                "ERROR",
                "the journal was not written",
                &[("experiment_id", "exp-steps")],
            )
        },
    ]);

    let doc = report_of_steps(&spans, &logs);

    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[WARN] 1 span of this trace names a parent that is not on record. It and the spans \
             under it are not in this report.",
            "[WARN] the ledger is slow",
            "[WARN] the ledger took long to restore",
            "[ERROR] the journal was not written",
        ]
    );
    assert_eq!(
        paragraph_under(&doc, "Rollback"),
        "The rollback ran 1 step (restore-ledger); status: all clean."
    );
}

/// What the plugin of a step did inside it: a span under the span `step`.
fn inside(
    id: &str,
    step: &str,
    name: &str,
    at_ms: i64,
    error: Option<&str>,
) -> tumult_lake::SpanRow {
    span_of(id, Some(step), name, None, at_ms, 5, error)
}

#[test]
fn what_a_plugin_did_inside_a_step_is_no_line_of_the_timeline() {
    let poll = |id: &str, at_ms: i64| {
        let step = Some("payments-answer");
        span_of(id, Some("root"), "resilience.probe", step, at_ms, 10, None)
    };
    let spans = [
        run_span(None),
        poll("poll-1", 2_000),
        inside("run-1", "poll-1", "script.execute", 2_001, None),
        inside("call-1", "run-1", "http.request", 2_002, None),
        poll("poll-2", 3_000),
        inside("run-2", "poll-2", "script.execute", 3_001, None),
        poll("poll-3", 4_000),
        inside("run-3", "poll-3", "script.execute", 4_001, None),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "2.0 – 4.0|resilience.probe|payments-answer × 3|10.0|—",
        ]
    );
}

#[test]
fn spans_that_name_each_other_as_parent_do_not_hang_the_report() {
    let odd = |id: &str, parent: &str, at_ms: i64| {
        span_of(id, Some(parent), "script.execute", None, at_ms, 5, None)
    };
    let spans = [
        run_span(None),
        odd("a", "b", 500),
        odd("b", "a", 600),
        odd("alone", "alone", 700),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "0.5|script.execute|—|5.0|—",
            "0.6|script.execute|—|5.0|—",
            "0.7|script.execute|—|5.0|—",
        ]
    );
}

#[test]
fn a_root_span_under_its_own_step_does_not_hang_the_report() {
    let root = tumult_lake::SpanRow {
        parent_span_id: Some("fault".into()),
        ..run_span(None)
    };
    let spans = [root, action("fault", "slow-ledger", 1_000)];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(timeline_of(&doc).1, Vec::<String>::new());
    assert_eq!(value_of(&doc, "Scenario"), "slow-ledger");
}

#[test]
fn probes_sampled_in_turns_are_a_line_each() {
    let poll = |id: &str, step: &str, at_ms: i64, error: Option<&str>| {
        let name = "resilience.probe";
        span_of(id, Some("root"), name, Some(step), at_ms, 5, error)
    };
    let spans = [
        run_span(None),
        poll("latency-1", "latency", 2_000, None),
        poll("errors-1", "errors", 2_100, None),
        poll("latency-2", "latency", 3_000, None),
        poll("errors-2", "errors", 3_100, Some(OUTSIDE)),
        poll("latency-3", "latency", 4_000, None),
        poll("errors-3", "errors", 4_100, Some(OUTSIDE)),
        poll("latency-4", "latency", 5_000, None),
        poll("errors-4", "errors", 5_100, None),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "2.0 – 5.0|resilience.probe|latency × 4|5.0|—",
            "2.1|resilience.probe|errors|5.0|—",
            "3.1 – 4.1|resilience.probe|errors × 2|5.0|error",
            "5.1|resilience.probe|errors|5.0|—",
        ]
    );
}

#[test]
fn an_action_folds_only_into_the_line_right_before_it() {
    let spans = [
        run_span(None),
        action("first", "slow-ledger", 1_000),
        action("between", "restart-cache", 2_000),
        action("again", "slow-ledger", 3_000),
        action("once-more", "slow-ledger", 4_000),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "1.0|resilience.action|slow-ledger|250.0|—",
            "2.0|resilience.action|restart-cache|250.0|—",
            "3.0 – 4.0|resilience.action|slow-ledger × 2|250.0|—",
        ]
    );
}

#[test]
fn a_probe_before_and_after_an_action_is_a_line_each_time() {
    let probe = |id: &str, at_ms: i64| {
        let name = "resilience.probe";
        span_of(id, Some("root"), name, Some("answers"), at_ms, 5, None)
    };
    let spans = [
        run_span(None),
        probe("before", 100),
        action("fault", "slow-ledger", 1_000),
        probe("after-1", 2_000),
        probe("after-2", 3_000),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "0.1|resilience.probe|answers|5.0|—",
            "1.0|resilience.action|slow-ledger|250.0|—",
            "2.0 – 3.0|resilience.probe|answers × 2|5.0|—",
        ]
    );
}

#[test]
fn a_span_inside_a_step_that_failed_is_still_a_finding() {
    let spans = [
        run_span(None),
        action("fault", "slow-ledger", 1_000),
        inside("run-1", "fault", "script.execute", 1_001, Some("exit 7")),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(timeline_of(&doc).1.len(), 2);
    assert_eq!(
        bullets_under(&doc, "Findings"),
        ["[ERROR] script.execute: exit 7"]
    );
}

#[test]
fn a_step_that_ended_another_way_starts_a_new_line() {
    let poll = |id: &str, at_ms: i64, error: Option<&str>| {
        let step = Some("payments-answer");
        span_of(id, Some("root"), "resilience.probe", step, at_ms, 5, error)
    };
    let spans = [
        run_span(None),
        poll("poll-1", 2_000, None),
        poll("poll-2", 3_000, Some(OUTSIDE)),
        poll("poll-3", 4_000, Some(OUTSIDE)),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "2.0|resilience.probe|payments-answer|5.0|—",
            "3.0 – 4.0|resilience.probe|payments-answer × 2|5.0|error",
        ]
    );
}

#[test]
fn an_action_and_a_probe_of_one_name_are_a_line_each() {
    let spans = [
        run_span(None),
        action("act", "ledger", 1_000),
        span_of(
            "probe",
            Some("root"),
            "resilience.probe",
            Some("ledger"),
            2_000,
            250,
            None,
        ),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "1.0|resilience.action|ledger|250.0|—",
            "2.0|resilience.probe|ledger|250.0|—",
        ]
    );
}

#[test]
fn the_same_step_in_two_phases_is_a_line_for_each_phase() {
    let probe = |id: &str, parent: &str, at_ms: i64| {
        let step = Some("payments-answer");
        span_of(id, Some(parent), "resilience.probe", step, at_ms, 5, None)
    };
    let spans = [
        run_span(None),
        span_of(
            "before",
            Some("root"),
            "resilience.hypothesis.before",
            None,
            100,
            50,
            None,
        ),
        probe("before-1", "before", 110),
        probe("poll-1", "root", 200),
        probe("poll-2", "root", 300),
    ];

    let doc = report_of_steps(&spans, &[]);

    assert_eq!(
        timeline_of(&doc).1,
        [
            "0.0|resilience.experiment|—|40000.0|—",
            "0.1|resilience.hypothesis.before|—|50.0|—",
            "0.1|resilience.probe|payments-answer|5.0|—",
            "0.2 – 0.3|resilience.probe|payments-answer × 2|5.0|—",
        ]
    );
}

/// The run `exp-steps` with `polls` polls of one probe a millisecond
/// apart, and at its end a rollback that timed out.
fn polled(polls: usize) -> Vec<tumult_lake::SpanRow> {
    let poll = |n: i64| {
        let step = Some("payments-answer");
        span_of(
            &format!("poll-{n}"),
            Some("root"),
            "resilience.probe",
            step,
            1_000 + n,
            5,
            None,
        )
    };
    let mut spans = vec![run_span(None)];
    spans.extend((0..i64::try_from(polls).unwrap()).map(poll));
    spans.push(span_of(
        "undo",
        Some("root"),
        "resilience.rollback",
        None,
        39_000,
        900,
        Some("timed out"),
    ));
    spans
}

/// Store the polls `from` to `to` of a run that has its first poll: each
/// is that poll a millisecond after the one before. One statement, because
/// the writer takes most of a minute for 20,000 spans.
fn store_polls(writer: &tumult_lake::Writer, from: usize, to: usize) {
    let sql = format!(
        "INSERT INTO spans \
         SELECT first.* REPLACE ( \
             first.ts_ns + n * 1000000 AS ts_ns, 'poll-' || n AS span_id \
         ) \
         FROM spans AS first, range({from}, {to} + 1) AS more (n) \
         WHERE first.span_id = 'poll-0'"
    );
    writer.execute(&sql, []).unwrap();
}

#[test]
fn a_run_is_read_to_its_end_up_to_the_most_spans_and_says_so_beyond() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    // The root span, the polls and the rollback are the most spans a report reads.
    let polls = game_day::MOST_SPANS - 2;
    writer.insert_spans(&polled(1)).unwrap();
    store_polls(&writer, 1, polls - 1);

    let whole = report_in(&store);

    assert_eq!(
        timeline_of(&whole).1[1],
        "1.0 – 21.0|resilience.probe|payments-answer × 19998|5.0|—"
    );
    assert_eq!(
        bullets_under(&whole, "Findings"),
        ["[ERROR] resilience.rollback: timed out"]
    );
    assert_eq!(
        paragraph_under(&whole, "Rollback"),
        "The rollback ran; status: errors present — investigate."
    );
    assert_eq!(value_of(&whole, "Rollback exercised"), "yes");

    // One more poll before the rollback, and the rollback is not read.
    store_polls(&writer, polls, polls);

    let cut = report_in(&store);

    assert_eq!(
        bullets_under(&cut, "Findings"),
        [
            "[WARN] The run has more than 20000 spans. This report reads the first 20000, so it \
             may miss a step that failed later."
        ]
    );
    assert_eq!(
        paragraph_under(&cut, "Rollback"),
        "This report did not read the run to its end, so it cannot say whether a rollback ran."
    );
    assert_eq!(value_of(&cut, "Rollback exercised"), "not read");
}

#[test]
fn a_rollback_that_was_not_read_to_its_end_is_not_called_clean() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let undo = |id: &str, step: &str, at_ms: i64, error: Option<&str>| {
        let name = "resilience.action";
        span_of(id, Some("undo"), name, Some(step), at_ms, 100, error)
    };
    let mut spans = polled(1);
    spans.pop();
    spans.extend([
        span_of(
            "undo",
            Some("root"),
            "resilience.rollback",
            None,
            39_000,
            900,
            None,
        ),
        undo("undo-1", "restore-ledger", 39_100, None),
        undo("undo-2", "warm-cache", 39_200, Some("exit 1")),
    ]);
    writer.insert_spans(&spans).unwrap();
    // The root span, the polls, the rollback and its first step are the most
    // spans a report reads. The step that failed is the one beyond.
    store_polls(&writer, 1, game_day::MOST_SPANS - 4);

    let doc = report_in(&store);

    assert_eq!(
        paragraph_under(&doc, "Rollback"),
        "The rollback ran 1 step (restore-ledger); status: not read to its end."
    );
    assert_eq!(value_of(&doc, "Rollback exercised"), "yes");
}

#[test]
fn a_report_that_was_cut_short_and_left_spans_out_says_it_did_not_read_the_rollback() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let (mut spans, logs) = game_day_of_two();
    spans.push(span_of(
        "poll-0",
        Some("root"),
        "resilience.probe",
        Some("payments-answer"),
        1_000,
        5,
        None,
    ));
    writer.insert_spans(&spans).unwrap();
    writer.insert_logs(&logs).unwrap();
    store_polls(&writer, 1, game_day::MOST_SPANS);

    let doc = report_in(&store);

    assert_eq!(value_of(&doc, "Rollback exercised"), "not read");
    assert_eq!(
        paragraph_under(&doc, "Rollback"),
        "This report did not read the run to its end, so it cannot say whether a rollback ran."
    );
}

#[test]
fn a_run_that_sent_no_trace_counts_as_tested_in_the_period() {
    use tumult_core::types::ExperimentStatus;
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let journal = tumult_core::types::Journal {
        started_at_ns: BASE_NS,
        ended_at_ns: BASE_NS + 300_000_000_000,
        ..tumult_lake::duckdb_store::sample_journal("exp-plain", ExperimentStatus::Completed)
    };
    assert!(store
        .writer()
        .unwrap()
        .ingest_journal(&journal, None)
        .unwrap());

    let doc = digest_of_the_day(&store);

    assert_eq!(
        kpi(&doc, "Experiments tested"),
        ("1 / 1".to_string(), Some("in the period".to_string()))
    );
}

/// The rows of the first table under the heading `title`, as text.
fn rows_under(doc: &ReportDoc, title: &str) -> Vec<String> {
    doc.blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == title))
        .skip(1)
        .take_while(|b| !matches!(b, Block::H2(_)))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.iter().map(|row| row_text(row)).collect()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no table under {title}"))
}

/// The blocks between the heading `title` and the next heading.
fn blocks_under<'a>(doc: &'a ReportDoc, title: &str) -> Vec<&'a Block> {
    doc.blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == title))
        .skip(1)
        .take_while(|b| !matches!(b, Block::H2(_)))
        .collect()
}

const TWO_DOMAINS: &str = "
nodes:
  - {name: commerce, kind: domain}
  - {name: platform, kind: domain}
  - {name: checkout-team, parent: commerce}
  - {name: search-team, parent: platform}
assignments:
  - team: checkout-team
    targets: [\"checkout-*\"]
  - team: search-team
    targets: [\"search-*\"]
";

/// A store with one run for each `(name, status)`, all at `BASE_NS`.
fn runs_of(runs: &[(&str, &str)]) -> (tempfile::TempDir, tumult_lake::Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let spans: Vec<tumult_lake::SpanRow> = runs
        .iter()
        .map(|(name, status)| tumult_lake::SpanRow {
            ts_ns: BASE_NS,
            trace_id: format!("trace-{name}"),
            span_id: format!("span-{name}"),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            duration_ns: HOUR_NS,
            service_name: "tumult".into(),
            experiment_id: Some(format!("exp-{name}")),
            experiment_name: Some((*name).into()),
            outcome_status: Some((*status).into()),
            events: "[]".into(),
            ..Default::default()
        })
        .collect();
    store.writer().unwrap().insert_spans(&spans).unwrap();
    (d, store)
}

/// The digest of the day up to an hour after `BASE_NS`, by `TWO_DOMAINS`.
fn digest_by_domain(store: &tumult_lake::Store) -> ReportDoc {
    let reader = store.read_only().unwrap();
    let org = OrgTree::from_yaml(TWO_DOMAINS).unwrap();
    let as_of = BASE_NS + HOUR_NS;
    build_executive(&reader, &org, as_of, DAY_NS, as_of, &[]).unwrap()
}

#[test]
fn a_domain_at_the_full_score_has_no_weakest_member() {
    let (_d, store) = runs_of(&[
        ("checkout-restart", "completed"),
        ("search-node-loss", "deviated"),
        ("billing-export", "completed"),
    ]);

    let doc = digest_by_domain(&store);

    assert_eq!(
        rows_under(&doc, "By domain"),
        [
            "platform|50.0|fair|1/1|search-node-loss",
            "(unassigned)|100.0|good|1/1|—",
            "commerce|100.0|good|1/1|—",
        ]
    );
}

#[test]
fn a_domain_with_nothing_scored_shows_no_score() {
    use tumult_lake::{ExerciseType, ManualOutcome, NewManualExperiment};
    let (_d, store) = runs_of(&[("checkout-restart", "completed")]);
    // A drill of the platform domain that is written down and not yet verified.
    store
        .writer()
        .unwrap()
        .create_manual_draft(&NewManualExperiment {
            experiment_name: "search-failover-drill".into(),
            exercise_type: ExerciseType::GameDay,
            executed_at_ns: BASE_NS,
            hypothesis: "search answers from the second region".into(),
            method: "took the first region out of the pool".into(),
            outcome: ManualOutcome::Passed,
            hypothesis_met: Some(true),
            findings: None,
            action_items: vec![],
            target_system: Some("search".into()),
            target_environment: None,
            blast_radius: None,
            recovery_time_s: None,
            duration_s: None,
            entered_by: "alice".into(),
            attestation: "I attest this record reflects the exercise.".into(),
            renewal_due_ns: None,
            framework_refs: vec![],
            exercise: tumult_lake::ExerciseDetail::default(),
        })
        .unwrap();

    let doc = digest_by_domain(&store);

    // A domain with no score comes after the scored ones.
    assert_eq!(
        rows_under(&doc, "By domain"),
        [
            "commerce|100.0|good|1/1|—",
            "(unassigned)|—|—|0/0|—",
            "platform|—|—|0/1|—",
        ]
    );
}

#[test]
fn a_digest_of_an_empty_store_draws_no_empty_chart_or_table() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();

    let doc = digest_of_the_day(&store);

    for title in [
        "Experiment scores",
        "Coverage",
        "Target scores",
        "Open weaknesses and decisions needed",
    ] {
        let said: Vec<String> = blocks_under(&doc, title)
            .into_iter()
            .map(|block| match block {
                Block::Paragraph(text) => text.clone(),
                other => format!("{other:?}"),
            })
            .collect();

        assert_eq!(said, ["No experiments on record."], "{title}");
    }
}

#[test]
fn an_evidence_pack_of_one_experiment_says_one_experiment() {
    let (_d, store) = one_run("queue-restart", "completed");
    let reader = store.read_only().unwrap();

    let doc = build_evidence_pack(
        &reader,
        "dora",
        None,
        BASE_NS + HOUR_NS,
        &[],
        &House::of(&crate::dora::HouseParams::default()),
    )
    .unwrap();

    let scope = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Paragraph(p) => Some(p.as_str()),
            _ => None,
        })
        .expect("scope paragraph");
    assert!(
        scope.contains("framework; 1 experiment is on record. An automated experiment is"),
        "{scope}"
    );
}

/// An experiment that has no run on record.
fn never_run(name: &str) -> crate::scoring::ExperimentScore {
    crate::scoring::ExperimentScore {
        name: name.into(),
        target: None,
        environment: None,
        evidence_id: None,
        score: 0,
        state: crate::scoring::RunState::NeverRun,
        band: "poor".into(),
        last_run_ns: None,
        last_outcome: None,
        severity: None,
        runs: 0,
        origin: "automated".into(),
        journal_only: false,
    }
}

#[test]
fn the_outlook_counts_the_experiments_that_never_ran() {
    let card_of = |experiments| crate::scoring::Scorecard {
        portfolio: Some(0.0),
        band: "poor".into(),
        delta: None,
        as_of_ns: BASE_NS,
        targets: Vec::new(),
        experiments,
    };

    assert_eq!(
        outlook(&card_of(vec![never_run("a")])),
        "Next period priorities: schedule first runs for 1 untested experiment."
    );
    assert_eq!(
        outlook(&card_of(vec![never_run("a"), never_run("b")])),
        "Next period priorities: schedule first runs for 2 untested experiments."
    );
}

#[test]
fn a_run_that_recorded_no_blast_radius_says_so() {
    let doc = report_of_steps(&[run_span(None)], &[]);

    assert_eq!(value_of(&doc, "Blast radius"), "not recorded");
}

#[test]
fn the_appendix_holds_the_first_forty_attributes_by_name() {
    let root = tumult_lake::SpanRow {
        span_attrs: (1..=45)
            .map(|n| (format!("key-{n:02}"), n.to_string()))
            .collect(),
        ..run_span(None)
    };

    let doc = report_of_steps(&[root], &[]);

    let appendix = blocks_under(&doc, "Configuration appendix");
    let Some(Block::KeyValues(pairs)) = appendix.first() else {
        panic!("the appendix is no list of pairs");
    };
    assert_eq!(pairs.len(), 40);
    assert_eq!(pairs[39].0, "key-40");
}

#[test]
fn what_a_report_left_out_is_said_before_the_steps_that_failed() {
    let (mut spans, logs) = game_day_of_two();
    let fault = spans.iter_mut().find(|span| span.span_id == "fault");
    let fault = fault.unwrap();
    fault.status_code = "Error".into();
    fault.status_message = "exit 1".into();

    let doc = report_of_steps(&spans, &logs);

    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[WARN] 1 span of this trace names a parent that is not on record. It and the spans \
             under it are not in this report.",
            "[ERROR] slow-ledger (resilience.action): exit 1",
            "[WARN] the ledger is slow",
        ]
    );
}

#[test]
fn a_line_the_run_logged_many_times_is_one_finding_with_the_number_of_times() {
    let mut logs: Vec<_> = (0..150)
        .map(|n| logged_on("root", 1_000 + n, "WARN", "the ledger is slow"))
        .collect();
    logs.push(logged_on(
        "root",
        2_000,
        "ERROR",
        "the ledger did not answer",
    ));

    let doc = report_of_steps(&[run_span(None)], &logs);

    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[WARN] the ledger is slow (× 150)",
            "[ERROR] the ledger did not answer",
        ]
    );
}

#[test]
fn a_warning_and_an_error_of_one_text_are_two_findings_in_the_order_they_came_first() {
    let logs = [
        logged_on("root", 1_000, "WARN", "the ledger is slow"),
        logged_on("root", 2_000, "ERROR", "the ledger is slow"),
        logged_on("root", 3_000, "WARN", "the ledger is slow"),
    ];

    let doc = report_of_steps(&[run_span(None)], &logs);

    assert_eq!(
        bullets_under(&doc, "Findings"),
        [
            "[WARN] the ledger is slow (× 2)",
            "[ERROR] the ledger is slow",
        ]
    );
}

#[test]
fn a_report_shows_the_first_hundred_different_lines_a_run_logged_and_says_how_many_there_were() {
    let lines = |count: i64| -> Vec<tumult_lake::LogRow> {
        (0..count)
            .map(|n| {
                let body = format!("call {n} to the ledger was slow");
                logged_on("root", 1_000 + n, "WARN", &body)
            })
            .collect()
    };

    let hundred = report_of_steps(&[run_span(None)], &lines(100));

    let findings = bullets_under(&hundred, "Findings");
    assert_eq!(findings.len(), 100);
    assert_eq!(findings[0], "[WARN] call 0 to the ledger was slow");
    assert_eq!(findings[99], "[WARN] call 99 to the ledger was slow");

    let more = report_of_steps(&[run_span(None)], &lines(101));

    let findings = bullets_under(&more, "Findings");
    assert_eq!(findings.len(), 101);
    assert_eq!(
        findings[0],
        "[WARN] The run logged 101 different warnings and errors. This report shows the first \
         100."
    );
    assert_eq!(findings[1], "[WARN] call 0 to the ledger was slow");
    assert_eq!(findings[100], "[WARN] call 99 to the ledger was slow");
}

#[test]
fn a_report_that_shows_part_of_the_lines_says_so_before_what_it_left_out_of_the_trace() {
    let lines: Vec<_> = (0..101)
        .map(|n| {
            let body = format!("call {n} to the ledger was slow");
            logged_on("root", 1_000 + n, "WARN", &body)
        })
        .collect();
    let (spans, _) = game_day_of_two();

    let doc = report_of_steps(&spans, &lines);

    let findings = bullets_under(&doc, "Findings");
    assert_eq!(
        findings[..2],
        [
            "[WARN] The run logged 101 different warnings and errors. This report shows the \
             first 100.",
            "[WARN] 1 span of this trace names a parent that is not on record. It and the spans \
             under it are not in this report.",
        ]
    );
}

#[test]
fn a_report_cut_short_in_its_spans_says_so_before_the_lines_it_does_not_show() {
    let d = tempfile::TempDir::new().unwrap();
    let store = tumult_lake::Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    writer.insert_spans(&polled(1)).unwrap();
    store_polls(&writer, 1, game_day::MOST_SPANS);
    let lines: Vec<_> = (0..101)
        .map(|n| {
            let body = format!("call {n} to the ledger was slow");
            logged_on("root", 1_000 + n, "WARN", &body)
        })
        .collect();
    writer.insert_logs(&lines).unwrap();

    let doc = report_in(&store);

    let findings = bullets_under(&doc, "Findings");
    assert_eq!(
        findings[..2],
        [
            "[WARN] The run has more than 20000 spans. This report reads the first 20000, so it \
             may miss a step that failed later.",
            "[WARN] The run logged 101 different warnings and errors. This report shows the \
             first 100.",
        ]
    );
    assert_eq!(findings.len(), 102);
}
