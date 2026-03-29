# Tumult Metadata Model

**Version**: 2.0.0
**Last Updated**: 2026-03-29
**Namespace**: `resilience.*`

---

## Abstract

This document defines the Tumult Metadata Model for resilience testing. It provides a structured vocabulary for describing experiments, targets, faults, measurements, outcomes, scoring, and regulatory evidence. The model follows [OpenTelemetry Semantic Convention](https://opentelemetry.io/docs/specs/semconv/) naming rules and is designed to be carried as span attributes, resource attributes, and metric dimensions within OTel-instrumented systems.

The namespace is **`resilience.*`**. All attributes defined here live under this single root.

### Requirement Tiers

The model is organised into three tiers:

| Tier | Groups | When |
|------|--------|------|
| **Required** | `experiment`, `target`, `fault`, `outcome` | Every experiment |
| **Phase data** (required if enabled) | `estimate`, `baseline`, `during`, `post`, `analysis` | When five-phase execution is active |
| **Optional enrichment** | `taxonomy`, `safety`, `environment`, `load`, `regulatory`, `score` | Enhances analytics, compliance, and scoring |

---

## Table of Contents

- [1. Conventions](#1-conventions)
- [2. Required Attribute Groups](#2-required-attribute-groups)
  - [2.1 resilience.experiment.*](#21-resilienceexperiment)
  - [2.2 resilience.target.*](#22-resiliencetarget)
  - [2.3 resilience.fault.*](#23-resiliencefault)
  - [2.4 resilience.outcome.*](#24-resilienceoutcome)
- [3. Phase Data Attribute Groups](#3-phase-data-attribute-groups)
  - [3.1 resilience.estimate.*](#31-resilienceestimate)
  - [3.2 resilience.baseline.*](#32-resiliencebaseline)
  - [3.3 resilience.during.*](#33-resilienceduring)
  - [3.4 resilience.post.*](#34-resiliencepost)
  - [3.5 resilience.analysis.*](#35-resilienceanalysis)
- [4. Optional Enrichment Groups](#4-optional-enrichment-groups)
  - [4.1 resilience.taxonomy.*](#41-resiliencetaxonomy) (NEW)
  - [4.2 resilience.safety.*](#42-resiliencesafety)
  - [4.3 resilience.environment.*](#43-resilienceenvironment)
  - [4.4 resilience.load.*](#44-resilienceload) (NEW)
  - [4.5 resilience.regulatory.*](#45-resilienceregulatory)
  - [4.6 resilience.score.*](#46-resiliencescore) (NEW — 3 layers)
- [5. Fault Subtype Taxonomy](#5-fault-subtype-taxonomy)
- [6. Time Standard](#6-time-standard)
- [7. Cardinality Rules](#7-cardinality-rules)
- [8. OTel Integration Rules](#8-otel-integration-rules)
- [9. Scientific Rules](#9-scientific-rules)
- [10. Scoring Methodology](#10-scoring-methodology)
- [11. Conformance](#11-conformance)

---

## 1. Conventions

| Convention | Rule |
|---|---|
| Naming | Dot-separated, lowercase, snake_case segments: `resilience.<group>.<attribute>` |
| Requirement levels | **Required** -- MUST be present on every conforming span. **Recommended** -- SHOULD be present when the information is available. **Optional** -- MAY be present. **Conditional** -- MUST be present when a stated condition is true. |
| Enums | Closed sets. Implementations MUST NOT invent values outside the documented set without proposing an addition to this standard. |
| Arrays | Denoted as `string[]`. Serialised as JSON arrays in span attributes and as repeated fields in protobuf. |
| Timestamps | Epoch nanoseconds (`int64`). See [Time Standard](#4-time-standard). |
| Durations | Seconds as `float64`. |

---

## 2. Required Attribute Groups

These four groups MUST be present on every experiment.

### 2.1 resilience.experiment.*

Identity and lineage of a single experiment run.

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `resilience.experiment.id` | `string` | Required | UUID v4 uniquely identifying this experiment run. |
| `resilience.experiment.name` | `string` | Required | Human-readable experiment name. Stable across runs (e.g. `postgresql-failover-recovery`). |
| `resilience.experiment.version` | `string` | Recommended | Semantic version of the experiment definition (e.g. `2.1.0`). |
| `resilience.experiment.suite` | `string` | Optional | Parent campaign or suite that groups related experiments. |
| `resilience.experiment.tags` | `string[]` | Optional | Free-form tags for filtering and grouping (e.g. `["network", "critical-path", "dora"]`). |
| `resilience.experiment.created_by` | `string` | Recommended | Identity of the user or system that initiated the run. |
| `resilience.experiment.run_number` | `int` | Recommended | Monotonically increasing run counter for this experiment name. Enables trend analysis across runs. |

---

### 2.2 resilience.target.*

The system, service, or component under test.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.target.system` | `string` | Required | Category of the target system. | `database`, `cache`, `message-broker`, `api`, `container`, `vm`, `host`, `kubernetes`, `network`, `application`, `middleware`, `cloud-service` |
| `resilience.target.technology` | `string` | Required | Specific technology or product name. | Examples: `postgresql`, `mysql`, `mariadb`, `oracle`, `redis`, `memcached`, `kafka`, `rabbitmq`, `nats`, `pulsar`, `docker`, `podman`, `kubernetes`, `nginx`, `envoy`, `haproxy`, `jvm`, `dotnet`, `nodejs` |
| `resilience.target.component` | `string` | Required | Specific instance or component identifier (e.g. `payments-db-primary`, `cart-service-pod-3`). High-cardinality -- see [Cardinality Rules](#5-cardinality-rules). |
| `resilience.target.tier` | `string` | Recommended | Architectural tier of the target. | `frontend`, `backend`, `data`, `infrastructure`, `network`, `platform` |
| `resilience.target.criticality` | `string` | Recommended | Business criticality classification. | `critical`, `high`, `medium`, `low` |
| `resilience.target.environment` | `string` | Required | Deployment environment where the experiment runs. | `production`, `staging`, `development`, `sandbox` |
| `resilience.target.region` | `string` | Optional | Cloud region or data centre location (e.g. `eu-west-1`, `us-east-1`, `dc-ams-01`). |

---

### 2.3 resilience.fault.*

The fault being injected.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.fault.type` | `string` | Required | Top-level fault category. | `termination`, `resource-stress`, `network`, `state`, `dependency`, `configuration`, `data` |
| `resilience.fault.subtype` | `string` | Required | Specific fault action within the type. See [Fault Subtype Taxonomy](#3-fault-subtype-taxonomy) for the full closed set per type. | See Section 3 |
| `resilience.fault.severity` | `string` | Required | Expected severity of the injected fault. | `minor`, `moderate`, `major`, `catastrophic` |
| `resilience.fault.duration_s` | `float` | Recommended | Planned duration of fault injection in seconds. |  |
| `resilience.fault.blast_radius` | `string` | Required | Scope of impact of the fault. | `single-instance`, `service`, `availability-zone`, `region`, `global` |
| `resilience.fault.reversible` | `boolean` | Required | Whether the fault can be automatically reversed after the experiment. |  |
| `resilience.fault.plugin` | `string` | Required | Name of the chaos plugin or driver executing the fault (e.g. `chaostooling-extension-db`, `litmus`, `toxiproxy`). |  |
| `resilience.fault.action` | `string` | Required | Fully qualified action identifier within the plugin (e.g. `kill_postgresql_process`, `inject_network_latency`). |  |

---

### 2.4 resilience.outcome.*

Final outcome of the experiment run. Moved here as a required group — see original definition below.

---

## 3. Phase Data Attribute Groups

These groups are required when five-phase execution is active (estimate → baseline → during → post → analysis).

### 3.1 resilience.estimate.*

Pre-experiment predictions. These are recorded before fault injection begins and are compared against actual outcomes in `resilience.analysis.*`.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.estimate.expected_outcome` | `string` | Required | Predicted outcome category. | `deviated`, `recovered`, `unaffected` |
| `resilience.estimate.expected_recovery_s` | `float` | Recommended | Predicted time to recovery in seconds. |  |
| `resilience.estimate.expected_degradation` | `string` | Recommended | Predicted level of service degradation during fault. | `none`, `minor`, `moderate`, `severe` |
| `resilience.estimate.expected_data_loss` | `boolean` | Recommended | Whether data loss is expected. |  |
| `resilience.estimate.confidence` | `string` | Recommended | Confidence level in the prediction. | `low`, `medium`, `high` |
| `resilience.estimate.rationale` | `string` | Recommended | Free-text explanation of why this outcome is expected. Stored on traces only (high cardinality). |  |
| `resilience.estimate.prior_runs` | `int` | Optional | Number of prior runs of this experiment used to inform the estimate. |  |

---

### 2.5 resilience.baseline.*

Steady-state measurements captured before fault injection. These establish the "normal" against which deviation is measured.

#### Top-level baseline attributes

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.baseline.method` | `string` | Required | Statistical method used to establish baseline thresholds. | `static`, `percentile`, `mean-stddev`, `iqr`, `learned` |
| `resilience.baseline.started_at` | `int64` | Required | Epoch nanoseconds when baseline collection began. |  |
| `resilience.baseline.ended_at` | `int64` | Required | Epoch nanoseconds when baseline collection ended. |  |
| `resilience.baseline.duration_s` | `float` | Required | Total baseline collection duration in seconds. |  |
| `resilience.baseline.warmup_s` | `float` | Recommended | Warm-up period in seconds excluded from statistical calculations. |  |
| `resilience.baseline.samples` | `int` | Required | Total number of samples collected during the baseline window. |  |
| `resilience.baseline.interval_s` | `float` | Required | Sampling interval in seconds. |  |
| `resilience.baseline.confidence` | `float` | Recommended | Statistical confidence level, `0.0` to `1.0` (e.g. `0.95` for 95% confidence). |  |
| `resilience.baseline.sigma` | `float` | Conditional | Number of standard deviations used for threshold. Required when `method` is `mean-stddev`. |  |
| `resilience.baseline.source` | `string` | Recommended | Origin of the baseline data. | `live`, `historical`, `aqe` |
| `resilience.baseline.anomaly_detected` | `boolean` | Recommended | Whether anomalies were detected in the baseline window that may affect reliability. |  |

#### Per-probe baseline metrics

For each probe `{name}`, the following attributes are emitted under `resilience.baseline.probe.{name}.*`:

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `resilience.baseline.probe.{name}.mean` | `float` | Required | Arithmetic mean of probe values during baseline. |
| `resilience.baseline.probe.{name}.stddev` | `float` | Recommended | Standard deviation of probe values. |
| `resilience.baseline.probe.{name}.p50` | `float` | Recommended | 50th percentile (median). |
| `resilience.baseline.probe.{name}.p95` | `float` | Recommended | 95th percentile. |
| `resilience.baseline.probe.{name}.p99` | `float` | Recommended | 99th percentile. |
| `resilience.baseline.probe.{name}.min` | `float` | Recommended | Minimum observed value. |
| `resilience.baseline.probe.{name}.max` | `float` | Recommended | Maximum observed value. |
| `resilience.baseline.probe.{name}.error_rate` | `float` | Recommended | Proportion of failed probe executions, `0.0` to `1.0`. |

> `{name}` is the probe's stable identifier (e.g. `query_latency`, `connection_count`). It MUST be lowercase snake_case.

---

### 2.6 resilience.during.*

Measurements captured while the fault is active.

#### Top-level during-fault attributes

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `resilience.during.started_at` | `int64` | Required | Epoch nanoseconds when fault injection began. |
| `resilience.during.ended_at` | `int64` | Required | Epoch nanoseconds when fault injection ended. |
| `resilience.during.fault_active_s` | `float` | Required | Actual duration the fault was active in seconds. |
| `resilience.during.sample_interval_s` | `float` | Required | Sampling interval in seconds during the fault window. |
| `resilience.during.degradation_onset_s` | `float` | Recommended | Seconds from fault start to first observable degradation. |
| `resilience.during.degradation_peak_s` | `float` | Recommended | Seconds from fault start to peak degradation. |
| `resilience.during.degradation_magnitude` | `float` | Recommended | Peak degradation magnitude expressed in sigma (standard deviations from baseline mean). |
| `resilience.during.graceful_degradation` | `boolean` | Recommended | Whether the system degraded gracefully (maintained partial service) rather than failing hard. |

#### Per-probe during-fault metrics

For each probe `{name}`:

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `resilience.during.probe.{name}.samples` | `int` | Required | Number of samples collected for this probe during the fault window. |
| `resilience.during.probe.{name}.mean` | `float` | Required | Mean probe value during the fault. |
| `resilience.during.probe.{name}.max` | `float` | Recommended | Maximum probe value during the fault. |
| `resilience.during.probe.{name}.min` | `float` | Recommended | Minimum probe value during the fault. |
| `resilience.during.probe.{name}.error_rate` | `float` | Recommended | Proportion of failed probe executions, `0.0` to `1.0`. |
| `resilience.during.probe.{name}.breached_at` | `int64` | Recommended | Epoch nanoseconds of the first threshold breach. Absent if no breach occurred. |
| `resilience.during.probe.{name}.breach_count` | `int` | Recommended | Number of times the probe breached its baseline threshold. |

---

### 2.7 resilience.post.*

Measurements captured after fault removal to assess recovery.

#### Top-level post-fault attributes

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `resilience.post.started_at` | `int64` | Required | Epoch nanoseconds when post-fault observation began. |
| `resilience.post.ended_at` | `int64` | Required | Epoch nanoseconds when post-fault observation ended. |
| `resilience.post.duration_s` | `float` | Required | Total post-fault observation duration in seconds. |
| `resilience.post.samples` | `int` | Required | Total number of samples collected during the post-fault window. |
| `resilience.post.recovery_time_s` | `float` | Required | Mean Time to Recovery (MTTR) -- seconds from fault removal to all probes returning within baseline thresholds. |
| `resilience.post.full_recovery` | `boolean` | Required | Whether the system fully returned to baseline levels. |
| `resilience.post.residual_degradation` | `float` | Recommended | Remaining degradation at end of observation, expressed in sigma from baseline mean. `0.0` means fully recovered. |
| `resilience.post.data_integrity_verified` | `boolean` | Recommended | Whether a data integrity check was performed after recovery. |
| `resilience.post.data_loss_detected` | `boolean` | Recommended | Whether data loss was detected after recovery. |

#### Per-probe post-fault metrics

For each probe `{name}`:

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `resilience.post.probe.{name}.mean` | `float` | Required | Mean probe value during the post-fault window. |
| `resilience.post.probe.{name}.p95` | `float` | Recommended | 95th percentile during the post-fault window. |
| `resilience.post.probe.{name}.error_rate` | `float` | Recommended | Error rate during the post-fault window, `0.0` to `1.0`. |
| `resilience.post.probe.{name}.returned_to_baseline` | `boolean` | Required | Whether this probe returned to within baseline thresholds. |
| `resilience.post.probe.{name}.recovery_time_s` | `float` | Required | Seconds from fault removal until this probe returned to baseline. |

---

### 2.8 resilience.outcome.*

Final outcome of the experiment run.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.outcome.status` | `string` | Required | Terminal status of the experiment run. | `completed`, `deviated`, `aborted`, `failed`, `interrupted` |
| `resilience.outcome.hypothesis_met` | `boolean` | Required | Whether the experiment's hypothesis was confirmed. |  |
| `resilience.outcome.deviation_detected` | `boolean` | Required | Whether the system deviated from expected behaviour. |  |
| `resilience.outcome.deviation_magnitude` | `float` | Conditional | Magnitude of deviation in sigma. Required when `deviation_detected` is `true`. |  |
| `resilience.outcome.recovery_time_s` | `float` | Recommended | Observed total recovery time in seconds. |  |
| `resilience.outcome.mttr_s` | `float` | Recommended | Mean Time to Recovery in seconds. Equivalent to `resilience.post.recovery_time_s` but included here for query convenience. |  |
| `resilience.outcome.data_loss` | `boolean` | Recommended | Whether data loss occurred during the experiment. |  |
| `resilience.outcome.rollback_executed` | `boolean` | Required | Whether a rollback action was triggered (manual or automatic). |  |
| `resilience.outcome.rollback_success` | `boolean` | Conditional | Whether the rollback succeeded. Required when `rollback_executed` is `true`. |  |

---

### 2.9 resilience.safety.*

Safety constraints and guardrails for the experiment.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.safety.abort_conditions` | `string[]` | Recommended | List of conditions that trigger automatic experiment abort (e.g. `["error_rate > 0.5", "p95_latency > 10s", "data_loss detected"]`). |  |
| `resilience.safety.max_duration_s` | `float` | Recommended | Maximum allowed experiment duration in seconds. The engine MUST abort if exceeded. |  |
| `resilience.safety.max_blast_radius` | `string` | Recommended | Maximum allowed blast radius. The engine MUST NOT inject faults beyond this scope. |  |
| `resilience.safety.requires_approval` | `boolean` | Optional | Whether this experiment requires manual approval before fault injection. |  |
| `resilience.safety.rollback_strategy` | `string` | Required | Strategy for rollback after fault injection. | `always`, `on-deviation`, `never`, `manual` |

---

### 2.10 resilience.environment.*

Infrastructure context where the experiment runs.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.environment.platform` | `string` | Required | Compute platform type. | `bare-metal`, `vm`, `container`, `kubernetes`, `serverless` |
| `resilience.environment.cloud_provider` | `string` | Optional | Cloud provider, if applicable. | `aws`, `gcp`, `azure`, `on-premise` |
| `resilience.environment.cluster` | `string` | Optional | Cluster name or identifier (e.g. `prod-eu-01`, `staging-k8s`). |  |
| `resilience.environment.namespace` | `string` | Optional | Kubernetes namespace or equivalent isolation boundary. |  |

---

### 2.11 resilience.regulatory.*

Regulatory and compliance mapping for audit evidence.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.regulatory.frameworks` | `string[]` | Recommended | Regulatory frameworks this experiment provides evidence for. | `DORA`, `NIS2`, `PCI-DSS`, `ISO-22301`, `ISO-27001`, `SOC2`, `Basel-III` |
| `resilience.regulatory.requirement_id` | `string` | Recommended | Specific requirement or control identifier within the framework (e.g. `DORA-Art.25`, `PCI-DSS-12.10.2`). |  |
| `resilience.regulatory.evidence_type` | `string` | Recommended | Type of evidence this experiment run produces (e.g. `recovery-test`, `failover-validation`, `backup-verification`). |  |
| `resilience.regulatory.compliance_status` | `string` | Recommended | Compliance determination based on experiment outcome. | `compliant`, `non-compliant`, `partial` |

---

### 2.12 resilience.analysis.*

Post-experiment analysis comparing predictions to actuals and tracking trends.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.analysis.estimate_accuracy` | `float` | Recommended | Accuracy of the pre-experiment estimate, `0.0` to `1.0`. `1.0` means the prediction exactly matched the outcome. |  |
| `resilience.analysis.estimate_recovery_delta` | `float` | Recommended | Difference between predicted and actual recovery time in seconds (`predicted - actual`). Positive means recovery was faster than predicted. |  |
| `resilience.analysis.trend` | `string` | Recommended | Resilience trend based on comparison with prior runs of the same experiment. | `improving`, `stable`, `degrading` |
| `resilience.analysis.resilience_score` | `float` | Recommended | Composite resilience score, `0.0` to `1.0`. Computed per the platform's documented score methodology. |  |
| `resilience.analysis.regulatory_evidence` | `string[]` | Recommended | List of regulatory evidence artefact identifiers produced by this run (e.g. report IDs, audit log references). |  |

---

## 3. Fault Subtype Taxonomy

Every `resilience.fault.subtype` value MUST belong to exactly one `resilience.fault.type`. The following is the complete closed set.

### termination

Faults that stop processes, containers, or hosts.

| Subtype | Description |
|---|---|
| `process-kill` | Kill a specific OS process (SIGKILL or equivalent). |
| `container-kill` | Stop and remove a container. |
| `vm-stop` | Power off a virtual machine. |
| `node-shutdown` | Shut down a physical or virtual node. |
| `pod-delete` | Delete a Kubernetes pod. |
| `service-stop` | Stop a systemd/init service. |

### resource-stress

Faults that exhaust compute, memory, storage, or OS resources.

| Subtype | Description |
|---|---|
| `cpu-stress` | Saturate CPU cores to a target utilisation. |
| `memory-stress` | Allocate memory to a target utilisation or OOM threshold. |
| `io-stress` | Generate sustained disk I/O load. |
| `disk-fill` | Fill a filesystem to a target percentage. |
| `fd-exhaustion` | Exhaust available file descriptors. |
| `thread-exhaustion` | Exhaust available threads or goroutines. |

### network

Faults that degrade or disrupt network communication.

| Subtype | Description |
|---|---|
| `latency-injection` | Add artificial latency to network packets. |
| `packet-loss` | Drop a percentage of network packets. |
| `partition` | Create a network partition between components. |
| `dns-failure` | Cause DNS resolution failures. |
| `bandwidth-limit` | Throttle network bandwidth. |
| `corruption` | Corrupt network packet payloads. |
| `port-block` | Block traffic on specific ports. |
| `connection-reset` | Force TCP connection resets. |

### state

Faults that manipulate internal system state.

| Subtype | Description |
|---|---|
| `connection-kill` | Terminate active database or service connections. |
| `lock-injection` | Inject lock contention or deadlocks. |
| `replication-lag` | Introduce artificial replication delay. |
| `failover-trigger` | Force a failover from primary to replica. |
| `leader-election` | Trigger a leader re-election in a consensus group. |
| `cache-flush` | Flush in-memory caches, forcing cold-start behaviour. |

### dependency

Faults that affect upstream or downstream dependencies.

| Subtype | Description |
|---|---|
| `upstream-timeout` | Cause upstream service calls to time out. |
| `downstream-error` | Make a downstream dependency return errors. |
| `circuit-break` | Force a circuit breaker to open state. |
| `service-unavailable` | Make a dependency completely unreachable. |

### configuration

Faults that alter runtime configuration.

| Subtype | Description |
|---|---|
| `config-change` | Modify a runtime configuration parameter. |
| `feature-toggle` | Flip a feature flag to an unexpected state. |
| `resource-limit` | Reduce resource limits (connection pools, memory caps, rate limits). |
| `permission-revoke` | Revoke permissions or credentials at runtime. |

### data

Faults that affect data integrity or ordering.

| Subtype | Description |
|---|---|
| `corruption` | Corrupt stored or in-flight data. |
| `deletion` | Delete data records or files. |
| `reorder` | Reorder messages or events. |
| `duplication` | Duplicate messages or records. |
| `schema-drift` | Introduce schema changes that cause incompatibility. |

---

## 4. Time Standard

All temporal values in this standard follow a single set of rules to eliminate ambiguity and conversion overhead.

| Context | Format | Example |
|---|---|---|
| Timestamps in span attributes and journals | Epoch nanoseconds (`int64`) | `1711670400000000000` |
| Durations in span attributes and journals | Seconds (`float64`) | `12.345` |
| Experiment definitions (human-authored JSON/YAML) | Human-friendly strings | `120s`, `5m`, `1h` |
| Internal storage and analytics | Epoch nanoseconds | Native in ClickHouse, DuckDB, OTel |

**Rationale**:

- **OTel native**: OpenTelemetry's protobuf encoding uses `fixed64` nanosecond timestamps. No conversion is needed at the exporter boundary.
- **Database native**: ClickHouse `DateTime64(9)` and DuckDB `TIMESTAMP_NS` store nanosecond precision natively.
- **Zero conversion**: A single canonical format eliminates an entire class of timezone, precision, and parsing bugs.

Experiment engines MUST convert human-friendly durations in experiment definitions to `float64` seconds before emitting span attributes. The human-friendly format is a convenience for authors, not a wire format.

---

## 5. Cardinality Rules

Controlling attribute cardinality is essential for metric backends (Prometheus, ClickHouse, etc.) that create time series per unique label combination.

| Rule | Detail |
|---|---|
| Enum attributes use finite, documented values only | Implementations MUST NOT add ad-hoc enum values. New values require a standard revision. |
| No free-text in metric dimensions | Attributes like `resilience.estimate.rationale` are high-cardinality and MUST NOT be used as metric labels. They belong on spans only. |
| `resilience.target.component` is high-cardinality | Use on traces and logs. NEVER use as a metric dimension. |
| Metric-safe fault dimensions | `resilience.fault.type` and `resilience.fault.subtype` are bounded enums and are safe for metric dimensions. |
| Metric-safe target dimensions | `resilience.target.system`, `resilience.target.technology`, and `resilience.target.environment` are bounded and metric-safe. |
| Per-probe attributes are inherently high-cardinality | The `probe.{name}.*` pattern creates attributes dynamically. Use these on traces. For metrics, emit per-probe data as separate metric instruments (e.g. `resilience.probe.latency{probe.name="query_latency"}`). |

---

## 6. OTel Integration Rules

### Span Attributes

All `resilience.*` attributes are carried as **span attributes** on the spans emitted by the chaos engine during experiment execution.

### Resource Attributes

`resilience.experiment.*` attributes are set as **resource attributes** on the root span of the experiment trace. This ensures they propagate to all child spans and are available for trace-level queries.

### Span Propagation

`resilience.target.*` attributes are set on **every span** within the experiment trace. This enables filtering any span in the trace by target system, technology, or environment.

### Metric Dimensions

The following attributes are approved for use as metric label dimensions:

| Metric Dimension | Cardinality | Source Group |
|---|---|---|
| `resilience.fault.type` | 7 values | `resilience.fault.*` |
| `resilience.fault.subtype` | ~35 values | `resilience.fault.*` |
| `resilience.target.system` | 12 values | `resilience.target.*` |
| `resilience.target.technology` | Open but bounded | `resilience.target.*` |
| `resilience.target.environment` | 4 values | `resilience.target.*` |
| `resilience.outcome.status` | 5 values | `resilience.outcome.*` |

### Recommended Metric Instruments

| Metric Name | Type | Unit | Dimensions |
|---|---|---|---|
| `resilience.experiment.duration` | Histogram | `s` | `fault.type`, `target.system`, `outcome.status` |
| `resilience.experiment.count` | Counter | `{experiment}` | `fault.type`, `target.system`, `outcome.status` |
| `resilience.recovery.duration` | Histogram | `s` | `fault.type`, `target.system` |
| `resilience.deviation.count` | Counter | `{deviation}` | `fault.type`, `target.system`, `fault.severity` |

---

## 7. Scientific Rules

Resilience testing produces evidence. Evidence must meet scientific standards to be credible, reproducible, and useful for regulatory audit.

### 1. Reproducibility

Experiment metadata MUST be sufficient to re-run the experiment identically. This means:

- All `resilience.experiment.*`, `resilience.target.*`, `resilience.fault.*`, `resilience.safety.*`, and `resilience.environment.*` attributes are populated.
- The experiment definition (JSON/YAML) is version-controlled and referenced by `resilience.experiment.version`.
- Any external state dependency (e.g. "requires 3-node cluster") is documented.

### 2. Falsifiability

Every hypothesis MUST have measurable bounds:

- `resilience.estimate.expected_outcome` declares what the experimenter expects.
- `resilience.estimate.expected_recovery_s` sets a numeric threshold.
- `resilience.outcome.hypothesis_met` provides a boolean verdict.

A hypothesis of "the system should be resilient" is not falsifiable. A hypothesis of "the system will recover to baseline within 30 seconds after primary database failover" is.

### 3. Baseline Integrity

Raw samples MUST be preserved, not just derived thresholds:

- `resilience.baseline.samples` records the count.
- Per-probe `.mean`, `.stddev`, `.p50`, `.p95`, `.p99`, `.min`, `.max` provide distributional summary.
- Implementations SHOULD additionally store raw sample arrays in the experiment journal for full reproducibility.

### 4. Controlled Variables

The environment MUST be recorded so that changes between runs are visible:

- `resilience.environment.*` captures platform, provider, cluster, and namespace.
- `resilience.target.*` captures the specific component under test.
- Differences between runs (e.g. changed cluster size) surface automatically in attribute comparison.

### 5. Blast Radius Accounting

Scope MUST be declared before injection and enforced during:

- `resilience.fault.blast_radius` declares intended scope.
- `resilience.safety.max_blast_radius` sets the hard limit.
- The engine MUST abort if observed impact exceeds the declared blast radius.

### 6. Prediction Tracking

Estimate vs actual MUST be compared to enable organisational learning:

- `resilience.estimate.*` records predictions before injection.
- `resilience.analysis.estimate_accuracy` and `resilience.analysis.estimate_recovery_delta` compare predictions to outcomes.
- Over time, improving estimate accuracy demonstrates growing system understanding.
- `resilience.analysis.trend` tracks whether resilience is improving, stable, or degrading.

---

## 8. Conformance

An implementation is **conformant** with this standard if:

1. All attributes marked **Required** are present on every experiment span.
2. All attributes marked **Conditional** are present when their stated condition is true.
3. All enum values are drawn from the documented closed sets.
4. Timestamps use epoch nanoseconds (`int64`).
5. Durations use seconds (`float64`).
6. The fault subtype taxonomy in Section 3 is respected -- subtypes are not mixed across types.
7. Per-probe attributes follow the `probe.{name}.*` naming pattern with lowercase snake_case probe names.
8. High-cardinality attributes (`resilience.target.component`, `resilience.estimate.rationale`, per-probe attributes) are not used as metric dimensions.

Implementations MAY extend this standard with additional attributes under a vendor-specific namespace (e.g. `resilience.vendor.mycompany.*`), provided the core `resilience.*` attributes remain conformant.

---

---

## 10. Scoring Methodology (Optional — 3 Layers)

The scoring model is optional but enables deep analytics when populated. Scores are computed in DuckDB from journal data — not during experiment execution.

### Layer 1 — Experiment Quality (`resilience.score.quality.*`)

Six orthogonal signals computed per experiment. Scientific basis: cyclomatic complexity (McCabe, 1976), FMEA severity (IEC 60812:2018), chaos engineering principles (Basiri et al., 2016).

| Attribute | Type | Description | Values |
|---|---|---|---|
| `resilience.score.quality.complexity` | `string` | Structural complexity based on step count, config params, rollback presence. | `low`, `medium`, `high`, `critical` |
| `resilience.score.quality.blast_radius` | `string` | Fault propagation scope based on fault type analysis. | `contained`, `moderate`, `broad`, `systemic` |
| `resilience.score.quality.risk` | `string` | Operational risk combining hypothesis absence, rollback quality, deviation history. | `low`, `medium`, `high`, `critical` |
| `resilience.score.quality.rollback` | `string` | Rollback completeness: action present, verification probe, idempotent pattern. | `none`, `weak`, `good`, `excellent` |
| `resilience.score.quality.grade` | `string` | Run history grade based on success rate over trailing 10 runs. | `A` (>=90%), `B` (>=70%), `C` (>=50%), `D` (<50%) |
| `resilience.score.quality.otel_score` | `float` | Continuous 0-100 score for time-series trending. Weighted combination of complexity, risk, probe count, rollback ratio. | `0.0` to `100.0` |
| `resilience.score.quality.needs_review` | `boolean` | Automatically set when risk is HIGH/CRITICAL AND blast_radius is BROAD/SYSTEMIC. | |

#### FMEA Extension (`resilience.fault.fmea_*`)

Optional FMEA (Failure Mode and Effects Analysis) attributes per IEC 60812:2018:

| Attribute | Type | Description |
|---|---|---|
| `resilience.fault.fmea_severity` | `int` | Severity rating 1-10 (10 = most severe). |
| `resilience.fault.fmea_detectability` | `int` | Detectability rating 1-10 (10 = hardest to detect). |
| `resilience.fault.fmea_rpn` | `int` | Risk Priority Number = severity x detectability. |

### Layer 2 — DORA Four Keys (`resilience.score.dora.*`)

Operational performance metrics. Requires external data (CI/CD events, incident management). This layer is an **integration point** — Tumult does not compute these values directly but accepts them from external systems for correlation.

Scientific basis: Forsgren, Humble, Kim (2018), *Accelerate*; Google DORA State of DevOps Reports.

| Attribute | Type | Description |
|---|---|---|
| `resilience.score.dora.deployment_frequency_per_day` | `float` | Deployments per calendar day. |
| `resilience.score.dora.lead_time_hours` | `float` | Mean time from commit to production deploy. |
| `resilience.score.dora.change_failure_rate` | `float` | Proportion of deployments causing incidents (0.0-1.0). |
| `resilience.score.dora.mttr_hours` | `float` | Mean time to recovery from incidents. |
| `resilience.score.dora.chaos_coverage_pct` | `float` | Proportion of services with experiment runs in period (0.0-1.0). |

### Layer 3 — Regulatory Confidence (`resilience.score.regulatory.*`)

Per-control compliance confidence. Scientific basis: ISO 31000:2018 risk treatment confidence model. The multiplicative formula ensures a single failing factor reduces overall confidence proportionally.

| Attribute | Type | Description |
|---|---|---|
| `resilience.score.regulatory.framework` | `string` | Framework identifier (DORA, NIS2, PCI-DSS, ISO-22301, ISO-27001, SOC2). |
| `resilience.score.regulatory.control_id` | `string` | Specific control (e.g. DORA-Art25.1, PCI-DSS-12.10.2). |
| `resilience.score.regulatory.confidence` | `float` | Confidence score 0.0-1.0, computed as: `base_weight x quality_score x complexity_factor x rollback_factor x min(probe_count/threshold, 1.0) x outcome_factor`. |
| `resilience.score.regulatory.outcome_factor` | `float` | PASSED=1.0, FAILED_ROLLBACK_OK=0.85, FAILED=0.30. A failed experiment with successful rollback is stronger evidence than untested. |

### Scoring References

1. McCabe, T.J. (1976). *A Complexity Measure*. IEEE TSE, 2(4), 308-320.
2. IEC 60812:2018. *Failure modes and effects analysis (FMEA and FMECA)*.
3. Forsgren, N., Humble, J. & Kim, G. (2018). *Accelerate*. IT Revolution Press.
4. ISO 31000:2018. *Risk management — Guidelines*.
5. Basiri, A. et al. (2016). *Chaos Engineering*. IEEE Software, 33(3), 35-41.
6. EU Regulation 2022/2554. *Digital Operational Resilience Act (DORA)*.

---

## Appendix A: resilience.taxonomy.* (Optional Classification)

Domain classification for slicing experiments by what is being tested. Optional — enhances analytics but is not required for experiment execution.

| Attribute | Type | Requirement | Description | Enum Values |
|---|---|---|---|---|
| `resilience.taxonomy.track` | `string` | Optional | Testing track. | `chaos`, `security`, `performance`, `continuity` |
| `resilience.taxonomy.category` | `string` | Optional | Fault injection category. | `fault_injection`, `latency`, `resource_stress`, `state_manipulation`, `dependency_failure` |
| `resilience.taxonomy.domain` | `string` | Optional | Infrastructure domain. | `compute`, `network`, `storage`, `data`, `platform`, `application` |
| `resilience.taxonomy.component` | `string` | Optional | Specific component class. | `cpu`, `memory`, `io`, `disk`, `broker`, `primary`, `replica`, `pod`, `node`, `gateway` |
| `resilience.taxonomy.scenario` | `string` | Optional | Named scenario pattern. | `cpu_stress`, `memory_pressure`, `io_saturation`, `partition`, `failover`, `leader_election`, `connection_kill` |

### Analytics Example

```sql
SELECT
    taxonomy_domain,
    taxonomy_scenario,
    AVG(post_recovery_time_s) as avg_mttr,
    COUNT(*) as runs
FROM journals
WHERE taxonomy_track = 'chaos'
GROUP BY taxonomy_domain, taxonomy_scenario
ORDER BY avg_mttr DESC
```

---

## Appendix B: resilience.load.* (Optional Load Integration)

Attributes for correlating experiments with load testing tools (k6, JMeter).

| Attribute | Type | Requirement | Description |
|---|---|---|---|
| `resilience.load.tool` | `string` | Optional | Load testing tool. Values: `k6`, `jmeter`. |
| `resilience.load.vus` | `int` | Optional | Virtual users / threads. |
| `resilience.load.duration_s` | `float` | Optional | Load test duration in seconds. |
| `resilience.load.throughput_rps` | `float` | Optional | Observed requests per second. |
| `resilience.load.latency_p95_ms` | `float` | Optional | 95th percentile response time in milliseconds. |
| `resilience.load.error_rate` | `float` | Optional | Error rate 0.0-1.0 during load. |
| `resilience.load.thresholds_met` | `boolean` | Optional | Whether all load thresholds passed. |

---

*Tumult Metadata Model v2.0. The `resilience.*` namespace is designed for OTel compatibility and can be adopted by any resilience testing platform.*
