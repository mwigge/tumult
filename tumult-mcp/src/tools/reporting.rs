//! Reporting tools: journal reports, compliance summaries, and metric trends.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::StructuredReport;

use tumult_core::compliance::{
    compliance_verdict, ComplianceFramework, ComplianceSignals, DEFAULT_MTTR_TARGET_S,
    EVIDENCE_DISCLAIMER,
};
use tumult_core::journal::read_journal;
use tumult_core::types::Journal;

/// Metrics supported by [`trend`], in the same set as `tumult trend`.
const TREND_METRICS: &[&str] = &[
    "resilience_score",
    "duration_ms",
    "estimate_accuracy",
    "method_step_count",
];

/// Render a journal as a `json` or `junit` report (mirrors `tumult report`;
/// HTML/PDF remain CLI-only).
///
/// With `output_path` (already resolved/contained by the caller) the report
/// is written to disk and the structured object carries `output_path`;
/// otherwise the content is returned inline in both the text and the
/// structured `content` field, capped at 512 KiB.
///
/// # Errors
///
/// Returns a [`ToolError`] if the format is unknown, the journal cannot be
/// read or serialized, or the output file cannot be written.
pub fn report(
    journal_path: &str,
    format: &str,
    output_path: Option<&Path>,
) -> Result<StructuredReport, ToolError> {
    if format != "json" && format != "junit" {
        return Err(ToolError::InvalidInput(format!(
            "unsupported report format '{format}'; valid values: json, junit \
             (html/pdf are only available via the tumult CLI)"
        )));
    }

    let journal = read_journal(Path::new(journal_path))
        .map_err(|e| ToolError::Parse(format!("failed to read journal: {e}")))?;

    let content = if format == "junit" {
        tumult_core::report::junit_report(&journal)
    } else {
        tumult_core::report::json_report(&journal)
            .map_err(|e| ToolError::Execution(e.to_string()))?
    };

    let mut structured = serde_json::Map::new();
    structured.insert("format".into(), serde_json::json!(format));

    if let Some(out) = output_path {
        std::fs::write(out, &content)
            .map_err(|e| ToolError::Execution(format!("failed to write report: {e}")))?;
        let out_str = out.display().to_string();
        structured.insert("output_path".into(), serde_json::json!(out_str));
        return Ok(StructuredReport {
            text: format!("Report generated: {out_str}"),
            structured,
        });
    }

    let capped = crate::tools::cap_text(content, "pass output_path to persist the full report");
    structured.insert("content".into(), serde_json::json!(capped));
    Ok(StructuredReport {
        text: capped,
        structured,
    })
}

/// Read every journal under `journals_path` (a `.toon` file or a directory
/// of them, non-recursive — CLI parity), invoking `on_journal` per journal.
/// Returns `(loaded, skipped)` counts; unreadable files are skipped.
fn for_each_journal(
    journals_path: &str,
    mut on_journal: impl FnMut(Journal),
) -> Result<(usize, usize), ToolError> {
    let path = Path::new(journals_path);
    let mut loaded = 0usize;
    let mut skipped = 0usize;

    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry_path = entry?.path();
            if entry_path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&entry_path) {
                    Ok(journal) => {
                        on_journal(journal);
                        loaded += 1;
                    }
                    Err(_) => skipped += 1,
                }
            }
        }
    } else if path.is_file() {
        let journal = read_journal(path)
            .map_err(|e| ToolError::Parse(format!("failed to read journal: {e}")))?;
        on_journal(journal);
        loaded = 1;
    } else {
        return Err(ToolError::NotFound(format!(
            "path does not exist: {journals_path}"
        )));
    }

    Ok((loaded, skipped))
}

