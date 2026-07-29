# Research: manual evidence entry (v0.5.0, Part B)

Status: implemented in v0.5.0 — see `kronika_store::manual`,
`/api/manual/*`, the UI's Manual page, and ADR 0004.

## Problem

kronika's evidence trail is only as good as its telemetry. Real resilience
programmes run plenty of tests that never touch an agent:

- **game days** coordinated over a call, with actions executed by hand;
- **tabletop exercises** (walkthroughs of failure scenarios — required by
  several frameworks and completely invisible to OTLP);
- **failover drills** performed by a vendor or a cloud console;
- **pentest** findings with resilience implications.

Today none of these can be recorded, scored, or exported into an evidence
pack. An auditor asking "show me your testing programme" gets only the
automated slice. The feature closes that gap *without* pretending a
hand-typed record is the same class of evidence as an agent-observed trace
— hence attestation, segregated review, and a tamper-evident audit trail
rather than a bare CRUD table.

## Regulatory drivers

- **DORA Art. 24–26** (EU 2022/2554): a digital resilience testing
  programme covering "all ICT systems and applications supporting critical
  or important functions", with tests performed by **independent** internal
  or external testers (Art. 24(4)) — the direct motivation for the
  reviewer ≠ enterer rule.
- **ISO/IEC 27001:2022 A.5.35** ("Independent review of information
  security") and **A.5.30** ("ICT readiness for business continuity",
  tested at planned intervals): test *results* must exist as records and be
  subject to independent review.
- **SOC 2 A1.2/A1.3**: recovery testing of backup and recovery
  infrastructure with evidence of *who* performed and *who* reviewed.
- Common thread: the record must show **execution vs entry vs review** as
  distinct facts with distinct actors and timestamps — which is why the
  register tracks `executed_at_ns`, `entered_by/at_ns` and
  `reviewed_by/at_ns` separately and the R2 pack prints all three.

## Lifecycle design

```
 draft ──submit──▶ submitted ──verify──▶ verified
   ▲                  │
   └──────reject──────┘ (reject is terminal in v0.5.0)
```

- **draft**: fully mutable (PUT replaces the content; attachments allowed).
  Drafts carry zero score weight.
- **submit**: requires a non-empty **attestation** text (supplied at
  creation or refreshed at submit) and locks the record — edits are
  rejected with 409. This is the point of no return where the enterer
  asserts the record is complete and truthful.
- **verify**: only from `submitted`, and `reviewed_by` must differ from
  `entered_by` (segregation of duties, app-enforced; same-user verify →
  400). Verified records score.
- **reject**: only from `submitted`, same segregation rule, and a review
  note is mandatory. Rejected records stay visible (status `rejected`)
  with zero score weight. There is deliberately no un-reject path in
  v0.5.0 — corrections mean a new record.

Mutable-only-in-draft plus append-only everything else is what makes the
audit trail meaningful: you cannot quietly fix a record after review.

## Tamper evidence: content hash + hash-chained audit

Every mutation appends a row to `manual_experiment_audit` (action, actor,
timestamp, diff JSON, `prev_hash`, `new_hash`). `content_hash` is SHA-256
over the canonical JSON of the content fields (fixed field order, enums as
their string forms); the experiment row always carries the latest hash, and
the audit rows chain `prev → new` from the `create` genesis. Anyone with
store access can re-serialise and verify the chain; a silent UPDATE
against the file breaks it. Attachments do not change the content hash
(the audit row chains `prev == new`) — they append evidence, they don't
edit the record.

This is tamper-*evidence*, not tamper-proofing: the store admin can rewrite
history wholesale. That is the correct threat model for an embedded,
single-tenant evidence store; the R2 pack's SHA-256 artifact hashes cover
the export side.

## Scoring semantics

Verified manual records score **exactly like automated runs** — that is
the point of the review gate:

| outcome       | score | note                                        |
| ------------- | ----- | ------------------------------------------- |
| passed        | 100   | 30-day staleness decay to 75 applies too    |
| partial       | 75    | new `RunState::Partial` — distinct from "stale pass" even though the numbers coincide |
| failed        | 50    |                                             |
| inconclusive  | —     | **excluded from scoring entirely**          |

Inconclusive-excluded is a judgement call, documented here: an
inconclusive exercise produced no verdict, so any score would invent
information (0 punishes honesty; 50 rewards ambiguity; 100 is absurd).
Inconclusive records still count toward coverage's `expected`, so the gap
stays visible.

Draft/submitted records count toward coverage as *pending verification*
with zero score weight; rejected records are out of both (visible in the
register only).

Span-level aggregates (MTTR, issue stats, dashboards) exclude manual
records — they have no spans; the experiment *list* UNIONs them in with an
`origin` column and the scoring layer keys leaves by `(name, origin)`.

## Attachments: URIs only, no file storage

`evidence_attachments` links external evidence (`url`, `ticket` accepted by
the API; `file` and `log_excerpt` reserved in the schema). No file storage
in v0.5.0: blobs in DuckDB would bloat the single-writer store and raise
retention/scanning questions; a link to the wiki/ticket/CI artifact is
what auditors actually follow. `file_hash` exists so a future file feature
can pin content.

## The "acting as" caveat

There is no authentication in kronika. The API takes plain `entered_by` /
`by` / `reviewer` strings (the UI keeps an "acting as" name in
localStorage) and the store enforces the lifecycle *given those strings* —
including reviewer ≠ enterer. This is honest workflow scaffolding, not
access control: anyone can claim any name. It is the right scope for a
single-tenant local tool, and it is documented on the Manual page, here,
and in ADR 0004. When auth lands (reverse proxy, SSO), the same fields map
onto verified identities with no schema change.

## Bulk import

`POST /api/manual/import` accepts a JSON array and creates every record as
a **draft** in one transaction, recorded in `import_batches` with a shared
`batch_id` and per-record audit rows. Import deliberately does **not**
bypass attestation: each record carries its own attestation text and still
needs the submit → verify path (by a different person) before it scores. A
failed validation rolls the whole batch back — no partial imports.
