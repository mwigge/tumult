# Research findings — the analytics layer

Distilled research behind the analytics layer's product and architecture
decisions. Companion ADRs: [../adr/ADR-006-kronika-stack.md](../adr/ADR-006-kronika-stack.md), [../adr/ADR-007-ai-layer.md](../adr/ADR-007-ai-layer.md),
[../adr/ADR-008-typst-report-pipeline.md](../adr/ADR-008-typst-report-pipeline.md).

## 1. Presentation / BI landscape

What to steal, and what to position against:

- **Rill** (https://github.com/rilldata/rill) — KPI + delta + sparkline rows,
  automatic time comparisons, metrics-as-YAML semantic layer. Our
  `metrics/*.yaml` files and the `tumult-metrics` crate are directly
  modeled on Rill's metrics views.
- **Evidence** (https://github.com/evidence-dev/evidence) — report-as-code,
  scheduled email digests, sparkline tables. Our `tumult-report` digest
  model follows this.
- **Grafana** (https://grafana.com/grafana/) — trace waterfall UX, URL-encoded
  time-range/view state, annotations on charts. The waterfall and URL state
  are adopted; the "everything is a panel grid" aesthetic is not.
- **Metabase / Superset / Lightdash** — the generic-BI aesthetic this product
  positions *against*: Tumult is presentation-first, opinionated, and
  chaos-domain-specific rather than a general chart warehouse.
- **Mosaic** (https://github.com/uwdata/mosaic, TVCG'24,
  https://idl.cs.washington.edu/papers/mosaic) — state-of-the-art
  crossfiltering over DuckDB with pixel-resolution pre-aggregation. The
  project self-declares as *not production-ready*, so it is **deferred to
  Phase 2**, pinned to a specific version and wrapped behind an internal
  selection abstraction so views never depend on it directly.

## 2. Visualization library picks

- **uPlot** (https://github.com/leeoniya/uPlot) — ~50 KB, the fastest lean
  time-series renderer. Workhorse for rollup trends and KPI sparklines.
- **ECharts** (https://echarts.apache.org), tree-shaken — heatmaps and
  calendar views of experiment activity.
- **Custom span-waterfall** — the signature component; a well-understood
  build (references: https://github.com/grafana/flamegraph,
  https://github.com/jlfwong/speedscope). Owning it lets us encode
  `resilience.*` semantics (outcome color, fault annotations) directly.
- **Observable Plot** (https://github.com/observablehq/plot) — bespoke
  analytical marks.
- **Plotly — rejected**: too heavy for this UI.

## 3. Design practices

- Information hierarchy: **KPI → trend → breakdown → table**.
- Saturated color only for status (hypothesis met / deviated / failed).
- Aggregate above leaf level; always show `n`, the time window, and the
  aggregation level.
- Mute gridlines; label directly, no legends where avoidable.
- Scheduled HTML/email digests carry static server-rendered charts plus links
  back to live, parameterized views (URL-serialized state).
- WCAG 2.2 AA contrast throughout.

## 4. AI analytics

- **Realistic text-to-SQL expectations** (BIRD benchmark,
  https://bird-bench.github.io/): 60–75 % first-shot accuracy, 85–95 % with
  grounding. Grounding ROI order: **semantic layer > golden Q→SQL few-shot >
  schema linking > self-correction with execution feedback > constrained
  decoding**. The governed semantic layer (`tumult-metrics`) is therefore
  the primary grounding artifact.
- **Guardrails are mandatory** (implemented in `tumult_intelligence::sql_guard` +
  read-only connections): read-only connection, allow-listed views, single
  `SELECT` parse check, `EXPLAIN` validation, injected `LIMIT` + statement
  timeout, ≤ 3 self-correction retries, always show the generated SQL, a
  golden-set eval harness in CI, and prompt-injection caution for log content
  (log bodies are untrusted input to any prompt).
- **Narrative reporting**: compute every number deterministically; the LLM
  only verbalizes a *facts package*. Every numeral in prose must exist in the
  facts package (verified). JSON-schema-constrained digests. *Selection* of
  what matters is the hard part, not prose generation.
- **Anomaly detection**: MAD / rolling-median in SQL first; STL + robust ESD
  in a Python sidecar for flagged series; before/during/after experiment
  comparisons are the chaos-specific core. The LLM explains anomalies only
  from an evidence package with mandatory citations.
- **LLM access**: one OpenAI-compatible interface
  (`tumult_intelligence::OpenAiCompatClient`). Interactive paths → direct API /
  LiteLLM / Ollama (default); batch digests → can delegate to smedja.
- **Phase plan**: 1. NL query → 2. narrative digests → 3. anomaly
  explanation → 4. suggested insights.

## 5. Ingestion & schema

- **tumult** emits OTLP/gRPC (bare `host:4317`, no path;
  `TUMULT_OTEL_ENABLED=true`, `OTEL_EXPORTER_OTLP_ENDPOINT`, service name
  `tumult`). **smedja** emits OTLP/HTTP protobuf with `/v1/*` paths
  (`SMEDJA_OTLP_ENDPOINT`, service name `smdjad`). The analytics layer must
  accept both — hence two servers in `tumult-ingest`.
- **tumult span vocabulary**: `resilience.experiment`,
  `resilience.hypothesis.before|after`, `resilience.action|probe|rollback|load|gameday`
  plus `k8s.*`, `ssh.*`, `net.*` spans. Metrics include
  `tumult.experiments.total`, `tumult.action|probe|experiment.duration`
  histograms, `tumult.hypothesis.deviations.total`,
  `tumult.rollbacks.total`, with `resilience.*` dimensions.
- **Attribute bible**: tumult's `resilience-metadata-standard.md` v2.0.
  Promote low-cardinality, high-selectivity keys to materialized columns;
  keep dynamic keys (e.g. `resilience.baseline.probe.{name}.*`) in
  `MAP(VARCHAR, VARCHAR)` columns. Only bounded enums are metric-safe
  dimensions.
- **Alternative ingest**: the `duckdb-otlp` community extension
  (https://community-extensions.duckdb.org) can load OTLP directly, but is
  early-stage, single-writer, no WAL — pin it if ever adopted.
- **Schema**: ClickHouse-exporter-aligned wide tables
  (https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/exporter/clickhouseexporter)
  + `MAP(VARCHAR, VARCHAR)` attrs. DuckDB single-writer + read-only readers
  (tumult-analytics precedent). Lake export via
  `COPY TO Parquet PARTITION_BY date` on the roadmap.
- **Chaos reporting KPIs**: hypothesis pass rate, MTTR /
  `recovery_time_s`, MTTD, deviation rate, coverage of critical services,
  rollback success, estimate accuracy, resilience score trend, DORA/NIS2
  regulatory evidence. References: Litmus resilience score
  (https://github.com/litmuschaos/litmus), Steadybit coverage reporting
  (https://www.steadybit.com).
