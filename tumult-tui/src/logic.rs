//! Pure, side-effect-free helpers: status classification, sort/filter of the
//! experiment history, block-ramp micro-bars, block sparklines, trend series,
//! and value formatting. Everything here is deterministic and unit-tested so
//! the rendering layer stays a thin projection over verified logic.

use tumult_core::types::{ActivityStatus, ExperimentStatus};
use tumult_lake::RunVerdict;

use crate::model::ExperimentRow;

/// The tone a status is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// A run that passed, or a step that succeeded.
    Pass,
    /// A run that failed, or a step that failed or timed out.
    Fail,
    /// A run that proves nothing either way, a step that did not run, or a
    /// status the TUI does not know.
    Neutral,
    /// A run that has not ended.
    Running,
}

impl From<RunVerdict> for Tone {
    fn from(verdict: RunVerdict) -> Self {
        match verdict {
            RunVerdict::Pass => Self::Pass,
            RunVerdict::Fail => Self::Fail,
            RunVerdict::NoEvidence => Self::Neutral,
        }
    }
}

/// Normalised outcome class for an experiment or activity status string:
/// the word of its pill and the tone it is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusKind {
    label: &'static str,
    tone: Tone,
}

/// The pill word of each status a run ends with, in display order.
const RUN_PILLS: [(ExperimentStatus, &str); 6] = [
    (ExperimentStatus::Completed, "PASS"),
    (ExperimentStatus::Deviated, "DEVIATED"),
    (ExperimentStatus::Failed, "FAIL"),
    (ExperimentStatus::Halted, "HALTED"),
    (ExperimentStatus::Aborted, "ABORTED"),
    (ExperimentStatus::Interrupted, "INTERRUPTED"),
];

/// The pill of each status a step ends with.
const STEP_PILLS: [(ActivityStatus, StatusKind); 3] = [
    (
        ActivityStatus::Succeeded,
        StatusKind::new("PASS", Tone::Pass),
    ),
    (ActivityStatus::Failed, StatusKind::new("FAIL", Tone::Fail)),
    (
        ActivityStatus::Skipped,
        StatusKind::new("SKIPPED", Tone::Neutral),
    ),
];

impl StatusKind {
    /// A run that has not ended.
    pub const RUNNING: Self = Self::new("RUNNING", Tone::Running);
    /// A status the TUI does not know.
    pub const OTHER: Self = Self::new("UNKNOWN", Tone::Neutral);

    const fn new(label: &'static str, tone: Tone) -> Self {
        Self { label, tone }
    }

    /// The kind of the status a run is stored with (case-insensitive). A
    /// run that ended has the tone of its verdict
    /// ([`tumult_lake::run_verdict`]).
    #[must_use]
    pub fn of_run(status: &str) -> Self {
        if let Some(verdict) = tumult_lake::run_verdict(status) {
            return RUN_PILLS
                .iter()
                .find(|(ending, _)| ending.to_string().eq_ignore_ascii_case(status))
                .map_or(Self::OTHER, |(_, label)| Self::new(label, verdict.into()));
        }
        match status.to_ascii_lowercase().as_str() {
            "running" | "in_progress" | "started" => Self::RUNNING,
            _ => Self::OTHER,
        }
    }

    /// The kind of the status a step is stored with (case-insensitive).
    #[must_use]
    pub fn of_step(status: &str) -> Self {
        STEP_PILLS
            .iter()
            .find(|(step, _)| step.to_string().eq_ignore_ascii_case(status))
            .map_or(Self::OTHER, |(_, kind)| *kind)
    }

    /// Short uppercase pill label.
    #[must_use]
    pub fn label(self) -> &'static str {
        self.label
    }

    /// The tone the pill is drawn in.
    #[must_use]
    pub fn tone(self) -> Tone {
        self.tone
    }

    /// Whether this outcome counts as a success for success-rate trends.
    #[must_use]
    pub fn is_success(self) -> bool {
        self.tone == Tone::Pass
    }
}

/// Sort keys the history table cycles through with the `s` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Time,
    Duration,
    Status,
    HypothesisMet,
}

impl SortKey {
    /// Cycle to the next sort key.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Time => Self::Duration,
            Self::Duration => Self::Status,
            Self::Status => Self::HypothesisMet,
            Self::HypothesisMet => Self::Time,
        }
    }

    /// Human label for the status bar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Duration => "duration",
            Self::Status => "status",
            Self::HypothesisMet => "hypothesis met",
        }
    }
}

