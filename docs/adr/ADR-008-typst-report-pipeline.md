---
title: "ADR-008: Typst Report Pipeline"
parent: Architecture Decisions
nav_order: 8
---

# ADR-008: Typst Report Pipeline

- Status: accepted (live in v0.4.0)
- Date: 2026-07-28

## Context

v0.4.0 adds compliance-grade reports (R1 executive digest, R3 game-day
report, R2 evidence pack) that must hold up as filed evidence: A4 layout,
document control, page headers/footers, vector charts, reproducible output.
The v1 digest pipeline (`tumult-report`) renders HTML strings by hand —
fine for a browser digest, not for a document an auditor prints.

Options considered:

1. **HTML → headless-Chromium PDF** (weasyprint/wkhtmltopdf/puppeteer) —
   heavy external runtime, painful in the offline-reproducible docker
   image, pagination control is poor.
2. **LaTeX** — unmatched print quality, but a multi-GB toolchain and a
   template language nobody on the project wants to debug at 2 a.m.
3. **Typst embedded as a Rust library** — modern typesetter, pure-Rust
   (`typst` + `typst-pdf` crates), compiles in-process in milliseconds,
   deterministic, no external runtime.

## Decision

**Embed Typst (0.15) in the `tumult-compliance` crate, behind a
renderer-agnostic content model.**

- `ReportDoc` (`DocMeta` + `Vec<Block>`) is the single source of truth.
  Builders (`tumult_compliance::builders`) query the store and produce it;
  renderers consume it. Adding R4/R5 or a notebook composer later touches
  only builders.
- Two renderers: `typst_pdf` (Typst markup → in-memory `World` → PDF) and
  `html` (print-styled preview with `@page` rules for browser printing).
  Charts are vector SVGs from one shared `svg` module, so both outputs are
  pixel-consistent.
- **Fonts are vendored** (OFL Inter + Source Serif 4 variable TTFs in
  `assets/fonts/`, `include_bytes!` into the binary). The docker image
  needs no font packages and the PDF is byte-stable across hosts (modulo
  Typst version bumps).
- Chart SVGs reach the Typst world as virtual files (`/charts/cN.svg`) on
  an in-memory `World` implementation; `today()` returns `None` — every
  date is baked into the markup as a string from the document's own
  data-as-of timestamp, keeping renders reproducible.
- Artifacts are persisted as `{doc_id}.pdf` / `.html` / `.json` under
  `reports/v2/`; the JSON meta carries the PDF's SHA-256 for tamper
  evidence. Document IDs are `KRK-<code>-<yyyymmdd>-<hash6>`.
- Scoring (`tumult_compliance::scoring`) lives in the same crate because both
  the executive digest and `/api/scores` consume it.

## Consequences

- Docker image builds get slower (the Typst dependency tree is large) and
  the binary grows by the four embedded fonts (~2 MB). Accepted: offline
  reproducibility beats image size here.
- We are pinned to the Typst crate API (0.15: `LazyHash`, `main() ->
  FileId`, `Bytes::new`, `LibraryExt`); upgrades are deliberate chores.
- Typst markup injection is handled by escaping all user/store strings in
  `markup::esc`; the SQL side reuses the established single-quote-doubling
  convention.
