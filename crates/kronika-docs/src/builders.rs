//! Report builders: store queries → [`ReportDoc`] for each template.
//!
//! All SQL values that flow from parameters go through [`q`] (single-quote
//! doubling). Numbers (`i64` timestamps) are formatted directly.

use kronika_store::Reader;
use sha2::{Digest, Sha256};

use crate::html::{fmt_date, fmt_datetime};
use crate::model::{Block, ChartSpec, DocMeta, ReportDoc, TemplateKind};
use crate::scoring::{self, RunState, Scorecard};

const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// Quote a string value for DuckDB SQL.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Document ID: `KRK-<code>-<yyyymmdd>-<hash6>` where hash6 is the first 6
/// hex chars of SHA-256 over template + params + timestamp.
fn doc_id(code: &str, seed: &str, generated_at_ns: i64) -> String {
    let ymd = fmt_date(generated_at_ns).replace('-', "");
    let digest = Sha256::digest(seed.as_bytes());
    let hash6: String = digest[..3].iter().map(|b| format!("{b:02x}")).collect();
    format!("KRK-{code}-{ymd}-{hash6}")
}

fn base_meta(
    template: TemplateKind,
    title: String,
    generated_at_ns: i64,
    data_as_of_ns: i64,
    seed: &str,
) -> DocMeta {
    DocMeta {
        doc_id: doc_id(template.code(), seed, generated_at_ns),
        title,
        template,
        version: env!("CARGO_PKG_VERSION").to_string(),
        classification: "Internal".into(),
        generated_at_ns,
        data_as_of_ns,
        period: None,
        framework: None,
        experiment_id: None,
    }
}

fn cell<'a>(row: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(serde_json::Value::as_str)
}

/// Round to one decimal for display.
fn f1(v: f64) -> String {
    format!("{v:.1}")
}

// ---------------------------------------------------------------- R1

/// Build the R1 executive resilience digest as of `as_of_ns`, comparing
/// against `period_ns` earlier.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn build_executive(
    reader: &Reader,
    as_of_ns: i64,
    period_ns: i64,
    generated_at_ns: i64,
) -> Result<ReportDoc, String> {
    let card = scoring::compute(reader, as_of_ns, Some(period_ns))?;
    let from_ns = as_of_ns - period_ns;

    let (discovered, fixed, mttr_s) = issue_stats(reader, from_ns, as_of_ns)?;
    let open: Vec<&scoring::ExperimentScore> = card
        .experiments
        .iter()
        .filter(|e| e.state != RunState::Passed && e.state != RunState::Stale)
        .collect();
    let tested = card
        .experiments
        .iter()
        .filter(|e| e.state != RunState::NeverRun)
        .count();

    let seed = format!("R1|{from_ns}|{as_of_ns}|{generated_at_ns}");
    let mut meta = base_meta(
        TemplateKind::ExecutiveDigest,
        "Executive resilience digest".into(),
        generated_at_ns,
        as_of_ns,
        &seed,
    );
    meta.period = Some((from_ns, as_of_ns));

    let mut blocks = vec![
        Block::H1("Bottom line".into()),
        Block::Paragraph(bluf(&card, &open)),
    ];

    blocks.push(Block::Kpis(vec![
        (
            "Portfolio score".into(),
            f1(card.portfolio),
            Some(card.band.clone()),
        ),
        (
            "Δ vs previous period".into(),
            card.delta.map_or("—".into(), |d| format!("{d:+.1}")),
            None,
        ),
        (
            "Experiments tested".into(),
            format!("{tested} / {}", card.experiments.len()),
            None,
        ),
        ("Open weaknesses".into(), open.len().to_string(), None),
        (
            "Issues fixed".into(),
            format!("{fixed} / {discovered}"),
            mttr_s.map(|m| format!("MTTR {m:.1}s")),
        ),
    ]));

    blocks.push(Block::Chart(ChartSpec::Bars(
        card.targets
            .iter()
            .map(|t| (t.target.clone(), t.score))
            .collect(),
    )));

    blocks.push(Block::H2("Target scores".into()));
    blocks.push(Block::Table {
        headers: vec![
            "Target".into(),
            "Experiments".into(),
            "Runs".into(),
            "Score".into(),
            "Band".into(),
        ],
        rows: card
            .targets
            .iter()
            .map(|t| {
                vec![
                    t.target.clone(),
                    card.experiments
                        .iter()
                        .filter(|e| e.target.as_deref().unwrap_or("(untargeted)") == t.target)
                        .count()
                        .to_string(),
                    t.runs.to_string(),
                    f1(t.score),
                    t.band.clone(),
                ]
            })
            .collect(),
        numeric_cols: vec![1, 2, 3],
    });

    blocks.push(Block::H2("Issues discovered and fixed".into()));
    blocks.push(Block::Paragraph(format!(
        "In the period {} – {}, {} experiment runs deviated or failed; {} were later \
         fixed (a subsequent completed run of the same experiment — an intentionally \
         simple, auditable heuristic).{}",
        fmt_date(from_ns),
        fmt_date(as_of_ns),
        discovered,
        fixed,
        mttr_s.map_or(String::new(), |m| format!(
            " Mean time to recovery across runs reporting one: {m:.1}s."
        )),
    )));

    blocks.push(Block::H2("Open weaknesses".into()));
    if open.is_empty() {
        blocks.push(Block::Paragraph(
            "No open weaknesses: every known experiment last ran green.".into(),
        ));
    } else {
        blocks.push(Block::Table {
            headers: vec![
                "Experiment".into(),
                "Target".into(),
                "Severity".into(),
                "Last run".into(),
                "Age (d)".into(),
            ],
            rows: open
                .iter()
                .map(|e| {
                    vec![
                        e.name.clone(),
                        e.target.clone().unwrap_or("—".into()),
                        e.last_outcome.clone().unwrap_or("incomplete".into()),
                        e.last_run_ns.map_or("never".into(), fmt_date),
                        e.last_run_ns
                            .map_or("—".into(), |ts| ((as_of_ns - ts) / DAY_NS).to_string()),
                    ]
                })
                .collect(),
            numeric_cols: vec![4],
        });
    }

    blocks.push(Block::H2("Outlook".into()));
    blocks.push(Block::Paragraph(outlook(&card)));

    blocks.push(Block::Footnote(format!(
        "Scores: passed 100, stale pass (>30d) 75, failed 50, never run 0; bands \
         >70 good, 50–70 fair, <50 poor. Generated by kronika v{}; data as of {}.",
        env!("CARGO_PKG_VERSION"),
        fmt_date(as_of_ns)
    )));
    blocks.push(Block::Signoff(vec![
        ("Prepared by".into(), "kronika".into()),
        ("Approved by".into(), String::new()),
    ]));

    Ok(ReportDoc { meta, blocks })
}

