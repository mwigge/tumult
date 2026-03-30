# <img src="../images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Compliance as Code: DORA, NIS2, and Regulatory Evidence with Tumult

![Tumult Banner](../images/tumult-banner.png)

*Part 9 of the Tumult series. [← Part 8: Statistical Baselines](./08-statistical-baselines.md)*

---

Regulatory compliance and chaos engineering have more in common than most engineering teams realize. DORA, NIS2, PCI-DSS, ISO 22301 — every major operational resilience regulation requires the same thing: **evidence that you tested your systems under failure conditions and that those systems behaved as expected**.

The typical approach is to run chaos experiments, write a report manually, and submit it to compliance teams. This works, but it is fragile: the mapping between experiment results and regulatory requirements is done by a person, with all the inconsistency and gaps that entails.

Tumult takes a different approach: embed the regulatory mapping in the experiment definition itself, and generate the compliance evidence automatically from the structured journal data.

---

## The Regulatory Landscape

The frameworks Tumult supports for compliance evidence generation:

| Framework | Applies To | Key Requirement |
|-----------|-----------|----------------|
| **DORA** (EU 2022/2554) | EU financial entities (banks, payment processors, fintech) | ICT resilience testing programme with documented evidence |
| **NIS2** (EU 2022/2555) | EU essential/important entities across 18 sectors | Technical and organisational resilience measures with effectiveness assessment |
| **PCI-DSS 4.0** | Any entity handling cardholder data globally | Penetration testing, segmentation testing, incident response testing |
| **ISO 22301** | Voluntary (often contractual) | Business continuity exercises with formal post-exercise reports |
| **ISO 27001 / SOC 2** | Voluntary (often contractual) | IT service continuity controls, tested and documented |
| **Basel III / BCBS 239** | Global banking | Risk data systems function under stress |

DORA is the most immediately urgent for many organizations: it became applicable to EU financial entities on 17 January 2025, with explicit requirements for ICT resilience testing programmes and formal evidence.

---

## Embedding Regulatory Mapping in Experiments

Every Tumult experiment can declare which regulatory requirements it satisfies:

```toon
title: Payment database failover validates DORA Article 25 requirements
description: |
  Kill database primary connections and verify automatic reconnection.
  Produces evidence for DORA Art. 25 (ICT resilience testing) and
  Art. 11 (Response and Recovery).

tags[4]: database, resilience, regulatory:dora, regulatory:nis2

regulatory:
  frameworks[2]: DORA, NIS2
  requirements[3]:
    - id: DORA-Art25
      description: ICT resilience testing — scenario-based tests
      evidence: Database failover recovery within declared RTO

    - id: DORA-Art11
      description: Response and recovery — recovery time validation
      evidence: MTTR measured and compared against RTO target of 30s

    - id: NIS2-Art21-2c
      description: Business continuity — backup and recovery procedures
      evidence: Automatic reconnection and data integrity verified
```

This mapping is stored in the journal and exposed as attributes on OTel spans. It enables the critical query: "show me all experiments that provided evidence for DORA Article 25 this quarter."

---

## What Constitutes Evidence

TOON journals are the primary audit artefact. Every journal contains structured evidence across multiple dimensions:

| Evidence | Journal Field | Regulatory Value |
|----------|--------------|-----------------|
| **What was tested** | `title`, `description`, `method_results` | Proves the test scenario |
| **When it was tested** | `started_at_ns`, `ended_at_ns` | Proves testing frequency and dates |
| **Normal operating parameters** | Phase 1 baseline statistics | Proves what "normal" looks like |
| **Impact under fault** | Phase 2 during-fault observations | Proves impact was assessed |
| **Recovery time** | Phase 3 `recovery_time_s`, `mttr_s` | Proves RTO compliance |
| **Data integrity** | `data_integrity_verified`, `data_loss_detected` | Proves no data was lost |
| **Prediction vs actual** | Phase 0 estimate vs Phase 2/3 observations | Proves organisational learning |
| **Full trace lineage** | `trace_id`, `span_id` on every activity | Proves end-to-end auditability |

The evidence chain is complete and verifiable:

