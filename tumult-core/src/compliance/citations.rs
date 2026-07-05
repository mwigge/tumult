//! The compliance mapping registry — the single, dated, sourced source of
//! truth mapping regulatory controls to the evidence Tumult experiments
//! supply toward them.

use super::evidence::{EvidenceStrength, EvidenceType};
use super::framework::ComplianceFramework;

/// One row of the compliance mapping registry: a single regulatory control,
/// the official source it comes from, the date that citation was last
/// verified against that source, and the evidence a Tumult experiment
/// supplies toward it.
///
/// This is the single source of truth shared by the CLI `compliance` command
/// and the `tumult_compliance` MCP tool. Editing a citation here updates both
/// surfaces and the sources listing at once — there are no hardcoded article
/// strings elsewhere.
#[derive(Debug, Clone, Copy)]
pub struct Citation {
    /// Framework this control belongs to.
    pub framework: ComplianceFramework,
    /// Control / article / requirement identifier, e.g. `Art. 25`, `CC7.5`,
    /// `A.5.30`, `Req. 12.10.2`, `Principle 4`.
    pub control_id: &'static str,
    /// Official title of the control.
    pub title: &'static str,
    /// One-line summary of what the control actually requires (paraphrased
    /// from the official text).
    pub summary: &'static str,
    /// The evidence type a Tumult experiment supplies toward this control.
    pub evidence_type: EvidenceType,
    /// Evidence-strength grade of the mapping.
    pub strength: EvidenceStrength,
    /// How a Tumult experiment provides evidence — worded as EVIDENCE toward
    /// the control, never as a guarantee of compliance.
    pub evidence_note: &'static str,
    /// Official source URL for this specific control.
    pub source_url: &'static str,
    /// ISO-8601 date (`YYYY-MM-DD`) this citation was last checked against the
    /// official source. Drives the staleness audit.
    pub last_verified: &'static str,
}

/// Version of the citation registry. Bump on any citation change so audit
/// exports can be pinned to a known mapping set.
pub const REGISTRY_VERSION: &str = "2026.07";

/// Number of months after which a citation is considered stale and must be
/// re-verified against its official source. Enforced by a test so drift fails
/// CI rather than silently rotting.
pub const CITATION_MAX_AGE_MONTHS: i64 = 18;

/// Shown alongside every compliance report. Chaos-engineering evidence is
/// evidence *toward* controls; it is not a compliance determination.
pub const EVIDENCE_DISCLAIMER: &str =
    "Tumult experiments produce technical EVIDENCE toward the controls below. \
Passing experiments do not by themselves establish regulatory compliance or constitute a legal \
attestation; a compliance determination requires assessment by a qualified auditor against the \
official source text. Citations are dated (last_verified) and must be re-checked against the \
official source before relying on them.";