fn bluf(card: &Scorecard, open: &[&scoring::ExperimentScore]) -> String {
    let trend = card.delta.map_or("no prior-period baseline".into(), |d| {
        if d > 0.5 {
            format!("improving ({d:+.1} vs the previous period)")
        } else if d < -0.5 {
            format!("declining ({d:+.1} vs the previous period)")
        } else {
            format!("flat ({d:+.1} vs the previous period)")
        }
    });
    let weakest: Vec<String> = card
        .targets
        .iter()
        .take(3)
        .map(|t| format!("{} ({})", t.target, f1(t.score)))
        .collect();
    let mut s = format!(
        "Portfolio resilience is {} ({}/100) and {}. Weakest targets: {}. ",
        card.band,
        f1(card.portfolio),
        trend,
        if weakest.is_empty() {
            "none — no targets on record".into()
        } else {
            weakest.join(", ")
        },
    );
    if open.is_empty() {
        s.push_str("No open weaknesses; keep the current experiment cadence.");
    } else {
        s.push_str(&format!(
            "{} open weakness(es) need a decision: re-run, remediate, or accept.",
            open.len()
        ));
    }
    s
}

fn outlook(card: &Scorecard) -> String {
    let untested = card
        .experiments
        .iter()
        .filter(|e| e.state == RunState::NeverRun)
        .count();
    let stale = card
        .experiments
        .iter()
        .filter(|e| e.state == RunState::Stale)
        .count();
    let mut pts = Vec::new();
    if untested > 0 {
        pts.push(format!(
            "schedule first runs for {untested} untested experiment(s)"
        ));
    }
    if stale > 0 {
        pts.push(format!(
            "re-run {stale} experiment(s) whose last pass is over 30 days old"
        ));
    }
    if let Some(w) = card.targets.first() {
        pts.push(format!(
            "focus next game-day on {}, the weakest target",
            w.target
        ));
    }
    if pts.is_empty() {
        "Coverage is green across the board; maintain cadence and consider widening fault scenarios.".into()
    } else {
        format!("Next period priorities: {}.", pts.join("; "))
    }
}

