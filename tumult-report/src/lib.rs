//! `tumult-report` — report model, self-contained HTML renderer and a
//! tokio-interval scheduler producing reports from semantic metric
//! definitions over a read-only store connection.

pub mod narrative;

use std::path::PathBuf;
use std::time::Duration;

use tumult_metrics::{to_sql, MetricDef, MetricsError};
use tumult_lake::{Reader, StoreError};

/// Report errors.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Metrics(#[from] MetricsError),
}

/// One report section.
#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    /// A headline number, Rill-style, with an optional delta vs. the
    /// previous window.
    Kpi {
        label: String,
        value: String,
        delta: Option<String>,
    },
    /// A plain table.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Prose (deterministic in v1; LLM-narrated in later phases).
    Narrative { text: String },
    /// A reference to a live chart in the web UI; renders as a sparkline
    /// placeholder in the static digest.
    ChartRef {
        metric: String,
        dimension: Option<String>,
    },
}

/// A report digest.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub title: String,
    pub generated_at_ns: i64,
    pub sections: Vec<Section>,
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Render a report as a clean, self-contained HTML digest (inline CSS,
/// muted design, sparkline placeholder divs). Suitable for email.
#[must_use]
pub fn render_html(report: &Report) -> String {
    let mut body = String::new();
    for section in &report.sections {
        match section {
            Section::Kpi {
                label,
                value,
                delta,
            } => {
                let delta_html = delta.as_ref().map_or(String::new(), |d| {
                    format!(r#"<span class="delta">{}</span>"#, escape_html(d))
                });
                body.push_str(&format!(
                    r#"<div class="kpi"><div class="kpi-label">{}</div><div class="kpi-value">{}</div>{delta_html}<div class="sparkline" data-metric="{}"></div></div>"#,
                    escape_html(label),
                    escape_html(value),
                    escape_html(label),
                ));
            }
            Section::Table { headers, rows } => {
                body.push_str("<table><thead><tr>");
                for h in headers {
                    body.push_str(&format!("<th>{}</th>", escape_html(h)));
                }
                body.push_str("</tr></thead><tbody>");
                for row in rows {
                    body.push_str("<tr>");
                    for cell in row {
                        body.push_str(&format!("<td>{}</td>", escape_html(cell)));
                    }
                    body.push_str("</tr>");
                }
                body.push_str("</tbody></table>");
            }
            Section::Narrative { text } => {
                body.push_str(&format!("<p>{}</p>", escape_html(text)));
            }
            Section::ChartRef { metric, dimension } => {
                let dim = dimension.as_deref().unwrap_or("");
                body.push_str(&format!(
                    r#"<div class="chart-ref"><div class="sparkline" data-metric="{}" data-dimension="{}"></div><div class="chart-label">{} by {}</div></div>"#,
                    escape_html(metric),
                    escape_html(dim),
                    escape_html(metric),
                    escape_html(dim),
                ));
            }
        }
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<style>
  body {{ font-family: -apple-system, "Segoe UI", Roboto, sans-serif; margin: 2rem auto;
         max-width: 44rem; color: #1c1e21; background: #fafafa; }}
  h1 {{ font-size: 1.25rem; font-weight: 600; }}
  .meta {{ color: #6b7280; font-size: 0.8rem; margin-bottom: 1.5rem; }}
  .kpi {{ display: inline-block; background: #fff; border: 1px solid #e5e7eb;
         border-radius: 6px; padding: 0.75rem 1rem; margin: 0 0.75rem 0.75rem 0;
         min-width: 8rem; }}
  .kpi-label {{ color: #6b7280; font-size: 0.75rem; text-transform: uppercase;
               letter-spacing: 0.05em; }}
  .kpi-value {{ font-size: 1.5rem; font-weight: 600; }}
  .delta {{ color: #6b7280; font-size: 0.8rem; }}
  .sparkline {{ height: 2rem; background: repeating-linear-gradient(90deg,
      #e5e7eb 0, #e5e7eb 2px, transparent 2px, transparent 8px); margin-top: 0.5rem; }}
  .chart-ref {{ margin: 1rem 0; }}
  .chart-label {{ color: #6b7280; font-size: 0.8rem; margin-top: 0.25rem; }}
  table {{ border-collapse: collapse; width: 100%; background: #fff; margin: 1rem 0; }}
  th, td {{ text-align: left; padding: 0.4rem 0.6rem; border-bottom: 1px solid #e5e7eb;
           font-size: 0.9rem; }}
  th {{ color: #6b7280; font-weight: 500; }}
</style>
</head>
<body>
<h1>{title}</h1>
<div class="meta">generated at {generated} (epoch ns)</div>
{body}
</body>
</html>"#,
        title = escape_html(&report.title),
        generated = report.generated_at_ns,
        body = body,
    )
}

/// Format a JSON value (from the store's `row_to_json` rows) for display.
fn fmt_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "—".to_string(),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{}", f as i64)
                } else {
                    format!("{f:.3}")
                }
            } else {
                n.to_string()
            }
        }
        other => other.to_string().trim_matches('"').to_string(),
    }
}

/// Build a KPI report from metric definitions over `time_range`
/// (`[start_ns, end_ns)`). Every number is computed deterministically by the
/// store; nothing here invents data.
///
/// # Errors
/// Returns an error if a metric fails to compile or its query fails.
pub fn build_report(
    reader: &Reader,
    defs: &[MetricDef],
    title: &str,
    time_range: Option<(i64, i64)>,
) -> Result<Report, ReportError> {
    let mut sections = Vec::new();
    for def in defs {
        let sql = to_sql(def, &[], time_range)?;
        let rows = reader.query_json_rows(&sql)?;
        // The headline number must describe the whole window, so for
        // dimensioned defs it comes from an ungrouped query (the grouped
        // rows below become the breakdown table).
        let value_row = if def.dimensions.is_empty() {
            rows.first().cloned()
        } else {
            let mut ungrouped = def.clone();
            ungrouped.dimensions = Vec::new();
            let ungrouped_sql = to_sql(&ungrouped, &[], time_range)?;
            reader.query_json_rows(&ungrouped_sql)?.into_iter().next()
        };
        let value = value_row
            .as_ref()
            .and_then(|r| r.get("value"))
            .map_or_else(|| "—".to_string(), fmt_value);
        sections.push(Section::Kpi {
            label: def.name.clone(),
            value,
            // TODO(report): compute delta vs. the previous window.
            delta: None,
        });
        if !def.dimensions.is_empty() {
            // Dimensioned metrics render their groups as a real table so the
            // digest shows the breakdown (e.g. which experiments ran), not
            // just a chart placeholder.
            let headers: Vec<String> = def
                .dimensions
                .iter()
                .cloned()
                .chain(std::iter::once("value".to_string()))
                .collect();
            let table_rows = rows
                .iter()
                .map(|row| {
                    def.dimensions
                        .iter()
                        .map(|d| row.get(d).map_or_else(|| "—".to_string(), fmt_value))
                        .chain(std::iter::once(
                            row.get("value").map_or_else(|| "—".to_string(), fmt_value),
                        ))
                        .collect()
                })
                .collect();
            sections.push(Section::Table {
                headers,
                rows: table_rows,
            });
            sections.push(Section::ChartRef {
                metric: def.name.clone(),
                dimension: def.dimensions.first().cloned(),
            });
        }
    }
    let generated_at_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64);
    Ok(Report {
        title: title.to_string(),
        generated_at_ns,
        sections,
    })
}

/// Produces reports on a tokio interval, reading through a fresh read-only
/// connection per tick (so it never fights the writer).
pub struct Scheduler {
    store_path: PathBuf,
    defs: Vec<MetricDef>,
    interval: Duration,
    title: String,
}

impl Scheduler {
    #[must_use]
    pub fn new(
        store_path: PathBuf,
        defs: Vec<MetricDef>,
        interval: Duration,
        title: String,
    ) -> Self {
        Self {
            store_path,
            defs,
            interval,
            title,
        }
    }

    /// Run forever, emitting a report per tick. Reports are logged via
    /// `tracing` in v1; delivery (email/webhook/file spool) is Phase 2
    /// // TODO(report): plumb report delivery targets.
    pub async fn run(self) {
        let mut ticker = tokio::time::interval(self.interval);
        loop {
            ticker.tick().await;
            match self.produce_once() {
                Ok(report) => {
                    let kpis = report
                        .sections
                        .iter()
                        .filter_map(|s| match s {
                            Section::Kpi { label, value, .. } => Some(format!("{label}={value}")),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    tracing::info!(kpis = %kpis, "scheduled report produced");
                }
                Err(e) => tracing::warn!(error = %e, "scheduled report failed"),
            }
        }
    }

    fn produce_once(&self) -> Result<Report, ReportError> {
        let store = tumult_lake::Store::open(&self.store_path)?;
        let reader = store.read_only()?;
        build_report(&reader, &self.defs, &self.title, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_contains_kpi_value_and_escapes_html() {
        let report = Report {
            title: "Resilience digest".into(),
            generated_at_ns: 1_774_980_000_000_000_000,
            sections: vec![
                Section::Kpi {
                    label: "hypothesis_pass_rate".into(),
                    value: "0.875".into(),
                    delta: Some("+2.1%".into()),
                },
                Section::Narrative {
                    text: "<script>alert('xss')</script> & friends".into(),
                },
                Section::Table {
                    headers: vec!["target".into()],
                    rows: vec![vec!["<b>pg</b>".into()]],
                },
            ],
        };
        let html = render_html(&report);
        assert!(html.contains("0.875"));
        assert!(html.contains("hypothesis_pass_rate"));
        assert!(html.contains("Resilience digest"));
        // Untrusted text is escaped.
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<b>pg</b>"));
        assert!(html.contains("&lt;b&gt;pg&lt;/b&gt;"));
        // Sparkline placeholders exist for KPI cards.
        assert!(html.contains("class=\"sparkline\""));
    }
}
