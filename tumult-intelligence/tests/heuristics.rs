//! Behavioral tests for the heuristic report and recommendation pipeline,
//! driven by synthetic experiment journals in a real (temp-file) analytics
//! store.
//!
//! Plugin discovery scans ambient paths (`./plugins`, `~/.tumult/plugins`),
//! so these tests only assert on store-derived report content — coverage
//! wording, failure ranking, and staleness ordering — never on the exact
//! plugin catalog. Catalog-sensitive coverage math is exercised hermetically
//! in `plugin_coverage.rs`.

use std::path::{Path, PathBuf};

use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, ExperimentStatus, Journal, SpanId, TraceId,
};
use tumult_intelligence::{heuristic_report, recommend, OutputFormat, RecommendOptions};
use tumult_lake::AnalyticsStore;

const BASE_NS: i64 = 1_700_000_000_000_000_000;
const HOUR_NS: i64 = 3_600_000_000_000;

/// Build a synthetic journal with one succeeded action per name.
fn journal(
    id: &str,
    title: &str,
    status: ExperimentStatus,
    started_at_ns: i64,
    actions: &[&str],
) -> Journal {
    Journal {
        experiment_title: title.into(),
        experiment_id: id.into(),
        status,
        started_at_ns,
        ended_at_ns: started_at_ns + 60_000_000_000,
        duration_ms: 60_000,
        steady_state_before: None,
        steady_state_after: None,
        method_results: actions
            .iter()
            .map(|name| ActivityResult {
                name: (*name).into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns,
                duration_ms: 10,
                output: None,
                error: None,
                trace_id: TraceId("00000000000000000000000000000000".into()),
                span_id: SpanId("0000000000000000".into()),
            })
            .collect(),
        rollback_results: vec![],
        rollback_failures: 0,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: None,
        regulatory: None,
        halt: None,
        blast_radius: None,
    }
}

/// Create a store at `dir/analytics.duckdb`, ingest `journals`, and close it
/// so the report can reopen the file.
fn build_store(dir: &Path, journals: &[Journal]) -> PathBuf {
    let path = dir.join("analytics.duckdb");
    let store = AnalyticsStore::open(&path).expect("create store");
    let ingested = store.ingest_journals(journals).expect("ingest journals");
    assert_eq!(
        ingested,
        journals.len(),
        "all synthetic journals must ingest"
    );
    drop(store);
    path
}

fn position(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("expected {needle:?} in report:\n{haystack}"))
}

// ── heuristic_report ──────────────────────────────────────────

#[test]
fn empty_history_reports_coverage_but_no_failure_or_staleness_sections() {
    let dir = tempfile::TempDir::new().unwrap();
    let store_path = build_store(dir.path(), &[]);

    let report = heuristic_report(&store_path);

    assert!(
        report.contains("=== Recommendations ==="),
        "report: {report}"
    );
    assert!(
        report.contains("Coverage: "),
        "existing store must report coverage: {report}"
    );
    assert!(
        !report.contains("Most failing experiments:"),
        "no runs means no failure ranking: {report}"
    );
    assert!(
        !report.contains("Oldest experiments:"),
        "no runs means no staleness list: {report}"
    );
}

#[test]
fn single_completed_run_is_listed_as_oldest_but_not_failing() {
    let dir = tempfile::TempDir::new().unwrap();
    let store_path = build_store(
        dir.path(),
        &[journal(
            "exp-1",
            "db failover drill",
            ExperimentStatus::Completed,
            BASE_NS,
            &["kill-process"],
        )],
    );

    let report = heuristic_report(&store_path);

    assert!(report.contains("Oldest experiments:"), "report: {report}");
    assert!(report.contains("  - db failover drill"), "report: {report}");
    assert!(
        !report.contains("Most failing experiments:"),
        "completed runs must not rank as failures: {report}"
    );
}

#[test]
fn failing_experiments_are_ranked_by_failure_count_and_exclude_successes() {
    let dir = tempfile::TempDir::new().unwrap();
    let store_path = build_store(
        dir.path(),
        &[
            journal("f-1", "flaky", ExperimentStatus::Failed, BASE_NS, &[]),
            journal(
                "f-2",
                "flaky",
                ExperimentStatus::Failed,
                BASE_NS + HOUR_NS,
                &[],
            ),
            journal(
                "f-3",
                "flaky",
                ExperimentStatus::Aborted,
                BASE_NS + 2 * HOUR_NS,
                &[],
            ),
            journal("s-1", "sometimes", ExperimentStatus::Deviated, BASE_NS, &[]),
            journal("ok-1", "solid", ExperimentStatus::Completed, BASE_NS, &[]),
        ],
    );

    let report = heuristic_report(&store_path);

    let section_start = position(&report, "Most failing experiments:");
    let failing_section = &report[section_start..];
    assert!(
        failing_section.contains("flaky (3 failures)"),
        "report: {report}"
    );
    assert!(
        failing_section.contains("sometimes (1 failures)"),
        "report: {report}"
    );
    assert!(
        position(failing_section, "flaky") < position(failing_section, "sometimes"),
        "higher failure counts must rank first: {report}"
    );
    // "solid" only ever completed — it must not appear as a failing experiment.
    let staleness_start = position(&report, "Oldest experiments:");
    assert!(
        !report[section_start..staleness_start].contains("solid"),
        "completed-only experiments must not be listed as failing: {report}"
    );
}

