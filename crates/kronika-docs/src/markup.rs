//! `ReportDoc` → Typst markup (with chart SVGs as virtual files).

use std::collections::HashMap;
use std::fmt::Write as _;

use typst::foundations::Bytes;
use typst::syntax::VirtualPath;

use crate::html::{fmt_date, fmt_datetime};
use crate::model::{Block, DocMeta, ReportDoc, TemplateKind};

/// Escape characters that carry meaning in Typst markup.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' | '#' | '[' | ']' | '{' | '}' | '*' | '_' | '`' | '$' | '~' | '=' | '+' | '-'
            | '@' | '<' | '>' | '/' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn template_label(kind: TemplateKind) -> &'static str {
    match kind {
        TemplateKind::ExecutiveDigest => "R1 \\u{2014} Executive resilience digest",
        TemplateKind::GameDay => "R3 \\u{2014} Game-day report",
        TemplateKind::EvidencePack => "R2 \\u{2014} Compliance evidence pack",
    }
}

/// Build the Typst source for `doc` plus the virtual files it references.
///
/// Charts are emitted as `#image("/charts/cN.svg")`; the returned map carries
/// the SVG bytes the in-memory `World` serves for those paths.
pub fn doc_to_typst(doc: &ReportDoc) -> (String, HashMap<VirtualPath, Bytes>) {
    let mut files = HashMap::new();
    let mut out = String::new();
    let meta = &doc.meta;

    // Page + typography preamble.
    out.push_str("#set page(\n  paper: \"a4\",\n");
    out.push_str("  margin: (top: 20mm, bottom: 20mm, left: 18mm, right: 18mm),\n");
    let _ = write!(
        out,
        "  header: context [\n    #text(size: 7.5pt, fill: luma(110))[#smallcaps[{}] #h(1fr) {}]\n    #v(2pt)\n    #line(length: 100%, stroke: 0.5pt + luma(200))\n  ],\n",
        esc(&meta.title),
        esc(&meta.classification),
    );
    let _ = write!(
        out,
        "  footer: context [\n    #text(size: 7.5pt, fill: luma(110))[{} #h(1fr) Page #counter(page).display(\"1 of 1\", both: true)]\n  ],\n)\n",
        esc(&meta.doc_id),
    );
    out.push_str("#set text(font: \"Inter\", size: 10.5pt, lang: \"en\")\n");
    out.push_str("#set par(justify: true, leading: 0.6em)\n");
    out.push_str("#show heading: set text(font: \"Source Serif 4\", weight: \"regular\")\n");
    out.push_str("#show heading.where(level: 1): set text(size: 16pt)\n");
    out.push_str("#show heading.where(level: 2): set text(size: 12.5pt)\n");

    cover(&mut out, meta);

    let mut chart_idx = 0usize;
    for block in &doc.blocks {
        match block {
            Block::H1(t) => {
                let _ = writeln!(out, "\n#v(6pt)\n= {}\n", esc(t));
            }
            Block::H2(t) => {
                let _ = writeln!(out, "\n== {}\n", esc(t));
            }
            Block::Paragraph(t) => {
                let _ = writeln!(out, "\n{}\n", esc(t));
            }
            Block::Kpis(kpis) => kpis_block(&mut out, kpis),
            Block::KeyValues(kvs) => kv_block(&mut out, kvs),
            Block::Table {
                headers,
                rows,
                numeric_cols,
            } => table_block(&mut out, headers, rows, numeric_cols),
            Block::Bullets(items) => {
                out.push_str("\n#list(\n");
                for item in items {
                    let _ = writeln!(out, "  [{}],", esc(item));
                }
                out.push_str(")\n");
            }
            Block::Chart(spec) => {
                let path = format!("charts/c{chart_idx}.svg");
                let svg = crate::svg::render_svg(spec);
                files.insert(
                    VirtualPath::new(&path).expect("static chart path"),
                    Bytes::new(svg.into_bytes()),
                );
                let _ = writeln!(
                    out,
                    "\n#figure(\n  image(\"/{path}\", width: 100%),\n  caption: [Source: kronika · data as of {}],\n)\n",
                    esc(&fmt_date(meta.data_as_of_ns)),
                );
                chart_idx += 1;
            }
            Block::PageBreak => out.push_str("\n#pagebreak()\n"),
            Block::Footnote(t) => {
                let _ = writeln!(
                    out,
                    "\n#v(6pt)\n#text(size: 8pt, fill: luma(110))[{}]\n",
                    esc(t)
                );
            }
            Block::Signoff(entries) => signoff_block(&mut out, entries),
        }
    }

    (out, files)
}

