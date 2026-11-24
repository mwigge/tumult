//! Regulatory compliance domain logic shared by the CLI and MCP server.
//!
//! Owns the framework catalog (names and canonical report identifiers), the
//! journal-derived compliance signals (pass rate and recovery compliance),
//! and the COMPLIANT / PARTIAL / NON-COMPLIANT verdict thresholds — one
//! source of truth so `tumult compliance` and the `tumult_compliance` MCP
//! tool cannot drift apart.

mod citations;
mod dates;
mod dora_refs;
mod evidence;
mod framework;
mod ids;
mod refs;
mod requirements;
mod scenarios;
mod signals;

pub use citations::{
    Citation, CITATIONS, CITATION_MAX_AGE_MONTHS, EVIDENCE_DISCLAIMER, REGISTRY_VERSION,
};
pub use dates::{current_year_month, months_since_verified, parse_year_month, stale_citations};
pub use dora_refs::{dora_paragraph_count, dora_ref_exists, parse_dora_ref, DoraRef};
pub use evidence::{EvidenceStrength, EvidenceType};
pub use framework::ComplianceFramework;
pub use ids::{
    check_entity_id, entity_id_is_valid, lei_is_valid, truncated, EntityIdError, ENTITY_ID_ATTR,
    ENTITY_ID_MALFORMED_ATTR, ENTITY_ID_MAX_LEN, ENTITY_ID_PATTERN, ENTITY_ID_TRUNCATION_MARK,
};
pub use refs::{
    check_reference, check_run_requirement, citation_of, requirement_id, run_requirement_id,
    UnknownRequirement,
};
pub use requirements::{
    applicable, article_title, requirement, EntityFlags, EntityProfile, EvidenceClass, Feed,
    FlagRule, FunctionScope, MicroRule, Profile, ProfileSet, Requirement, ScopeBasis, Selector,
    ARTICLES, CORE_WEIGHT_MIN, EXPECTED_STATUS_DESIGNED, EXPECTED_STATUS_EXISTING,
    EXPECTED_STATUS_OPERATING, FLAG_NAMES, GATING_WEIGHT, REQUIREMENTS, REQUIREMENTS_VERSION,
};
pub use scenarios::{
    check_scenario, required_by, scenario, Scenario, UnknownScenario, RTS_1774_ELI, SCENARIOS,
};
pub use signals::{compliance_verdict, ComplianceSignals, RecoveryMeasure, DEFAULT_MTTR_TARGET_S};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Journal;

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

    /// One action of a method that ended with `status`.
    fn action(status: crate::types::ActivityStatus) -> crate::types::ActivityResult {
        crate::types::ActivityResult {
            name: "fault".into(),
            activity_type: crate::types::ActivityType::Action,
            status,
            started_at_ns: 0,
            duration_ms: 0,
            output: None,
            error: None,
            trace_id: "".into(),
            span_id: "".into(),
        }
    }

    /// A completed run whose fault went in and that watched for recovery
    /// to the end: it saw it after `mttr_s` seconds, or did not see it.
    fn watched(mttr_s: Option<f64>) -> Journal {
        let mut journal = bare_journal(crate::types::ExperimentStatus::Completed);
        journal.method_results = vec![action(crate::types::ActivityStatus::Succeeded)];
        journal.post_result = Some(crate::types::PostResult {
            started_at_ns: 0,
            ended_at_ns: 0,
            duration_s: 0.0,
            samples: 0,
            probes: vec![],
            recovery_time_s: mttr_s.unwrap_or(0.0),
            full_recovery: mttr_s.is_some(),
            mttr_s,
            impact_observed: None,
            outage_started_at_ns: None,
            recovered_at_ns: None,
        });
        journal
    }

    fn signals_of(journals: &[Journal]) -> ComplianceSignals {
        let mut signals = ComplianceSignals::default();
        for journal in journals {
            signals.accumulate(journal);
        }
        signals
    }

    /// Two recoveries within 30 s, one of them at 30 s exactly, a slower
    /// one, a run that saw none, and a run that did not watch.
    fn mixed_runs() -> ComplianceSignals {
        signals_of(&[
            watched(Some(10.0)),
            watched(Some(30.0)),
            watched(Some(30.5)),
            watched(None),
            bare_journal(crate::types::ExperimentStatus::Completed),
        ])
    }

    #[test]
    fn recovery_is_the_share_of_the_watched_runs_that_came_back_within_the_target() {
        let signals = mixed_runs();

        assert!((signals.pass_rate() - 1.0).abs() < f64::EPSILON);
        assert_eq!(signals.recovered_within(DEFAULT_MTTR_TARGET_S), 2);
        assert_eq!(
            signals.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(0.5)
        );
        assert_eq!(signals.recovery_compliance(30.5), Some(0.75));
        assert_eq!(signals.recovery_compliance(1.0), Some(0.0));
    }

    /// A run whose probes saw no outage watched nothing recover: it is no
    /// sign of recovery, for it or against it.
    #[test]
    fn a_run_that_saw_no_outage_is_not_a_run_that_watched_for_recovery() {
        let seen = |mttr_s: Option<f64>, impact_observed: bool| {
            let mut journal = watched(mttr_s);
            let post = journal.post_result.as_mut().expect("a post phase");
            post.full_recovery = true;
            post.impact_observed = Some(impact_observed);
            journal
        };

        let signals = signals_of(&[seen(None, false), seen(Some(10.0), true)]);

        assert_eq!(signals.recovered_within(DEFAULT_MTTR_TARGET_S), 1);
        assert_eq!(
            signals.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(1.0)
        );
        let nothing_seen = signals_of(&[seen(None, false)]);
        assert_eq!(
            nothing_seen.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            None
        );
    }

    /// A run that declares a recovery time objective of `rto_target_s` and
    /// came back after `mttr_s` seconds.
    fn with_rto(rto_target_s: f64, mttr_s: f64) -> Journal {
        let mut journal = watched(Some(mttr_s));
        journal.objectives = Some(crate::types::ObjectiveOutcome {
            rto_target_s: Some(rto_target_s),
            rpo_target_s: None,
            rto_measured_s: Some(mttr_s),
            rpo_measured_s: None,
            verdicts: vec![],
        });
        journal
    }

    #[test]
    fn a_run_is_judged_against_its_own_rto_and_a_run_without_one_against_the_default() {
        // 40 s is within an RTO of 60 s, though above the default of 30 s.
        let slow_failover = signals_of(&[with_rto(60.0, 40.0)]);
        assert_eq!(slow_failover.recovered_within(DEFAULT_MTTR_TARGET_S), 1);
        assert_eq!(
            slow_failover.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(1.0)
        );
        // 4 s is above an RTO of 2 s, though within the default.
        let tight = signals_of(&[with_rto(2.0, 4.0)]);
        assert_eq!(tight.recovery_compliance(DEFAULT_MTTR_TARGET_S), Some(0.0));
        // A run with no RTO of its own is judged against the default.
        let mixed = signals_of(&[
            with_rto(60.0, 40.0),
            watched(Some(30.0)),
            watched(Some(31.0)),
        ]);
        assert_eq!(mixed.recovered_within(DEFAULT_MTTR_TARGET_S), 2);
        assert_eq!(mixed.recovered_within(31.0), 3);
    }

    /// A run whose probes saw an outage that had not ended when the watch
    /// ended, but which measured no recovery from its fault: the method
    /// failed, or a stop cut the watch short.
    fn measured_none(status: crate::types::ExperimentStatus) -> Journal {
        let mut journal = watched(None);
        if status == crate::types::ExperimentStatus::Failed {
            journal.method_results = vec![action(crate::types::ActivityStatus::Failed)];
        }
        let post = journal.post_result.as_mut().expect("a post phase");
        post.impact_observed = Some(true);
        journal.status = status;
        journal
    }

    #[test]
    fn a_run_that_watched_and_measured_no_recovery_is_left_out_of_the_share() {
        use crate::types::ExperimentStatus;
        let method_failed = measured_none(ExperimentStatus::Failed);
        let cut_short = measured_none(ExperimentStatus::Interrupted);

        let signals = signals_of(&[
            method_failed.clone(),
            cut_short.clone(),
            watched(Some(10.0)),
        ]);

        assert_eq!(
            signals.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(1.0)
        );
        let none_measured = signals_of(&[method_failed, cut_short]);
        assert_eq!(
            none_measured.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            None
        );
        // A run that watched to the end and did not see its target come
        // back measured a recovery that failed.
        let not_back = signals_of(&[watched(None), watched(Some(10.0))]);
        assert_eq!(
            not_back.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(0.5)
        );
    }

    #[test]
    fn a_run_that_measured_no_recovery_fails_a_recovery_that_is_required() {
        use crate::types::ExperimentStatus;
        let no_outage = {
            let mut journal = watched(None);
            let post = journal.post_result.as_mut().expect("a post phase");
            post.full_recovery = true;
            post.impact_observed = Some(false);
            journal
        };
        let signals = signals_of(&[
            watched(Some(10.0)),
            measured_none(ExperimentStatus::Failed),
            measured_none(ExperimentStatus::Interrupted),
            no_outage,
            bare_journal(ExperimentStatus::Completed),
        ]);

        // Left out, the three runs that measured none leave one run, and it
        // came back within the target.
        assert_eq!(
            signals.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(1.0)
        );
        // Required, they are recoveries that failed. The run that did not
        // watch is in neither share.
        assert_eq!(
            signals.required_recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(0.25)
        );
        let unwatched = signals_of(&[bare_journal(ExperimentStatus::Completed)]);
        assert_eq!(
            unwatched.required_recovery_compliance(DEFAULT_MTTR_TARGET_S),
            None
        );
    }

    #[test]
    fn a_run_that_completed_is_no_sign_of_recovery() {
        let mut journal = bare_journal(crate::types::ExperimentStatus::Completed);
        journal.analysis = Some(crate::types::AnalysisResult {
            estimate_accuracy: None,
            estimate_recovery_delta_s: None,
            estimate_data_loss_correct: None,
            trend: None,
            resilience_score: Some(1.0),
        });

        let signals = signals_of(&[journal]);

        assert!((signals.pass_rate() - 1.0).abs() < f64::EPSILON);
        assert_eq!(signals.recovery_compliance(DEFAULT_MTTR_TARGET_S), None);
    }

    #[test]
    fn the_recovery_line_says_the_share_and_the_runs_it_is_of() {
        let signals = mixed_runs();

        assert_eq!(
            signals.recovery_line(DEFAULT_MTTR_TARGET_S),
            "Recovery within target: 50.0% of the runs that measured a recovery \
             (2 of 4; 0 against their own RTO, 4 against 30s)"
        );
        assert_eq!(
            signals.recovery_line(30.5),
            "Recovery within target: 75.0% of the runs that measured a recovery \
             (3 of 4; 0 against their own RTO, 4 against 30.5s)"
        );
        let own = signals_of(&[
            with_rto(60.0, 40.0),
            watched(Some(10.0)),
            measured_none(crate::types::ExperimentStatus::Failed),
        ]);
        assert_eq!(
            own.recovery_line(DEFAULT_MTTR_TARGET_S),
            "Recovery within target: 100.0% of the runs that measured a recovery \
             (2 of 2; 1 against their own RTO, 1 against 30s). \
             Left out: 1 run(s) that measured no recovery."
        );
    }

    #[test]
    fn the_recovery_line_says_when_no_run_measured_a_recovery() {
        let signals = signals_of(&[bare_journal(crate::types::ExperimentStatus::Completed)]);

        assert_eq!(
            signals.recovery_line(DEFAULT_MTTR_TARGET_S),
            "Recovery within target: not measured, no run measured a recovery. \
             The verdict rests on the pass rate only (reduced assurance)."
        );
    }

    #[test]
    fn empty_signals_have_zero_pass_rate() {
        let signals = ComplianceSignals::default();
        assert!(signals.pass_rate().abs() < f64::EPSILON);
    }

    fn bare_journal(status: crate::types::ExperimentStatus) -> Journal {
        Journal {
            target_version: None,
            target: None,
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
            kind: crate::types::TestKind::default(),
            objectives: None,
            probe_samples: vec![],
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        }
    }

    #[test]
    // clippy sees a const array's length statically and calls the emptiness
    // guard tautological — it is not: it trips the moment someone empties
    // the registry.
    #[allow(clippy::const_is_empty)]
    fn registry_is_well_formed() {
        assert!(!CITATIONS.is_empty(), "registry must not be empty");
        for c in CITATIONS {
            assert!(!c.control_id.is_empty(), "control_id empty");
            assert!(!c.title.is_empty(), "title empty for {}", c.control_id);
            assert!(!c.summary.is_empty(), "summary empty for {}", c.control_id);
            assert!(
                !c.evidence_note.is_empty(),
                "evidence_note empty for {}",
                c.control_id
            );
            assert!(
                c.source_url.starts_with("https://"),
                "source_url must be https for {}: {}",
                c.control_id,
                c.source_url
            );
            assert!(
                parse_year_month(c.last_verified).is_some(),
                "last_verified must be YYYY-MM-DD for {}: {}",
                c.control_id,
                c.last_verified
            );
        }
    }

    fn dora_citation(control_id: &str) -> &'static Citation {
        CITATIONS
            .iter()
            .find(|c| c.framework == ComplianceFramework::Dora && c.control_id == control_id)
            .unwrap_or_else(|| panic!("no DORA row for {control_id}"))
    }

    #[test]
    fn dora_art25_states_no_test_frequency() {
        let art25 = dora_citation("Art. 25");
        for text in [art25.summary, art25.evidence_note] {
            assert!(
                !text.to_ascii_lowercase().contains("yearly"),
                "Art. 25 sets no frequency: {text}"
            );
        }
    }

    #[test]
    fn dora_yearly_testing_is_cited_as_art24_paragraph_6() {
        let summary = dora_citation("Art. 24(6)").summary;
        assert!(summary.contains("at least yearly"), "{summary}");
        assert!(
            summary.contains("critical or important functions"),
            "{summary}"
        );
        assert!(summary.contains("microenterprises"), "{summary}");
    }

    #[test]
    fn dora_backup_and_restoration_is_cited_under_art12_not_art11() {
        let art12 = dora_citation("Art. 12").summary.to_ascii_lowercase();
        assert!(art12.contains("backup"), "{art12}");
        assert!(art12.contains("restoration"), "{art12}");
        let art11 = dora_citation("Art. 11");
        for text in [art11.summary, art11.evidence_note] {
            let lowered = text.to_ascii_lowercase();
            assert!(!lowered.contains("backup"), "{text}");
            assert!(!lowered.contains("integrity"), "{text}");
        }
    }

    #[test]
    fn dora_art26_applies_to_entities_identified_by_the_authority() {
        let summary = dora_citation("Art. 26").summary;
        assert!(
            summary.contains("identified by the competent authority"),
            "{summary}"
        );
        assert!(
            !summary.to_ascii_lowercase().contains("systemically"),
            "{summary}"
        );
    }

    #[test]
    fn registry_version_and_dora_review_date_follow_the_citation_fixes() {
        assert_eq!(REGISTRY_VERSION, "2026.11");
        for c in ComplianceFramework::Dora.citations() {
            assert_eq!(c.last_verified, "2026-11-13", "{}", c.control_id);
        }
    }

    #[test]
    fn every_framework_has_at_least_one_citation() {
        for framework in ComplianceFramework::ALL {
            assert!(
                !framework.citations().is_empty(),
                "{} has no citations",
                framework.as_report_str()
            );
        }
    }

    #[test]
    fn no_citation_is_stale() {
        // Fails once any citation exceeds CITATION_MAX_AGE_MONTHS as of today,
        // forcing a re-verification pass against the official sources. This is
        // the CI staleness gate.
        let now = current_year_month();
        let stale = stale_citations(now, CITATION_MAX_AGE_MONTHS);
        assert!(
            stale.is_empty(),
            "citations overdue for re-verification (>{CITATION_MAX_AGE_MONTHS} months): {}",
            stale
                .iter()
                .map(|c| format!(
                    "{} {} ({})",
                    c.framework.as_report_str(),
                    c.control_id,
                    c.last_verified
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    #[test]
    fn no_citation_is_dated_in_the_future() {
        let now = current_year_month();
        for c in CITATIONS {
            let age = months_since_verified(c.last_verified, now).expect("registry dates parse");
            assert!(
                age >= 0,
                "{} {} last_verified {} is in the future",
                c.framework.as_report_str(),
                c.control_id,
                c.last_verified
            );
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
        assert_eq!(signals.recovery_compliance(DEFAULT_MTTR_TARGET_S), None);
    }
}