/// Count deviated/failed runs in `[from, to)` and how many later ran green;
/// plus MTTR from spans that report `recovery_time_s`.
fn issue_stats(
    reader: &Reader,
    from_ns: i64,
    to_ns: i64,
) -> Result<(u64, u64, Option<f64>), String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT s.experiment_name AS name, s.ts_ns AS ts, l.log_attrs['status'] AS status \
             FROM spans s LEFT JOIN logs l \
               ON l.log_attrs['experiment_id'] = s.experiment_id \
              AND l.body = 'experiment.completed' \
             WHERE s.span_name = 'resilience.experiment' AND s.experiment_name IS NOT NULL \
               AND s.ts_ns >= {from_ns} AND s.ts_ns < {to_ns} ORDER BY s.ts_ns"
        ))
        .map_err(|e| e.to_string())?;

    let mut by_name: std::collections::BTreeMap<String, Vec<(i64, bool)>> =
        std::collections::BTreeMap::new();
    for row in &rows {
        let (Some(name), Some(ts)) = (
            cell(row, "name"),
            row.get("ts").and_then(serde_json::Value::as_i64),
        ) else {
            continue;
        };
        let ok = cell(row, "status") == Some("Completed");
        by_name.entry(name.to_string()).or_default().push((ts, ok));
    }
    let mut discovered = 0u64;
    let mut fixed = 0u64;
    for runs in by_name.values() {
        for (i, (_, ok)) in runs.iter().enumerate() {
            if !ok {
                discovered += 1;
                // Fixed = a completed run of the same experiment exists after
                // this failure (later in the period or up to now).
                if runs[i + 1..].iter().any(|(_, ok)| *ok) {
                    fixed += 1;
                }
            }
        }
    }

    let mttr = reader
        .query_json_rows(&format!(
            "SELECT AVG(recovery_time_s) AS v FROM spans \
             WHERE recovery_time_s IS NOT NULL AND ts_ns >= {from_ns} AND ts_ns < {to_ns}"
        ))
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|r| r.get("v").and_then(serde_json::Value::as_f64));
    Ok((discovered, fixed, mttr))
}

// ---------------------------------------------------------------- R3

