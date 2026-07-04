//! Regulatory compliance domain logic shared by the CLI and MCP server.
//!
//! Owns the framework catalog (names and canonical report identifiers), the
//! journal-derived compliance signals (pass rate and recovery compliance),
//! and the COMPLIANT / PARTIAL / NON-COMPLIANT verdict thresholds — one
//! source of truth so `tumult compliance` and the `tumult_compliance` MCP
//! tool cannot drift apart.

use crate::types::Journal;

/// Default MTTR target in seconds. Matches
/// [`ScoringConfig`](crate::types::ScoringConfig) `mttr_target_s` default.
pub const DEFAULT_MTTR_TARGET_S: f64 = 30.0;

/// A supported regulatory compliance framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComplianceFramework {
    /// EU Digital Operational Resilience Act (EU 2022/2554).
    Dora,
    /// EU Network and Information Security Directive (EU 2022/2555).
    Nis2,
    /// Payment Card Industry Data Security Standard 4.0.
    PciDss,
    /// ISO 22301 Business Continuity Management.
    Iso22301,
    /// ISO 27001 Information Security Management.
    Iso27001,
    /// SOC 2 Service Organization Control Type 2.
    Soc2,
    /// Basel III / BCBS 239 Risk Data Aggregation.
    BaselIii,
}

impl ComplianceFramework {
    /// Every supported framework, in display order.
    pub const ALL: [Self; 7] = [
        Self::Dora,
        Self::Nis2,
        Self::PciDss,
        Self::Iso22301,
        Self::Iso27001,
        Self::Soc2,
        Self::BaselIii,
    ];

    /// Parse a framework from its CLI value name (e.g. `dora`, `pci-dss`),
    /// case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns an error message naming the bad value and listing every valid
    /// value when `value` matches no framework.
    pub fn parse(value: &str) -> Result<Self, String> {
        let normalized = value.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|framework| framework.name() == normalized)
            .ok_or_else(|| {
                let valid: Vec<&str> = Self::ALL.iter().map(|f| f.name()).collect();
                format!(
                    "unknown framework '{value}'; valid values: {}",
                    valid.join(", ")
                )
            })
    }

    /// Stable lowercase identifier, matching the CLI's `--framework` values.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dora => "dora",
            Self::Nis2 => "nis2",
            Self::PciDss => "pci-dss",
            Self::Iso22301 => "iso-22301",
            Self::Iso27001 => "iso-27001",
            Self::Soc2 => "soc2",
            Self::BaselIii => "basel-iii",
        }
    }

    /// Canonical string identifier used in report output.
    #[must_use]
    pub fn as_report_str(self) -> &'static str {
        match self {
            Self::Dora => "DORA",
            Self::Nis2 => "NIS2",
            Self::PciDss => "PCI-DSS",
            Self::Iso22301 => "ISO-22301",
            Self::Iso27001 => "ISO-27001",
            Self::Soc2 => "SOC2",
            Self::BaselIii => "Basel-III",
        }
    }

    /// Full human-readable framework name with its source reference.
    #[must_use]
    pub fn full_name(self) -> &'static str {
        match self {
            Self::Dora => "DORA — Digital Operational Resilience Act (EU 2022/2554)",
            Self::Nis2 => "NIS2 — Network and Information Security Directive (EU 2022/2555)",
            Self::PciDss => "PCI-DSS 4.0 — Payment Card Industry Data Security Standard",
            Self::Iso22301 => "ISO 22301 — Business Continuity Management Systems",
            Self::Iso27001 => "ISO 27001 — Information Security Management Systems",
            Self::Soc2 => "SOC 2 — Service Organization Control Type 2 (TSC 2017)",
            Self::BaselIii => {
                "Basel Committee (BCBS) — operational resilience & risk data aggregation"
            }
        }
    }

    /// Primary official source URL for the framework (the canonical text an
    /// auditor would consult). Individual control citations carry their own
    /// [`Citation::source_url`], which may differ (e.g. the Basel operational
    /// resilience principles live in a different publication from BCBS 239).
    #[must_use]
    pub fn source_url(self) -> &'static str {
        match self {
            Self::Dora => "https://eur-lex.europa.eu/eli/reg/2022/2554/oj",
            Self::Nis2 => "https://eur-lex.europa.eu/eli/dir/2022/2555/oj",
            Self::PciDss => "https://www.pcisecuritystandards.org/document_library/",
            Self::Iso22301 => "https://www.iso.org/standard/75106.html",
            Self::Iso27001 => "https://www.iso.org/standard/27001",
            Self::Soc2 => {
                "https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2"
            }
            Self::BaselIii => "https://www.bis.org/bcbs/publ/d516.htm",
        }
    }

    /// All registry [`Citation`]s that map evidence to this framework, in
    /// registry order.
    #[must_use]
    pub fn citations(self) -> Vec<&'static Citation> {
        CITATIONS.iter().filter(|c| c.framework == self).collect()
    }
}

