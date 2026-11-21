---
title: Agentic Observability
parent: Work with AI agents
grand_parent: Guides
nav_order: 6
summary: "Spans and metrics for agent runs"
---

# Agentic Observability

Agentic fault injection extends Tumult's existing OpenTelemetry model with
GenAI-aware spans and metadata-only evidence. The goal is to answer three
questions after every run:

- Which scenario, fault, and contract produced the result?
- Which agent/model/tool spans correlate to the experiment?
- Did the evidence avoid storing raw prompts, completions, tool payloads, or
  retrieved documents?

## Span Shape

Agentic runs should correlate with the normal experiment root span:

```text
tumult.experiment                  one live run
└── tumult.agentic.fault           one proxied request
tumult.agentic.tool                one MCP tool call
└── resilience.agentic.experiment  the run, with fault and contract events
```

The current local smoke path records deterministic trace metadata and contract
evidence without exporting spans. Production adapters should propagate
`traceparent` to HTTP or MCP targets and record returned trace/span IDs when an
agent produces them.

## Attribute Reference

Tumult keeps low-cardinality resilience attributes for experiment analysis.
Spans of the client itself carry their own `gen_ai.*` attributes. The
collector maps them, see
[cross-client observability](agentic-cross-client-observability.md).

| Attribute | Purpose |
|---|---|
| `resilience.experiment.id` | Stable experiment identifier |
| `resilience.agent.scenario` | Scenario name or pack entry |
| `resilience.agent.fault.type` | Low-cardinality fault type |
| `resilience.agent.contract` | Contract name when recording an evaluation |
| `resilience.agent.score` | Run or matrix resilience score |
| `resilience.agent.payload.capture_policy` | `metadata_only`, `redacted_content`, or `raw_content` |
| `gen_ai.tool.name` | Tool name for tool-call spans |

Keep scenario, fault, contract, and evaluation labels bounded. Put detailed
diagnostics in the journal, not in high-cardinality span attributes.

## Privacy Defaults

The default capture policy is `metadata_only`. Journals and spans should store:

- token counts, byte counts, hashes, scenario names, fault labels, contract
  outcomes, scores, trace IDs, and replay IDs
- no raw prompts, completions, tool inputs, tool outputs, retrieval documents,
  PII, secrets, or provider credentials

Use `redacted_content` or `raw_content` only for controlled test environments
where the target allowlist and data handling rules are explicit.

## Smoke Trace Loop

Use the smoke command before broader tests:

```bash
tumult agentic smoke
```

The output names the adapter, target type, scenario, injected fault, expected
contract result, actual contract result, resilience score, trace identifier, and
journal path, analytics ingestion result, trace assertion summary, and next
diagnostic command. A passing smoke run confirms that the fault was observed and
the contract feedback loop is readable.

## Analytics Tables

Agentic runs can be queried from the embedded analytics store:

```sql
SELECT run_id, scenario, resilience_score FROM agentic_runs;
SELECT run_id, contract_type, passed, reason FROM agentic_contract_outcomes;
SELECT run_id, fault_type, applied FROM agentic_fault_applications;
SELECT run_id, replay_id, passed FROM agentic_replay_outcomes;
```

These tables store low-cardinality labels, scores, trace IDs, and replay IDs.
They do not store raw prompts, completions, tool payloads, or retrieved
documents.
