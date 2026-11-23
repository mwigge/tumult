//! Journal report renderers shared by the CLI and MCP server.
//!
//! JSON (the raw journal serialized) and `JUnit` XML (one `<testcase>` per
//! step of every phase, one per check of the steady state and one per
//! objective). Richer formats (HTML/PDF) stay in the CLI.

use std::borrow::Cow;
use std::fmt::Write as _;

use crate::types::{
    ActivityResult, ActivityStatus, HypothesisResult, Journal, NfrCriticality, NfrOperator,
    NfrState, NfrVerdict,
};

/// HTML-safe escaping of `&`, `<`, `>`, and `"`.
#[must_use]
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// XML-safe escaping. Extends [`html_escape`] with the apostrophe entity.
#[must_use]
pub fn xml_escape(s: &str) -> String {
    html_escape(s).replace('\'', "&apos;")
}

/// The raw journal serialized as pretty JSON.
///
/// # Errors
///
/// Returns an error if the journal cannot be serialized.
pub fn json_report(journal: &Journal) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(journal)
}

/// How one case of the suite ended.
enum Verdict<'a> {
    Passed,
    /// Passed, with a note for the output of the case.
    Noted(String),
    /// Not judged, with the reason when there is one.
    Skipped(Option<String>),
    /// The `message` of the `<failure>` element and its text.
    Failed(Cow<'a, str>, Cow<'a, str>),
}

/// One `<testcase>`: a step of a phase, a check of the steady state, or an
/// objective.
struct Case<'a> {
    name: &'a str,
    class: Cow<'a, str>,
    duration_ms: u64,
    verdict: Verdict<'a>,
}

fn activity_case<'a>(phase: &'static str, result: &'a ActivityResult) -> Case<'a> {
    let text = result.error.as_deref().or(result.output.as_deref());
    let verdict = match result.status {
        ActivityStatus::Failed => Verdict::Failed("Failed".into(), text.unwrap_or("").into()),
        ActivityStatus::Skipped => Verdict::Skipped(None),
        ActivityStatus::Succeeded => Verdict::Passed,
    };
    Case {
        name: &result.name,
        class: phase.into(),
        duration_ms: result.duration_ms,
        verdict,
    }
}

/// The case of one check of the steady state, then one case per probe of it.
/// The check fails when the steady state did not hold, so a deviated run has
/// a failing case whatever its probes say.
fn hypothesis_cases<'a>(
    phase: &'static str,
    hypothesis: Option<&'a HypothesisResult>,
) -> Vec<Case<'a>> {
    let Some(hypothesis) = hypothesis else {
        return Vec::new();
    };
    let verdict = if hypothesis.met {
        Verdict::Passed
    } else {
        Verdict::Failed("NotMet".into(), "the steady state did not hold".into())
    };
    let check = Case {
        name: &hypothesis.title,
        class: phase.into(),
        duration_ms: hypothesis.probe_results.iter().map(|r| r.duration_ms).sum(),
        verdict,
    };
    let probes = hypothesis
        .probe_results
        .iter()
        .map(|result| activity_case(phase, result));
    std::iter::once(check).chain(probes).collect()
}

fn activity_cases<'a>(phase: &'static str, results: &'a [ActivityResult]) -> Vec<Case<'a>> {
    results
        .iter()
        .map(|result| activity_case(phase, result))
        .collect()
}

/// How the operator of an objective reads in a sentence.
fn must_be(operator: NfrOperator) -> &'static str {
    match operator {
        NfrOperator::Lte => "at most",
        NfrOperator::Gte => "at least",
        NfrOperator::Eq => "equal to",
    }
}

