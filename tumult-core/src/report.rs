//! Journal report renderers shared by the CLI and MCP server.
//!
//! JSON (the raw journal serialized) and `JUnit` XML (one `<testcase>` per
//! step of every phase and one per check of the steady state). Richer
//! formats (HTML/PDF) stay in the CLI.

use std::fmt::Write as _;

use crate::types::{ActivityResult, ActivityStatus, HypothesisResult, Journal};

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
    Skipped,
    /// The `message` of the `<failure>` element and its text.
    Failed(&'static str, &'a str),
}

/// One `<testcase>`: a step of a phase, or a check of the steady state.
struct Case<'a> {
    name: &'a str,
    phase: &'static str,
    duration_ms: u64,
    verdict: Verdict<'a>,
}

fn activity_case<'a>(phase: &'static str, result: &'a ActivityResult) -> Case<'a> {
    let text = result.error.as_deref().or(result.output.as_deref());
    let verdict = match result.status {
        ActivityStatus::Failed => Verdict::Failed("Failed", text.unwrap_or("")),
        ActivityStatus::Timeout => Verdict::Failed("Timeout", text.unwrap_or("")),
        ActivityStatus::Skipped => Verdict::Skipped,
        ActivityStatus::Succeeded => Verdict::Passed,
    };
    Case {
        name: &result.name,
        phase,
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
        Verdict::Failed("NotMet", "the steady state did not hold")
    };
    let check = Case {
        name: &hypothesis.title,
        phase,
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
        case.phase,
        seconds(case.duration_ms)
    );
    match case.verdict {
        Verdict::Failed(message, text) => {
            let _ = write!(
                out,
                r#"<failure message="{message}">{}</failure>"#,
                xml_escape(text)
            );
        }
        Verdict::Skipped => out.push_str("<skipped/>"),
        Verdict::Passed => {}
    }
    out.push_str("</testcase>\n");
}

/// Minimal `JUnit` XML: one `<testcase>` per step of every phase, and one per
/// check of the steady state.
#[must_use]
pub fn junit_report(journal: &Journal) -> String {
    let mut cases = hypothesis_cases("hypothesis_before", journal.steady_state_before.as_ref());
    cases.extend(activity_cases("method", &journal.method_results));
    cases.extend(hypothesis_cases(
        "hypothesis_after",
        journal.steady_state_after.as_ref(),
    ));
    cases.extend(activity_cases("rollback", &journal.rollback_results));

    let failures = cases
        .iter()
        .filter(|case| matches!(case.verdict, Verdict::Failed(..)))
        .count();

    let mut out = String::new();
    let _ = writeln!(out, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        out,
        r#"<testsuite name="{}" tests="{}" failures="{}" time="{:.3}">"#,
        xml_escape(&journal.experiment_title),
        cases.len(),
        failures,
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
    use crate::types::ExperimentStatus;

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
}
