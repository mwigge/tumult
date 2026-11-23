//! The R5 DR exercise report: one manual DR record as an auditor reads it.
//! Its plan, the people who took part, the steps with their times, the
//! RTO and the RPO planned and actual, the data check, the findings, the
//! actions with their owners, the files with their SHA-256, the sign-offs,
//! and what the audit verify of the record finds at the time of the
//! report.

use serde_json::Value;
use tumult_core::compliance::EVIDENCE_DISCLAIMER;
use tumult_lake::{AuditVerdict, ManualDetail, Reader};

use super::dr::{self, planned, seconds};
use super::{base_meta, cell, counted, House};
use crate::html::{fmt_date, fmt_datetime};
use crate::model::{Block, Cell, ReportDoc, TemplateKind};

/// The record statuses a DR exercise report is of: those the DR view
/// counts.
const COUNTED_STATUSES: [&str; 2] = ["submitted", "verified"];

/// The text `key` of a row, or `fallback`.
fn text_or<'a>(row: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    cell(row, key).unwrap_or(fallback)
}

/// A time of a row as a date and a time of day, or `fallback`.
fn at_or(row: &Value, key: &str, fallback: &str) -> String {
    dr::time(row, key).map_or_else(|| fallback.to_string(), fmt_datetime)
}

/// Who did something and when, as `alice on 2027-01-01 10:00 UTC`.
fn by_on(row: &Value, by: &str, at: &str, nobody: &str) -> String {
    match cell(row, by) {
        Some(who) => format!("{who} on {}", at_or(row, at, "an unknown time")),
        None => nobody.to_string(),
    }
}

/// Whether the record is one the report is of: in `envs` (empty = every
/// environment), of a type the house counts as DR, and submitted or
/// verified.
fn reported(record: &Value, house: &House<'_>, envs: &[String]) -> bool {
    let in_scope = envs.is_empty()
        || cell(record, "target_environment").is_some_and(|env| envs.iter().any(|e| e == env));
    let dr_type = cell(record, "exercise_type")
        .is_some_and(|kind| house.params.dr_exercise_types().iter().any(|t| t == kind));
    let counted = cell(record, "status").is_some_and(|s| COUNTED_STATUSES.contains(&s));
    in_scope && dr_type && counted
}

/// The key facts of the record.
fn summary(id: &str, record: &Value) -> Block {
    let hypothesis_met = match dr::flag(record, "hypothesis_met") {
        Some(true) => "yes",
        Some(false) => "no",
        None => "not recorded",
    };
    let duration = dr::number(record, "duration_s").map_or_else(|| "not recorded".into(), seconds);
    let text = |key: &str, fallback: &'static str| Cell::text(text_or(record, key, fallback));
    Block::KeyValues(vec![
        ("Record id".into(), Cell::text(id)),
        ("Exercise".into(), text("experiment_name", "unnamed")),
        ("Type".into(), text("exercise_type", "not recorded")),
        ("Target".into(), text("target_system", "not recorded")),
        (
            "Environment".into(),
            text("target_environment", "not recorded"),
        ),
        (
            "Executed".into(),
            Cell::text(at_or(record, "executed_at_ns", "not recorded")),
        ),
        ("Duration".into(), Cell::text(duration)),
        ("Status".into(), text("status", "not recorded")),
        ("Outcome".into(), text("outcome_status", "not recorded")),
        ("Hypothesis".into(), text("hypothesis", "not recorded")),
        ("Method".into(), text("method", "not recorded")),
        ("Hypothesis met".into(), Cell::text(hypothesis_met)),
        (
            "Entered by".into(),
            Cell::text(by_on(record, "entered_by", "entered_at_ns", "not recorded")),
        ),
        (
            "Reviewed by".into(),
            Cell::text(by_on(
                record,
                "reviewed_by",
                "reviewed_at_ns",
                "not reviewed yet",
            )),
        ),
        ("Attestation".into(), text("attestation", "not recorded")),
        ("Seal".into(), text("content_hash", "not recorded")),
    ])
}

