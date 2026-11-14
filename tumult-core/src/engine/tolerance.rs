//! Tolerance evaluation and experiment status determination.

use std::collections::HashMap;

use crate::types::{ExperimentStatus, Tolerance};

/// Thread-safe cache of compiled regex patterns for tolerance checks.
static REGEX_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, regex_lite::Regex>>> =
    std::sync::OnceLock::new();

fn regex_cache() -> &'static std::sync::Mutex<HashMap<String, regex_lite::Regex>> {
    REGEX_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Evaluate a tolerance check: does the actual value match the expected?
#[must_use]
pub fn evaluate_tolerance(actual: &serde_json::Value, tolerance: &Tolerance) -> bool {
    match tolerance {
        Tolerance::Exact { value } => actual == value,
        Tolerance::Range { from, to } => {
            if let Some(n) = actual.as_f64() {
                n >= *from && n <= *to
            } else {
                false
            }
        }
        Tolerance::Regex { pattern } => {
            // Match against the value's text form. A bare JSON string matches on
            // its contents (`"ok"` → `ok`); any other JSON value (object,
            // number, bool, array) matches on its serialized form, so a regex
            // like `ok` works against a probe returning `{"status":"ok"}`.
            let haystack = match actual.as_str() {
                Some(s) => s.to_owned(),
                None => actual.to_string(),
            };
            let cache = regex_cache()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(re) = cache.get(pattern.as_str()) {
                return re.is_match(&haystack);
            }
            drop(cache);
            match regex_lite::Regex::new(pattern) {
                Ok(re) => {
                    let matched = re.is_match(&haystack);
                    let mut cache = regex_cache()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    cache.insert(pattern.clone(), re);
                    matched
                }
                Err(_) => false,
            }
        }
    }
}

/// Determine the experiment status from method results.
#[must_use]
pub fn determine_status(
    hypothesis_before_met: Option<bool>,
    hypothesis_after_met: Option<bool>,
    all_actions_succeeded: bool,
) -> ExperimentStatus {
    if let Some(false) = hypothesis_before_met {
        return ExperimentStatus::Aborted;
    }
    if !all_actions_succeeded {
        return ExperimentStatus::Failed;
    }
    if let Some(false) = hypothesis_after_met {
        return ExperimentStatus::Deviated;
    }
    ExperimentStatus::Completed
}

/// What the end of a run knows about itself, for [`conclude`].
// Independent facts, not a state machine in disguise.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunFacts {
    /// The caller's cancellation token fired.
    pub cancelled: bool,
    pub hypothesis_before_met: Option<bool>,
    pub hypothesis_after_met: Option<bool>,
    pub actions_succeeded: bool,
    /// The declared load sent at least one request (true without a load).
    pub load_has_evidence: bool,
    /// A blocking objective was breached.
    pub blocking_breach: bool,
    /// The load ran but missed its thresholds or its plan.
    pub load_missed_thresholds: bool,
    /// A fault-injecting (action) activity started in the method.
    pub fault_started: bool,
}

impl RunFacts {
    /// A run in which nothing went wrong.
    #[must_use]
    pub fn clean() -> Self {
        Self {
            cancelled: false,
            hypothesis_before_met: None,
            hypothesis_after_met: None,
            actions_succeeded: true,
            load_has_evidence: true,
            blocking_breach: false,
            load_missed_thresholds: false,
            fault_started: false,
        }
    }

    /// Anything that by itself makes a finished run a deviation.
    fn deviates(self) -> bool {
        self.hypothesis_after_met == Some(false)
            || self.blocking_breach
            || self.load_missed_thresholds
            || !self.load_has_evidence
    }
}

/// The status a run ends with and whether its rollbacks must run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conclusion {
    pub status: ExperimentStatus,
    pub needs_rollback: bool,
}

/// Conclude a run that got past its method.
///
/// Status, first match wins:
///
/// 1. `Interrupted` — the run was cancelled.
/// 2. `Aborted` — the hypothesis did not hold before the method.
/// 3. `Failed` — an action failed, or a declared load left no evidence.
/// 4. `Deviated` — the hypothesis did not hold afterwards, a blocking
///    objective was breached, or the load missed its thresholds or plan.
/// 5. `Completed`.
///
/// A higher status never hides the cleanup a lower one calls for:
/// rollbacks run whenever the facts amount to a deviation, whatever the
/// status reads, and after a failed or interrupted run in which a fault
/// was started.
#[must_use]
pub fn conclude(facts: &RunFacts) -> Conclusion {
    let status = concluded_status(*facts);
    let fault_left_behind = facts.fault_started
        && matches!(
            status,
            ExperimentStatus::Failed | ExperimentStatus::Interrupted
        );
    Conclusion {
        needs_rollback: facts.deviates() || fault_left_behind,
        status,
    }
}

