# Krönika web UI

The web UI of Tumult: a static SvelteKit app that `tumultd` embeds and
serves on its HTTP port, next to `/api/*`. The npm package is
`kronika-web`. Each page is described in
[Krönika pages](../docs/reference/kronika-pages.md). How the UI looks and
behaves, its components and the checks a change must pass are in the
[UI standard](../docs/reference/ui-standard.md).

![The Overview page of Krönika](../docs/images/3.0/screenshots/overview.png)

## Build

```sh
npm ci
npm run build   # writes build/, which tumultd embeds
```

`tumultd` embeds `web/build/` when it compiles, so `cargo build -p tumultd`
fails until the UI is built once. The Dockerfile builds the UI in its own
node stage first.

| Command                | What it does                                                                               |
| ---------------------- | ------------------------------------------------------------------------------------------ |
| `npm run dev`          | Starts vite, with `/api` sent to a `tumultd` on `http://127.0.0.1:4318`                    |
| `npm run check`        | svelte-check; the bar is 0 errors                                                          |
| `npm run lint`         | ESLint with no warnings allowed, then `npm run lint:styles`                                |
| `npm test`             | The unit tests (Vitest)                                                                    |
| `npm run test:e2e`     | The Playwright tests, on pages with a mocked API                                           |
| `npm run test:live`    | The Playwright tests on a real `tumultd`; `TUMULTD_BIN` and `TUMULT_BIN` name the binaries |
| `npm run test:report`  | The HTML report of `tumult report` in a browser; `TUMULT_BIN` names the binary             |
| `npm run build:tokens` | Writes the token outputs from `tokens/`                                                    |
| `npm run build:report` | Builds the chart script of the HTML report, which `tumult-cli` embeds                      |

## Where things are

| Path                           | What it holds                                                    |
| ------------------------------ | ---------------------------------------------------------------- |
| `tokens/`                      | The design tokens, in the DTCG 2025.10 format                    |
| `scripts/build-tokens.mjs`     | The generator of the token outputs                               |
| `scripts/style-literals.mjs`   | The style lint, with its allow list `style-literals-allow.txt`   |
| `src/lib/theme.css`            | The base styles, and the classes that many pages use             |
| `src/lib/components/`          | The components of the UI standard, and the parts of single pages |
| `src/lib/chart-theme.ts`       | The theme of the charts, from the tokens                         |
| `tests/`                       | The Playwright tests on mocked pages, and the style snapshot     |
| `live-tests/`, `report-tests/` | The Playwright tests on a real daemon and on the HTML report     |

## Design tokens

Change a token file in `tokens/`, run `npm run build:tokens` and commit
the token file and the outputs together. A unit test and CI fail when an
output is not what the token files give, so never edit an output. The
command writes:

- `src/lib/tokens.css`: the custom properties the pages use, such as
  `--bg` and `--space-2`.
- `src/lib/tokens.gen.ts`: the colours, font and sizes of the charts.
  ECharts draws on a canvas and cannot read a custom property.
- `tumult-cli/src/commands/report/html/tokens.css`: the colours of the CLI
  HTML report, light, and dark when the system asks for it.
- `docs/_sass/color_schemes/_tumult-tokens.scss` and
  `docs/assets/css/tokens.css`: the colours of the docs site and its
  landing page. Jekyll builds the site and cannot run the command.
- `docs/images/3.0/tokens/`: the colour swatches and the picture of the
  scale on the UI standard page.

The paths of the last three are from the root of the repository.

## Charts

Chart code takes its values from `src/lib/chart-theme.ts`. The theme it
registers sets the axis, grid, legend and tooltip colours and the font, so
an option leaves them out. `src/lib/chart-code.test.ts` fails on a hex
colour or a font size written in script code, and on a second observer of
the theme: `followTheme` in `src/lib/echarts.ts` is the one.

## Components

The components are in `src/lib/components/`. The UI standard says when to
use each. A call of each:

```svelte
<Badge kind="finding" value={risk.severity} />
<Badge tone="warn" label="rollback pending" />
<PageHeader title="Runs" sub="newest first">…badges and controls…</PageHeader>
<Panel title="Runs per day">…</Panel>
<MetaRow><Panel variant="meta" title="Queued">…</Panel>…</MetaRow>
<Segmented label="time range" options={ranges} {value} {onchange} />
<Tabs id="manual" label="manual sections" options={TABS} value={tab} onchange={setTab} />
<TabPanel id="manual" value={tab}>…</TabPanel>
<Drawer labelledby={title} {onclose} backdrop>…</Drawer>
<State kind="error">Failed to load runs: {error}</State>
<State kind="loading" height={240} />
```