fn cover(out: &mut String, meta: &DocMeta) {
    out.push_str("#text(size: 9pt, weight: \"bold\", tracking: 0.25em)[KRONIKA]\n");
    out.push_str("#v(52pt)\n");
    let _ = writeln!(
        out,
        "#text(font: \"Source Serif 4\", size: 25pt)[{}]",
        esc(&meta.title)
    );
    out.push_str("#v(10pt)\n");
    let _ = writeln!(
        out,
        "#text(size: 11pt, fill: luma(90))[{} #h(6pt) · #h(6pt) {}]",
        template_label(meta.template),
        esc(&meta.classification),
    );
    out.push_str("#v(30pt)\n");

    let mut rows: Vec<(String, String)> = vec![
        ("Document".into(), meta.doc_id.clone()),
        ("Version".into(), meta.version.clone()),
        ("Generated".into(), fmt_datetime(meta.generated_at_ns)),
        ("Data as of".into(), fmt_date(meta.data_as_of_ns)),
    ];
    if let Some((from, to)) = meta.period {
        rows.push((
            "Period".into(),
            format!("{} – {}", fmt_date(from), fmt_date(to)),
        ));
    }
    if let Some(f) = &meta.framework {
        rows.push(("Framework".into(), f.clone()));
    }
    if let Some(e) = &meta.experiment_id {
        rows.push(("Experiment".into(), e.clone()));
    }
    out.push_str("#table(\n  columns: (auto, 1fr),\n  stroke: none,\n  inset: (x: 0pt, y: 4pt),\n");
    for (i, (k, v)) in rows.iter().enumerate() {
        if i > 0 {
            out.push_str("  table.hline(stroke: 0.5pt + luma(210)),\n");
        }
        let _ = writeln!(
            out,
            "  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[{}]]], [{}],",
            esc(k),
            esc(v)
        );
    }
    out.push_str(")\n#pagebreak()\n");
}

fn kpis_block(out: &mut String, kpis: &[(String, String, Option<String>)]) {
    let cols = kpis.len().clamp(1, 4);
    let colspec = vec!["1fr"; cols].join(", ");
    let _ = writeln!(out, "\n#grid(\n  columns: ({colspec}),\n  gutter: 10pt,");
    for (label, value, sub) in kpis {
        out.push_str("  block(\n    width: 100%,\n    stroke: 0.5pt + luma(210),\n    radius: 3pt,\n    inset: (x: 10pt, y: 8pt),\n  )[\n");
        let _ = writeln!(
            out,
            "    #text(size: 7.5pt, fill: luma(100), tracking: 0.08em)[#smallcaps[{}]]",
            esc(label)
        );
        out.push_str("    #v(3pt)\n");
        let _ = writeln!(out, "    #text(size: 16pt, weight: \"bold\")[{}]", esc(value));
        if let Some(sub) = sub {
            out.push_str("    #v(2pt)\n");
            let _ = writeln!(out, "    #text(size: 8pt, fill: luma(100))[{}]", esc(sub));
        }
        out.push_str("  ],\n");
    }
    out.push_str(")\n");
}

fn kv_block(out: &mut String, kvs: &[(String, String)]) {
    out.push_str("\n#table(\n  columns: (auto, 1fr),\n  stroke: none,\n  inset: (x: 0pt, y: 3.5pt),\n  column-gutter: 16pt,\n");
    for (k, v) in kvs {
        let _ = writeln!(
            out,
            "  [#text(size: 9pt, fill: luma(100))[#smallcaps[{}]]], [{}],",
            esc(k),
            esc(v)
        );
    }
    out.push_str(")\n");
}