/// Compliance summary over journals for a target framework (mirrors
/// `tumult compliance`, sharing its scoring via `tumult_core::compliance`).
///
/// # Errors
///
/// Returns a [`ToolError`] if the framework is unknown (listing the valid
/// values) or the journals cannot be read.
pub fn compliance(journals_path: &str, framework: &str) -> Result<StructuredReport, ToolError> {
    let framework = ComplianceFramework::parse(framework).map_err(ToolError::InvalidInput)?;

    let mut signals = ComplianceSignals::default();
    let (loaded, skipped) = for_each_journal(journals_path, |journal| {
        signals.accumulate(&journal);
    })?;

    let pass_rate = signals.pass_rate();
    let recovery_compliance = signals.recovery_compliance(DEFAULT_MTTR_TARGET_S);
    let verdict = compliance_verdict(pass_rate, recovery_compliance);

    // Citations from the single sourced, dated registry in
    // `tumult_core::compliance` — the same source of truth the CLI renders.
    let citations: Vec<serde_json::Value> = framework
        .citations()
        .iter()
        .map(|c| {
            serde_json::json!({
                "control_id": c.control_id,
                "title": c.title,
                "requires": c.summary,
                "evidence_type": c.evidence_type.as_str(),
                "strength": c.strength.as_str(),
                "evidence_note": c.evidence_note,
                "source_url": c.source_url,
                "last_verified": c.last_verified,
            })
        })
        .collect();

    let mut structured = serde_json::Map::new();
    structured.insert(
        "framework".into(),
        serde_json::json!(framework.as_report_str()),
    );
    structured.insert("pass_rate".into(), serde_json::json!(pass_rate));
    structured.insert(
        "recovery_compliance".into(),
        serde_json::json!(recovery_compliance),
    );
    structured.insert("verdict".into(), serde_json::json!(verdict));
    structured.insert("journals_evaluated".into(), serde_json::json!(loaded));
    structured.insert("disclaimer".into(), serde_json::json!(EVIDENCE_DISCLAIMER));
    structured.insert(
        "source_url".into(),
        serde_json::json!(framework.source_url()),
    );
    structured.insert("citations".into(), serde_json::json!(citations));

    let mut text = String::new();
    writeln!(text, "=== {} ===", framework.full_name()).ok();
    writeln!(text).ok();
    writeln!(text, "{EVIDENCE_DISCLAIMER}").ok();
    writeln!(text).ok();
    writeln!(text, "Journals analyzed: {loaded}").ok();
    if skipped > 0 {
        writeln!(text, "Skipped (unreadable): {skipped}").ok();
    }
    writeln!(
        text,
        "With regulatory tagging: {}",
        signals.journals_with_regulatory
    )
    .ok();
    writeln!(text).ok();
    writeln!(text, "Evidence summary (NOT a compliance determination):").ok();
    writeln!(text, "  Pass rate: {:.1}%", pass_rate * 100.0).ok();
    if let Some(rc) = recovery_compliance {
        writeln!(
            text,
            "  Recovery compliance: {:.1}% (MTTR<={DEFAULT_MTTR_TARGET_S}s, or avg resilience proxy)",
            rc * 100.0
        )
        .ok();
    } else {
        writeln!(
            text,
            "  Recovery compliance: N/A — no MTTR or resilience_score present in journals;"
        )
        .ok();
        writeln!(
            text,
            "  verdict based on pass rate ONLY (reduced assurance)."
        )
        .ok();
    }
    writeln!(text, "  Evidence verdict: {verdict}").ok();

    writeln!(text).ok();
    writeln!(text, "Source: {}", framework.source_url()).ok();
    writeln!(
        text,
        "Mapped controls (evidence toward, not proof of, compliance):"
    )
    .ok();
    for c in framework.citations() {
        writeln!(text, "  {} — {}", c.control_id, c.title).ok();
        writeln!(
            text,
            "    Evidence [{} / {}]: {}",
            c.strength.as_str(),
            c.evidence_type.as_str(),
            c.evidence_note
        )
        .ok();
        writeln!(
            text,
            "    Source: {} (last verified {})",
            c.source_url, c.last_verified
        )
        .ok();
    }

    Ok(StructuredReport { text, structured })
}

/// A single trend data point.
struct TrendPoint {
    ts: i64,
    value: f64,
}

