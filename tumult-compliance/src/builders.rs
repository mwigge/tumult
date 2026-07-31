//! Report builders: store queries → [`ReportDoc`] for each template.
//!
//! All SQL values that flow from parameters go through [`q`] (single-quote
//! doubling). Numbers (`i64` timestamps) are formatted directly.

use sha2::{Digest, Sha256};
use tumult_lake::Reader;

use crate::html::{fmt_date, fmt_datetime};
use crate::model::{Block, Cell, ChartSpec, DocMeta, ReportDoc, TemplateKind};
use crate::org::{OrgTree, ScoredLeaf};
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
/// against `period_ns` earlier. `org` drives the "By domain" rollup section
/// (an empty tree collapses everything into `(unassigned)`). `envs` confines
/// every aggregate to the given environments (empty = unscoped); a scoped
/// principal gets a digest of its own environments only.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn build_executive(
    reader: &Reader,
    org: &OrgTree,
    as_of_ns: i64,
    period_ns: i64,
    generated_at_ns: i64,
    envs: &[String],
) -> Result<ReportDoc, String> {
    let card = scoring::compute_scoped(reader, as_of_ns, Some(period_ns), envs)?;
    let from_ns = as_of_ns - period_ns;

    let (discovered, fixed, mttr_s) = issue_stats(reader, from_ns, as_of_ns, envs)?;
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

    // The scope set is part of the document identity: a scoped digest is a
    // different document from the global one for the same instant.
    let seed = format!(
        "R1|{}|{from_ns}|{as_of_ns}|{generated_at_ns}",
        envs.join(",")
    );
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

    // Score trend across the period (portfolio sampled at 10 instants).
    let trend = scoring::portfolio_series_scoped(reader, as_of_ns, period_ns, 10, envs)?;
    blocks.push(Block::H2("Score trend".into()));
    blocks.push(Block::Chart(ChartSpec::Lines(vec![(
        "portfolio".into(),
        trend,
    )])));

    // Per-experiment bars, weakest first (experiments are sorted by score).
    blocks.push(Block::H2("Experiment scores".into()));
    blocks.push(Block::Chart(ChartSpec::Bars(
        card.experiments
            .iter()
            .map(|e| (e.name.clone(), f64::from(e.score)))
            .collect(),
    )));

    // By domain: top-level org rollup (criticality-weighted mean over all
    // leaves in each subtree; pending manual records count toward coverage).
    let mut leaves: Vec<ScoredLeaf> = card
        .experiments
        .iter()
        .map(|e| ScoredLeaf {
            name: e.name.clone(),
            score: Some(e.score),
        })
        .collect();
    leaves.extend(
        scoring::pending_manual_leaves_scoped(reader, envs)?
            .into_iter()
            .map(|name| ScoredLeaf { name, score: None }),
    );
    blocks.push(Block::H2("By domain".into()));
    if let Some(root) = org.compute_node("", &leaves) {
        if root.children.is_empty() {
            blocks.push(Block::Paragraph("No org structure on record.".into()));
        } else {
            blocks.push(Block::Table {
                headers: vec![
                    "Domain".into(),
                    "Score".into(),
                    "Band".into(),
                    "Coverage".into(),
                    "Weakest member".into(),
                ],
                rows: root
                    .children
                    .iter()
                    .map(|c| {
                        vec![
                            Cell::text(c.name.clone()),
                            Cell::text(f1(c.score)),
                            Cell::status(c.band.clone()),
                            Cell::text(format!("{}/{}", c.scored, c.expected)),
                            Cell::text(c.weakest.clone().unwrap_or("—".into())),
                        ]
                    })
                    .collect(),
                numeric_cols: vec![1],
                widths: Some(vec![2.6, 0.8, 1.0, 1.0, 2.2]),
            });
        }
    }

    // Coverage: experiments with ≥ 1 run inside the window vs not.
    let in_window = runs_in_window(reader, from_ns, as_of_ns, envs)?;
    let tested_in_window = card
        .experiments
        .iter()
        .filter(|e| in_window.contains(&e.name))
        .count() as f64;
    let not_run = card.experiments.len() as f64 - tested_in_window;
    blocks.push(Block::H2("Coverage".into()));
    if card.experiments.is_empty() {
        blocks.push(Block::Paragraph("No experiments on record.".into()));
    } else {
        blocks.push(Block::Chart(ChartSpec::Donut(vec![
            ("Tested in window".into(), tested_in_window),
            ("Not run in window".into(), not_run),
        ])));
    }

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
                    Cell::text(t.target.clone()),
                    Cell::text(
                        card.experiments
                            .iter()
                            .filter(|e| e.target.as_deref().unwrap_or("(untargeted)") == t.target)
                            .count()
                            .to_string(),
                    ),
                    Cell::text(t.runs.to_string()),
                    Cell::text(f1(t.score)),
                    Cell::status(t.band.clone()),
                ]
            })
            .collect(),
        numeric_cols: vec![1, 2, 3],
        widths: Some(vec![3.0, 1.1, 0.8, 0.8, 1.0]),
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

    blocks.push(Block::H2("Open weaknesses and decisions needed".into()));
    if open.is_empty() {
        blocks.push(Block::Paragraph(
            "No open weaknesses: every known experiment last ran green.".into(),
        ));
    } else {
        blocks.push(Block::Table {
            headers: vec![
                "Severity".into(),
                "Experiment".into(),
                "Last run".into(),
                "Age (d)".into(),
                "Decision needed".into(),
            ],
            rows: open
                .iter()
                .map(|e| {
                    vec![
                        Cell::status(e.severity.clone().unwrap_or_else(|| "medium".into())),
                        Cell::text(e.name.clone()),
                        Cell::text(e.last_run_ns.map_or("never".into(), fmt_date)),
                        Cell::text(
                            e.last_run_ns
                                .map_or("—".into(), |ts| ((as_of_ns - ts) / DAY_NS).to_string()),
                        ),
                        Cell::text(decision_for(e.state)),
                    ]
                })
                .collect(),
            numeric_cols: vec![3],
            widths: Some(vec![1.2, 3.0, 1.0, 0.7, 2.0]),
        });
    }

    blocks.push(Block::H2("Outlook".into()));
    blocks.push(Block::Paragraph(outlook(&card)));

    let n_manual = card
        .experiments
        .iter()
        .filter(|e| e.origin == "manual")
        .count();
    let n_automated = card.experiments.len() - n_manual;
    blocks.push(Block::Footnote(format!(
        "Scores: passed 100, stale pass (>30d) 75, failed 50, never run 0; bands \
         >70 good, 50–70 fair, <50 poor. Evidence mix: {n_automated} automated, \
         {n_manual} verified manual experiment(s) (manual partials score 75; \
         inconclusive manual outcomes are excluded). Generated by Tumult v{}; \
         data as of {}.",
        env!("CARGO_PKG_VERSION"),
        fmt_date(as_of_ns)
    )));
    blocks.push(Block::Signoff(vec![
        ("Prepared by".into(), "Tumult".into()),
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
/// The decision an open weakness asks of the reader, by run state.
fn decision_for(state: RunState) -> String {
    match state {
        RunState::Failed => "Re-run, remediate, or accept".into(),
        RunState::Partial => "Re-run to a full pass, or accept".into(),
        RunState::Stale => "Re-run to refresh evidence".into(),
        RunState::NeverRun => "Schedule first run".into(),
        RunState::Passed => "—".into(),
    }
}

/// Experiment names with at least one run in `[from_ns, to_ns)` — automated
/// runs plus verified manual executions in the window. Scoped to `envs`
/// (empty = unscoped).
fn runs_in_window(
    reader: &Reader,
    from_ns: i64,
    to_ns: i64,
    envs: &[String],
) -> Result<std::collections::BTreeSet<String>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT DISTINCT experiment_name AS name FROM spans \
             WHERE span_name = 'resilience.experiment' AND experiment_name IS NOT NULL \
               AND ts_ns >= {from_ns} AND ts_ns < {to_ns}{} \
             UNION \
             SELECT DISTINCT experiment_name FROM manual_experiments \
             WHERE status = 'verified' \
               AND executed_at_ns >= {from_ns} AND executed_at_ns < {to_ns}{}",
            scoring::and_env(scoring::env_in("target_environment", envs)),
            scoring::and_env(scoring::env_in("target_environment", envs)),
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| cell(r, "name").map(str::to_owned))
        .collect())
}

