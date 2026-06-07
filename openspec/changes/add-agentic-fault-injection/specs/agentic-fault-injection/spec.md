## ADDED Requirements

### Requirement: Agentic Target Adapters
Tumult SHALL support agentic AI systems as experiment targets through typed adapters. The MVP SHALL include HTTP agent targets and MCP tool/server targets.

#### Scenario: Run against HTTP agent target
- **WHEN** an experiment defines an agent target with `type: http` and an invocation endpoint
- **THEN** Tumult invokes the endpoint as an agentic scenario target and records the request outcome, latency, trace identifiers, and contract results in the journal

#### Scenario: Run against MCP target
- **WHEN** an experiment defines an agent target with `type: mcp` and a tool invocation
- **THEN** Tumult invokes the MCP target through validated tool input and records the tool outcome, latency, trace identifiers, and contract results in the journal

### Requirement: Agentic Fault Catalogue
Tumult SHALL provide GenAI-specific fault definitions for model latency, model timeout, provider error, rate limit, malformed output, output truncation, hallucinated tool call, tool latency, tool failure, retrieval poisoning, context truncation, token budget exhaustion, and retry-loop pressure.

#### Scenario: Inject model latency fault
- **WHEN** an agentic experiment configures a `model_latency` fault with latency, duration, and probability
- **THEN** Tumult applies the latency to the configured model-call boundary and records the active fault window and applied probability in telemetry and the journal

#### Scenario: Inject malformed output fault
- **WHEN** an agentic experiment configures a `malformed_output` fault for a structured response
- **THEN** Tumult returns a syntactically invalid or schema-invalid response according to the fault configuration and evaluates the agent's recovery behavior

#### Scenario: Reject unsupported fault
- **WHEN** an agentic experiment references an unknown fault type
- **THEN** validation fails before execution with a structured error naming the unsupported fault

### Requirement: Behavioral Contracts
Tumult SHALL evaluate behavioral contracts across agent scenarios and fault injections. The MVP SHALL include deterministic contracts for valid JSON, required citation, no PII, no secret leakage, max latency, retry budget, max tool calls, max token usage, fallback used, and graceful error handling.

#### Scenario: Contract passes under fault
- **WHEN** an agent response satisfies all configured contracts while a fault is active
- **THEN** Tumult marks each contract as passed and includes the result in the contract matrix

#### Scenario: Contract fails under fault
- **WHEN** an agent response violates a configured contract while a fault is active
- **THEN** Tumult marks the contract as failed, records the low-cardinality failure reason, and updates the agent resilience score

#### Scenario: Sensitive content is redacted
- **WHEN** a contract evaluates PII or secret leakage
- **THEN** Tumult records only redacted evidence and low-cardinality labels in logs, spans, and journals by default

### Requirement: Contract x Fault Matrix Scoring
Tumult SHALL compute an agent resilience score from scenario, fault, and contract outcomes. The score SHALL support severity weights and SHALL emit sub-scores for latency, recovery, retry budget, cost/token usage, and replay regression when those dimensions are present.

#### Scenario: Compute weighted resilience score
- **WHEN** an agentic run completes a matrix of scenarios, faults, and contracts
- **THEN** Tumult computes a weighted score between 0.0 and 1.0 and records per-contract outcomes that explain the score

#### Scenario: Fail CI gate on score threshold
- **WHEN** a CLI run is configured with a minimum resilience score and the computed score is below that threshold
- **THEN** the command exits non-zero and prints the failing scenarios, faults, contracts, and score delta

### Requirement: Deterministic Replay Regression
Tumult SHALL support replaying captured agent sessions through normalized replay fixtures. Replay fixtures SHALL preserve the sequence of user input, model responses, tool results, retrieval results, and expected contracts without requiring the original external provider.

#### Scenario: Replay captured failure
- **WHEN** a replay fixture defines a previously captured failed session
- **THEN** Tumult replays the session deterministically and reports whether the current agent behavior satisfies the configured contracts