/// Cross-run metric trend over journals (mirrors `tumult trend`): ingests
/// the journals into an in-memory analytics store and returns time-ordered
/// `{ts, value}` points plus a direction verdict.
///
/// # Errors
///
/// Returns a [`ToolError`] if the metric or `last` window is invalid, the
/// journals cannot be read, or the analytics query fails.
pub fn trend(
    journals_path: &str,
    metric: &str,
    last: Option<&str>,
    target: Option<&str>,
) -> Result<StructuredReport, ToolError> {
    if !TREND_METRICS.contains(&metric) {
        return Err(ToolError::InvalidInput(format!(
            "unknown metric '{metric}'; valid values: {}",
            TREND_METRICS.join(", ")
        )));
    }

    // Parse `last` ("30d" or "30") into a nanosecond cutoff filter.
    let time_filter = if let Some(window) = last {
        let days: i64 = window.trim_end_matches('d').parse().map_err(|_| {
            ToolError::InvalidInput(format!(
                "last must be a number of days (e.g., 30d), got: {window}"
            ))
        })?;
        let now_ns = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        )
        .unwrap_or(i64::MAX);
        let cutoff_ns = now_ns.saturating_sub(days.saturating_mul(86_400 * 1_000_000_000));
        format!(" AND started_at_ns >= {cutoff_ns}")
    } else {
        String::new()
    };

    let store = tumult_analytics::AnalyticsStore::in_memory()
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let (loaded, skipped) = for_each_journal(journals_path, |journal| {
        // Ingest failures surface as missing points, not tool failures.
        let _ = store.ingest_journal(&journal);
    })?;

    // Pre-built queries keyed by metric — no interpolation of user input.
    let base_sql = match metric {
        "resilience_score" => "SELECT started_at_ns, resilience_score FROM experiments WHERE resilience_score IS NOT NULL",
        "duration_ms" => "SELECT started_at_ns, duration_ms FROM experiments WHERE duration_ms IS NOT NULL",
        "estimate_accuracy" => "SELECT started_at_ns, estimate_accuracy FROM experiments WHERE estimate_accuracy IS NOT NULL",
        "method_step_count" => "SELECT started_at_ns, method_step_count FROM experiments WHERE method_step_count IS NOT NULL",
        _ => unreachable!("validated above"),
    };
    // Bind the LIKE pattern as a query parameter to prevent SQL injection.
    let target_filter = if target.is_some() {
        " AND lower(title) LIKE ?"
    } else {
        ""
    };
    let sql = format!("{base_sql}{time_filter}{target_filter} ORDER BY started_at_ns");

    let rows = if let Some(t) = target {
        let like_pattern = format!("%{}%", t.to_lowercase());
        store
            .query_with_param(&sql, &like_pattern)
            .map_err(|e| ToolError::Store(e.to_string()))?
    } else {
        store
            .query(&sql)
            .map_err(|e| ToolError::Store(e.to_string()))?
    };

    let points: Vec<TrendPoint> = rows
        .iter()
        .filter_map(|row| {
            let ts = row.first()?.parse::<i64>().ok()?;
            let value = row.get(1)?.parse::<f64>().ok()?;
            Some(TrendPoint { ts, value })
        })
        .collect();

    let verdict = trend_verdict(metric, &points);

    let mut structured = serde_json::Map::new();
    structured.insert("metric".into(), serde_json::json!(metric));
    structured.insert(
        "points".into(),
        serde_json::Value::Array(
            points
                .iter()
                .map(|p| serde_json::json!({ "ts": p.ts, "value": p.value }))
                .collect(),
        ),
    );
    structured.insert("target".into(), serde_json::json!(target));
    structured.insert("verdict".into(), serde_json::json!(verdict));

    let mut text = String::new();
    writeln!(text, "Loaded {loaded} journal(s)").ok();
    if skipped > 0 {
        writeln!(text, "Skipped (unreadable): {skipped}").ok();
    }
    writeln!(text).ok();
    if points.is_empty() {
        writeln!(text, "No data points for metric: {metric}").ok();
    } else {
        writeln!(text, "Trend: {} ({} data points)", metric, points.len()).ok();
        for point in &points {
            writeln!(text, "  {}  {}", point.ts, point.value).ok();
        }
        let (min, max, sum) = points.iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY, 0.0_f64),
            |(min, max, sum), p| (min.min(p.value), max.max(p.value), sum + p.value),
        );
        #[allow(clippy::cast_precision_loss)]
        let avg = sum / points.len() as f64;
        writeln!(
            text,
            "\nSummary: {} runs, min={min}, max={max}, avg={avg}",
            points.len()
        )
        .ok();
    }
    writeln!(text, "Verdict: {verdict}").ok();

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "narrow with last/target to reduce points"),
        structured,
    })
}

