# Research findings — compliance-grade reports

Distilled research behind the v0.4.0 report pipeline (`kronika-docs`,
`/api/reports/v2/*`, `/api/scores`). Companion ADR:
[adr/0003-typst-report-pipeline.md](adr/0003-typst-report-pipeline.md).

## 1. What an auditor actually needs

Across DORA, NIS2, ISO 27001 and SOC 2 resilience-testing evidence requests,
the recurring must-haves are:

- **Document control** — a unique document ID, version, classification,
  generation date, and the data-as-of date, on the cover of every artifact.
  An evidence document without provenance is rejected outright.
- **Traceability** — clause → test → run date → result → finding →
  remediation, in a single matrix an auditor can walk end to end.
- **Independence statement** — DORA Art. 24(4) expects testing to be
  designed/executed independently of the teams running the affected
  services. R2 states this explicitly.
- **Signatures** — prepared-by / approved-by lines. Even when left blank
  they signal the document is meant to enter a review workflow.
- **Tamper evidence** — every v2 artifact carries the SHA-256 of its PDF in
  the JSON meta, so the artifact filed with a regulator can be re-verified.

## 2. Report shapes (R1–R3)

- **R1 — executive digest** (CIO/board, period-based): BLUF paragraph first
  (band verdict, trend vs previous period, weakest targets, decisions
  needed), then portfolio KPIs, target score table, issues discovered vs
  fixed with MTTR, open weaknesses, outlook. Everything deterministic —
  the LLM never writes compliance prose.
- **R3 — game-day report** (per experiment run): run summary (scenario,
  hypothesis, outcome), blast radius and safety (rollback exercised?),
  full span timeline, verdict, findings (WARN/ERROR logs), rollback
  outcome, configuration appendix.
- **R2 — evidence pack** (per framework, auditor-facing): scope and
  methodology, independence, traceability matrix, test register, findings
  log, sign-off. v0.4.0 ships this as a **skeleton**: the matrix rows are
  the clause list with current evidence summary; finding/remediation
  columns are left for the compliance workflow.

### Clause numbers are indicative

The clause lists (DORA Art. 11(4)/12(2)/24–26; NIS2 Art. 21(2)(c)(f);
ISO 27001 A.5.29/30, A.8.13/14; SOC 2 A1.2/3) are compile-time constants in
`kronika_docs::builders::FRAMEWORK_CLAUSES`. **They must be verified against
the licensed framework text before any submission** — every R2 carries this
footnote twice, deliberately. Treat the skeleton as a structure to fill in,
not legal truth.

## 3. Scoring model

Gremlin-style operational scoring with freshness decay, computed
deterministically from each experiment's latest run at the as-of timestamp:

| State                        | Score |
| ---------------------------- | ----- |
| Passed (fresh, ≤ 30 days)    | 100   |
| Passed but stale (> 30 days) | 75    |
| Failed / deviated            | 50    |
| Never run                    | 0     |

Bands: **> 70 good**, **50–70 fair**, **< 50 poor**. Target scores are the
equal-weight mean of their experiments; the portfolio score is the mean of
target scores. `GET /api/scores` additionally reports the delta against the
previous equal window.

Rationale: linear and explainable — a board member can verify any number
with the table above. Known limitation: "never run" only surfaces for
experiments present in the store; kronika has no external experiment
inventory to diff against. A missing outcome log (run never completed)
counts as a failed attempt — conservative by design.

## 4. Design language

Print-first, editorial rather than dashboard-y:

- **A4**, 20 mm top/bottom, 18 mm left/right margins.
- **Inter** for body text, **Source Serif 4** for headings — both OFL,
  vendored into `assets/fonts/` and embedded in the binary so docker builds
  stay offline-reproducible.
- Running header (document title · classification) and footer (document ID
  · page x of y) on every page.
- KPI cards as light-bordered boxes, small-caps letterspaced labels.
- Tables with light horizontal rules only; numeric columns right-aligned
  with tabular figures (`number-width: "tabular"` in Typst,
  `font-variant-numeric: tabular-nums` in the HTML preview and the web UI).
- Charts are vector SVGs (Okabe–Ito palette, direct labels, no legends
  where avoidable), shared verbatim between the PDF and the HTML preview.
- Every chart caption carries "Source: kronika · data as of …".
- **Status is glyph + label, never hue alone** — outcomes, run states,
  severities and OTel status codes share one mapping (`Cell::glyph`):
  green ● good, orange ▲ warning, vermillion × bad, grey ○ unknown. Glyphs
  are restricted to ● ▲ × ○, all present in the vendored Inter.

The HTML preview (`GET /api/reports/v2/{id}/html`) is the same document
styled for the browser iframe with `@page` rules so it also prints cleanly
from the browser.