#### Scenario: Reject incomplete replay fixture
- **WHEN** a replay fixture omits a required step output
- **THEN** validation fails before execution with a structured error naming the missing replay step

### Requirement: OpenTelemetry GenAI Correlation
Tumult SHALL correlate agentic experiments with OpenTelemetry GenAI semantic conventions. Agentic telemetry SHALL preserve `resilience.*` experiment/fault attributes and SHALL add applicable `gen_ai.*` attributes, metrics, exceptions, and evaluation events.

#### Scenario: Emit correlated experiment and GenAI spans
- **WHEN** an agentic experiment executes with trace context propagation enabled
- **THEN** Tumult records a `resilience.experiment` span and correlates agent workflow, model, retrieval, and tool spans through parent context or recorded trace/span identifiers

#### Scenario: Emit evaluation event
- **WHEN** a contract produces an evaluation score or label
- **THEN** Tumult emits or records a `gen_ai.evaluation.result` event with the evaluation name, score label or value, and no raw sensitive content by default

### Requirement: Agentic Journals and Analytics
Tumult SHALL write agentic run evidence to TOON journals and analytics-ready tables or views. The evidence SHALL include target type, scenario name, active fault, contract outcomes, score, sub-scores, trace identifiers, and replay identifiers when present.

#### Scenario: Write agentic journal
- **WHEN** an agentic experiment completes
- **THEN** the journal includes an agentic result section with scenarios, fault applications, contract outcomes, scores, and trace correlation fields

#### Scenario: Query agentic analytics
- **WHEN** agentic journals are ingested into analytics storage
- **THEN** users can query agentic runs, contract failures, fault types, scores, and replay outcomes with SQL

### Requirement: Scenario Packs
Tumult SHALL ship local scenario packs for common agentic resilience cases. The MVP SHALL include concurrency storm, hallucination under timeout, cost explosion detector, malformed JSON recovery, tool timeout fallback, and retrieval poisoning.

#### Scenario: List scenario packs
- **WHEN** the user lists available agentic scenarios
- **THEN** Tumult prints the scenario pack names, supported target adapters, fault types, and required contracts

#### Scenario: Run scenario pack
- **WHEN** the user runs a bundled scenario pack against a supported agent target
- **THEN** Tumult expands the pack into an executable agentic experiment and records the generated experiment definition

### Requirement: Smoke-Test Feedback Loop
Tumult SHALL provide fast smoke tests for each implementation slice. Smoke tests SHALL be deterministic, SHALL avoid external LLM/provider dependencies, SHALL run against local fake HTTP or MCP agents, and SHALL print clear failure feedback naming the broken adapter, fault, contract, or telemetry expectation.

#### Scenario: Agentic smoke test passes
- **WHEN** a developer runs the agentic smoke-test command after implementing a slice
- **THEN** the command completes quickly, exits zero, and prints the exercised adapter, injected fault, contract checks, journal path, and trace assertion summary

#### Scenario: Agentic smoke test fails clearly
- **WHEN** a smoke-test expectation fails
- **THEN** the command exits non-zero and prints the expected value, actual value, scenario name, fault type, contract name, and next diagnostic command

#### Scenario: No network dependency in smoke tests
- **WHEN** smoke tests execute in CI without external network access
- **THEN** all agentic smoke tests use local fixtures, fake agents, or replay data and do not call external model providers

### Requirement: Safety and Privacy Controls
Tumult SHALL default to metadata-only capture for agent prompts, completions, tool inputs, tool outputs, and retrieval context. Raw content capture SHALL require explicit opt-in and SHALL support redaction before logs, spans, journals, or analytics ingestion.

#### Scenario: Metadata-only default
- **WHEN** an agentic experiment runs without explicit content capture enabled
- **THEN** Tumult records token counts, sizes, hashes, labels, and trace identifiers without storing raw prompts, completions, tool payloads, or retrieved documents

#### Scenario: Target allowlist required
- **WHEN** an agentic experiment targets an HTTP URL or MCP endpoint
- **THEN** Tumult validates the target against the configured allowlist before invoking it
