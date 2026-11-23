//! Journal-derived compliance signals and the verdict thresholds.

use crate::types::Journal;

/// Default MTTR target in seconds. Matches
/// [`ScoringConfig`](crate::types::ScoringConfig) `mttr_target_s` default.
pub const DEFAULT_MTTR_TARGET_S: f64 = 30.0;

/// Journal-level compliance signals accumulated across a set of journals.
///
/// They count two things that are not the same. The pass rate is the share
/// of the runs that completed. The recovery is the share of the runs that
/// watched their target after the fault and saw it come back within the
/// target time. A run watches for recovery when it has a post phase, and
/// that takes a steady state to probe. A run that only completed says
/// nothing of recovery, so it is in the pass rate and nowhere else. Nor
/// does a run whose probes saw no outage: nothing recovered.
#[derive(Debug, Clone, Default)]
pub struct ComplianceSignals {
    /// Total journals accumulated.
    pub total_journals: usize,
    /// Journals whose status is `completed`.
    pub completed_journals: usize,
    /// Journals that declared a regulatory mapping.
    pub journals_with_regulatory: usize,
    /// Journals of runs that watched for recovery: they have a post phase,
    /// and their probes did not say that they saw no outage.
    pub recovery_watched: usize,
    /// The recovery times, in seconds, of the watched runs that saw their
    /// target recover.
    pub mttrs: Vec<f64>,
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
        let watched = journal.post_result.as_ref();
        if watched.is_some_and(|post| post.impact_observed != Some(false)) {
            self.recovery_watched += 1;
        }
        if let Some(recovery_time_s) = journal.recovery_time_s() {
            self.mttrs.push(recovery_time_s);
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

    /// How many runs saw their target recover within `mttr_target_s`
    /// seconds. A recovery that took exactly the target is within it.
    #[must_use]
    pub fn recovered_within(&self, mttr_target_s: f64) -> usize {
        self.mttrs
            .iter()
            .filter(|mttr| **mttr <= mttr_target_s)
            .count()
    }

    /// Of the runs that watched for recovery, the share that saw their
    /// target recover within `mttr_target_s` seconds. A watched run that saw
    /// no recovery is not within the target. `None` when no run watched:
    /// the verdict then rests on the pass rate only (reduced assurance).
    #[must_use]
    pub fn recovery_compliance(&self, mttr_target_s: f64) -> Option<f64> {
        if self.recovery_watched == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            Some(self.recovered_within(mttr_target_s) as f64 / self.recovery_watched as f64)
        }
    }

    /// What a report says of recovery, in one line: the share of
    /// [`recovery_compliance`](Self::recovery_compliance) and the runs it
    /// is of, or that no run watched for recovery.
    #[must_use]
    pub fn recovery_line(&self, mttr_target_s: f64) -> String {
        let Some(share) = self.recovery_compliance(mttr_target_s) else {
            return format!(
                "Recovery within {mttr_target_s}s: not measured, no run watched for recovery. \
                 The verdict rests on the pass rate only (reduced assurance)."
            );
        };
        format!(
            "Recovery within {mttr_target_s}s: {:.1}% of the runs that watched for recovery \
             ({} of {})",
            share * 100.0,
            self.recovered_within(mttr_target_s),
            self.recovery_watched
        )
    }
}

/// Recovery-aware compliance verdict.
///
/// The COMPLIANT / PARTIAL / NON-COMPLIANT verdict requires BOTH a pass rate
/// and a recovery signal. `recovery_compliance` is `None` when no run
/// watched for recovery, in which case the verdict falls back to
/// pass-rate-only thresholds (reduced assurance). Thresholds are aligned
/// with `ResilienceScore::status` (0.90 / 0.75).
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