/// Sort `rows` in place by `key`. `ascending` flips the direction; the default
/// (descending) puts the most-recent / longest / worst first.
pub fn sort_experiments(rows: &mut [ExperimentRow], key: SortKey, ascending: bool) {
    match key {
        SortKey::Time => rows.sort_by_key(|a| a.started_at_ns),
        SortKey::Duration => rows.sort_by_key(|a| a.duration_ms),
        SortKey::Status => rows.sort_by(|a, b| {
            a.status
                .cmp(&b.status)
                .then(b.started_at_ns.cmp(&a.started_at_ns))
        }),
        // Not known, then not met, then met: descending puts met first.
        SortKey::HypothesisMet => rows.sort_by_key(|a| a.hypothesis_met),
    }
    if !ascending {
        rows.reverse();
    }
}

/// Whether `row` passes the active status + title-substring filter.
///
/// `status_filter` matches against the normalised [`StatusKind`] label
/// (case-insensitive substring); an empty string matches all. `title_query`
/// is a case-insensitive substring over the title; empty matches all.
#[must_use]
pub fn matches_filter(row: &ExperimentRow, status_filter: &str, title_query: &str) -> bool {
    let status_ok = status_filter.is_empty()
        || StatusKind::of_run(&row.status)
            .label()
            .to_ascii_lowercase()
            .contains(&status_filter.to_ascii_lowercase())
        || row
            .status
            .to_ascii_lowercase()
            .contains(&status_filter.to_ascii_lowercase());
    let title_ok = title_query.is_empty()
        || row
            .title
            .to_ascii_lowercase()
            .contains(&title_query.to_ascii_lowercase());
    status_ok && title_ok
}

/// Return the indices of `rows` that pass the filter, preserving order.
#[must_use]
pub fn filter_indices(
    rows: &[ExperimentRow],
    status_filter: &str,
    title_query: &str,
) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| matches_filter(r, status_filter, title_query))
        .map(|(i, _)| i)
        .collect()
}

/// Eight-step block ramp used for the fractional cell of a micro-bar.
const RAMP: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Render a proportional horizontal bar of `width` cells for `value` relative
/// to `max`, using eighth-block characters for sub-cell precision. A dolphie
/// style `microbar()`. `max == 0` or `width == 0` yields an all-blank bar.
#[must_use]
pub fn microbar(value: f64, max: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if max <= 0.0 || value <= 0.0 {
        return " ".repeat(width);
    }
    let frac = (value / max).clamp(0.0, 1.0);
    // Total eighths of fill across the whole bar.
    let total_eighths = (frac * (width as f64) * 8.0).round() as usize;
    let full = total_eighths / 8;
    let remainder = total_eighths % 8;
    let mut out = String::with_capacity(width);
    for _ in 0..full.min(width) {
        out.push('█');
    }
    if full < width {
        out.push(RAMP[remainder]);
        for _ in (full + 1)..width {
            out.push(' ');
        }
    }
    out
}

/// Render a compact block sparkline (one cell per bucket) for `values`,
/// down-/up-sampling to at most `width` cells. Each cell is a ▁-█ ramp scaled
/// to the series maximum. Empty input or `width == 0` yields an empty string.
#[must_use]
pub fn sparkline(values: &[f64], width: usize) -> String {
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let max = values.iter().copied().fold(f64::MIN, f64::max);
    if max <= 0.0 {
        return "▁".repeat(values.len().min(width));
    }
    // Bucket the series into at most `width` columns, averaging each bucket.
    let cells = values.len().min(width);
    let mut out = String::with_capacity(cells);
    for i in 0..cells {
        let start = i * values.len() / cells;
        let end = ((i + 1) * values.len() / cells).max(start + 1);
        let slice = &values[start..end.min(values.len())];
        let avg = slice.iter().sum::<f64>() / (slice.len() as f64);
        let idx = ((avg / max) * 8.0).round().clamp(1.0, 8.0) as usize;
        out.push(RAMP[idx]);
    }
    out
}

/// Chronological (oldest→newest) success flags mapped to 0.0/1.0 for a trend.
#[must_use]
pub fn success_series(chronological: &[ExperimentRow]) -> Vec<f64> {
    chronological
        .iter()
        .map(|r| {
            if StatusKind::of_run(&r.status).is_success() {
                1.0
            } else {
                0.0
            }
        })
        .collect()
}

