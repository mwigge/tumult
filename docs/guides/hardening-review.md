---
title: Functional hardening review
parent: Guides
---

# Functional hardening review — 2.22.0

This release addresses the functional review made against 2.20.0, revalidated against 2.21.0 before implementation. Four specialists covered execution, API/UI, data/reporting, and documentation/distribution. A second review of the implementations found additional defects and added regression tests before final release gates.

## Finding coverage

| Findings | Delivered correction | Regression evidence |
|---|---|---|
| F01–F03 | Probe-only observation, failed-probe rejection, cancellation-aware dispatch. | Core safety tests and CLI marker test. |
| F04–F05 | Atomic durable start, conditional state transitions, retryable cleanup and original target/config checks. | Worker transaction, state-race, credential-drift and repeated-crash tests. |
| F06–F08 | Masked previews, authoritative operator bindings, private execution pins and uniform resource/environment scopes. | API scope and execution-policy regressions. |
| F09/F19 | Committed full-table snapshots, schema/content identity, file checksums and serialized publication. | Late/tied arrivals, publication retry, corruption, schema-change and symlink tests. |
| F10 | Correct image argv, writable persisted workspace and container runtime assets. | Local distribution contracts and composed-argument checks completed. Tagged container builds, version and entrypoint-help checks await the release workflow; deployed startup/persistence is a separate acceptance check. |
| F11 | Shared CLI/MCP/daemon provider composition and explicit load execution failure. | MCP provider dispatch and core load tests. |
| F12 | Draft resume/edit/submit, idempotent retry and exact-preview start protection. | Browser journeys against mocked API contracts, plus real API integration tests. |
| F13/F18 | Protobuf OTLP responses/errors and unsupported-point accounting. | HTTP/gRPC protocol and mixed metric batch tests. |
| F14 | Separate portable evidence snapshots from complete private database backup/restore. | Future-table/view/credential restore, tamper, destination and permission tests. |
| F15–F16 | Unverified independence, unmapped clauses and target/environment-aware scoring. | Report wording and same-name distinct-target tests. |
| F17 | Explicitly reject automatic retention while queries remain hot-only. | Both daemon retention settings and direct sweep refusal tests. |
| Documentation findings | Current lifecycle/capability descriptions, working startup prerequisites, checked installer/proof commands and accurate backup/concurrency guidance. | Documentation checks and distribution regression suite. |
| Execution review: script timing | Measure script duration after execution, including failures. | Script-provider elapsed-time regression. |

## Release verification boundary

This matrix describes implemented corrections and their regression coverage;
it is not a declaration that the release has been published or deployed. Final
source gate results belong in the PR evidence. At this review point, the
tagged release workflow has not completed. Its bounded container checks run
the CLI `--version` and `mcp serve --help`, plus the MCP image's own entrypoint
with deployment arguments and `--help`. These check executable packaging
without starting a server. They do not validate listeners, the daemon image,
Kubernetes deployment, persisted journals or restart cleanup.

