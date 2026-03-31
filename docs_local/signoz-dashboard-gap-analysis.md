# SigNoz Dashboard Gap Analysis — Tumult Platform
**Date:** 2026-03-31  
**Phase coverage:** 0–5 (complete), 6 (runner hardening, in-progress), 8–9 (planned)

---

## Executive Summary

This analysis audits the visualisation coverage of Tumult's observable stack against:
1. All signals emitted by the OTel Collector (`docker/otel-collector-e2e.yaml`)
2. All custom spans and metrics from Tumult crates (`tumult-otel/src/metrics.rs`, `tumult-otel/src/attributes.rs`)
3. OpenSpec proposals for Phase 8 (Resilience Scoring, Post-Mortem Engine) and Phase 9 (Observable Stack)
4. Regulatory compliance requirements from `docs/regulatory-mapping.md`

**19 SigNoz dashboards** now exist across two sessions. The table below summarises coverage, followed by identified gaps.

---

## Dashboard Inventory

| File | Title | Signals Covered |
|---|---|---|
| `tumult-experiments-overview.json` | Experiment Overview | `tumult.experiments.total`, `tumult.hypothesis.deviations.total`, `tumult.plugin.errors.total` |
| `tumult-actions-probes.json` | Actions & Probes | `tumult.actions.total`, `tumult.probes.total`, `tumult.probe.duration` |
| `tumult-store-health.json` | Store Health | `resilience.store.*` gauges, DuckDB + ClickHouse ingest spans |
| `tumult-infra-ops.json` | Infra Operations | `ssh.*`, `k8s.*` spans |
| `tumult-mcp.json` | MCP | `mcp.tool.call` spans by tool name |
| `tumult-plugins-baseline.json` | Plugins & Baseline | `script.execute`, `baseline.acquire`, tolerance bounds |
| `tumult-containers-host.json` | Containers & Host | `container.*`, `system.*` (host metrics) |
| `tumult-infra-targets.json` | Infra Targets (combined) | PostgreSQL + Redis + Kafka metrics |
| `tumult-resilience-compliance.json` | Compliance (basic) | experiment pass rate, MTTR proxy, DORA top-level |
| `tumult-containers-all.json` | **All Containers** | `container.cpu/memory/network/blockio` by `container.name` |
| `tumult-postgres.json` | **PostgreSQL** | `postgresql.*` metrics — backends, locks, commits, rollbacks, rows, db_size |
| `tumult-redis.json` | **Redis** | `redis.*` metrics — clients, memory, keyspace hits/misses, ops/sec, slaves |
| `tumult-kafka.json` | **Kafka** | `kafka.*` metrics — brokers, consumer lag, partition offsets |
| `tumult-clickhouse.json` | **ClickHouse** | Container resources (CPU/mem/net) + `clickhouse.connect` spans |
| `tumult-duckdb-analytics.json` | **DuckDB Analytics** | `resilience.analytics.*` spans, `resilience.store.size_bytes` |
| `tumult-experiment-dataflow.json` | **Experiment Data Flow** | Pipeline: `tumult.experiment` → ingest → query → export |
| `tumult-resilience-score.json` | **Resilience Score** | `resilience.score`, `resilience.estimate.accuracy`, rollbacks, hypothesis met/not-met |
| `tumult-compliance-dora-nis2.json` | **Compliance Deep-Dive** | DORA CFR/MTTR, NIS2 coverage, PCI-DSS Req.12 evidence |
| `tumult-experiment-phases.json` | **Experiment Phases** | Activity counts/latency/errors per phase: estimate/baseline/during/post/analysis/rollback |

---

## Remaining Gaps (Not Yet Visualised)

### Gap 1 — Phase 8: Post-Mortem Engine (not yet implemented)
**Source:** OpenSpec Phase 8 design (`postmortem-engine.md`)  
**Signals when implemented:** `tumult.postmortem.*` spans, `postmortem.created`, `postmortem.pagerduty.sent`, `postmortem.opsgenie.sent`  
**Recommended action:** Create `tumult-postmortem.json` skeleton with placeholder panels pointing to `postmortem.*` span names. This pre-builds the dashboard so it lights up automatically when Phase 8 ships.

