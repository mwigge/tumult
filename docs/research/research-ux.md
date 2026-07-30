# Research findings — UX polish (v0.4.0)

What the UX review surfaced, what v0.4.0 adopted, and what is deferred with
reasons. Companion: [research-compliance.md](research-compliance.md).

## Adopted

- **Tabular numerals everywhere numbers align** — `font-variant-numeric:
  tabular-nums` on `table.data` and `.mono` in `theme.css`, plus
  `number-width: "tabular"` in the Typst tables. Cheap, immediately
  improves scanning of score/duration columns.
- **Reports as documents, not dashboards** — the rebuilt Reports page
  leads with the compliance artifacts (document ID, type, SHA-256, preview
  iframe, PDF download) and demotes the v1 metric digests to "Quick
  digests". This matches how the artifacts are actually consumed: preview
  in the browser, file the PDF.

## Deferred (with reasons)

- **⌘K command palette** — real value once the page count grows, but it
  wants a global search backend (experiments/traces/logs by id) to be more
  than a nav menu; bundling a half-feature into a release about document
  quality would dilute it. Roadmap.
- **Brush/range-select on Overview charts** — echarts `dataZoom` brush is
  easy to enable but needs coordinated re-querying semantics across every
  panel to not be misleading; deferred until the selection model is
  designed (likely alongside Mosaic crossfiltering, Phase 2).
- **BubbleUp-style "explain this spike"** — needs a designed drill-down
  contract (dimension ranking over the store); roadmap.
- **Notebook-style ad-hoc reports** (user-composed blocks) — the v2 content
  model (`ReportDoc`/`Block`) is built to allow this later; not exposed in
  v0.4.0.
- **R4/R5 templates** (service-level deep-dive, regulatory run-log export)
  — pending real auditor feedback on R1–R3.
- **Triage inbox for open weaknesses** — the R1 open-weaknesses table is
  the read model for this; the workflow itself is roadmap.
