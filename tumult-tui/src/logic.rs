//! Pure, side-effect-free helpers: status classification, sort/filter of the
//! experiment history, block-ramp micro-bars, block sparklines, trend series,
//! and value formatting. Everything here is deterministic and unit-tested so
//! the rendering layer stays a thin projection over verified logic.

use crate::model::ExperimentRow;

/// Normalised outcome class for an experiment or activity status string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Pass,
    Deviated,
    Failed,
    Aborted,
    Running,
    Other,
}

impl StatusKind {
    /// Classify a raw store status string (case-insensitive).
    #[must_use]
    pub fn classify(status: &str) -> Self {
        match status.to_ascii_lowercase().as_str() {
            "completed" | "succeeded" | "passed" | "pass" => Self::Pass,
            "deviated" | "deviation" => Self::Deviated,
            "failed" | "error" => Self::Failed,
            "aborted" | "halted" | "cancelled" | "canceled" => Self::Aborted,
            "running" | "in_progress" | "started" => Self::Running,
            _ => Self::Other,
        }
    }

    /// Short uppercase pill label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Deviated => "DEVIATED",
            Self::Failed => "FAIL",
            Self::Aborted => "ABORTED",
            Self::Running => "RUNNING",
            Self::Other => "UNKNOWN",
        }
    }

    /// Whether this outcome counts as a success for success-rate trends.
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Pass)
    }
}

/// Sort keys the history table cycles through with the `s` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Time,
    Duration,
    Status,
    Resilience,
}

impl SortKey {
    /// Cycle to the next sort key.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Time => Self::Duration,
            Self::Duration => Self::Status,
            Self::Status => Self::Resilience,
            Self::Resilience => Self::Time,
        }
    }

    /// Human label for the status bar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Duration => "duration",
            Self::Status => "status",
            Self::Resilience => "resilience",
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
        SortKey::Resilience => rows.sort_by(|a, b| {
            a.resilience
                .unwrap_or(f64::MIN)
                .partial_cmp(&b.resilience.unwrap_or(f64::MIN))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
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
        || StatusKind::classify(&row.status)
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
            if StatusKind::classify(&r.status).is_success() {
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

/// Chronological resilience series; missing scores contribute 0.0.
#[must_use]
pub fn resilience_series(chronological: &[ExperimentRow]) -> Vec<f64> {
    chronological
        .iter()
        .map(|r| r.resilience.unwrap_or(0.0))
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
        .filter(|r| StatusKind::classify(&r.status).is_success())
        .count();
    (ok as f64) / (rows.len() as f64)
}

/// Count experiments per normalised status kind, returned in a stable display
/// order (Pass, Deviated, Failed, Aborted, Running, Other), skipping zeros.
#[must_use]
pub fn status_breakdown(rows: &[ExperimentRow]) -> Vec<(StatusKind, usize)> {
    const ORDER: [StatusKind; 6] = [
        StatusKind::Pass,
        StatusKind::Deviated,
        StatusKind::Failed,
        StatusKind::Aborted,
        StatusKind::Running,
        StatusKind::Other,
    ];
    ORDER
        .iter()
        .filter_map(|kind| {
            let n = rows
                .iter()
                .filter(|r| StatusKind::classify(&r.status) == *kind)
                .count();
            (n > 0).then_some((*kind, n))
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

/// Format an optional resilience score as a two-decimal value or a dash.
#[must_use]
pub fn format_resilience(resilience: Option<f64>) -> String {
    resilience.map_or_else(|| "—".to_string(), |v| format!("{v:.2}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp(id: &str, status: &str, ns: i64, dur: u64, res: Option<f64>) -> ExperimentRow {
        ExperimentRow {
            id: id.into(),
            title: format!("Experiment {id}"),
            status: status.into(),
            started_at_ns: ns,
            duration_ms: dur,
            resilience: res,
            steps: 1,
            deviations: 0,
        }
    }

    #[test]
    fn classify_covers_known_statuses() {
        assert_eq!(StatusKind::classify("completed"), StatusKind::Pass);
        assert_eq!(StatusKind::classify("DEVIATED"), StatusKind::Deviated);
        assert_eq!(StatusKind::classify("failed"), StatusKind::Failed);
        assert_eq!(StatusKind::classify("aborted"), StatusKind::Aborted);
        assert_eq!(StatusKind::classify("running"), StatusKind::Running);
        assert_eq!(StatusKind::classify("weird"), StatusKind::Other);
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
    fn sort_by_resilience_descending_handles_none() {
        let mut rows = vec![
            exp("a", "completed", 1, 1, Some(0.5)),
            exp("b", "completed", 2, 1, None),
            exp("c", "completed", 3, 1, Some(0.9)),
        ];
        sort_experiments(&mut rows, SortKey::Resilience, false);
        assert_eq!(rows[0].id, "c");
        assert_eq!(rows[2].id, "b"); // None sorts lowest
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
        assert_eq!(bd, vec![(StatusKind::Pass, 2), (StatusKind::Deviated, 1)]);
    }

    #[test]
    fn format_helpers() {
        assert_eq!(format_duration(500), "500ms");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_resilience(Some(0.833)), "0.83");
        assert_eq!(format_resilience(None), "—");
        assert_eq!(format_time(0), "—");
        // A known epoch instant renders deterministically in UTC.
        assert_eq!(format_time(1_609_459_200_000_000_000), "01-01 00:00:00");
    }
}
