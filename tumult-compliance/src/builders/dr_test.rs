//! The R4 DR test report: every DR test of one target over a period,
//! automated and manual, each against the RTO and the RPO it planned, with
//! its evidence, the due state of the target, and a block left for the DR
//! owner and the reviewer to sign.

use std::collections::BTreeMap;

use serde_json::Value;
use tumult_core::compliance::EVIDENCE_DISCLAIMER;
use tumult_lake::{DrScope, DrTestFilter, Reader, SqlParam};

use super::dr::{self, Objectives, JOURNAL_HASH_NOT_KEPT};
use super::{base_meta, cell, counted, House};
use crate::html::fmt_date;
use crate::model::{Block, Cell, ReportDoc, TemplateKind};

/// The trace of each automated test, by run id: the root span of a run
/// that sent its trace. A run stored only as a journal has none.
const TRACES_SQL: &str = "SELECT experiment_id, any_value(trace_id) AS trace_id \
     FROM run_roots \
     WHERE trace_id IS NOT NULL \
       AND list_contains(from_json(CAST($1 AS VARCHAR), '[\"VARCHAR\"]'), experiment_id) \
     GROUP BY experiment_id";

/// The target and the period of a DR test report. Both ends of the period
/// are in it, in unix nanoseconds.
#[derive(Debug, Clone, Copy)]
pub struct DrTestRequest<'a> {
    pub target: &'a str,
    pub from_ns: i64,
    pub to_ns: i64,
}

/// The environments of a report in words.
fn environments(envs: &[String]) -> String {
    match envs {
        [] => "every environment".into(),
        more => more.join(", "),
    }
}

/// What the report covers and what it does not say.
fn scope_paragraph(request: &DrTestRequest<'_>, envs: &[String], dr_types: &[String]) -> String {
    format!(
        "This report lists the DR tests of {} in {} from {} to {}: the runs of kind dr, and the \
         manual exercises of a type the house counts as DR ({}) that are submitted or verified. \
         Each test is set against the RTO and the RPO it planned. A test that did not recover, \
         and a run whose probes saw no outage, say so. The report holds evidence only. It is \
         not a compliance verdict.",
        request.target,
        environments(envs),
        fmt_date(request.from_ns),
        fmt_date(request.to_ns),
        dr_types.join(", "),
    )
}

/// The tests of the period per verdict, from the rows of
/// `Reader::dr_test_counts`.
fn kpis(counts: &[Value]) -> Block {
    let mut per_verdict: BTreeMap<&str, u64> = BTreeMap::new();
    let mut total = 0;
    for row in counts {
        let tests = row.get("tests").and_then(Value::as_u64).unwrap_or(0);
        *per_verdict
            .entry(cell(row, "verdict").unwrap_or("not_measured"))
            .or_default() += tests;
        total += tests;
    }
    let of = |verdict: &str| per_verdict.get(verdict).copied().unwrap_or(0).to_string();
    Block::Kpis(vec![
        ("DR tests".into(), total.to_string(), None),
        ("Met".into(), of("met"), None),
        ("Breached".into(), of("breached"), None),
        ("Not recovered".into(), of("not_recovered"), None),
        ("Not measured".into(), of("not_measured"), None),
    ])
}

/// The interval of a target and where it comes from, in words.
fn interval(row: &Value) -> String {
    let source = match cell(row, "interval_source") {
        Some("plan") => "the plan",
        Some("function") => "the test plan of its function",
        _ => "house default",
    };
    let days = dr::time(row, "interval_days").unwrap_or(0);
    let days = counted(usize::try_from(days).unwrap_or(0), "day", "days");
    format!("{days} ({source})")
}

/// The last DR test of a target that proves something, in words.
fn last_test(row: &Value) -> String {
    let Some(at) = dr::time(row, "last_test_at_ns") else {
        return "none".into();
    };
    format!(
        "{} ({}, {})",
        fmt_date(at),
        cell(row, "last_test_origin").unwrap_or("unknown origin"),
        cell(row, "last_test_verdict")
            .unwrap_or("not_measured")
            .replace('_', " "),
    )
}

/// One row of the due state: a target in one environment.
fn due_row(row: &Value) -> Vec<Cell> {
    let state = cell(row, "state")
        .unwrap_or("never_tested")
        .replace('_', " ");
    let next =
        dr::time(row, "next_due_at_ns").map_or_else(|| "no date: never tested".into(), fmt_date);
    vec![
        Cell::text(environment(row)),
        Cell::status(state),
        Cell::text(last_test(row)),
        Cell::text(next),
        Cell::text(interval(row)),
    ]
}

