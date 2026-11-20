//! How the value of a metric is shown: a duration in a unit a reader
//! knows, a ratio as a percentage, a count as it is.

use tumult_metrics::Unit;

/// The units a duration is shown in, as (nanoseconds, short name, name),
/// the smallest first.
const DURATIONS: [(f64, &str, &str); 6] = [
    (1.0, "ns", "nanoseconds"),
    (1e3, "µs", "microseconds"),
    (1e6, "ms", "milliseconds"),
    (1e9, "s", "seconds"),
    (60e9, "min", "minutes"),
    (3_600e9, "h", "hours"),
];

/// The unit of no time at all: seconds.
const NO_TIME: (f64, &str, &str) = DURATIONS[3];

/// The largest unit a duration of `ns` nanoseconds fills.
fn unit_of(ns: f64) -> (f64, &'static str, &'static str) {
    if ns == 0.0 {
        return NO_TIME;
    }
    DURATIONS
        .into_iter()
        .rev()
        .find(|(size, ..)| ns.abs() >= *size)
        .unwrap_or(DURATIONS[0])
}

/// A plain number: whole when it is whole, with three decimals when not.
pub(crate) fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    format!("{value:.3}")
}

/// A duration of `ns` nanoseconds in the largest unit it fills.
fn duration(ns: f64) -> String {
    if ns == 0.0 {
        return "0 s".to_string();
    }
    let (size, name, _) = unit_of(ns);
    format!("{:.1} {name}", ns / size)
}

/// The text of `value` in `unit`.
pub(crate) fn value(unit: Option<Unit>, value: f64) -> String {
    match unit {
        Some(Unit::Seconds) => duration(value * 1e9),
        Some(Unit::Nanoseconds) => duration(value),
        Some(Unit::Ratio) => format!("{:.1}%", value * 100.0),
        Some(Unit::Count) | None => number(value),
    }
}

/// The text of a difference of `size` between two values in `unit`. Two
/// ratios differ by percentage points, not by a percentage.
fn difference(unit: Option<Unit>, size: f64) -> String {
    match unit {
        Some(Unit::Ratio) => format!("{:.1} points", size * 100.0),
        _ => value(unit, size),
    }
}

/// What a value did against the one of the period before.
pub(crate) fn change(unit: Option<Unit>, now: f64, before: Option<f64>) -> String {
    let Some(before) = before else {
        return "no data in the period before".to_string();
    };
    let size = difference(unit, (now - before).abs());
    // A change too small to show is no change.
    if !size.chars().any(|digit| matches!(digit, '1'..='9')) {
        return "same as the period before".to_string();
    }
    let sign = if now > before { '+' } else { '-' };
    format!("{sign}{size} vs the period before")
}