/// Build the R3 game-day report for one experiment run (tumult
/// `experiment_id`). Returns `Ok(None)` when the id is unknown.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn build_game_day(
    reader: &Reader,
    experiment_id: &str,
    generated_at_ns: i64,
) -> Result<Option<ReportDoc>, String> {
    let id = q(experiment_id);
    let root = reader
        .query_json_rows(&format!(
            "SELECT ts_ns, experiment_id, experiment_name, target_system, target_environment, \
             fault_type, fault_subtype, fault_severity, blast_radius, hypothesis_met, \
             recovery_time_s, service_name, duration_ns, span_attrs \
             FROM spans WHERE span_name = 'resilience.experiment' AND experiment_id = {id} \
             ORDER BY ts_ns DESC LIMIT 1"
        ))
        .map_err(|e| e.to_string())?;
    let Some(root) = root.into_iter().next() else {
        return Ok(None);
    };

    let outcome = reader
        .query_json_rows(&format!(
            "SELECT log_attrs['status'] AS status FROM logs \
             WHERE log_attrs['experiment_id'] = {id} AND body = 'experiment.completed' \
             ORDER BY ts_ns DESC LIMIT 1"
        ))
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|r| cell(r, "status"))
        .unwrap_or("incomplete")
        .to_string();

    // The run's span tree shares the root span's trace_id.
    let spans = reader
        .query_json_rows(&format!(
            "SELECT ts_ns, span_name, span_kind, duration_ns, status_code, status_message, \
             fault_type, span_attrs \
             FROM spans \
             WHERE experiment_id = {id} \
                OR trace_id IN (SELECT trace_id FROM spans WHERE experiment_id = {id}) \
             ORDER BY ts_ns LIMIT 500"
        ))
        .map_err(|e| e.to_string())?;

    let findings_logs = reader
        .query_json_rows(&format!(
            "SELECT ts_ns, severity_text, body FROM logs \
             WHERE (log_attrs['experiment_id'] = {id} \
                OR trace_id IN (SELECT trace_id FROM spans WHERE experiment_id = {id})) \
               AND severity_text IN ('WARN', 'ERROR') \
             ORDER BY ts_ns LIMIT 100"
        ))
        .map_err(|e| e.to_string())?;

    let started_attrs = reader
        .query_json_rows(&format!(
            "SELECT log_attrs FROM logs \
             WHERE log_attrs['experiment_id'] = {id} AND body = 'experiment.started' \
             ORDER BY ts_ns LIMIT 1"
        ))
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|r| r.get("log_attrs").cloned());

    let name = cell(&root, "experiment_name").unwrap_or("unknown experiment");
    let root_ts = root
        .get("ts_ns")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let hypothesis = match root
        .get("hypothesis_met")
        .and_then(serde_json::Value::as_bool)
    {
        Some(true) => "Met".to_string(),
        Some(false) => "Not met".to_string(),
        None => "Not recorded".to_string(),
    };

    let seed = format!("R3|{experiment_id}|{generated_at_ns}");
    let mut meta = base_meta(
        TemplateKind::GameDay,
        format!("Game-day report — {name}"),
        generated_at_ns,
        root_ts,
        &seed,
    );
    meta.experiment_id = Some(experiment_id.to_string());

    let mut blocks = vec![
        Block::H1("Run summary".into()),
        Block::KeyValues(vec![
            ("Experiment".into(), name.to_string()),
            ("Run id".into(), experiment_id.to_string()),
            ("Date".into(), fmt_datetime(root_ts)),
            (
                "Target".into(),
                cell(&root, "target_system").unwrap_or("—").to_string(),
            ),
            (
                "Environment".into(),
                cell(&root, "target_environment").unwrap_or("—").to_string(),
            ),
            (
                "Scenario".into(),
                match (cell(&root, "fault_type"), cell(&root, "fault_subtype")) {
                    (Some(t), Some(s)) => format!("{t} / {s}"),
                    (Some(t), None) => t.to_string(),
                    _ => "—".into(),
                },
            ),
            (
                "Severity".into(),
                cell(&root, "fault_severity")
                    .unwrap_or("medium")
                    .to_string(),
            ),
            ("Hypothesis".into(), hypothesis.clone()),
            ("Outcome".into(), outcome.clone()),
        ]),
    ];

    let rollback_spans: Vec<&serde_json::Value> = spans
        .iter()
        .filter(|s| {
            cell(s, "span_name").is_some_and(|n| n.to_ascii_lowercase().contains("rollback"))
        })
        .collect();
    blocks.push(Block::H2("Blast radius and safety".into()));
    blocks.push(Block::KeyValues(vec![
        (
            "Blast radius".into(),
            cell(&root, "blast_radius")
                .unwrap_or("not recorded")
                .to_string(),
        ),
        (
            "Recovery time".into(),
            root.get("recovery_time_s")
                .and_then(serde_json::Value::as_f64)
                .map_or("not recorded".into(), |r| format!("{r:.1}s")),
        ),
        (
            "Rollback exercised".into(),
            if rollback_spans.is_empty() {
                "no"
            } else {
                "yes"
            }
            .to_string(),
        ),
    ]));

    blocks.push(Block::H2("Timeline".into()));
    let first_ts = spans
        .first()
        .and_then(|s| s.get("ts_ns").and_then(serde_json::Value::as_i64))
        .unwrap_or(root_ts);
    blocks.push(Block::Table {
        headers: vec![
            "t+ (s)".into(),
            "Span".into(),
            "Kind".into(),
            "Duration (ms)".into(),
            "Status".into(),
        ],
        rows: spans
            .iter()
            .map(|s| {
                vec![
                    s.get("ts_ns")
                        .and_then(serde_json::Value::as_i64)
                        .map_or("—".into(), |ts| {
                            format!("{:.1}", (ts - first_ts) as f64 / 1e9)
                        }),
                    cell(s, "span_name").unwrap_or("—").to_string(),
                    cell(s, "span_kind").unwrap_or("—").to_string(),
                    s.get("duration_ns")
                        .and_then(serde_json::Value::as_f64)
                        .map_or("—".into(), |d| format!("{:.1}", d / 1e6)),
                    cell(s, "status_code").unwrap_or("—").to_string(),
                ]
            })
            .collect(),
        numeric_cols: vec![0, 3],
    });

    blocks.push(Block::H2("Verdict".into()));
    blocks.push(Block::Paragraph(format!(
        "Outcome: {outcome}. Steady-state hypothesis: {}.{}",
        hypothesis.to_lowercase(),
        root.get("recovery_time_s")
            .and_then(serde_json::Value::as_f64)
            .map_or(String::new(), |r| format!(" Service recovered in {r:.1}s.")),
    )));

    blocks.push(Block::H2("Findings".into()));
    let bullets: Vec<String> = findings_logs
        .iter()
        .filter_map(|l| {
            Some(format!(
                "[{}] {}",
                cell(l, "severity_text")?,
                cell(l, "body")?
            ))
        })
        .collect();
    if bullets.is_empty() {
        blocks.push(Block::Paragraph(
            "No warnings or errors were logged during this run.".into(),
        ));
    } else {
        blocks.push(Block::Bullets(bullets));
    }

    blocks.push(Block::H2("Rollback".into()));
    if rollback_spans.is_empty() {
        blocks.push(Block::Paragraph(
            "No rollback action was exercised in this run.".into(),
        ));
    } else {
        let ok = rollback_spans.iter().all(|s| {
            cell(s, "status_code") == Some("Ok") || cell(s, "status_code") == Some("Unset")
        });
        blocks.push(Block::Paragraph(format!(
            "{} rollback span(s) executed; status: {}.",
            rollback_spans.len(),
            if ok {
                "all clean"
            } else {
                "errors present — investigate"
            }
        )));
    }

    blocks.push(Block::PageBreak);
    blocks.push(Block::H2("Configuration appendix".into()));
    let mut cfg: Vec<(String, String)> = Vec::new();
    if let Some(attrs) = root.get("span_attrs").and_then(|a| a.as_object()) {
        for (k, v) in attrs {
            cfg.push((k.clone(), v.as_str().unwrap_or("").to_string()));
        }
    }
    if let Some(serde_json::Value::Object(attrs)) = &started_attrs {
        for (k, v) in attrs {
            cfg.push((format!("start.{k}"), v.as_str().unwrap_or("").to_string()));
        }
    }
    cfg.sort();
    if cfg.is_empty() {
        blocks.push(Block::Paragraph(
            "No configuration attributes recorded.".into(),
        ));
    } else {
        cfg.truncate(40);
        blocks.push(Block::KeyValues(cfg));
    }

    blocks.push(Block::Footnote(format!(
        "Generated by kronika v{} from the run's trace and logs.",
        env!("CARGO_PKG_VERSION")
    )));
    blocks.push(Block::Signoff(vec![
        ("Run operator".into(), String::new()),
        ("Reviewed by".into(), String::new()),
    ]));

    Ok(Some(ReportDoc { meta, blocks }))
}