/// The plan the record fulfils.
fn plan(plan: Option<&Value>) -> Block {
    let Some(plan) = plan else {
        return Block::Paragraph("The record fulfils no plan.".into());
    };
    let interval = dr::time(plan, "interval_days").map_or_else(
        || "once".into(),
        |days| counted(usize::try_from(days).unwrap_or(0), "day", "days"),
    );
    let text = |key: &str, fallback: &'static str| Cell::text(text_or(plan, key, fallback));
    Block::KeyValues(vec![
        ("Plan id".into(), text("id", "unknown")),
        ("Name".into(), text("name", "unnamed")),
        ("Owner".into(), text("owner", "not recorded")),
        ("Status".into(), text("status", "not recorded")),
        (
            "Due".into(),
            Cell::text(dr::time(plan, "due_at_ns").map_or_else(|| "no date".into(), fmt_date)),
        ),
        ("Interval".into(), Cell::text(interval)),
        (
            "RTO planned".into(),
            Cell::text(planned(dr::number(plan, "rto_target_s"))),
        ),
        (
            "RPO planned".into(),
            Cell::text(planned(dr::number(plan, "rpo_target_s"))),
        ),
        ("Scope".into(), text("scope", "not recorded")),
    ])
}

/// The people who took part, with their roles.
fn participants(record: &Value) -> Block {
    let people: Vec<Vec<Cell>> = record
        .get("participants")
        .and_then(Value::as_array)
        .map(|people| {
            people
                .iter()
                .map(|p| {
                    vec![
                        Cell::text(text_or(p, "name", "unnamed")),
                        Cell::text(text_or(p, "role", "no role")),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    if people.is_empty() {
        return Block::Paragraph("No participants on record.".into());
    }
    Block::Table {
        headers: vec!["Name".into(), "Role".into()],
        rows: people,
        numeric_cols: vec![],
        widths: Some(vec![1.0, 2.0]),
    }
}

/// The steps of the exercise in the order they were taken.
fn steps(detail: &ManualDetail) -> Block {
    if detail.steps.is_empty() {
        return Block::Paragraph("No steps on record.".into());
    }
    let row = |step: &Value| {
        vec![
            Cell::text(dr::time(step, "ordinal").map_or_else(String::new, |n| n.to_string())),
            Cell::text(text_or(step, "title", "untitled")),
            Cell::text(text_or(step, "actor", "not recorded")),
            Cell::text(at_or(step, "started_at_ns", "not recorded")),
            Cell::text(at_or(step, "ended_at_ns", "not recorded")),
            Cell::text(text_or(step, "result", "not recorded")),
            Cell::text(text_or(step, "note", "none")),
        ]
    };
    Block::Table {
        headers: vec![
            "#".into(),
            "Step".into(),
            "Actor".into(),
            "Started".into(),
            "Ended".into(),
            "Result".into(),
            "Note".into(),
        ],
        rows: detail.steps.iter().map(row).collect(),
        numeric_cols: vec![0],
        widths: Some(vec![0.3, 1.8, 0.8, 1.3, 1.3, 0.7, 1.4]),
    }
}

/// Whether an objective was met: `no target` without a plan, `not
/// measured` without an actual, else `met` or `not met`.
fn result(target: Option<f64>, actual: Option<f64>) -> &'static str {
    match (target, actual) {
        (None, _) => "no target",
        (Some(_), None) => "not measured",
        (Some(target), Some(actual)) if actual <= target => "met",
        (Some(_), Some(_)) => "not met",
    }
}

/// One row of the recovery objectives.
fn objective(name: &str, target: Option<f64>, actual: Option<f64>) -> Vec<Cell> {
    vec![
        Cell::text(name),
        Cell::text(planned(target)),
        Cell::text(actual.map_or_else(|| "not measured".into(), seconds)),
        Cell::status(result(target, actual)),
    ]
}

/// The RTO and the RPO planned and actual, and the data check.
fn objectives(record: &Value) -> Vec<Block> {
    vec![
        Block::Table {
            headers: vec![
                "Objective".into(),
                "Planned".into(),
                "Actual".into(),
                "Result".into(),
            ],
            rows: vec![
                objective(
                    "RTO",
                    dr::number(record, "rto_target_s"),
                    dr::number(record, "recovery_time_s"),
                ),
                objective(
                    "RPO",
                    dr::number(record, "rpo_target_s"),
                    dr::number(record, "rpo_measured_s"),
                ),
            ],
            numeric_cols: vec![],
            widths: Some(vec![1.0, 1.0, 1.0, 1.0]),
        },
        Block::KeyValues(vec![(
            "Data verified".into(),
            Cell::text(dr::data_verified(dr::flag(record, "data_verified"))),
        )]),
    ]
}

/// The actions with their owners and where they stand. A record of before
/// v22 has its actions as plain texts.
fn actions(detail: &ManualDetail) -> Block {
    if detail.actions.is_empty() {
        let older: Vec<String> = detail
            .experiment
            .get("action_items")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if older.is_empty() {
            return Block::Paragraph("No actions on record.".into());
        }
        return Block::Bullets(older);
    }
    let row = |action: &Value| {
        vec![
            Cell::text(dr::time(action, "ordinal").map_or_else(String::new, |n| n.to_string())),
            Cell::text(text_or(action, "title", "untitled")),
            Cell::text(text_or(action, "owner", "no owner")),
            Cell::text(dr::time(action, "due_at_ns").map_or_else(|| "no date".into(), fmt_date)),
            Cell::text(text_or(action, "ticket", "none")),
            Cell::text(text_or(action, "status", "open")),
        ]
    };
    Block::Table {
        headers: vec![
            "#".into(),
            "Action".into(),
            "Owner".into(),
            "Due".into(),
            "Ticket".into(),
            "Status".into(),
        ],
        rows: detail.actions.iter().map(row).collect(),
        numeric_cols: vec![0],
        widths: Some(vec![0.3, 2.4, 0.8, 0.9, 0.9, 0.7]),
    }
}

/// Each attachment with what the lake keeps of it.
fn attachments(detail: &ManualDetail) -> Vec<Block> {
    if detail.attachments.is_empty() {
        return vec![Block::Paragraph("No attachments on record.".into())];
    }
    let mut blocks = Vec::with_capacity(detail.attachments.len() * 2);
    for (n, row) in (1..).zip(&detail.attachments) {
        let kind = text_or(row, "kind", "link");
        let name = cell(row, "label")
            .or_else(|| cell(row, "uri"))
            .unwrap_or("unnamed");
        blocks.push(Block::H3(format!("Attachment {n}")));
        let mut kvs = vec![
            ("Name".to_string(), Cell::text(name)),
            ("Kind".into(), Cell::text(kind)),
        ];
        if kind == "file" {
            let size = dr::time(row, "size_bytes")
                .map_or_else(|| "unknown".into(), |n| format!("{n} bytes"));
            kvs.push((
                "Type".into(),
                Cell::text(text_or(row, "media_type", "unknown")),
            ));
            kvs.push(("Size".into(), Cell::text(size)));
        } else {
            kvs.push((
                "Points to".into(),
                Cell::text(text_or(row, "uri", "nowhere")),
            ));
        }
        kvs.push((
            "SHA-256".into(),
            Cell::text(text_or(row, "file_hash", "not recorded")),
        ));
        kvs.push((
            "Added by".into(),
            Cell::text(by_on(row, "added_by", "added_at_ns", "not recorded")),
        ));
        blocks.push(Block::KeyValues(kvs));
    }
    blocks
}

/// The sign-offs, and how many the type of the record needs.
fn signoffs(detail: &ManualDetail, house: &House<'_>) -> Vec<Block> {
    let kind = text_or(&detail.experiment, "exercise_type", "exercise");
    let required = house.params.required_signoffs(kind);
    let approved = detail
        .signoffs
        .iter()
        .filter(|s| cell(s, "decision") == Some("approved"))
        .count();
    let needed = usize::try_from(required).unwrap_or(usize::MAX);
    let mut blocks = vec![Block::Paragraph(format!(
        "{} of the {needed} that a {kind} needs.",
        counted(approved, "approval", "approvals"),
    ))];
    if detail.signoffs.is_empty() {
        return blocks;
    }
    let row = |s: &Value| {
        vec![
            Cell::text(text_or(s, "role", "no role")),
            Cell::text(text_or(s, "signed_by", "unknown")),
            Cell::text(at_or(s, "signed_at_ns", "unknown")),
            Cell::status(text_or(s, "decision", "unknown")),
            Cell::text(text_or(s, "note", "none")),
        ]
    };
    blocks.push(Block::Table {
        headers: vec![
            "Role".into(),
            "Signed by".into(),
            "Signed".into(),
            "Decision".into(),
            "Note".into(),
        ],
        rows: detail.signoffs.iter().map(row).collect(),
        numeric_cols: vec![],
        widths: Some(vec![0.8, 0.9, 1.3, 0.9, 1.6]),
    });
    blocks
}

/// What the audit verify of the record found, at `at_ns`.
fn audit(verdict: Option<&AuditVerdict>, at_ns: i64) -> Vec<Block> {
    let at = fmt_datetime(at_ns);
    let Some(verdict) = verdict else {
        return vec![Block::Paragraph(format!(
            "The audit verify found no record at {at}."
        ))];
    };
    let mut blocks = Vec::new();
    if verdict.ok {
        blocks.push(Block::Paragraph(format!(
            "The audit verify found no problem at {at}: the seal of the record, its audit \
             trail, its reviewer, its sign-offs and its attachments with their stored files all \
             check out."
        )));
    } else {
        blocks.push(Block::Paragraph(format!(
            "The audit verify found {} at {at}:",
            counted(verdict.problems.len(), "problem", "problems"),
        )));
        blocks.push(Block::Table {
            headers: vec![
                "Table".into(),
                "Row".into(),
                "Field".into(),
                "Problem".into(),
            ],
            rows: verdict
                .problems
                .iter()
                .map(|p| {
                    vec![
                        Cell::text(p.table.clone()),
                        Cell::text(p.row.clone()),
                        Cell::text(p.field.clone().unwrap_or_else(|| "the row".into())),
                        Cell::text(p.problem.clone()),
                    ]
                })
                .collect(),
            numeric_cols: vec![],
            widths: Some(vec![1.3, 1.5, 0.9, 2.5]),
        });
    }
    if verdict.legacy_rows != 0 {
        blocks.push(Block::Paragraph(format!(
            "{} at the start of the trail came before the link hash and are not checked as \
             links.",
            counted(verdict.legacy_rows, "audit row", "audit rows"),
        )));
    }
    blocks
}

/// Build the R5 DR exercise report of the manual record `record_id` as of
/// `generated_at_ns`. `Ok(None)` when there is no such record, when it is
/// outside `envs` (empty = every environment), when its type is not one
/// the house counts as DR, or when it is not submitted or verified: the DR
/// view counts no such record either.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn build_dr_exercise(
    reader: &Reader,
    house: &House<'_>,
    record_id: &str,
    generated_at_ns: i64,
    envs: &[String],
) -> Result<Option<ReportDoc>, String> {
    let store = |e: tumult_lake::StoreError| e.to_string();
    let Some(detail) = reader.manual_experiment_detail(record_id).map_err(store)? else {
        return Ok(None);
    };
    let record = &detail.experiment;
    if !reported(record, house, envs) {
        return Ok(None);
    }
    let plan_row = match cell(record, "plan_id") {
        Some(id) => reader
            .manual_plan_detail(id)
            .map_err(store)?
            .map(|p| p.plan),
        None => None,
    };
    let verdict = reader.verify_manual_record(record_id).map_err(store)?;

    let name = text_or(record, "experiment_name", "unnamed exercise");
    let mut meta = base_meta(
        TemplateKind::DrExercise,
        format!("DR exercise report: {name}"),
        generated_at_ns,
        generated_at_ns,
        &format!("R5|{record_id}|{generated_at_ns}"),
    );
    meta.subject = vec![("Record id".into(), record_id.to_string())];

    let mut blocks = vec![
        Block::H1("Exercise".into()),
        summary(record_id, record),
        Block::H2("Plan".into()),
        plan(plan_row.as_ref()),
        Block::H2("Participants".into()),
        participants(record),
        Block::H2("Steps".into()),
        steps(&detail),
        Block::H2("Recovery objectives".into()),
    ];
    blocks.extend(objectives(record));
    blocks.push(Block::H2("Findings".into()));
    blocks.push(Block::Paragraph(
        text_or(record, "findings", "No findings on record.").to_string(),
    ));
    blocks.push(Block::H2("Actions".into()));
    blocks.push(actions(&detail));
    blocks.push(Block::H2("Attachments".into()));
    blocks.extend(attachments(&detail));
    blocks.push(Block::H2("Sign-offs".into()));
    blocks.extend(signoffs(&detail, house));
    blocks.push(Block::H2("Audit verify".into()));
    blocks.extend(audit(verdict.as_ref(), generated_at_ns));
    blocks.push(Block::Footnote(EVIDENCE_DISCLAIMER.into()));
    blocks.push(Block::Signoff(vec![
        ("DR owner".into(), String::new()),
        ("Reviewer".into(), String::new()),
    ]));
    Ok(Some(ReportDoc { meta, blocks }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_objective_is_met_up_to_its_target_and_not_met_above_it() {
        assert_eq!(result(None, Some(1.0)), "no target");
        assert_eq!(result(Some(60.0), None), "not measured");
        assert_eq!(result(Some(60.0), Some(60.0)), "met");
        assert_eq!(result(Some(60.0), Some(60.5)), "not met");
    }
}
