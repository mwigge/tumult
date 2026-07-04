//! Multi-turn agent-graph fault modeling.
//!
//! The per-request [`crate::engine`] answers "what does one model/tool call do
//! under a fault?". This module answers the harder, agent-shaped question:
//! "what does a *trajectory* — an ordered sequence of model and tool calls — do
//! when a fault is injected at a specific step?".
//!
//! A [`Trajectory`] is a list of [`TrajectoryStep`]s. Each step carries a
//! baseline metadata response and its own per-step [`ContractSpec`]s. Faults are
//! attached to a specific step index via [`StepFault`], so a run can, e.g.,
//! poison retrieval at step 0 and observe the answer step at index 2 lose its
//! grounding. On top of the per-step contracts sit whole-trajectory contracts
//! ([`TrajectoryContractSpec`]) that only make sense across turns: does the
//! agent *recover* after a bad step, does it *loop*, does it *terminate*
//! healthy, does it stay within a *step budget*.
//!
//! Retrieval context propagates forward across steps: documents retrieved (or
//! poisoned) at an earlier step flow into any later step that
//! [`TrajectoryStep::consumes_retrieval`], so grounding failures cascade the way
//! they do in a real RAG agent instead of staying local to one call.

use std::collections::HashSet;

use crate::contracts::{evaluate_contract, ContractSpec};
use crate::engine::to_agent_response;
use crate::faults::{apply_fault, FaultEngine, FaultSpec, FaultTargetResponse};
use crate::model::{AgenticError, ContractOutcome};
use crate::scoring::{agentic_score, AgenticScore};

/// Whether a trajectory step is a model call or a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Model,
    Tool,
}

impl StepKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Tool => "tool",
        }
    }
}

/// One node in an agent trajectory: a single model or tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct TrajectoryStep {
    /// Stable label used in reports and loop-signature detection.
    pub label: String,
    pub kind: StepKind,
    /// When `true`, the step folds the running retrieval context into its answer,
    /// so poisoned/irrelevant context retrieved earlier degrades this step.
    pub consumes_retrieval: bool,
    /// Pre-fault metadata response for the step.
    pub baseline: FaultTargetResponse,
    /// Contracts evaluated against this step's post-fault response.
    pub contracts: Vec<ContractSpec>,
}

/// A fault attached to a specific step of a trajectory.
#[derive(Debug, Clone, PartialEq)]
pub struct StepFault {
    pub step_index: usize,
    pub fault: FaultSpec,
}

/// A whole-trajectory contract: an assertion over the sequence of step outcomes
/// that cannot be expressed against any single call.
#[derive(Debug, Clone, PartialEq)]
pub enum TrajectoryContractSpec {
    /// After the first unhealthy step, a healthy step must occur within
    /// `max_steps` turns (the agent recovers rather than staying broken).
    RecoversWithin {
        max_steps: usize,
        severity: Option<f64>,
    },
    /// No two steps may share the same signature (label + tool + body): a repeat
    /// is a loop / infinite-reflection signal.
    NoRepeatedStep { severity: Option<f64> },
    /// The final step must be healthy — the agent terminates in a good state.
    TerminatesHealthy { severity: Option<f64> },
    /// The trajectory must not exceed `max_steps` total steps.
    StepBudget {
        max_steps: usize,
        severity: Option<f64>,
    },
}

impl TrajectoryContractSpec {
    #[must_use]
    pub fn contract_type(&self) -> &'static str {
        match self {
            Self::RecoversWithin { .. } => "recovers_within",
            Self::NoRepeatedStep { .. } => "no_repeated_step",
            Self::TerminatesHealthy { .. } => "terminates_healthy",
            Self::StepBudget { .. } => "step_budget",
        }
    }

    #[must_use]
    fn severity(&self) -> f64 {
        match self {
            Self::RecoversWithin { severity, .. }
            | Self::NoRepeatedStep { severity }
            | Self::TerminatesHealthy { severity }
            | Self::StepBudget { severity, .. } => severity.unwrap_or(1.0),
        }
    }
}

/// The observed outcome of one executed trajectory step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepOutcome {
    pub index: usize,
    pub label: String,
    pub kind: &'static str,
    /// Fault type injected at this step, if any actually applied.
    pub injected_fault: Option<String>,
    /// `true` when every per-step contract passed.
    pub healthy: bool,
    /// Loop-detection signature: `label|tool|body`.
    pub signature: String,
    pub retry_count: u32,
    pub tool_calls: u32,
    pub contracts: Vec<ContractOutcome>,
}

