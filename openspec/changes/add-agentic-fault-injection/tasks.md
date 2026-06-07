## 1. Verification Harness and Toolkit Rules

- [x] 1.0 Detach or feature-gate the existing `tumult-intelligence` / `rs-llmctl-client` coupling so the Tumult workspace builds without a checkout of the standalone `rs-llmctl` project
- [x] 1.1 Record the applied `/home/morgan/dev/src/agent-toolkit-bundle` rules in the implementation notes: Rust/TDD, AI developer, chaos engineer, observability, security/compliance, and CI/CD gates
- [x] 1.2 Add a `tumult-agentic` smoke-test plan that defines the local fake HTTP agent, fake MCP target, replay fixture, expected journal fields, and expected trace metadata
- [x] 1.3 Add the first failing smoke test for the end-to-end path: fake HTTP target -> malformed output fault -> valid JSON contract failure -> journal evidence -> clear failure output
- [x] 1.4 Add a smoke-test command shape, either `cargo test -p tumult-agentic smoke_ -- --nocapture` or `tumult agentic smoke`, with documented expected output
- [x] 1.5 Ensure smoke tests require no external network, no external LLM provider, no secrets, and no raw prompt/completion persistence

## 2. Workspace and Data Model

- [x] 2.1 Create the `tumult-agentic` crate with workspace dependencies, lint inheritance, typed errors using `thiserror`, and no library `.unwrap()` usage
- [x] 2.2 Define agent target types for HTTP, MCP, and replay-backed targets with validation errors for unsupported target types
- [x] 2.3 Define agent scenario, fault, contract, replay, score, and result structs with serde support for TOON-compatible serialization
- [x] 2.4 Define privacy controls for metadata-only capture, explicit content-capture opt-in, redaction labels, and target allowlist enforcement
- [x] 2.5 Add unit tests for config validation, unsupported faults, incomplete replay fixtures, target allowlists, and metadata-only defaults

## 3. Fault Engine

- [x] 3.1 Implement deterministic fault selection with duration, probability, and seedable behavior for tests
- [x] 3.2 Implement MVP model/provider faults: latency, timeout, provider error, rate limit, malformed output, output truncation, and token budget exhaustion
- [x] 3.3 Implement MVP tool/retrieval/context faults: tool latency, tool failure, hallucinated tool call, retrieval poisoning, context truncation, and retry-loop pressure
- [x] 3.4 Add unit tests for every fault type, including expected failure messages and low-cardinality fault labels
- [x] 3.5 Update the smoke test to assert active fault window, injected fault type, and contract impact are visible in output and journal evidence

## 4. Target Adapters

- [x] 4.1 Implement the HTTP agent target adapter with request timeout, target allowlist validation, structured errors, and trace context propagation
- [x] 4.2 Implement the MCP target adapter with schema-validated tool input, structured tool errors, request timeout, and trace context propagation
- [x] 4.3 Implement the replay adapter using normalized local fixtures for model responses, tool results, retrieval results, and expected contracts
- [x] 4.4 Add fake HTTP and fake MCP test fixtures that exercise success, timeout, malformed output, and tool failure paths
- [x] 4.5 Add adapter smoke tests with clear pass/fail output naming adapter, scenario, fault, contract, expected value, actual value, and next diagnostic command

## 5. Behavioral Contracts and Scoring

- [x] 5.1 Implement deterministic contracts for valid JSON, required citation, no PII, no secret leakage, max latency, retry budget, max tool calls, max token usage, fallback used, and graceful error handling
- [x] 5.2 Ensure contract evidence is redacted by default and records only labels, hashes, lengths, token counts, and trace identifiers unless content capture is explicitly enabled
- [x] 5.3 Implement the contract x fault matrix with severity weights and per-contract pass/fail evidence
- [x] 5.4 Implement agent resilience score and sub-scores for latency, recovery, retry budget, cost/token usage, and replay regression where inputs exist
- [x] 5.5 Add tests for score thresholds causing non-zero CLI-style failures with scenario, fault, contract, and score delta in the output

## 6. Telemetry, Journals, and Analytics

- [x] 6.1 Add telemetry constants/helpers for Tumult `resilience.*` and applicable OTel `gen_ai.*` attributes, metrics, exceptions, and evaluation events
- [x] 6.2 Emit spans or span-ready records for agent scenario execution, fault application, contract evaluation, replay step execution, and score calculation
- [x] 6.3 Write agentic result sections into TOON journals with target type, scenario, active fault, contract outcomes, scores, trace IDs, and replay IDs
- [x] 6.4 Add analytics ingestion support for agentic runs, contract outcomes, fault applications, replay outcomes, and score tables or views
- [x] 6.5 Add tests proving telemetry/journal output excludes raw prompts, completions, tool payloads, and retrieved documents by default

## 7. CLI and MCP Surface

- [x] 7.1 Add CLI commands for listing scenario packs, running an agentic experiment, running replay regression, and running agentic smoke tests
- [x] 7.2 Add clear CLI failure output for validation errors, target allowlist failures, smoke-test failures, and resilience threshold failures
- [x] 7.3 Add optional MCP tools for discovering agentic scenarios and running agentic experiments with input schema validation
- [x] 7.4 Add tests for CLI parsing, structured errors, exit codes, and MCP schema validation
- [x] 7.5 Ensure new CLI and MCP surfaces never require secrets and never log raw sensitive payloads

## 8. Scenario Packs and Examples

- [x] 8.1 Add bundled scenario packs for concurrency storm, hallucination under timeout, cost explosion detector, malformed JSON recovery, tool timeout fallback, and retrieval poisoning
- [x] 8.2 Add local example agent fixtures and TOON examples that run entirely offline
- [x] 8.3 Add scenario-pack validation tests for supported adapters, fault types, required contracts, and generated experiment definitions
- [x] 8.4 Add a smoke run for at least one scenario pack that produces a journal and trace assertion summary

## 9. Documentation and Feedback Loops

- [x] 9.1 Update README positioning to describe agentic fault injection as a new module that complements AI observability and traditional chaos engineering
- [x] 9.2 Add an agentic quickstart with the local fake agent, smoke command, expected passing output, and expected failing output
- [x] 9.3 Add an OTel GenAI correlation guide documenting `resilience.*`, `gen_ai.*`, privacy defaults, and trace/journal correlation
- [x] 9.4 Add a scenario-pack reference and replay-regression guide
- [x] 9.5 Add a quality-gate section listing required commands: smoke tests, crate tests, `cargo fmt --check`, `cargo clippy -- -D warnings -W clippy::pedantic`, `cargo test --workspace`, and `cargo audit`

## 10. Final Validation

- [x] 10.1 Run agentic smoke tests and capture output showing adapter, fault, contracts, journal path, trace assertion summary, and no external network dependency
- [x] 10.2 Run `cargo test -p tumult-agentic`
- [x] 10.3 Run `cargo test --workspace`
- [x] 10.4 Run `cargo fmt --check`
- [x] 10.5 Run `cargo clippy -- -D warnings -W clippy::pedantic`
- [x] 10.6 Run `cargo audit`
- [x] 10.7 Run `openspec validate add-agentic-fault-injection --strict`
