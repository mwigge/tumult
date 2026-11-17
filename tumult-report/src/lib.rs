// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumult-report` — the metric digest: its model, the builder that reads
//! the numbers of semantic metric definitions over a read-only store
//! connection, and a self-contained HTML renderer.

pub mod narrative;
mod show;
mod trend;

use std::time::Duration;

use serde_json::Value;
use tumult_compliance::html::fmt_datetime;
use tumult_compliance::svg::render_svg;
use tumult_compliance::ChartSpec;
use tumult_lake::{Reader, StoreError};
use tumult_metrics::{to_sql, to_sql_bucketed, MetricDef, MetricsError, Unit};

/// Report errors.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Metrics(#[from] MetricsError),
}

/// The value of a metric in each of a row of equal stretches of time.
#[derive(Debug, Clone, PartialEq)]
pub struct Trend {
    /// The stretch of time one point covers, in words: `day`, `5 minutes`.
    pub per: String,
    /// The start of each stretch in epoch nanoseconds and the value in it,
    /// the oldest first.
    pub points: Vec<(i64, f64)>,
}

/// One report section.
#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    /// A headline number, Rill-style.
    Kpi {
        label: String,
        value: String,
        /// What the value did against the period before, when the digest
        /// has a period.
        delta: Option<String>,
        /// What the number measures, in a sentence.
        about: Option<String>,
        /// The value over the period, drawn as a small line.
        trend: Option<Trend>,
    },
    /// A plain table.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Prose (deterministic in v1; LLM-narrated in later phases).
    Narrative { text: String },
    /// The value of a metric for each group of its dimensions, drawn as
    /// bars under `title`.
    Bars {
        title: String,
        bars: Vec<(String, f64)>,
    },
}

/// A report digest.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub title: String,
    pub generated_at_ns: i64,
    /// The `[start_ns, end_ns)` the numbers are about; `None` for all data.
    pub period: Option<(i64, i64)>,
    pub sections: Vec<Section>,
}

/// The title of the digest the daemon writes every `interval`.
#[must_use]
pub fn digest_title(interval: Duration) -> String {
    format!("Tumult digest — last {}", stretch(interval.as_secs()))
}