/// The environment of a row of the register; `any` for a plan that names
/// none.
fn environment(row: &Value) -> &str {
    cell(row, "target_environment").unwrap_or("any")
}

/// The planned DR exercise that covers the target in the environment of
/// `row`, in a sentence, when one does.
fn covering_plan(row: &Value, target: &str) -> Option<Block> {
    let id = cell(row, "plan_id")?;
    let due = dr::time(row, "plan_due_at_ns").map_or_else(|| "no date".into(), fmt_date);
    Some(Block::Paragraph(format!(
        "A planned DR exercise covers {target} in {}: plan {id}, due {due}.",
        environment(row)
    )))
}

/// The due state of the target: one row per environment of the DR test
/// register, and the planned exercises that cover it.
fn due_state(targets: &[Value], target: &str) -> Vec<Block> {
    if targets.is_empty() {
        return vec![Block::Paragraph(format!(
            "The DR test register holds no entry for {target} in these environments: it has no \
             DR test and declares no RTO."
        ))];
    }
    let mut blocks = vec![Block::Table {
        headers: vec![
            "Environment".into(),
            "State".into(),
            "Last DR test".into(),
            "Next due".into(),
            "Interval".into(),
        ],
        rows: targets.iter().map(due_row).collect(),
        numeric_cols: vec![],
        widths: Some(vec![1.0, 0.9, 2.0, 1.1, 1.8]),
    }];
    blocks.extend(targets.iter().filter_map(|row| covering_plan(row, target)));
    blocks
}

/// Where a test comes from: `automated`, or `manual` with its type.
fn origin(row: &Value) -> String {
    match (cell(row, "origin"), cell(row, "exercise_type")) {
        (Some("manual"), Some(kind)) => format!("manual {kind}"),
        (origin, _) => origin.unwrap_or("unknown").to_string(),
    }
}

/// The name of a test.
fn name(row: &Value) -> &str {
    cell(row, "experiment_name").unwrap_or("unnamed test")
}

/// The name of a test and where it comes from, for the register.
fn named(row: &Value) -> String {
    format!("{} ({})", name(row), origin(row))
}

/// The date a test ran.
fn date(row: &Value) -> String {
    dr::time(row, "executed_at_ns").map_or_else(|| "undated".into(), fmt_date)
}

/// One row of the register: a test against the RTO and the RPO it planned.
fn test_row(row: &Value) -> Vec<Cell> {
    let objectives = Objectives::of(row);
    vec![
        Cell::text(date(row)),
        Cell::text(named(row)),
        Cell::text(objectives.rto_planned()),
        Cell::text(objectives.rto_actual()),
        Cell::text(objectives.rpo_planned()),
        Cell::text(objectives.rpo_actual()),
        Cell::status(dr::verdict(row)),
    ]
}

/// The tests of the period, oldest first.
fn register(tests: &[Value], target: &str) -> Block {
    if tests.is_empty() {
        return Block::Paragraph(format!("No DR test of {target} in this period."));
    }
    Block::Table {
        headers: vec![
            "Date".into(),
            "Test".into(),
            "RTO planned".into(),
            "RTO actual".into(),
            "RPO planned".into(),
            "RPO actual".into(),
            "Verdict".into(),
        ],
        rows: tests.iter().map(test_row).collect(),
        numeric_cols: vec![],
        widths: Some(vec![1.45, 2.5, 1.1, 1.3, 1.1, 1.2, 1.35]),
    }
}

/// The trace id of each automated test of `tests` that sent its trace.
fn traces_of(reader: &Reader, tests: &[Value]) -> Result<BTreeMap<String, String>, String> {
    let runs: Vec<String> = tests
        .iter()
        .filter(|row| cell(row, "origin") == Some("automated"))
        .filter_map(|row| cell(row, "evidence_id").map(str::to_owned))
        .collect();
    if runs.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows = reader
        .query_json_rows_bound(TRACES_SQL, &[SqlParam::text_list(&runs)])
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some((
                cell(row, "experiment_id")?.to_string(),
                cell(row, "trace_id")?.to_string(),
            ))
        })
        .collect())
}

/// The evidence of an automated test: the run, how it ended, its trace
/// and its journal.
fn run_evidence(row: &Value, traces: &BTreeMap<String, String>) -> Vec<(String, Cell)> {
    let id = cell(row, "evidence_id").unwrap_or("unknown");
    let trace = traces
        .get(id)
        .map_or("none: the run sent no trace", String::as_str);
    vec![
        ("Run id".into(), Cell::text(id)),
        (
            "Outcome".into(),
            Cell::text(cell(row, "outcome").unwrap_or("not recorded")),
        ),
        ("Trace id".into(), Cell::text(trace)),
        ("Journal SHA-256".into(), Cell::text(JOURNAL_HASH_NOT_KEPT)),
    ]
}