```
Experiment definition (.toon) — what was planned
    │
    ▼
Journal (.toon) — what actually happened (all 5 phases)
    │
    ├──▶ trace_id → OTel backend (distributed trace, nanosecond precision)
    ├──▶ experiment_id → DuckDB (queryable analytics)
    ├──▶ Parquet export (long-term archival, 10-20x compressed)
    └──▶ HTML report (human-readable summary for auditors)
```

An auditor starts from the HTML report, drills into the journal for raw data, and follows the `trace_id` into the observability stack for the full distributed trace. Every claim in the report is backed by traceable, timestamped evidence.

---

## DORA in Practice

DORA is the most prescriptive framework for financial services. Three articles are directly relevant to chaos engineering:

### Article 24 — General requirements for ICT resilience testing

Requires financial entities to have a testing programme that covers ICT systems supporting critical functions. The programme must be proportionate to risk and undertaken regularly.

**Tumult provides:**
- Experiment definitions that document the test scenario, scope, and expected outcome (estimate)
- Journals with timestamps proving testing dates and frequency
- `resilience.target.criticality` attribute for risk-based prioritisation
- Trend analysis showing programme regularity and improvement over time

### Article 25 — Testing of ICT tools and systems

Requires scenario-based tests, performance testing, end-to-end testing, and penetration testing.

**Tumult provides:**
- Experiment methods that define the fault scenario (connection kill, pod delete, node drain)
- Baseline and during-fault measurements that constitute "performance testing"
- Multi-target experiments covering full transaction paths
- `resilience.fault.type` and `resilience.fault.subtype` taxonomy for scenario classification

### Article 11 — Response and recovery

Requires that recovery time objectives (RTOs) are tested and documented.

**Tumult provides:**
- Phase 3 `recovery_time_s` — the measured MTTR
- Phase 0 `expected_recovery_s` — the declared RTO
- Analysis phase comparison: did the actual MTTR meet the declared RTO?
- Rollback execution evidence: the rollback was run, and it succeeded

---

## SQL Queries for Compliance Reporting

### DORA Article 24: Testing programme coverage

```sql
-- Which critical systems have been tested in the past 90 days?
SELECT
    title,
    MAX(TIMESTAMP 'epoch' + started_at_ns * INTERVAL '1 nanosecond') AS last_tested,
    COUNT(*) AS test_runs,
    AVG(CASE WHEN status = 'Completed' THEN 1.0 ELSE 0.0 END) AS success_rate
FROM experiments
WHERE started_at_ns > (EPOCH_NS(NOW()) - 90 * 24 * 3600 * 1000000000)
GROUP BY title
ORDER BY last_tested ASC;
```

The systems sorted to the top — those with the oldest last test date — are the coverage gaps.

### DORA Article 11: RTO compliance evidence

```sql
-- Did experiments meet their declared recovery time objectives?
SELECT
    title,
    estimate_expected_recovery_s AS rto_target_s,
    post_recovery_time_s AS actual_recovery_s,
    CASE
        WHEN post_recovery_time_s <= estimate_expected_recovery_s THEN 'COMPLIANT'
        ELSE 'EXCEEDED_RTO'
    END AS rto_status,
    post_recovery_time_s - estimate_expected_recovery_s AS rto_delta_s
FROM experiments
WHERE post_recovery_time_s IS NOT NULL
ORDER BY rto_delta_s DESC;
```

### PCI-DSS 11.4.2: Annual testing frequency proof

```sql
-- Confirm all critical experiments ran at least once in the past year
SELECT
    title,
    COUNT(*) AS runs_this_year,
    MIN(TIMESTAMP 'epoch' + started_at_ns * INTERVAL '1 nanosecond') AS first_run,
    MAX(TIMESTAMP 'epoch' + started_at_ns * INTERVAL '1 nanosecond') AS last_run
FROM experiments
WHERE started_at_ns > (EPOCH_NS(NOW()) - 365 * 24 * 3600 * 1000000000)
    AND array_contains(tags, 'regulatory:pci-dss')
GROUP BY title
HAVING runs_this_year >= 1;
```

### ISO 22301: Post-exercise reporting

