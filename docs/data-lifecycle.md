---
title: Data Lifecycle
parent: Reference
nav_order: 1
---

# Tumult Data Lifecycle

This reference describes the journal fields produced by the current runner.
The five-phase model also reserves fields for future measurement features;
a field in the schema does not imply that the runner populates it. Optional
results are absent when their phase does not run.

| Phase | Current behavior | Limits |
|---|---|---|
| Estimate | Preserve the optional operator estimate in the journal. | A prediction is not a measured result. |
| Baseline | Evaluate the declared steady-state probes before faults. | Statistical baseline acquisition is a separate library capability; ordinary runs do not acquire or apply derived tolerances. |
| During | Sample hypothesis probes while the method executes. | Probe timing, errors and tolerance breaches are measured; degradation onset, peak and shape classification are reserved. |
| Post | Sample hypothesis probes after method completion, before final hypothesis and rollback. | Stops after one round in which all probes pass, cancellation, or timeout. This is not sustained recovery after cleanup. |
| Analysis | Compare an optional estimate to the run outcome. | Runner scores are binary indicators; cross-run trends and report scoring are separate analytics operations. |

Continuous steady-state verification is a separate operating mode, not a
sixth phase of an experiment.

## Baseline modes

| `tumult run --baseline-mode` | Behavior |
|---|---|
| `full` (default) | Run probes and the method with the declared tolerances. |
| `skip` | Use the same declared tolerances; automatic statistical acquisition is not integrated. |
| `only` | Evaluate steady-state probes once; exclude definition controls, method actions, load and rollback. Requires hypothesis probes. |

Baseline-only journals are labeled as observations and are not automatically
ingested as fault-execution evidence. Probe implementations must themselves be
safe to run: declaring a shell command a probe does not sandbox its effects.
See [Statistical Baselines](guides/baseline-guide.md) for the separate statistics
library and [Execution Flow](guides/execution-flow.md) for runtime ordering.

## During results

`during_result` records the sampling window, actual `sample_interval_s`, and
per-probe sample count, mean/min/max execution duration in milliseconds,
error rate and tolerance breaches. The default interval is one second, with
at most 300 sampling rounds. There is no sampling when the experiment has no
hypothesis probes.

These `DuringResult` fields are reserved and remain absent (`None`):

- `degradation_onset_s`
- `degradation_peak_s`
- `degradation_magnitude`
- `graceful_degradation`

The duration statistics describe execution of the probe, not arbitrary numeric
values printed by it. A database integrity check must be supplied as an
explicit probe with an appropriate tolerance.

## Post results

`post_result` records observations starting immediately after the method
finishes. Probes use their declared tolerances and the same sampling interval
as the during phase. Sampling stops at the first round in which every probe
passes, on cancellation, or at the recovery timeout (default 30 seconds).
The runner does not require ten consecutive successful samples.

`full_recovery` reports whether all probes were observed to recover;
`recovery_time_s` and per-probe timings measure from the post-phase start.
`mttr_s` is absent if recovery was not observed. Always inspect the recovery
flag alongside timings: a duration alone does not prove recovery. Final
hypothesis and rollback results are separate evidence.

These `PostResult` fields are reserved and remain absent (`None`):

- `residual_degradation`
- `data_integrity_verified`
- `data_loss_detected`

The runner does not automatically verify data integrity or assert absence of
data loss. Post sampling precedes rollback, so a fault requiring explicit
rollback can remain active during this window. Supply and inspect cleanup
steps; a Completed status does not imply every resource was restored.

## Analysis results

When an estimate is present, the runner populates `estimate_accuracy` and
`resilience_score` with coarse 0/1 outcome indicators. The journal's
`estimate_recovery_delta_s` and `trend` fields remain absent. Use the analytics
commands for cross-run analysis; report scorecards use their own scoring
model. Evidence reports summarize observations and disclose unverified
mappings; they do not establish regulatory compliance.

## Time and persistence

Fields ending in `_ns` are epoch nanoseconds; `_ms` durations are milliseconds;
`_s` durations are seconds. Read the field's unit rather than assuming one unit
for every duration.

The CLI writes a TOON journal, then optionally imports journal detail into the
DuckDB lake. When a daemon owns the database, use the daemon import API rather
than opening the database from another writing process. OTLP traces/metrics
are a separate export path and require `OTEL_EXPORTER_OTLP_ENDPOINT`; they can
be sent directly to `tumultd` or through an OTel Collector. The collector routes
signals to the chosen backend. See [Observability Setup](guides/observability-setup.md).

Normal queries and reports read the hot database. Parquet archives are read
through their committed manifest and are not automatically unioned into hot
queries. Retention is disabled until that completeness guarantee exists.
[Data portability and recovery](guides/data-portability.md) defines archive
coverage, credential exclusions and complete operational backup/restore.
Local Parquet files and hash chains do not enforce WORM storage or protect
against an administrator who can rewrite both evidence and hashes.

## SQL over imported journals

The implemented journal-detail tables are `experiments`, `activity_results`
and `load_results`; there is no `journals` table. Detailed recovery fields
remain in the TOON journal and are not columns in `experiments`.

### Recent run outcomes

```sql
SELECT title, status, started_at_ns, duration_ms,
       hypothesis_before_met, hypothesis_after_met
FROM experiments
ORDER BY started_at_ns DESC
LIMIT 20;
```

### Failed activities with their run

```sql
SELECT e.experiment_id, e.title, a.name, a.phase, a.status, a.error
FROM experiments e
JOIN activity_results a ON a.experiment_id = e.experiment_id
WHERE a.status = 'failed'
ORDER BY a.started_at_ns DESC;
```

### Available estimate comparisons

```sql
SELECT experiment_id, title, started_at_ns, estimate_accuracy, resilience_score
FROM experiments
WHERE estimate_accuracy IS NOT NULL
ORDER BY started_at_ns DESC;
```
