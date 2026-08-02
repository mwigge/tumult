//! Behavior tests for projecting run results into metadata-only journal
//! evidence and writing that evidence to disk.

use tumult_agentic::journal::{
    encode_result, metadata_evidence_from_result, metadata_evidence_from_trajectory,
    write_metadata_journal_file,
};
use tumult_agentic::model::{AgenticError, AgenticRunResult, ContractOutcome, FaultApplication};
use tumult_agentic::scoring::agentic_score;
use tumult_agentic::smoke::InjectedStepFault;
use tumult_agentic::trajectory::{StepOutcome, TrajectoryRunResult};

fn contract(contract_type: &str, scenario: &str, passed: bool) -> ContractOutcome {
    ContractOutcome {
        contract_type: contract_type.to_string(),
        scenario: scenario.to_string(),
        passed,
        reason: (!passed).then(|| "contract breached".to_string()),
        severity: 1.0,
    }
}

fn run_result(trace_id: Option<String>) -> AgenticRunResult {
    AgenticRunResult {
        target_type: "http".to_string(),
        scenarios: vec!["latency-spike".to_string(), "tool-timeout".to_string()],
        faults: vec![FaultApplication {
            fault_type: "latency".to_string(),
            scenario: "latency-spike".to_string(),
            applied: true,
            started_at_ns: 1,
            ended_at_ns: 2,
        }],
        contracts: vec![
            contract("fallback_used", "tool-timeout", true),
            contract("latency_budget", "latency-spike", false),
        ],
        resilience_score: 55.0,
        trace_id,
        replay_id: None,
    }
}

#[test]
fn evidence_uses_the_run_trace_id_when_present() {
    let evidence = metadata_evidence_from_result(
        "exp-1",
        "run-1",
        &run_result(Some("4bf92f3577b34da6a3ce929d0e0e4736".to_string())),
    );

    assert_eq!(evidence.trace.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(evidence.trace.span_id, "span-agentic-local");
    assert_eq!(evidence.trace.parent_span_id, None);
}

#[test]
fn evidence_falls_back_to_a_run_scoped_trace_id() {
    let evidence = metadata_evidence_from_result("exp-1", "run-42", &run_result(None));

    assert_eq!(evidence.trace.trace_id, "trace-run-42");
}

#[test]
fn evidence_projects_scenarios_faults_and_contract_pass_rate() {
    let evidence = metadata_evidence_from_result("exp-1", "run-1", &run_result(None));

    assert_eq!(evidence.experiment_id, "exp-1");
    assert_eq!(evidence.capture_policy, "metadata_only");
    assert_eq!(evidence.scenarios.len(), 2);
    assert!(
        evidence.scenarios[0]
            .input_sha256
            .starts_with("metadata-only-"),
        "scenario inputs must be hashed, not raw: {}",
        evidence.scenarios[0].input_sha256
    );
    assert!(!evidence.scenarios[0].input_sha256.contains("latency-spike"));
    assert_eq!(evidence.faults.len(), 1);
    assert_eq!(evidence.faults[0].fault_type, "latency");
    assert!(evidence.faults[0].applied);
    assert_eq!(evidence.faults[0].latency_ms, None);
    // One of two contracts passed.
    assert!((evidence.contract_pass_rate - 0.5).abs() < f64::EPSILON);
    assert!((evidence.resilience_score - 55.0).abs() < f64::EPSILON);
    assert!(evidence.tool_calls.is_empty());
}

#[test]
fn evidence_treats_a_contract_free_run_as_fully_passing() {
    let mut result = run_result(None);
    result.contracts.clear();

    let evidence = metadata_evidence_from_result("exp-1", "run-1", &result);

    assert!((evidence.contract_pass_rate - 1.0).abs() < f64::EPSILON);
}

#[test]
fn run_result_encodes_to_toon() {
    let encoded = encode_result(&run_result(None)).expect("run result encodes");

    assert!(encoded.contains("latency-spike"));
    assert!(encoded.contains("resilience_score"));
}

#[test]
fn writing_a_journal_creates_parent_dirs_and_reports_counts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("journal.toon");
    let evidence = metadata_evidence_from_result("exp-1", "run-7", &run_result(None));

    let summary = write_metadata_journal_file(&path, &evidence).expect("journal writes");

    assert_eq!(summary.path, path.display().to_string());
    assert_eq!(summary.run_id, "run-7");
    assert_eq!(summary.trace_id, "trace-run-7");
    assert_eq!(summary.scenario_count, 2);
    assert_eq!(summary.fault_count, 1);
    assert_eq!(summary.contract_count, 2);
    let written = std::fs::read_to_string(&path).expect("journal file exists");
    assert!(written.contains("metadata_only"));
    assert!(written.contains("trace-run-7"));
}