#[test]
fn oldest_experiments_are_ordered_by_most_recent_run_per_title() {
    let dir = tempfile::TempDir::new().unwrap();
    let store_path = build_store(
        dir.path(),
        &[
            journal(
                "a-1",
                "ancient",
                ExperimentStatus::Completed,
                BASE_NS + HOUR_NS,
                &[],
            ),
            journal(
                "m-1",
                "middle",
                ExperimentStatus::Completed,
                BASE_NS + 2 * HOUR_NS,
                &[],
            ),
            // "refreshed" ran before "ancient" but again afterwards — the
            // per-title max(started_at_ns) decides staleness, so it is newest.
            journal(
                "r-1",
                "refreshed",
                ExperimentStatus::Completed,
                BASE_NS,
                &[],
            ),
            journal(
                "r-2",
                "refreshed",
                ExperimentStatus::Completed,
                BASE_NS + 3 * HOUR_NS,
                &[],
            ),
        ],
    );

    let report = heuristic_report(&store_path);

    let section = &report[position(&report, "Oldest experiments:")..];
    let ancient = position(section, "- ancient");
    let middle = position(section, "- middle");
    let refreshed = position(section, "- refreshed");
    assert!(
        ancient < middle && middle < refreshed,
        "staleness must order by last run ascending: {report}"
    );
}

#[test]
fn missing_store_suggests_running_experiments() {
    let report = heuristic_report(Path::new("/definitely/not/a/store.duckdb"));

    assert!(
        report.contains("No analytics store found"),
        "report: {report}"
    );
    assert!(
        report.contains("Run experiments to build history"),
        "report: {report}"
    );
    assert!(report.contains("Available actions:"), "report: {report}");
}

#[test]
fn corrupt_store_file_degrades_to_message_not_panic() {
    let dir = tempfile::TempDir::new().unwrap();
    let store_path = dir.path().join("analytics.duckdb");
    std::fs::write(&store_path, b"this is not a duckdb file").unwrap();

    let report = heuristic_report(&store_path);

    assert!(
        report.contains("Analytics store could not be opened."),
        "report: {report}"
    );
}

// ── recommend ─────────────────────────────────────────────────

#[test]
fn recommend_text_falls_back_to_heuristics_when_store_is_missing() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut options = RecommendOptions::new(dir.path().join("missing.duckdb"));
    options.goal = Some("harden checkout flow".to_string());

    let text = recommend(&options).expect("recommend renders");

    assert!(
        text.contains("=== AI-Powered Tumult Recommendations ==="),
        "text: {text}"
    );
    assert!(text.contains("Source: heuristic-fallback"), "text: {text}");
    assert!(text.contains("Goal: harden checkout flow"), "text: {text}");
    assert!(
        text.contains("1. Close the largest untested action coverage gaps"),
        "fallback must still produce an actionable recommendation: {text}"
    );
    assert!(text.contains("Preconditions:"), "text: {text}");
    assert!(text.contains("Notes:"), "text: {text}");
}

#[test]
fn recommend_json_exposes_full_output_shape() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut options = RecommendOptions::new(dir.path().join("missing.duckdb"));
    options.goal = Some("cover database failover".to_string());
    options.format = OutputFormat::Json;

    let rendered = recommend(&options).expect("recommend renders");
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");

    assert_eq!(value["source"], "heuristic-fallback");
    assert_eq!(value["goal"], "cover database failover");
    assert_eq!(value["recommendations"][0]["rank"], 1);
    assert!(value["recommendations"][0]["preconditions"]
        .as_array()
        .is_some_and(|p| !p.is_empty()));
    assert!(value["draft_toon"].is_null());
    assert!(value["draft_valid"].is_null());
    assert!(value["heuristic_context"]
        .as_str()
        .is_some_and(|c| c.contains("No analytics store found")));
}

#[test]
fn recommend_json_embeds_history_signals_from_populated_store() {
    let dir = tempfile::TempDir::new().unwrap();
    let store_path = build_store(
        dir.path(),
        &[
            journal(
                "f-1",
                "flaky checkout",
                ExperimentStatus::Failed,
                BASE_NS,
                &[],
            ),
            journal(
                "f-2",
                "flaky checkout",
                ExperimentStatus::Failed,
                BASE_NS + HOUR_NS,
                &[],
            ),
            journal(
                "ok-1",
                "solid cache",
                ExperimentStatus::Completed,
                BASE_NS,
                &[],
            ),
        ],
    );
    let mut options = RecommendOptions::new(store_path);
    options.format = OutputFormat::Json;

    let rendered = recommend(&options).expect("recommend renders");
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");

    let context = value["heuristic_context"].as_str().expect("context string");
    assert!(
        context.contains("Most failing experiments:"),
        "context: {context}"
    );
    assert!(
        context.contains("flaky checkout (2 failures)"),
        "context: {context}"
    );
    assert!(
        context.contains("Oldest experiments:"),
        "context: {context}"
    );
}