/// A fully observed trajectory run: per-step outcomes, trajectory-level contract
/// outcomes, and the rolled-up agentic resilience subscores.
#[derive(Debug, Clone, PartialEq)]
pub struct TrajectoryRunResult {
    pub pack: String,
    pub steps: Vec<StepOutcome>,
    pub trajectory_contracts: Vec<ContractOutcome>,
    pub score: AgenticScore,
}

/// Execute an ordered trajectory, injecting faults at their target steps.
///
/// Each step starts from its baseline response; any [`StepFault`] targeting the
/// step is gated by the seeded [`FaultEngine`] and applied in order. Retrieval
/// context accumulates across steps and contaminates any later
/// retrieval-consuming step. Per-step contracts are evaluated against each
/// step's post-fault response, then the trajectory contracts are evaluated over
/// the full sequence and the agentic subscores are computed.
///
/// # Errors
///
/// Returns [`AgenticError`] if [`apply_fault`] rejects a fault parameter.
pub fn execute_trajectory(
    pack_name: &str,
    steps: &[TrajectoryStep],
    faults: &[StepFault],
    contracts: &[TrajectoryContractSpec],
    seed: u64,
) -> Result<TrajectoryRunResult, AgenticError> {
    let mut engine = FaultEngine::new(seed);
    let mut carried_docs: Vec<String> = Vec::new();
    let mut step_outcomes = Vec::with_capacity(steps.len());
    let mut all_step_contracts: Vec<ContractOutcome> = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        let mut response = step.baseline.clone();
        let mut injected_fault = None;
        for step_fault in faults.iter().filter(|fault| fault.step_index == index) {
            if engine.should_apply(&step_fault.fault) {
                response = apply_fault(&step_fault.fault, response)?.response;
                injected_fault = Some(step_fault.fault.fault_type().to_string());
            }
        }

        // Contribute this step's retrieved documents to the running context.
        if !response.retrieved_documents.is_empty() {
            carried_docs.extend(response.retrieved_documents.iter().cloned());
        }
        // A retrieval-consuming step inherits the running context; poisoned
        // context degrades its answer so grounding contracts observe the cascade.
        if step.consumes_retrieval {
            response.retrieved_documents.clone_from(&carried_docs);
            if carried_docs.iter().any(|doc| doc.contains("poisoned")) {
                let joined = carried_docs.join(" ");
                response.body = format!(
                    r#"{{"answer":"ungrounded answer built from poisoned context: {joined}"}}"#
                );
            }
        }

        let observed = to_agent_response(&response);
        let outcomes: Vec<ContractOutcome> = step
            .contracts
            .iter()
            .map(|contract| evaluate_contract(&step.label, contract, &observed))
            .collect();
        let healthy = outcomes.iter().all(|outcome| outcome.passed);
        let signature = format!(
            "{}|{}|{}",
            step.label,
            response.tool_name.as_deref().unwrap_or(""),
            response.body
        );
        all_step_contracts.extend(outcomes.iter().cloned());

        step_outcomes.push(StepOutcome {
            index,
            label: step.label.clone(),
            kind: step.kind.as_str(),
            injected_fault,
            healthy,
            signature,
            retry_count: response.retry_count,
            tool_calls: response.tool_calls,
            contracts: outcomes,
        });
    }

    let trajectory_contracts: Vec<ContractOutcome> = contracts
        .iter()
        .map(|contract| evaluate_trajectory_contract(pack_name, contract, &step_outcomes))
        .collect();

    let score = agentic_score(&all_step_contracts, &trajectory_contracts);

    Ok(TrajectoryRunResult {
        pack: pack_name.to_string(),
        steps: step_outcomes,
        trajectory_contracts,
        score,
    })
}

/// Evaluate a single whole-trajectory contract against the observed steps.
#[must_use]
pub fn evaluate_trajectory_contract(
    pack_name: &str,
    contract: &TrajectoryContractSpec,
    steps: &[StepOutcome],
) -> ContractOutcome {
    let severity = contract.severity();
    let (passed, reason) = match contract {
        TrajectoryContractSpec::TerminatesHealthy { .. } => {
            let ok = steps.last().is_some_and(|step| step.healthy);
            (ok, reason_if(ok, "final_step_unhealthy"))
        }
        TrajectoryContractSpec::RecoversWithin { max_steps, .. } => {
            let ok = match steps.iter().position(|step| !step.healthy) {
                None => true,
                Some(first) => steps.iter().enumerate().any(|(idx, step)| {
                    idx > first && idx <= first.saturating_add(*max_steps) && step.healthy
                }),
            };
            (ok, reason_if(ok, "did_not_recover"))
        }
        TrajectoryContractSpec::NoRepeatedStep { .. } => {
            let mut seen: HashSet<&str> = HashSet::new();
            let looped = steps
                .iter()
                .any(|step| !seen.insert(step.signature.as_str()));
            (!looped, reason_if(!looped, "loop_detected"))
        }
        TrajectoryContractSpec::StepBudget { max_steps, .. } => {
            let ok = steps.len() <= *max_steps;
            (ok, reason_if(ok, "step_budget_exceeded"))
        }
    };

    ContractOutcome {
        contract_type: contract.contract_type().to_string(),
        scenario: pack_name.to_string(),
        passed,
        reason,
        severity,
    }
}

