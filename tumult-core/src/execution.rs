//! Method and rollback execution logic.

use crate::types::{Activity, ActivityResult, ActivityStatus, ActivityType, SpanId, TraceId};

use thiserror::Error;

// Retained for future provider-level error propagation.
#[allow(dead_code)]
#[derive(Error, Debug)]
pub enum ExecutionError {
    #[error("activity '{name}' failed: {reason}")]
    ActivityFailed { name: String, reason: String },
}

/// Rollback strategy — when to execute rollbacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackStrategy {
    /// Always execute rollbacks regardless of outcome.
    Always,
    /// Only execute rollbacks when deviation is detected.
    OnDeviation,
    /// Never execute rollbacks.
    Never,
}

/// Determine if rollbacks should execute given the strategy and experiment outcome.
#[must_use]
pub fn should_rollback(strategy: &RollbackStrategy, deviated: bool) -> bool {
    match strategy {
        RollbackStrategy::Always => true,
        RollbackStrategy::OnDeviation => deviated,
        RollbackStrategy::Never => false,
    }
}

/// Parameters for creating an `ActivityResult`.
pub struct ResultParams<'a> {
    pub activity: &'a Activity,
    pub started_at_ns: i64,
    pub duration_ms: u64,
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    pub trace_id: TraceId,
    pub span_id: SpanId,
}

/// Create an `ActivityResult` from execution outcome.
#[must_use]
pub fn make_result(params: ResultParams<'_>) -> ActivityResult {
    ActivityResult {
        name: params.activity.name.clone(),
        activity_type: params.activity.activity_type.clone(),
        status: if params.success {
            ActivityStatus::Succeeded
        } else {
            ActivityStatus::Failed
        },
        started_at_ns: params.started_at_ns,
        duration_ms: params.duration_ms,
        output: params.output,
        error: params.error,
        trace_id: params.trace_id,
        span_id: params.span_id,
    }
}

/// Check if all activity results succeeded.
#[must_use]
pub fn all_succeeded(results: &[ActivityResult]) -> bool {
    !results.is_empty()
        && results
            .iter()
            .all(|r| r.status == ActivityStatus::Succeeded)
}

/// Count activities by type in a method.
#[must_use]
pub fn count_by_type(activities: &[Activity], activity_type: &ActivityType) -> usize {
    activities
        .iter()
        .filter(|a| a.activity_type == *activity_type)
        .count()
}

/// Separate method steps into sequential and background activities.
#[must_use]
pub fn partition_background(activities: &[Activity]) -> (Vec<&Activity>, Vec<&Activity>) {
    let mut sequential = Vec::new();
    let mut background = Vec::new();
    for activity in activities {
        if activity.background {
            background.push(activity);
        } else {
            sequential.push(activity);
        }
    }
    (sequential, background)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use std::collections::HashMap;

    fn test_action(name: &str, background: bool) -> Activity {
        Activity {
            name: name.into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "test".into(),
                function: "noop".into(),
                arguments: HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background,
        }
    }

    fn test_probe(name: &str) -> Activity {
        Activity {
            name: name.into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://localhost/health".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        }
    }

    // ── should_rollback ────────────────────────────────────────

    #[test]
    fn rollback_always_returns_true_regardless() {
        assert!(should_rollback(&RollbackStrategy::Always, false));
        assert!(should_rollback(&RollbackStrategy::Always, true));
    }

    #[test]
    fn rollback_on_deviation_only_when_deviated() {
        assert!(should_rollback(&RollbackStrategy::OnDeviation, true));
        assert!(!should_rollback(&RollbackStrategy::OnDeviation, false));
    }

    #[test]
    fn rollback_never_returns_false() {
        assert!(!should_rollback(&RollbackStrategy::Never, false));
        assert!(!should_rollback(&RollbackStrategy::Never, true));
    }

    // ── make_result ────────────────────────────────────────────

    #[test]
    fn make_result_success() {
        let activity = test_action("kill-pod", false);
        let result = make_result(ResultParams {
            activity: &activity,
            started_at_ns: 1_774_980_135_000_000_000,
            duration_ms: 342,
            success: true,
            output: Some("done".into()),
            error: None,
            trace_id: "trace-1".into(),
            span_id: "span-1".into(),
        });
        assert_eq!(result.status, ActivityStatus::Succeeded);
        assert_eq!(result.name, "kill-pod");
        assert_eq!(result.duration_ms, 342);
    }

    #[test]
    fn make_result_failure() {
        let activity = test_action("kill-pod", false);
        let result = make_result(ResultParams {
            activity: &activity,
            started_at_ns: 1_774_980_135_000_000_000,
            duration_ms: 500,
            success: false,
            output: None,
            error: Some("connection refused".into()),
            trace_id: "trace-1".into(),
            span_id: "span-1".into(),
        });
        assert_eq!(result.status, ActivityStatus::Failed);
        assert_eq!(result.error.unwrap(), "connection refused");
    }

    // ── all_succeeded ──────────────────────────────────────────

    #[test]
    fn all_succeeded_empty_is_false() {
        assert!(!all_succeeded(&[]));
    }

    #[test]
    fn all_succeeded_with_success() {
        let results = vec![ActivityResult {
            name: "a".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns: 0,
            duration_ms: 0,
            output: None,
            error: None,
            trace_id: "".into(),
            span_id: "".into(),
        }];
        assert!(all_succeeded(&results));
    }

    #[test]
    fn all_succeeded_with_failure() {
        let results = vec![
            ActivityResult {
                name: "a".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 0,
                duration_ms: 0,
                output: None,
                error: None,
                trace_id: "".into(),
                span_id: "".into(),
            },
            ActivityResult {
                name: "b".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Failed,
                started_at_ns: 0,
                duration_ms: 0,
                output: None,
                error: None,
                trace_id: "".into(),
                span_id: "".into(),
            },
        ];
        assert!(!all_succeeded(&results));
    }

    // ── count_by_type ──────────────────────────────────────────

    #[test]
    fn count_actions_and_probes() {
        let activities = vec![
            test_action("a1", false),
            test_probe("p1"),
            test_action("a2", false),
            test_probe("p2"),
            test_probe("p3"),
        ];
        assert_eq!(count_by_type(&activities, &ActivityType::Action), 2);
        assert_eq!(count_by_type(&activities, &ActivityType::Probe), 3);
    }

    #[test]
    fn count_empty_method() {
        assert_eq!(count_by_type(&[], &ActivityType::Action), 0);
    }

    // ── partition_background ───────────────────────────────────

    #[test]
    fn partition_separates_background_activities() {
        let activities = vec![
            test_action("seq-1", false),
            test_action("bg-1", true),
            test_action("seq-2", false),
            test_action("bg-2", true),
        ];
        let (seq, bg) = partition_background(&activities);
        assert_eq!(seq.len(), 2);
        assert_eq!(bg.len(), 2);
        assert_eq!(seq[0].name, "seq-1");
        assert_eq!(bg[0].name, "bg-1");
    }

    #[test]
    fn partition_all_sequential() {
        let activities = vec![test_action("a", false), test_action("b", false)];
        let (seq, bg) = partition_background(&activities);
        assert_eq!(seq.len(), 2);
        assert!(bg.is_empty());
    }

    #[test]
    fn partition_all_background() {
        let activities = vec![test_action("a", true), test_action("b", true)];
        let (seq, bg) = partition_background(&activities);
        assert!(seq.is_empty());
        assert_eq!(bg.len(), 2);
    }
}