```sql
-- Generate summary evidence for ISO 22301 Section 8.5
SELECT
    title,
    TIMESTAMP 'epoch' + started_at_ns * INTERVAL '1 nanosecond' AS test_date,
    status,
    estimate_expected_outcome AS expected_outcome,
    CASE
        WHEN status = 'Completed' AND hypothesis_after_met THEN 'PASSED'
        WHEN status = 'Deviated' THEN 'DEVIATED'
        ELSE 'FAILED'
    END AS exercise_result,
    post_recovery_time_s AS recovery_time_s,
    resilience_score
FROM experiments
WHERE array_contains(tags, 'regulatory:iso22301')
ORDER BY started_at_ns DESC;
```

---

## Generating Compliance Reports

```bash
# Generate a DORA compliance report from all journals
tumult compliance journals/ --framework dora

# NIS2 report
tumult compliance journals/ --framework nis2

# PCI-DSS report
tumult compliance journals/ --framework pci-dss

# Multiple frameworks
tumult compliance journals/ --framework dora --framework nis2 --framework pci-dss
```

The compliance command queries the journal data using SQL, maps results to framework requirements, and produces a structured report indicating:

- Which requirements are covered by experiment evidence
- Which requirements have no experiment evidence (coverage gaps)
- For covered requirements: the most recent test date, pass/fail status, and MTTR measurements

---

## Tagging Experiments for Audit Filtering

Consistent tagging enables precise filtering for audit evidence:

```toon
# Tag format: regulatory:<framework>:<article-or-requirement-id>
tags[5]:
  - database
  - resilience
  - regulatory:dora:art24
  - regulatory:dora:art25
  - regulatory:nis2:art21-2c
```

Then filter in SQL:

```sql
-- All DORA Article 24 evidence for Q1 2025
SELECT *
FROM experiments
WHERE array_contains(tags, 'regulatory:dora:art24')
    AND started_at_ns BETWEEN
        EPOCH_NS(TIMESTAMP '2025-01-01') AND
        EPOCH_NS(TIMESTAMP '2025-04-01')
ORDER BY started_at_ns;
```

---

## Evidence Retention

| Framework | Minimum Retention | Why |
|-----------|------------------|-----|
| DORA | 5 years (Art. 28) | Financial supervisory access window |
| NIS2 | Per member state | Varies, commonly 3-5 years |
| PCI-DSS | 1 year (Req. 10.7) | Annual audit cycle |
| ISO 22301 | 3-year certification cycle | Evidence for next audit |
| SOC 2 | 12-month audit period | Annual attestation |

Parquet export makes long-term retention practical. Journals compressed as Parquet are typically 10-20x smaller than equivalent JSON. A compliance archive of 5 years of daily experiment runs is tens of gigabytes in Parquet — trivial to store in S3 or cold storage.

```bash
# Archive journals as Parquet for long-term retention
tumult export journals/2025/*.toon --format parquet --output archives/2025/

# Query archived data directly (DuckDB reads Parquet from S3)
tumult analyze 's3://your-bucket/archives/2025/*.parquet' \
  --query "SELECT * FROM experiments WHERE array_contains(tags, 'regulatory:dora:art24')"
```

---

## The Compliance Argument for Engineering Teams

Chaos engineering is often positioned as an engineering discipline. For many organizations, the regulatory compliance angle is what actually gets it funded.

The argument is straightforward: you are required by law to test your ICT resilience, document the results, and retain the evidence. Manual testing and report-writing is slow, expensive, and inconsistent. Tumult automates the evidence generation — every experiment run produces structured, queryable, auditable evidence that maps directly to regulatory requirements.

The cost of maintaining a structured testing programme with Tumult is significantly lower than the alternative: manual testing, manual documentation, and the penalty exposure of inadequate evidence (EUR 10M or 2% of global revenue for NIS2 non-compliance; substantial fines for DORA; loss of payment processing rights for PCI-DSS).

For platform teams, this is the conversation that turns chaos engineering from "interesting engineering practice" to "critical business requirement."

---

*Next in the series: [Part 10 — Chaos Under Load: Network Faults and Load Testing →](./10-chaos-under-load.md)*
