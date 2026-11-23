//! The R2 compliance evidence pack, per framework. The DORA pack reads its
//! traceability matrix from the requirement registry as the DORA score
//! reads it; the other frameworks have a skeleton of clauses.

use std::collections::BTreeMap;

use serde_json::Value;
use tumult_core::compliance::{
    Requirement, EVIDENCE_DISCLAIMER, REQUIREMENTS, REQUIREMENTS_VERSION,
};
use tumult_lake::{scope, Reader, SqlParam};

use super::dr::{self, Objectives};
use super::{base_meta, cell, counted, House};
use crate::dora::coverage::{Matrix, MatrixRow};
use crate::dora::loader::{self, Request};
use crate::dora::report::SCOPE_NOTE;
use crate::dora::status::{self, Status, StatusReport};
use crate::html::fmt_date;
use crate::model::{Block, Cell, ReportDoc, TemplateKind};
use crate::scoring;

/// Clause lists per framework (skeleton — must be verified against the
/// licensed framework text; see the mandatory footnote on every R2). DORA
/// has none: its matrix is the requirement registry
/// ([`tumult_core::compliance::REQUIREMENTS`]).
pub const FRAMEWORK_CLAUSES: &[(&str, &[&str])] = &[
    ("dora", &[]),
    (
        "nis2",
        &[
            "Art. 21(2)(c) — business continuity and crisis management",
            "Art. 21(2)(f) — policies on effectiveness of risk measures",
        ],
    ),
    (
        "iso27001",
        &[
            "A.5.29 — information security during disruption",
            "A.5.30 — ICT readiness for business continuity",
            "A.8.13 — information backup",
            "A.8.14 — redundancy of information processing facilities",
        ],
    ),
    (
        "soc2",
        &[
            "A1.2 — recovery commitments and system requirements",
            "A1.3 — recovery testing of backup infrastructure",
        ],
    ),
];

pub const CLAUSE_VERIFY_FOOTNOTE: &str = "Clause references should be verified \
     against the licensed framework text before submission.";

/// The most evidence ids a row of the DORA matrix names.
const MOST_EVIDENCE: usize = 6;

/// The kind, the RTO and the RPO of each automated run of the register, by
/// run id, with the columns of the lake view `dr_tests`.
const RUN_OBJECTIVES_SQL: &str = "SELECT e.experiment_id AS id, e.test_kind AS kind, \
     e.rto_target_s, coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s, \
     v.recovered, NOT e.impact_observed AS no_outage, e.rpo_target_s, e.rpo_measured_s \
     FROM experiments AS e \
     LEFT JOIN ( \
         SELECT experiment_id, bool_and(recovered) AS recovered FROM nfr_verdicts \
         WHERE kind = 'rto' AND nfr_id = 'rto' GROUP BY experiment_id \
     ) AS v ON e.experiment_id = v.experiment_id \
     WHERE list_contains(from_json(CAST($1 AS VARCHAR), '[\"VARCHAR\"]'), e.experiment_id)";

/// The evidence status of a row in words.
fn status_words(status: Status) -> String {
    match status {
        Status::Na => "not applicable".into(),
        other => other.as_str().replace('_', " "),
    }
}

/// The evidence behind a row of the matrix: the best evidence of each
/// function it binds, each named once, [`MOST_EVIDENCE`] at most. A row
/// with none says why.
fn evidence_of(row: &MatrixRow<'_>) -> String {
    if row.cells.is_empty() {
        return "binds no function in scope".into();
    }
    let mut ids: Vec<String> = Vec::new();
    for best in row.cells.iter().filter_map(|c| c.cell.best.as_ref()) {
        let id = format!("{} {}", best.evidence.source.as_str(), best.evidence.id);
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return row.reason(None).unwrap_or("no evidence").to_string();
    }
    let more = ids.len().saturating_sub(MOST_EVIDENCE);
    ids.truncate(MOST_EVIDENCE);
    if more != 0 {
        ids.push(format!("and {more} more"));
    }
    ids.join(", ")
}

