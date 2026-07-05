//! Journal-derived compliance signals and the verdict thresholds.

use crate::types::Journal;

/// Default MTTR target in seconds. Matches
/// [`ScoringConfig`](crate::types::ScoringConfig) `mttr_target_s` default.
pub const DEFAULT_MTTR_TARGET_S: f64 = 30.0;

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
