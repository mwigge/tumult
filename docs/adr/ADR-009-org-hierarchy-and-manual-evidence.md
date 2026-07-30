---
title: "ADR-009: Org Hierarchy and Manual Evidence"
parent: Architecture Decisions
nav_order: 9
---

# ADR-009: Org Hierarchy and Manual Evidence (imported from kronika ADR 0004)

- Status: accepted (live in v0.5.0)
- Date: 2026-07-29

## Context

v0.4.0 gave Krönika per-experiment resilience scores and compliance-grade
reports. Two gaps remained for programme-level resilience management:

1. **No organisational axis.** Scores roll up by target system and
   portfolio only; "which team/domain is under-tested?" is unanswerable.
2. **No manual evidence.** Game days, tabletops and vendor-run failovers
   leave no agent telemetry, so a whole class of framework-required
   testing (DORA Art. 24, ISO 27001 A.5.30/A.5.35, SOC 2 A1.2–A1.3) was
   invisible to the scorecard and the evidence packs.

The full analysis is in `docs/research-org-rollups.md` and
`docs/research-manual-evidence.md`; this ADR records the decisions.

## Decision

### A. Org hierarchy: YAML tree, weighted-from-leaves aggregation

- The hierarchy is a **single-parent tree of arbitrary depth** declared in
  a Backstage-style `org.yaml` (`nodes`, `assignments`, `defaults`),
  loaded once at daemon start from `KRONIKA_ORG_FILE` (default
  `<db dir>/org.yaml`); a missing file means an empty tree (everything
  rolls up under a visible synthetic `(unassigned)` node at the root).
  Single-parent keeps paths canonical (`platform/edge/edge-team`) and
  every score unambiguous; matrix/multi-parent membership is rejected as
  out of scope.
- Experiments map to teams by **`*`-only glob** patterns (first matching
  assignment wins), with per-name criticality (`critical` = 3, `high` = 2,
  default 1, times `defaults.weight`). No new dependency: `glob_match` is
  a small hand-rolled matcher; `serde_yaml` was already a workspace dep.
- **Node scores are recomputed from all leaves in the subtree** as a
  criticality-weighted mean — never an average of child means, which
  flatters domains whose smallest team is strongest (worked example in the
  research doc). Companion **coverage = scored / expected**, where pending
  manual records count toward expected only.
- Computation happens **at read time** from the existing latest-run SQL —
  no table migration for Part A.
- UI: a Scores page with KPI cards + an ECharts **treemap** at the root
  (area ∝ leaves × criticality, band hues from Okabe–Ito), click-to-drill
  with breadcrumb and `?node=` URL state, and an indented tree table below
  the root. R1 gains a "By domain" section.

### B. Manual evidence: attested, reviewed, hash-chained

- Three new DuckDB tables (schema v2): `manual_experiments` (content +
  lifecycle + provenance + `content_hash`), `manual_experiment_audit`
  (append-only, hash-chained `prev_hash → new_hash`), and
  `evidence_attachments` (external URIs only — **no file storage**).
- Lifecycle: **draft** (mutable) → **submitted** (requires attestation,
  locked) → **verified** / **rejected** (review note mandatory on reject).
  Verification requires **reviewer ≠ enterer**, app-enforced (DORA
  Art. 24(4) independence / ISO A.5.35); same-user verify returns 400.
- **Verified manual records score exactly like automated runs** (passed
  100 / partial 75 via a new `RunState::Partial` / failed 50, with the
  same 30-day staleness decay); **inconclusive is excluded from scoring**
  — an exercise without a verdict must not invent one, though it still
  counts toward coverage's expected. Drafts/submitted count toward
  coverage as pending, with zero score weight.
- IDs are **hand-rolled monotonic ULIDs** (Crockford base32, 48-bit
  millis + 80-bit `/dev/urandom` randomness) — no new crate for 30 lines.
- Mutations ride the daemon's **single writer**: the API never opens a
  write connection; lifecycle closures are sent through a new
  `Batch::Exec` variant on the ingest channel.
- `POST /api/manual/import` bulk-creates records as **drafts** in one
  transaction under an `import_batches` row — attestation is not bypassed;
  every record still needs submit → verify to score.
- **No auth.** The API takes plain "acting as" user strings and enforces
  the lifecycle given those strings. This is workflow scaffolding, not
  access control — documented on the Manual page and in both research
  docs.

### Reporting integration

- R1 gains a "By domain" table (top-level org children: score, band,
  coverage, weakest) and an evidence-mix footnote ("N automated, M
  verified manual").
- R2's test register carries provenance (origin badge, executed vs entered
  dates, verifier) and gains a **per-entry attestation appendix** for
  verified manual records; the independence paragraph now describes the
  attestation + segregated-review control.
- R3 is unchanged (it reports one automated run).

## Consequences

- Schema v2 is additive (`CREATE TABLE IF NOT EXISTS`); existing stores
  migrate on open. No data backfill is needed.
- `GET /api/scores/tree` costs ~12 latest-run computations per request
  (current, previous, ten sparkline points); fine at real-world scale, and
  the one-level children shape bounds it. Per-child deltas/sparklines were
  deliberately not computed (cost multiplier) — the tree table shows `—`.
- An experiment name present in both origins yields one leaf per origin
  (documented double-count edge). Pending records are read as-of-now for
  historical sparkline points (coverage history is approximate; score
  history is exact).
- The hash chain is tamper-*evidence* against quiet edits, not proof
  against a store admin rewriting history — the right model for an
  embedded single-tenant store.
- kronika-api now depends on kronika-ingest (for `Batch::Exec`); the
  layering stays acyclic.
- Deferred: org file hot-reload, un-reject / revision flows, file-blob
  attachments, auth, origin-aware dedupe.
