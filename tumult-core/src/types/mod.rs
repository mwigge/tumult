//! Core data model types for Tumult experiments.
//!
//! All types derive `serde::Serialize` and `serde::Deserialize`
//! for round-trip TOON serialization.

mod definition;
mod enums;
mod gameday;
mod ids;
mod journal;
mod results;
mod target;

pub use definition::*;
pub use enums::*;
pub use gameday::*;
pub use ids::*;
pub use journal::*;
pub use results::*;
pub use target::*;

#[cfg(test)]
pub(crate) mod test_support {
    /// Round-trip a value through the TOON encoder/decoder.
    pub(crate) fn toon_round_trip<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
    {
        let encoded = toon_format::encode_default(value).expect("TOON encode failed");
        toon_format::decode_default(&encoded).expect("TOON decode failed")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::toon_round_trip;
    use super::*;
    use indexmap::IndexMap;
    use std::collections::HashMap;

    // ── Full experiment construction + TOON round-trip ────────

    fn build_sample_experiment() -> Experiment {
        Experiment {
            version: "v1".into(),
            title: "Database failover validates automatic reconnection".into(),
            description: Some("Kill PostgreSQL primary and verify app reconnects".into()),
            tags: vec!["database".into(), "resilience".into()],
            configuration: IndexMap::from([(
                "db_host".into(),
                ConfigValue::Env {
                    key: "DATABASE_HOST".into(),
                },
            )]),
            secrets: IndexMap::new(),
            controls: vec![],
            steady_state_hypothesis: Some(Hypothesis {
                title: "Application responds healthy".into(),
                probes: vec![Activity {
                    name: "health-check".into(),
                    activity_type: ActivityType::Probe,
                    provider: Provider::Process {
                        path: "scripts/health-check.sh".into(),
                        arguments: vec![],
                        env: HashMap::new(),
                        timeout_s: Some(5.0),
                    },
                    tolerance: Some(Tolerance::Exact {
                        value: serde_json::Value::Number(200.into()),
                    }),
                    pause_before_s: None,
                    pause_after_s: None,
                    background: false,
                    label_selector: None,
                }],
            }),
            method: vec![Activity {
                name: "kill-db-connections".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Native {
                    plugin: "tumult-db".into(),
                    function: "terminate_connections".into(),
                    arguments: HashMap::from([(
                        "database".into(),
                        serde_json::Value::String("myapp".into()),
                    )]),
                },
                tolerance: None,
                pause_before_s: None,
                pause_after_s: Some(5.0),
                background: false,
                label_selector: None,
            }],
            rollbacks: vec![Activity {
                name: "restore-connections".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Native {
                    plugin: "tumult-db".into(),
                    function: "reset_connection_pool".into(),
                    arguments: HashMap::new(),
                },
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            }],
            estimate: Some(Estimate {
                expected_outcome: ExpectedOutcome::Recovered,
                expected_recovery_s: Some(15.0),
                expected_degradation: Some(DegradationLevel::Moderate),
                expected_data_loss: Some(false),
                confidence: Some(Confidence::High),
                rationale: Some("Tested monthly with consistent recovery".into()),
                prior_runs: Some(5),
            }),
            baseline: Some(BaselineConfig {
                duration_s: 120.0,
                warmup_s: Some(15.0),
                interval_s: 2.0,
                method: BaselineMethod::MeanStddev,
                sigma: Some(2.0),
                confidence: Some(0.95),
            }),
            load: None,
            regulatory: Some(RegulatoryMapping {
                frameworks: vec!["DORA".into()],
                requirements: vec![RegulatoryRequirement {
                    id: "DORA-Art24".into(),
                    description: "ICT resilience testing".into(),
                    evidence: "Recovery within RTO".into(),
                }],
            }),
        }
    }

    #[test]
    fn full_experiment_round_trips_through_toon() {
        let exp = build_sample_experiment();
        let decoded: Experiment = toon_round_trip(&exp);
        assert_eq!(decoded, exp);
    }

    #[test]
    fn full_experiment_has_all_sections() {
        let exp = build_sample_experiment();
        assert_eq!(
            exp.title,
            "Database failover validates automatic reconnection"
        );
        assert_eq!(exp.tags, vec!["database", "resilience"]);
        assert!(exp.estimate.is_some());
        assert!(exp.baseline.is_some());
        assert!(exp.regulatory.is_some());
        assert!(exp.steady_state_hypothesis.is_some());
        assert_eq!(exp.method.len(), 1);
        assert_eq!(exp.rollbacks.len(), 1);
        assert_eq!(exp.method[0].activity_type, ActivityType::Action);
        assert_eq!(
            exp.steady_state_hypothesis.as_ref().unwrap().probes[0].activity_type,
            ActivityType::Probe
        );
    }
}