/// The kind of chaos-engineering evidence a Tumult experiment supplies toward
/// a regulatory control. Deliberately narrow: chaos experiments produce
/// resilience/recovery evidence, not (for example) threat-led penetration
/// testing or incident-reporting-timeline evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceType {
    /// Scenario-based fault injection proving a system behaves under stress.
    ResilienceTesting,
    /// Measured recovery (RTO/MTTR, rollback, data integrity) after a fault.
    RecoveryValidation,
    /// Business-continuity exercising/testing under a plausible scenario.
    ContinuityExercise,
    /// Baseline-vs-fault comparison demonstrating control effectiveness.
    EffectivenessAssessment,
    /// Observability/detection coverage evidenced during an experiment.
    Monitoring,
    /// Exercising of the incident-response process by a controlled fault.
    IncidentResponseTesting,
}

impl EvidenceType {
    /// Short human-readable label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResilienceTesting => "resilience testing",
            Self::RecoveryValidation => "recovery validation",
            Self::ContinuityExercise => "continuity exercise",
            Self::EffectivenessAssessment => "effectiveness assessment",
            Self::Monitoring => "monitoring",
            Self::IncidentResponseTesting => "incident-response testing",
        }
    }
}

/// How strongly a Tumult experiment can evidence a control. This is an
/// evidence-strength grade, not a compliance determination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceStrength {
    /// The control is directly about resilience/recovery testing; a passing
    /// experiment is first-class evidence toward it.
    Direct,
    /// The experiment corroborates the control but is not the whole of it.
    Supporting,
    /// The mapping is indirect; the experiment touches the control's subject
    /// but does not, on its own, evidence what the control actually requires.
    Indirect,
}

impl EvidenceStrength {
    /// Short human-readable label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Supporting => "supporting",
            Self::Indirect => "indirect",
        }
    }
}

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

/// Parse an ISO-8601 `YYYY-MM-DD` date into `(year, month)`. Returns `None`
/// on any malformed input.
#[must_use]
pub fn parse_year_month(date: &str) -> Option<(i64, u32)> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let _day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month))
}

/// Current `(year, month)` in UTC, derived from the system clock. Used by the
/// staleness audit so it reflects the real calendar date at check time.
#[must_use]
pub fn current_year_month() -> (i64, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    #[allow(clippy::cast_possible_wrap)]
    let days = (secs / 86_400) as i64;
    let (y, m, _d) = civil_from_days(days);
    (y, m)
}

/// Howard Hinnant's `civil_from_days`: convert days since the Unix epoch
/// (1970-01-01) into a proleptic-Gregorian `(year, month, day)`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe.cast_signed() + era * 400; // yoe ∈ [0, 399], no wrap
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// Whole months from `last_verified` to `(now_year, now_month)`. Negative if
/// the citation date is in the future. Returns `None` on a malformed date.
#[must_use]
pub fn months_since_verified(last_verified: &str, now: (i64, u32)) -> Option<i64> {
    let (vy, vm) = parse_year_month(last_verified)?;
    let verified_months = vy * 12 + i64::from(vm - 1);
    let now_months = now.0 * 12 + i64::from(now.1 - 1);
    Some(now_months - verified_months)
}

/// Every citation older than `max_age_months` as of `now`, i.e. due for
/// re-verification against its official source.
#[must_use]
pub fn stale_citations(now: (i64, u32), max_age_months: i64) -> Vec<&'static Citation> {
    CITATIONS
        .iter()
        .filter(|c| match months_since_verified(c.last_verified, now) {
            Some(age) => age > max_age_months,
            None => true,
        })
        .collect()
}

/// Journal-level compliance signals accumulated across a set of journals.
///
/// The analytics `experiments` table has no MTTR column and a true
/// `ResilienceScore` (with `recovery_compliance`) is a GameDay-only
/// aggregate, so recovery compliance is derived from `PostResult::mttr_s`,
/// falling back to `AnalysisResult::resilience_score`, then to pass-rate
/// only (`None`).
#[derive(Debug, Clone, Default)]
pub struct ComplianceSignals {
    /// Total journals accumulated.
    pub total_journals: usize,
    /// Journals whose status is `completed`.
    pub completed_journals: usize,
    /// Journals that declared a regulatory mapping.
    pub journals_with_regulatory: usize,
    /// Observed `mttr_s` values from post-phase results.
    pub mttrs: Vec<f64>,
    /// Observed `resilience_score` values from analysis results.
    pub resilience_scores: Vec<f64>,
}

