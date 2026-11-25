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
| `npm run lint`         | ESLint with no warnings allowed, then `npm run lint:styles`             |
| `npm test`             | The unit tests (Vitest)                                                 |
| `npm run test:e2e`     | The Playwright tests                                                    |
| `npm run build:tokens` | Writes the token outputs from `tokens/` (see below)                     |

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
which the page sets. A failing test names the element, the property and the value
before and after.

A change that should look different writes the baselines again, in the
same commit:

```sh
STYLE_SNAPSHOT_UPDATE=1 npx playwright test tests/style-snapshot.spec.ts
```

## Design tokens

The colours, type, spacing, radii, border widths and layers of Krönika are
design tokens in `tokens/`, in the DTCG 2025.10 format:

- `primitive.tokens.json`: the raw colours and fonts.
- `dark.tokens.json` and `light.tokens.json`: the colour roles of each
  theme, such as `bg` and `text-dim`. Each names a raw colour. They also
  hold the series colours of charts, each 3:1 or more on the panel of its
  theme.
- `semantic.tokens.json`: the roles that are the same in each theme, such
  as `mono`, and the chart roles: the chart font and sizes, ink on a fill,
  the band and status colours and the Star Map colours.
- `page.tokens.json`: the roles only pages use, each with its own value:
  the body font, font sizes (`font-size-sm`), weight, line height, letter
  spacing, spacing, radii, border widths, layers (`z-sticky`) and the
  widths of drawers (`drawer-narrow`).
  Spacing is on a 4 px scale: `space-2` is 8 px, and the half step
  `space-1-5` is 6 px.
- `docs.tokens.json`: the colours of the docs site and its landing page,
  under `docs`. Most name a light role; a few are their own, such as the
  copper of the logo and the darker code colours. The GameDay terminal
  of the landing page names the dark roles, as it is dark on purpose.
- `tumult.resolver.json`: the sets and the two themes. Dark is the default.

`npm run build:tokens` writes `src/lib/tokens.css`, the custom properties
the pages use (`--bg`, `--space-2`), and `src/lib/tokens.gen.ts`, the
colours, font and sizes for the charts: ECharts draws on a canvas and
cannot read a custom property. It also writes the colours of the CLI HTML
report, `tumult-cli/src/commands/report/html/tokens.css`: light, and dark
when the system asks for it. Jekyll builds the docs site and cannot run
the command, so it also writes the colour variables of the docs theme,
`docs/_sass/color_schemes/_tumult-tokens.scss`, and the colours of the
landing page, `docs/assets/css/tokens.css`. All are committed. A unit
test and CI fail when they are not what the token files give, so change a
token file and run the command, never the outputs.

Chart code takes these values from `src/lib/chart-theme.ts`. The theme
it registers sets the axis, grid, legend and tooltip colours and the
font, so an option leaves them out. `chart-code.test.ts` fails on a hex
colour or a font size written in script code, and on a second observer
of the theme: `followTheme` in `src/lib/echarts.ts` is the one.

`npm run lint:styles` fails on a raw colour, size or spacing in the styles
of `src/` that `style-literals-allow.txt` does not hold; use a token instead.
That list holds each raw value left today by file, kind and value, and the
lint fails on a line that matches nothing, so the list only shrinks.

## Theme

The theme button keeps the choice (`dark`, `light` or `auto`) in
`localStorage['tumult.theme']` and puts it on `<html data-theme>`. The app
has no server render, so `app.html` loads `static/early-theme.js` in its
head: it puts the saved theme there before the first paint, and the layout
starts from it. When the script did not run, the layout reads the saved
theme itself, so it is never lost. A user who chose light never sees the
dark default first.
It is a file and not inline code, so that a strict content security policy
can allow it. `tests/first-paint.spec.ts` checks the first paint.

## Shared classes

`src/lib/theme.css` holds, once, the rules that many pages use:

- Buttons: `.btn`, with `primary`, `danger` and `add`.
- Text: `.dim` and `.note` (faint and small), `.hint`, `.label-caps` (the
  small capital name above a value), `.ts` (the time of an audit row) and
  `.sr-only` (for a screen reader only).
- Parts: the meta tile (`.panel.meta`), filter chips (`.chip`), the open
  row of a table (`.detail-row`) and `.actions`.
- Tables: a row header (`th scope="row"` in `table.data`) reads as a cell.
  It wraps, as a cell does.
- Rows that open: a row that opens on a click (`tr.clickable`) has a
  link (`.row-link`) or a button (`.row-button`) in the cell that names
  it, so that the keyboard opens it too. Both look as the text of the
  cell. A button that opens a row in place says so with `aria-expanded`.
  `onRow` in `src/lib/rows.ts` is the click handler of the row.
  `src/lib/keyboard-scan.test.ts` fails on a row with a click handler and
  no link or button.