### Gap 2 — Load Test Correlation (tumult-loadtest plugin)
**Source:** `tumult-loadtest` crate (k6 OTLP bridge, Phase 4 complete)  
**Signals:** k6 OTLP spans forwarded via `otlp` receiver — span names: `k6.http_req`, `k6.iteration`, `k6.vu`  
**Recommended action:** Create `tumult-loadtest.json` correlating k6 HTTP duration metrics with chaos experiment timelines. Key panels: request rate during chaos phases, P99 latency degradation, VU count, iteration errors. This is the primary "blast radius" visualisation.  
**Priority:** High — load test correlation is the #1 missing chaos engineering visualisation pattern.

### Gap 3 — SSH Connection Pooling Detail
**Source:** `tumult-ssh` crate (`ssh.connect`, `ssh.execute`, `ssh.upload` spans)  
**Current state:** `tumult-infra-ops.json` has SSH span rate and error panels but no per-host breakdown  
**Missing:** Per-`ssh.target.host` latency and error rate panel showing which remote hosts are failing  
**Recommended action:** Add `groupBy: ssh.target.host` panels to `tumult-infra-ops.json`

### Gap 4 — Kubernetes Node-Level Chaos
**Source:** `k8s.node.cordon`, `k8s.node.drain`, `k8s.node.status` spans  
**Current state:** `tumult-infra-ops.json` groups all k8s spans together  
**Missing:** Node-specific view showing which nodes are being cordoned/drained and their recovery  
**Recommended action:** Split `tumult-infra-ops.json` into separate rows for pod-level vs node-level operations, or create `tumult-k8s-nodes.json`

### Gap 5 — Alerting Rules (Phase 9 deliverable)
**Source:** OpenSpec Phase 9 — `alert-rules.md`  
**Current state:** No SigNoz alerting rules configured  
**Missing alert conditions:**
- `tumult.hypothesis.deviations.total` rate > 0 for > 5 min → `CHAOS_EXPERIMENT_DEVIATION`
- `kafka.consumer_group.lag_sum` > 1000 → `KAFKA_LAG_HIGH`
- `postgresql.backends / postgresql.connection.max` > 0.85 → `POSTGRES_CONNECTION_SATURATION`
- `redis.memory.used` rate of change > 50MB/min → `REDIS_MEMORY_SPIKE`
- `resilience.score` < 60 (last value) → `RESILIENCE_SCORE_DEGRADED`  

**Recommended action:** Create `docker/signoz/alerts/` directory with `alert-rules.json` or document these in `docs/signoz-alerts.md` as the SigNoz alert API endpoint is `POST /api/v1/rules`

### Gap 6 — Baseline Tolerance Bounds Drift
**Source:** `tumult-otel/src/metrics.rs` — `baseline.tolerance.*` gauges (lower/upper bounds)  
**Current state:** `tumult-plugins-baseline.json` shows tolerance bounds as static gauges  
**Missing:** Drift detection — a panel showing `probe value vs tolerance bounds over time` as a multi-series overlay. When probe values approach tolerance limits, it indicates system degradation before hypothesis failure.  
**Recommended action:** Add a graph panel to `tumult-plugins-baseline.json` with `baseline.value`, `baseline.tolerance.lower`, `baseline.tolerance.upper` as three series per probe name

### Gap 7 — Script Execution Output Classification
**Source:** `script.execute` spans with `script.exit_code` attribute  
**Current state:** `tumult-plugins-baseline.json` shows script rate and latency  
**Missing:** Exit code breakdown — success (0) vs failure (non-zero) grouped by `script.interpreter`  
**Recommended action:** Add `groupBy: script.exit_code` panel to `tumult-plugins-baseline.json`

