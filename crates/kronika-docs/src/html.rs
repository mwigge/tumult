//! Print-styled HTML rendering of a [`ReportDoc`] — light paper, serif
//! headings, A4 `@page` rules. This is the UI preview / browser
//! save-as-PDF path; the Typst PDF is the canonical artifact.

use crate::model::{Block, ReportDoc};
use crate::svg::render_svg;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Format epoch ns as `YYYY-MM-DD` (UTC, Howard Hinnant's algorithm).
#[must_use]
pub fn fmt_date(ns: i64) -> String {
    let days = ns.div_euclid(86_400 * 1_000_000_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Format epoch ns as `YYYY-MM-DD HH:MM` UTC.
#[must_use]
pub fn fmt_datetime(ns: i64) -> String {
    let secs_of_day = ns.rem_euclid(86_400 * 1_000_000_000) / 1_000_000_000;
    format!(
        "{} {:02}:{:02} UTC",
        fmt_date(ns),
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    )
}

/// Render the document as a self-contained print-styled HTML page.
#[must_use]
pub fn render_html(doc: &ReportDoc) -> String {
    let m = &doc.meta;
    let mut body = String::new();

    // Cover block (first page).
    body.push_str(&format!(
        "<section class='cover'>\
         <div class='doc-class'>{class}</div>\
         <h1 class='display'>{title}</h1>\
         <div class='cover-sub'>{subtitle}</div>\
         <table class='docmeta'><tbody>\
         <tr><th>Document ID</th><td>{doc_id}</td></tr>\
         <tr><th>Version</th><td>{version}</td></tr>\
         <tr><th>Generated</th><td>{generated}</td></tr>\
         <tr><th>Data as of</th><td>{data_as_of}</td></tr>{period}\
         </tbody></table></section>",
        class = esc(&m.classification),
        title = esc(&m.title),
        subtitle = esc(&subtitle(m)),
        doc_id = esc(&m.doc_id),
        version = esc(&m.version),
        generated = fmt_datetime(m.generated_at_ns),
        data_as_of = fmt_datetime(m.data_as_of_ns),
        period = m.period.map_or(String::new(), |(f, t)| format!(
            "<tr><th>Period</th><td>{} — {}</td></tr>",
            fmt_date(f),
            fmt_date(t)
        )),
    ));

    for block in &doc.blocks {
        body.push_str(&render_block(block));
    }

    format!(
        "<!DOCTYPE html><html lang='en'><head><meta charset='utf-8'>\
         <title>{title}</title><style>{CSS}</style></head>\
         <body><main class='page'>{body}</main></body></html>",
        title = esc(&m.title),
    )
}

fn subtitle(m: &crate::model::DocMeta) -> String {
    match m.template {
        crate::model::TemplateKind::ExecutiveDigest => "Executive Resilience Digest".into(),
        crate::model::TemplateKind::GameDay => format!(
            "Game-Day Experiment Report — {}",
            m.experiment_id.as_deref().unwrap_or("unknown run")
        ),
        crate::model::TemplateKind::EvidencePack => format!(
            "Compliance Evidence Pack — {}",
            m.framework.as_deref().unwrap_or("framework")
        ),
    }
}

fn render_block(block: &Block) -> String {
    match block {
        Block::H1(text) => format!("<h1 class='section'>{}</h1>", esc(text)),
        Block::H2(text) => format!("<h2>{}</h2>", esc(text)),
        Block::Paragraph(text) => format!("<p>{}</p>", esc(text)),
        Block::Kpis(kpis) => {
            let cards: String = kpis
                .iter()
                .map(|(label, value, sub)| {
                    format!(
                        "<div class='kpi'><div class='kpi-label'>{}</div>\
                         <div class='kpi-value'>{}</div>{}</div>",
                        esc(label),
                        esc(value),
                        sub.as_ref().map_or(String::new(), |s| format!(
                            "<div class='kpi-sub'>{}</div>",
                            esc(s)
                        )),
                    )
                })
                .collect();
            format!("<div class='kpis'>{cards}</div>")
        }
        Block::KeyValues(kvs) => {
            let rows: String = kvs
                .iter()
                .map(|(k, v)| format!("<tr><th>{}</th><td>{}</td></tr>", esc(k), esc(v)))
                .collect();
            format!("<table class='docmeta'><tbody>{rows}</tbody></table>")
        }
        Block::Table {
            headers,
            rows,
            numeric_cols,
        } => {
            let head: String = headers
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let cls = if numeric_cols.contains(&i) {
                        " class='num'"
                    } else {
                        ""
                    };
                    format!("<th{cls}>{}</th>", esc(h))
                })
                .collect();
            let body: String = rows
                .iter()
                .map(|row| {
                    let cells: String = row
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let cls = if numeric_cols.contains(&i) {
                                " class='num'"
                            } else {
                                ""
                            };
                            format!("<td{cls}>{}</td>", esc(c))
                        })
                        .collect();
                    format!("<tr>{cells}</tr>")
                })
                .collect();
            format!(
                "<table class='data'><thead><tr>{head}</tr></thead><tbody>{body}</tbody></table>"
            )
        }
        Block::Bullets(items) => {
            let lis: String = items
                .iter()
                .map(|i| format!("<li>{}</li>", esc(i)))
                .collect();
            format!("<ul>{lis}</ul>")
        }
        Block::Chart(spec) => {
            format!(
                "<figure>{}<figcaption>Source: kronika · data as of {}</figcaption></figure>",
                render_svg(spec),
                "{{DATA_AS_OF}}"
            )
        }
        Block::PageBreak => "<div class='pagebreak'></div>".into(),
        Block::Footnote(text) => format!("<p class='footnote'>{}</p>", esc(text)),
        Block::Signoff(roles) => {
            let rows: String = roles
                .iter()
                .map(|(role, name)| {
                    format!(
                        "<div class='sig'><div class='sig-line'></div>\
                         <div class='sig-role'>{}</div><div class='sig-name'>{}</div></div>",
                        esc(role),
                        if name.is_empty() { "Name / date" } else { name },
                    )
                })
                .collect();
            format!("<div class='signoff'>{rows}</div>")
        }
    }
}

