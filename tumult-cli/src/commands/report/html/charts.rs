//! The charts of the report: the timeline of the run, its load and its
//! probe samples over time, and the headroom of its objectives.
//!
//! A chart is an element to draw in and, next to it, its `ECharts` option
//! as JSON. The script of the report draws each option. A colour of the
//! theme is named in an option (`token:ok`), and the script gives it the
//! colour the browser asks for, light or dark.

use std::time::Duration;

use serde_json::{json, Value};
use tumult_core::types::{
    ActivityResult, ActivityStatus, HypothesisResult, Journal, LoadResult, LoadSample, NfrState,
    NfrVerdict, ProbeSample,
};

use super::fmt::{millis, number, offset, seconds};
use super::measures::must_be;

/// How the option of a chart begins in a report. No text of a journal can
/// write this: the report escapes every `<` of one.
pub(super) const OPTION: &str = r#"<script type="application/json" data-chart=""#;

const OK: &str = "token:ok";
const FAIL: &str = "token:fail";
const MUTED: &str = "token:axis";
const ACCENT: &str = "token:accent";

const LINE_CHART_PX: usize = 260;

/// The room the timeline keeps above its bars for the name of the fault
/// window.
const FAULT_NAME_PX: usize = 16;

/// The height of a chart of `rows` bars, one under the other.
fn bars_px(rows: usize) -> usize {
    70 + 26 * rows
}

/// The element the chart `id` is drawn in, and its option next to it.
/// `label` says what the chart shows to who cannot see it.
fn figure(id: &str, label: &str, height_px: usize, option: &Value) -> String {
    // A `<` in a text of the journal must not end the element.
    let option = option.to_string().replace('<', "\\u003c");
    format!(
        "<figure>\n\
         <div class=\"chart\" id=\"{id}\" data-height=\"{height_px}\" role=\"img\" \
         aria-label=\"{label}\"></div>\n\
         {OPTION}{id}\">{option}</script>\n\
         </figure>\n"
    )
}

