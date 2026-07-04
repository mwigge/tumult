---
title: Regulatory Mapping
parent: Reference
nav_order: 3
---

# Tumult Regulatory Mapping

How Tumult experiment evidence maps to regulatory requirements for operational resilience. This document covers the major frameworks that financial institutions, critical infrastructure operators, and technology providers must satisfy.

> **Evidence, not compliance.** Tumult experiments produce technical **evidence toward** the controls below. Passing experiments do not by themselves establish regulatory compliance or constitute a legal attestation — a compliance determination requires assessment by a qualified auditor against the official source text. Each mapping is graded by how directly a chaos experiment can evidence the control (direct / supporting / indirect). The machine-readable, dated source of truth for these mappings lives in `tumult-core/src/compliance.rs` (`CITATIONS`); run `tumult compliance --framework <fw> --sources` to list every citation with its official source URL and `last_verified` date.

> **Disambiguation:** DORA in this document refers exclusively to the **Digital Operational Resilience Act (EU 2022/2554)** — the EU regulation for financial sector ICT resilience. This is distinct from the DevOps Research and Assessment (DORA) programme and its "Four Keys" metrics, which are covered separately in the [Resilience Metadata Standard](resilience-metadata-standard.md) under `resilience.score.devops.*`.

---

## Frameworks at a Glance