/// Chronological duration series in milliseconds (as `f64` for charting).
#[must_use]
pub fn duration_series(chronological: &[ExperimentRow]) -> Vec<f64> {
    chronological.iter().map(|r| r.duration_ms as f64).collect()
}

/// Chronological hypothesis verdicts: 1.0 when met, 0.0 when not. A run
/// that judged no hypothesis is left out: it is not a "no".
#[must_use]
pub fn hypothesis_met_series(chronological: &[ExperimentRow]) -> Vec<f64> {
    chronological
        .iter()
        .filter_map(|r| r.hypothesis_met)
        .map(|met| if met { 1.0 } else { 0.0 })
        .collect()
}

/// Overall success rate (0.0–1.0) across `rows`. Empty input yields 0.0.
#[must_use]
pub fn success_rate(rows: &[ExperimentRow]) -> f64 {
    if rows.is_empty() {
        return 0.0;
    }
    let ok = rows
        .iter()
        .filter(|r| StatusKind::of_run(&r.status).is_success())
        .count();
    (ok as f64) / (rows.len() as f64)
}

/// Count experiments per normalised status kind, returned in a stable display
/// order (the endings of a run in the order of `RUN_PILLS`, then Running,
/// then Other), skipping zeros.
#[must_use]
pub fn status_breakdown(rows: &[ExperimentRow]) -> Vec<(StatusKind, usize)> {
    RUN_PILLS
        .iter()
        .map(|(ending, _)| StatusKind::of_run(&ending.to_string()))
        .chain([StatusKind::RUNNING, StatusKind::OTHER])
        .filter_map(|kind| {
            let n = rows
                .iter()
                .filter(|r| StatusKind::of_run(&r.status) == kind)
                .count();
            (n > 0).then_some((kind, n))
        })
        .collect()
}

/// Format a nanoseconds-since-epoch timestamp as `MM-DD HH:MM:SS` in UTC.
/// A non-positive timestamp renders as a dash placeholder.
#[must_use]
pub fn format_time(started_at_ns: i64) -> String {
    if started_at_ns <= 0 {
        return "—".to_string();
    }
    let secs = started_at_ns / 1_000_000_000;
    let nsec = (started_at_ns % 1_000_000_000) as u32;
    chrono::DateTime::from_timestamp(secs, nsec).map_or_else(
        || "—".to_string(),
        |dt| dt.format("%m-%d %H:%M:%S").to_string(),
    )
}

/// Format a millisecond duration compactly (`ms` under 1s, else `s`).
#[must_use]
pub fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1000 {
        format!("{duration_ms}ms")
    } else {
        format!("{:.1}s", duration_ms as f64 / 1000.0)
    }
}

