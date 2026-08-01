//! The R2 compliance evidence pack skeleton, per framework.

use tumult_lake::Reader;

use super::{base_meta, cell};
use crate::html::fmt_date;
use crate::model::{Block, Cell, ReportDoc, TemplateKind};
use crate::scoring::{self, RunState};

/// Clause lists per framework (skeleton — must be verified against the
/// licensed framework text; see the mandatory footnote on every R2).
pub const FRAMEWORK_CLAUSES: &[(&str, &[&str])] = &[
    (
        "dora",
        &[
            "Art. 11(4) — response and recovery plans",
            "Art. 12(2) — backup, restoration and recovery",
            "Art. 24–26 — resilience testing programme",
        ],
    ),
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

/// Build the R2 evidence pack skeleton for `framework` ("dora" | "nis2" |
/// "iso27001" | "soc2"). `envs` confines the scorecard, test register,
/// approval chain and attestation appendix to the given environments
/// (empty = unscoped).
///
/// # Errors
/// Returns an error string for unknown frameworks or failed queries.
pub fn build_evidence_pack(
    reader: &Reader,
    framework: &str,
    period_ns: Option<i64>,
    generated_at_ns: i64,
    envs: &[String],
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
             for the {} framework{}{}. Experiments are chaos/continuity tests \
             executed via tumult; outcomes are taken from each run's completion log.",
            key.to_uppercase(),
            from_ns.map_or(String::new(), |f| format!(
                " over the period {} – {}",
                fmt_date(f),
                fmt_date(as_of_ns)
            )),
            if card.experiments.is_empty() {
                ". No experiments are on record in the store".into()
            } else {
                format!("; {} experiments are on record", card.experiments.len())
            },
        )),
        Block::H2("Independence".into()),
        Block::Paragraph(
            "Testing performed in line with the independence expectations of DORA \
             Art. 24(4): experiment design and execution are separated from the \
             teams operating the affected services. Automated results are recorded \
             without manual editing; manually executed tests are entered under an \
             attestation and verified by a reviewer other than the person who \
             entered them (segregation of duties)."
                .into(),
        ),
    ];

    blocks.push(Block::H2("Traceability matrix".into()));
    let tested_names: Vec<String> = card
        .experiments
        .iter()
        .filter(|e| e.state != RunState::NeverRun)
        .map(|e| e.name.clone())
        .collect();
    let tested_summary = if tested_names.is_empty() {
        "—".to_string()
    } else if tested_names.len() <= 3 {
        tested_names.join(", ")
    } else {
        format!("See test register ({})", tested_names.len())
    };
    blocks.push(Block::Table {
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
                    Cell::text(tested_summary.clone()),
                    Cell::status(card.band.clone()),
                    Cell::text("—"),
                    Cell::text("—"),
                ]
            })
            .collect(),
        numeric_cols: vec![],
        widths: Some(vec![3.2, 2.2, 1.0, 0.9, 1.1]),
    });
    blocks.push(Block::Footnote(CLAUSE_VERIFY_FOOTNOTE.into()));

    blocks.push(Block::H2("Test register".into()));
    if card.experiments.is_empty() {
        blocks.push(Block::Paragraph("No experiments on record.".into()));
    } else {
        // Provenance for verified manual records, latest record per name.
        let manual_rows = reader
            .query_json_rows(&format!(
                "SELECT experiment_name AS name, executed_at_ns, entered_by, entered_at_ns, \
                 reviewed_by, reviewed_at_ns FROM manual_experiments \
                 WHERE status = 'verified'{} ORDER BY executed_at_ns DESC",
                scoring::and_env(scoring::env_in("target_environment", envs)),
            ))
            .map_err(|e| e.to_string())?;
        let mut manual_by_name: std::collections::BTreeMap<String, &serde_json::Value> =
            std::collections::BTreeMap::new();
        for row in &manual_rows {
            if let Some(name) = cell(row, "name") {
                manual_by_name.entry(name.to_string()).or_insert(row);
            }
        }
        blocks.push(Block::Table {
            headers: vec![
                "Experiment".into(),
                "Origin".into(),
                "Target".into(),
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
                    let manual = manual_by_name.get(&e.name);
                    let entered = manual
                        .and_then(|m| m.get("entered_at_ns"))
                        .and_then(serde_json::Value::as_i64);
                    let verifier = manual.and_then(|m| cell(m, "reviewed_by"));
                    vec![
                        Cell::text(e.name.clone()),
                        Cell::status(e.origin.clone()),
                        Cell::text(e.target.clone().unwrap_or("—".into())),
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
            "No approval-gated runs in the period.".into(),
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
            "SELECT experiment_name AS name, exercise_type, executed_at_ns, entered_by, \
             entered_at_ns, reviewed_by, reviewed_at_ns, attestation, framework_refs \
             FROM manual_experiments WHERE status = 'verified'{} \
             ORDER BY experiment_name, executed_at_ns DESC",
            scoring::and_env(scoring::env_in("target_environment", envs)),
        ))
        .map_err(|e| e.to_string())?;
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
            blocks.push(Block::KeyValues(vec![
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
            ]));
        }
    }

    let open: Vec<&scoring::ExperimentScore> = card
        .experiments
        .iter()
        .filter(|e| matches!(e.state, RunState::Failed | RunState::NeverRun))
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
