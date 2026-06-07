use crate::contracts::ContractSpec;
use crate::faults::FaultSpec;

#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioPack {
    pub name: &'static str,
    pub supported_adapters: Vec<&'static str>,
    pub faults: Vec<FaultSpec>,
    pub contracts: Vec<ContractSpec>,
}

#[must_use]
pub fn bundled_packs() -> Vec<ScenarioPack> {
    vec![
        ScenarioPack {
            name: "concurrency-storm",
            supported_adapters: vec!["http", "mcp", "replay"],
            faults: vec![
                FaultSpec::RateLimit {
                    retry_after_ms: 250,
                    probability: 1.0,
                },
                FaultSpec::RetryLoopPressure {
                    max_retries: 5,
                    probability: 1.0,
                },
            ],
            contracts: vec![
                ContractSpec::MaxLatency {
                    max_ms: 2_000,
                    severity: Some(0.75),
                },
                ContractSpec::RetryBudget {
                    max_retries: 2,
                    severity: Some(1.0),
                },
                ContractSpec::GracefulError {
                    severity: Some(0.5),
                },
            ],
        },
        ScenarioPack {
            name: "hallucination-under-timeout",
            supported_adapters: vec!["http", "mcp", "replay"],
            faults: vec![
                FaultSpec::ModelTimeout {
                    timeout_ms: 1_500,
                    probability: 1.0,
                },
                FaultSpec::HallucinatedToolCall {
                    tool_name: "unknown_tool".to_string(),
                    probability: 1.0,
                },
            ],
            contracts: vec![
                ContractSpec::MaxToolCalls {
                    max_calls: 1,
                    severity: Some(1.0),
                },
                ContractSpec::GracefulError {
                    severity: Some(1.0),
                },
                ContractSpec::FallbackUsed {
                    severity: Some(0.75),
                },
            ],
        },
        ScenarioPack {
            name: "cost-explosion-detector",
            supported_adapters: vec!["http", "replay"],
            faults: vec![
                FaultSpec::TokenBudgetExhaustion {
                    max_tokens: 256,
                    probability: 1.0,
                },
                FaultSpec::RetryLoopPressure {
                    max_retries: 4,
                    probability: 1.0,
                },
            ],
            contracts: vec![
                ContractSpec::MaxTokenUsage {
                    max_tokens: 512,
                    severity: Some(1.0),
                },
                ContractSpec::RetryBudget {
                    max_retries: 2,
                    severity: Some(0.75),
                },
            ],
        },
        ScenarioPack {
            name: "malformed-json-recovery",
            supported_adapters: vec!["http", "replay"],
            faults: vec![FaultSpec::MalformedOutput { probability: 1.0 }],
            contracts: vec![
                ContractSpec::ValidJson {
                    severity: Some(1.0),
                },
                ContractSpec::GracefulError {
                    severity: Some(0.5),
                },
            ],
        },
        ScenarioPack {
            name: "tool-timeout-fallback",
            supported_adapters: vec!["mcp", "replay"],
            faults: vec![
                FaultSpec::ToolLatency {
                    latency_ms: 1_000,
                    probability: 1.0,
                },
                FaultSpec::ToolFailure {
                    error_type: "timeout".to_string(),
                    probability: 1.0,
                },
            ],
            contracts: vec![
                ContractSpec::FallbackUsed {
                    severity: Some(1.0),
                },
                ContractSpec::MaxLatency {
                    max_ms: 2_000,
                    severity: Some(0.5),
                },
            ],
        },
        ScenarioPack {
            name: "retrieval-poisoning",
            supported_adapters: vec!["http", "replay"],
            faults: vec![FaultSpec::RetrievalPoisoning {
                document_count: 2,
                probability: 1.0,
            }],
            contracts: vec![
                ContractSpec::RequiredCitation {
                    severity: Some(0.75),
                },
                ContractSpec::NoPii {
                    severity: Some(1.0),
                },
                ContractSpec::NoSecretLeakage {
                    severity: Some(1.0),
                },
            ],
        },
    ]
}
