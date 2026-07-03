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
            if let Some(s) = actual.as_str() {
                let cache = regex_cache()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(re) = cache.get(pattern.as_str()) {
                    return re.is_match(s);
                }
                drop(cache);
                match regex_lite::Regex::new(pattern) {
                    Ok(re) => {
                        let matched = re.is_match(s);
                        let mut cache = regex_cache()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        cache.insert(pattern.clone(), re);
                        matched
                    }
                    Err(_) => false,
                }
            } else {
                false
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

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
    fn regex_tolerance_returns_false_for_non_string() {
        let actual = serde_json::json!(42);
        let tolerance = Tolerance::Regex {
            pattern: ".*".into(),
        };
        assert!(!evaluate_tolerance(&actual, &tolerance));
    }
}