- `Badge`: `src/lib/tone.ts` holds one table of tones per kind, such as
  `status`, `finding` and `verdict`. A word its table does not hold is
  neutral.
- `PageHeader`: `back` adds the link to the page above. A page with no
  head, such as the login page, names the browser tab with `Title`.
- `Panel`: `scroll` lets a wide table scroll inside the panel. A panel
  that a page writes itself as `.panel` takes `data-scroll` for the same.
- `Tabs`: the page keeps the tab in its URL, as `?tab=` on the Manual page
  and `?view=` on the Scores page.
- `Drawer`: modal by default, `narrow` or `wide`, with `backdrop` to shade
  the page. With `modal={false}` it is a panel in the flow of the page, as
  on the Star Map. `focus="heading"` gives the focus to its heading.
- `State`: `action` adds a link after an empty message. A loading state
  with no `height` is its message, such as "Scanning…".

## Shared classes

`src/lib/theme.css` holds, once, the rules that many pages use. A page
uses the class and keeps no copy of the rule in its own style block:
`src/lib/theme-rules.test.ts` fails on a copy, and on a `:global()` rule in
the Manual page or its components.

- Buttons: `.btn`, with `primary`, `danger` and `add`.
- Text: `.dim` and `.note`, `.hint`, `.label-caps` (the small capital name
  above a value), `.ts` (the time of an audit row) and `.sr-only` (for a
  screen reader only).
- Parts: the meta tile (`.panel.meta`), filter chips (`.chip`), the open
  row of a table (`.detail-row`) and `.actions`.
- Tables: a row header (`th scope="row"` in `table.data`) reads and wraps
  as a cell.
- Rows that open: `tr.clickable`, with a link (`.row-link`) or a button
  (`.row-button`) in the cell that names it. `onRow` in `src/lib/rows.ts`
  is the click handler of the row. A button that opens a row in place says
  so with `aria-expanded`.
- Forms: `.form`, `.row2` and the list editor `.grid-rows`.
- The skip link `.skip-link`, and the focus ring of fields.

## Theme

The theme button keeps the choice (`dark`, `light` or `auto`) in
`localStorage['tumult.theme']` and puts it on `<html data-theme>`. The app
has no server render, so `app.html` loads `static/early-theme.js` in its
head: it puts the saved theme there before the first paint, and the layout
starts from it. When the script did not run, the layout reads the saved
theme itself, so it is never lost. A user who chose light never sees the
dark default first. It is a file and not inline code, so that a strict
content security policy can allow it. `tests/first-paint.spec.ts` checks
the first paint.

## Style snapshot

`tests/style-snapshot.spec.ts` opens every page with a mocked API, in the
light and the dark theme, 1440 and 390 px wide. It also opens states that a
click or the data shows, such as the Manual tabs with data, an open row, the
span drawer, filter chips and a page whose data failed or is on its way,
1440 px wide only (`STATES` in `tests/style-snapshot-pages.ts`). It reads
the computed styles of each visible element and compares them with the
baseline in `tests/style-snapshots/`, one file per page or state, theme and
width. It reads no size or position, so the fonts of a host do not change
the result. The one size it reads is the height of a busy loading block,
which the page sets. A failing test names the element, the property and the
value before and after.

A change that should look different writes the baselines again, in the
same commit:

```sh
STYLE_SNAPSHOT_UPDATE=1 npx playwright test tests/style-snapshot.spec.ts
```

## Stack

| Choice                                                                       | Why                                                                                                                                                                    |
| ---------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| SvelteKit 3 and Svelte 5 (runes), static adapter                             | A small runtime, and routes that follow the drill-down from a list to one run. A `200.html` fallback serves the client routes.                                         |
| ECharts, tree-shaken (`src/lib/echarts.ts`)                                  | Time series, heatmaps and donuts with zoom and tooltips; only the chart types in use ship                                                                              |
| An own span waterfall (`Waterfall.svelte`)                                   | The span tree with status colours and a drawer with attributes, events and logs. It reads the `resilience.*` attributes directly.                                      |
| Own components on design tokens (`src/lib/components/`, `src/lib/theme.css`) | Svelte components with scoped CSS and native `<dialog>`, and plain CSS for the shared rules, in a dark theme and a light theme. No UI framework in the embedded build. |