impl ComplianceSignals {
    /// Fold one journal's signals into the accumulator.
    pub fn accumulate(&mut self, journal: &Journal) {
        use crate::types::ExperimentStatus;

        self.total_journals += 1;
        if matches!(journal.status, ExperimentStatus::Completed) {
            self.completed_journals += 1;
        }
        if journal.regulatory.is_some() {
            self.journals_with_regulatory += 1;
        }
        if let Some(score) = journal.analysis.as_ref().and_then(|a| a.resilience_score) {
            self.resilience_scores.push(score);
        }
        if let Some(mttr) = journal.post_result.as_ref().and_then(|p| p.mttr_s) {
            self.mttrs.push(mttr);
        }
    }

    /// Fraction of accumulated journals that completed (0.0 when empty).
    #[must_use]
    pub fn pass_rate(&self) -> f64 {
        if self.total_journals == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            self.completed_journals as f64 / self.total_journals as f64
        }
    }

    /// Recovery-compliance proxy: fraction of MTTRs at or under
    /// `mttr_target_s`; falls back to the average resilience score, and
    /// returns `None` when neither signal is present (pass-rate-only
    /// verdict, reduced assurance).
    #[must_use]
    pub fn recovery_compliance(&self, mttr_target_s: f64) -> Option<f64> {
        #[allow(clippy::cast_precision_loss)]
        if self.mttrs.is_empty() {
            if self.resilience_scores.is_empty() {
                None
            } else {
                Some(
                    self.resilience_scores.iter().sum::<f64>()
                        / self.resilience_scores.len() as f64,
                )
            }
        } else {
            Some(
                self.mttrs.iter().filter(|m| **m <= mttr_target_s).count() as f64
                    / self.mttrs.len() as f64,
            )
        }
    }
}

