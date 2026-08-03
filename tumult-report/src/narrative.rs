//! LLM narrative grounding for digests.
//!
//! The pipeline: build a facts package from the report's own numbers, ask
//! the LLM for a short prose summary, then keep only the sentences whose
//! numeric literals are all grounded in those facts (percent values match
//! both the `x` and `x/100` forms). Ungrounded sentences — the ones likely
//! to carry hallucinated figures — are dropped; if nothing survives, the
//! digest ships without a narrative at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tumult_intelligence::llm::{Llm, Message, Role};

use crate::{Report, Section};

/// One numeric literal found in text: value and whether it carried a `%`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Numeral {
    pub value: f64,
    pub percent: bool,
}

/// Extract numeric literals from `text` (optional sign, decimals, optional
/// `%` suffix). Thousands separators are not supported; version-like tokens
/// (`0.3.0`) yield only their leading number.
#[must_use]
pub fn extract_numerals(text: &str) -> Vec<Numeral> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let starts_number = c.is_ascii_digit() && prev != Some('.')
            || (c == '-' || c == '+')
                && chars.get(i + 1).is_some_and(char::is_ascii_digit)
                && prev != Some('.');
        if !starts_number {
            i += 1;
            continue;
        }
        let start = i;
        i += usize::from(c == '-' || c == '+');
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        }
        let percent = chars.get(i) == Some(&'%');
        if percent {
            i += 1;
        }
        if let Ok(value) = chars[start..i - usize::from(percent)]
            .iter()
            .collect::<String>()
            .parse::<f64>()
        {
            out.push(Numeral { value, percent });
        }
    }
    out
}

/// Split into sentences on `.`/`!`/`?` followed by whitespace or end of
/// text (a `.` between digits never splits, so decimals stay whole).
fn sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..chars.len() {
        let c = chars[i];
        if c != '.' && c != '!' && c != '?' {
            continue;
        }
        let next = chars.get(i + 1);
        if next.is_some_and(|n| !n.is_whitespace()) {
            continue;
        }
        let s: String = chars[start..=i].iter().collect();
        let s = s.trim();
        if !s.is_empty() {
            out.push(s.to_string());
        }
        start = i + 1;
    }
    let tail: String = chars[start..].iter().collect();
    let tail = tail.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// The facts package handed to the LLM: every number the narrative is
/// allowed to cite, straight from the report sections.
#[must_use]
pub fn facts_package(report: &Report) -> Value {
    let kpis: Vec<Value> = report
        .sections
        .iter()
        .filter_map(|s| match s {
            Section::Kpi {
                label,
                value,
                delta,
            } => Some(json!({"label": label, "value": value, "delta": delta})),
            _ => None,
        })
        .collect();
    let tables: Vec<Value> = report
        .sections
        .iter()
        .filter_map(|s| match s {
            Section::Table { headers, rows } => Some(json!({"headers": headers, "rows": rows})),
            _ => None,
        })
        .collect();
    json!({"title": report.title, "kpis": kpis, "tables": tables})
}

/// Every number cited by the report. Percent-suffixed facts are recorded in
/// both `x` and `x/100` forms so a narrative may use either convention.
fn facts_numbers(report: &Report) -> Vec<f64> {
    let mut texts: Vec<&str> = Vec::new();
    for section in &report.sections {
        match section {
            Section::Kpi {
                label,
                value,
                delta,
            } => {
                texts.push(label);
                texts.push(value);
                if let Some(d) = delta {
                    texts.push(d);
                }
            }
            Section::Table { headers, rows } => {
                texts.extend(headers.iter().map(String::as_str));
                texts.extend(rows.iter().flatten().map(String::as_str));
            }
            Section::Narrative { .. } | Section::ChartRef { .. } => {}
        }
    }
    let mut numbers = Vec::new();
    for text in texts {
        for n in extract_numerals(text) {
            numbers.push(n.value);
            if n.percent {
                numbers.push(n.value / 100.0);
            }
        }
    }
    numbers
}

/// Relative tolerance for grounding: LLMs round, so a numeral matches a
/// fact when it is within 1% (absolute 0.01 for facts near zero).
fn grounded(n: f64, facts: &[f64]) -> bool {
    facts
        .iter()
        .any(|f| (n - f).abs() <= 0.01 * f.abs().max(1.0))
}

