//! What a metric digest says about the rows of a real store.
//!
//! Each test writes spans into a store on disk and builds the digest with
//! the SQL the daemon runs. The numbers are small on purpose, so that the
//! right answer can be worked out by hand.

use std::time::Duration;
use tumult_lake::{Reader, SpanRow, Store};
use tumult_metrics::{Measure, MetricDef, Unit};

use tumult_report::{
    build_report, build_report_scoped, digest_title, render_html, Report, Section, Trend,
};

const SECOND: i64 = 1_000_000_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
/// 2026-11-18 00:00 UTC: the end of the week the digests are about.
const END: i64 = 20_775 * DAY;
const THIS_WEEK: (i64, i64) = (END - 7 * DAY, END);

/// The start of day `number` of the week, counted from 0.
fn day(number: i64) -> i64 {
    THIS_WEEK.0 + number * DAY
}

/// A store on disk that holds `spans`, and the directory it lives in.
fn store_of(spans: &[SpanRow]) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("a directory");
    let store = Store::open(&dir.path().join("lake.duckdb")).expect("a store");
    store
        .writer()
        .expect("a writer")
        .insert_spans(spans)
        .expect("the spans");
    (dir, store)
}

fn reader_of(store: &Store) -> Reader {
    store.read_only().expect("a reader")
}

