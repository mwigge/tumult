# kronika-web

Presentation-first web UI for kronika — the chronicle of your resilience work.

> **Status: skeleton.** This directory is intentionally hand-written — no
> `npm create` / `npm install` has been run yet (no network guarantee at
> scaffold time). `package.json` is complete and pinned; run `npm install`
> when you're ready to bring the UI up.

## Stack, and why

| Choice | Why |
|---|---|
| **SvelteKit** | Small runtime, fast first paint, file-based routing that maps 1:1 onto the drill-down hierarchy (fleet → experiment → operation/span). Compiled components keep the dashboard snappy over large rollups. |
| **uPlot** (~50 KB) | The fastest lean time-series library available. Workhorse for rollup trends (hour/day/week) and KPI sparklines. |
| **ECharts (tree-shaken imports)** | Only for what uPlot doesn't do well: heatmaps (experiment × time) and the calendar view of resilience work. Imported per-chart to keep the bundle lean. |
| **Custom span-waterfall component** | The signature piece of the product: a Grafana-style trace waterfall for experiment traces (`resilience.experiment` → hypothesis/action/probe/rollback spans), purpose-built on our DuckDB span model. References: `@grafana/flamegraph`, speedscope — the component is well-understood, and owning it lets us encode `resilience.*` semantics (outcome color, fault annotations) directly. |
| **Observable Plot (planned)** | Bespoke marks for one-off analytical views. |
| **mosaic-core — deferred to Phase 2** | State-of-the-art crossfiltering over DuckDB (TVCG'24, pixel-resolution pre-aggregation), but self-declared not production-ready. Pinned and wrapped behind an internal *selection abstraction* when adopted, so views never depend on Mosaic directly. |
| **Perspective (optional)** | Ad-hoc exploration widget only; never the core layout. |

Explicitly rejected: Plotly (too heavy), generic-BI dashboard aesthetics
(Metabase/Superset/Lightdash) — kronika is presentation-first, not a chart
warehouse.

## Design language

- Hierarchy: **KPI row → rollup trend → dimension leaderboard → drill-down table.**
- Saturated color is reserved for status (hypothesis met / deviated / failed).
- Aggregate above leaf level; every panel shows `n`, the time window, and the
  aggregation level.
- Muted gridlines, direct labels, no chartjunk.
- All view state (time range, filters, selected experiment) is serialized in
  the URL — Grafana-style, so every view is shareable.
- WCAG 2.2 AA contrast.

## Layout (see `src/routes/+page.svelte` for the static mock)

```
┌──────────────────────────────────────────────────────┐
│ KPI row: pass rate · MTTR · deviation rate · coverage │
├───────────────────────────────┬──────────────────────┤
│ Rollup trend (uPlot)          │ Dimension leaderboard │
├───────────────────────────────┴──────────────────────┤
│ Drill-down: fleet → experiment → operation/span       │
│ (span-waterfall lives here)                           │
└──────────────────────────────────────────────────────┘
```