fn issue_stats(
    reader: &Reader,
    from_ns: i64,
    to_ns: i64,
    envs: &[String],
) -> Result<(u64, u64, Option<f64>), String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT s.experiment_name AS name, s.ts_ns AS ts, l.log_attrs['status'] AS status \
             FROM spans s LEFT JOIN logs l \
               ON l.log_attrs['experiment_id'] = s.experiment_id \
              AND l.body = 'experiment.completed' \
             WHERE s.span_name = 'resilience.experiment' AND s.experiment_name IS NOT NULL \
               AND s.ts_ns >= {from_ns} AND s.ts_ns < {to_ns}{} ORDER BY s.ts_ns",
            scoring::and_env(scoring::env_in("s.target_environment", envs)),
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
             WHERE recovery_time_s IS NOT NULL AND ts_ns >= {from_ns} AND ts_ns < {to_ns}{}",
            scoring::and_env(scoring::env_in("target_environment", envs)),
        ))
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|r| r.get("v").and_then(serde_json::Value::as_f64));
    Ok((discovered, fixed, mttr))
}

// ---------------------------------------------------------------- R3

/// Build the R3 game-day report for one experiment run (tumult
/// `experiment_id`). Returns `Ok(None)` when the id is unknown — or when
/// `envs` is scoped and the run's root span sits outside those environments
/// (no existence leak across scopes). Child spans and logs correlate
/// through the run's trace, so confining the root span confines the report.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn build_game_day(
    reader: &Reader,
    experiment_id: &str,
    generated_at_ns: i64,
    envs: &[String],
) -> Result<Option<ReportDoc>, String> {
    let id = q(experiment_id);
    let root = reader
        .query_json_rows(&format!(
            "SELECT ts_ns, experiment_id, experiment_name, target_system, target_environment, \
             fault_type, fault_subtype, fault_severity, blast_radius, hypothesis_met, \
             recovery_time_s, service_name, duration_ns, span_attrs \
             FROM spans WHERE span_name = 'resilience.experiment' AND experiment_id = {id}{} \
             ORDER BY ts_ns DESC LIMIT 1",
            scoring::and_env(scoring::env_in("target_environment", envs)),
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
            ("Experiment".into(), Cell::text(name)),
            ("Run id".into(), Cell::text(experiment_id)),
            ("Date".into(), Cell::text(fmt_datetime(root_ts))),
            (
                "Target".into(),
                Cell::text(cell(&root, "target_system").unwrap_or("—")),
            ),
            (
                "Environment".into(),
                Cell::text(cell(&root, "target_environment").unwrap_or("—")),
            ),
            (
                "Scenario".into(),
                Cell::text(
                    match (cell(&root, "fault_type"), cell(&root, "fault_subtype")) {
                        (Some(t), Some(s)) => format!("{t} / {s}"),
                        (Some(t), None) => t.to_string(),
                        _ => "—".into(),
                    },
                ),
            ),
            (
                "Severity".into(),
                Cell::status(cell(&root, "fault_severity").unwrap_or("medium")),
            ),
            (
                "Hypothesis".into(),
                if hypothesis == "Not recorded" {
                    Cell::text(hypothesis.clone())
                } else {
                    Cell::status(hypothesis.clone())
                },
            ),
            ("Outcome".into(), Cell::status(outcome.clone())),
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
            Cell::text(cell(&root, "blast_radius").unwrap_or("not recorded")),
        ),
        (
            "Recovery time".into(),
            Cell::text(
                root.get("recovery_time_s")
                    .and_then(serde_json::Value::as_f64)
                    .map_or("not recorded".into(), |r| format!("{r:.1}s")),
            ),
        ),
        (
            "Rollback exercised".into(),
            Cell::text(if rollback_spans.is_empty() {
                "no"
            } else {
                "yes"
            }),
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
                    Cell::text(
                        s.get("ts_ns")
                            .and_then(serde_json::Value::as_i64)
                            .map_or("—".into(), |ts| {
                                format!("{:.1}", (ts - first_ts) as f64 / 1e9)
                            }),
                    ),
                    Cell::text(cell(s, "span_name").unwrap_or("—")),
                    Cell::text(cell(s, "span_kind").unwrap_or("—")),
                    Cell::text(
                        s.get("duration_ns")
                            .and_then(serde_json::Value::as_f64)
                            .map_or("—".into(), |d| format!("{:.1}", d / 1e6)),
                    ),
                    // OTel codes → readable status: Unset is "no status".
                    match cell(s, "status_code") {
                        Some("Ok") => Cell::status("ok"),
                        Some("Error") => Cell::status("error"),
                        _ => Cell::text("—"),
                    },
                ]
            })
            .collect(),
        numeric_cols: vec![0, 3],
        widths: Some(vec![0.7, 2.6, 0.9, 1.0, 0.9]),
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
    let mut cfg: Vec<(String, Cell)> = Vec::new();
    if let Some(attrs) = root.get("span_attrs").and_then(|a| a.as_object()) {
        for (k, v) in attrs {
            cfg.push((k.clone(), Cell::text(v.as_str().unwrap_or(""))));
        }
    }
    if let Some(serde_json::Value::Object(attrs)) = &started_attrs {
        for (k, v) in attrs {
            cfg.push((format!("start.{k}"), Cell::text(v.as_str().unwrap_or(""))));
        }
    }
    cfg.sort_by(|a, b| a.0.cmp(&b.0));
    if cfg.is_empty() {
        blocks.push(Block::Paragraph(
            "No configuration attributes recorded.".into(),
        ));
    } else {
        cfg.truncate(40);
        blocks.push(Block::KeyValues(cfg));
    }

    blocks.push(Block::Footnote(format!(
        "Generated by Tumult v{} from the run's trace and logs.",
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
        let doc = build_evidence_pack(&reader, "soc2", Some(DAY_NS + 12 * HOUR_NS - 1), now, &[])
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

        let scoped =
            build_evidence_pack(&reader, "soc2", None, now, &["staging".to_string()]).unwrap();
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
}
