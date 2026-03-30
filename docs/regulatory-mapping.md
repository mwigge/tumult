---
title: Regulatory Mapping
parent: Reference
nav_order: 3
---

# Tumult Regulatory Mapping

How Tumult experiment evidence maps to regulatory requirements for operational resilience. This document covers the six major frameworks that financial institutions, critical infrastructure operators, and technology providers must satisfy.

---

## Frameworks at a Glance

| Framework | Jurisdiction | Applies from | Penalty |
|-----------|-------------|-------------|---------|
| DORA (EU 2022/2554) | EU financial entities | 17 January 2025 | Administrative penalties per member state |
| NIS2 (EU 2022/2555) | EU essential/important entities | 17 October 2024 | Up to EUR 10M or 2% global revenue |
| PCI-DSS 4.0 | Global (card payment handling) | 31 March 2025 (v4.0.1 full) | Fines, increased transaction fees, loss of processing rights |
| Basel III / BCBS 239 | Global banking | Phased since 2013 | Supervisory action |
| ISO 22301 | Global (voluntary, often contractual) | N/A | Certification loss |
| ISO 27001 / SOC 2 | Global (voluntary, often contractual) | N/A | Certification/attestation loss |

---

## DORA — Digital Operational Resilience Act (EU 2022/2554)

DORA is the most prescriptive framework for resilience testing in financial services. It explicitly requires testing programmes with documented evidence.

### Article 24 — General requirements for ICT resilience testing

**Requirement**: Financial entities shall establish, maintain, and review an ICT resilience testing programme as an integral part of the digital operational resilience framework.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Sound and comprehensive testing programme | Experiment definitions in TOON with steady-state hypothesis, method, rollbacks | `tumult.experiment.title`, `tumult.experiment.id` |
| Testing covers ICT systems supporting critical functions | Target tagging per system/function | `resilience.target.system`, `resilience.target.criticality` |
| Testing programme is proportionate to risks | Risk-based experiment selection, estimate confidence levels | `resilience.estimate.confidence`, `resilience.estimate.rationale` |
| Testing is undertaken by independent parties | Operator identity in journal, separation of roles | `tumult.operator.id`, `tumult.operator.role` |

### Article 25 — Testing of ICT tools and systems

**Requirement**: The ICT resilience testing programme shall provide for the execution of appropriate tests, including vulnerability assessments and scans, open-source analyses, network security assessments, gap analyses, physical security reviews, questionnaires and scanning software solutions, source code reviews, scenario-based tests, compatibility testing, performance testing, end-to-end testing, and penetration testing.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Scenario-based tests | Experiment method steps define the fault scenario | `tumult.action.name`, `tumult.probe.name` |
| Performance testing | Baseline and during-fault metrics with statistical analysis | `resilience.baseline.*`, `resilience.during.*` |
| End-to-end testing | Multi-target experiments covering full transaction paths | `tumult.target.type`, `tumult.target.id` |
| Testing at least yearly | Journal timestamps prove execution dates and frequency | `tumult.experiment.started_at`, `resilience.analysis.trend_run_count` |
| Documented results | TOON journals with full trace linkage | Journal files, OTel traces |

### Article 26 — Advanced testing (TLPT / TIBER-EU)

**Requirement**: Financial entities identified as systemically important shall carry out threat-led penetration testing (TLPT) at least every 3 years, covering critical or important functions.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Threat-led scenarios | Experiments designed from threat intelligence, tagged with threat model | `resilience.threat.model`, `resilience.threat.scenario` |
| Cover critical functions | Criticality tagging on experiment targets | `resilience.target.criticality` |
| Live production testing | Execution target and environment recorded | `tumult.execution.target`, `tumult.environment` |
| Testing every 3 years minimum | Journal history with timestamps spanning the required period | `tumult.experiment.started_at` |

### Article 11 — Response and recovery

**Requirement**: Financial entities shall put in place an ICT business continuity policy and ICT response and recovery plans.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Recovery time objectives validated | Phase 3 recovery measurement against declared RTO | `resilience.post.recovery_duration_s`, `resilience.post.mttr_s` |
| Recovery plans tested | Rollback execution and verification in journal | `tumult.rollback.*`, `resilience.post.fully_recovered` |
| Lessons learned from testing | Phase 0 vs Phase 3 comparison, trend analysis | `resilience.analysis.estimate_accuracy`, `resilience.analysis.trend_direction` |

---

## NIS2 — Network and Information Security Directive (EU 2022/2555)

NIS2 applies to essential and important entities across 18 sectors. It requires risk management measures including testing and audit.

### Article 21(2)(c) — Business continuity and crisis management

**Requirement**: Member States shall ensure that essential and important entities take appropriate and proportionate technical, operational, and organisational measures to manage risks, including business continuity and crisis management.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Business continuity validated | Experiments that inject faults and measure recovery | `resilience.post.recovery_duration_s`, `resilience.post.fully_recovered` |
| Crisis management tested | Multi-fault experiments, cascading failure scenarios | `tumult.experiment.title`, `tumult.action.name` |
| Backup and recovery procedures | Data integrity verification post-fault | `resilience.post.data_integrity_verified`, `resilience.post.data_loss_detected` |