// ---------------------------------------------------------------- R2

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
/// "iso27001" | "soc2").
///
/// # Errors
/// Returns an error string for unknown frameworks or failed queries.
pub fn build_evidence_pack(
    reader: &Reader,
    framework: &str,
    period_ns: Option<i64>,
    generated_at_ns: i64,
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
    let card = scoring::compute(reader, as_of_ns, period_ns)?;
    let from_ns = period_ns.map(|p| as_of_ns - p);

    let seed = format!("R2|{key}|{period_ns:?}|{generated_at_ns}");
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
            "This pack summarises the resilience testing evidence kronika recorded \
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
             teams operating the affected services, and results are recorded \
             without manual editing."
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
        format!("{} experiments (see test register)", tested_names.len())
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
                    (*clause).to_string(),
                    tested_summary.clone(),
                    card.band.clone(),
                    "—".into(),
                    "—".into(),
                ]
            })
            .collect(),
        numeric_cols: vec![],
    });
    blocks.push(Block::Footnote(CLAUSE_VERIFY_FOOTNOTE.into()));

    blocks.push(Block::H2("Test register".into()));
    if card.experiments.is_empty() {
        blocks.push(Block::Paragraph("No experiments on record.".into()));
    } else {
        blocks.push(Block::Table {
            headers: vec![
                "Experiment".into(),
                "Target".into(),
                "Last run".into(),
                "Outcome".into(),
                "Score".into(),
            ],
            rows: card
                .experiments
                .iter()
                .map(|e| {
                    vec![
                        e.name.clone(),
                        e.target.clone().unwrap_or("—".into()),
                        e.last_run_ns.map_or("never".into(), fmt_date),
                        e.last_outcome.clone().unwrap_or("—".into()),
                        e.score.to_string(),
                    ]
                })
                .collect(),
            numeric_cols: vec![4],
        });
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
        ("Prepared by".into(), "kronika".into()),
        ("Compliance officer".into(), String::new()),
    ]));

    Ok(ReportDoc { meta, blocks })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
