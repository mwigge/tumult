use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::{AgenticError, AgenticRunResult};

/// Encode agentic run evidence as TOON.
///
/// # Errors
///
/// Returns [`AgenticError::Journal`] if encoding fails.
pub fn encode_result(result: &AgenticRunResult) -> Result<String, AgenticError> {
    toon_format::encode_default(result).map_err(|err| AgenticError::Journal(err.to_string()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgenticJournalEvidence {
    pub experiment_id: String,
    pub run_id: String,
    pub capture_policy: String,
    pub trace: JournalTraceCorrelation,
    pub scenarios: Vec<AgenticJournalScenario>,
    pub faults: Vec<AgenticJournalFault>,
    pub contracts: Vec<AgenticJournalContract>,
    pub tool_calls: Vec<AgenticJournalToolCall>,
    pub contract_pass_rate: f64,
    pub resilience_score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalTraceCorrelation {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgenticJournalScenario {
    pub name: String,
    pub input_sha256: String,
    pub expected_behavior_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgenticJournalFault {
    pub fault_type: String,
    pub scenario: String,
    pub applied: bool,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgenticJournalContract {
    pub contract_type: String,
    pub scenario: String,
    pub passed: bool,
    pub reason: Option<String>,
    pub severity: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgenticJournalToolCall {
    pub tool_name: String,
    pub operation: String,
    pub payload_sha256: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgenticJournalWriteSummary {
    pub path: String,
    pub run_id: String,
    pub trace_id: String,
    pub scenario_count: usize,
    pub fault_count: usize,
    pub contract_count: usize,
}

/// Encode metadata-only agentic evidence as TOON.
///
/// # Errors
///
/// Returns [`AgenticError::Journal`] if encoding fails.
pub fn encode_metadata_journal(evidence: &AgenticJournalEvidence) -> Result<String, AgenticError> {
    toon_format::encode_default(evidence).map_err(|err| AgenticError::Journal(err.to_string()))
}

#[must_use]
pub fn metadata_evidence_from_result(
    experiment_id: impl Into<String>,
    run_id: impl Into<String>,
    result: &AgenticRunResult,
) -> AgenticJournalEvidence {
    let run_id = run_id.into();
    let trace_id = result
        .trace_id
        .clone()
        .unwrap_or_else(|| format!("trace-{run_id}"));
    let passed = result
        .contracts
        .iter()
        .filter(|contract| contract.passed)
        .count();
    let contract_pass_rate = if result.contracts.is_empty() {
        1.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            passed as f64 / result.contracts.len() as f64
        }
    };

    AgenticJournalEvidence {
        experiment_id: experiment_id.into(),
        run_id,
        capture_policy: "metadata_only".to_string(),
        trace: JournalTraceCorrelation {
            trace_id,
            span_id: "span-agentic-local".to_string(),
            parent_span_id: None,
        },
        scenarios: result
            .scenarios
            .iter()
            .map(|name| AgenticJournalScenario {
                name: name.clone(),
                input_sha256: metadata_hash(name),
                expected_behavior_sha256: None,
            })
            .collect(),
        faults: result
            .faults
            .iter()
            .map(|fault| AgenticJournalFault {
                fault_type: fault.fault_type.clone(),
                scenario: fault.scenario.clone(),
                applied: fault.applied,
                latency_ms: None,
            })
            .collect(),
        contracts: result
            .contracts
            .iter()
            .map(|contract| AgenticJournalContract {
                contract_type: contract.contract_type.clone(),
                scenario: contract.scenario.clone(),
                passed: contract.passed,
                reason: contract.reason.clone(),
                severity: contract.severity,
            })
            .collect(),
        tool_calls: Vec::new(),
        contract_pass_rate,
        resilience_score: result.resilience_score,
    }
}

/// Write metadata-only agentic evidence to a TOON journal file.
///
/// # Errors
///
/// Returns [`AgenticError::Journal`] if the parent directory cannot be created,
/// the evidence cannot be encoded, or the journal cannot be written.
pub fn write_metadata_journal_file(
    path: &Path,
    evidence: &AgenticJournalEvidence,
) -> Result<AgenticJournalWriteSummary, AgenticError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| AgenticError::Journal(err.to_string()))?;
    }
    let content = encode_metadata_journal(evidence)?;
    std::fs::write(path, content).map_err(|err| AgenticError::Journal(err.to_string()))?;

    Ok(AgenticJournalWriteSummary {
        path: path.display().to_string(),
        run_id: evidence.run_id.clone(),
        trace_id: evidence.trace.trace_id.clone(),
        scenario_count: evidence.scenarios.len(),
        fault_count: evidence.faults.len(),
        contract_count: evidence.contracts.len(),
    })
}

#[must_use]
fn metadata_hash(value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("metadata-only-{hash:016x}")
}