/// One row of the DORA matrix: a row of the registry, its status and its
/// evidence. A row the profile of the entity excludes is not applicable.
fn matrix_row(matrix: &Matrix<'_>, report: &StatusReport, req: &Requirement) -> Vec<Cell> {
    let (status, evidence) = match (matrix.row(req.id), report.row(req.id)) {
        (Some(row), Some(row_status)) => (status_words(row_status.status), evidence_of(row)),
        _ => (
            status_words(Status::Na),
            "the profile of the legal entity excludes this row".into(),
        ),
    };
    vec![
        Cell::text(req.cite),
        Cell::text(req.title),
        Cell::text(status),
        Cell::text(evidence),
    ]
}

/// The DORA traceability matrix: every row of the requirement registry,
/// with the evidence status `GET /api/dora` gives it and the evidence
/// behind it, as of `as_of_ns` in `envs`.
fn dora_matrix(
    reader: &Reader,
    house: &House<'_>,
    as_of_ns: i64,
    envs: &[String],
) -> Result<Vec<Block>, String> {
    let request = Request {
        envs,
        lei: None,
        profile: None,
        as_of_ns,
        workloads: house.workloads,
        proof_days: house.proof_days,
    };
    let loaded = loader::load(reader, request, house.params).map_err(|e| e.to_string())?;
    let matrix = Matrix::build(&loaded.inputs, house.params);
    let report = status::report(&matrix, None);
    let entity = if loaded.entity.lei.is_empty() {
        "no registered legal entity".to_string()
    } else {
        format!("{} (LEI {})", loaded.entity.name, loaded.entity.lei)
    };
    let mut blocks = vec![Block::Paragraph(format!(
        "Each row is a row of the DORA requirement registry, version {REQUIREMENTS_VERSION}, \
         scored for {entity} under the {} profile as of {}, as GET /api/dora scores it. The \
         status is the evidence status of the row: none, designed, existing or operating, or \
         unknown, no events or not applicable when it cannot be scored. The evidence is the \
         best run or manual record behind each function the row binds. A status says what \
         evidence is held. It is not a compliance verdict.",
        loaded.inputs.entity.label(),
        fmt_date(as_of_ns),
    ))];
    if loaded.scoped {
        blocks.push(Block::Paragraph(SCOPE_NOTE.into()));
    }
    blocks.push(Block::Table {
        headers: vec![
            "Article and paragraph".into(),
            "Requirement".into(),
            "Status".into(),
            "Evidence".into(),
        ],
        rows: REQUIREMENTS
            .iter()
            .map(|req| matrix_row(&matrix, &report, req))
            .collect(),
        numeric_cols: vec![],
        widths: Some(vec![1.1, 2.4, 0.9, 2.8]),
    });
    blocks.push(Block::Footnote(EVIDENCE_DISCLAIMER.into()));
    Ok(blocks)
}

/// The skeleton matrix of a framework with no registry: every clause
/// unmapped.
fn unmapped(clauses: &[&str]) -> Block {
    Block::Table {
        headers: vec![
            "Clause".into(),
            "Evidence".into(),
            "Result".into(),
            "Finding".into(),
            "Remediation".into(),
        ],
        rows: clauses
            .iter()
            .map(|clause| {
                vec![
                    Cell::text(*clause),
                    Cell::text("No reviewed clause mapping"),
                    Cell::status("Unmapped"),
                    Cell::text("Clause applicability and coverage not verified"),
                    Cell::text("Map evidence and obtain independent review"),
                ]
            })
            .collect(),
        numeric_cols: vec![],
        widths: Some(vec![3.2, 2.2, 1.0, 0.9, 1.1]),
    }
}

/// The kind and the objectives of each automated run of `ids`, by run id.
fn run_objectives(reader: &Reader, ids: &[String]) -> Result<BTreeMap<String, Value>, String> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows = reader
        .query_json_rows_bound(RUN_OBJECTIVES_SQL, &[SqlParam::text_list(ids)])
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter_map(|row| Some((cell(&row, "id")?.to_string(), row)))
        .collect())
}

