//! The trend of a metric: its value in each of a row of equal stretches of
//! time.

use std::collections::BTreeMap;

use serde_json::Value;
use tumult_metrics::Measure;

const SECOND: i64 = 1_000_000_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// The stretches of time one point can cover, as (nanoseconds, name), the
/// shortest first.
const WIDTHS: [(i64, &str); 6] = [
    (MINUTE, "minute"),
    (5 * MINUTE, "5 minutes"),
    (HOUR, "hour"),
    (DAY, "day"),
    (7 * DAY, "week"),
    (30 * DAY, "30 days"),
];

/// A trend has a point for each day of a month at most: more of them do
/// not fit the small line they are drawn as.
const MOST_POINTS: i64 = 31;

/// The stretch of time one point of a trend over `period` covers: the
/// shortest one that keeps the trend to [`MOST_POINTS`]. A trend over all
/// data is by day.
pub(crate) fn width_for(period: Option<(i64, i64)>) -> (i64, &'static str) {
    let Some((start, end)) = period else {
        return WIDTHS[3];
    };
    let length = end.saturating_sub(start);
    WIDTHS
        .into_iter()
        .find(|(width, _)| length <= MOST_POINTS * width)
        .unwrap_or(WIDTHS[5])
}

/// The point of a row of a bucketed query: the start of its stretch in
/// epoch nanoseconds, and its value. A stretch without a value has none.
pub(crate) fn point_of(row: &Value) -> Option<(i64, f64)> {
    let start_s = row.get("bucket_s")?.as_i64()?;
    Some((start_s * SECOND, row.get("value")?.as_f64()?))
}

/// Whether `measure` counts rows or adds them up: a stretch without a row
/// is then a stretch of zero, not a stretch without a value.
pub(crate) fn counts(measure: &Measure) -> bool {
    matches!(
        measure,
        Measure::Count | Measure::CountDistinct { .. } | Measure::Sum { .. }
    )
}

/// `found` with a zero for every stretch of `width` that has no point: all
/// stretches `period` touches, or without a period those from the first to
/// the last point.
pub(crate) fn filled(
    found: &[(i64, f64)],
    width: i64,
    period: Option<(i64, i64)>,
) -> Vec<(i64, f64)> {
    let start_of = |instant: i64| instant.div_euclid(width) * width;
    let ends = match period {
        Some((start, end)) => Some((start_of(start), start_of(end - 1))),
        None => found
            .first()
            .zip(found.last())
            .map(|(first, last)| (first.0, last.0)),
    };
    let Some((first, last)) = ends else {
        return Vec::new();
    };
    let known: BTreeMap<i64, f64> = found.iter().copied().collect();
    (0..=(last - first) / width)
        .map(|number| first + number * width)
        .map(|start| (start, known.get(&start).copied().unwrap_or(0.0)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_covers_the_shortest_stretch_that_keeps_a_trend_to_a_month_of_points() {
        for (length, width, name) in [
            (0, MINUTE, "minute"),
            (31 * MINUTE, MINUTE, "minute"),
            (31 * MINUTE + 1, 5 * MINUTE, "5 minutes"),
            (31 * 5 * MINUTE, 5 * MINUTE, "5 minutes"),
            (31 * 5 * MINUTE + 1, HOUR, "hour"),
            (31 * HOUR, HOUR, "hour"),
            (31 * HOUR + 1, DAY, "day"),
            (31 * DAY, DAY, "day"),
            (31 * DAY + 1, 7 * DAY, "week"),
            (31 * 7 * DAY, 7 * DAY, "week"),
            (31 * 7 * DAY + 1, 30 * DAY, "30 days"),
            (i64::MAX, 30 * DAY, "30 days"),
        ] {
            assert_eq!(width_for(Some((0, length))), (width, name), "{length}");
        }
        assert_eq!(width_for(None), (DAY, "day"));
    }

    #[test]
    fn a_point_is_the_start_of_its_stretch_and_its_value() {
        let row = serde_json::json!({"bucket_s": 86_400, "value": 2.5});

        assert_eq!(point_of(&row), Some((DAY, 2.5)));
    }

    #[test]
    fn a_stretch_without_a_value_has_no_point() {
        assert_eq!(
            point_of(&serde_json::json!({"bucket_s": 86_400, "value": null})),
            None
        );
        assert_eq!(point_of(&serde_json::json!({"value": 2.5})), None);
    }

    #[test]
    fn counts_and_sums_are_zero_where_nothing_is() {
        let column = || "c".to_string();

        assert!(counts(&Measure::Count));
        assert!(counts(&Measure::CountDistinct { column: column() }));
        assert!(counts(&Measure::Sum { column: column() }));
        assert!(!counts(&Measure::Avg { column: column() }));
        assert!(!counts(&Measure::Quantile {
            column: column(),
            quantile: 0.5
        }));
    }

    #[test]
    fn a_period_before_the_epoch_is_filled_from_the_start_of_its_stretches() {
        let filled = filled(&[], DAY, Some((-DAY - HOUR, -HOUR)));

        assert_eq!(filled, [(-2 * DAY, 0.0), (-DAY, 0.0)]);
    }

    #[test]
    fn a_period_that_ends_before_it_starts_has_no_points() {
        assert!(filled(&[(DAY, 1.0)], DAY, Some((3 * DAY, DAY))).is_empty());
    }

    #[test]
    fn nothing_found_in_all_data_is_no_trend() {
        assert!(filled(&[], DAY, None).is_empty());
    }
}
