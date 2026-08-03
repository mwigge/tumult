// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumult-report` — report model, self-contained HTML renderer and a
//! tokio-interval scheduler producing reports from semantic metric
//! definitions over a read-only store connection.

pub mod narrative;

use std::path::PathBuf;
use std::time::Duration;

use tumult_lake::{Reader, StoreError};
use tumult_metrics::{to_sql, MetricDef, MetricsError};

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

/// Environment predicate confining a metric query to a principal's
/// environment scopes, mirroring the API layer: `spans` binds
/// `target_environment` directly (it lives on the root span), the
/// `metric_*` tables reach it through `experiment_name` correlation against
/// the root spans. `None` when unscoped (empty set = all environments).
fn env_predicate(table: &str, envs: &[String]) -> Option<String> {
    if envs.is_empty() {
        return None;
    }
    let list = envs
        .iter()
        .map(|e| format!("'{}'", e.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    Some(if table == "spans" {
        format!("target_environment IN ({list})")
    } else {
        format!(
            "EXISTS (SELECT 1 FROM spans se \
             WHERE se.experiment_name = {table}.experiment_name \
               AND se.target_environment IN ({list}))"
        )
    })
}

/// Insert an environment predicate into SQL compiled by
/// `tumult_metrics::to_sql` (`SELECT … FROM <table>` + optional `WHERE` +
/// optional `GROUP BY … ORDER BY …`): appended to the existing WHERE clause
/// or added as a fresh one, always before any GROUP BY. Unchanged when
/// `envs` is empty (unscoped).
fn confine_sql(sql: &str, table: &str, envs: &[String]) -> String {
    let Some(pred) = env_predicate(table, envs) else {
        return sql.to_string();
    };
    let (head, tail) = match sql.find("\nGROUP BY") {
        Some(pos) => sql.split_at(pos),
        None => (sql, ""),
    };
    if head.contains("\nWHERE ") {
        format!("{head}\n  AND ({pred}){tail}")
    } else {
        format!("{head}\nWHERE ({pred}){tail}")
    }
}

/// Build a KPI report from metric definitions over `time_range`
/// (`[start_ns, end_ns)`), unscoped (all environments). Every number is
/// computed deterministically by the store; nothing here invents data.
///
/// # Errors
/// Returns an error if a metric fails to compile or its query fails.
pub fn build_report(
    reader: &Reader,
    defs: &[MetricDef],
    title: &str,
    time_range: Option<(i64, i64)>,
) -> Result<Report, ReportError> {
    build_report_scoped(reader, defs, title, time_range, &[])
}

/// Scoped variant of [`build_report`]: `envs` confines every metric query to
/// the given environments (empty = unscoped). A scoped principal gets a
/// digest of its own environments only.
///
/// # Errors
/// Returns an error if a metric fails to compile or its query fails.
pub fn build_report_scoped(
    reader: &Reader,
    defs: &[MetricDef],
    title: &str,
    time_range: Option<(i64, i64)>,
    envs: &[String],
) -> Result<Report, ReportError> {
    let mut sections = Vec::new();
    for def in defs {
        let sql = confine_sql(&to_sql(def, &[], time_range)?, &def.source_table, envs);
        let rows = reader.query_json_rows(&sql)?;
        // The headline number must describe the whole window, so for
        // dimensioned defs it comes from an ungrouped query (the grouped
        // rows below become the breakdown table).
        let value_row = if def.dimensions.is_empty() {
            rows.first().cloned()
        } else {
            let mut ungrouped = def.clone();
            ungrouped.dimensions = Vec::new();
            let ungrouped_sql = confine_sql(
                &to_sql(&ungrouped, &[], time_range)?,
                &ungrouped.source_table,
                envs,
            );
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
    use tumult_metrics::{Measure, MetricDef};

    fn count_def(table: &str, dimensions: Vec<String>) -> MetricDef {
        MetricDef {
            name: "experiment_count".into(),
            description: None,
            source_table: table.into(),
            measure: Measure::Count,
            dimensions,
            time_col: "ts_ns".into(),
            condition: None,
        }
    }

    #[test]
    fn confine_sql_is_identity_when_unscoped() {
        let sql = to_sql(&count_def("spans", vec![]), &[], None).unwrap();
        assert_eq!(confine_sql(&sql, "spans", &[]), sql);
    }

    #[test]
    fn confine_sql_adds_where_before_group_by() {
        let envs = vec!["dev".to_string()];
        // No existing WHERE: a fresh one appears ahead of GROUP BY.
        let sql = to_sql(&count_def("spans", vec!["target_system".into()]), &[], None).unwrap();
        let confined = confine_sql(&sql, "spans", &envs);
        assert!(
            confined.contains("\nWHERE (target_environment IN ('dev'))\nGROUP BY"),
            "{confined}"
        );
        // Existing WHERE (time range): the predicate joins it with AND.
        let sql = to_sql(&count_def("spans", vec![]), &[], Some((1, 2))).unwrap();
        let confined = confine_sql(&sql, "spans", &envs);
        assert!(
            confined.contains("\n  AND (target_environment IN ('dev'))"),
            "{confined}"
        );
    }

    #[test]
    fn confine_sql_correlates_metric_tables_through_root_spans() {
        let envs = vec!["dev".to_string(), "staging".to_string()];
        let sql = to_sql(&count_def("metric_sums", vec![]), &[], None).unwrap();
        let confined = confine_sql(&sql, "metric_sums", &envs);
        assert!(
            confined.contains(
                "EXISTS (SELECT 1 FROM spans se \
                 WHERE se.experiment_name = metric_sums.experiment_name \
                   AND se.target_environment IN ('dev', 'staging'))"
            ),
            "{confined}"
        );
    }

    #[test]
    fn build_report_scoped_counts_only_in_scope_rows() {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let root = |id: &str, env: &str, ts: i64| tumult_lake::SpanRow {
            ts_ns: ts,
            trace_id: format!("trace-{id}"),
            span_id: format!("span-{id}-root"),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            service_name: "tumult".into(),
            experiment_id: Some(id.into()),
            experiment_name: Some(id.into()),
            target_environment: Some(env.into()),
            events: "[]".into(),
            ..Default::default()
        };
        store
            .writer()
            .unwrap()
            .insert_spans(&[root("exp-dev", "dev", 1), root("exp-prd", "prod", 2)])
            .unwrap();
        let reader = store.read_only().unwrap();
        let defs = vec![count_def("spans", vec![])];

        let kpi_value = |report: &Report| match &report.sections[0] {
            Section::Kpi { value, .. } => value.clone(),
            other => panic!("expected KPI section, got {other:?}"),
        };
        let global = build_report(&reader, &defs, "t", None).unwrap();
        assert_eq!(kpi_value(&global), "2");
        let scoped = build_report_scoped(&reader, &defs, "t", None, &["dev".to_string()]).unwrap();
        assert_eq!(kpi_value(&scoped), "1");
    }

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

    #[test]
    fn render_kpi_without_delta_and_chart_refs() {
        let report = Report {
            title: "t".into(),
            generated_at_ns: 0,
            sections: vec![
                Section::Kpi {
                    label: "experiment_count".into(),
                    value: "7".into(),
                    delta: None,
                },
                Section::ChartRef {
                    metric: "deviation_rate".into(),
                    dimension: Some("target".into()),
                },
                Section::ChartRef {
                    metric: "mttr".into(),
                    dimension: None,
                },
            ],
        };
        let html = render_html(&report);
        // No delta means no delta span at all.
        assert!(!html.contains("class=\"delta\""));
        // Dimensioned chart ref names both metric and dimension.
        assert!(html.contains(r#"data-metric="deviation_rate" data-dimension="target""#));
        assert!(html.contains("deviation_rate by target"));
        // A dimensionless ref renders an empty dimension placeholder.
        assert!(html.contains(r#"data-metric="mttr" data-dimension="""#));
        assert!(html.contains("mttr by"));
    }

    #[test]
    fn fmt_value_formats_every_json_shape() {
        assert_eq!(fmt_value(&serde_json::Value::Null), "—");
        assert_eq!(fmt_value(&serde_json::json!(42)), "42");
        assert_eq!(fmt_value(&serde_json::json!(-3)), "-3");
        assert_eq!(fmt_value(&serde_json::json!(2.5)), "2.500");
        // Integral floats beyond the exact-i64 window print as floats.
        assert_eq!(fmt_value(&serde_json::json!(1e16)), "10000000000000000.000");
        assert_eq!(fmt_value(&serde_json::json!("plain")), "plain");
        assert_eq!(fmt_value(&serde_json::json!(true)), "true");
    }

    #[test]
    fn scheduler_produces_a_report_from_the_store() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("k.duckdb");
        {
            let store = tumult_lake::Store::open(&path).unwrap();
            store
                .writer()
                .unwrap()
                .insert_spans(&[tumult_lake::SpanRow {
                    ts_ns: 1,
                    trace_id: "trace-1".into(),
                    span_id: "span-1".into(),
                    span_name: "resilience.experiment".into(),
                    span_kind: "Internal".into(),
                    service_name: "tumult".into(),
                    experiment_id: Some("exp-1".into()),
                    experiment_name: Some("exp-1".into()),
                    events: "[]".into(),
                    ..Default::default()
                }])
                .unwrap();
        } // store dropped: the scheduler opens its own read-only connection

        let scheduler = Scheduler::new(
            path,
            vec![count_def("spans", vec![])],
            Duration::from_secs(60),
            "scheduled digest".into(),
        );
        let report = scheduler.produce_once().unwrap();
        assert_eq!(report.title, "scheduled digest");
        match &report.sections[0] {
            Section::Kpi { value, .. } => assert_eq!(value, "1"),
            other => panic!("expected KPI section, got {other:?}"),
        }
    }
}