### Gap 8 — DuckDB Schema Version Tracking
**Source:** `schema_meta` table in DuckDB (key=`schema_version`, value=`1`)  
**Current state:** Not visualised anywhere  
**Missing:** A panel in `tumult-store-health.json` or `tumult-duckdb-analytics.json` showing the current schema version as a stat panel. Useful for detecting unintended downgrades.  
**Recommended action:** Add a `resilience.store.schema_version` gauge panel (requires metric to be emitted from `tumult-analytics` — this is itself a gap in metric coverage)

### Gap 9 — Experiment Duration Distribution (Histogram)
**Source:** `tumult.experiment` span `durationNano`  
**Current state:** `tumult-experiments-overview.json` shows rates but no duration distribution  
**Missing:** Duration histogram/heatmap showing how long experiments take — useful for identifying outlier experiments  
**Recommended action:** Add a `p50/p90/p99/max` duration panel to `tumult-experiments-overview.json`

---

## Signal Coverage Matrix

### Metrics Fully Covered

| Metric | Dashboard |
|---|---|
| `tumult.experiments.total` | experiments-overview, compliance |
| `tumult.hypothesis.deviations.total` | experiments-overview, compliance |
| `tumult.plugin.errors.total` | experiments-overview, compliance |
| `tumult.actions.total` | actions-probes, experiment-phases |
| `tumult.probes.total` | actions-probes, experiment-phases |
| `tumult.rollbacks.total` | resilience-score, compliance, experiment-phases |
| `resilience.store.size_bytes` | duckdb-analytics, experiment-dataflow |
| `resilience.score` | resilience-score |
| `resilience.estimate.accuracy` | resilience-score |
| `postgresql.*` (all 7 metrics) | postgres |
| `redis.*` (all 6 metrics) | redis |
| `kafka.*` (all 6 metrics) | kafka |
| `container.cpu/memory/network/blockio` | containers-all, containers-host, clickhouse |
| `system.cpu/memory/disk/filesystem/network` | containers-host |

### Spans Fully Covered

| Span | Dashboard |
|---|---|
| `tumult.experiment` | experiments-overview, experiment-dataflow, compliance |
| `tumult.probe` | actions-probes, experiment-phases |
| `tumult.action` | actions-probes, experiment-phases |
| `resilience.analytics.ingest` | duckdb-analytics, experiment-dataflow |
| `resilience.analytics.query` | duckdb-analytics, experiment-dataflow |
| `resilience.analytics.export` | duckdb-analytics, experiment-dataflow |
| `resilience.analytics.import` | duckdb-analytics |
| `baseline.acquire` | plugins-baseline |
| `script.execute` | plugins-baseline |
| `ssh.connect`, `ssh.execute`, `ssh.upload` | infra-ops |
| `k8s.*` (all 9 span names) | infra-ops |
| `clickhouse.connect` | clickhouse |
| `mcp.tool.call` | mcp |

### Spans Not Yet Covered

| Span | Gap Ticket |
|---|---|
| `k6.http_req`, `k6.iteration`, `k6.vu` | Gap 2 (load test correlation) |
| `tumult.postmortem.*` (Phase 8, not emitted yet) | Gap 1 (post-mortem skeleton) |

### Metrics Not Yet Emitted (Future Phase 8/9 Gaps)

| Metric | Gap |
|---|---|
| `resilience.store.schema_version` | Gap 8 (DuckDB schema version panel) |
| `baseline.value` (per-probe observation) | Gap 6 (tolerance drift overlay) |

---

## Priority Matrix

| Gap | Priority | Effort | Phase |
|---|---|---|---|
| Load test correlation (Gap 2) | **High** | Medium | 4 (complete, unharvested) |
| Alerting rules (Gap 5) | **High** | Medium | 9 |
| Post-mortem skeleton (Gap 1) | Medium | Low | 8 (planned) |
| Baseline drift overlay (Gap 6) | Medium | Low | Existing |
| SSH per-host breakdown (Gap 3) | Low | Low | Existing |
| K8s node-level view (Gap 4) | Low | Low | Existing |
| Script exit code breakdown (Gap 7) | Low | Low | Existing |
| Experiment duration histogram (Gap 9) | Low | Low | Existing |
| Schema version gauge (Gap 8) | Low | Medium | Requires new metric emission |
