//! The charts of the HTML report `tumult report` writes: the option each
//! chart gives the script of the report, and the script itself.
//!
//! Each journal is written to a file as a run writes it, and the binary
//! reads it from there. That the script draws the options in a browser is
//! shown by the Playwright test in `web/report-tests`.
#![cfg(unix)]

mod lab;
mod report_page;

use report_page::journals::{
    during, journal, load, report_for, step, verdict, window, MILLISECOND, STARTED,
};
use report_page::{chart, chart_host, chart_ids, chart_script, has_chart, section};
use serde_json::{json, Value};
use tumult_core::types::{
    ActivityResult, ActivityStatus, HypothesisResult, Journal, LoadExecution, NfrOperator,
    NfrState, NfrVerdict, ObjectiveOutcome, ProbeSample, SamplePhase,
};

const SECOND: i64 = 1000 * MILLISECOND;

/// A steady state that was checked with `probes`.
fn checked(probes: Vec<ActivityResult>) -> HypothesisResult {
    HypothesisResult {
        title: "The target answers".into(),
        met: true,
        probe_results: probes,
    }
}

/// What `option` holds at `pointer`, a JSON pointer.
fn at<'a>(option: &'a Value, pointer: &str) -> &'a Value {
    option
        .pointer(pointer)
        .unwrap_or_else(|| panic!("no {pointer} in: {option}"))
}

/// What each item of the list at `pointer` holds at `inside`.
fn each(option: &Value, pointer: &str, inside: &str) -> Value {
    let items = at(option, pointer).as_array().expect("a list");
    items.iter().map(|item| at(item, inside).clone()).collect()
}

/// A run of four steps that end in every way a step can, between two
/// checks of the steady state and before a rollback.
fn run_of_every_step() -> Journal {
    Journal {
        steady_state_before: Some(checked(vec![step(
            "api",
            ActivityStatus::Succeeded,
            100,
            50,
        )])),
        method_results: vec![
            step("break", ActivityStatus::Succeeded, 2000, 500),
            step("wait", ActivityStatus::Failed, 2500, 1500),
            step("ask", ActivityStatus::Failed, 4000, 30_000),
            step("tidy", ActivityStatus::Skipped, 34_000, 0),
        ],
        steady_state_after: Some(checked(vec![step(
            "api",
            ActivityStatus::Failed,
            35_000,
            75,
        )])),
        rollback_results: vec![step("undo", ActivityStatus::Succeeded, 35_500, 250)],
        ..journal()
    }
}