#[test]
fn writing_a_journal_under_a_file_fails_with_a_journal_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "not a directory").expect("write blocker");
    let evidence = metadata_evidence_from_result("exp-1", "run-1", &run_result(None));

    let error = write_metadata_journal_file(&blocker.join("journal.toon"), &evidence)
        .expect_err("a file as parent must fail");

    assert!(
        matches!(error, AgenticError::Journal(_)),
        "unexpected error: {error:?}"
    );
}

fn step(index: usize, label: &str, contracts: Vec<ContractOutcome>) -> StepOutcome {
    StepOutcome {
        index,
        label: label.to_string(),
        kind: "reasoning",
        injected_fault: None,
        healthy: contracts.iter().all(|c| c.passed),
        signature: format!("{label}|tool|body"),
        retry_count: 0,
        tool_calls: 1,
        contracts,
    }
}

#[test]
fn trajectory_evidence_flattens_step_and_trajectory_contracts() {
    let result = TrajectoryRunResult {
        pack: "multi-turn-drift".to_string(),
        steps: vec![
            step(0, "gather-context", Vec::new()),
            step(
                1,
                "answer",
                vec![contract("grounded_answer", "answer", false)],
            ),
        ],
        trajectory_contracts: vec![contract("no_drift", "multi-turn-drift", true)],
        score: agentic_score(
            &[contract("grounded_answer", "answer", false)],
            &[contract("no_drift", "multi-turn-drift", true)],
        ),
    };
    let injected = vec![
        InjectedStepFault {
            step_index: 1,
            fault_type: "context_truncation".to_string(),
        },
        InjectedStepFault {
            step_index: 99,
            fault_type: "latency".to_string(),
        },
    ];

    let evidence = metadata_evidence_from_trajectory("exp-t", "run-t", &result, &injected);

    // Both steps become journaled scenarios.
    assert_eq!(evidence.scenarios.len(), 2);
    assert_eq!(evidence.scenarios[1].name, "answer");
    // Per-step plus trajectory-level contracts are flattened together.
    assert_eq!(evidence.contracts.len(), 2);
    assert!((evidence.contract_pass_rate - 0.5).abs() < f64::EPSILON);
    // A valid step index resolves to the step label; an out-of-range index
    // falls back to the pack name so the fault stays attributable.
    assert_eq!(evidence.faults[0].scenario, "answer");
    assert_eq!(evidence.faults[1].scenario, "multi-turn-drift");
    assert!(evidence.faults.iter().all(|f| f.applied));
    assert_eq!(evidence.trace.trace_id, "trace-run-t");
    assert_eq!(evidence.trace.span_id, "span-agentic-trajectory");
    assert!((evidence.resilience_score - result.score.overall).abs() < f64::EPSILON);
}

#[test]
fn trajectory_evidence_with_no_contracts_passes_fully() {
    let result = TrajectoryRunResult {
        pack: "empty-pack".to_string(),
        steps: Vec::new(),
        trajectory_contracts: Vec::new(),
        score: agentic_score(&[], &[]),
    };

    let evidence = metadata_evidence_from_trajectory("exp-t", "run-t", &result, &[]);

    assert!((evidence.contract_pass_rate - 1.0).abs() < f64::EPSILON);
    assert!(evidence.scenarios.is_empty());
    assert!(evidence.faults.is_empty());
}