/// Whether the hypothesis of a run was met: `yes` or `no`, and a dash for
/// a run that judged none.
#[must_use]
pub fn format_hypothesis_met(hypothesis_met: Option<bool>) -> String {
    match hypothesis_met {
        Some(true) => "yes",
        Some(false) => "no",
        None => "—",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp(id: &str, status: &str, ns: i64, dur: u64, met: Option<bool>) -> ExperimentRow {
        ExperimentRow {
            id: id.into(),
            title: format!("Experiment {id}"),
            status: status.into(),
            started_at_ns: ns,
            duration_ms: dur,
            hypothesis_met: met,
            steps: 1,
            deviations: 0,
        }
    }

    #[test]
    fn classify_covers_known_statuses() {
        let pill = |kind: StatusKind| (kind.label(), kind.tone());
        assert_eq!(pill(StatusKind::of_run("completed")), ("PASS", Tone::Pass));
        assert_eq!(
            pill(StatusKind::of_run("DEVIATED")),
            ("DEVIATED", Tone::Fail)
        );
        assert_eq!(pill(StatusKind::of_run("failed")), ("FAIL", Tone::Fail));
        assert_eq!(pill(StatusKind::of_run("Halted")), ("HALTED", Tone::Fail));
        assert_eq!(
            pill(StatusKind::of_run("aborted")),
            ("ABORTED", Tone::Neutral)
        );
        assert_eq!(
            pill(StatusKind::of_run("interrupted")),
            ("INTERRUPTED", Tone::Neutral)
        );
        assert_eq!(StatusKind::of_run("running"), StatusKind::RUNNING);
        assert_eq!(StatusKind::of_run("weird"), StatusKind::OTHER);
        assert_eq!(pill(StatusKind::of_step("succeeded")), ("PASS", Tone::Pass));
        assert_eq!(pill(StatusKind::of_step("Failed")), ("FAIL", Tone::Fail));
        assert_eq!(
            pill(StatusKind::of_step("skipped")),
            ("SKIPPED", Tone::Neutral)
        );
        assert_eq!(StatusKind::of_step("completed"), StatusKind::OTHER);
    }

    #[test]
    fn sort_by_time_descending_puts_newest_first() {
        let mut rows = vec![
            exp("a", "completed", 100, 10, None),
            exp("b", "completed", 300, 10, None),
            exp("c", "completed", 200, 10, None),
        ];
        sort_experiments(&mut rows, SortKey::Time, false);
        assert_eq!(rows[0].id, "b");
        assert_eq!(rows[2].id, "a");
    }

    #[test]
    fn sort_by_duration_ascending() {
        let mut rows = vec![
            exp("a", "completed", 1, 30, None),
            exp("b", "completed", 2, 10, None),
        ];
        sort_experiments(&mut rows, SortKey::Duration, true);
        assert_eq!(rows[0].id, "b");
    }

    #[test]
    fn sort_by_hypothesis_puts_met_first_then_not_met_then_not_known() {
        let mut rows = vec![
            exp("a", "aborted", 1, 1, Some(false)),
            exp("b", "completed", 2, 1, None),
            exp("c", "failed", 3, 1, Some(true)),
        ];
        sort_experiments(&mut rows, SortKey::HypothesisMet, false);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    #[test]
    fn filter_matches_status_and_title() {
        let rows = vec![
            exp("a", "completed", 1, 1, None),
            exp("b", "deviated", 2, 1, None),
        ];
        // Status filter by pill label.
        let idx = filter_indices(&rows, "deviated", "");
        assert_eq!(idx, vec![1]);
        // Title substring.
        let idx = filter_indices(&rows, "", "experiment a");
        assert_eq!(idx, vec![0]);
        // Empty filter matches all.
        assert_eq!(filter_indices(&rows, "", "").len(), 2);
    }

    #[test]
    fn microbar_full_and_empty() {
        assert_eq!(microbar(10.0, 10.0, 4), "████");
        assert_eq!(microbar(0.0, 10.0, 4), "    ");
        assert_eq!(microbar(5.0, 0.0, 4), "    ");
        assert_eq!(microbar(1.0, 1.0, 0), "");
    }

    #[test]
    fn microbar_half_is_partly_filled() {
        let bar = microbar(1.0, 2.0, 4);
        assert_eq!(bar.chars().count(), 4);
        // Half of a 4-cell bar → two full blocks.
        assert!(bar.starts_with("██"));
    }

    #[test]
    fn sparkline_length_bounded_by_width() {
        let vals = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let s = sparkline(&vals, 3);
        assert_eq!(s.chars().count(), 3);
    }

    #[test]
    fn sparkline_peaks_render_taller_than_troughs() {
        let s = sparkline(&[1.0, 8.0], 2);
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars.len(), 2);
        assert!(
            RAMP.iter().position(|c| *c == chars[1]) > RAMP.iter().position(|c| *c == chars[0])
        );
    }

    #[test]
    fn success_rate_and_series() {
        let rows = vec![
            exp("a", "completed", 1, 1, None),
            exp("b", "deviated", 2, 1, None),
            exp("c", "completed", 3, 1, None),
        ];
        assert!((success_rate(&rows) - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(success_series(&rows), vec![1.0, 0.0, 1.0]);
    }

    #[test]
    fn status_breakdown_counts_and_skips_zero() {
        let rows = vec![
            exp("a", "completed", 1, 1, None),
            exp("b", "completed", 2, 1, None),
            exp("c", "deviated", 3, 1, None),
        ];
        let bd = status_breakdown(&rows);
        assert_eq!(
            bd,
            vec![
                (StatusKind::of_run("completed"), 2),
                (StatusKind::of_run("deviated"), 1)
            ]
        );
    }

    #[test]
    fn format_helpers() {
        assert_eq!(format_duration(500), "500ms");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_hypothesis_met(Some(true)), "yes");
        assert_eq!(format_hypothesis_met(Some(false)), "no");
        assert_eq!(format_hypothesis_met(None), "—");
        assert_eq!(format_time(0), "—");
        // A known epoch instant renders deterministically in UTC.
        assert_eq!(format_time(1_609_459_200_000_000_000), "01-01 00:00:00");
    }
}