/// `value` with at most three decimals and no trailing zero.
fn number(value: f64) -> String {
    let written = format!("{value:.3}");
    written
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// The case of one objective; its class names the kind and the metric.
/// The case follows the verdict of the run: only a breach of a blocking
/// objective fails the run, so only that breach fails the case. An
/// objective the run did not measure fails no run, so its case is skipped.
fn objective_case(verdict: &NfrVerdict) -> Case<'_> {
    let observed = verdict.observed.map_or_else(
        || "not measured".to_string(),
        |value| format!("observed {}", number(value)),
    );
    let finding = format!(
        "{} must be {} {}, {observed}",
        verdict.metric,
        must_be(verdict.operator),
        number(verdict.threshold)
    );
    let outcome = match verdict.state {
        NfrState::Met => Verdict::Passed,
        NfrState::Inconclusive => Verdict::Skipped(Some(finding)),
        NfrState::Breached if verdict.criticality == NfrCriticality::Blocking => {
            Verdict::Failed(finding.clone().into(), finding.into())
        }
        NfrState::Breached => Verdict::Noted(format!(
            "breached: {finding}. A {} objective does not fail the run.",
            verdict.criticality
        )),
    };
    Case {
        name: &verdict.nfr_id,
        class: format!("objective.{}.{}", verdict.kind, verdict.metric).into(),
        duration_ms: 0,
        verdict: outcome,
    }
}

fn seconds(duration_ms: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let ms = duration_ms as f64;
    ms / 1000.0
}

