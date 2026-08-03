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
fn framework_lists_cover_all_four() {
    assert_eq!(FRAMEWORK_CLAUSES.len(), 4);
    for (_, clauses) in FRAMEWORK_CLAUSES {
        assert!(!clauses.is_empty());
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
    let doc = build_evidence_pack(&reader, "soc2", None, now, &[]).unwrap();

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
    let doc = build_evidence_pack(&reader, "soc2", None, BASE_NS, &[]).unwrap();
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "No approval-gated runs in the period."
    )));
}

#[test]
fn evidence_pack_approval_chain_respects_period() {
    let (_d, store) = gated_fixture();
    let reader = store.read_only().unwrap();
    let now = BASE_NS + DAY_NS + 12 * HOUR_NS;

    // A one-hour period excludes both requests.
    let doc = build_evidence_pack(&reader, "soc2", Some(HOUR_NS), now, &[]).unwrap();
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "No approval-gated runs in the period."
    )));

    // A period reaching back past run-b but not run-a only keeps run-b.
    let doc =
        build_evidence_pack(&reader, "soc2", Some(DAY_NS + 12 * HOUR_NS - 1), now, &[]).unwrap();
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

    let scoped = build_evidence_pack(&reader, "soc2", None, now, &["staging".to_string()]).unwrap();
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
    let err = build_evidence_pack(&reader, "pci-dss", None, BASE_NS, &[]).unwrap_err();
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
            framework_refs: vec!["DORA Art. 24(7)".into(), "ISO 27001 A.5.29".into()],
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
    let doc = build_evidence_pack(&reader, "dora", Some(12 * HOUR_NS), now, &[]).unwrap();

    assert_eq!(doc.meta.framework.as_deref(), Some("DORA"));
    assert_eq!(doc.meta.period, Some((now - 12 * HOUR_NS, now)));
    // The scope paragraph names the framework and the period.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p.contains("DORA") && p.contains('–') && p.contains("experiments are on record")
    )));
    // Traceability matrix: one row per DORA clause, tested summary joined.
    let matrix = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Traceability matrix"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("traceability matrix");
    assert_eq!(matrix.len(), 3);
    assert!(row_text(&matrix[0]).contains("flaky-exp, drill-exp"));

    // Register: the manual record carries entered/verifier provenance.
    let register = register_table(&doc);
    assert_eq!(register.len(), 2, "rows: {register:?}");
    let manual_row = register
        .iter()
        .find(|r| row_text(r).contains("drill-exp"))
        .expect("manual row");
    let text = row_text(manual_row);
    assert!(text.contains("manual"), "{text}");
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
    assert_eq!(findings.len(), 1);
    assert!(findings[0].contains("flaky-exp"), "{findings:?}");
    assert!(findings[0].contains("Deviated"), "{findings:?}");
}

#[test]
fn evidence_pack_traceability_defers_to_register_past_three_experiments() {
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
    let doc = build_evidence_pack(&reader, "nis2", None, BASE_NS + HOUR_NS, &[]).unwrap();
    let matrix = doc
        .blocks
        .iter()
        .skip_while(|b| !matches!(b, Block::H2(t) if t == "Traceability matrix"))
        .find_map(|b| match b {
            Block::Table { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .expect("traceability matrix");
    assert!(row_text(&matrix[0]).contains("See test register (4)"));
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
            Block::Paragraph(p) if p.starts_with("Portfolio resilience") => Some(p.clone()),
            _ => None,
        })
        .expect("bluf paragraph");
    assert!(bluf.contains("declining"), "{bluf}");
    assert!(bluf.contains("open weakness"), "{bluf}");

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
            Block::Paragraph(p) if p.starts_with("Portfolio resilience") => Some(p.clone()),
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
                span_name: "resilience.rollback.restart".into(),
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
    assert_eq!(get("Outcome"), "Completed");

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

    // Rollback section counts the clean rollback spans.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "1 rollback span(s) executed; status: all clean."
    )));

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
    let pdf = crate::typst_pdf::render_pdf(&doc).expect("pdf render");
    assert!(pdf.starts_with(b"%PDF"));
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
    // No completion log: incomplete outcome; no hypothesis recorded;
    // fault type without a subtype stands alone.
    assert_eq!(get("Outcome"), "incomplete");
    assert_eq!(get("Hypothesis"), "Not recorded");
    assert_eq!(get("Scenario"), "network");
    assert_eq!(get("Severity"), "medium"); // default

    // A failed rollback is flagged for investigation.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p.contains("errors present — investigate")
    )));
    // No WARN/ERROR logs: the quiet-run message.
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        Block::Paragraph(p) if p == "No warnings or errors were logged during this run."
    )));
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