| Framework | Jurisdiction | Applies from | Penalty | Source |
|-----------|-------------|-------------|---------|--------|
| DORA (EU 2022/2554) | EU financial entities | 17 January 2025 | Administrative penalties per member state | [EUR-Lex](https://eur-lex.europa.eu/eli/reg/2022/2554/oj) |
| NIS2 (EU 2022/2555) | EU essential/important entities | 17 October 2024 | Up to EUR 10M or 2% global revenue | [EUR-Lex](https://eur-lex.europa.eu/eli/dir/2022/2555/oj) |
| PCI-DSS 4.0 | Global (card payment handling) | 31 March 2025 (v4.0.1 full) | Fines, increased transaction fees, loss of processing rights | [PCI SSC](https://www.pcisecuritystandards.org/document_library/) |
| Basel III / BCBS 239 | Global banking | Phased since 2013 | Supervisory action | [BIS](https://www.bis.org/publ/bcbs239.htm) |
| ISO 22301:2019 | Global (voluntary, often contractual) | N/A | Certification loss | [ISO](https://www.iso.org/standard/75106.html) |
| ISO 27001:2022 / SOC 2 | Global (voluntary, often contractual) | N/A | Certification/attestation loss | [ISO](https://www.iso.org/standard/27001) / [AICPA](https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2) |

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

**Requirement**: Financial entities identified as systemically important shall carry out threat-led penetration testing (TLPT) at least every 3 years on live production systems, covering critical or important functions, per the TLPT Regulatory Technical Standards and TIBER-EU.

> **Scope note (indirect mapping):** Tumult experiments are resilience tests, **not** threat-led penetration tests, and do **not** satisfy TLPT. TLPT requires a red team simulating a real adversary. Tumult can *inform* TLPT scenario design and evidence recovery under the kinds of scenarios a red team might trigger — nothing more. Do not represent Tumult runs as TLPT evidence.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Scenario design input | Experiments designed from threat intelligence, tagged with threat model | `resilience.threat.model`, `resilience.threat.scenario` |
| Cover critical functions | Criticality tagging on experiment targets | `resilience.target.criticality` |
| Recovery under adversarial scenarios | Recovery measurement for scenarios a red team may trigger | `resilience.post.*` |

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

### Article 21(2)(b) — Incident handling

**Requirement**: Essential and important entities shall take measures for incident handling.

> **Scope note:** Incident *handling* is Art. 21(2)(b). The distinct incident *reporting* obligations (24-hour early warning, 72-hour notification, one-month final report) are **Art. 23**, which is a reporting process — a chaos experiment does not evidence reporting-timeline compliance. Earlier versions of this mapping cited "Art. 23 — Incident handling and reporting"; that label conflated the two.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Incident handling procedures exercised | Controlled fault injection triggers incident-response procedures | `tumult.experiment.title`, journal evidence |

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

> **Overreach removed.** Requirement **11.4 (penetration testing)** and **11.4.5 (segmentation control testing)** are *security* tests — 11.4 finds exploitable vulnerabilities; 11.4.5 verifies that the cardholder data environment is isolated from out-of-scope networks. Chaos-engineering fault injection (e.g. `network-partition`) is **not** a penetration test and does **not** evidence that segmentation *controls* prevent unauthorised access. Prior versions of this document mapped resilience experiments to 11.4.1/11.4.2/11.4.3/11.4.5; those mappings were a category error and have been removed. The defensible PCI-DSS mapping is incident-response testing (12.10.2) below.

### Requirement 12.10 — Incident response testing

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| 12.10.2: Incident response plan tested at least annually | Experiments that trigger incident response procedures | `tumult.experiment.title`, journal evidence |
| 12.10.4: Personnel trained through testing | Operator identity and role in journal | `tumult.operator.id`, `tumult.operator.role` |

---

## Basel Committee (BCBS) — Operational Resilience

> **Framing correction.** Earlier versions labelled this framework "Basel III / BCBS 239". BCBS 239 is the *risk data aggregation and risk reporting* standard — it is **not** part of the Basel III capital framework, and its subject is data aggregation, not resilience testing. The correct anchor for chaos-engineering evidence is the Basel Committee's **[Principles for Operational Resilience](https://www.bis.org/bcbs/publ/d516.htm) (March 2021)**, whose **Principle 4 (Business continuity planning and testing)** directly concerns resilience exercises.

### Principles for Operational Resilience — Principle 4 (Business continuity planning and testing)

**Requirement**: Banks should have business continuity plans in place and conduct business continuity exercises under a range of severe but plausible scenarios in order to test their ability to deliver critical operations through disruption.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| BC exercises under severe-but-plausible scenarios | Fault-injection experiments modelling plausible disruptions | `tumult.experiment.title`, `resilience.during.*` |
| Ability to deliver critical operations through disruption | During-fault probes on critical systems | `resilience.target.criticality`, `resilience.during.*` |
| Recovery of critical operations | Phase 3 recovery measurement | `resilience.post.recovery_duration_s`, `resilience.post.data_integrity_verified` |

### BCBS 239 — Principle 6 (Adaptability) — *indirect*

**Requirement**: A bank should be able to generate aggregate risk data to meet a broad range of on-demand, ad hoc risk management reporting requests, including requests during stress/crisis situations.

> **Indirect mapping.** BCBS 239 concerns risk-*data* aggregation, not infrastructure resilience. Experiments that keep reporting/data systems available under fault only indirectly touch this principle. Retained for continuity; do not over-claim.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| Reporting capability available under stress | Probes measuring query performance and data availability during faults | `resilience.baseline.mean`, `resilience.during.peak_value` |
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

### ISO 27001:2022 — Annex A.5.30: ICT readiness for business continuity

> **Updated control numbering.** ISO/IEC 27001:2022 restructured Annex A. The old **A.17.1.3** cited in prior versions is from the **withdrawn 2013 edition**. The correct 2022 controls are **A.5.30 (ICT readiness for business continuity)** — which explicitly requires ICT continuity to be *tested* — and **A.5.29 (Information security during disruption)**.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| A.5.30: ICT continuity planned, maintained and **tested** | Recovery-measuring experiments prove ICT continuity is tested | `resilience.baseline.*`, `resilience.during.*`, `resilience.post.*` |
| A.5.29: Maintain information security during disruption | During-fault observations | `resilience.during.*` |
| Regular testing and review | Journal frequency and trend data | `resilience.analysis.trend_run_count` |

### SOC 2 — CC7.5: Recovery from identified security incidents

> **Label correction.** Prior versions cited "CC7.4 — Detection and monitoring". In the 2017 Trust Services Criteria, **CC7.4 is *incident response*** (responding to identified security incidents); monitoring/detection is **CC7.1/CC7.2**. The recovery mapping is **CC7.5**.

| Requirement | Tumult Evidence | Attributes |
|-------------|----------------|------------|
| CC7.5: Recover from identified security incidents | Phase 3 recovery evidence with MTTR | `resilience.post.recovery_duration_s`, `resilience.post.mttr_s` |
| CC7.4: Respond to identified security incidents | Controlled faults exercise the incident-response programme | `tumult.rollback.*` |
| CC7.2: Monitor system components for anomalies | Observability data (OTel traces/metrics) during faults | `resilience.during.*` |

---

## Tagging Experiments with Regulatory Requirements

Experiments can declare which regulatory requirements they satisfy using the `resilience.regulatory.*` attributes. This enables filtering journals by framework for audit purposes.

### Experiment-level tags

```toon
tags:
  - regulatory:dora:art24
  - regulatory:dora:art25
  - regulatory:nis2:art21-2c
  - regulatory:pci-dss:12.10.2
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
| DORA | Retention is not set by a single "5-year" article; keep testing records for the period competent authorities may request (commonly cited as ~5 years). **The prior "Art. 28" citation was wrong — Art. 28 governs ICT third-party risk, not record retention.** Confirm the applicable basis with your supervisor. | 7 years |
| NIS2 | Per member state transposition | 5 years |
| PCI-DSS | 1 year — audit log retention is **Req. 10.5.1** (12 months, 3 months immediately available). *(Prior "Req. 10.7" was wrong: 10.7 covers detecting failures of critical security controls.)* | 3 years |
| ISO 22301 | Certification cycle (3 years) | 5 years |
| SOC 2 | Audit period (typically 12 months) | 3 years |

Parquet export enables cost-effective long-term archival. Journals compressed as Parquet are typically 10-20x smaller than the equivalent JSON, making 7-year retention practical even at high experiment frequency.

---

## Cross-Framework Mapping Summary

| Capability | DORA | NIS2 | PCI-DSS | BCBS (OpRes) | ISO 22301 | ISO 27001 / SOC 2 |
|-----------|------|------|---------|-----------|-----------|-------------------|
| Scenario-based testing | Art. 25 | Art. 21(2)(f) | -- | OpRes P4 | 8.5 | A.5.30 |
| Recovery validation | Art. 11 | Art. 21(2)(c) | -- | OpRes P4 | 8.5 | CC7.5 |
| Testing frequency proof | Art. 24 | Art. 21(2)(f) | 12.10.2 | OpRes P4 | 8.5 | CC7.5 |
| Data integrity verification | Art. 11 | Art. 21(2)(c) | -- | OpRes P4 | -- | A.5.30 |
| Trend analysis / learning | Art. 24 | -- | -- | -- | 8.5 | -- |
| Incident-response testing | Art. 11 | Art. 21(2)(b) | 12.10.2 | -- | -- | CC7.4 |
| Threat-led testing (TLPT) — *indirect only* | Art. 26 | -- | -- | -- | -- | -- |
| Audit trail / evidence | Art. 24 | Art. 21(2)(f) | 10.5.1 | BCBS 239 P6 | 8.5 | CC7.5 |

*Removed as overreach:* PCI 11.4/11.4.1/11.4.2/11.4.4/11.4.5 (security penetration & segmentation testing — not what chaos experiments evidence). *Corrected:* A.17.1.3 → A.5.30 (2022 numbering); "CC7.4 — Detection and monitoring" → CC7.4 is incident response, monitoring is CC7.2; PCI retention 10.7 → 10.5.1; Basel III/BCBS 239 → BCBS Principles for Operational Resilience (P4) as the primary anchor.