fn write_case(out: &mut String, case: &Case) {
    let _ = write!(
        out,
        r#"  <testcase name="{}" classname="{}" time="{:.3}">"#,
        xml_escape(case.name),
        xml_escape(&case.class),
        seconds(case.duration_ms)
    );
    match &case.verdict {
        Verdict::Failed(message, text) => {
            let _ = write!(
                out,
                r#"<failure message="{}">{}</failure>"#,
                xml_escape(message),
                xml_escape(text)
            );
        }
        Verdict::Skipped(None) => out.push_str("<skipped/>"),
        Verdict::Skipped(Some(reason)) => {
            let reason = xml_escape(reason);
            let _ = write!(out, r#"<skipped message="{reason}">{reason}</skipped>"#);
        }
        Verdict::Noted(note) => {
            let _ = write!(out, "<system-out>{}</system-out>", xml_escape(note));
        }
        Verdict::Passed => {}
    }
    out.push_str("</testcase>\n");
}

/// Minimal `JUnit` XML: one `<testcase>` per step of every phase, one per
/// check of the steady state, and one per objective. The suite is the form
/// Jenkins and GitLab read (`junit-10.xsd`); it never holds an `<error>`.
#[must_use]
pub fn junit_report(journal: &Journal) -> String {
    let mut cases = hypothesis_cases("hypothesis_before", journal.steady_state_before.as_ref());
    cases.extend(activity_cases("method", &journal.method_results));
    cases.extend(hypothesis_cases(
        "hypothesis_after",
        journal.steady_state_after.as_ref(),
    ));
    cases.extend(activity_cases("rollback", &journal.rollback_results));
    cases.extend(
        journal
            .objectives
            .iter()
            .flat_map(|outcome| &outcome.verdicts)
            .map(objective_case),
    );

    let failures = cases
        .iter()
        .filter(|case| matches!(case.verdict, Verdict::Failed(..)))
        .count();
    let skipped = cases
        .iter()
        .filter(|case| matches!(case.verdict, Verdict::Skipped(_)))
        .count();

    let mut out = String::new();
    let _ = writeln!(out, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        out,
        r#"<testsuite name="{}" tests="{}" failures="{failures}" errors="0" skipped="{skipped}" time="{:.3}">"#,
        xml_escape(&journal.experiment_title),
        cases.len(),
        seconds(journal.duration_ms)
    );
    for case in &cases {
        write_case(&mut out, case);
    }
    out.push_str("</testsuite>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExperimentStatus, NfrKind, ObjectiveOutcome};

    #[test]
    fn xml_escape_covers_all_entities() {
        assert_eq!(
            xml_escape(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&apos;"
        );
    }

    #[test]
    fn junit_report_emits_suite_and_cases() {
        let journal = Journal {
            target_version: None,
            target: None,
            experiment_title: "Report & test".into(),
            experiment_id: "exp-1".into(),
            status: crate::types::ExperimentStatus::Completed,
            started_at_ns: 0,
            ended_at_ns: 0,
            duration_ms: 1500,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![
                ActivityResult {
                    name: "step-ok".into(),
                    activity_type: crate::types::ActivityType::Action,
                    status: ActivityStatus::Succeeded,
                    started_at_ns: 0,
                    duration_ms: 100,
                    output: None,
                    error: None,
                    trace_id: crate::types::TraceId::empty(),
                    span_id: crate::types::SpanId::empty(),
                },
                ActivityResult {
                    name: "step-bad".into(),
                    activity_type: crate::types::ActivityType::Action,
                    status: ActivityStatus::Failed,
                    started_at_ns: 0,
                    duration_ms: 50,
                    output: None,
                    error: Some("boom <&>".into()),
                    trace_id: crate::types::TraceId::empty(),
                    span_id: crate::types::SpanId::empty(),
                },
            ],
            rollback_results: vec![],
            rollback_failures: 0,
            kind: crate::types::TestKind::default(),
            objectives: None,
            probe_samples: vec![],
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        };
        let xml = junit_report(&journal);
        assert!(xml.contains(r#"<testsuite name="Report &amp; test" tests="2" failures="1""#));
        assert!(xml.contains(r#"<testcase name="step-ok" classname="method""#));
        assert!(xml.contains("boom &lt;&amp;&gt;"));
        assert!(xml.ends_with("</testsuite>\n"));

        let json = json_report(&journal).unwrap();
        assert!(json.contains("\"experiment_title\": \"Report & test\""));
    }

    fn probe(name: &str) -> ActivityResult {
        ActivityResult {
            name: name.into(),
            activity_type: crate::types::ActivityType::Probe,
            status: ActivityStatus::Succeeded,
            started_at_ns: 0,
            duration_ms: 250,
            output: Some("503".into()),
            error: None,
            trace_id: crate::types::TraceId::empty(),
            span_id: crate::types::SpanId::empty(),
        }
    }

    fn steady_state(met: bool) -> HypothesisResult {
        HypothesisResult {
            title: "the API answers <200>".into(),
            met,
            probe_results: vec![probe("health"), probe("orders")],
        }
    }

    /// A run that ended `status`, with the steady state checked before and
    /// after the method.
    fn judged(status: ExperimentStatus, before: bool, after: bool) -> Journal {
        let experiment = crate::types::Experiment {
            title: "Judged".into(),
            ..Default::default()
        };
        Journal {
            steady_state_before: Some(steady_state(before)),
            steady_state_after: Some(steady_state(after)),
            ..Journal::for_experiment(&experiment, "exp-2".into(), status, 0)
        }
    }

    #[test]
    fn a_deviated_run_has_a_failing_case_for_the_hypothesis() {
        let xml = junit_report(&judged(ExperimentStatus::Deviated, true, false));

        assert!(xml.contains(r#"tests="6" failures="1""#), "{xml}");
        assert!(
            xml.contains(concat!(
                r#"<testcase name="the API answers &lt;200&gt;" classname="hypothesis_after" time="0.500">"#,
                r#"<failure message="NotMet">the steady state did not hold</failure></testcase>"#
            )),
            "{xml}"
        );
    }

    #[test]
    fn a_hypothesis_that_held_is_a_passing_case() {
        let xml = junit_report(&judged(ExperimentStatus::Completed, true, true));

        assert!(xml.contains(r#"tests="6" failures="0""#), "{xml}");
        assert!(
            xml.contains(concat!(
                r#"<testcase name="the API answers &lt;200&gt;" classname="hypothesis_before" time="0.500">"#,
                "</testcase>"
            )),
            "{xml}"
        );
    }

    #[test]
    fn a_run_aborted_before_the_method_fails_the_first_check() {
        let journal = Journal {
            steady_state_after: None,
            ..judged(ExperimentStatus::Aborted, false, false)
        };

        let xml = junit_report(&journal);

        assert!(xml.contains(r#"tests="3" failures="1""#), "{xml}");
        assert!(
            xml.contains(
                r#"classname="hypothesis_before" time="0.500"><failure message="NotMet">"#
            ),
            "{xml}"
        );
    }

    // ── Objectives ─────────────────────────────────────────────

    /// The verdict of the objective `id`: `metric` at most 2.
    fn verdict(
        id: &str,
        kind: NfrKind,
        metric: &str,
        observed: Option<f64>,
        state: NfrState,
        criticality: NfrCriticality,
    ) -> NfrVerdict {
        NfrVerdict {
            nfr_id: id.into(),
            kind,
            metric: metric.into(),
            operator: NfrOperator::Lte,
            threshold: 2.0,
            observed,
            state,
            margin: observed.map(|value| 2.0 - value),
            criticality,
            recovered: None,
        }
    }

    /// The recovery time objective of a DR run, breached at about 5 s.
    fn rto_breached() -> NfrVerdict {
        verdict(
            "rto",
            NfrKind::Rto,
            "recovery.mttr_s",
            Some(5.014_501),
            NfrState::Breached,
            NfrCriticality::Blocking,
        )
    }

    /// A run whose steady state held before and after, with these verdicts.
    fn with_objectives(status: ExperimentStatus, verdicts: Vec<NfrVerdict>) -> Journal {
        Journal {
            objectives: Some(ObjectiveOutcome {
                rto_target_s: Some(2.0),
                verdicts,
                ..ObjectiveOutcome::default()
            }),
            ..judged(status, true, true)
        }
    }

    #[test]
    fn a_run_that_breaches_its_rto_has_a_failing_case_for_it() {
        let xml = junit_report(&with_objectives(
            ExperimentStatus::Deviated,
            vec![rto_breached()],
        ));

        assert!(xml.contains(r#"tests="7" failures="1""#), "{xml}");
        assert!(
            xml.contains(concat!(
                r#"<testcase name="rto" classname="objective.rto.recovery.mttr_s" time="0.000">"#,
                r#"<failure message="recovery.mttr_s must be at most 2, observed 5.015">"#,
                "recovery.mttr_s must be at most 2, observed 5.015</failure></testcase>"
            )),
            "{xml}"
        );
    }

    #[test]
    fn a_met_objective_is_a_passing_case() {
        let met = verdict(
            "p95",
            NfrKind::Latency,
            "probe.health.p95_ms",
            Some(1.5),
            NfrState::Met,
            NfrCriticality::Blocking,
        );

        let xml = junit_report(&with_objectives(ExperimentStatus::Completed, vec![met]));

        assert!(xml.contains(r#"tests="7" failures="0""#), "{xml}");
        assert!(
            xml.contains(concat!(
                r#"<testcase name="p95" classname="objective.latency.probe.health.p95_ms" time="0.000">"#,
                "</testcase>"
            )),
            "{xml}"
        );
    }

    #[test]
    fn an_objective_the_run_did_not_measure_is_a_skipped_case() {
        // The run does not fail on a blocking objective it could not
        // measure, so neither does the report.
        let unmeasured = verdict(
            "rpo",
            NfrKind::Rpo,
            "rpo",
            None,
            NfrState::Inconclusive,
            NfrCriticality::Blocking,
        );

        let xml = junit_report(&with_objectives(
            ExperimentStatus::Completed,
            vec![unmeasured],
        ));

        assert!(
            xml.contains(r#"tests="7" failures="0" errors="0" skipped="1""#),
            "{xml}"
        );
        assert!(
            xml.contains(concat!(
                r#"<testcase name="rpo" classname="objective.rpo.rpo" time="0.000">"#,
                r#"<skipped message="rpo must be at most 2, not measured">"#,
                "rpo must be at most 2, not measured</skipped></testcase>"
            )),
            "{xml}"
        );
    }

    #[test]
    fn a_breach_that_does_not_block_the_run_passes_with_a_note() {
        // A major or minor breach leaves the status of the run alone, so it
        // does not fail the report either. The note says what was breached.
        let major = NfrVerdict {
            operator: NfrOperator::Gte,
            threshold: 0.999,
            ..verdict(
                "avail",
                NfrKind::Availability,
                "post.availability",
                Some(0.5),
                NfrState::Breached,
                NfrCriticality::Major,
            )
        };

        let xml = junit_report(&with_objectives(ExperimentStatus::Completed, vec![major]));

        assert!(xml.contains(r#"tests="7" failures="0""#), "{xml}");
        assert!(
            xml.contains(concat!(
                r#"<testcase name="avail" classname="objective.availability.post.availability" time="0.000">"#,
                "<system-out>breached: post.availability must be at least 0.999, observed 0.5. ",
                "A major objective does not fail the run.</system-out></testcase>"
            )),
            "{xml}"
        );
    }

    #[test]
    fn the_objective_cases_follow_the_cases_of_the_steps() {
        let xml = junit_report(&with_objectives(
            ExperimentStatus::Deviated,
            vec![rto_breached()],
        ));

        let classes: Vec<&str> = xml
            .split(r#"classname=""#)
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect();
        assert_eq!(
            classes,
            [
                "hypothesis_before",
                "hypothesis_before",
                "hypothesis_before",
                "hypothesis_after",
                "hypothesis_after",
                "hypothesis_after",
                "objective.rto.recovery.mttr_s",
            ]
        );
    }

    // ── Schema ─────────────────────────────────────────────────

    /// What the Jenkins schema of `JUnit` XML allows (`junit-10.xsd` of the
    /// xunit plugin). GitLab reads the same elements.
    const SUITE_ATTRIBUTES: &[&str] = &[
        "name",
        "tests",
        "failures",
        "errors",
        "group",
        "time",
        "skipped",
        "timestamp",
        "hostname",
        "id",
        "package",
        "file",
        "log",
        "url",
        "version",
    ];
    const SUITE_REQUIRED: &[&str] = &["name", "tests", "failures", "errors"];
    const SUITE_CHILDREN: &[&str] = &[
        "testsuite",
        "properties",
        "testcase",
        "system-out",
        "system-err",
    ];
    const CASE_ATTRIBUTES: &[&str] = &["name", "time", "classname", "group"];
    const CASE_CHILDREN: &[&str] = &[
        "skipped",
        "error",
        "failure",
        "rerunFailure",
        "rerunError",
        "flakyFailure",
        "flakyError",
        "system-out",
        "system-err",
    ];
    const OUTCOME_ATTRIBUTES: &[&str] = &["type", "message"];
    /// The `SUREFIRE_TIME` type of the time of the suite.
    const SUREFIRE_TIME: &str = r"^(?:(?:[0-9]{0,3},)*[0-9]{3}|[0-9]{0,3})*(?:\.[0-9]{0,3})?$";

    fn elements<'a>(
        node: roxmltree::Node<'a, 'a>,
    ) -> impl Iterator<Item = roxmltree::Node<'a, 'a>> {
        node.children().filter(roxmltree::Node::is_element)
    }

    fn assert_attributes_allowed(node: roxmltree::Node, allowed: &[&str]) {
        for attribute in node.attributes() {
            assert!(
                allowed.contains(&attribute.name()),
                "<{}> may not have the attribute {}",
                node.tag_name().name(),
                attribute.name()
            );
        }
    }

    /// How many cases of the suite have a child named `outcome`.
    fn cases_with(suite: roxmltree::Node, outcome: &str) -> usize {
        elements(suite)
            .filter(|case| elements(*case).any(|child| child.has_tag_name(outcome)))
            .count()
    }

    /// `xml` parses, holds only what the schema allows, and its counts are
    /// the counts of its cases.
    fn assert_valid_junit(xml: &str) {
        let document = roxmltree::Document::parse(xml).unwrap_or_else(|e| panic!("{e}: {xml}"));
        let suite = document.root_element();
        assert!(suite.has_tag_name("testsuite"), "{xml}");
        assert_attributes_allowed(suite, SUITE_ATTRIBUTES);
        for required in SUITE_REQUIRED {
            assert!(suite.attribute(*required).is_some(), "no {required}: {xml}");
        }
        let time = suite.attribute("time").unwrap_or_default();
        let time_type = regex_lite::Regex::new(SUREFIRE_TIME).expect("the pattern");
        assert!(time_type.is_match(time), "time {time}");

        for case in elements(suite) {
            let tag = case.tag_name().name();
            assert!(
                SUITE_CHILDREN.contains(&tag),
                "<testsuite> may not hold <{tag}>"
            );
            assert!(case.has_tag_name("testcase"), "{xml}");
            assert_attributes_allowed(case, CASE_ATTRIBUTES);
            assert!(
                case.attribute("name").is_some(),
                "a case without a name: {xml}"
            );
            for outcome in elements(case) {
                let tag = outcome.tag_name().name();
                assert!(
                    CASE_CHILDREN.contains(&tag),
                    "<testcase> may not hold <{tag}>"
                );
                assert_attributes_allowed(outcome, OUTCOME_ATTRIBUTES);
                assert_eq!(elements(outcome).count(), 0, "<{tag}> holds only text");
            }
        }

        let count = |name: &str| suite.attribute(name).map(str::to_string);
        let cases = elements(suite).count().to_string();
        assert_eq!(count("tests"), Some(cases), "{xml}");
        for (attribute, outcome) in [
            ("failures", "failure"),
            ("errors", "error"),
            ("skipped", "skipped"),
        ] {
            assert_eq!(
                count(attribute),
                Some(cases_with(suite, outcome).to_string()),
                "{attribute}: {xml}"
            );
        }
    }

    #[test]
    fn the_report_is_valid_junit_for_jenkins_and_gitlab() {
        let mut journal = with_objectives(
            ExperimentStatus::Deviated,
            vec![
                rto_breached(),
                verdict(
                    "rpo <&>",
                    NfrKind::Rpo,
                    "probe.<db>.error_rate",
                    None,
                    NfrState::Inconclusive,
                    NfrCriticality::Blocking,
                ),
                verdict(
                    "slow",
                    NfrKind::Latency,
                    "probe.<db>.max_ms",
                    Some(3.0),
                    NfrState::Breached,
                    NfrCriticality::Minor,
                ),
                verdict(
                    "p95",
                    NfrKind::Latency,
                    r#"probe.it's "up".p95_ms"#,
                    Some(3.0),
                    NfrState::Breached,
                    NfrCriticality::Blocking,
                ),
                verdict(
                    "fast",
                    NfrKind::Latency,
                    "fault.duration_s",
                    Some(1.0),
                    NfrState::Met,
                    NfrCriticality::Major,
                ),
            ],
        );
        journal.duration_ms = 1_234_567;
        let mut failed = probe("restore <primary>");
        failed.status = ActivityStatus::Failed;
        failed.error = Some(r#"exit 1 & "no backup""#.into());
        let mut skipped = probe("verify");
        skipped.status = ActivityStatus::Skipped;
        journal.method_results = vec![failed, skipped];

        let xml = junit_report(&journal);

        assert_valid_junit(&xml);
        assert!(
            xml.contains(r#"tests="13" failures="3" errors="0" skipped="2""#),
            "{xml}"
        );
    }
}