### Article 21(2)(f) — Assessment of cybersecurity measures effectiveness

**Requirement**: Policies and procedures to assess the effectiveness of cybersecurity risk-management measures.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Effectiveness assessment | Baseline vs during-fault comparison proves control effectiveness | `resilience.baseline.*`, `resilience.during.*` |
| Regular assessment | Journal timestamps and run frequency | `resilience.analysis.trend_run_count` |
| Documented results | TOON journals with statistical analysis | Journal files, Parquet exports |

### Penalty Context

NIS2 fines reach EUR 10M or 2% of total worldwide annual turnover for essential entities. The cost of maintaining a testing programme with documented evidence is trivial compared to the penalty exposure.

---

## PCI-DSS 4.0 — Payment Card Industry Data Security Standard

PCI-DSS applies to any entity that stores, processes, or transmits cardholder data. Version 4.0 strengthens testing requirements.

### Requirement 11.4 — Penetration testing

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| 11.4.1: Penetration testing methodology defined | Experiment definitions with hypothesis, method, rollbacks | `tumult.experiment.title`, experiment TOON files |
| 11.4.2: Internal penetration testing at least annually | Journal timestamps prove execution | `tumult.experiment.started_at` |
| 11.4.3: External penetration testing at least annually | Remote target experiments via SSH | `tumult.execution.target`, `tumult.target.id` |
| 11.4.4: Exploitable vulnerabilities corrected and retested | Trend analysis showing remediation | `resilience.analysis.trend_direction` |

### Requirement 11.4.5 — Segmentation control testing

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Segmentation controls tested at least every 6 months (service providers) | Network partition experiments with recovery verification | `tumult.action.name` (e.g., `network-partition`), `resilience.post.fully_recovered` |
| Confirm segmentation is operational and effective | Probe results showing isolation holds during fault | `resilience.during.*`, `tumult.probe.name` |

### Requirement 12.10 — Incident response testing

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| 12.10.2: Incident response plan tested at least annually | Experiments that trigger incident response procedures | `tumult.experiment.title`, journal evidence |
| 12.10.4: Personnel trained through testing | Operator identity and role in journal | `tumult.operator.id`, `tumult.operator.role` |

---

## Basel III / BCBS 239 — Risk Data Aggregation and Risk Reporting

BCBS 239 principles govern how banks aggregate and report risk data. Principle 6 (Adaptability) directly relates to resilience testing.

### Principle 6 — Adaptability

**Requirement**: A bank should be able to generate aggregate risk data to meet a broad range of on-demand, ad hoc risk management reporting requests, including requests during stress/crisis situations.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Systems function under stress | Experiments validating database, messaging, and compute under fault conditions | `tumult.target.type`, `resilience.during.*` |
| Data aggregation during crisis | Probes measuring query performance and data availability during faults | `resilience.baseline.mean`, `resilience.during.peak_value` |
| Recovery of reporting capability | Phase 3 recovery measurement for data systems | `resilience.post.recovery_duration_s`, `resilience.post.data_integrity_verified` |

---

## ISO 22301 — Business Continuity Management

ISO 22301 Section 8.5 requires exercising and testing of business continuity arrangements.

### 8.5 — Exercising and testing

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Exercises are consistent with scope of BCMS | Experiments tagged by business function and scope | `resilience.target.system`, `resilience.target.criticality` |
| Based on appropriate scenarios | Experiment definitions with hypothesis and rationale | `tumult.experiment.title`, `resilience.estimate.rationale` |
| Produce formal post-exercise reports | HTML reports generated from journals via `tumult report` | Journal files, HTML reports |
| Results analysed and acted upon | Trend analysis and estimate accuracy tracking | `resilience.analysis.*` |
| Conducted at planned intervals | Journal history with regular execution timestamps | `tumult.experiment.started_at` |

---

## ISO 27001 — Information Security / SOC 2

### ISO 27001 — Annex A.17: IT Service Continuity

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| A.17.1.3: Verify and review continuity controls | Experiment results proving controls function under fault | `resilience.baseline.*`, `resilience.during.*`, `resilience.post.*` |
| Regular testing and review | Journal frequency and trend data | `resilience.analysis.trend_run_count` |

### SOC 2 — CC7.5: Recovery from Disruptions

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Entity recovers from identified disruptions | Phase 3 recovery evidence with MTTR | `resilience.post.recovery_duration_s`, `resilience.post.mttr_s` |
| Recovery procedures are tested | Rollback execution recorded in journal | `tumult.rollback.*` |
| Recovery meets defined objectives | Recovery time compared against declared RTO | `resilience.post.recovery_duration_s` |

---

## Tagging Experiments with Regulatory Requirements

Experiments can declare which regulatory requirements they satisfy using the `resilience.regulatory.*` attributes. This enables filtering journals by framework for audit purposes.

