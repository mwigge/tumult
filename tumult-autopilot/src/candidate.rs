//! Gate inputs: the candidate under decision, the ambient environment
//! snapshot, and the per-class autonomy record.
//!
//! Every field is caller-computed. The gate reads no clocks and holds no
//! handles, so whatever "now" means — business hours, cooldown age, runs
//! today, concurrent experiments — is resolved by the engine *before*
//! evaluation and frozen into these structs. That freeze is what makes a
//! decision replayable: the corpus serialises exactly these types as JSON,
//! and any verdict is reproducible from them plus the policy text.

use serde::{Deserialize, Serialize};

/// A recommendation joined with its playbook and experiment introspection —
/// the unit the validator and gate decide on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Caller-supplied decision id (a UUID in the engine).
    pub id: String,
    /// The service the fault would target.
    pub service_id: String,
    /// The target's declared tier, when the topology knows one.
    pub tier: Option<String>,
    /// Plugin owning the recommended action.
    pub plugin: String,
    /// The recommended fault action.
    pub action: String,
    /// The compliance article the injection would inform.
    pub article_id: String,
    /// Recommender score (transparent factor product).
    pub score: f64,
    /// One human-readable reason per recommender scoring factor.
    pub reasons: Vec<String>,
    /// Confidence tier; the caller maps it from the score threshold.
    pub confidence: ConfidenceTier,
    /// Resolved experiment path from the playbook; `None` = no playbook, so
    /// the experiment introspection flags below are meaningless.
    pub playbook_experiment: Option<String>,
    /// Whether the resolved experiment declares a guard.
    pub experiment_has_guard: bool,
    /// Whether the resolved experiment declares a rollback.
    pub experiment_has_rollback: bool,
    /// Whether the resolved experiment declares a steady-state probe.
    pub experiment_has_steady_state: bool,
    /// Number of faults the resolved experiment injects.
    pub experiment_fault_count: usize,
    /// What woke the autopilot up for this candidate.
    pub trigger: Trigger,
}

/// Why the recommender surfaced a candidate now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    /// Evidence for an article aged past its TTL.
    Staleness {
        /// The article whose evidence went stale.
        article_id: String,
        /// Evidence age in days at trigger time.
        age_days: u32,
    },
    /// A control flipped to broken.
    BrokenControl {
        /// The article whose control broke.
        article_id: String,
    },
    /// An operator asked for revalidation.
    Manual,
    /// A deploy/config change invalidated the target's evidence.
    ChangeEvent {
        /// What reported the change (a deploy webhook, a config watcher, …).
        source: String,
        /// Optional human-readable detail about what changed.
        detail: Option<String>,
    },
}

/// Recommendation confidence, gate-relevant only in two buckets: `High` is
/// eligible for enact, `Directional` is propose-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceTier {
    /// Strong enough evidence to act on autonomously.
    High,
    /// Worth a human's attention, not worth autonomous action.
    Directional,
}

/// Caller-computed environment snapshot at decision time. Frozen before the
/// gate runs — the gate itself never observes the world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmbientContext {
    /// The target or one of its dependents has a recent open deviation —
    /// never inject into an already-degraded system.
    pub open_deviation_for_target: bool,
    /// Autopilot runs already performed today (any service).
    pub runs_today: u32,
    /// Hours since the last autopilot run on this service; `None` = never.
    pub hours_since_last_run_on_service: Option<f64>,
    /// Whether the caller considers "now" to be within business hours.
    pub within_business_hours: bool,
    /// Experiments currently running per the global impact ledger; the
    /// autopilot holds one fault at a time in v2.15.
    pub concurrent_experiments: u32,
    /// Guard-telemetry pre-flight ("can I see what I'm about to break?"):
    /// `None` = not checked, `Some(false)` = checked and blind.
    pub guard_telemetry_ok: Option<bool>,
}

/// Per-fault-class autonomy history; the caller aggregates it from the
/// decision store. `enacted_clean` counts enacted runs that completed
/// without veto, override, or a failed post-run steady-state re-check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutonomyRecord {
    /// Total enacted runs for the class.
    pub enacted_total: u32,
    /// Enacted runs that completed clean.
    pub enacted_clean: u32,
}

#[cfg(test)]
mod tests {
    use super::{ConfidenceTier, Trigger};

    // The corpus files depend on these serde shapes; a change here is a
    // corpus-format break and must be deliberate.
    #[test]
    fn trigger_serialises_with_a_type_tag() {
        let staleness = Trigger::Staleness {
            article_id: "compliance:DORA/Art.25".to_string(),
            age_days: 120,
        };
        let json = serde_json::to_string(&staleness).unwrap();
        assert_eq!(
            json,
            r#"{"type":"staleness","article_id":"compliance:DORA/Art.25","age_days":120}"#
        );
        let manual: Trigger = serde_json::from_str(r#"{"type":"manual"}"#).unwrap();
        assert_eq!(manual, Trigger::Manual);
    }

    #[test]
    fn change_event_trigger_round_trips() {
        let event = Trigger::ChangeEvent {
            source: "deploy-webhook".to_string(),
            detail: Some("image tag changed".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"change_event","source":"deploy-webhook","detail":"image tag changed"}"#
        );
        assert_eq!(serde_json::from_str::<Trigger>(&json).unwrap(), event);

        let bare = Trigger::ChangeEvent {
            source: "config-watcher".to_string(),
            detail: None,
        };
        let json = serde_json::to_string(&bare).unwrap();
        assert_eq!(
            json,
            r#"{"type":"change_event","source":"config-watcher","detail":null}"#
        );
        assert_eq!(serde_json::from_str::<Trigger>(&json).unwrap(), bare);
    }

    #[test]
    fn confidence_serialises_lowercase() {
        assert_eq!(
            serde_json::to_string(&ConfidenceTier::High).unwrap(),
            r#""high""#
        );
        let tier: ConfidenceTier = serde_json::from_str(r#""directional""#).unwrap();
        assert_eq!(tier, ConfidenceTier::Directional);
    }
}