/// Substitute the data-as-of placeholder in chart captions.
#[must_use]
pub fn finalize(html: String, doc: &ReportDoc) -> String {
    html.replace("{{DATA_AS_OF}}", &fmt_date(doc.meta.data_as_of_ns))
}

/// Render in one step.
#[must_use]
pub fn render(doc: &ReportDoc) -> String {
    finalize(render_html(doc), doc)
}

const CSS: &str = r"
@page { size: A4; margin: 20mm 18mm; }
* { box-sizing: border-box; }
body { margin: 0; background: #e8eaec; font: 10.5pt/1.55 'Inter', system-ui, sans-serif;
       color: #1a1d21; font-variant-numeric: lining-nums tabular-nums; }
.page { max-width: 210mm; margin: 24px auto; background: #fff; padding: 20mm 18mm;
        box-shadow: 0 1px 4px rgba(0,0,0,.18); }
@media print { body { background: #fff; } .page { margin: 0; box-shadow: none; max-width: none; } }
h1.section { font: 600 16pt 'Source Serif 4', Georgia, serif; margin: 1.4em 0 .5em;
             border-bottom: 1.5px solid #1a1d21; padding-bottom: 4px; }
h2 { font: 600 12pt 'Source Serif 4', Georgia, serif; margin: 1.2em 0 .35em; }
p { margin: .45em 0; }
.cover { border-bottom: 2.5px solid #1a1d21; padding-bottom: 18px; margin-bottom: 8px; }
.doc-class { font-size: 8.5pt; letter-spacing: .18em; text-transform: uppercase; color: #6b7280;
             font-variant: small-caps; margin-bottom: 28px; }
h1.display { font: 700 26pt/1.15 'Source Serif 4', Georgia, serif; margin: 0 0 6px; }
.cover-sub { font-size: 12pt; color: #4b5563; margin-bottom: 26px; }
table.docmeta { border-collapse: collapse; font-size: 9.5pt; }
table.docmeta th { text-align: left; font-weight: 500; color: #6b7280; padding: 2px 24px 2px 0;
                   font-variant: small-caps; letter-spacing: .08em; }
table.docmeta td { padding: 2px 0; }
.kpis { display: flex; flex-wrap: wrap; gap: 10px; margin: 10px 0 14px; }
.kpi { border: 1px solid #d8dce0; border-radius: 4px; padding: 10px 16px; min-width: 110px; }
.kpi-label { font-size: 8pt; letter-spacing: .14em; text-transform: uppercase; color: #6b7280; }
.kpi-value { font: 600 17pt 'Source Serif 4', Georgia, serif; margin-top: 2px; }
.kpi-sub { font-size: 8.5pt; color: #6b7280; }
table.data { border-collapse: collapse; width: 100%; margin: 10px 0 14px; font-size: 9.5pt; }
table.data thead { display: table-header-group; }
table.data th { text-align: left; font-weight: 600; font-size: 8pt; letter-spacing: .1em;
                text-transform: uppercase; color: #6b7280; border-bottom: 1.2px solid #1a1d21;
                padding: 4px 10px 4px 0; }
table.data td { border-bottom: 0.6px solid #e3e6e9; padding: 5px 10px 5px 0; }
table.data .num, table.data th.num { text-align: right; }
ul { margin: .4em 0 .8em; padding-left: 1.3em; }
li { margin: .2em 0; }
figure { margin: 12px 0 16px; }
figcaption { font-size: 8pt; color: #6b7280; margin-top: 4px; }
.pagebreak { break-after: page; }
.footnote { font-size: 8pt; color: #6b7280; border-top: 0.6px solid #d8dce0;
            padding-top: 6px; margin-top: 24px; }
.signoff { display: flex; gap: 40px; margin-top: 48px; }
.sig { flex: 1; }
.sig-line { border-bottom: 1px solid #1a1d21; height: 28px; }
.sig-role { font-size: 8pt; letter-spacing: .12em; text-transform: uppercase; color: #6b7280;
            margin-top: 4px; }
.sig-name { font-size: 9pt; color: #6b7280; }
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_date_handles_epoch_and_recent() {
        assert_eq!(fmt_date(0), "1970-01-01");
        assert_eq!(fmt_date(1_785_273_590_i64 * 1_000_000_000), "2026-07-28");
        // Negative (pre-epoch) dates work too.
        assert_eq!(fmt_date(-86_400 * 1_000_000_000), "1969-12-31");
    }

    #[test]
    fn render_escapes_and_carries_document_control() {
        let doc = ReportDoc {
            meta: crate::model::DocMeta {
                doc_id: "KRK-R1-20260728-ab12cd".into(),
                title: "Digest <x>".into(),
                template: crate::model::TemplateKind::ExecutiveDigest,
                version: "1.0".into(),
                classification: "Internal — Compliance Evidence".into(),
                generated_at_ns: 0,
                data_as_of_ns: 0,
                period: None,
                framework: None,
                experiment_id: None,
            },
            blocks: vec![Block::Paragraph("a < b".into())],
        };
        let html = render(&doc);
        assert!(html.contains("KRK-R1-20260728-ab12cd"));
        assert!(html.contains("Digest &lt;x&gt;"));
        assert!(!html.contains("<x>"));
    }
}