/// The compliance mapping registry — the single, dated, sourced source of
/// truth. Every citation traces to an official source verified on
/// `last_verified`; see `docs/regulatory-mapping.md` for the narrative.
pub const CITATIONS: &[Citation] = &[
    // ── DORA — Regulation (EU) 2022/2554, Chapter IV (Arts. 24–27) + Art. 11 ──
    Citation {
        framework: ComplianceFramework::Dora,
        control_id: "Art. 24",
        title: "General requirements for the performance of digital operational resilience testing",
        summary: "Establish, maintain and review a sound, comprehensive ICT resilience testing \
programme, proportionate to risk and covering ICT systems supporting critical or important functions.",
        evidence_type: EvidenceType::ResilienceTesting,
        strength: EvidenceStrength::Supporting,
        evidence_note: "Experiment definitions (hypothesis, method, rollbacks) and a run history of \
executed journals evidence a maintained, risk-based testing programme.",
        source_url: "https://eur-lex.europa.eu/eli/reg/2022/2554/oj",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::Dora,
        control_id: "Art. 25",
        title: "Testing of ICT tools and systems",
        summary: "Perform appropriate tests — including scenario-based tests, performance testing \
and end-to-end testing — on systems supporting critical or important functions at least yearly.",
        evidence_type: EvidenceType::ResilienceTesting,
        strength: EvidenceStrength::Direct,
        evidence_note: "Scenario-based fault-injection experiments with baseline/during-fault \
statistics are direct evidence of scenario-based and performance testing; journal timestamps prove \
at-least-yearly cadence.",
        source_url: "https://eur-lex.europa.eu/eli/reg/2022/2554/oj",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::Dora,
        control_id: "Art. 26",
        title: "Advanced testing of ICT tools, systems and processes based on TLPT",
        summary: "Systemically important entities carry out threat-led penetration testing (TLPT) \
on live production systems at least every three years, per the TLPT RTS and TIBER-EU.",
        evidence_type: EvidenceType::ResilienceTesting,
        strength: EvidenceStrength::Indirect,
        evidence_note: "NOTE: Tumult experiments are resilience tests, NOT threat-led penetration \
tests, and do not satisfy TLPT. They can inform TLPT scenario design and evidence recovery under \
the scenarios a red team might trigger, nothing more.",
        source_url: "https://eur-lex.europa.eu/eli/reg/2022/2554/oj",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::Dora,
        control_id: "Art. 11",
        title: "Response and recovery",
        summary: "Put in place ICT business continuity plans and ICT response and recovery plans, \
including backup and restoration procedures, and test them.",
        evidence_type: EvidenceType::RecoveryValidation,
        strength: EvidenceStrength::Direct,
        evidence_note: "Phase-3 recovery measurement (recovery duration/MTTR, rollback verification, \
data-integrity checks) against a declared RTO is direct evidence that response and recovery plans \
were exercised and measured.",
        source_url: "https://eur-lex.europa.eu/eli/reg/2022/2554/oj",
        last_verified: "2026-07-04",
    },
    // ── NIS2 — Directive (EU) 2022/2555, Art. 21(2) ──
    Citation {
        framework: ComplianceFramework::Nis2,
        control_id: "Art. 21(2)(c)",
        title: "Business continuity, backup management, disaster recovery and crisis management",
        summary: "Take measures for business continuity, such as backup management and disaster \
recovery, and crisis management.",
        evidence_type: EvidenceType::RecoveryValidation,
        strength: EvidenceStrength::Direct,
        evidence_note: "Fault-injection experiments that measure recovery and verify data integrity \
evidence that backup/disaster-recovery and continuity measures function.",
        source_url: "https://eur-lex.europa.eu/eli/dir/2022/2555/oj",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::Nis2,
        control_id: "Art. 21(2)(b)",
        title: "Incident handling",
        summary: "Take measures for incident handling.",
        evidence_type: EvidenceType::IncidentResponseTesting,
        strength: EvidenceStrength::Supporting,
        evidence_note: "Controlled fault injection exercises incident-handling procedures. NB: the \
separate incident-REPORTING obligations of Art. 23 (24h/72h/1-month timelines) are a process, not \
something a chaos experiment evidences.",
        source_url: "https://eur-lex.europa.eu/eli/dir/2022/2555/oj",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::Nis2,
        control_id: "Art. 21(2)(f)",
        title: "Policies to assess the effectiveness of cybersecurity risk-management measures",
        summary: "Have policies and procedures to assess the effectiveness of cybersecurity \
risk-management measures.",
        evidence_type: EvidenceType::EffectivenessAssessment,
        strength: EvidenceStrength::Supporting,
        evidence_note: "Baseline-vs-during-fault comparison demonstrates whether resilience controls \
are effective, contributing to the effectiveness-assessment evidence base.",
        source_url: "https://eur-lex.europa.eu/eli/dir/2022/2555/oj",
        last_verified: "2026-07-04",
    },
    // ── PCI-DSS 4.0 ──
    Citation {
        framework: ComplianceFramework::PciDss,
        control_id: "Req. 12.10.2",
        title: "Incident response plan reviewed and tested at least once every 12 months",
        summary: "Review and test the incident response plan at least once every 12 months, \
including all elements listed in Requirement 12.10.1.",
        evidence_type: EvidenceType::IncidentResponseTesting,
        strength: EvidenceStrength::Supporting,
        evidence_note: "Experiments that trigger incident-response procedures, with dated journals, \
contribute evidence that the IR plan was exercised. NOTE: chaos experiments are NOT the security \
penetration testing of Req. 11.4, nor the segmentation-control testing of Req. 11.4.5 — do not map \
resilience experiments to those requirements.",
        source_url: "https://www.pcisecuritystandards.org/document_library/",
        last_verified: "2026-07-04",
    },
    // ── ISO 22301:2019 ──
    Citation {
        framework: ComplianceFramework::Iso22301,
        control_id: "Clause 8.5",
        title: "Exercise programme (exercising and testing)",
        summary: "Implement and maintain a programme of exercises and tests that validates business \
continuity arrangements against realistic scenarios and produces formal post-exercise reports.",
        evidence_type: EvidenceType::ContinuityExercise,
        strength: EvidenceStrength::Direct,
        evidence_note: "Scenario-based experiments with generated post-exercise reports and trend \
analysis are direct evidence of a maintained exercise programme.",
        source_url: "https://www.iso.org/standard/75106.html",
        last_verified: "2026-07-04",
    },
    // ── ISO 27001:2022 (Annex A, 2022 control numbering) ──
    Citation {
        framework: ComplianceFramework::Iso27001,
        control_id: "A.5.30",
        title: "ICT readiness for business continuity",
        summary: "Plan, implement, maintain and TEST ICT continuity based on business continuity \
objectives and ICT continuity requirements. (2022 control; replaces the withdrawn 2013 A.17.)",
        evidence_type: EvidenceType::RecoveryValidation,
        strength: EvidenceStrength::Direct,
        evidence_note: "Recovery-measuring experiments are direct evidence that ICT continuity is \
tested. This is the correct 2022 control — the earlier A.17.1.3 citation was from the withdrawn \
ISO/IEC 27001:2013.",
        source_url: "https://www.iso.org/standard/27001",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::Iso27001,
        control_id: "A.5.29",
        title: "Information security during disruption",
        summary: "Plan how to maintain information security at an appropriate level during \
disruption.",
        evidence_type: EvidenceType::ResilienceTesting,
        strength: EvidenceStrength::Supporting,
        evidence_note: "During-fault observations evidence that security-relevant behaviour is \
understood and maintained under disruption.",
        source_url: "https://www.iso.org/standard/27001",
        last_verified: "2026-07-04",
    },
    // ── SOC 2 — Trust Services Criteria (2017) ──
    Citation {
        framework: ComplianceFramework::Soc2,
        control_id: "CC7.5",
        title: "Recovery from identified security incidents",
        summary: "Identify, develop and implement activities to recover from identified security \
incidents.",
        evidence_type: EvidenceType::RecoveryValidation,
        strength: EvidenceStrength::Direct,
        evidence_note: "Phase-3 recovery evidence with MTTR against a defined objective directly \
evidences tested recovery activities.",
        source_url: "https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::Soc2,
        control_id: "CC7.4",
        title: "Responds to identified security incidents",
        summary: "Respond to identified security incidents by executing a defined incident-response \
programme. (This is incident response — detection/monitoring is CC7.1/CC7.2, not CC7.4.)",
        evidence_type: EvidenceType::IncidentResponseTesting,
        strength: EvidenceStrength::Supporting,
        evidence_note: "Controlled faults exercise the incident-response programme, contributing \
evidence that it functions.",
        source_url: "https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::Soc2,
        control_id: "CC7.2",
        title: "Monitors system components for anomalies",
        summary: "Monitor system components and their operation for anomalies indicative of \
malicious acts, natural disasters and errors.",
        evidence_type: EvidenceType::Monitoring,
        strength: EvidenceStrength::Supporting,
        evidence_note: "Observability data (OTel traces/metrics) captured during experiments \
evidences monitoring coverage of the affected components.",
        source_url: "https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2",
        last_verified: "2026-07-04",
    },
    // ── Basel Committee (BCBS) — operational resilience & risk data aggregation ──
    Citation {
        framework: ComplianceFramework::BaselIii,
        control_id: "OpRes Principle 4",
        title: "Business continuity planning and testing (BCBS Principles for Operational Resilience, 2021)",
        summary: "Have business continuity plans and conduct business continuity exercises under a \
range of severe but plausible scenarios to test the ability to deliver critical operations through \
disruption.",
        evidence_type: EvidenceType::ContinuityExercise,
        strength: EvidenceStrength::Direct,
        evidence_note: "Severe-but-plausible fault-injection experiments with recovery measurement \
are direct evidence of business continuity exercises. This is a better anchor than BCBS 239, whose \
subject is risk-data aggregation rather than resilience testing.",
        source_url: "https://www.bis.org/bcbs/publ/d516.htm",
        last_verified: "2026-07-04",
    },
    Citation {
        framework: ComplianceFramework::BaselIii,
        control_id: "BCBS 239 Principle 6",
        title: "Adaptability (Principles for effective risk data aggregation and risk reporting)",
        summary: "Be able to generate aggregate risk data to meet a broad range of on-demand, ad \
hoc reporting requests, including during stress/crisis situations.",
        evidence_type: EvidenceType::EffectivenessAssessment,
        strength: EvidenceStrength::Indirect,
        evidence_note: "NOTE: BCBS 239 concerns risk-DATA aggregation, not infrastructure \
resilience. Experiments that keep reporting/data systems available under fault only indirectly \
touch this principle; it is retained for continuity but should not be over-claimed.",
        source_url: "https://www.bis.org/publ/bcbs239.htm",
        last_verified: "2026-07-04",
    },
];