/// The `values` of a metric in `unit` as the bars of one chart, and the
/// name of the unit the bars are in. Durations are all in the unit the
/// largest of them fills, ratios are percentages, a count has no unit.
pub(crate) fn bars(unit: Option<Unit>, values: &[f64]) -> (Vec<f64>, Option<&'static str>) {
    let ns_in_one = match unit {
        Some(Unit::Seconds) => 1e9,
        Some(Unit::Nanoseconds) => 1.0,
        Some(Unit::Ratio) => return (values.iter().map(|v| v * 100.0).collect(), Some("%")),
        Some(Unit::Count) | None => return (values.to_vec(), None),
    };
    let largest = values.iter().fold(0.0_f64, |largest, value| {
        largest.max((value * ns_in_one).abs())
    });
    let (size, _, name) = unit_of(largest);
    let bars = values.iter().map(|v| v * ns_in_one / size).collect();
    (bars, Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_number_is_whole_when_it_is_whole() {
        assert_eq!(number(42.0), "42");
        assert_eq!(number(-3.0), "-3");
        assert_eq!(number(2.5), "2.500");
        // A whole number too large to be exact keeps its decimals.
        assert_eq!(number(1e15), "1000000000000000.000");
        assert_eq!(number(-1e15), "-1000000000000000.000");
    }

    #[test]
    fn a_duration_is_in_the_largest_unit_it_fills() {
        let shown = |ns| value(Some(Unit::Nanoseconds), ns);

        assert_eq!(shown(0.0), "0 s");
        assert_eq!(shown(250.0), "250.0 ns");
        assert_eq!(shown(999.0), "999.0 ns");
        assert_eq!(shown(1e3), "1.0 µs");
        assert_eq!(shown(36_747_737.667), "36.7 ms");
        assert_eq!(shown(1e9), "1.0 s");
        assert_eq!(shown(59e9), "59.0 s");
        assert_eq!(shown(60e9), "1.0 min");
        assert_eq!(shown(5_400e9), "1.5 h");
    }

    #[test]
    fn a_duration_below_zero_keeps_its_unit_and_its_sign() {
        assert_eq!(value(Some(Unit::Nanoseconds), -2e9), "-2.0 s");
    }

    #[test]
    fn a_value_in_seconds_is_a_duration() {
        assert_eq!(value(Some(Unit::Seconds), 0.25), "250.0 ms");
        assert_eq!(value(Some(Unit::Seconds), 90.0), "1.5 min");
    }

    #[test]
    fn a_ratio_is_a_percentage() {
        assert_eq!(value(Some(Unit::Ratio), 0.875), "87.5%");
    }

    #[test]
    fn a_count_and_a_value_without_a_unit_are_plain_numbers() {
        assert_eq!(value(Some(Unit::Count), 7.0), "7");
        assert_eq!(value(None, 0.875), "0.875");
    }

    #[test]
    fn a_change_says_its_direction_and_its_size_in_the_unit() {
        assert_eq!(
            change(Some(Unit::Count), 5.0, Some(3.0)),
            "+2 vs the period before"
        );
        assert_eq!(
            change(Some(Unit::Seconds), 1.0, Some(3.5)),
            "-2.5 s vs the period before"
        );
        assert_eq!(change(None, 1.5, Some(1.0)), "+0.500 vs the period before");
    }

    #[test]
    fn two_ratios_differ_by_points() {
        assert_eq!(
            change(Some(Unit::Ratio), 0.5, Some(0.75)),
            "-25.0 points vs the period before"
        );
    }

    #[test]
    fn a_change_that_shows_as_zero_is_no_change() {
        assert_eq!(
            change(Some(Unit::Count), 3.0, Some(3.0)),
            "same as the period before"
        );
        assert_eq!(
            change(Some(Unit::Ratio), 0.5, Some(0.5004)),
            "same as the period before"
        );
        assert_eq!(change(None, 1.0, Some(1.0004)), "same as the period before");
    }

    #[test]
    fn a_value_with_nothing_before_it_claims_no_change() {
        assert_eq!(
            change(Some(Unit::Count), 3.0, None),
            "no data in the period before"
        );
    }

    #[test]
    fn bars_of_durations_are_in_the_unit_the_largest_one_fills() {
        let ns = Some(Unit::Nanoseconds);
        let s = Some(Unit::Seconds);

        assert_eq!(
            bars(ns, &[750.0, 250.0]),
            (vec![750.0, 250.0], Some("nanoseconds"))
        );
        assert_eq!(
            bars(ns, &[2_500.0, 500.0]),
            (vec![2.5, 0.5], Some("microseconds"))
        );
        assert_eq!(
            bars(ns, &[250e6, 40e6]),
            (vec![250.0, 40.0], Some("milliseconds"))
        );
        assert_eq!(bars(ns, &[40e6, 2.5e9]), (vec![0.04, 2.5], Some("seconds")));
        assert_eq!(
            bars(s, &[0.25, 0.5]),
            (vec![250.0, 500.0], Some("milliseconds"))
        );
        assert_eq!(bars(s, &[2.5, 30.0]), (vec![2.5, 30.0], Some("seconds")));
        assert_eq!(bars(s, &[90.0, 30.0]), (vec![1.5, 0.5], Some("minutes")));
        assert_eq!(
            bars(s, &[5_400.0, 1_800.0]),
            (vec![1.5, 0.5], Some("hours"))
        );
    }

    #[test]
    fn bars_of_durations_below_zero_are_sized_by_how_far_from_zero_they_are() {
        assert_eq!(
            bars(Some(Unit::Seconds), &[-90.0, 30.0]),
            (vec![-1.5, 0.5], Some("minutes"))
        );
    }

    #[test]
    fn bars_of_no_time_at_all_are_in_seconds() {
        assert_eq!(
            bars(Some(Unit::Seconds), &[0.0]),
            (vec![0.0], Some("seconds"))
        );
        assert_eq!(
            bars(Some(Unit::Nanoseconds), &[0.0]),
            (vec![0.0], Some("seconds"))
        );
    }

    #[test]
    fn bars_of_ratios_are_percentages() {
        assert_eq!(
            bars(Some(Unit::Ratio), &[0.5, 0.125]),
            (vec![50.0, 12.5], Some("%"))
        );
    }

    #[test]
    fn bars_of_counts_are_the_counts() {
        assert_eq!(bars(Some(Unit::Count), &[3.0, 1.0]), (vec![3.0, 1.0], None));
        assert_eq!(bars(None, &[3.0, 1.0]), (vec![3.0, 1.0], None));
    }
}