/// The evidence of a manual test: the record, its status, its reviewer,
/// its attachments with their SHA-256 and its sign-offs.
fn record_evidence(reader: &Reader, row: &Value) -> Result<Vec<(String, Cell)>, String> {
    let id = cell(row, "evidence_id").unwrap_or("unknown");
    let detail = reader
        .manual_experiment_detail(id)
        .map_err(|e| e.to_string())?;
    let status = detail
        .as_ref()
        .and_then(|d| cell(&d.experiment, "status"))
        .unwrap_or("not on record");
    let mut evidence = vec![
        ("Record id".to_string(), Cell::text(id)),
        ("Status".into(), Cell::text(status)),
        (
            "Outcome".into(),
            Cell::text(cell(row, "outcome").unwrap_or("not recorded")),
        ),
        (
            "Reviewer".into(),
            Cell::text(cell(row, "reviewer").unwrap_or("not reviewed yet")),
        ),
    ];
    if let Some(detail) = &detail {
        evidence.extend(dr::record_evidence(detail));
    }
    Ok(evidence)
}

/// The evidence of each test: a heading and its links.
fn evidence(
    reader: &Reader,
    tests: &[Value],
    traces: &BTreeMap<String, String>,
) -> Result<Vec<Block>, String> {
    let mut blocks = Vec::with_capacity(tests.len() * 2);
    for row in tests {
        blocks.push(Block::H3(format!("{} {}", date(row), name(row))));
        let links = if cell(row, "origin") == Some("manual") {
            record_evidence(reader, row)?
        } else {
            run_evidence(row, traces)
        };
        blocks.push(Block::KeyValues(links));
    }
    Ok(blocks)
}

/// Build the R4 DR test report of `request.target` over its period, in
/// `envs` (empty = every environment), as of `generated_at_ns`. The tests
/// are the rows of the lake view `dr_tests` with the DR types of the
/// house; the due state is the DR test register as of `generated_at_ns`.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn build_dr_test(
    reader: &Reader,
    house: &House<'_>,
    request: &DrTestRequest<'_>,
    generated_at_ns: i64,
    envs: &[String],
) -> Result<ReportDoc, String> {
    let scope = DrScope {
        dr_types: house.params.dr_exercise_types(),
        envs: envs.to_vec(),
    };
    let filter = DrTestFilter {
        target: Some(request.target.to_string()),
        from_ns: Some(request.from_ns),
        to_ns: Some(request.to_ns),
    };
    let store = |e: tumult_lake::StoreError| e.to_string();
    // Every test of the period: a target tests DR a few times a year.
    let mut tests = reader
        .dr_tests_page(&scope, &filter, None, u32::MAX)
        .map_err(store)?;
    tests.reverse();
    let counts = reader.dr_test_counts(&scope, &filter).map_err(store)?;
    let targets = reader
        .dr_targets(&scope, Some(request.target), generated_at_ns)
        .map_err(store)?;
    let traces = traces_of(reader, &tests)?;

    let seed = format!(
        "R4|{}|{}|{}|{}|{generated_at_ns}",
        request.target,
        envs.join(","),
        request.from_ns,
        request.to_ns
    );
    let mut meta = base_meta(
        TemplateKind::DrTest,
        format!("DR test report: {}", request.target),
        generated_at_ns,
        generated_at_ns,
        &seed,
    );
    meta.period = Some((request.from_ns, request.to_ns));
    meta.subject = vec![
        ("Target".into(), request.target.to_string()),
        ("Environments".into(), environments(envs)),
    ];

    let mut blocks = vec![
        Block::H1("Scope".into()),
        Block::Paragraph(scope_paragraph(request, envs, &scope.dr_types)),
        kpis(&counts),
        Block::H2("Due state".into()),
    ];
    blocks.extend(due_state(&targets, request.target));
    blocks.extend([
        Block::H2("DR tests".into()),
        register(&tests, request.target),
    ]);
    if !tests.is_empty() {
        blocks.push(Block::H2("Evidence".into()));
        blocks.extend(evidence(reader, &tests, &traces)?);
    }
    blocks.push(Block::Footnote(EVIDENCE_DISCLAIMER.into()));
    blocks.push(Block::Signoff(vec![
        ("DR owner".into(), String::new()),
        ("Reviewer".into(), String::new()),
    ]));
    Ok(ReportDoc { meta, blocks })
}