fn concluded_status(facts: RunFacts) -> ExperimentStatus {
    if facts.cancelled {
        return ExperimentStatus::Interrupted;
    }
    let status = determine_status(
        facts.hypothesis_before_met,
        facts.hypothesis_after_met,
        // A declared load that left no evidence is a failed run, not a
        // completed one that happens to lack numbers.
        facts.actions_succeeded && facts.load_has_evidence,
    );
    if status == ExperimentStatus::Completed && facts.deviates() {
        return ExperimentStatus::Deviated;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    // ── conclude ───────────────────────────────────────────────

    #[test]
    fn a_clean_run_completes_without_rollbacks() {
        let conclusion = conclude(&RunFacts::clean());
        assert_eq!(conclusion.status, ExperimentStatus::Completed);
        assert!(!conclusion.needs_rollback);
    }

    #[test]
    fn status_precedence_is_interrupted_aborted_failed_deviated() {
        let everything_wrong = RunFacts {
            cancelled: true,
            hypothesis_before_met: Some(false),
            hypothesis_after_met: Some(false),
            actions_succeeded: false,
            load_has_evidence: false,
            blocking_breach: true,
            load_missed_thresholds: true,
            fault_started: true,
        };
        assert_eq!(
            conclude(&everything_wrong).status,
            ExperimentStatus::Interrupted
        );
        let not_cancelled = RunFacts {
            cancelled: false,
            ..everything_wrong
        };
        assert_eq!(conclude(&not_cancelled).status, ExperimentStatus::Aborted);
        let before_held = RunFacts {
            hypothesis_before_met: Some(true),
            ..not_cancelled
        };
        assert_eq!(conclude(&before_held).status, ExperimentStatus::Failed);
        let actions_fine = RunFacts {
            actions_succeeded: true,
            ..before_held
        };
        assert_eq!(
            conclude(&actions_fine).status,
            ExperimentStatus::Failed,
            "a load without evidence fails the run"
        );
        let load_ran = RunFacts {
            load_has_evidence: true,
            ..actions_fine
        };
        assert_eq!(conclude(&load_ran).status, ExperimentStatus::Deviated);
    }

    #[test]
    fn each_deviation_alone_deviates_and_rolls_back() {
        let cases = [
            RunFacts {
                hypothesis_after_met: Some(false),
                ..RunFacts::clean()
            },
            RunFacts {
                blocking_breach: true,
                ..RunFacts::clean()
            },
            RunFacts {
                load_missed_thresholds: true,
                ..RunFacts::clean()
            },
        ];
        for facts in cases {
            let conclusion = conclude(&facts);
            assert_eq!(conclusion.status, ExperimentStatus::Deviated, "{facts:?}");
            assert!(conclusion.needs_rollback, "{facts:?}");
        }
    }

    #[test]
    fn a_failed_status_does_not_hide_the_rollbacks_of_a_deviation() {
        // Probe-only method (no fault started): the load left no evidence
        // and the hypothesis did not hold afterwards. `Failed` wins the
        // status; the deviation still gets its rollbacks.
        let masked = RunFacts {
            hypothesis_after_met: Some(false),
            load_has_evidence: false,
            ..RunFacts::clean()
        };
        let conclusion = conclude(&masked);
        assert_eq!(conclusion.status, ExperimentStatus::Failed);
        assert!(conclusion.needs_rollback);

        // A load without evidence would have been a deviation of its own.
        let no_evidence = RunFacts {
            load_has_evidence: false,
            ..RunFacts::clean()
        };
        let conclusion = conclude(&no_evidence);
        assert_eq!(conclusion.status, ExperimentStatus::Failed);
        assert!(conclusion.needs_rollback);

        // An action failed and a blocking objective was breached.
        let failed_and_breached = RunFacts {
            actions_succeeded: false,
            blocking_breach: true,
            ..RunFacts::clean()
        };
        let conclusion = conclude(&failed_and_breached);
        assert_eq!(conclusion.status, ExperimentStatus::Failed);
        assert!(conclusion.needs_rollback);
    }

    #[test]
    fn a_failure_or_interrupt_rolls_back_only_what_was_started() {
        let failed_probe_only = RunFacts {
            actions_succeeded: false,
            ..RunFacts::clean()
        };
        assert!(!conclude(&failed_probe_only).needs_rollback);
        let failed_after_fault = RunFacts {
            fault_started: true,
            ..failed_probe_only
        };
        assert!(conclude(&failed_after_fault).needs_rollback);

        let interrupted = RunFacts {
            cancelled: true,
            ..RunFacts::clean()
        };
        assert!(!conclude(&interrupted).needs_rollback);
        let interrupted_after_fault = RunFacts {
            fault_started: true,
            ..interrupted
        };
        assert!(conclude(&interrupted_after_fault).needs_rollback);

        // A fault alone, in a run that completed, needs no cleanup.
        let completed_after_fault = RunFacts {
            fault_started: true,
            ..RunFacts::clean()
        };
        assert!(!conclude(&completed_after_fault).needs_rollback);
    }

    // ── determine_status ───────────────────────────────────────

    #[test]
    fn status_completed_when_all_pass() {
        assert_eq!(
            determine_status(Some(true), Some(true), true),
            ExperimentStatus::Completed
        );
    }

    #[test]
    fn status_deviated_when_after_hypothesis_fails() {
        assert_eq!(
            determine_status(Some(true), Some(false), true),
            ExperimentStatus::Deviated
        );
    }

    #[test]
    fn status_aborted_when_before_hypothesis_fails() {
        assert_eq!(
            determine_status(Some(false), None, true),
            ExperimentStatus::Aborted
        );
    }

    #[test]
    fn status_failed_when_actions_fail() {
        assert_eq!(
            determine_status(Some(true), Some(true), false),
            ExperimentStatus::Failed
        );
    }

    #[test]
    fn status_completed_when_no_hypothesis() {
        assert_eq!(
            determine_status(None, None, true),
            ExperimentStatus::Completed
        );
    }

    // ── evaluate_tolerance ─────────────────────────────────────

    #[test]
    fn exact_tolerance_matches_integer() {
        let actual = serde_json::Value::Number(200.into());
        let tolerance = Tolerance::Exact {
            value: serde_json::Value::Number(200.into()),
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn exact_tolerance_rejects_mismatch() {
        let actual = serde_json::Value::Number(500.into());
        let tolerance = Tolerance::Exact {
            value: serde_json::Value::Number(200.into()),
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn exact_tolerance_matches_string() {
        let actual = serde_json::Value::String("OK".into());
        let tolerance = Tolerance::Exact {
            value: serde_json::Value::String("OK".into()),
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn range_tolerance_accepts_within() {
        let actual = serde_json::json!(50.0);
        let tolerance = Tolerance::Range {
            from: 0.0,
            to: 100.0,
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn range_tolerance_rejects_outside() {
        let actual = serde_json::json!(150.0);
        let tolerance = Tolerance::Range {
            from: 0.0,
            to: 100.0,
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn regex_tolerance_matches_pattern() {
        let actual = serde_json::Value::String("OK: all systems operational".into());
        let tolerance = Tolerance::Regex {
            pattern: "^OK".into(),
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn regex_tolerance_rejects_non_match() {
        let actual = serde_json::Value::String("ERROR: timeout".into());
        let tolerance = Tolerance::Regex {
            pattern: "^OK".into(),
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn regex_tolerance_matches_against_json_object_body() {
        // A probe curling a JSON health endpoint: output parses to an object,
        // and a bare-substring pattern must still match its serialized form.
        let actual = serde_json::json!({ "status": "ok" });
        let tolerance = Tolerance::Regex {
            pattern: "ok".into(),
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn regex_tolerance_matches_against_number() {
        let actual = serde_json::json!(42);
        let tolerance = Tolerance::Regex {
            pattern: "^4".into(),
        };
        assert!(evaluate_tolerance(&actual, &tolerance));
    }

    #[test]
    fn regex_tolerance_rejects_non_match_on_json_object() {
        let actual = serde_json::json!({ "status": "degraded" });
        let tolerance = Tolerance::Regex {
            pattern: "\"status\":\"ok\"".into(),
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }
}
