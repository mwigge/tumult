# kronika-web

The Krönika analytics UI — a static SvelteKit SPA embedded into `kronikad`
(via `rust-embed`) and served on the daemon's HTTP port alongside `/api/*`.

## Build

```sh
npm ci
npm run build   # writes build/ — required before `cargo build -p kronikad`
```

`kronikad` embeds `web/build/` at **compile time**, so a local `cargo build`
fails until the UI has been built once. The Dockerfile handles this ordering
with a dedicated node stage. `npm run check` runs svelte-check (0 errors is
the bar); `npm run dev` starts vite with `/api` proxied to
`http://127.0.0.1:4318`.

## Stack, and why

| Choice | Why |
|---|---|
| **SvelteKit 2 + Svelte 5 (runes), adapter-static SPA** | Small runtime, file-based routing that maps 1:1 onto the drill-down hierarchy (overview → experiment → span drawer). `ssr = false`, prerendered shells plus a `200.html` fallback served by kronikad for client-side routes. |
| **ECharts, tree-shaken** (`echarts/core`, `web/src/lib/echarts.ts`) | Time-series bar, calendar heatmap (experiments/day) and donut (fault breakdown) with zoom/tooltip — only the registered chart types ship. |
| **Custom span waterfall** (`Waterfall.svelte`) | The signature piece: ruler, indented span tree, status-coloured duration bars (Ok emerald / Error red / Unset slate), click → drawer with attributes, events and correlated logs. Owning it lets us encode `resilience.*` semantics directly. |
| **Hand-rolled CSS** (`lib/theme.css`) | Near-black Grafana-caliber theme; saturated colour is reserved for status and data. No UI framework — fewer moving parts in the embedded build. |

Explicitly rejected: Tailwind/component kits (build weight for no benefit at
this size), Plotly (too heavy), generic-BI dashboard aesthetics.

## Design language

- Hierarchy: **KPI row (value · delta vs previous window · sparkline) →
  trend/heatmap → leaderboard → drill-down table → waterfall.**
- Aggregate above leaf level; panels always show the window they describe and
  render honest empty states (e.g. target coverage before targets are
  annotated, MTTR before recovery times are emitted).
- Filters are URL-synced everywhere so a view is a shareable link.

## Pages

- `/` — Overview: KPI cards, experiments-per-day (bar 24h / calendar heatmap
  7d+), fault donut, target-system leaderboard.
- `/experiments` — filterable run list (range, outcome, fault, target,
  free-text); newest first.
- `/experiments/[id]` — trace waterfall + span drawer, correlated logs,
  metric points.
- `/ask` — NL → guarded SQL → result table; golden answers work without an
  LLM, otherwise graceful setup hint (`{configured:false}` from the API).
- `/reports` — digests written by the daemon's `KRONIKA_REPORT_INTERVAL`
  scheduler.