### Experiment-level tags

```toon
tags:
  - regulatory:dora:art24
  - regulatory:dora:art25
  - regulatory:nis2:art21-2c
  - regulatory:pci-dss:11.4.2
  - regulatory:iso22301:8.5

configuration:
  regulatory_frameworks: "DORA,NIS2,PCI-DSS"
  regulatory_evidence_level: "formal"
  audit_retention_days: 2555
```

### Attributes on every experiment run

```
resilience.regulatory.frameworks       = "DORA,NIS2,PCI-DSS"
resilience.regulatory.articles         = "art24,art25,art21-2c,11.4.2"
resilience.regulatory.evidence_level   = "formal"        # formal, informal, exploratory
resilience.regulatory.audit_period     = "2025-Q1"
resilience.regulatory.retention_days   = 2555            # 7 years for DORA
```

### Filtering journals for audit

```sql
-- All DORA Article 24 evidence for 2025
SELECT
    experiment_title,
    started_at,
    status,
    recovery_duration_s,
    resilience_score
FROM journals
WHERE tags @> ARRAY['regulatory:dora:art24']
    AND started_at >= '2025-01-01'
    AND started_at < '2026-01-01'
ORDER BY started_at;

-- Compliance coverage: which requirements have been tested this quarter
SELECT
    UNNEST(tags) AS tag,
    COUNT(*) AS run_count,
    MAX(started_at) AS last_run,
    AVG(CASE WHEN status = 'completed' THEN 1.0 ELSE 0.0 END) AS success_rate
FROM journals
WHERE tags[1] LIKE 'regulatory:%'
    AND started_at >= DATE_TRUNC('quarter', CURRENT_DATE)
GROUP BY tag
ORDER BY tag;
```

---

## Journals as Audit Evidence

TOON journals are the primary audit artefact. Every journal contains:

| Evidence | Journal field | Regulatory value |
|----------|--------------|-----------------|
| What was tested | `experiment.title`, `experiment.description`, `method_results` | Proves scope and scenario |
| When it was tested | `started_at`, `ended_at` | Proves testing frequency |
| What was the baseline | Phase 1 baseline statistics | Proves normal operating parameters |
| What happened under fault | Phase 2 during-fault observations | Proves impact assessment |
| How fast recovery occurred | Phase 3 recovery measurement | Proves RTO compliance |
| Whether data was lost | `resilience.post.data_integrity_verified` | Proves data integrity |
| What was predicted vs actual | Phase 0 estimate vs Phase 2/3 observations | Proves organizational learning |
| Full trace lineage | `trace_id`, `span_id` on every activity result | Proves end-to-end auditability |

### Evidence chain

```
Experiment definition (.toon)
    │
    ▼
Journal (.toon) ←── contains all 5 phases
    │
    ├──> OTel traces (Jaeger/Tempo) ←── correlated by trace_id
    ├──> OTel metrics (Prometheus) ←── correlated by experiment_id
    ├──> DuckDB (local analytics)
    ├──> Parquet (archival export)
    └──> HTML report (human-readable summary)
```

Every piece of evidence traces back to the journal's `trace_id`. An auditor can start from the HTML report, drill into the journal for raw data, and follow the trace_id into the observability stack for the full distributed trace.

### Retention

| Framework | Minimum retention | Recommended |
|-----------|------------------|-------------|
| DORA | 5 years (Art. 28) | 7 years |
| NIS2 | Per member state transposition | 5 years |
| PCI-DSS | 1 year (Req. 10.7) | 3 years |
| ISO 22301 | Certification cycle (3 years) | 5 years |
| SOC 2 | Audit period (typically 12 months) | 3 years |

Parquet export enables cost-effective long-term archival. Journals compressed as Parquet are typically 10-20x smaller than the equivalent JSON, making 7-year retention practical even at high experiment frequency.

---

## Cross-Framework Mapping Summary

| Capability | DORA | NIS2 | PCI-DSS | Basel III | ISO 22301 | ISO 27001 / SOC 2 |
|-----------|------|------|---------|-----------|-----------|-------------------|
| Scenario-based testing | Art. 25 | Art. 21(2)(f) | 11.4.1 | -- | 8.5 | A.17.1.3 |
| Recovery validation | Art. 11 | Art. 21(2)(c) | 12.10.2 | Principle 6 | 8.5 | CC7.5 |
| Testing frequency proof | Art. 24 | Art. 21(2)(f) | 11.4.2 | -- | 8.5 | CC7.5 |
| Data integrity verification | Art. 11 | Art. 21(2)(c) | -- | Principle 6 | -- | -- |
| Trend analysis / learning | Art. 24 | -- | 11.4.4 | -- | 8.5 | -- |
| Threat-led testing | Art. 26 | -- | 11.4 | -- | -- | -- |
| Segmentation testing | -- | -- | 11.4.5 | -- | -- | -- |
| Audit trail / evidence | Art. 24 | Art. 21(2)(f) | 10.7 | BCBS 239 | 8.5 | CC7.5 |
