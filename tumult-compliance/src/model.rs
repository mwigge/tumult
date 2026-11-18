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

/// A table/key-value cell: plain text or a status value rendered as
/// glyph + label (never hue alone).
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Status(String),
}

impl Cell {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }
    pub fn status(s: impl Into<String>) -> Self {
        Self::Status(s.into())
    }

    /// Glyph + Okabe-Ito color for a status/severity string, by its
    /// [`Tone`].
    #[must_use]
    pub fn glyph(status: &str) -> (&'static str, &'static str) {
        Tone::of(status).glyph()
    }
}

/// How a status cell reads: good, in between, bad, or no judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
    Neutral,
}

/// The tone of each status word that is not a status a run ends with:
/// manual outcomes, run states, severities, bands, hypothesis results and
/// OTel status codes. Any other word is neutral.
const WORD_TONES: [(&str, Tone); 17] = [
    ("passed", Tone::Good),
    ("ok", Tone::Good),
    ("low", Tone::Good),
    ("good", Tone::Good),
    ("met", Tone::Good),
    // A manual exercise that worked in part: between a pass and a failure.
    ("partial", Tone::Warn),
    ("stale", Tone::Warn),
    ("medium", Tone::Warn),
    ("warning", Tone::Warn),
    ("fair", Tone::Warn),
    ("error", Tone::Bad),
    ("high", Tone::Bad),
    ("critical", Tone::Bad),
    ("poor", Tone::Bad),
    ("not met", Tone::Bad),
    // A manual exercise that showed nothing either way, and a clause no
    // evidence is mapped to: no judgement.
    ("inconclusive", Tone::Neutral),
    ("unmapped", Tone::Neutral),
];

impl Tone {
    /// The tone of `status`, in any case. A status a run ends with has the
    /// tone of its verdict ([`tumult_lake::run_verdict`]); any other word
    /// has the tone [`WORD_TONES`] gives it.
    #[must_use]
    pub fn of(status: &str) -> Self {
        if let Some(verdict) = tumult_lake::run_verdict(status) {
            return verdict.into();
        }
        WORD_TONES
            .iter()
            .find(|(word, _)| word.eq_ignore_ascii_case(status))
            .map_or(Self::Neutral, |(_, tone)| *tone)
    }

    /// Glyph + Okabe-Ito color: green ● good, orange ▲ warn, vermillion
    /// × bad, grey ○ neutral. (Glyphs restricted to ● ▲ × ○ — present in
    /// the vendored Inter.)
    #[must_use]
    pub const fn glyph(self) -> (&'static str, &'static str) {
        match self {
            Self::Good => ("●", "#009E73"),
            Self::Warn => ("▲", "#E69F00"),
            Self::Bad => ("×", "#D55E00"),
            Self::Neutral => ("○", "#6B7280"),
        }
    }
}

impl From<tumult_lake::RunVerdict> for Tone {
    fn from(verdict: tumult_lake::RunVerdict) -> Self {
        match verdict {
            tumult_lake::RunVerdict::Pass => Self::Good,
            tumult_lake::RunVerdict::Fail => Self::Bad,
            tumult_lake::RunVerdict::NoEvidence => Self::Neutral,
        }
    }
}

impl From<String> for Cell {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

/// One content block of a report.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    H1(String),
    H2(String),
    H3(String),
    Paragraph(String),
    /// (label, value, optional sub-line) KPI cards.
    Kpis(Vec<(String, String, Option<String>)>),
    /// Key/value definition list (document control, headers).
    KeyValues(Vec<(String, Cell)>),
    /// Tables: light horizontal rules, right-aligned numeric columns,
    /// optional relative column widths (`[3.0, 1.0]` → `3fr, 1fr`).
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<Cell>>,
        numeric_cols: Vec<usize>,
        widths: Option<Vec<f64>>,
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
    /// Lines over time: what the y axis measures, and for each series its
    /// name and its (time in epoch ns, value) points.
    Trend {
        y_name: String,
        series: Vec<(String, Vec<(i64, f64)>)>,
    },
    /// A small line without axes, for the card of one number: the
    /// (time in epoch ns, value) points, the oldest first.
    Sparkline(Vec<(i64, f64)>),
}

/// A complete report document.
#[derive(Debug, Clone)]
pub struct ReportDoc {
    pub meta: DocMeta,
    pub blocks: Vec<Block>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_maps_outcomes_severities_and_bands() {
        for good in [
            "completed",
            "passed",
            "ok",
            "low",
            "good",
            "met",
            "Completed",
        ] {
            assert_eq!(Cell::glyph(good), ("●", "#009E73"), "{good}");
        }
        for warn in ["partial", "stale", "medium", "warning", "fair"] {
            assert_eq!(Cell::glyph(warn), ("▲", "#E69F00"), "{warn}");
        }
        for bad in ["failed", "error", "high", "critical", "poor", "not met"] {
            assert_eq!(Cell::glyph(bad), ("×", "#D55E00"), "{bad}");
        }
        for neutral in ["inconclusive", "Unmapped", "queued", "—"] {
            assert_eq!(Cell::glyph(neutral), ("○", "#6B7280"), "{neutral}");
        }
    }

    #[test]
    fn a_status_a_run_ends_with_is_drawn_in_the_tone_of_its_verdict() {
        // A run that passed is good, a run that failed is bad, and a run
        // that proves nothing either way is neutral, in either spelling.
        let good = ("●", "#009E73");
        let bad = ("×", "#D55E00");
        let neutral = ("○", "#6B7280");
        for (status, glyph) in [
            ("completed", good),
            ("deviated", bad),
            ("failed", bad),
            ("halted", bad),
            ("aborted", neutral),
            ("interrupted", neutral),
        ] {
            assert_eq!(Cell::glyph(status), glyph, "{status}");
            let shouted = status.to_ascii_uppercase();
            assert_eq!(Cell::glyph(&shouted), glyph, "{shouted}");
        }
    }

    #[test]
    fn cell_constructors_and_conversion() {
        assert_eq!(Cell::text("a"), Cell::Text("a".into()));
        assert_eq!(Cell::status("b"), Cell::Status("b".into()));
        assert_eq!(Cell::from("c".to_string()), Cell::Text("c".into()));
    }

    #[test]
    fn template_kind_codes_are_stable() {
        assert_eq!(TemplateKind::ExecutiveDigest.code(), "R1");
        assert_eq!(TemplateKind::GameDay.code(), "R3");
        assert_eq!(TemplateKind::EvidencePack.code(), "R2");
    }
}
