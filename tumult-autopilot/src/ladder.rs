//! Earned autonomy per fault class: `enact` is graduated into, never
//! configured from hope.
//!
//! Each `(plugin, action, tier)` class starts at propose. It reaches enact
//! only when its precision — enacted runs that completed without veto,
//! override, or a failed post-run steady-state re-check — crosses the
//! policy threshold over enough samples, or when the operator pretrusted
//! the class explicitly (the bootstrap escape hatch, since a fresh install
//! could otherwise never enact anything).
//!
//! A human veto or override resets the ladder to zero: the caller detects
//! the event and stores [`reset`]'s output. Everything here is pure — the
//! caller aggregates records from the decision store and persists the
//! transitions back.

use crate::candidate::AutonomyRecord;
use crate::policy::AutopilotPolicy;

/// Canonical key for a fault class, used to aggregate autonomy records and
/// to name the class in audit reasons. `None` tiers render as `-` so the
/// key is total and unambiguous (a tierless class is its own class).
#[must_use]
pub fn class_key(plugin: &str, action: &str, tier: Option<&str>) -> String {
    let tier = tier.unwrap_or("-");
    format!("{plugin}::{action}@{tier}")
}

/// Decide whether a fault class has earned autonomy, with the reason either
/// way — the reason string feeds straight into the gate's audit record.
///
/// Pretrust wins first (operator said so). Otherwise the record must exist,
/// hold at least `autonomy_min_samples` enacted runs, and show precision at
/// or above `autonomy_threshold`.
#[must_use]
pub fn autonomy_earned(
    policy: &AutopilotPolicy,
    record: Option<&AutonomyRecord>,
    pretrusted: bool,
) -> (bool, String) {
    if pretrusted {
        return (true, "pretrusted by operator policy".to_string());
    }
    let Some(record) = record else {
        return (
            false,
            "no autonomy record — class starts at propose".to_string(),
        );
    };
    if record.enacted_total == 0 || record.enacted_total < policy.autonomy_min_samples {
        return (
            false,
            format!(
                "only {} enacted sample(s), {} required",
                record.enacted_total, policy.autonomy_min_samples
            ),
        );
    }
    // Clamp clean to total so a corrupted record can never report precision
    // above 1.0 and buy autonomy it did not earn.
    let clean = record.enacted_clean.min(record.enacted_total);
    let precision = f64::from(clean) / f64::from(record.enacted_total);
    let threshold = policy.autonomy_threshold;
    let total = record.enacted_total;
    if precision >= threshold {
        (
            true,
            format!("precision {precision:.2} >= threshold {threshold:.2} over {total} run(s)"),
        )
    } else {
        (
            false,
            format!("precision {precision:.2} below threshold {threshold:.2} over {total} run(s)"),
        )
    }
}

/// Fold one enacted-run outcome into a class record. `clean = false` covers
/// veto, override, and a failed post-run steady-state re-check; on veto or
/// override the caller should store [`reset`] instead — a human saying "no"
/// outweighs any accumulated precision.
#[must_use]
pub fn record_outcome(record: &AutonomyRecord, clean: bool) -> AutonomyRecord {
    AutonomyRecord {
        enacted_total: record.enacted_total.saturating_add(1),
        enacted_clean: if clean {
            record.enacted_clean.saturating_add(1)
        } else {
            record.enacted_clean
        },
    }
}

/// Reset a class's ladder to zero (human veto or override). Takes the old
/// record so call sites read as a transition, not a construction.
#[must_use]
pub fn reset(_record: &AutonomyRecord) -> AutonomyRecord {
    AutonomyRecord::default()
}

#[cfg(test)]
mod tests {
    use super::{autonomy_earned, class_key, record_outcome, reset};
    use crate::candidate::AutonomyRecord;
    use crate::policy::AutopilotPolicy;

    fn policy() -> AutopilotPolicy {
        // Defaults: threshold 0.8, min samples 3.
        AutopilotPolicy::default()
    }

    #[test]
    fn class_key_is_total_over_missing_tiers() {
        assert_eq!(
            class_key("tumult-net", "inject_latency", Some("service")),
            "tumult-net::inject_latency@service"
        );
        assert_eq!(
            class_key("tumult-net", "inject_latency", None),
            "tumult-net::inject_latency@-"
        );
    }

    #[test]
    fn pretrust_wins_even_without_a_record() {
        let (earned, reason) = autonomy_earned(&policy(), None, true);
        assert!(earned);
        assert!(reason.contains("pretrusted"));
    }

    #[test]
    fn no_record_means_propose() {
        let (earned, reason) = autonomy_earned(&policy(), None, false);
        assert!(!earned);
        assert!(reason.contains("no autonomy record"));
    }

    #[test]
    fn too_few_samples_blocks_regardless_of_precision() {
        let record = AutonomyRecord {
            enacted_total: 2,
            enacted_clean: 2,
        };
        let (earned, reason) = autonomy_earned(&policy(), Some(&record), false);
        assert!(!earned);
        assert!(reason.contains("2 enacted sample(s), 3 required"));
    }

    #[test]
    fn precision_below_threshold_blocks() {
        let record = AutonomyRecord {
            enacted_total: 4,
            enacted_clean: 3, // 0.75 < 0.8
        };
        let (earned, reason) = autonomy_earned(&policy(), Some(&record), false);
        assert!(!earned);
        assert!(reason.contains("0.75"));
    }

    #[test]
    fn precision_at_threshold_earns() {
        let record = AutonomyRecord {
            enacted_total: 5,
            enacted_clean: 4, // exactly 0.8
        };
        let (earned, reason) = autonomy_earned(&policy(), Some(&record), false);
        assert!(earned);
        assert!(reason.contains("0.80"));
    }

    #[test]
    fn corrupted_record_precision_is_clamped() {
        let record = AutonomyRecord {
            enacted_total: 3,
            enacted_clean: 9,
        };
        let (earned, reason) = autonomy_earned(&policy(), Some(&record), false);
        assert!(earned); // clamped to 1.00, not >1
        assert!(reason.contains("1.00"));
    }

    #[test]
    fn outcomes_accumulate_and_reset_zeroes() {
        let start = AutonomyRecord::default();
        let after_clean = record_outcome(&start, true);
        let after_dirty = record_outcome(&after_clean, false);
        assert_eq!(after_dirty.enacted_total, 2);
        assert_eq!(after_dirty.enacted_clean, 1);
        assert_eq!(reset(&after_dirty), AutonomyRecord::default());
    }
}