#[test]
fn a_run_that_did_nothing_has_no_chart_and_no_script() {
    let html = report_for(&journal());

    assert_eq!(chart_ids(&html), Vec::<String>::new());
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains(r#"class="chart""#), "{html}");
}

#[test]
fn the_timeline_has_a_row_for_every_probe_step_and_rollback() {
    let html = report_for(&run_of_every_step());

    let timeline = chart(&html, "chart-timeline");
    assert_eq!(
        timeline["yAxis"]["data"],
        json!([
            "api (before)",
            "break",
            "wait",
            "ask",
            "tidy",
            "api (after)",
            "undo (rollback)"
        ])
    );
    // The rows are read from the top, as the table below them.
    assert_eq!(timeline["yAxis"]["inverse"], json!(true));
    // A bar begins where its step began: after a bar no one sees.
    assert_eq!(
        timeline["series"][0]["data"],
        json!([0.1, 2.0, 2.5, 4.0, 34.0, 35.0, 35.5])
    );
    assert_eq!(
        timeline["series"][0]["itemStyle"]["color"],
        json!("transparent")
    );
    assert_eq!(timeline["series"][0]["silent"], json!(true));
    assert_eq!(
        timeline["series"][0]["stack"],
        timeline["series"][1]["stack"]
    );
    assert_eq!(
        each(&timeline, "/series/1/data", "/value"),
        json!([0.05, 0.5, 1.5, 30.0, 0.0, 0.075, 0.25])
    );
    assert_eq!(
        timeline["xAxis"]["name"],
        json!("Time from the start of the run (s)")
    );
    assert_eq!(timeline["xAxis"]["min"], json!(0));
}

#[test]
fn a_bar_of_the_timeline_has_the_colour_of_how_its_step_ended() {
    let html = report_for(&run_of_every_step());

    let timeline = chart(&html, "chart-timeline");
    assert_eq!(
        each(&timeline, "/series/1/data", "/itemStyle/color"),
        json!([
            "token:ok",
            "token:ok",
            "token:fail",
            "token:fail",
            "token:axis",
            "token:fail",
            "token:ok"
        ])
    );
}

#[test]
fn a_bar_of_the_timeline_says_in_words_what_it_shows() {
    let html = report_for(&run_of_every_step());

    let timeline = chart(&html, "chart-timeline");
    assert_eq!(
        each(&timeline, "/series/1/data", "/name"),
        json!([
            "api (before): succeeded, began at +100 ms, took 50 ms",
            "break: succeeded, began at +2.0 s, took 500 ms",
            "wait: failed, began at +2.5 s, took 1.5 s",
            "ask: failed, began at +4.0 s, took 30.0 s",
            "tidy: skipped, began at +34.0 s, took 0 ms",
            "api (after): failed, began at +35.0 s, took 75 ms",
            "undo (rollback): succeeded, began at +35.5 s, took 250 ms"
        ])
    );
    // The words are shown as text: the script escapes what it puts in `{b}`.
    assert_eq!(
        timeline["tooltip"],
        json!({"trigger": "item", "formatter": "{b}"})
    );
}

#[test]
fn the_timeline_is_as_high_as_its_rows_need() {
    let seven = report_for(&run_of_every_step());
    let one = report_for(&Journal {
        rollback_results: vec![step("undo", ActivityStatus::Succeeded, 500, 250)],
        ..journal()
    });

    assert!(
        chart_host(&seven, "chart-timeline").starts_with(r#" data-height="268""#),
        "{seven}"
    );
    assert!(
        chart_host(&one, "chart-timeline").starts_with(r#" data-height="112""#),
        "{one}"
    );
}

#[test]
fn a_run_without_a_step_has_no_timeline() {
    let html = report_for(&Journal {
        steady_state_before: Some(checked(vec![step(
            "api",
            ActivityStatus::Succeeded,
            100,
            50,
        )])),
        load_result: Some(load(LoadExecution::default(), vec![])),
        ..journal()
    });

    assert!(!has_chart(&html, "chart-timeline"), "{html}");
}

#[test]
fn a_step_that_began_before_its_run_is_drawn_from_the_start() {
    let html = report_for(&Journal {
        method_results: vec![step("early", ActivityStatus::Succeeded, -250, 500)],
        ..journal()
    });

    let timeline = chart(&html, "chart-timeline");
    assert_eq!(timeline["series"][0]["data"], json!([0.0]));
    assert_eq!(
        each(&timeline, "/series/1/data", "/name"),
        json!(["early: succeeded, began at -250 ms, took 500 ms"])
    );
}

#[test]
fn the_timeline_shows_the_load_and_when_the_fault_was_active() {
    let html = report_for(&Journal {
        method_results: vec![step("break", ActivityStatus::Succeeded, 2000, 500)],
        during_result: Some(during()),
        load_result: Some(load(LoadExecution::default(), vec![])),
        ..journal()
    });

    let timeline = chart(&html, "chart-timeline");
    assert_eq!(timeline["yAxis"]["data"], json!(["break", "load (k6)"]));
    assert_eq!(timeline["series"][0]["data"], json!([2.0, 2.0]));
    assert_eq!(
        timeline["series"][1]["data"][1],
        json!({
            "value": 60.0,
            "name": "load (k6): began at +2.0 s, took 1 min 0 s",
            "itemStyle": {"color": "token:accent"}
        })
    );
    assert_eq!(
        timeline["series"][1]["markArea"]["data"],
        json!([[{"name": "fault active", "xAxis": 2.0}, {"xAxis": 14.5}]])
    );
    assert_eq!(
        timeline["series"][1]["markArea"]["itemStyle"]["color"],
        json!("token:fail")
    );
    assert!(
        chart_host(&html, "chart-timeline").starts_with(r#" data-height="138""#),
        "{html}"
    );
}

#[test]
fn a_run_without_a_fault_window_has_none_in_its_charts() {
    let html = report_for(&Journal {
        method_results: vec![step("break", ActivityStatus::Succeeded, 2000, 500)],
        load_result: Some(load(
            LoadExecution::default(),
            vec![window(2000, 1.0, 2.0, 3.0), window(12_000, 1.0, 2.0, 3.0)],
        )),
        probe_samples: vec![sample("api", SamplePhase::During, 3000, 12, true)],
        ..journal()
    });

    for id in [
        "chart-timeline",
        "chart-load-latency",
        "chart-load-throughput",
        "chart-probes",
    ] {
        let written = chart(&html, id).to_string();
        assert!(!written.contains("markArea"), "{id}: {written}");
    }
}

/// The name of the time axis of the timeline of a run of `length_ns` with
/// one step, and where that step begins and how long it is drawn.
fn timeline_of_a_run_of(length_ns: i64, step_after_ms: i64) -> (Value, Value, Value) {
    let html = report_for(&Journal {
        ended_at_ns: STARTED + length_ns,
        method_results: vec![step(
            "break",
            ActivityStatus::Succeeded,
            step_after_ms,
            30_000,
        )],
        ..journal()
    });
    let timeline = chart(&html, "chart-timeline");
    (
        timeline["xAxis"]["name"].clone(),
        timeline["series"][0]["data"][0].clone(),
        timeline["series"][1]["data"][0]["value"].clone(),
    )
}

#[test]
fn the_time_of_a_long_run_is_drawn_in_minutes_and_of_a_longer_one_in_hours() {
    let seconds = json!("Time from the start of the run (s)");
    let minutes = json!("Time from the start of the run (min)");
    let hours = json!("Time from the start of the run (h)");

    assert_eq!(
        timeline_of_a_run_of(600 * SECOND - 1, 90_000),
        (seconds, json!(90.0), json!(30.0))
    );
    assert_eq!(
        timeline_of_a_run_of(600 * SECOND, 90_000),
        (minutes.clone(), json!(1.5), json!(0.5))
    );
    assert_eq!(
        timeline_of_a_run_of(36_000 * SECOND - 1, 5_400_000),
        (minutes, json!(90.0), json!(0.5))
    );
    assert_eq!(
        timeline_of_a_run_of(36_000 * SECOND, 5_400_000),
        (hours, json!(1.5), json!(0.008))
    );
}

#[test]
fn a_time_in_a_chart_has_three_decimals_at_most() {
    let html = report_for(&Journal {
        method_results: vec![step("break", ActivityStatus::Succeeded, 1234, 2)],
        probe_samples: vec![ProbeSample {
            ts_ns: STARTED + 1_234_567_800,
            ..sample("api", SamplePhase::During, 0, 12, true)
        }],
        ..journal()
    });

    assert_eq!(
        chart(&html, "chart-probes")["series"][0]["data"],
        json!([[1.235, 12]])
    );
    let timeline = chart(&html, "chart-timeline");
    assert_eq!(timeline["series"][0]["data"], json!([1.234]));
    assert_eq!(timeline["series"][1]["data"][0]["value"], json!(0.002));
}

#[test]
fn a_load_of_two_windows_or_more_is_drawn_over_time() {
    let html = report_for(&Journal {
        during_result: Some(during()),
        load_result: Some(load(
            LoadExecution::default(),
            vec![
                window(2000, 40.0, 900.26, 1000.0),
                window(12_000, 41.5, 950.0, 1200.75),
                window(22_500, 39.0, 800.0, 990.0),
            ],
        )),
        ..journal()
    });

    let latency = chart(&html, "chart-load-latency");
    assert_eq!(
        each(&latency, "/series", "/name"),
        json!(["p50", "p95", "p99"])
    );
    assert_eq!(
        each(&latency, "/series", "/type"),
        json!(["line", "line", "line"])
    );
    assert_eq!(
        latency["series"][0]["data"],
        json!([[2.0, 40.0], [12.0, 41.5], [22.5, 39.0]])
    );
    assert_eq!(
        latency["series"][1]["data"],
        json!([[2.0, 900.26], [12.0, 950.0], [22.5, 800.0]])
    );
    assert_eq!(
        latency["series"][2]["data"],
        json!([[2.0, 1000.0], [12.0, 1200.75], [22.5, 990.0]])
    );
    // A window is a point: a load has few of them.
    assert_eq!(
        each(&latency, "/series", "/symbol"),
        json!(["circle", "circle", "circle"])
    );
    assert_eq!(latency["yAxis"]["name"], json!("Latency (ms)"));
    assert_eq!(
        latency["xAxis"]["name"],
        json!("Time from the start of the run (s)")
    );
    let fault = json!([[{"name": "fault active", "xAxis": 2.0}, {"xAxis": 14.5}]]);
    assert_eq!(latency["series"][0]["markArea"]["data"], fault);

    let throughput = chart(&html, "chart-load-throughput");
    assert_eq!(
        each(&throughput, "/series", "/name"),
        json!(["throughput", "error rate"])
    );
    assert_eq!(
        throughput["series"][0]["data"],
        json!([[2.0, 12.5], [12.0, 12.5], [22.5, 12.5]])
    );
    // A share of the requests, drawn in percent on an axis of its own.
    assert_eq!(
        throughput["series"][1]["data"],
        json!([[2.0, 25.0], [12.0, 25.0], [22.5, 25.0]])
    );
    assert_eq!(
        each(&throughput, "/yAxis", "/name"),
        json!(["Requests/s", "Errors (%)"])
    );
    assert_eq!(
        each(&throughput, "/series", "/symbol"),
        json!(["circle", "circle"])
    );
    assert_eq!(throughput["series"][0].get("yAxisIndex"), None);
    assert_eq!(throughput["series"][1]["yAxisIndex"], json!(1));
    assert_eq!(
        throughput["series"][1]["itemStyle"]["color"],
        json!("token:fail")
    );
    assert_eq!(
        throughput["series"][1]["lineStyle"]["color"],
        json!("token:fail")
    );
    assert_eq!(throughput["series"][0]["markArea"]["data"], fault);
}

#[test]
fn an_error_rate_in_a_chart_has_three_decimals_at_most() {
    let odd = |after_ms| {
        let mut odd = window(after_ms, 1.0, 2.0, 3.0);
        odd.error_rate = 0.123_456_7;
        odd
    };
    let html = report_for(&Journal {
        load_result: Some(load(LoadExecution::default(), vec![odd(2000), odd(12_000)])),
        ..journal()
    });

    assert_eq!(
        chart(&html, "chart-load-throughput")["series"][1]["data"],
        json!([[2.0, 12.346], [12.0, 12.346]])
    );
}

#[test]
fn a_load_of_one_window_has_no_chart() {
    let html = report_for(&Journal {
        load_result: Some(load(
            LoadExecution::default(),
            vec![window(2000, 40.0, 900.26, 1000.0)],
        )),
        ..journal()
    });

    assert_eq!(chart_ids(&html), Vec::<String>::new());
    assert!(!html.contains("<script"), "{html}");
}

/// A sample `probe` took `after_ms` into the run.
fn sample(probe: &str, phase: SamplePhase, after_ms: i64, took_ms: u64, ok: bool) -> ProbeSample {
    ProbeSample {
        probe: probe.into(),
        phase,
        ts_ns: STARTED + after_ms * MILLISECOND,
        duration_ms: took_ms,
        ok,
    }
}

#[test]
fn the_samples_of_each_probe_are_drawn_over_time() {
    let html = report_for(&Journal {
        during_result: Some(during()),
        probe_samples: vec![
            sample("api", SamplePhase::During, 2500, 12, true),
            sample("db", SamplePhase::During, 2600, 3, true),
            sample("api", SamplePhase::During, 3500, 80, false),
            sample("db", SamplePhase::During, 3600, 4, true),
            sample("api", SamplePhase::Post, 15_000, 10, true),
        ],
        ..journal()
    });

    let probes = chart(&html, "chart-probes");
    assert_eq!(each(&probes, "/series", "/name"), json!(["api", "db"]));
    assert_eq!(each(&probes, "/series", "/type"), json!(["line", "line"]));
    // A sample out of its tolerance is a larger point in the colour of a
    // failure.
    assert_eq!(
        probes["series"][0]["data"],
        json!([
            [2.5, 12],
            {
                "value": [3.5, 80],
                "symbol": "circle",
                "symbolSize": 9,
                "itemStyle": {"color": "token:fail"}
            },
            [15.0, 10]
        ])
    );
    assert_eq!(probes["series"][1]["data"], json!([[2.6, 3], [3.6, 4]]));
    // A probe has many samples: only those out of tolerance are points.
    assert_eq!(probes["series"][0].get("symbol"), None);
    assert_eq!(probes["yAxis"]["name"], json!("Probe time (ms)"));
    assert_eq!(
        probes["xAxis"]["name"],
        json!("Time from the start of the run (s)")
    );
    assert_eq!(
        probes["series"][0]["markArea"]["data"],
        json!([[{"name": "fault active", "xAxis": 2.0}, {"xAxis": 14.5}]])
    );
    assert_eq!(probes["series"][1].get("markArea"), None);
}

#[test]
fn a_run_without_samples_has_no_chart_of_them() {
    let html = report_for(&Journal {
        during_result: Some(during()),
        ..journal()
    });

    assert!(!has_chart(&html, "chart-probes"), "{html}");
}

/// A verdict with a margin to its threshold.
fn measured(id: &str, threshold: f64, margin: f64, state: NfrState) -> NfrVerdict {
    NfrVerdict {
        observed: Some(threshold - margin),
        margin: Some(margin),
        ..verdict(id, NfrOperator::Lte, threshold, state)
    }
}

fn objectives(verdicts: Vec<NfrVerdict>) -> ObjectiveOutcome {
    ObjectiveOutcome {
        verdicts,
        ..ObjectiveOutcome::default()
    }
}

#[test]
fn the_headroom_of_an_objective_is_drawn_in_percent_of_its_threshold() {
    let html = report_for(&Journal {
        objectives: Some(objectives(vec![
            measured("latency", 1000.0, 200.0, NfrState::Met),
            measured("errors", 0.25, -0.5, NfrState::Breached),
            measured("drift", -50.0, 10.0, NfrState::Met),
            measured("odd", 3.0, 1.0, NfrState::Met),
        ])),
        ..journal()
    });

    let headroom = chart(&html, "chart-headroom");
    assert_eq!(
        headroom["yAxis"]["data"],
        json!(["latency", "errors", "drift", "odd"])
    );
    assert_eq!(headroom["yAxis"]["inverse"], json!(true));
    assert_eq!(
        headroom["series"][0]["data"],
        json!([
            {
                "value": 20.0,
                "name": "latency: met, 800 against at most 1000",
                "itemStyle": {"color": "token:ok"}
            },
            {
                "value": -200.0,
                "name": "errors: breached, 0.75 against at most 0.25",
                "itemStyle": {"color": "token:fail"}
            },
            {
                "value": 20.0,
                "name": "drift: met, -60 against at most -50",
                "itemStyle": {"color": "token:ok"}
            },
            {
                "value": 33.333,
                "name": "odd: met, 2 against at most 3",
                "itemStyle": {"color": "token:ok"}
            }
        ])
    );
    assert_eq!(headroom["series"][0]["type"], json!("bar"));
    assert_eq!(
        headroom["xAxis"]["name"],
        json!("Headroom (% of the threshold)")
    );
    assert_eq!(
        headroom["tooltip"],
        json!({"trigger": "item", "formatter": "{b}"})
    );
    assert!(
        chart_host(&html, "chart-headroom").starts_with(r#" data-height="174""#),
        "{html}"
    );
}

#[test]
fn an_objective_says_how_its_value_must_compare() {
    let html = report_for(&Journal {
        objectives: Some(objectives(vec![
            NfrVerdict {
                operator: NfrOperator::Gte,
                ..measured("uptime", 99.0, 0.5, NfrState::Met)
            },
            NfrVerdict {
                operator: NfrOperator::Eq,
                ..measured("copies", 3.0, 0.0, NfrState::Met)
            },
        ])),
        ..journal()
    });

    assert_eq!(
        each(&chart(&html, "chart-headroom"), "/series/0/data", "/name"),
        json!([
            "uptime: met, 98.5 against at least 99",
            "copies: met, 3 against equal to 3"
        ])
    );
}

#[test]
fn an_objective_without_a_headroom_has_no_bar() {
    let unmeasured = verdict("unmeasured", NfrOperator::Lte, 10.0, NfrState::Met);
    let html = report_for(&Journal {
        objectives: Some(objectives(vec![
            measured("of-nothing", 0.0, 1.0, NfrState::Met),
            measured("unsure", 10.0, 1.0, NfrState::Inconclusive),
            unmeasured.clone(),
            measured("latency", 1000.0, 200.0, NfrState::Met),
        ])),
        ..journal()
    });
    let none = report_for(&Journal {
        objectives: Some(objectives(vec![unmeasured])),
        ..journal()
    });

    let headroom = chart(&html, "chart-headroom");
    assert_eq!(headroom["yAxis"]["data"], json!(["latency"]));
    assert_eq!(each(&headroom, "/series/0/data", "/value"), json!([20.0]));
    assert!(!has_chart(&none, "chart-headroom"), "{none}");
    assert!(!none.contains("<script"), "{none}");
}

/// A run with every chart a report can have.
fn run_of_every_chart() -> Journal {
    Journal {
        during_result: Some(during()),
        load_result: Some(load(
            LoadExecution::default(),
            vec![
                window(2000, 40.0, 900.26, 1000.0),
                window(12_000, 41.5, 950.0, 1200.75),
            ],
        )),
        probe_samples: vec![sample("api", SamplePhase::During, 2500, 12, true)],
        objectives: Some(objectives(vec![measured(
            "latency",
            1000.0,
            200.0,
            NfrState::Met,
        )])),
        ..run_of_every_step()
    }
}

#[test]
fn each_chart_is_in_the_section_of_what_it_shows() {
    let html = report_for(&run_of_every_chart());

    assert_eq!(
        chart_ids(&html),
        [
            "chart-timeline",
            "chart-headroom",
            "chart-load-latency",
            "chart-load-throughput",
            "chart-probes"
        ]
    );
    assert_eq!(chart_ids(section(&html, "steps")), ["chart-timeline"]);
    assert_eq!(chart_ids(section(&html, "objectives")), ["chart-headroom"]);
    assert_eq!(
        chart_ids(section(&html, "load")),
        ["chart-load-latency", "chart-load-throughput"]
    );
    assert_eq!(chart_ids(section(&html, "measurements")), ["chart-probes"]);
}

#[test]
fn each_chart_says_what_it_shows_to_who_cannot_see_it() {
    let html = report_for(&run_of_every_chart());

    let said = [
        (
            "chart-timeline",
            "Timeline of the run: when each step began and how long it took",
        ),
        (
            "chart-headroom",
            "Headroom of each objective, in percent of its threshold",
        ),
        (
            "chart-load-latency",
            "Latency of the load over time: p50, p95 and p99 of each window",
        ),
        (
            "chart-load-throughput",
            "Throughput and error rate of the load over time",
        ),
        (
            "chart-probes",
            "Time of each probe sample during and after the fault",
        ),
    ];
    for (id, label) in said {
        let host = chart_host(&html, id);
        assert!(
            host.ends_with(&format!(r#" role="img" aria-label="{label}""#)),
            "{id}: {host}"
        );
    }
    assert!(
        chart_host(&html, "chart-load-latency").starts_with(r#" data-height="260""#),
        "{html}"
    );
}

#[test]
fn no_chart_moves_when_it_is_drawn() {
    let html = report_for(&run_of_every_chart());

    let ids = chart_ids(&html);
    assert_eq!(ids.len(), 5);
    for id in ids {
        assert_eq!(chart(&html, &id)["animation"], json!(false), "{id}");
    }
}

#[test]
fn a_report_with_a_chart_ends_with_the_script_that_draws_it() {
    let html = report_for(&run_of_every_chart());
    let script = chart_script();

    assert_eq!(html.matches(&script).count(), 1);
    assert!(
        html.ends_with(&format!(
            "</main>\n<script>\n{script}\n</script>\n</body>\n</html>\n"
        )),
        "the script is not the end of the report"
    );
    // One element for the script, and one for the option of each chart.
    assert_eq!(html.matches("<script").count(), 6);
    assert_eq!(html.matches("</script>").count(), 6);
}

#[test]
fn the_script_of_the_charts_can_stand_in_a_page() {
    let script = chart_script().to_lowercase();

    // Neither ends the element the script stands in.
    assert!(!script.contains("</script"));
    assert!(!script.contains("<!--"));
}

#[test]
fn a_text_of_a_journal_cannot_end_the_option_of_a_chart() {
    let hostile = "</script><script>alert(1)</script><!-- <b>";
    let html = report_for(&Journal {
        method_results: vec![step(hostile, ActivityStatus::Succeeded, 2000, 500)],
        probe_samples: vec![sample(hostile, SamplePhase::During, 2500, 12, true)],
        objectives: Some(objectives(vec![measured(
            hostile,
            1000.0,
            200.0,
            NfrState::Met,
        )])),
        ..journal()
    });

    // The text is in the options as it is in the journal.
    assert_eq!(
        chart(&html, "chart-timeline")["yAxis"]["data"],
        json!([hostile])
    );
    assert_eq!(
        chart(&html, "chart-probes")["series"][0]["name"],
        json!(hostile)
    );
    assert_eq!(
        chart(&html, "chart-headroom")["yAxis"]["data"],
        json!([hostile])
    );
    // And no part of it is markup: three options and the script.
    assert_eq!(html.matches("<script").count(), 4);
    assert_eq!(html.matches("</script>").count(), 4);
    assert!(!html.contains("<!--"), "no comment");
    assert!(!html.contains("<b>"), "no markup");
    assert!(
        html.contains(r"\u003c/script>\u003cscript>alert(1)\u003c/script>\u003c!-- \u003cb>"),
        "the text as the option holds it"
    );
}