fn reason_if(passed: bool, reason: &str) -> Option<String> {
    if passed {
        None
    } else {
        Some(reason.to_string())
    }
}

/// A bundled multi-turn scenario pack: a named trajectory, the faults injected
/// into it, its trajectory contracts, and the headline outcome it demonstrates.
#[derive(Debug, Clone, PartialEq)]
pub struct TrajectoryPack {
    pub name: &'static str,
    pub description: &'static str,
    pub steps: Vec<TrajectoryStep>,
    pub faults: Vec<StepFault>,
    pub contracts: Vec<TrajectoryContractSpec>,
    /// The headline trajectory contract surfaced in smoke reports.
    pub headline_contract: &'static str,
    /// The expected observed outcome of the headline contract, encoded like the
    /// single-call packs: `trajectory_contract_passed` or
    /// `trajectory_contract_failed:<reason>`.
    pub headline_expected: &'static str,
}

fn base(body: &str) -> FaultTargetResponse {
    FaultTargetResponse {
        body: body.to_string(),
        latency_ms: 40,
        retry_count: 0,
        tool_calls: 1,
        input_tokens: 16,
        output_tokens: 16,
        fallback_used: false,
        tool_name: None,
        retrieved_documents: Vec::new(),
    }
}

/// The bundled multi-turn trajectory packs.
///
/// These exercise agent-graph failure modes the single-call packs cannot reach:
/// grounding failure that cascades across retrieval and answer steps, an
/// infinite-reflection loop, and a resilient multi-tool cascade that recovers
/// via a fallback. All run against in-process metadata baselines — no network.
#[must_use]
pub fn bundled_trajectory_packs() -> Vec<TrajectoryPack> {
    vec![
        rag_grounding_failure_pack(),
        reflection_loop_pack(),
        multi_tool_cascade_pack(),
    ]
}