/// The kind, the RTO and the RPO of each test of the register, in its
/// order: the exercise type of a manual record, the test kind of a run. A
/// run that did not recover, and a run whose probes saw no outage, say so.
fn objectives_table(
    reader: &Reader,
    card: &scoring::Scorecard,
    manual_by_id: &BTreeMap<String, &Value>,
) -> Result<Block, String> {
    let run_ids: Vec<String> = card
        .experiments
        .iter()
        .filter(|e| e.origin != "manual")
        .filter_map(|e| e.evidence_id.clone())
        .collect();
    let runs_by_id = run_objectives(reader, &run_ids)?;
    let rows = card
        .experiments
        .iter()
        .map(|e| {
            let manual = (e.origin == "manual")
                .then(|| {
                    e.evidence_id
                        .as_ref()
                        .and_then(|id| manual_by_id.get(id).copied())
                })
                .flatten();
            let run = (e.origin != "manual")
                .then(|| e.evidence_id.as_ref().and_then(|id| runs_by_id.get(id)))
                .flatten();
            let test = manual.or(run);
            let kind = manual
                .and_then(|m| cell(m, "exercise_type"))
                .or_else(|| run.and_then(|r| cell(r, "kind")))
                .unwrap_or("not recorded");
            let objectives = test.map(Objectives::of).unwrap_or_default();
            vec![
                Cell::text(e.name.clone()),
                Cell::text(kind),
                Cell::text(objectives.rto_planned()),
                Cell::text(objectives.rto_actual()),
                Cell::text(objectives.rpo_planned()),
                Cell::text(objectives.rpo_actual()),
            ]
        })
        .collect();
    Ok(Block::Table {
        headers: vec![
            "Experiment".into(),
            "Kind".into(),
            "RTO planned".into(),
            "RTO actual".into(),
            "RPO planned".into(),
            "RPO actual".into(),
        ],
        rows,
        numeric_cols: vec![],
        widths: Some(vec![2.4, 1.0, 1.0, 1.2, 1.0, 1.1]),
    })
}

/// The DR detail of a verified manual record of a DR type: the RTO and
/// the RPO planned and actual, the data check, the plan, and the
/// attachments and sign-offs of the record.
fn dr_detail(reader: &Reader, row: &Value) -> Result<Vec<(String, Cell)>, String> {
    let objectives = Objectives::of(row);
    let mut detail = vec![
        (
            "RTO planned / actual".to_string(),
            Cell::text(objectives.rto()),
        ),
        ("RPO planned / actual".into(), Cell::text(objectives.rpo())),
        (
            "Data verified".into(),
            Cell::text(dr::data_verified(dr::flag(row, "data_verified"))),
        ),
        (
            "Plan".into(),
            Cell::text(cell(row, "plan_id").unwrap_or("none")),
        ),
    ];
    let Some(id) = cell(row, "id") else {
        return Ok(detail);
    };
    let record = reader
        .manual_experiment_detail(id)
        .map_err(|e| e.to_string())?;
    if let Some(record) = &record {
        detail.extend(dr::record_evidence(record));
    }
    Ok(detail)
}