/// The root span of run `number` of `experiment`, started at `at`.
fn run_at(experiment: &str, number: usize, at: i64) -> SpanRow {
    SpanRow {
        ts_ns: at,
        trace_id: format!("trace-{experiment}-{number}"),
        span_id: format!("root-{experiment}-{number}"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        service_name: "tumult".into(),
        experiment_id: Some(format!("{experiment}-{number}")),
        experiment_name: Some(experiment.into()),
        target_environment: Some("dev".into()),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

/// `count` runs of `experiment`, a minute apart from `at` on.
fn runs(experiment: &str, count: usize, at: i64) -> Vec<SpanRow> {
    (0..count)
        .map(|number| {
            let offset = i64::try_from(number).expect("a small number") * MINUTE;
            let mut run = run_at(experiment, number, at + offset);
            run.span_id = format!("root-{experiment}-{at}-{number}");
            run
        })
        .collect()
}

/// A run that came back after `recovery_s` seconds.
fn recovered(experiment: &str, at: i64, recovery_s: f64) -> SpanRow {
    SpanRow {
        recovery_time_s: Some(recovery_s),
        span_id: format!("root-{experiment}-{at}"),
        ..run_at(experiment, 0, at)
    }
}

/// The span of an action of `plugin` that took `duration_ms`.
fn action(plugin: &str, number: i64, at: i64, duration_ms: i64) -> SpanRow {
    SpanRow {
        ts_ns: at,
        trace_id: format!("trace-{plugin}"),
        span_id: format!("action-{plugin}-{number}"),
        span_name: "resilience.action".into(),
        span_kind: "Internal".into(),
        service_name: "tumult".into(),
        duration_ns: duration_ms * 1_000_000,
        plugin_name: Some(plugin.into()),
        events: "[]".into(),
        ..SpanRow::default()
    }
}

fn metric(name: &str, unit: Unit, measure: Measure, span_name: &str) -> MetricDef {
    MetricDef {
        name: name.into(),
        description: Some(format!("What {name} measures.")),
        unit: Some(unit),
        source_table: "spans".into(),
        measure,
        dimensions: Vec::new(),
        time_col: "ts_ns".into(),
        condition: Some(tumult_metrics::Condition {
            column: "span_name".into(),
            equals: tumult_metrics::Literal::Str(span_name.into()),
        }),
    }
}

/// The number of runs.
fn run_count() -> MetricDef {
    metric(
        "experiment_count",
        Unit::Count,
        Measure::Count,
        "resilience.experiment",
    )
}

/// The mean time the runs took to come back.
fn mttr() -> MetricDef {
    metric(
        "mttr",
        Unit::Seconds,
        Measure::Avg {
            column: "recovery_time_s".into(),
        },
        "resilience.experiment",
    )
}

/// The 95th percentile of the time the actions took.
fn action_p95() -> MetricDef {
    metric(
        "action_duration_p95",
        Unit::Nanoseconds,
        Measure::Quantile {
            column: "duration_ns".into(),
            quantile: 0.95,
        },
        "resilience.action",
    )
}

/// The share of the runs that came back at once.
fn instant_share() -> MetricDef {
    let term = |condition| tumult_metrics::Term {
        column: "*".into(),
        condition,
        conditions: Vec::new(),
    };
    metric(
        "instant_share",
        Unit::Ratio,
        Measure::Rate {
            num: term(Some(tumult_metrics::Condition {
                column: "recovery_time_s".into(),
                equals: tumult_metrics::Literal::Num(0.0),
            })),
            den: term(None),
        },
        "resilience.experiment",
    )
}

fn digest_of(spans: &[SpanRow], def: &MetricDef, period: Option<(i64, i64)>) -> Report {
    let (_dir, store) = store_of(spans);
    build_report(&reader_of(&store), std::slice::from_ref(def), "t", period).expect("the digest")
}

/// The headline number of the digest: its value, its change, its trend.
fn kpi(report: &Report) -> (String, Option<String>, Option<Trend>) {
    match &report.sections[0] {
        Section::Kpi {
            value,
            delta,
            trend,
            ..
        } => (value.clone(), delta.clone(), trend.clone()),
        other => panic!("the first section is not a number: {other:?}"),
    }
}

fn change(report: &Report) -> Option<String> {
    kpi(report).1
}

fn points(report: &Report) -> Vec<(i64, f64)> {
    kpi(report).2.expect("a trend").points
}

#[test]
fn more_runs_than_in_the_week_before_is_a_change_with_a_plus() {
    let mut spans = runs("a", 1, day(-3));
    spans.extend(runs("b", 3, day(2)));

    let report = digest_of(&spans, &run_count(), Some(THIS_WEEK));

    assert_eq!(kpi(&report).0, "3");
    assert_eq!(change(&report).as_deref(), Some("+2 vs the period before"));
}

#[test]
fn fewer_runs_than_in_the_week_before_is_a_change_with_a_minus() {
    let mut spans = runs("a", 3, day(-3));
    spans.extend(runs("b", 1, day(2)));

    let report = digest_of(&spans, &run_count(), Some(THIS_WEEK));

    assert_eq!(change(&report).as_deref(), Some("-2 vs the period before"));
}

#[test]
fn as_many_runs_as_in_the_week_before_is_no_change() {
    let mut spans = runs("a", 2, day(-3));
    spans.extend(runs("b", 2, day(2)));

    let report = digest_of(&spans, &run_count(), Some(THIS_WEEK));

    assert_eq!(
        change(&report).as_deref(),
        Some("same as the period before")
    );
}

#[test]
fn the_period_before_is_as_long_as_the_period_and_ends_where_it_starts() {
    // One run just inside the week before, one just before it, one on the
    // first instant of this week.
    let mut spans = runs("a", 1, day(-7));
    spans.extend(runs("b", 1, day(-7) - 1));
    spans.extend(runs("c", 1, day(0)));

    let report = digest_of(&spans, &run_count(), Some(THIS_WEEK));

    // This week has the run of day 0. The week before has one run: the one
    // an instant before it is too old.
    assert_eq!(kpi(&report).0, "1");
    assert_eq!(
        change(&report).as_deref(),
        Some("same as the period before")
    );
}

#[test]
fn a_mean_with_nothing_in_the_period_before_does_not_claim_a_change() {
    let spans = vec![recovered("a", day(2), 4.0)];

    let report = digest_of(&spans, &mttr(), Some(THIS_WEEK));

    assert_eq!(kpi(&report).0, "4.0 s");
    assert_eq!(
        change(&report).as_deref(),
        Some("no data in the period before")
    );
}

#[test]
fn a_number_without_a_value_has_no_change() {
    let spans = vec![recovered("a", day(-3), 4.0)];

    let report = digest_of(&spans, &mttr(), Some(THIS_WEEK));

    assert_eq!(kpi(&report).0, "—");
    assert_eq!(change(&report), None);
}

#[test]
fn the_change_of_a_duration_is_a_duration() {
    let spans = vec![recovered("a", day(-3), 1.5), recovered("b", day(2), 4.0)];

    let report = digest_of(&spans, &mttr(), Some(THIS_WEEK));

    assert_eq!(
        change(&report).as_deref(),
        Some("+2.5 s vs the period before")
    );
}

#[test]
fn a_ratio_is_a_percentage_and_its_change_is_in_points() {
    let spans = vec![
        // The week before: 1 of 4 runs came back at once.
        recovered("a", day(-3), 0.0),
        recovered("b", day(-3), 2.0),
        recovered("c", day(-3), 2.0),
        recovered("d", day(-3), 2.0),
        // This week: 1 of 2.
        recovered("e", day(2), 0.0),
        recovered("f", day(2), 2.0),
    ];

    let report = digest_of(&spans, &instant_share(), Some(THIS_WEEK));

    assert_eq!(kpi(&report).0, "50.0%");
    assert_eq!(
        change(&report).as_deref(),
        Some("+25.0 points vs the period before")
    );
}

#[test]
fn a_digest_over_all_data_has_no_period_and_no_change() {
    let report = digest_of(&runs("a", 2, day(2)), &run_count(), None);

    assert_eq!(report.period, None);
    assert_eq!(kpi(&report).0, "2");
    assert_eq!(change(&report), None);
}

#[test]
fn a_digest_knows_the_period_it_is_about() {
    let report = digest_of(&runs("a", 2, day(2)), &run_count(), Some(THIS_WEEK));

    assert_eq!(report.period, Some(THIS_WEEK));
}

#[test]
fn the_trend_of_a_count_over_a_week_has_a_point_for_every_day() {
    let mut spans = runs("a", 2, day(1) + HOUR);
    spans.extend(runs("b", 1, day(4) + 23 * HOUR));

    let report = digest_of(&spans, &run_count(), Some(THIS_WEEK));

    let trend = kpi(&report).2.expect("a trend");
    assert_eq!(trend.per, "day");
    assert_eq!(
        trend.points,
        [
            (day(0), 0.0),
            (day(1), 2.0),
            (day(2), 0.0),
            (day(3), 0.0),
            (day(4), 1.0),
            (day(5), 0.0),
            (day(6), 0.0),
        ]
    );
}

#[test]
fn the_trend_of_a_mean_has_a_point_only_where_runs_are() {
    let spans = vec![
        recovered("a", day(1), 2.0),
        recovered("b", day(1) + HOUR, 4.0),
        recovered("c", day(5), 9.0),
    ];

    let report = digest_of(&spans, &mttr(), Some(THIS_WEEK));

    // A day without a run has no mean; it is not a mean of zero.
    assert_eq!(points(&report), [(day(1), 3.0), (day(5), 9.0)]);
}

#[test]
fn the_trend_of_a_period_has_no_point_from_before_or_after_it() {
    let spans = vec![
        recovered("before", day(-1), 50.0),
        recovered("a", day(1), 2.0),
        recovered("b", day(5), 9.0),
        recovered("after", day(7) + HOUR, 70.0),
    ];

    let report = digest_of(&spans, &mttr(), Some(THIS_WEEK));

    assert_eq!(points(&report), [(day(1), 2.0), (day(5), 9.0)]);
}

#[test]
fn the_groups_of_a_period_count_its_runs_only() {
    let mut spans = runs("checkout", 2, day(1));
    spans.extend(runs("checkout", 3, day(-2)));
    spans.extend(runs("search", 1, day(8)));
    let mut def = run_count();
    def.dimensions = vec!["experiment_name".into()];

    let report = digest_of(&spans, &def, Some(THIS_WEEK));

    assert_eq!(
        report.sections[2],
        Section::Bars {
            title: "experiment_count by experiment_name".into(),
            bars: vec![("checkout".into(), 2.0)],
        }
    );
}

#[test]
fn a_group_of_two_dimensions_is_named_by_both() {
    let mut def = run_count();
    def.dimensions = vec!["experiment_name".into(), "target_environment".into()];

    let report = digest_of(&runs("checkout", 2, day(1)), &def, Some(THIS_WEEK));

    assert_eq!(
        report.sections[2],
        Section::Bars {
            title: "experiment_count by experiment_name, target_environment".into(),
            bars: vec![("checkout / dev".into(), 2.0)],
        }
    );
}

#[test]
fn the_trend_of_an_hour_is_by_five_minutes() {
    let hour = (END - HOUR, END);
    let spans = runs("a", 3, END - 20 * MINUTE);

    let report = digest_of(&spans, &run_count(), Some(hour));

    let trend = kpi(&report).2.expect("a trend");
    assert_eq!(trend.per, "5 minutes");
    assert_eq!(trend.points.len(), 12);
    assert_eq!(trend.points[0], (END - HOUR, 0.0));
    // The three runs are a minute apart, from 20 minutes before the end.
    assert_eq!(trend.points[8], (END - 20 * MINUTE, 3.0));
    assert_eq!(trend.points[11], (END - 5 * MINUTE, 0.0));
}

#[test]
fn the_trend_over_all_data_is_by_day_from_the_first_to_the_last_run() {
    let mut spans = runs("a", 1, day(1) + HOUR);
    spans.extend(runs("b", 2, day(4)));

    let report = digest_of(&spans, &run_count(), None);

    let trend = kpi(&report).2.expect("a trend");
    assert_eq!(trend.per, "day");
    assert_eq!(
        trend.points,
        [(day(1), 1.0), (day(2), 0.0), (day(3), 0.0), (day(4), 2.0)]
    );
}

#[test]
fn a_value_in_one_stretch_only_is_no_trend() {
    let spans = vec![
        recovered("a", day(2), 4.0),
        recovered("b", day(2) + HOUR, 6.0),
    ];

    let report = digest_of(&spans, &mttr(), Some(THIS_WEEK));

    assert_eq!(kpi(&report).0, "5.0 s");
    assert_eq!(kpi(&report).2, None);
}

#[test]
fn a_number_without_rows_has_no_trend() {
    let report = digest_of(&runs("a", 1, day(1)), &mttr(), Some(THIS_WEEK));

    assert_eq!(kpi(&report).2, None);
}

#[test]
fn a_p95_in_nanoseconds_is_shown_in_a_unit_a_reader_knows() {
    // 21 actions that took 0, 10, 20 ... 200 ms: the p95 is 190 ms.
    let spans: Vec<SpanRow> = (0..21)
        .map(|number| action("process", number, day(2) + number * SECOND, number * 10))
        .collect();

    let report = digest_of(&spans, &action_p95(), Some(THIS_WEEK));

    assert_eq!(kpi(&report).0, "190.0 ms");
}

#[test]
fn a_metric_with_a_dimension_has_a_table_and_bars_of_its_groups() {
    let mut spans = runs("checkout", 2, day(1));
    spans.extend(runs("search", 1, day(2)));
    let mut def = run_count();
    def.dimensions = vec!["experiment_name".into()];

    let report = digest_of(&spans, &def, Some(THIS_WEEK));

    // The headline is about all runs, the groups follow it.
    assert_eq!(kpi(&report).0, "3");
    assert_eq!(
        report.sections[1],
        Section::Table {
            headers: vec!["experiment_name".into(), "value".into()],
            rows: vec![
                vec!["checkout".into(), "2".into()],
                vec!["search".into(), "1".into()],
            ],
        }
    );
    assert_eq!(
        report.sections[2],
        Section::Bars {
            title: "experiment_count by experiment_name".into(),
            bars: vec![("checkout".into(), 2.0), ("search".into(), 1.0)],
        }
    );
}

#[test]
fn the_bars_of_a_duration_say_their_unit_and_are_in_it() {
    let spans = vec![
        action("process", 0, day(1), 40),
        action("kafka", 1, day(1), 250),
    ];
    let mut def = action_p95();
    def.dimensions = vec!["plugin_name".into()];

    let report = digest_of(&spans, &def, Some(THIS_WEEK));

    assert_eq!(
        report.sections[1],
        Section::Table {
            headers: vec!["plugin_name".into(), "value".into()],
            rows: vec![
                vec!["kafka".into(), "250.0 ms".into()],
                vec!["process".into(), "40.0 ms".into()],
            ],
        }
    );
    assert_eq!(
        report.sections[2],
        Section::Bars {
            title: "action_duration_p95 by plugin_name (milliseconds)".into(),
            bars: vec![("kafka".into(), 250.0), ("process".into(), 40.0)],
        }
    );
}

#[test]
fn the_bars_of_a_duration_are_in_the_unit_that_fits_the_largest_one() {
    let spans = vec![
        recovered("fast", day(1), 0.25),
        recovered("slow", day(2), 0.5),
    ];
    let mut def = mttr();
    def.dimensions = vec!["experiment_name".into()];

    let report = digest_of(&spans, &def, Some(THIS_WEEK));

    assert_eq!(
        report.sections[2],
        Section::Bars {
            title: "mttr by experiment_name (milliseconds)".into(),
            bars: vec![("fast".into(), 250.0), ("slow".into(), 500.0)],
        }
    );
}

#[test]
fn a_group_without_a_name_is_shown_as_a_dash() {
    let mut unnamed = action("process", 0, day(1), 40);
    unnamed.plugin_name = None;
    let mut def = action_p95();
    def.dimensions = vec!["plugin_name".into()];

    let report = digest_of(&[unnamed], &def, Some(THIS_WEEK));

    assert_eq!(
        report.sections[2],
        Section::Bars {
            title: "action_duration_p95 by plugin_name (milliseconds)".into(),
            bars: vec![("—".into(), 40.0)],
        }
    );
}

#[test]
fn the_number_says_what_it_measures() {
    let report = digest_of(&runs("a", 1, day(1)), &run_count(), Some(THIS_WEEK));

    match &report.sections[0] {
        Section::Kpi { label, about, .. } => {
            assert_eq!(label, "experiment_count");
            assert_eq!(about.as_deref(), Some("What experiment_count measures."));
        }
        other => panic!("not a number: {other:?}"),
    }
}

#[test]
fn a_scoped_digest_counts_changes_and_trends_in_its_environments_only() {
    let mut spans = runs("dev-run", 1, day(-3));
    spans.extend(runs("dev-run-now", 2, day(2)));
    let mut elsewhere = runs("prod-run", 4, day(2));
    elsewhere.extend(runs("prod-run-before", 5, day(-2)));
    for run in &mut elsewhere {
        run.target_environment = Some("prod".into());
    }
    spans.extend(elsewhere);
    let (_dir, store) = store_of(&spans);

    let report = build_report_scoped(
        &reader_of(&store),
        &[run_count()],
        "t",
        Some(THIS_WEEK),
        &["dev".to_string()],
    )
    .expect("the digest");

    assert_eq!(kpi(&report).0, "2");
    assert_eq!(change(&report).as_deref(), Some("+1 vs the period before"));
    assert_eq!(points(&report)[2], (day(2), 2.0));
}

#[test]
fn the_page_says_its_period_and_when_it_was_made_in_words() {
    let mut report = digest_of(&runs("a", 2, day(2)), &run_count(), Some(THIS_WEEK));
    report.generated_at_ns = END + 90 * MINUTE;

    let html = render_html(&report);

    assert!(
        html.contains("2026-11-11 00:00 UTC to 2026-11-18 00:00 UTC"),
        "{html}"
    );
    assert!(html.contains("made 2026-11-18 01:30 UTC"), "{html}");
    assert!(!html.contains("epoch ns"), "{html}");
}

#[test]
fn the_page_of_a_digest_over_all_data_says_so() {
    let mut report = digest_of(&runs("a", 2, day(2)), &run_count(), None);
    report.generated_at_ns = END;

    let html = render_html(&report);

    assert!(
        html.contains("All data · made 2026-11-18 00:00 UTC"),
        "{html}"
    );
}

#[test]
fn the_page_draws_the_trend_the_change_and_the_bars() {
    let mut spans = runs("checkout", 1, day(-3));
    spans.extend(runs("checkout", 2, day(1)));
    spans.extend(runs("search", 1, day(4)));
    let mut def = run_count();
    def.dimensions = vec!["experiment_name".into()];
    let report = digest_of(&spans, &def, Some(THIS_WEEK));

    let html = render_html(&report);

    assert!(html.contains("+2 vs the period before"), "{html}");
    assert!(html.contains("What experiment_count measures."), "{html}");
    // The trend: a line of seven points, and what one point is.
    assert_eq!(html.matches("<polyline").count(), 1, "{html}");
    assert!(html.contains("per day"), "{html}");
    // The bars: one for each experiment, under their title.
    assert!(
        html.contains("experiment_count by experiment_name"),
        "{html}"
    );
    assert_eq!(html.matches("<rect").count(), 2, "{html}");
    // Nothing is left of the grey boxes that stood where a chart should be.
    assert!(!html.contains("repeating-linear-gradient"), "{html}");
}

#[test]
fn a_number_without_a_trend_has_no_empty_chart() {
    let report = digest_of(&runs("a", 1, day(1)), &mttr(), Some(THIS_WEEK));

    let html = render_html(&report);

    assert!(!html.contains("<svg"), "{html}");
    assert!(!html.contains("class=\"sparkline\""), "{html}");
}

#[test]
fn the_title_of_a_digest_says_its_stretch_of_time_in_words() {
    let title = |seconds| digest_title(Duration::from_secs(seconds));

    assert_eq!(title(7 * 86_400), "Tumult digest — last 7 days");
    assert_eq!(title(86_400), "Tumult digest — last day");
    assert_eq!(title(36 * 3_600), "Tumult digest — last 36 hours");
    assert_eq!(title(3_600), "Tumult digest — last hour");
    assert_eq!(title(30 * 60), "Tumult digest — last 30 minutes");
    assert_eq!(title(60), "Tumult digest — last minute");
    assert_eq!(title(90), "Tumult digest — last 90 seconds");
    assert_eq!(title(1), "Tumult digest — last second");
}

#[test]
fn a_trend_has_at_most_a_point_for_each_day_of_a_month() {
    let per = |length: i64| {
        let report = digest_of(
            &runs("a", 1, day(1)),
            &run_count(),
            Some((END - length, END)),
        );
        kpi(&report).2.expect("a trend").per
    };

    assert_eq!(per(31 * MINUTE), "minute");
    assert_eq!(per(31 * MINUTE + 1), "5 minutes");
    assert_eq!(per(31 * 5 * MINUTE + 1), "hour");
    assert_eq!(per(31 * HOUR), "hour");
    assert_eq!(per(31 * HOUR + 1), "day");
    assert_eq!(per(31 * DAY), "day");
    assert_eq!(per(31 * DAY + 1), "week");
    assert_eq!(per(31 * 7 * DAY + 1), "30 days");
    // Nothing is wider: ten years are still by 30 days.
    assert_eq!(per(3_650 * DAY), "30 days");
}

#[test]
fn the_trend_of_a_period_that_starts_inside_a_day_covers_each_day_it_touches() {
    let period = (day(0) + HOUR, day(2) + HOUR);
    let spans = runs("a", 1, day(1) + 2 * HOUR);

    let report = digest_of(&spans, &run_count(), Some(period));

    assert_eq!(
        points(&report),
        [(day(0), 0.0), (day(1), 1.0), (day(2), 0.0)]
    );
}

#[test]
fn a_metric_with_a_dimension_and_no_rows_has_no_bars() {
    let mut def = run_count();
    def.dimensions = vec!["experiment_name".into()];

    let report = digest_of(&[], &def, Some(THIS_WEEK));

    assert_eq!(report.sections.len(), 2, "{:?}", report.sections);
    assert!(matches!(report.sections[1], Section::Table { .. }));
}

#[test]
fn a_change_too_small_to_show_is_no_change() {
    let spans = vec![
        recovered("a", day(-3), 4.0),
        recovered("b", day(2), 4.000_000_000_01),
    ];

    let report = digest_of(&spans, &mttr(), Some(THIS_WEEK));

    assert_eq!(
        change(&report).as_deref(),
        Some("same as the period before")
    );
}

#[test]
fn a_shorter_duration_than_before_is_a_change_with_a_minus() {
    let spans = vec![recovered("a", day(-3), 90.0), recovered("b", day(2), 30.0)];

    let report = digest_of(&spans, &mttr(), Some(THIS_WEEK));

    assert_eq!(kpi(&report).0, "30.0 s");
    assert_eq!(
        change(&report).as_deref(),
        Some("-1.0 min vs the period before")
    );
}
