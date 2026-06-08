//! Deterministic agentic fault-execution engine.
//!
//! This is the single place that turns a declarative fault list into an
//! observed [`AgenticRunResult`]. It runs every fault through the seeded
//! [`FaultEngine`] gate and the [`apply_fault`] mutator against a baseline
//! response, then evaluates every contract against the resulting response.
//!
//! The scenario-pack smoke runner, the replay path, and the live proxy all
//! funnel through here so that "what the fault does" is defined exactly once.

use crate::adapters::AgentResponse;
use crate::contracts::{evaluate_contract, ContractSpec};
use crate::faults::{apply_fault, FaultEngine, FaultSpec, FaultTargetResponse};
use crate::model::{AgenticError, AgenticRunResult, ContractOutcome, FaultApplication};
use crate::scoring::resilience_score;

/// A fully observed run: the post-fault response plus the per-fault and
/// per-contract evidence derived from it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutedRun {
    pub response: FaultTargetResponse,
    pub result: AgenticRunResult,
}

/// Identifying context for a single fault-execution run.
#[derive(Debug, Clone)]
pub struct RunContext<'a> {
    /// Target adapter kind recorded on the result (`http`, `mcp`, `replay`).
    pub target_type: &'a str,
    /// Scenario label recorded on every fault and contract outcome.
    pub scenario: &'a str,
    /// Seed for the per-run [`FaultEngine`] gate, keeping decisions reproducible.
    pub seed: u64,
    /// Optional trace id propagated onto the result.
    pub trace_id: Option<String>,
    /// Optional replay id propagated onto the result.
    pub replay_id: Option<String>,
}

/// Run an ordered fault list against `baseline`, then evaluate `contracts`
/// against the resulting response.
///
/// Faults are applied in order: each fault is first gated by the seeded
/// [`FaultEngine`] (so probabilistic faults stay reproducible for a given
/// `RunContext::seed`), and applied faults mutate the response that later faults
/// and all contracts observe.
///
/// # Errors
///
/// Returns [`AgenticError`] if [`apply_fault`] rejects a fault parameter.
pub fn execute(
    context: &RunContext,
    baseline: FaultTargetResponse,
    faults: &[FaultSpec],
    contracts: &[ContractSpec],
) -> Result<ExecutedRun, AgenticError> {
    let mut engine = FaultEngine::new(context.seed);
    let mut response = baseline;
    let mut applications = Vec::with_capacity(faults.len());

    for (index, fault) in faults.iter().enumerate() {
        let applied = engine.should_apply(fault);
        if applied {
            response = apply_fault(fault, response)?.response;
        }
        let started_at_ns = i64::try_from(index).unwrap_or(i64::MAX);
        applications.push(FaultApplication {
            fault_type: fault.fault_type().to_string(),
            scenario: context.scenario.to_string(),
            applied,
            started_at_ns,
            ended_at_ns: started_at_ns.saturating_add(1),
        });
    }

    let observed = to_agent_response(&response);
    let outcomes: Vec<ContractOutcome> = contracts
        .iter()
        .map(|contract| evaluate_contract(context.scenario, contract, &observed))
        .collect();

    let result = AgenticRunResult {
        target_type: context.target_type.to_string(),
        scenarios: vec![context.scenario.to_string()],
        faults: applications,
        resilience_score: resilience_score(&outcomes),
        contracts: outcomes,
        trace_id: context.trace_id.clone(),
        replay_id: context.replay_id.clone(),
    };

    emit_experiment_telemetry(context, &result);
    Ok(ExecutedRun { response, result })
}

/// Emit tumult's own experiment-side observability (span tree + metrics) for a
/// completed run, via the canonical tumult-otel instrumentation. This is the
/// experiment side of the two-sided picture — independent of the target's own
/// telemetry and produced even for offline scenario-pack/replay runs.
fn emit_experiment_telemetry(context: &RunContext, result: &AgenticRunResult) {
    use tumult_otel::agentic_span::{
        record_agentic_run, AgenticRunTelemetry, ContractRecord, FaultRecord,
    };

    let faults: Vec<FaultRecord> = result
        .faults
        .iter()
        .map(|fault| FaultRecord {
            fault_type: fault.fault_type.clone(),
            applied: fault.applied,
        })
        .collect();
    let contracts: Vec<ContractRecord> = result
        .contracts
        .iter()
        .map(|contract| ContractRecord {
            contract_type: contract.contract_type.clone(),
            passed: contract.passed,
            reason: contract.reason.clone(),
            severity: contract.severity,
        })
        .collect();

    record_agentic_run(
        &AgenticRunTelemetry {
            scenario: context.scenario,
            target_type: context.target_type,
            client: None,
            resilience_score: result.resilience_score,
            faults: &faults,
            contracts: &contracts,
        },
        None,
    );
}

/// Project the fault-engine response onto the contract-evaluation view.
///
/// Contracts score observable agent behaviour (body, latency, retries, tool
/// calls, token usage, fallback), so the richer [`FaultTargetResponse`] fields
/// used only during injection (`tool_name`, `retrieved_documents`) are dropped.
#[must_use]
pub fn to_agent_response(response: &FaultTargetResponse) -> AgentResponse {
    AgentResponse {
        body: response.body.clone(),
        latency_ms: response.latency_ms,
        tool_calls: response.tool_calls,
        retry_count: response.retry_count,
        input_tokens: response.input_tokens,
        output_tokens: response.output_tokens,
        fallback_used: response.fallback_used,
    }
}