/// RAG grounding failure: retrieval poisoned at the *retrieve* step (index 0)
/// propagates into the *answer* step (index 2), which loses its citation and
/// terminates unhealthy. Demonstrates a fault injected at one step failing a
/// contract at a later step.
fn rag_grounding_failure_pack() -> TrajectoryPack {
    let mut retrieve = base(r#"{"docs":["clean-doc-0"]}"#);
    retrieve.tool_name = Some("vector_search".to_string());
    let mut plan = base(r#"{"plan":"answer from retrieved context"}"#);
    plan.tool_name = None;
    let mut answer = base(r#"{"answer":"grounded answer [doc-0]"}"#);
    answer.tool_name = None;

    let steps = vec![
        TrajectoryStep {
            label: "retrieve".to_string(),
            kind: StepKind::Tool,
            consumes_retrieval: false,
            baseline: retrieve,
            contracts: vec![ContractSpec::ValidJson {
                severity: Some(0.5),
            }],
        },
        TrajectoryStep {
            label: "plan".to_string(),
            kind: StepKind::Model,
            consumes_retrieval: false,
            baseline: plan,
            contracts: vec![ContractSpec::ValidJson {
                severity: Some(0.5),
            }],
        },
        TrajectoryStep {
            label: "answer".to_string(),
            kind: StepKind::Model,
            consumes_retrieval: true,
            baseline: answer,
            contracts: vec![
                ContractSpec::RequiredCitation {
                    severity: Some(1.0),
                },
                ContractSpec::ValidJson {
                    severity: Some(0.5),
                },
            ],
        },
    ];

    TrajectoryPack {
        name: "rag-grounding-failure",
        description: "retrieval poisoned at step 0 leaves the answer at step 2 ungrounded",
        steps,
        faults: vec![StepFault {
            step_index: 0,
            fault: FaultSpec::RetrievalPoisoning {
                document_count: 2,
                probability: 1.0,
            },
        }],
        contracts: vec![
            TrajectoryContractSpec::TerminatesHealthy {
                severity: Some(1.0),
            },
            TrajectoryContractSpec::NoRepeatedStep {
                severity: Some(0.5),
            },
            TrajectoryContractSpec::StepBudget {
                max_steps: 4,
                severity: Some(0.5),
            },
        ],
        headline_contract: "terminates_healthy",
        headline_expected: "trajectory_contract_failed:final_step_unhealthy",
    }
}

/// Infinite reflection loop: a retry-pressure fault at the plan step blows the
/// per-step retry budget, and the agent then re-reflects with identical steps.
/// `NoRepeatedStep` catches the loop and `StepBudget` catches the runaway
/// length.
fn reflection_loop_pack() -> TrajectoryPack {
    let mut plan = base(r#"{"plan":"reflect until confident"}"#);
    plan.tool_name = None;
    let reflect_body = r#"{"reflect":"insufficient grounding, retrying"}"#;

    let reflect_step = |contracts: Vec<ContractSpec>| TrajectoryStep {
        label: "reflect".to_string(),
        kind: StepKind::Model,
        consumes_retrieval: false,
        baseline: base(reflect_body),
        contracts,
    };

    let steps = vec![
        TrajectoryStep {
            label: "plan".to_string(),
            kind: StepKind::Model,
            consumes_retrieval: false,
            baseline: plan,
            contracts: vec![ContractSpec::RetryBudget {
                max_retries: 2,
                severity: Some(1.0),
            }],
        },
        reflect_step(vec![ContractSpec::ValidJson {
            severity: Some(0.5),
        }]),
        reflect_step(vec![ContractSpec::ValidJson {
            severity: Some(0.5),
        }]),
        reflect_step(vec![ContractSpec::ValidJson {
            severity: Some(0.5),
        }]),
    ];

    TrajectoryPack {
        name: "reflection-loop",
        description: "retry pressure sends the agent into an identical-step reflection loop",
        steps,
        faults: vec![StepFault {
            step_index: 0,
            fault: FaultSpec::RetryLoopPressure {
                max_retries: 5,
                probability: 1.0,
            },
        }],
        contracts: vec![
            TrajectoryContractSpec::NoRepeatedStep {
                severity: Some(1.0),
            },
            TrajectoryContractSpec::StepBudget {
                max_steps: 3,
                severity: Some(1.0),
            },
            TrajectoryContractSpec::TerminatesHealthy {
                severity: Some(0.5),
            },
        ],
        headline_contract: "no_repeated_step",
        headline_expected: "trajectory_contract_failed:loop_detected",
    }
}

/// Resilient multi-tool cascade: a tool failure at step 1 is contained because
/// the synthesize step at index 2 falls back to a cached result and terminates
/// healthy. Demonstrates a trajectory that *passes* its recovery contract.
fn multi_tool_cascade_pack() -> TrajectoryPack {
    let mut search = base(r#"{"results":["hit-0"]}"#);
    search.tool_name = Some("web_search".to_string());
    let mut lookup = base(r#"{"detail":"ok"}"#);
    lookup.tool_name = Some("db_lookup".to_string());
    let mut synthesize = base(r#"{"answer":"answer from cached fallback [cache-0]"}"#);
    synthesize.tool_name = None;
    synthesize.fallback_used = true;

    let steps = vec![
        TrajectoryStep {
            label: "search".to_string(),
            kind: StepKind::Tool,
            consumes_retrieval: false,
            baseline: search,
            contracts: vec![ContractSpec::ValidJson {
                severity: Some(0.5),
            }],
        },
        TrajectoryStep {
            label: "lookup".to_string(),
            kind: StepKind::Tool,
            consumes_retrieval: false,
            baseline: lookup,
            contracts: vec![ContractSpec::FallbackUsed {
                severity: Some(1.0),
            }],
        },
        TrajectoryStep {
            label: "synthesize".to_string(),
            kind: StepKind::Model,
            consumes_retrieval: false,
            baseline: synthesize,
            contracts: vec![
                ContractSpec::FallbackUsed {
                    severity: Some(1.0),
                },
                ContractSpec::RequiredCitation {
                    severity: Some(0.75),
                },
            ],
        },
    ];

    TrajectoryPack {
        name: "multi-tool-cascade",
        description: "a tool failure at step 1 is contained by a fallback at step 2",
        steps,
        faults: vec![StepFault {
            step_index: 1,
            fault: FaultSpec::ToolFailure {
                error_type: "timeout".to_string(),
                probability: 1.0,
            },
        }],
        contracts: vec![
            TrajectoryContractSpec::RecoversWithin {
                max_steps: 2,
                severity: Some(1.0),
            },
            TrajectoryContractSpec::TerminatesHealthy {
                severity: Some(1.0),
            },
            TrajectoryContractSpec::NoRepeatedStep {
                severity: Some(0.5),
            },
        ],
        headline_contract: "recovers_within",
        headline_expected: "trajectory_contract_passed",
    }
}
