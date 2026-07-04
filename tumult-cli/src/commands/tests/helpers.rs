//! Shared test fixtures used across the CLI command test submodules.

use super::super::*;
use tumult_core::types::{Activity, ActivityType};

// ── Helper: write a valid experiment file ─────────────────

pub(crate) fn write_valid_experiment(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("test-experiment.toon");
    let exp = Experiment {
        version: "v1".into(),
        title: "CLI test experiment".into(),
        description: Some("Tests CLI command execution".into()),
        tags: vec!["test".into()],
        configuration: indexmap::IndexMap::new(),
        secrets: indexmap::IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![Activity {
            name: "echo-action".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
    };
    let toon = toon_format::encode_default(&exp).unwrap();
    std::fs::write(&path, toon).unwrap();
    path
}

pub(crate) fn write_invalid_experiment(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("invalid.toon");
    std::fs::write(&path, "this is not valid toon {{{").unwrap();
    path
}

/// Writes an experiment that still uses the removed `http` provider, which
/// must now fail TOON parsing with a serde unknown-variant error.
pub(crate) fn write_http_provider_experiment(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("http-provider.toon");
    let toon = "title: Experiment using the removed http provider

method[1]:
  - name: http-probe
    activity_type: probe
    provider:
      type: http
      method: GET
      url: http://localhost:8080/health
";
    std::fs::write(&path, toon).unwrap();
    path
}

pub(crate) fn write_empty_method_experiment(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("empty-method.toon");
    let exp = Experiment {
        version: "v1".into(),
        title: "Empty method experiment".into(),
        description: None,
        tags: vec![],
        configuration: indexmap::IndexMap::new(),
        secrets: indexmap::IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
        guards: vec![],
        blast_radius: None,
        max_concurrent_faults: None,
    };
    let toon = toon_format::encode_default(&exp).unwrap();
    std::fs::write(&path, toon).unwrap();
    path
}

/// Journal with one succeeded + one failed method activity, both carrying a
/// non-empty `trace_id`; the failed one carries an error message.
pub(crate) fn journal_with_failure() -> tumult_core::types::Journal {
    use tumult_core::types::*;
    Journal {
        experiment_title: "Failure & <recovery>".into(),
        experiment_id: "exp-fail-1".into(),
        status: ExperimentStatus::Deviated,
        started_at_ns: 1_774_980_000_000_000_000,
        ended_at_ns: 1_774_980_060_000_000_000,
        duration_ms: 60_000,
        method_results: vec![
            ActivityResult {
                name: "ok-step".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1_774_980_000_000_000_000,
                duration_ms: 100,
                output: Some("fine".into()),
                error: None,
                trace_id: TraceId("aabbccddeeff00112233445566778899".into()),
                span_id: SpanId::empty(),
            },
            ActivityResult {
                name: "bad-step".into(),
                activity_type: ActivityType::Probe,
                status: ActivityStatus::Failed,
                started_at_ns: 1_774_980_001_000_000_000,
                duration_ms: 250,
                output: None,
                error: Some("connection refused on port 5432".into()),
                trace_id: TraceId("00112233445566778899aabbccddeeff".into()),
                span_id: SpanId::empty(),
            },
        ],
        steady_state_before: None,
        steady_state_after: None,
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

pub(crate) fn sample_k6_summary() -> serde_json::Value {
    serde_json::json!({
        "metrics": {
            "iterations": { "count": 1025, "rate": 51.006_998 },
            "iteration_duration": {
                "avg": 97.77, "min": 55.75, "med": 63.81, "max": 201.09,
                "p(90)": 67.34, "p(95)": 148.01, "p(99)": 180.0
            },
            "checks_total": { "count": 1025 },
            "checks_failed": { "count": 5 }
        }
    })
}
