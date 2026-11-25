# Krönika web UI

The web UI of Tumult: a static SvelteKit app that `tumultd` embeds and
serves on its HTTP port, next to `/api/*`. The npm package is
`kronika-web`. Each page is described in
[Krönika pages](../docs/reference/kronika-pages.md).

![The Overview page of Krönika](../docs/images/3.0/screenshots/overview.png)

## Build

```sh
npm ci
npm run build   # writes build/, which tumultd embeds
```

`tumultd` embeds `web/build/` when it compiles, so `cargo build -p tumultd`
fails until the UI is built once. The Dockerfile builds the UI in its own
node stage first.

| Command                | What it does                                                            |
| ---------------------- | ----------------------------------------------------------------------- |
| `npm run dev`          | Starts vite, with `/api` sent to a `tumultd` on `http://127.0.0.1:4318` |
| `npm run check`        | svelte-check; the bar is 0 errors                                       |
| `npm run lint`         | ESLint with no warnings allowed                                         |
| `npm test`             | The unit tests (Vitest)                                                 |
| `npm run test:e2e`     | The Playwright tests                                                    |
| `npm run build:tokens` | Writes the token outputs from `tokens/` (see below)                     |

## Style snapshot

`tests/style-snapshot.spec.ts` opens every page with a mocked API, in the
light and the dark theme, 1440 and 390 px wide. It reads the computed
styles of each visible element and compares them with the baseline in
`tests/style-snapshots/`, one file per page, theme and width. It reads no
size or position, so the fonts of a host do not change the result. A
failing test names the element, the property and the value before and
after.

A change that should look different writes the baselines again, in the
same commit:

```sh
STYLE_SNAPSHOT_UPDATE=1 npx playwright test tests/style-snapshot.spec.ts
```

## Design tokens

The colours of Krönika are design tokens in `tokens/`, in the DTCG 2025.10
format:

- `primitive.tokens.json`: the raw colours and fonts.
- `dark.tokens.json` and `light.tokens.json`: the colour roles of each
  theme, such as `bg` and `text-dim`. Each names a raw colour.
- `semantic.tokens.json`: the roles that are the same in each theme, such
  as `mono` and the colours of chart series.
- `tumult.resolver.json`: the sets and the two themes. Dark is the default.

`npm run build:tokens` writes `src/lib/tokens.css`, the custom properties
the pages use (`--bg`, `--text-dim`), and `src/lib/tokens.gen.ts`, the same
values for the charts. Both are committed. A unit test and CI fail when
they are not what the token files give, so change a token file and run
the command, never the outputs.

## Stack

| Choice                                           | Why                                                                                                                               |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------- |
| SvelteKit 3 and Svelte 5 (runes), static adapter | A small runtime, and routes that follow the drill-down from a list to one run. A `200.html` fallback serves the client routes.    |
| ECharts, tree-shaken (`src/lib/echarts.ts`)      | Time series, heatmaps and donuts with zoom and tooltips; only the chart types in use ship                                         |
| An own span waterfall (`Waterfall.svelte`)       | The span tree with status colours and a drawer with attributes, events and logs. It reads the `resilience.*` attributes directly. |
| Plain CSS (`src/lib/theme.css`)                  | The base styles, on the colour tokens of a dark theme and a light theme. No UI framework in the embedded build.                   |

## Design rules

- Each page goes from a row of key numbers, to a trend, to a table, to one
  run.
- A panel names the time window it shows, and says so when it has no data.
- Filters live in the URL, so a view is a link you can share.
