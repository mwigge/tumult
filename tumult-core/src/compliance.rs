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
            Self::Soc2 => "SOC 2 — Service Organization Control Type 2",
            Self::BaselIii => "Basel III — BCBS 239 Risk Data Aggregation",
        }
    }
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
        }
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