- Forms: `.form`, `.row2` and the list editor `.grid-rows`.

A page uses the class and keeps no copy of the rule in its own style
block. `src/lib/theme-rules.test.ts` fails on a copy, and on a `:global()`
rule in the Manual page or its components.

## Badges

A badge is `src/lib/components/Badge.svelte`. Give it a `tone` and a
`label`, or a `kind` and a `value`:

```svelte
<Badge tone="warn" label="rollback pending" />
<Badge kind="finding" value={risk.severity} />
```

`src/lib/tone.ts` holds one table per kind, such as `status`, `finding`
and `verdict`, so a word of a kind has one tone on every page. A word that
its table does not hold is neutral. `src/lib/badge-scan.test.ts` fails on a badge
class or a tone that is worked out anywhere else.

## Page parts

`src/lib/components/PageHeader.svelte` writes the head of every page: a
link back (`.back`), the title, a sub line, and then the badges and the
controls the page puts in it. `src/lib/page-head.test.ts` fails on a page
head written by hand.

The title also names the browser tab, as "Runs · Krönika", through
`Title.svelte`. A page with no head, such as the login page, uses `Title`
itself. Every field has a name: a `<label>` around it, or an `aria-label`
where the page has no room for one. `src/lib/names-scan.test.ts` fails on
a page with no title and on a field with no name.

`src/lib/components/Panel.svelte` is the box of one part of a page, with
its title in an `h2`. With `variant="meta"` it is the stat tile: the title
is the small capital name above the value.

`src/lib/components/State.svelte` is a part of a page with no data to show:

- `kind="empty"`: a short message. An `action` adds a link to what the
  reader can do next.
- `kind="error"`: the failure, as an alert, so a screen reader says it at
  once.
- `kind="loading"`: with a `height` in px, a busy block with the hidden
  word "Loading" for a screen reader. With no height, it is its message,
  such as "Scanning…".

```svelte
<State kind="error">Failed to load runs: {error}</State>
<State kind="loading" height={240} />
```

`src/lib/state-scan.test.ts` fails on a `.state` or `.skeleton` written by
hand, and on a loading block with an inline height.

## Segmented controls and tabs

A group of buttons where one is chosen, such as a time range or a
filter, is `src/lib/components/Segmented.svelte`. It is a named group,
and each button says whether it is pressed (`aria-pressed`):

```svelte
<Segmented label="Period" options={PERIODS} value={period} onchange={setPeriod} />
```

A switch between the panels of a page is `Tabs.svelte`, with its panel
in `TabPanel.svelte` of the same `id`. The arrow keys, Home and End move
to a tab and select it. The page keeps the tab in its URL: `?tab=` on
the Manual page and `?view=` on the Scores page. `src/lib/segmented-scan.test.ts`
fails on a page that draws a group of buttons itself, and on a group with
a button that does not say whether it is pressed.

## Drawers

A drawer is `src/lib/components/Drawer.svelte`, on a native `<dialog>`.
Give it the id of the heading that names it, and an `onclose` that
removes it:

```svelte
<Drawer labelledby={title} {onclose} backdrop>
    <div class="title" id={title}>{span.span_name}</div>
</Drawer>
```

- A drawer is modal by default: it lies over the right edge of the
  window, `narrow` (480 px) or `wide` (620 px). With `backdrop` it shades
  the page, and a click on the shade closes it.
- With `modal={false}` it is a panel in the flow of the page, such as the
  drawer of the Star Map.
- Escape closes it. When it opens, the focus goes to its first control,
  or to its heading with `focus="heading"`. When it closes, the focus
  goes back to the element that opened it.

`src/lib/components/drawers.test.ts` fails on a drawer built any other
way: a style with `position: fixed` or a dialog in another file.

## Stack

| Choice                                           | Why                                                                                                                               |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------- |
| SvelteKit 3 and Svelte 5 (runes), static adapter | A small runtime, and routes that follow the drill-down from a list to one run. A `200.html` fallback serves the client routes.    |
| ECharts, tree-shaken (`src/lib/echarts.ts`)      | Time series, heatmaps and donuts with zoom and tooltips; only the chart types in use ship                                         |
| An own span waterfall (`Waterfall.svelte`)       | The span tree with status colours and a drawer with attributes, events and logs. It reads the `resilience.*` attributes directly. |
| Plain CSS (`src/lib/theme.css`)                  | The base styles, on the design tokens of a dark theme and a light theme. No UI framework in the embedded build.                   |

## Design rules

- Each page goes from a row of key numbers, to a trend, to a table, to one
  run.
- A panel names the time window it shows, and says so when it has no data.
- Filters live in the URL, so a view is a link you can share.
