//! Journal-derived compliance signals and the verdict thresholds.

use crate::types::{ExperimentStatus, Journal, PostResult};

/// Default MTTR target in seconds. Matches
/// [`ScoringConfig`](crate::types::ScoringConfig) `mttr_target_s` default.
/// It is the target of a run that declares no `rto_target_s` of its own.
pub const DEFAULT_MTTR_TARGET_S: f64 = 30.0;

/// What a run that watched for recovery measured of it: the recovery time,
/// or a watch to the end that did not see the target come back, and the
/// recovery time objective of the run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecoveryMeasure {
    /// Seconds from the outage seen to the recovery seen. `None` for a run
    /// that watched to the end and did not see its target come back.
    pub recovery_s: Option<f64>,
    /// The `rto_target_s` the run declared, if any.
    pub rto_target_s: Option<f64>,
}

impl RecoveryMeasure {
    /// Whether the run recovered within its own RTO, or within
    /// `default_target_s` when it declares none. A recovery that took
    /// exactly the target is within it.
    #[must_use]
    pub fn within(&self, default_target_s: f64) -> bool {
        let target = self.rto_target_s.unwrap_or(default_target_s);
        self.recovery_s
            .is_some_and(|recovery_s| recovery_s <= target)
    }
}

/// Journal-level compliance signals accumulated across a set of journals.
///
/// They count two things that are not the same. The pass rate is the share
/// of the runs that completed. The recovery is the share of the runs that
/// measured a recovery and saw their target come back within its target
/// time: the RTO of the run, or a default for a run that declares none.
/// A run watches for recovery when it has a post phase, and that takes a
/// steady state to probe. A run that only completed says nothing of
/// recovery, so it is in the pass rate and nowhere else. A run that
/// watched and measured no recovery (its probes saw no outage, its method
/// failed, or a stop cut the watch short) is left out of the share, unless
/// the recovery is required.
#[derive(Debug, Clone, Default)]
pub struct ComplianceSignals {
    /// Total journals accumulated.
    pub total_journals: usize,
    /// Journals whose status is `completed`.
    pub completed_journals: usize,
    /// Journals that declared a regulatory mapping.
    pub journals_with_regulatory: usize,
    /// What each run that measured a recovery measured.
    pub recoveries: Vec<RecoveryMeasure>,
    /// Journals of runs that watched for recovery and measured none.
    pub recovery_unmeasured: usize,
}

impl ComplianceSignals {
    /// Fold one journal's signals into the accumulator.
    pub fn accumulate(&mut self, journal: &Journal) {
        self.total_journals += 1;
        if matches!(journal.status, ExperimentStatus::Completed) {
            self.completed_journals += 1;
        }
        if journal.regulatory.is_some() {
            self.journals_with_regulatory += 1;
        }
        let Some(post) = journal.post_result.as_ref() else {
            return;
        };
        match recovery_measure(journal, post) {
            Some(measure) => self.recoveries.push(measure),
            None => self.recovery_unmeasured += 1,
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

    /// How many runs saw their target recover within their own RTO, or
    /// within `default_target_s` for a run that declares none.
    #[must_use]
    pub fn recovered_within(&self, default_target_s: f64) -> usize {
        self.recoveries
            .iter()
            .filter(|measure| measure.within(default_target_s))
            .count()
    }

    /// Of the runs that measured a recovery, the share that saw their target
    /// recover within its target (see
    /// [`recovered_within`](Self::recovered_within)). A run that watched to
    /// the end and saw no recovery is not within it. A run that measured
    /// none is left out. `None` when no run measured a recovery: the
    /// verdict then rests on the pass rate only (reduced assurance).
    #[must_use]
    pub fn recovery_compliance(&self, default_target_s: f64) -> Option<f64> {
        self.share_within(default_target_s, self.recoveries.len())
    }

    /// Like [`recovery_compliance`](Self::recovery_compliance), for runs
    /// that must show a recovery: a run that watched and measured none
    /// counts as a recovery that failed. `None` when no run watched for
    /// recovery.
    #[must_use]
    pub fn required_recovery_compliance(&self, default_target_s: f64) -> Option<f64> {
        let watched = self.recoveries.len() + self.recovery_unmeasured;
        self.share_within(default_target_s, watched)
    }

    /// The share of `runs` that recovered within their target, or `None`
    /// when there are no runs.
    fn share_within(&self, default_target_s: f64, runs: usize) -> Option<f64> {
        if runs == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            Some(self.recovered_within(default_target_s) as f64 / runs as f64)
        }
    }

    /// What a report says of recovery, in one line: the share of
    /// [`recovery_compliance`](Self::recovery_compliance), the runs it is
    /// of and the targets they were judged against, or that no run
    /// measured a recovery.
    #[must_use]
    pub fn recovery_line(&self, default_target_s: f64) -> String {
        let Some(share) = self.recovery_compliance(default_target_s) else {
            return "Recovery within target: not measured, no run measured a recovery. \
                    The verdict rests on the pass rate only (reduced assurance)."
                .to_string();
        };
        let own = self
            .recoveries
            .iter()
            .filter(|measure| measure.rto_target_s.is_some())
            .count();
        let line = format!(
            "Recovery within target: {:.1}% of the runs that measured a recovery \
             ({} of {}; {own} against their own RTO, {} against {default_target_s}s)",
            share * 100.0,
            self.recovered_within(default_target_s),
            self.recoveries.len(),
            self.recoveries.len() - own,
        );
        if self.recovery_unmeasured == 0 {
            return line;
        }
        format!(
            "{line}. Left out: {} run(s) that measured no recovery.",
            self.recovery_unmeasured
        )
    }
}

/// What a run that watched for recovery measured of it, or `None` when it
/// measured nothing: its probes saw no outage (it ended recovered, with no
/// time to recovery), an action of its method failed, or a stop cut the
/// watch short. A run that watched to the end and did not see its target
/// come back measured a recovery that failed. The rule is the one of the
/// recovery time verdict of the runner.
fn recovery_measure(journal: &Journal, post: &PostResult) -> Option<RecoveryMeasure> {
    let rto_target_s = journal.objectives.as_ref().and_then(|o| o.rto_target_s);
    if let Some(recovery_s) = post.mttr_s {
        return Some(RecoveryMeasure {
            recovery_s: Some(recovery_s),
            rto_target_s,
        });
    }
    let watched_to_the_end = crate::execution::actions_succeeded(&journal.method_results)
        && journal.status != ExperimentStatus::Interrupted;
    let not_back = watched_to_the_end && !post.full_recovery;
    not_back.then_some(RecoveryMeasure {
        recovery_s: None,
        rto_target_s,
    })
}

/// Recovery-aware compliance verdict.
///
/// The COMPLIANT / PARTIAL / NON-COMPLIANT verdict requires BOTH a pass rate
/// and a recovery signal. `recovery_compliance` is `None` when no run
/// measured a recovery, in which case the verdict falls back to
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