fn table_block(out: &mut String, headers: &[String], rows: &[Vec<String>], numeric_cols: &[usize]) {
    let n = headers.len().max(1);
    let aligns: Vec<&str> = (0..n)
        .map(|i| {
            if numeric_cols.contains(&i) {
                "right"
            } else {
                "left"
            }
        })
        .collect();
    let _ = writeln!(
        out,
        "\n#table(\n  columns: {n},\n  stroke: none,\n  inset: (x: 6pt, y: 4.5pt),\n  align: ({}),",
        aligns.join(", ")
    );
    out.push_str("  table.hline(stroke: 0.8pt + luma(140)),\n");
    out.push_str("  table.header(\n");
    for h in headers {
        let _ = writeln!(out, "    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[{}]]],", esc(h));
    }
    out.push_str("  ),\n  table.hline(stroke: 0.5pt + luma(200)),\n");
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if numeric_cols.contains(&i) {
                let _ = writeln!(out, "  [#text(number-width: \"tabular\")[{}]],", esc(cell));
            } else {
                let _ = writeln!(out, "  [{}],", esc(cell));
            }
        }
    }
    out.push_str("  table.hline(stroke: 0.8pt + luma(140)),\n)\n");
}

fn signoff_block(out: &mut String, entries: &[(String, String)]) {
    let cols = entries.len().clamp(1, 2);
    let colspec = vec!["1fr"; cols].join(", ");
    let _ = writeln!(out, "\n#v(18pt)\n#grid(\n  columns: ({colspec}),\n  gutter: 28pt,");
    for (role, name) in entries {
        out.push_str("  [\n    #v(26pt)\n    #line(length: 100%, stroke: 0.6pt + luma(110))\n    #v(2pt)\n");
        let _ = writeln!(
            out,
            "    #text(size: 8pt, fill: luma(100))[#smallcaps[{}]]",
            esc(role)
        );
        if !name.is_empty() {
            let _ = writeln!(out, "    #text(size: 9.5pt)[{}]", esc(name));
        }
        out.push_str("  ],\n");
    }
    out.push_str(")\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChartSpec, DocMeta, ReportDoc, TemplateKind};

    fn meta() -> DocMeta {
        DocMeta {
            doc_id: "KRK-R1-20260728-ab12cd".into(),
            title: "Executive resilience digest".into(),
            template: TemplateKind::ExecutiveDigest,
            version: "1.0".into(),
            classification: "Internal".into(),
            generated_at_ns: 1_785_273_590_000_000_000,
            data_as_of_ns: 1_785_273_590_000_000_000,
            period: Some((1_785_273_590_000_000_000 - 86_400_000_000_000 * 30, 1_785_273_590_000_000_000)),
            framework: None,
            experiment_id: None,
        }
    }

    #[test]
    fn escapes_markup_characters() {
        assert_eq!(esc("a-b#c[d]"), "a\\-b\\#c\\[d\\]");
        assert_eq!(esc("100%"), "100%");
    }

    #[test]
    fn executive_shaped_doc_renders_pdf_over_10kb() {
        let doc = ReportDoc {
            meta: meta(),
            blocks: vec![
                Block::H1("Bottom line".into()),
                Block::Paragraph(
                    "Portfolio resilience is good (82/100). One target regressed.".into(),
                ),
                Block::Kpis(vec![
                    ("Portfolio score".into(), "82".into(), Some("good".into())),
                    ("Delta 30d".into(), "+4".into(), None),
                    ("Open weaknesses".into(), "3".into(), None),
                ]),
                Block::H2("Target scores".into()),
                Block::Table {
                    headers: vec!["Target".into(), "Experiments".into(), "Score".into()],
                    rows: vec![
                        vec!["payments-api".into(), "6".into(), "88".into()],
                        vec!["ledger-db".into(), "2".into(), "50".into()],
                    ],
                    numeric_cols: vec![1, 2],
                },
                Block::Chart(ChartSpec::Bars(vec![
                    ("payments-api".into(), 88.0),
                    ("ledger-db".into(), 50.0),
                ])),
                Block::Bullets(vec!["Fix ledger-db timeout budget".into()]),
                Block::Footnote("Generated by kronika v0.4.0.".into()),
                Block::Signoff(vec![
                    ("Prepared by".into(), "kronika".into()),
                    ("Approved by".into(), String::new()),
                ]),
            ],
        };
        let pdf = crate::typst_pdf::render_pdf(&doc).expect("pdf render");
        assert!(pdf.starts_with(b"%PDF"), "not a pdf");
        assert!(pdf.len() > 10_000, "pdf too small: {} bytes", pdf.len());
    }
}