/// Build the R2 evidence pack for `framework` ("dora" | "nis2" |
/// "iso27001" | "soc2"). `envs` confines the scorecard, test register,
/// approval chain and attestation appendix to the given environments
/// (empty = unscoped). The DORA matrix is scored with the parameters and
/// workloads of `house`, as `GET /api/dora` scores it, and the appendix
/// gives the DR detail of a record of a type the house counts as DR.
///
/// # Errors
/// Returns an error string for unknown frameworks or failed queries.
pub fn build_evidence_pack(
    reader: &Reader,
    framework: &str,
    period_ns: Option<i64>,
    generated_at_ns: i64,
    envs: &[String],
    house: &House<'_>,
) -> Result<ReportDoc, String> {
    let key = framework.to_ascii_lowercase();
    let Some(clauses) = FRAMEWORK_CLAUSES
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, clauses)| *clauses)
    else {
        return Err(format!(
            "unknown framework {framework:?}; expected one of: dora, nis2, iso27001, soc2"
        ));
    };

    let as_of_ns = generated_at_ns;
    let card = scoring::compute_scoped(reader, as_of_ns, period_ns, envs)?;
    let from_ns = period_ns.map(|p| as_of_ns - p);

    let seed = format!(
        "R2|{}|{key}|{period_ns:?}|{generated_at_ns}",
        envs.join(",")
    );
    let mut meta = base_meta(
        TemplateKind::EvidencePack,
        format!("Evidence pack — {}", key.to_uppercase()),
        generated_at_ns,
        as_of_ns,
        &seed,
    );
    meta.framework = Some(key.to_uppercase());
    meta.period = from_ns.map(|f| (f, as_of_ns));

    let mut blocks = vec![
        Block::H1("Scope and methodology".into()),
        Block::Paragraph(format!(
            "This pack summarises the resilience testing evidence Tumult recorded \
             for the {} framework{}{}. An automated experiment is a chaos or continuity \
             test that Tumult ran. The outcome of a run is the status that run recorded; \
             a manual exercise has the outcome of its verified record.",
            key.to_uppercase(),
            from_ns.map_or(String::new(), |f| format!(
                " over the period {} – {}",
                fmt_date(f),
                fmt_date(as_of_ns)
            )),
            if card.experiments.is_empty() {
                ". No experiments are on record in the store".into()
            } else {
                let on_record = card.experiments.len();
                format!(
                    "; {}",
                    counted(on_record, "experiment is on record", "experiments are on record")
                )
            },
        )),
        Block::H2("Independence".into()),
        Block::Paragraph(
            "Independence has not been verified by this report. Organizational \
             separation of test design/execution from service operation requires \
             a reviewed attestation. Manual evidence review records identify the \
             enterer and verifier; those identities alone do not prove organizational independence."
                .into(),
        ),
    ];

    blocks.push(Block::H2("Traceability matrix".into()));
    if key == "dora" {
        blocks.extend(dora_matrix(reader, house, as_of_ns, envs)?);
    } else {
        blocks.push(unmapped(clauses));
    }
    blocks.push(Block::Footnote(CLAUSE_VERIFY_FOOTNOTE.into()));

    blocks.push(Block::H2("Test register".into()));
    if card.experiments.is_empty() {
        blocks.push(Block::Paragraph("No experiments on record.".into()));
    } else {
        // Provenance joins the exact scored manual record, never a display name.
        let manual_rows = reader
            .query_json_rows(&format!(
                "SELECT id, experiment_name AS name, executed_at_ns, entered_by, entered_at_ns, \
                 reviewed_by, reviewed_at_ns, exercise_type, rto_target_s, \
                 recovery_time_s AS rto_measured_s, rpo_target_s, rpo_measured_s \
                 FROM manual_experiments \
                 WHERE status = 'verified'{} ORDER BY executed_at_ns DESC",
                scope::and(scope::in_list("target_environment", envs)),
            ))
            .map_err(|e| e.to_string())?;
        let mut manual_by_id: std::collections::BTreeMap<String, &serde_json::Value> =
            std::collections::BTreeMap::new();
        for row in &manual_rows {
            if let Some(id) = cell(row, "id") {
                manual_by_id.insert(id.to_string(), row);
            }
        }
        blocks.push(Block::Table {
            headers: vec![
                "Experiment".into(),
                "Origin".into(),
                "Target / environment".into(),
                "Executed".into(),
                "Entered".into(),
                "Verifier".into(),
                "Outcome".into(),
                "Score".into(),
            ],
            rows: card
                .experiments
                .iter()
                .map(|e| {
                    let manual = (e.origin == "manual")
                        .then(|| e.evidence_id.as_ref().and_then(|id| manual_by_id.get(id)))
                        .flatten();
                    let entered = manual
                        .and_then(|m| m.get("entered_at_ns"))
                        .and_then(serde_json::Value::as_i64);
                    let verifier = manual.and_then(|m| cell(m, "reviewed_by"));
                    vec![
                        Cell::text(e.name.clone()),
                        Cell::text(e.origin.clone()),
                        Cell::text(format!(
                            "{} / {}",
                            e.target.as_deref().unwrap_or("—"),
                            e.environment.as_deref().unwrap_or("—")
                        )),
                        Cell::text(e.last_run_ns.map_or("never".into(), fmt_date)),
                        Cell::text(entered.map_or("—".into(), fmt_date)),
                        Cell::text(verifier.unwrap_or("—")),
                        e.last_outcome
                            .as_ref()
                            .map_or_else(|| Cell::text("—"), |o| Cell::status(o.clone())),
                        Cell::text(e.score.to_string()),
                    ]
                })
                .collect(),
            numeric_cols: vec![7],
            widths: Some(vec![1.8, 0.8, 0.7, 1.2, 1.2, 0.9, 1.0, 0.6]),
        });
        blocks.push(Block::Paragraph(
            "The kind of each test, and the RTO and the RPO it planned and measured, in the \
             order of the register:"
                .into(),
        ));
        blocks.push(objectives_table(reader, &card, &manual_by_id)?);
    }

    // SOC 2 CC8.1 change-management evidence: one row per approval-gated run.
    blocks.push(Block::H2("Approval chain (change management)".into()));
    blocks.push(Block::Paragraph(
        "Approval-gated runs (tiers T1–T3) execute only after explicit human \
         approval — the change-management evidence for SOC 2 CC8.1. Each request \
         pins the exact change content by hash, enforces an approver quorum with \
         segregation of duties, and lapses on a TTL; approvals are single-use and \
         break-glass overrides carry a mandatory justification. Pin hashes are \
         re-verified at dispatch, and the full per-run, hash-chained `run_audit` \
         trail is available via the API. Runs are shown by their id prefix; the \
         full id and pin hash are available via the run-detail API."
            .into(),
    ));
    blocks.push(Block::Paragraph(
        "Approval-chain appendix: at most the 500 newest installation records are sampled, \
         then filtered to this scope and period. This appendix is not a complete approval \
         export; use the portable run_audit and approval tables for complete evidence."
            .into(),
    ));
    let approvals = reader.approvals_list(500).map_err(|e| e.to_string())?;
    let approvals: Vec<&serde_json::Value> = approvals
        .iter()
        .filter(|a| {
            // Scoped packs list only approvals for in-scope environments;
            // an approval with no environment on record fails closed.
            let env_visible =
                envs.is_empty() || cell(a, "env").is_some_and(|e| envs.iter().any(|s| s == e));
            env_visible
                && from_ns.is_none_or(|f| {
                    a.get("requested_at_ns")
                        .and_then(serde_json::Value::as_i64)
                        .is_some_and(|t| (f..=as_of_ns).contains(&t))
                })
        })
        .collect();
    if approvals.is_empty() {
        blocks.push(Block::Paragraph(
            "No approval-gated runs in the selected sample for this scope and period.".into(),
        ));
    } else {
        let ns_of =
            |row: &serde_json::Value, key: &str| row.get(key).and_then(serde_json::Value::as_i64);
        blocks.push(Block::Table {
            headers: vec![
                "Run".into(),
                "Definition".into(),
                "Tier".into(),
                "Env".into(),
                "Requested by".into(),
                "Quorum".into(),
                "Decisions".into(),
                "Break-glass".into(),
                "Consumed".into(),
                "Run state".into(),
            ],
            rows: approvals
                .iter()
                .map(|a| {
                    let approved = ns_of(a, "approved_count").unwrap_or(0);
                    let rejected = ns_of(a, "rejected_count").unwrap_or(0);
                    let break_glass = a
                        .get("break_glass")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    vec![
                        // Id prefix only: the full 36-char id overflows a
                        // table cell (no wrap points); the intro names the
                        // run-detail API for the full id.
                        Cell::text(
                            cell(a, "run_id")
                                .map(|id| id.chars().take(8).collect::<String>())
                                .unwrap_or_else(|| "—".into()),
                        ),
                        Cell::text(cell(a, "definition_name").unwrap_or("—")),
                        Cell::text(cell(a, "tier").unwrap_or("—")),
                        Cell::text(cell(a, "env").unwrap_or("—")),
                        Cell::text(format!(
                            "{} on {}",
                            cell(a, "requested_by").unwrap_or("—"),
                            ns_of(a, "requested_at_ns").map_or("—".into(), fmt_date)
                        )),
                        Cell::text(format!(
                            "{approved}/{}",
                            ns_of(a, "quorum_required").unwrap_or(0)
                        )),
                        Cell::text(format!("approved×{approved}, rejected×{rejected}")),
                        Cell::text(if break_glass {
                            format!("yes — {}", cell(a, "break_glass_by").unwrap_or("?"))
                        } else {
                            "no".into()
                        }),
                        Cell::text(if ns_of(a, "consumed_at_ns").is_some() {
                            "yes"
                        } else {
                            "no"
                        }),
                        Cell::status(cell(a, "run_state").unwrap_or("—")),
                    ]
                })
                .collect(),
            numeric_cols: vec![],
            widths: Some(vec![0.9, 1.3, 0.45, 0.5, 1.15, 0.75, 1.2, 0.9, 0.85, 0.95]),
        });
    }

    // Attestation appendix: one entry per verified manual record.
    let attested = reader
        .query_json_rows(&format!(
            "SELECT id, experiment_name AS name, exercise_type, executed_at_ns, entered_by, \
             entered_at_ns, reviewed_by, reviewed_at_ns, attestation, framework_refs, \
             rto_target_s, recovery_time_s AS rto_measured_s, rpo_target_s, rpo_measured_s, \
             data_verified, plan_id \
             FROM manual_experiments WHERE status = 'verified'{} \
             ORDER BY experiment_name, executed_at_ns DESC",
            scope::and(scope::in_list("target_environment", envs)),
        ))
        .map_err(|e| e.to_string())?;
    let dr_types = house.params.dr_exercise_types();
    if !attested.is_empty() {
        blocks.push(Block::H2("Manual attestation appendix".into()));
        blocks.push(Block::Paragraph(
            "Each manually executed test below was entered under the quoted attestation \
             and verified by a reviewer other than the person who entered it. Entries \
             are immutable once verified; corrections require a new record."
                .into(),
        ));
        for row in &attested {
            let name = cell(row, "name").unwrap_or("unknown");
            blocks.push(Block::H3(format!(
                "{} — {}",
                name,
                row.get("executed_at_ns")
                    .and_then(serde_json::Value::as_i64)
                    .map_or("undated".into(), fmt_date)
            )));
            let frameworks = row
                .get("framework_refs")
                .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
                .filter(|v| !v.is_empty())
                .map_or("—".into(), |v| v.join(", "));
            let mut entries = vec![
                (
                    "Exercise type".into(),
                    Cell::text(cell(row, "exercise_type").unwrap_or("—")),
                ),
                (
                    "Executed".into(),
                    Cell::text(
                        row.get("executed_at_ns")
                            .and_then(serde_json::Value::as_i64)
                            .map_or("—".into(), fmt_date),
                    ),
                ),
                (
                    "Entered by".into(),
                    Cell::text(format!(
                        "{} on {}",
                        cell(row, "entered_by").unwrap_or("—"),
                        row.get("entered_at_ns")
                            .and_then(serde_json::Value::as_i64)
                            .map_or("—".into(), fmt_date)
                    )),
                ),
                (
                    "Verified by".into(),
                    Cell::text(format!(
                        "{} on {}",
                        cell(row, "reviewed_by").unwrap_or("—"),
                        row.get("reviewed_at_ns")
                            .and_then(serde_json::Value::as_i64)
                            .map_or("—".into(), fmt_date)
                    )),
                ),
                ("Frameworks".into(), Cell::text(frameworks)),
                (
                    "Attestation".into(),
                    Cell::text(cell(row, "attestation").unwrap_or("—")),
                ),
            ];
            let dr_record =
                cell(row, "exercise_type").is_some_and(|t| dr_types.iter().any(|d| d == t));
            if dr_record {
                entries.extend(dr_detail(reader, row)?);
            }
            blocks.push(Block::KeyValues(entries));
        }
    }

    let open: Vec<&scoring::ExperimentScore> = card
        .experiments
        .iter()
        .filter(|e| e.state.is_open())
        .collect();
    blocks.push(Block::H2("Findings log".into()));
    if open.is_empty() {
        blocks.push(Block::Paragraph("No open findings.".into()));
    } else {
        blocks.push(Block::Bullets(
            open.iter()
                .map(|e| {
                    format!(
                        "{} ({}): {} — last run {}",
                        e.name,
                        e.target.clone().unwrap_or("—".into()),
                        e.last_outcome.clone().unwrap_or("never run".into()),
                        e.last_run_ns.map_or("never".into(), fmt_date),
                    )
                })
                .collect(),
        ));
    }

    blocks.push(Block::Footnote(CLAUSE_VERIFY_FOOTNOTE.into()));
    blocks.push(Block::Signoff(vec![
        ("Prepared by".into(), "Tumult".into()),
        ("Compliance officer".into(), String::new()),
    ]));

    Ok(ReportDoc { meta, blocks })
}
