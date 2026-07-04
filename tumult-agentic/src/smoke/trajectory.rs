//! Deterministic smoke runner for the bundled multi-turn trajectory packs.
//!
//! Mirrors [`super::packs`] for single-call packs: it runs a bundled
//! [`TrajectoryPack`] through the real [`execute_trajectory`] engine against
//! in-process metadata baselines (no network), then surfaces the pack's headline
//! trajectory-contract outcome alongside the full step/contract/score evidence.

use crate::model::AgenticError;
use crate::trajectory::{bundled_trajectory_packs, execute_trajectory, TrajectoryRunResult};

/// Fixed seed so local trajectory-pack runs are reproducible.
const TRAJECTORY_PACK_SEED: u64 = 0x7a7a;

/// A single injected fault, projected for reporting: which step it hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedStepFault {
    pub step_index: usize,
    pub fault_type: String,
}

/// The result of running one bundled trajectory pack.
#[derive(Debug, Clone, PartialEq)]
pub struct TrajectorySmokeReport {
    pub pack: String,
    pub adapter: String,
    pub description: String,
    pub headline_contract: String,
    pub expected: String,
    pub actual: String,
    pub passed: bool,
    pub injected: Vec<InjectedStepFault>,
    pub next_diagnostic_command: String,
    pub result: TrajectoryRunResult,
}

/// Run one bundled multi-turn trajectory pack through the real engine.
///
/// # Errors
///
/// Returns [`AgenticError::InvalidConfig`] when the pack is unknown, or
/// propagates [`AgenticError`] from fault application.
pub fn run_trajectory_pack_smoke(pack_name: &str) -> Result<TrajectorySmokeReport, AgenticError> {
    let pack = bundled_trajectory_packs()
        .into_iter()
        .find(|pack| pack.name == pack_name)
        .ok_or_else(|| {
            AgenticError::InvalidConfig(format!("unknown trajectory pack: {pack_name}"))
        })?;

    let result = execute_trajectory(
        pack.name,
        &pack.steps,
        &pack.faults,
        &pack.contracts,
        TRAJECTORY_PACK_SEED,
    )?;

    let actual = result
        .trajectory_contracts
        .iter()
        .find(|outcome| outcome.contract_type == pack.headline_contract)
        .map_or_else(
            || "trajectory_contract_missing".to_string(),
            |outcome| {
                if outcome.passed {
                    "trajectory_contract_passed".to_string()
                } else {
                    format!(
                        "trajectory_contract_failed:{}",
                        outcome.reason.as_deref().unwrap_or("unknown")
                    )
                }
            },
        );

    let injected = pack
        .faults
        .iter()
        .map(|fault| InjectedStepFault {
            step_index: fault.step_index,
            fault_type: fault.fault.fault_type().to_string(),
        })
        .collect();

    Ok(TrajectorySmokeReport {
        pack: pack.name.to_string(),
        adapter: "fake_trajectory".to_string(),
        description: pack.description.to_string(),
        headline_contract: pack.headline_contract.to_string(),
        expected: pack.headline_expected.to_string(),
        passed: actual == pack.headline_expected,
        actual,
        injected,
        next_diagnostic_command: format!(
            "cargo test -p tumult-agentic trajectory -- --nocapture # pack={}",
            pack.name
        ),
        result,
    })
}