Before accepting a deployment, start the exact release image with its intended
identity and mounts, run a harmless authorized experiment, verify journal and
telemetry persistence, then verify restart and cancellation behavior. Real
Kubernetes/cloud/Windows faults and live external AI-client integrations need
their own disposable-target checks. No such deployment certification is
claimed here. See [quality and release checks](quality-and-release.md) and
[provider prerequisites](production-deployment.md#provider-capabilities).

## Product-owner acceptance

As an experiment operator, I want observation and stop requests to constrain execution so that I can inspect or halt a target without starting unintended method actions.

- Given hypothesis probes and method actions, when I request baseline-only, then only the probes execute and the output is identified as an observation.
- Given an action waiting for a pause or concurrency slot, when I stop the run, then it does not dispatch and cleanup ownership remains durable.

As a scoped operator, I want previews and executions bound to my permitted destination so that a changed request cannot silently weaken approval or target another environment.

- Given a reviewed binding and permitted environment, when I preview and start the unchanged plan, then the server checks the same artifact and destination through dispatch.
- Given edited parameters, a stale preview or an unbound plan, when I start, then the server rejects the stale/scoped request or applies conservative review; secrets do not appear in preview content.

As an evidence author, I want to resume a saved draft so that my work is neither stranded nor duplicated.

- Given a saved draft, when I reopen, edit and submit it, then the original record progresses to review.
- Given a failed submission, when I retry, then no duplicate record is created and a contributor cannot verify their own content.

As a platform operator, I want complete and verified data recovery so that a backup or archive is dependable evidence.

- Given a private database backup, when I restore to a fresh destination, then all database tables and credentials are restored and verified without overwriting a live file.
- Given late telemetry, corrupt archive bytes or an interrupted publication, when I export/read evidence, then committed records remain recoverable and invalid content is rejected or repaired.

## Security assessment

The review traced authenticated API and daemon entrypoints, privileged execution, stored evidence and filesystem publication. It was a source and regression review, not a production penetration test.

| Boundary | Finding and correction | Residual assumption |
|---|---|---|
| Resolved credentials | Preview/error disclosure, public execution fingerprints and rotated-secret validation errors are protected by masking, opaque keyed tokens, private pin storage and safe preparation errors. | Unrestricted administrators can configure arbitrary execution and are trusted with binding fingerprints. Operator-supplied programs can print secrets; their output is evidence, not automatically sanitized text. |
| Environment authorization | List/mutation gaps, global metric reports and scheduled execution are checked consistently; actor revocation is checked before dispatch. | Environment scopes are not a multi-customer tenant boundary. |
| Governance | Contributors cannot independently verify their own evidence; stale decisions cannot alter already dispatched runs. | Administrative break-glass remains a deliberate audited privilege. |
| Execution and cleanup | Cancellation boundaries, durable start, retryable cleanup and descendant-held process pipes are covered by new regressions. | External actions cannot be made transactional; rollback must be idempotent and target connectivity must be available. |
| Evidence files | Checksums detect corruption, manifest publication selects committed files, restore refuses existing files/WAL entries, and journal publication syncs its directory. | An administrator able to replace both data and manifest can forge checksums; these are integrity checks, not signatures. Archive and restore directories must be operator-controlled. |
| Dependencies | Patched HTTP/TLS and web dependencies remove newly identified advisories; Cargo policy and npm audit are release gates. | Existing quick-xml advisories RUSTSEC-2026-0194/0195 remain in the Typst chain with documented trusted-input exceptions; compatible upstream releases were rechecked on 2026-10-09. |

Operational endpoints, credentials and bindings remain configurable. Protocol codes, output limits, schema versions and bounded execution defaults remain named domain constants. The approval appendix's 500-record limit is explicitly disclosed as a sample. Fixture identities and marker actions are test data.

## Independent review and deliberate limits

The second pass checks correctness, readability, duplication, configurable values, security and observable user journeys. Shared provider execution replaces the previous duplicated MCP process implementation; checksum, scope, preparation, cancellation and journal-output helpers centralize repeated decisions. Regression tests also cover corruption reuse, unsafe restore paths, contributor self-review, stale approval transitions, graph URL credentials and errors disclosing storage details.

This remains a single-tenant installation. The release does not implement the separate SaaS roadmap. Statistical baseline acquisition remains external; baseline-only is one declared-probe round. Sustained post-cleanup recovery, degradation-shape classification and automatic data-integrity verification remain unimplemented, as described in the [data lifecycle](../data-lifecycle.md). Evidence packs disclose unmapped clauses and unverified independence; they do not establish either automatically. Full-table snapshots prioritize correctness; cold-data queries are not implemented and automatic deletion is disabled. Rollback recovery is at least once and requires idempotent cleanup; changed configuration leaves cleanup pending for operator action. External attachment bytes and deployment secrets/assets need separate operational backup.

See [data portability](data-portability.md), [execution bindings](execution-bindings.md), and [quality gates](quality-and-release.md) for operator actions and verification commands. Final gate results are recorded in the release/PR evidence; individual red-log filenames are not treated as proof unless the command actually failed before implementation.