/// Recovery-aware compliance verdict.
///
/// The COMPLIANT / PARTIAL / NON-COMPLIANT verdict requires BOTH a pass rate
/// and a recovery signal. `recovery_compliance` is `None` when neither MTTR
/// nor `resilience_score` data is present in the journals, in which case the
/// verdict falls back to pass-rate-only thresholds (reduced assurance).
/// Thresholds are aligned with `ResilienceScore::status` (0.90 / 0.75).
#[must_use]
pub fn compliance_verdict(pass_rate: f64, recovery_compliance: Option<f64>) -> &'static str {
    match recovery_compliance {
        Some(rc) => {
            if pass_rate >= 0.95 && rc >= 0.90 {
                "COMPLIANT"
            } else if pass_rate >= 0.80 && rc >= 0.75 {
                "PARTIAL"
            } else {
                "NON-COMPLIANT"
            }
        }
        None => {
            if pass_rate >= 0.95 {
                "COMPLIANT (pass-rate only)"
            } else if pass_rate >= 0.80 {
                "PARTIAL (pass-rate only)"
            } else {
                "NON-COMPLIANT"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_every_cli_name_case_insensitively() {
        for framework in ComplianceFramework::ALL {
            assert_eq!(
                ComplianceFramework::parse(framework.name()).unwrap(),
                framework
            );
            assert_eq!(
                ComplianceFramework::parse(&framework.name().to_ascii_uppercase()).unwrap(),
                framework
            );
        }
    }

    #[test]
    fn parse_rejects_unknown_framework_listing_valid_values() {
        let err = ComplianceFramework::parse("hipaa").unwrap_err();
        assert!(err.contains("hipaa"), "must name the bad value: {err}");
        for framework in ComplianceFramework::ALL {
            assert!(
                err.contains(framework.name()),
                "must list '{}': {err}",
                framework.name()
            );
        }
    }

    #[test]
    fn verdict_requires_both_pass_and_recovery() {
        assert_eq!(compliance_verdict(1.0, Some(0.80)), "PARTIAL");
        assert_eq!(compliance_verdict(1.0, Some(0.50)), "NON-COMPLIANT");
        assert_eq!(compliance_verdict(0.96, Some(0.95)), "COMPLIANT");
        assert_eq!(compliance_verdict(0.85, Some(0.80)), "PARTIAL");
        assert_eq!(compliance_verdict(0.70, Some(0.99)), "NON-COMPLIANT");
    }

    #[test]
    fn verdict_pass_rate_only_fallback() {
        assert_eq!(compliance_verdict(0.96, None), "COMPLIANT (pass-rate only)");
        assert_eq!(compliance_verdict(0.85, None), "PARTIAL (pass-rate only)");
        assert_eq!(compliance_verdict(0.50, None), "NON-COMPLIANT");
    }

    #[test]
    fn signals_prefer_mttr_over_resilience_scores() {
        let signals = ComplianceSignals {
            total_journals: 4,
            completed_journals: 3,
            journals_with_regulatory: 0,
            mttrs: vec![10.0, 40.0],
            resilience_scores: vec![0.1],
        };
        assert!((signals.pass_rate() - 0.75).abs() < f64::EPSILON);
        // 1 of 2 MTTRs under target; resilience scores are ignored.
        assert_eq!(
            signals.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(0.5)
        );
    }

    #[test]
    fn signals_fall_back_to_resilience_scores_then_none() {
        let mut signals = ComplianceSignals {
            total_journals: 2,
            completed_journals: 2,
            journals_with_regulatory: 1,
            mttrs: vec![],
            resilience_scores: vec![0.8, 0.6],
        };
        let rc = signals
            .recovery_compliance(DEFAULT_MTTR_TARGET_S)
            .expect("resilience fallback");
        assert!((rc - 0.7).abs() < 1e-9);

        signals.resilience_scores.clear();
        assert_eq!(signals.recovery_compliance(DEFAULT_MTTR_TARGET_S), None);
    }

    #[test]
    fn empty_signals_have_zero_pass_rate() {
        let signals = ComplianceSignals::default();
        assert!(signals.pass_rate().abs() < f64::EPSILON);
    }

    fn bare_journal(status: crate::types::ExperimentStatus) -> Journal {
        Journal {
            experiment_title: "compliance test".into(),
            experiment_id: "exp-1".into(),
            status,
            started_at_ns: 0,
            ended_at_ns: 0,
            duration_ms: 0,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        }
    }

    #[test]
    fn registry_is_well_formed() {
        assert!(!CITATIONS.is_empty(), "registry must not be empty");
        for c in CITATIONS {
            assert!(!c.control_id.is_empty(), "control_id empty");
            assert!(!c.title.is_empty(), "title empty for {}", c.control_id);
            assert!(!c.summary.is_empty(), "summary empty for {}", c.control_id);
            assert!(
                !c.evidence_note.is_empty(),
                "evidence_note empty for {}",
                c.control_id
            );
            assert!(
                c.source_url.starts_with("https://"),
                "source_url must be https for {}: {}",
                c.control_id,
                c.source_url
            );
            assert!(
                parse_year_month(c.last_verified).is_some(),
                "last_verified must be YYYY-MM-DD for {}: {}",
                c.control_id,
                c.last_verified
            );
        }
    }

    #[test]
    fn every_framework_has_at_least_one_citation() {
        for framework in ComplianceFramework::ALL {
            assert!(
                !framework.citations().is_empty(),
                "{} has no citations",
                framework.as_report_str()
            );
        }
    }

    #[test]
    fn no_citation_is_stale() {
        // Fails once any citation exceeds CITATION_MAX_AGE_MONTHS as of today,
        // forcing a re-verification pass against the official sources. This is
        // the CI staleness gate.
        let now = current_year_month();
        let stale = stale_citations(now, CITATION_MAX_AGE_MONTHS);
        assert!(
            stale.is_empty(),
            "citations overdue for re-verification (>{CITATION_MAX_AGE_MONTHS} months): {}",
            stale
                .iter()
                .map(|c| format!(
                    "{} {} ({})",
                    c.framework.as_report_str(),
                    c.control_id,
                    c.last_verified
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    #[test]
    fn no_citation_is_dated_in_the_future() {
        let now = current_year_month();
        for c in CITATIONS {
            let age = months_since_verified(c.last_verified, now).expect("registry dates parse");
            assert!(
                age >= 0,
                "{} {} last_verified {} is in the future",
                c.framework.as_report_str(),
                c.control_id,
                c.last_verified
            );
        }
    }

    #[test]
    fn months_since_verified_math() {
        assert_eq!(months_since_verified("2025-01-15", (2026, 7)), Some(18));
        assert_eq!(months_since_verified("2026-07-01", (2026, 7)), Some(0));
        assert_eq!(months_since_verified("2026-08-01", (2026, 7)), Some(-1));
        assert_eq!(months_since_verified("not-a-date", (2026, 7)), None);
    }

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn accumulate_counts_completion_regulatory_and_metrics() {
        let mut signals = ComplianceSignals::default();
        signals.accumulate(&bare_journal(crate::types::ExperimentStatus::Completed));
        signals.accumulate(&bare_journal(crate::types::ExperimentStatus::Failed));

        assert_eq!(signals.total_journals, 2);
        assert_eq!(signals.completed_journals, 1);
        assert_eq!(signals.journals_with_regulatory, 0);
        assert!(signals.mttrs.is_empty());
        assert!(signals.resilience_scores.is_empty());
    }
}
