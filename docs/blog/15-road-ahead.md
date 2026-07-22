---
title: "The Road Ahead"
parent: Blog
nav_order: 15
updated: 2026-07-21
---

# The road ahead

*Originally published 2026-03-30; updated 2026-07-21 for Tumult 2.16.1.*

Tumult has moved beyond the phase-based roadmap described in the original
version of this post. This revision records the current product boundary and
the work that remains. For exact command and schema details, use the maintained
[guides](../guides/index.md) rather than this article.

## Current capabilities

| Area | Delivered behavior | Verification boundary |
|---|---|---|
| Experiment engine | Five-phase lifecycle, controls, recovery sampling, rollback, TOON journals | Workspace and example tests |
| Fault executors | Script plugins plus native SSH, TCP proxy, Kubernetes, cloud, and Windows executors | Local tests; external targets require their respective environments |
| Analytics | Arrow conversion, embedded DuckDB, Parquet/CSV/JSON/IPC export, trends and TUI | Workspace and DuckDB integration tests |
| Observability | OpenTelemetry spans and metrics across execution and adapters | Local collector and demo proofs |
| MCP | 40 tools over stdio and HTTP, resources, schemas, annotations, pagination and RBAC | MCP integration suite |
| GameDays | Coordinated campaigns, shared load and evidence scoring | Docker example workflow |
| ChaosGraph | Journal-derived graph, openCypher queries, coverage gaps and topology | Workspace and demo topology proofs |
| Autopilot | Deterministic recommendations, a 14-rule gate, enrollment, change events and audited decisions | Policy replay corpus and demo proofs |
| Agentic testing | Deterministic scenario packs, live proxy faults and multi-turn trajectory contracts | Local proxy and replay tests |

These rows describe implemented capabilities, not a claim that every external
provider has been exercised for every release. Release verification records
distinguish local, CI, and live-environment evidence.

## Current priorities

1. **Release reproducibility.** Keep toolchains, lockfiles, containers and
   published artifacts aligned. A tagged release must not advertise an image
   that did not build and pass its smoke test.
2. **Evidence quality.** Publish dated verification results and distinguish
   automated simulation from tests against live Windows, Kubernetes and cloud
   systems.
3. **Documentation accuracy.** Generate or check volatile counts against the
   codebase and qualify comparisons with dated primary sources.
4. **Operational safety.** Expand target enrollment, blast-radius controls,
   guard telemetry validation and failure recovery exercises.
5. **Interoperability.** Keep experiment data, OpenTelemetry attributes,
   Parquet exports and MCP schemas usable without a Tumult-specific backend.

## Explicit non-goals

- Tumult compliance output is evidence toward controls, not a legal or audit
  attestation.
- The autopilot does not use an LLM to decide whether a fault may run.
- Kubernetes discovery does not infer service dependencies and write them into
  the graph without review.
- Cloud and Windows executors are not represented as release-verified when the
  relevant external environment was unavailable.

## How progress is reported

The [changelog](../../CHANGELOG.md) records shipped behavior. The
[verification protocol](../testprotocol.md) describes the test matrix. Each
release verification report records which parts ran locally, in CI, in Docker,
or against an external system.

That separation is intentional: a roadmap explains direction, a changelog
records delivery, and a verification report records evidence.
