use std::fmt::Write as _;

use tumult_core::types::{ActivityResult, ActivityStatus, ExperimentStatus, Journal};

use super::escape::html_escape;

#[allow(clippy::too_many_lines)] // HTML report generation embeds styles, scripts, and data in one pass; splitting would not improve clarity
pub(crate) fn generate_html_report(
    journal: &Journal,
    trace_ui_base: Option<&str>,
    source_hash: &str,
) -> String {
    let status_class = match journal.status {
        ExperimentStatus::Completed => "success",
        ExperimentStatus::Deviated => "warning",
        _ => "error",
    };

    let mut activities_html = String::new();

    // Hypothesis before
    if let Some(ref hyp) = journal.steady_state_before {
        let _ = write!(
            activities_html,
            r#"<tr class="phase-header"><td colspan="7">Hypothesis Before: {} ({})</td></tr>"#,
            hyp.title,
            if hyp.met { "MET" } else { "NOT MET" }
        );
        for r in &hyp.probe_results {
            activities_html += &format_activity_row(r, "hypothesis_before", trace_ui_base);
        }
    }

    // Method
    if !journal.method_results.is_empty() {
        activities_html += r#"<tr class="phase-header"><td colspan="7">Method</td></tr>"#;
        for r in &journal.method_results {
            activities_html += &format_activity_row(r, "method", trace_ui_base);
        }
    }

    // Hypothesis after
    if let Some(ref hyp) = journal.steady_state_after {
        let _ = write!(
            activities_html,
            r#"<tr class="phase-header"><td colspan="7">Hypothesis After: {} ({})</td></tr>"#,
            hyp.title,
            if hyp.met { "MET" } else { "NOT MET" }
        );
        for r in &hyp.probe_results {
            activities_html += &format_activity_row(r, "hypothesis_after", trace_ui_base);
        }
    }

    // Rollbacks
    if !journal.rollback_results.is_empty() {
        activities_html += r#"<tr class="phase-header"><td colspan="7">Rollbacks</td></tr>"#;
        for r in &journal.rollback_results {
            activities_html += &format_activity_row(r, "rollback", trace_ui_base);
        }
    }

    // Analysis section
    let analysis_html = if let Some(ref a) = journal.analysis {
        format!(
            r#"<div class="section">
            <h2>Analysis</h2>
            <table>
                <tr><td>Estimate Accuracy</td><td>{}</td></tr>
                <tr><td>Resilience Score</td><td>{}</td></tr>
                <tr><td>Trend</td><td>{}</td></tr>
            </table>
            </div>"#,
            a.estimate_accuracy
                .map_or("N/A".into(), |v| format!("{:.1}%", v * 100.0)),
            a.resilience_score
                .map_or("N/A".into(), |v| format!("{v:.2}")),
            a.trend
                .as_ref()
                .map_or("N/A".into(), std::string::ToString::to_string),
        )
    } else {
        String::new()
    };

    // Regulatory section
    let regulatory_html = if let Some(ref reg) = journal.regulatory {
        format!(
            r#"<div class="section">
            <h2>Regulatory Mapping</h2>
            <p>Frameworks: {}</p>
            </div>"#,
            reg.frameworks.join(", ")
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Tumult Report: {title}</title>
<style>
  body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; margin: 2em; color: #1a1a2e; background: #f8f9fa; }}
  h1 {{ color: #16213e; border-bottom: 3px solid #0f3460; padding-bottom: 0.5em; }}
  h2 {{ color: #0f3460; margin-top: 1.5em; }}
  .header {{ display: flex; justify-content: space-between; align-items: center; }}
  .status {{ font-size: 1.2em; font-weight: bold; padding: 0.3em 0.8em; border-radius: 4px; }}
  .status.success {{ background: #d4edda; color: #155724; }}
  .status.warning {{ background: #fff3cd; color: #856404; }}
  .status.error {{ background: #f8d7da; color: #721c24; }}
  .meta {{ display: grid; grid-template-columns: repeat(3, 1fr); gap: 1em; margin: 1em 0; }}
  .meta-card {{ background: white; padding: 1em; border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }}
  .meta-card .label {{ font-size: 0.8em; color: #666; text-transform: uppercase; }}
  .meta-card .value {{ font-size: 1.4em; font-weight: bold; color: #16213e; }}
  table {{ width: 100%; border-collapse: collapse; background: white; border-radius: 8px; overflow: hidden; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }}
  th {{ background: #0f3460; color: white; text-align: left; padding: 0.8em; }}
  td {{ padding: 0.6em 0.8em; border-bottom: 1px solid #eee; }}
  tr:hover {{ background: #f5f5f5; }}
  .phase-header td {{ background: #e8eaf6; font-weight: bold; color: #0f3460; }}
  .section {{ margin-top: 2em; }}
  .trace-link {{ font-family: monospace; font-size: 0.85em; color: #666; }}
  a.trace-link {{ color: #0f3460; text-decoration: underline; }}
  .detail-error {{ color: #721c24; font-size: 0.85em; }}
  tr.row-failed td {{ background: #fdf0f0; }}
  .footer {{ margin-top: 3em; padding-top: 1em; border-top: 1px solid #ddd; color: #888; font-size: 0.85em; }}
</style>
</head>
<body>
<div class="header">
  <h1>Tumult Experiment Report</h1>
  <span class="status {status_class}">{status:?}</span>
</div>

<h2>{title}</h2>

<div class="meta">
  <div class="meta-card"><div class="label">Experiment ID</div><div class="value" style="font-size:0.9em">{id}</div></div>
  <div class="meta-card"><div class="label">Duration</div><div class="value">{duration_ms}ms</div></div>
  <div class="meta-card"><div class="label">Method Steps</div><div class="value">{method_count}</div></div>
</div>

<div class="section">
<h2>Activity Timeline</h2>
<table>
<tr><th>Phase</th><th>Name</th><th>Type</th><th>Status</th><th>Duration</th><th>Trace</th><th>Detail</th></tr>
{activities}
</table>
</div>

{analysis}
{regulatory}

<div class="footer">
  Generated by <strong>Tumult</strong> v{version} — Rust-native chaos engineering platform<br>
  Report generated: {generated_at}<br>
  Source journal hash (DefaultHasher/fnv-style, non-cryptographic): <code>{source_hash}</code>
</div>
</body>
</html>"#,
        title = html_escape(&journal.experiment_title),
        status_class = status_class,
        status = journal.status,
        id = html_escape(&journal.experiment_id),
        duration_ms = journal.duration_ms,
        method_count = journal.method_results.len(),
        activities = activities_html,
        analysis = analysis_html,
        regulatory = regulatory_html,
        // FIX 4: footer metadata — generation timestamp, tool version, content hash.
        generated_at = chrono::Utc::now().to_rfc3339(),
        version = env!("CARGO_PKG_VERSION"),
        source_hash = html_escape(source_hash),
    )
}

pub(crate) fn format_activity_row(
    r: &ActivityResult,
    phase: &str,
    trace_ui_base: Option<&str>,
) -> String {
    let status_emoji = match r.status {
        ActivityStatus::Succeeded => "&#10004;",
        ActivityStatus::Failed => "&#10008;",
        ActivityStatus::Timeout => "&#9203;",
        ActivityStatus::Skipped => "&#8212;",
    };
    let row_class = match r.status {
        ActivityStatus::Failed | ActivityStatus::Timeout => "row-failed",
        _ => "",
    };
    // FIX 2: link uses the FULL trace_id; only the display text is truncated to 16 chars.
    let trace = if r.trace_id.is_empty() {
        String::new()
    } else {
        let full = r.trace_id.as_str();
        let short = &full[..full.len().min(16)];
        match trace_ui_base {
            Some(base) => format!(
                r#"<a class="trace-link" href="{}/trace/{}">{}</a>"#,
                html_escape(base.trim_end_matches('/')),
                html_escape(full),
                short
            ),
            None => format!(r#"<span class="trace-link">{short}</span>"#),
        }
    };
    // FIX 3: surface failure detail (error/output text) for Failed/Timeout activities.
    // A per-activity diagnostic command is intentionally NOT synthesized here: the
    // core experiment path has no such field (`next_diagnostic_command` exists only
    // in tumult-agentic result types), and `activity_results` is not keyed by trace,
    // so any suggested command would be fabricated. The captured error text plus the
    // trace column are the actionable signal.
    let detail = match r.status {
        ActivityStatus::Failed | ActivityStatus::Timeout => {
            let msg = r
                .error
                .as_deref()
                .or(r.output.as_deref())
                .unwrap_or("(no detail captured)");
            format!(r#"<span class="detail-error">{}</span>"#, html_escape(msg))
        }
        _ => String::new(),
    };
    format!(
        "<tr class=\"{}\"><td>{}</td><td>{}</td><td>{:?}</td><td>{} {:?}</td><td>{}ms</td><td>{}</td><td>{}</td></tr>\n",
        row_class,
        phase,
        html_escape(&r.name),
        r.activity_type,
        status_emoji,
        r.status,
        r.duration_ms,
        trace,
        detail,
    )
}