fn rounded(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// The time axis of the charts of a run: from the start of the run, in
/// the unit that fits its length.
struct TimeAxis {
    start_ns: i64,
    unit: &'static str,
    unit_s: f64,
}

impl TimeAxis {
    fn of(journal: &Journal) -> Self {
        const TEN_MINUTES_NS: i64 = 600_000_000_000;
        const TEN_HOURS_NS: i64 = 60 * TEN_MINUTES_NS;
        let (unit, unit_s) = match journal.ended_at_ns.saturating_sub(journal.started_at_ns) {
            ..TEN_MINUTES_NS => ("s", 1.0),
            TEN_MINUTES_NS..TEN_HOURS_NS => ("min", 60.0),
            _ => ("h", 3600.0),
        };
        Self {
            start_ns: journal.started_at_ns,
            unit,
            unit_s,
        }
    }

    fn length(&self, length: Duration) -> f64 {
        rounded(length.as_secs_f64() / self.unit_s)
    }

    /// Where the time `ns` is on the axis. A time before the run is at its
    /// start.
    fn at(&self, ns: i64) -> f64 {
        self.length(between(self.start_ns, ns))
    }

    fn option(&self) -> Value {
        json!({
            "type": "value",
            "min": 0,
            "name": format!("Time from the start of the run ({})", self.unit),
            "nameLocation": "middle",
            "nameGap": 28,
        })
    }
}

/// The time from `start_ns` to `end_ns`; no time when the end is not later.
fn between(start_ns: i64, end_ns: i64) -> Duration {
    Duration::from_nanos(u64::try_from(end_ns.saturating_sub(start_ns)).unwrap_or(0))
}

/// The time the fault was active, as the area a series marks.
fn fault_area(journal: &Journal, axis: &TimeAxis) -> Option<Value> {
    let during = journal.during_result.as_ref()?;
    Some(json!({
        "silent": true,
        "itemStyle": {"color": FAIL, "opacity": 0.1},
        "label": {"color": FAIL},
        "data": [[
            {"name": "fault active", "xAxis": axis.at(during.started_at_ns)},
            {"xAxis": axis.at(during.ended_at_ns)},
        ]],
    }))
}

/// `series` with the area of the fault, when the run had one.
fn marked(mut series: Value, fault: Option<Value>) -> Value {
    if let Some(area) = fault {
        series["markArea"] = area;
    }
    series
}

/// A row of the timeline: what ran, when, and for how long.
struct Bar {
    label: String,
    said: String,
    starts: f64,
    lasts: f64,
    colour: &'static str,
}

/// When each step of the run began and how long it took, with the checks
/// of the steady state and the load. Nothing for a run without a step.
pub(super) fn timeline(journal: &Journal) -> String {
    if journal.method_results.is_empty() && journal.rollback_results.is_empty() {
        return String::new();
    }
    let axis = TimeAxis::of(journal);
    let bars = bars(journal, &axis);
    let labels: Vec<&str> = bars.iter().map(|bar| bar.label.as_str()).collect();
    let starts: Vec<f64> = bars.iter().map(|bar| bar.starts).collect();
    let shown: Vec<Value> = bars.iter().map(shown).collect();
    let run = json!({"type": "bar", "stack": "run", "barMinHeight": 2, "data": shown});
    let option = json!({
        "animation": false,
        "grid": {"left": 12, "right": 32, "top": 12 + FAULT_NAME_PX, "bottom": 12},
        "tooltip": {"trigger": "item", "formatter": "{b}"},
        "xAxis": axis.option(),
        "yAxis": names(&labels),
        "series": [
            // A bar no one sees moves the bar of a step to where it began.
            {
                "type": "bar",
                "stack": "run",
                "silent": true,
                "itemStyle": {"color": "transparent"},
                "data": starts,
            },
            marked(run, fault_area(journal, &axis)),
        ],
    });
    figure(
        "chart-timeline",
        "Timeline of the run: when each step began and how long it took",
        bars_px(bars.len()) + FAULT_NAME_PX,
        &option,
    )
}

/// An axis of names, read from the top.
fn names(labels: &[&str]) -> Value {
    json!({
        "type": "category",
        "inverse": true,
        "data": labels,
        "axisLabel": {"width": 220, "overflow": "truncate"},
    })
}

fn shown(bar: &Bar) -> Value {
    json!({"value": bar.lasts, "name": bar.said, "itemStyle": {"color": bar.colour}})
}

fn bars(journal: &Journal, axis: &TimeAxis) -> Vec<Bar> {
    let phases = [
        (" (before)", probes_of(journal.steady_state_before.as_ref())),
        ("", journal.method_results.as_slice()),
        (" (after)", probes_of(journal.steady_state_after.as_ref())),
        (" (rollback)", journal.rollback_results.as_slice()),
    ];
    let load = journal
        .load_result
        .as_ref()
        .map(|load| load_bar(load, axis));
    phases
        .iter()
        .flat_map(|(phase, steps)| steps.iter().map(move |step| step_bar(step, phase, axis)))
        .chain(load)
        .collect()
}

fn probes_of(check: Option<&HypothesisResult>) -> &[ActivityResult] {
    check.map_or(&[], |check| check.probe_results.as_slice())
}

fn step_bar(step: &ActivityResult, phase: &str, axis: &TimeAxis) -> Bar {
    let label = format!("{}{phase}", step.name);
    let colour = match step.status {
        ActivityStatus::Succeeded => OK,
        ActivityStatus::Failed | ActivityStatus::Timeout => FAIL,
        ActivityStatus::Skipped => MUTED,
    };
    Bar {
        said: format!(
            "{label}: {}, began at {}, took {}",
            step.status,
            offset(step.started_at_ns, axis.start_ns),
            millis(step.duration_ms)
        ),
        starts: axis.at(step.started_at_ns),
        lasts: axis.length(Duration::from_millis(step.duration_ms)),
        colour,
        label,
    }
}

fn load_bar(load: &LoadResult, axis: &TimeAxis) -> Bar {
    let label = format!("load ({})", load.tool);
    let lasted = between(load.started_at_ns, load.ended_at_ns);
    Bar {
        said: format!(
            "{label}: began at {}, took {}",
            offset(load.started_at_ns, axis.start_ns),
            seconds(lasted.as_secs_f64())
        ),
        starts: axis.at(load.started_at_ns),
        lasts: axis.length(lasted),
        colour: ACCENT,
        label,
    }
}

/// A chart of lines over the time of the run.
fn lines(y_axis: &Value, series: &[Value], axis: &TimeAxis) -> Value {
    json!({
        "animation": false,
        "grid": {"left": 12, "right": 32, "top": 40, "bottom": 12},
        "legend": {"top": 0},
        "tooltip": {"trigger": "axis"},
        "xAxis": axis.option(),
        "yAxis": y_axis,
        "series": series,
    })
}

fn line(name: &str, data: &[Value]) -> Value {
    json!({"type": "line", "name": name, "data": data})
}

/// A line of few points, each of which is shown.
fn dotted(name: &str, data: &[Value]) -> Value {
    json!({"type": "line", "name": name, "symbol": "circle", "symbolSize": 5, "data": data})
}

/// The latency of the load in each of its windows, and its throughput and
/// error rate. Nothing for a load of fewer than two windows: its totals
/// say all there is.
pub(super) fn load(journal: &Journal, load: &LoadResult) -> String {
    if load.samples.len() < 2 {
        return String::new();
    }
    let axis = TimeAxis::of(journal);
    let over_time = |value: fn(&LoadSample) -> f64| -> Vec<Value> {
        let point = |window: &LoadSample| json!([axis.at(window.ts_ns), value(window)]);
        load.samples.iter().map(point).collect()
    };
    let latency = [
        marked(
            dotted("p50", &over_time(|window| window.latency_p50_ms)),
            fault_area(journal, &axis),
        ),
        dotted("p95", &over_time(|window| window.latency_p95_ms)),
        dotted("p99", &over_time(|window| window.latency_p99_ms)),
    ];
    let mut errors = dotted(
        "error rate",
        &over_time(|window| rounded(window.error_rate * 100.0)),
    );
    errors["yAxisIndex"] = json!(1);
    errors["itemStyle"] = json!({"color": FAIL});
    errors["lineStyle"] = json!({"color": FAIL});
    let throughput = [
        marked(
            dotted("throughput", &over_time(|window| window.throughput_rps)),
            fault_area(journal, &axis),
        ),
        errors,
    ];
    let rates = json!([
        {"type": "value", "name": "Requests/s"},
        {"type": "value", "name": "Errors (%)", "min": 0, "splitLine": {"show": false}},
    ]);
    [
        figure(
            "chart-load-latency",
            "Latency of the load over time: p50, p95 and p99 of each window",
            LINE_CHART_PX,
            &lines(
                &json!({"type": "value", "name": "Latency (ms)"}),
                &latency,
                &axis,
            ),
        ),
        figure(
            "chart-load-throughput",
            "Throughput and error rate of the load over time",
            LINE_CHART_PX,
            &lines(&rates, &throughput, &axis),
        ),
    ]
    .concat()
}

/// How long each sample of each probe took, over the time of the run.
/// Nothing for a run without samples.
pub(super) fn probes(journal: &Journal) -> String {
    let samples = &journal.probe_samples;
    if samples.is_empty() {
        return String::new();
    }
    let axis = TimeAxis::of(journal);
    let mut probes: Vec<&str> = Vec::new();
    for sample in samples {
        if !probes.contains(&sample.probe.as_str()) {
            probes.push(&sample.probe);
        }
    }
    let mut fault = fault_area(journal, &axis);
    let series: Vec<Value> = probes
        .iter()
        .map(|probe| {
            let points: Vec<Value> = samples
                .iter()
                .filter(|sample| sample.probe == *probe)
                .map(|sample| point(sample, &axis))
                .collect();
            marked(line(probe, &points), fault.take())
        })
        .collect();
    figure(
        "chart-probes",
        "Time of each probe sample during and after the fault",
        LINE_CHART_PX,
        &lines(
            &json!({"type": "value", "name": "Probe time (ms)"}),
            &series,
            &axis,
        ),
    )
}

/// A sample as a point. One out of its tolerance is larger, and in the
/// colour of a failure.
fn point(sample: &ProbeSample, axis: &TimeAxis) -> Value {
    let at = json!([axis.at(sample.ts_ns), sample.duration_ms]);
    if sample.ok {
        return at;
    }
    json!({"value": at, "symbol": "circle", "symbolSize": 9, "itemStyle": {"color": FAIL}})
}

/// The margin of each objective to its threshold, in percent of the
/// threshold. Nothing when no objective has one.
pub(super) fn headroom(verdicts: &[NfrVerdict]) -> String {
    let bars: Vec<(&str, Value)> = verdicts
        .iter()
        .filter_map(|verdict| Some((verdict.nfr_id.as_str(), headroom_bar(verdict)?)))
        .collect();
    if bars.is_empty() {
        return String::new();
    }
    let labels: Vec<&str> = bars.iter().map(|(id, _)| *id).collect();
    let shown: Vec<&Value> = bars.iter().map(|(_, bar)| bar).collect();
    let option = json!({
        "animation": false,
        "grid": {"left": 12, "right": 32, "top": 12, "bottom": 12},
        "tooltip": {"trigger": "item", "formatter": "{b}"},
        "xAxis": {
            "type": "value",
            "name": "Headroom (% of the threshold)",
            "nameLocation": "middle",
            "nameGap": 28,
        },
        "yAxis": names(&labels),
        "series": [{"type": "bar", "data": shown}],
    });
    figure(
        "chart-headroom",
        "Headroom of each objective, in percent of its threshold",
        bars_px(bars.len()),
        &option,
    )
}

/// The bar of a verdict. None for a verdict that is not sure, has no
/// margin, or has a threshold of zero to measure the margin against.
fn headroom_bar(verdict: &NfrVerdict) -> Option<Value> {
    if verdict.state == NfrState::Inconclusive {
        return None;
    }
    let share = verdict.margin? * 100.0 / verdict.threshold.abs();
    let percent = Some(rounded(share)).filter(|percent| percent.is_finite())?;
    let colour = if verdict.state == NfrState::Met {
        OK
    } else {
        FAIL
    };
    let said = format!(
        "{}: {}, {} against {} {}",
        verdict.nfr_id,
        verdict.state,
        number(verdict.observed?),
        must_be(verdict.operator),
        number(verdict.threshold)
    );
    Some(json!({"value": percent, "name": said, "itemStyle": {"color": colour}}))
}