/// Keep only the sentences of `narrative` whose numerals are all grounded
/// in the report. Percent numerals may match in either `x` or `x/100` form.
/// Returns `None` when no sentence survives — an empty or fully ungrounded
/// reply must leave the digest unchanged.
#[must_use]
pub fn ground_narrative(narrative: &str, report: &Report) -> Option<String> {
    let facts = facts_numbers(report);
    let kept: Vec<String> = sentences(narrative)
        .into_iter()
        .filter(|s| {
            extract_numerals(s).iter().all(|n| {
                if n.percent {
                    grounded(n.value, &facts) || grounded(n.value / 100.0, &facts)
                } else {
                    grounded(n.value, &facts)
                }
            })
        })
        .collect();
    let text = kept.join(" ");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Chat messages requesting the narrative: a strict system prompt plus the
/// facts package as the user turn.
#[must_use]
pub fn narrative_messages(report: &Report) -> Vec<Message> {
    let facts = facts_package(report);
    vec![
        Message {
            role: Role::System,
            content: "You write the summary paragraph of an automated resilience-metrics \
                      digest. Rules: use ONLY numbers that appear verbatim in the facts \
                      JSON the user provides; never invent or estimate figures; write 2-4 \
                      short sentences of plain prose (no markdown, no headers, no bullets)."
                .into(),
        },
        Message {
            role: Role::User,
            content: format!(
                "Facts for digest {:?}:\n{}",
                report.title,
                serde_json::to_string_pretty(&facts).unwrap_or_else(|_| facts.to_string())
            ),
        },
    ]
}

/// Full pipeline: chat with a wall-clock timeout, ground the reply against
/// the report's numbers, and prepend a [`Section::Narrative`] when anything
/// survives. Any failure — LLM unreachable, timeout, empty or fully
/// ungrounded reply — returns the report unchanged.
pub async fn narrate(llm: &Arc<dyn Llm>, mut report: Report, timeout: Duration) -> Report {
    let messages = narrative_messages(&report);
    let reply = tokio::time::timeout(timeout, llm.chat(&messages)).await;
    match reply {
        Ok(Ok(text)) => {
            if let Some(grounded) = ground_narrative(&text, &report) {
                report
                    .sections
                    .insert(0, Section::Narrative { text: grounded });
            } else {
                tracing::info!("LLM narrative dropped: no grounded sentences survived");
            }
        }
        Ok(Err(e)) => tracing::info!(error = %e, "LLM narrative skipped"),
        Err(_) => tracing::info!("LLM narrative skipped: chat timed out"),
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        Report {
            title: "kronika digest".into(),
            generated_at_ns: 1,
            sections: vec![
                Section::Kpi {
                    label: "experiment_count".into(),
                    value: "6".into(),
                    delta: Some("+2".into()),
                },
                Section::Kpi {
                    label: "hypothesis_pass_rate".into(),
                    value: "0.875".into(),
                    delta: None,
                },
                Section::Table {
                    headers: vec!["experiment".into(), "value".into()],
                    rows: vec![vec!["pg-failover".into(), "4".into()]],
                },
            ],
        }
    }

    #[test]
    fn extract_numerals_finds_ints_decimals_signs_percents() {
        let ns = extract_numerals("pass rate was 87.5% across 6 runs, delta +2.1 and -3");
        assert_eq!(
            ns,
            vec![
                Numeral {
                    value: 87.5,
                    percent: true
                },
                Numeral {
                    value: 6.0,
                    percent: false
                },
                Numeral {
                    value: 2.1,
                    percent: false
                },
                Numeral {
                    value: -3.0,
                    percent: false
                },
            ]
        );
    }

    #[test]
    fn extract_numerals_skips_version_tails_and_words() {
        // `0.3.0` yields the leading 0.3 only; words carry no numerals.
        assert_eq!(
            extract_numerals("v0.3.0 shipped"),
            vec![Numeral {
                value: 0.3,
                percent: false
            }]
        );
        assert!(extract_numerals("no numbers here").is_empty());
    }

    #[test]
    fn sentences_split_on_terminal_punctuation_only() {
        assert_eq!(
            sentences("Rate was 0.875. It held steady! Really? yes"),
            vec!["Rate was 0.875.", "It held steady!", "Really?", "yes"]
        );
        // A decimal point mid-sentence never splits.
        assert_eq!(sentences("value 0.875 today"), vec!["value 0.875 today"]);
    }

    #[test]
    fn grounding_keeps_grounded_and_drops_invented_numbers() {
        let r = report();
        // 6 is a KPI value; one invented figure dooms only its own sentence.
        let g = ground_narrative("6 experiments ran. 42 targets were covered. No change.", &r);
        assert_eq!(g, Some("6 experiments ran. No change.".to_string()));
    }

    #[test]
    fn grounding_matches_percent_in_either_form() {
        let r = report(); // pass rate fact: 0.875
        assert_eq!(
            ground_narrative("Pass rate was 87.5%.", &r),
            Some("Pass rate was 87.5%.".to_string())
        );
        assert_eq!(
            ground_narrative("Pass rate was 0.875.", &r),
            Some("Pass rate was 0.875.".to_string())
        );
        // 62.5% is neither 0.625 nor 62.5 among the facts → dropped.
        assert_eq!(ground_narrative("Pass rate was 62.5%.", &r), None);
    }

    #[test]
    fn grounding_tolerates_llm_rounding() {
        let r = report(); // 0.875
        assert_eq!(
            ground_narrative("Pass rate held at 0.88.", &r),
            Some("Pass rate held at 0.88.".to_string())
        );
    }

    #[test]
    fn grounding_uses_table_cells_and_deltas() {
        let r = report();
        assert_eq!(
            ground_narrative("pg-failover scored 4, up +2 overall.", &r),
            Some("pg-failover scored 4, up +2 overall.".to_string())
        );
    }

    #[test]
    fn grounding_returns_none_for_empty_or_fully_ungrounded_text() {
        let r = report();
        assert_eq!(ground_narrative("", &r), None);
        assert_eq!(ground_narrative("   ", &r), None);
        assert_eq!(ground_narrative("999 experiments ran.", &r), None);
    }

    #[test]
    fn facts_package_contains_every_citable_number() {
        let r = report();
        let facts = facts_package(&r);
        assert_eq!(facts["kpis"][0]["value"], "6");
        assert_eq!(facts["kpis"][1]["value"], "0.875");
        assert_eq!(facts["tables"][0]["rows"][0][1], "4");
    }

    #[test]
    fn narrative_messages_carry_rules_then_facts() {
        let r = report();
        let messages = narrative_messages(&r);
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, Role::System));
        assert!(messages[0].content.contains("use ONLY numbers"));
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1].content.contains("kronika digest"));
        assert!(messages[1].content.contains("0.875"));
    }

    // ── narrate pipeline with a stub LLM ───────────────────────

    enum StubReply {
        Text(&'static str),
        Fail,
        Hang,
    }

    struct StubLlm(StubReply);

    // `Llm` is an `#[async_trait]` trait; implement its desugared signature
    // directly so the test needs no extra dev-dependency.
    impl Llm for StubLlm {
        fn chat<'life0, 'life1, 'async_trait>(
            &'life0 self,
            _messages: &'life1 [Message],
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<String, tumult_intelligence::llm::AiError>>
                    + Send
                    + 'async_trait,
            >,
        >
        where
            'life0: 'async_trait,
            'life1: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                match self.0 {
                    StubReply::Text(text) => Ok(text.to_string()),
                    StubReply::Fail => Err(tumult_intelligence::llm::AiError::Config(
                        "stub failure".into(),
                    )),
                    StubReply::Hang => {
                        std::future::pending::<Result<String, tumult_intelligence::llm::AiError>>()
                            .await
                    }
                }
            })
        }
    }

    #[tokio::test]
    async fn narrate_prepends_a_grounded_reply() {
        let llm: Arc<dyn Llm> = Arc::new(StubLlm(StubReply::Text("6 experiments ran.")));
        let out = narrate(&llm, report(), Duration::from_secs(5)).await;
        assert_eq!(out.sections.len(), report().sections.len() + 1);
        match &out.sections[0] {
            Section::Narrative { text } => assert_eq!(text, "6 experiments ran."),
            other => panic!("expected narrative section, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn narrate_drops_a_fully_ungrounded_reply() {
        let llm: Arc<dyn Llm> = Arc::new(StubLlm(StubReply::Text("999 experiments ran.")));
        let out = narrate(&llm, report(), Duration::from_secs(5)).await;
        assert_eq!(out, report());
    }

    #[tokio::test]
    async fn narrate_leaves_the_report_unchanged_on_llm_error() {
        let llm: Arc<dyn Llm> = Arc::new(StubLlm(StubReply::Fail));
        let out = narrate(&llm, report(), Duration::from_secs(5)).await;
        assert_eq!(out, report());
    }

    #[tokio::test]
    async fn narrate_leaves_the_report_unchanged_on_timeout() {
        let llm: Arc<dyn Llm> = Arc::new(StubLlm(StubReply::Hang));
        let out = narrate(&llm, report(), Duration::from_millis(50)).await;
        assert_eq!(out, report());
    }
}