/// Direction verdict comparing the mean of the later half of the series to
/// the earlier half (5% threshold).
///
/// `resilience_score` / `estimate_accuracy` are higher-is-better and
/// `duration_ms` is lower-is-better, mapping to `improving` / `declining` /
/// `stable`; `method_step_count` has no quality direction and maps to
/// `increasing` / `decreasing` / `stable`. Fewer than two points yields
/// `insufficient-data`.
fn trend_verdict(metric: &str, points: &[TrendPoint]) -> &'static str {
    /// Relative change below which the series counts as stable.
    const THRESHOLD: f64 = 0.05;

    if points.len() < 2 {
        return "insufficient-data";
    }
    let mid = points.len() / 2;
    #[allow(clippy::cast_precision_loss)]
    let mean =
        |slice: &[TrendPoint]| slice.iter().map(|p| p.value).sum::<f64>() / slice.len() as f64;
    let early = mean(&points[..mid]);
    let late = mean(&points[mid..]);
    let change = if early.abs() < f64::EPSILON {
        if late.abs() < f64::EPSILON {
            0.0
        } else {
            1.0
        }
    } else {
        (late - early) / early.abs()
    };

    if change.abs() <= THRESHOLD {
        return "stable";
    }
    let rising = change > 0.0;
    match metric {
        // No quality direction — report the raw movement.
        "method_step_count" => {
            if rising {
                "increasing"
            } else {
                "decreasing"
            }
        }
        // duration_ms: lower is better; everything else: higher is better.
        "duration_ms" => {
            if rising {
                "declining"
            } else {
                "improving"
            }
        }
        _ => {
            if rising {
                "improving"
            } else {
                "declining"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::write_run_journal;
    use tempfile::TempDir;

    // ── report ────────────────────────────────────────────────

    #[test]
    fn report_rejects_unknown_format() {
        let err = report("journal.toon", "html", None).expect_err("html must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("html"), "must name the bad value: {msg}");
        assert!(
            msg.contains("json") && msg.contains("junit"),
            "must list valid values: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_json_inline_returns_journal_json() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = report(journal_path.to_str().unwrap(), "json", None).unwrap();
        assert_eq!(result.structured["format"], "json");
        let content = result.structured["content"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content).unwrap();
        assert_eq!(parsed["experiment_title"], "MCP test experiment");
        assert_eq!(result.text, content);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_junit_writes_output_file() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());
        let out = dir.path().join("report.xml");

        let result = report(journal_path.to_str().unwrap(), "junit", Some(&out)).unwrap();
        assert!(out.exists(), "report file must be written");
        assert_eq!(
            result.structured["output_path"],
            out.display().to_string().as_str()
        );
        assert!(!result.structured.contains_key("content"));
        let xml = std::fs::read_to_string(&out).unwrap();
        assert!(xml.contains("<testsuite name=\"MCP test experiment\""));
    }

    // ── compliance ────────────────────────────────────────────

    #[test]
    fn compliance_rejects_unknown_framework_listing_values() {
        let err = compliance(".", "hipaa").expect_err("unknown framework must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("hipaa"), "must name the bad value: {msg}");
        assert!(
            msg.contains("dora") && msg.contains("basel-iii"),
            "must list valid values: {msg}"
        );
    }

    #[test]
    fn compliance_missing_path_is_not_found() {
        let err = compliance("/nonexistent/journals", "dora").expect_err("missing path");
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn compliance_over_single_completed_journal() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = compliance(journal_path.to_str().unwrap(), "dora").unwrap();
        assert_eq!(result.structured["framework"], "DORA");
        assert_eq!(result.structured["journals_evaluated"], 1);
        let pass_rate = result.structured["pass_rate"].as_f64().unwrap();
        assert!((pass_rate - 1.0).abs() < f64::EPSILON);
        let verdict = result.structured["verdict"].as_str().unwrap();
        assert!(
            verdict.starts_with("COMPLIANT"),
            "one completed journal must be compliant: {verdict}"
        );
        assert!(result.text.contains("Digital Operational Resilience Act"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn compliance_over_directory_skips_non_journals() {
        let dir = TempDir::new().unwrap();
        write_run_journal(dir.path());
        // The experiment definition also sits in the dir and is not a journal.
        let result = compliance(dir.path().to_str().unwrap(), "soc2").unwrap();
        assert_eq!(result.structured["journals_evaluated"], 1);
        assert!(result.text.contains("Skipped (unreadable): 1"));
    }

    // ── trend ─────────────────────────────────────────────────

    #[test]
    fn trend_rejects_unknown_metric_listing_values() {
        let err = trend(".", "latency", None, None).expect_err("unknown metric");
        let msg = err.to_string();
        assert!(msg.contains("latency"), "must name the bad value: {msg}");
        for metric in TREND_METRICS {
            assert!(msg.contains(metric), "must list '{metric}': {msg}");
        }
    }

    #[test]
    fn trend_rejects_malformed_last_window() {
        let err = trend(".", "duration_ms", Some("soon"), None).expect_err("bad window");
        assert!(err.to_string().contains("number of days"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trend_duration_over_journal_returns_points() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = trend(journal_path.to_str().unwrap(), "duration_ms", None, None).unwrap();
        assert_eq!(result.structured["metric"], "duration_ms");
        let points = result.structured["points"].as_array().unwrap();
        assert_eq!(points.len(), 1);
        assert!(points[0]["ts"].as_i64().is_some());
        assert!(points[0]["value"].as_f64().is_some());
        assert_eq!(result.structured["verdict"], "insufficient-data");
        assert_eq!(result.structured["target"], serde_json::Value::Null);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trend_target_filter_excludes_non_matching_titles() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = trend(
            journal_path.to_str().unwrap(),
            "duration_ms",
            None,
            Some("no-such-experiment"),
        )
        .unwrap();
        assert!(result.structured["points"].as_array().unwrap().is_empty());
        assert!(result.text.contains("No data points"));
    }

    // ── trend_verdict ─────────────────────────────────────────

    fn pts(values: &[f64]) -> Vec<TrendPoint> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| TrendPoint {
                ts: i64::try_from(i).unwrap(),
                value: *v,
            })
            .collect()
    }

    #[test]
    fn trend_verdict_directions() {
        assert_eq!(
            trend_verdict("resilience_score", &pts(&[])),
            "insufficient-data"
        );
        assert_eq!(
            trend_verdict("resilience_score", &pts(&[0.5, 0.9])),
            "improving"
        );
        assert_eq!(
            trend_verdict("resilience_score", &pts(&[0.9, 0.5])),
            "declining"
        );
        assert_eq!(
            trend_verdict("resilience_score", &pts(&[0.8, 0.8])),
            "stable"
        );
        // Lower duration is better.
        assert_eq!(
            trend_verdict("duration_ms", &pts(&[100.0, 50.0])),
            "improving"
        );
        assert_eq!(
            trend_verdict("duration_ms", &pts(&[50.0, 100.0])),
            "declining"
        );
        // Step count is directionless.
        assert_eq!(
            trend_verdict("method_step_count", &pts(&[2.0, 4.0])),
            "increasing"
        );
        assert_eq!(
            trend_verdict("method_step_count", &pts(&[4.0, 2.0])),
            "decreasing"
        );
    }
}
