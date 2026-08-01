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