/// A stretch of `seconds` in words, in the largest unit it is a whole
/// number of: `7 days`, `hour`, `90 seconds`.
fn stretch(seconds: u64) -> String {
    let (count, unit) = [(86_400, "day"), (3_600, "hour"), (60, "minute")]
        .into_iter()
        .find(|(size, _)| seconds.is_multiple_of(*size))
        .map_or((seconds, "second"), |(size, unit)| (seconds / size, unit));
    if count == 1 {
        return unit.to_string();
    }
    format!("{count} {unit}s")
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The card of a headline number: its name, its value, and when the digest
/// has them its change, what it measures and its trend.
fn kpi_html(
    label: &str,
    value: &str,
    delta: Option<&str>,
    about: Option<&str>,
    trend: Option<&Trend>,
) -> String {
    let part = |class: &str, text: Option<&str>| {
        text.map_or(String::new(), |text| {
            format!(r#"<div class="{class}">{}</div>"#, escape_html(text))
        })
    };
    format!(
        r#"<div class="kpi"><div class="kpi-label">{}</div><div class="kpi-value">{}</div>{}{}{}</div>"#,
        escape_html(label),
        escape_html(value),
        part("delta", delta),
        trend.map_or(String::new(), trend_html),
        part("kpi-about", about),
    )
}

/// The small line of a trend, and the stretch of time one point covers.
fn trend_html(trend: &Trend) -> String {
    format!(
        r#"<div class="trend">{}<span class="trend-per">per {}</span></div>"#,
        render_svg(&ChartSpec::Sparkline(trend.points.clone())),
        escape_html(&trend.per),
    )
}

fn table_html(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut out = String::from("<table><thead><tr>");
    for header in headers {
        out.push_str(&format!("<th>{}</th>", escape_html(header)));
    }
    out.push_str("</tr></thead><tbody>");
    for row in rows {
        out.push_str("<tr>");
        for cell in row {
            out.push_str(&format!("<td>{}</td>", escape_html(cell)));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    out
}

fn bars_html(title: &str, bars: &[(String, f64)]) -> String {
    format!(
        r#"<div class="chart"><div class="chart-label">{}</div>{}</div>"#,
        escape_html(title),
        render_svg(&ChartSpec::Bars(bars.to_vec())),
    )
}

fn section_html(section: &Section) -> String {
    match section {
        Section::Kpi {
            label,
            value,
            delta,
            about,
            trend,
        } => kpi_html(
            label,
            value,
            delta.as_deref(),
            about.as_deref(),
            trend.as_ref(),
        ),
        Section::Table { headers, rows } => table_html(headers, rows),
        Section::Narrative { text } => format!("<p>{}</p>", escape_html(text)),
        Section::Bars { title, bars } => bars_html(title, bars),
    }
}

/// The line under the title: the period of the digest and when it was made.
fn meta_line(report: &Report) -> String {
    let period = match report.period {
        Some((start, end)) => {
            format!("Period: {} to {}", fmt_datetime(start), fmt_datetime(end))
        }
        None => "All data".to_string(),
    };
    format!("{period} · made {}", fmt_datetime(report.generated_at_ns))
}

/// Render a report as a clean, self-contained HTML digest: inline CSS and
/// inline SVG charts, nothing loaded from elsewhere.
#[must_use]
pub fn render_html(report: &Report) -> String {
    let body: String = report.sections.iter().map(section_html).collect();
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
  .kpi {{ display: inline-block; vertical-align: top; background: #fff;
         border: 1px solid #e5e7eb; border-radius: 6px; padding: 0.75rem 1rem;
         margin: 0 0.75rem 0.75rem 0; width: 12rem; }}
  .kpi-label {{ color: #6b7280; font-size: 0.75rem; text-transform: uppercase;
               letter-spacing: 0.05em; }}
  .kpi-value {{ font-size: 1.5rem; font-weight: 600; }}
  .delta, .kpi-about, .trend-per, .chart-label {{ color: #6b7280; font-size: 0.8rem; }}
  .trend {{ margin-top: 0.5rem; }}
  .trend svg {{ vertical-align: middle; margin-right: 0.4rem; }}
  .kpi-about {{ margin-top: 0.5rem; }}
  .chart {{ margin: 1rem 0; }}
  .chart-label {{ margin-bottom: 0.4rem; }}
  table {{ border-collapse: collapse; width: 100%; background: #fff; margin: 1rem 0; }}
  th, td {{ text-align: left; padding: 0.4rem 0.6rem; border-bottom: 1px solid #e5e7eb;
           font-size: 0.9rem; }}
  th {{ color: #6b7280; font-weight: 500; }}
</style>
</head>
<body>
<h1>{title}</h1>
<div class="meta">{meta}</div>
{body}
</body>
</html>"#,
        title = escape_html(&report.title),
        meta = escape_html(&meta_line(report)),
    )
}

/// Format a JSON value (from the store's `row_to_json` rows) for display.
fn fmt_value(value: &Value) -> String {
    match value {
        Value::Null => "—".to_string(),
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), show::number),
        other => other.to_string().trim_matches('"').to_string(),
    }
}

/// The value of a metric in `unit` for display: a number in its unit,
/// anything else as it is.
fn shown(unit: Option<Unit>, value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| fmt_value(value), |number| show::value(unit, number))
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
/// `tumult_metrics::to_sql` or `to_sql_bucketed` (`SELECT … FROM <table>` +
/// optional `WHERE` + optional `GROUP BY … ORDER BY …`): appended to the
/// existing WHERE clause or added as a fresh one, always before any GROUP
/// BY. Unchanged when `envs` is empty (unscoped).
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

/// One metric, read from the store for a reader of the environments `envs`.
struct Source<'a> {
    reader: &'a Reader,
    def: &'a MetricDef,
    envs: &'a [String],
}

impl Source<'_> {
    /// The rows of `sql`, confined to the environments of the reader.
    fn rows(&self, sql: &str) -> Result<Vec<Value>, ReportError> {
        let confined = confine_sql(sql, &self.def.source_table, self.envs);
        Ok(self.reader.query_json_rows(&confined)?)
    }

    /// The value of the metric over the whole of `period`, whatever its
    /// dimensions: the headline number describes everything, the groups
    /// are its breakdown. `Null` when the store has no value.
    fn headline(&self, period: Option<(i64, i64)>) -> Result<Value, ReportError> {
        let mut whole = self.def.clone();
        whole.dimensions = Vec::new();
        let rows = self.rows(&to_sql(&whole, &[], period)?)?;
        let value = rows.first().and_then(|row| row.get("value"));
        Ok(value.cloned().unwrap_or(Value::Null))
    }

    /// What the value `now` of `period` did against the period of the same
    /// length just before it. A digest over all data has no period before,
    /// and a number without a value has nothing to compare.
    fn change(
        &self,
        now: &Value,
        period: Option<(i64, i64)>,
    ) -> Result<Option<String>, ReportError> {
        let (Some(now), Some((start, end))) = (now.as_f64(), period) else {
            return Ok(None);
        };
        let before = self.headline(Some((start - (end - start), start)))?;
        Ok(Some(show::change(self.def.unit, now, before.as_f64())))
    }

    /// The value over `period` in equal stretches of time. One stretch with
    /// a value is a number, not a trend: `None` below two points.
    fn trend(&self, period: Option<(i64, i64)>) -> Result<Option<Trend>, ReportError> {
        let (width, per) = trend::width_for(period);
        let rows = self.rows(&to_sql_bucketed(self.def, width, &[], period)?)?;
        let found: Vec<(i64, f64)> = rows.iter().filter_map(trend::point_of).collect();
        let points = if trend::counts(&self.def.measure) {
            trend::filled(&found, width, period)
        } else {
            found
        };
        if points.len() < 2 {
            return Ok(None);
        }
        Ok(Some(Trend {
            per: per.to_string(),
            points,
        }))
    }

    /// The headline number of the metric over `period`.
    fn kpi(&self, period: Option<(i64, i64)>) -> Result<Section, ReportError> {
        let now = self.headline(period)?;
        Ok(Section::Kpi {
            label: self.def.name.clone(),
            value: shown(self.def.unit, &now),
            delta: self.change(&now, period)?,
            about: self.def.description.clone(),
            trend: self.trend(period)?,
        })
    }

    /// The groups of a metric with dimensions over `period`: a table of
    /// them and, when there are any, their bars.
    fn breakdown(&self, period: Option<(i64, i64)>) -> Result<Vec<Section>, ReportError> {
        if self.def.dimensions.is_empty() {
            return Ok(Vec::new());
        }
        let rows = self.rows(&to_sql(self.def, &[], period)?)?;
        let mut sections = vec![self.table(&rows)];
        sections.extend(self.bars(&rows));
        Ok(sections)
    }

    /// The value of each dimension of the group `row`.
    fn group_of(&self, row: &Value) -> Vec<String> {
        let cell = |dimension| {
            row.get(dimension)
                .map_or_else(|| "—".to_string(), fmt_value)
        };
        self.def.dimensions.iter().map(cell).collect()
    }

    fn table(&self, rows: &[Value]) -> Section {
        let value_of = |row: &Value| shown(self.def.unit, row.get("value").unwrap_or(&Value::Null));
        Section::Table {
            headers: self
                .def
                .dimensions
                .iter()
                .cloned()
                .chain(std::iter::once("value".to_string()))
                .collect(),
            rows: rows
                .iter()
                .map(|row| {
                    let mut cells = self.group_of(row);
                    cells.push(value_of(row));
                    cells
                })
                .collect(),
        }
    }

    /// A bar for each group that has a value; `None` when no group has one.
    fn bars(&self, rows: &[Value]) -> Option<Section> {
        let found = |row: &Value| {
            let value = row.get("value")?.as_f64()?;
            Some((self.group_of(row).join(" / "), value))
        };
        let (groups, values): (Vec<String>, Vec<f64>) = rows.iter().filter_map(found).unzip();
        if groups.is_empty() {
            return None;
        }
        let (sizes, unit) = show::bars(self.def.unit, &values);
        let what = format!("{} by {}", self.def.name, self.def.dimensions.join(", "));
        let title = match unit {
            Some(unit) => format!("{what} ({unit})"),
            None => what,
        };
        let bars = groups.into_iter().zip(sizes).collect();
        Some(Section::Bars { title, bars })
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
        let source = Source { reader, def, envs };
        sections.push(source.kpi(time_range)?);
        sections.extend(source.breakdown(time_range)?);
    }
    let generated_at_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64);
    Ok(Report {
        title: title.to_string(),
        generated_at_ns,
        period: time_range,
        sections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_metrics::{Measure, MetricDef};

    fn count_def(table: &str, dimensions: Vec<String>) -> MetricDef {
        MetricDef {
            name: "experiment_count".into(),
            description: None,
            unit: None,
            source_table: table.into(),
            measure: Measure::Count,
            dimensions,
            time_col: "ts_ns".into(),
            condition: None,
        }
    }

    /// A headline number with nothing but its name and its value.
    fn bare_kpi(label: &str, value: &str) -> Section {
        Section::Kpi {
            label: label.into(),
            value: value.into(),
            delta: None,
            about: None,
            trend: None,
        }
    }

    fn report_of(sections: Vec<Section>) -> Report {
        Report {
            title: "Resilience digest".into(),
            generated_at_ns: 1_774_980_000_000_000_000,
            period: None,
            sections,
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
        let report = report_of(vec![
            Section::Kpi {
                label: "hypothesis_pass_rate".into(),
                value: "87.5%".into(),
                delta: Some("+2.1 points vs the period <before>".into()),
                about: Some("Runs that met their <hypothesis>.".into()),
                trend: None,
            },
            Section::Narrative {
                text: "<script>alert('xss')</script> & friends".into(),
            },
            Section::Table {
                headers: vec!["target".into()],
                rows: vec![vec!["<b>pg</b>".into()]],
            },
            Section::Bars {
                title: "runs by <target>".into(),
                bars: vec![("<i>pg</i>".into(), 2.0)],
            },
        ]);
        let html = render_html(&report);
        assert!(html.contains(r#"<div class="kpi-value">87.5%</div>"#));
        assert!(html.contains(r#"<div class="kpi-label">hypothesis_pass_rate</div>"#));
        assert!(html.contains("<h1>Resilience digest</h1>"));
        assert!(
            html.contains(r#"<div class="delta">+2.1 points vs the period &lt;before&gt;</div>"#)
        );
        assert!(html
            .contains(r#"<div class="kpi-about">Runs that met their &lt;hypothesis&gt;.</div>"#));
        // Untrusted text is escaped.
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<b>pg</b>"));
        assert!(html.contains("&lt;b&gt;pg&lt;/b&gt;"));
        assert!(html.contains(r#"<div class="chart-label">runs by &lt;target&gt;</div>"#));
        assert!(!html.contains("<i>pg</i>"));
        assert!(html.contains("&lt;i&gt;pg&lt;/i&gt;"));
    }

    #[test]
    fn a_bare_number_has_no_change_no_trend_and_no_text_about_it() {
        let html = render_html(&report_of(vec![bare_kpi("experiment_count", "7")]));

        assert!(
            html.contains(r#"<div class="kpi-value">7</div></div>"#),
            "{html}"
        );
        assert!(!html.contains(r#"class="delta""#));
        assert!(!html.contains(r#"class="kpi-about""#));
        assert!(!html.contains(r#"class="trend""#));
    }

    #[test]
    fn the_trend_of_a_number_is_drawn_in_its_card_with_what_a_point_covers() {
        let day = 86_400_000_000_000;
        let html = render_html(&report_of(vec![Section::Kpi {
            label: "experiment_count".into(),
            value: "4".into(),
            delta: None,
            about: None,
            trend: Some(Trend {
                per: "day".into(),
                points: vec![(0, 1.0), (day, 3.0)],
            }),
        }]));

        assert!(
            html.contains("<polyline points='3.0,25.0 117.0,3.0'"),
            "{html}"
        );
        assert!(
            html.contains(r#"</svg><span class="trend-per">per day</span></div>"#),
            "{html}"
        );
    }

    #[test]
    fn the_page_is_titled_and_escaped_in_its_head_too() {
        let mut report = report_of(Vec::new());
        report.title = "a <b> digest".into();

        let html = render_html(&report);

        assert!(html.contains("<title>a &lt;b&gt; digest</title>"), "{html}");
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
    fn a_value_that_is_not_a_number_is_shown_as_it_is() {
        assert_eq!(
            shown(Some(Unit::Seconds), &serde_json::json!("soon")),
            "soon"
        );
        assert_eq!(shown(Some(Unit::Seconds), &Value::Null), "—");
        assert_eq!(shown(Some(Unit::Seconds), &serde_json::json!(2)), "2.0 s");
    }
}
