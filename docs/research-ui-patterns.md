# Research: observability UI patterns (v0.6.0)

Status: living catalog — v0.6.0 implements patterns 1, 2 and 8 (experiment
overlays, click-to-filter, correlation legs); deferrals are marked per
pattern. Effort: S < 1 day, M = days, L = a release of its own.

Fifteen interaction patterns recur across mature observability UIs. Each
entry: who does it best, why it works, kronika's status, and effort. The
catalog exists so UI work is picked by value, not by what's easiest to
build next.

## 1. Facet click-to-filter
- **Best:** SigNoz, Datadog. Clicking an attribute value offers
  "filter for ⊕ / filter out ⊖" and applies it without retyping.
- **Why:** the detail view is where curiosity starts; making the answer to
  "show me only/none of these" one click removes the query-syntax tax for
  the most common refinement.
- **kronika:** **implemented v0.6.0** — hover actions on attribute tables
  in the log detail rows and span drawer, writing the page's existing
  filter state into URL params. Effort **S**.

## 2. Change/experiment markers on charts
- **Best:** Lightstep (change markers), Grafana (annotations). Deploys and
  incidents drawn as bands/lines over time series.
- **Why:** the first question on any anomaly is "what changed?" — the
  chart should answer it instead of requiring a second tool. For kronika
  the "change" is the experiment run itself.
- **kronika:** **implemented v0.6.0** — experiment runs overlapping the
  visible window render as outcome-coloured `markArea` bands + a start
  `markLine` on Overview and Metrics charts (`GET
  /api/experiments/windows?from&to`); clicking a band opens the run.
  Effort **M**.

## 3. Persistent URL time context
- **Best:** Grafana. Range in the URL on every page, shareable by copy.
- **Why:** links that reproduce exactly what the sender saw are the
  cheapest collaboration feature there is.
- **kronika:** done — `?range=` (and page-specific filters) live in the
  URL everywhere; pattern 1 extends it. Effort: done.

## 4. Structured query-builder chips
- **Best:** Honeycomb, Datadog. Filters as removable chips, not raw text.
- **Why:** chips make active filters visible and individually undoable;
  text queries hide their own state.
- **kronika:** partial — filter state exists as discrete URL params (chip
  semantics) but renders as plain controls, not chips. Effort **M**;
  candidate for a filter-bar component once a third page needs one.

## 5. BubbleUp-style anomaly overlay
- **Best:** Honeycomb BubbleUp — "which attributes explain this selection?"
- **Why:** turns "something looks wrong" into "here's the dimension that
  differs", skipping manual group-by roulette.
- **kronika:** **deferred (L)** — needs per-dimension deviation scoring
  over the attribute maps; a feature of its own, not a UI tweak.

## 6. Severity volume histogram with brush
- **Best:** Datadog logs, Kibana. Stacked severity bars over time;
  dragging selects a sub-window.
- **Why:** the volume shape tells you where to look before you read a
  single line; brushing makes the histogram itself the time picker.
- **kronika:** histogram half done (stacked severity volume on the Logs
  page); **brush zoom not implemented**. Effort **S** for the brush.

## 7. Sentry triage inbox
- **Best:** Sentry. Issues as an inbox: archive, assign, regress.
- **Why:** findings arrive faster than they're fixed; an explicit triage
  queue prevents silent accumulation.
- **kronika:** **deferred → v0.7.0** — open weaknesses already surface in
  R1 and on the Overview; the missing piece is per-finding state
  (ack/fixed/snoozed), which is a store schema change plus UI. Effort
  **M/L**.

## 8. Cross-signal correlation links (+ experiment-run leg)
- **Best:** Grafana exemplars, Tempo↔Loki links. One click from a log to
  its trace, from a span to its logs.
- **Why:** signals are one system observed three ways; navigation should
  treat them as such. Kronika adds a fourth leg most tools lack: the
  *experiment run* that caused the signal.
- **kronika:** **implemented v0.6.0** — log rows with `trace_id` link to
  `/traces/:id` (already present), span detail links to the overlapping
  experiment run; experiment links already existed on logs and traces.
  Effort **S**.

## 9. Saved views / "add to report"
- **Best:** Datadog saved views, Grafana "add to dashboard".
- **Why:** the view someone carefully filtered is usually the view they
  want in the report; capturing it in place beats re-describing it.
- **kronika:** not implemented — R1/R2/R3 generate from templates only.
  Effort **M**; natural pairing with pattern 4 chips.

## 10. Unified service page
- **Best:** Datadog APM service page, Grafana's service-oriented views.
- **Why:** "tell me about X" should be one destination aggregating every
  signal for X, not five filtered pages.
- **kronika:** closest analogue is the experiment detail page (spans +
  logs + scores for one run). A per-target-service page is unbuilt; target
  metadata is still sparse from tumult. Effort **M**, blocked on richer
  target tagging.

## 11. Live tail
- **Best:** Datadog live tail, `tail -f` made social.
- **Why:** during an active experiment you watch the stream, not history;
  polling with refresh buttons is a poor substitute.
- **kronika:** not implemented (pages poll on filter change only). Effort
  **M** (SSE or poll-interval on the logs page; ingest is already
  near-real-time).

## 12. Dashboard grammar
- **Best:** Grafana's panel grid, Vega-Lite as the extreme.
- **Why:** a grammar lets users compose arbitrary views — at the price of
  a builder UI and per-user dashboard sprawl.
- **kronika:** **deliberately skipped** — opinionated fixed pages + the
  YAML semantic layer are the product's stance; a builder contradicts
  "compliance tool with one canonical story". Recorded so the decision is
  explicit, not absent.

## 13. Density controls
- **Best:** Datadog/Grafana comfortable-vs-compact toggles; GitHub's
  density setting.
- **Why:** triage wants maximum rows per screen; review wants breathing
  room — one density serves neither.
- **kronika:** not implemented (fixed compact tables). Effort **S**; low
  priority while the audience is small.

## 14. Keyboard navigation
- **Best:** Linear, Sentry (`j/k`, `/` to search).
- **Why:** triage is repetitive; keyboard throughput beats mouse
  throughput an order of magnitude in.
- **kronika:** not implemented beyond browser defaults. Effort **M**
  (needs a focus model across list pages). Pairs naturally with pattern 7.

## 15. AI as editable query, not chat
- **Best:** the honest lesson of every "chat with your data" feature: the
  chat is a means; the *generated query you can see and edit* is the end.
- **Why:** an opaque answer can't be trusted or adjusted; a visible query
  is verifiable, teaches the syntax, and hands control back.
- **kronika:** done in spirit — `/api/ask` returns the SQL it ran and the
  Ask page shows it. A "open this query in Logs/Traces" hand-off is the
  missing leg. Effort **S/M**.

## Selection rationale for v0.6.0

Patterns 1, 2 and 8 were picked because they compound: overlays make
experiments visible on every chart, click-to-filter makes every attribute
actionable, and correlation legs connect what those actions find. All
three lean on existing state (URL filters, the `experiment_runs` view, the
waterfall drawer) rather than new subsystems — the highest
value-per-table-row of the catalog. The two deferrals (5 BubbleUp, 7
triage inbox) are the two patterns that require new scoring/state in the
store, not just UI.
