//! Renderer-agnostic report content model.

/// Report template identifiers (URL/JSON-facing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateKind {
    /// R1 — CIO/board digest.
    ExecutiveDigest,
    /// R3 — per-experiment run report.
    GameDay,
    /// R2 — per-framework auditor evidence pack.
    EvidencePack,
}

impl TemplateKind {
    /// Short code used in document IDs (`KRK-R1-…`).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::ExecutiveDigest => "R1",
            Self::GameDay => "R3",
            Self::EvidencePack => "R2",
        }
    }
}

/// Document-control metadata carried on every artifact.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocMeta {
    /// e.g. `KRK-R1-20260728-ab12cd`.
    pub doc_id: String,
    pub title: String,
    pub template: TemplateKind,
    pub version: String,
    pub classification: String,
    pub generated_at_ns: i64,
    pub data_as_of_ns: i64,
    pub period: Option<(i64, i64)>,
    pub framework: Option<String>,
    pub experiment_id: Option<String>,
}

/// One content block of a report.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    H1(String),
    H2(String),
    Paragraph(String),
    /// (label, value, optional sub-line) KPI cards.
    Kpis(Vec<(String, String, Option<String>)>),
    /// Key/value definition list (document control, headers).
    KeyValues(Vec<(String, String)>),
    /// Tables: light horizontal rules, right-aligned numeric columns.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        numeric_cols: Vec<usize>,
    },
    Bullets(Vec<String>),
    Chart(ChartSpec),
    PageBreak,
    /// A fine-print note (e.g. the clause-verification footnote).
    Footnote(String),
    /// Signature lines (role label, name — empty for "____").
    Signoff(Vec<(String, String)>),
}

/// Vector chart specifications; rendered as SVG for both outputs.
#[derive(Debug, Clone, PartialEq)]
pub enum ChartSpec {
    /// (label, value) horizontal bars.
    Bars(Vec<(String, f64)>),
    /// (label, value) donut slices.
    Donut(Vec<(String, f64)>),
    /// (series name, (x, y) points) lines.
    Lines(Vec<(String, Vec<(f64, f64)>)>),
}

/// A complete report document.
#[derive(Debug, Clone)]
pub struct ReportDoc {
    pub meta: DocMeta,
    pub blocks: Vec<Block>,
}
