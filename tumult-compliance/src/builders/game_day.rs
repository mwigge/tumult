//! The R3 game-day report for one experiment run.

use tumult_lake::Reader;

use super::{base_meta, cell, q};
use crate::html::fmt_datetime;
use crate::model::{Block, Cell, ReportDoc, TemplateKind};
use crate::scoring;

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
