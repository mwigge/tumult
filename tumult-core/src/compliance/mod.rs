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
mod requirements;
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
pub use requirements::{
    applicable, article_title, requirement, EntityFlags, EntityProfile, EvidenceClass, Feed,
    FlagRule, FunctionScope, MicroRule, Profile, ProfileSet, Requirement, ScopeBasis, Selector,
    ARTICLES, CORE_WEIGHT_MIN, EXPECTED_STATUS_DESIGNED, EXPECTED_STATUS_EXISTING,
    EXPECTED_STATUS_OPERATING, FLAG_NAMES, GATING_WEIGHT, REQUIREMENTS, REQUIREMENTS_VERSION,
};
pub use signals::{compliance_verdict, ComplianceSignals, DEFAULT_MTTR_TARGET_S};

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

    /// A completed run that watched for recovery and saw it after `mttr_s`
    /// seconds, or did not see it.
    fn watched(mttr_s: Option<f64>) -> Journal {
        let mut journal = bare_journal(crate::types::ExperimentStatus::Completed);
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
        assert_eq!(signals.recovery_watched, 4);
        assert_eq!(signals.mttrs, [10.0, 30.0, 30.5]);
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

        assert_eq!(signals.recovery_watched, 1);
        assert_eq!(signals.mttrs, [10.0]);
        assert_eq!(
            signals.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(1.0)
        );
        let nothing_seen = signals_of(&[seen(None, false)]);
        assert_eq!(nothing_seen.recovery_watched, 0);
        assert_eq!(
            nothing_seen.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            None
        );
    }

    #[test]
    fn a_run_that_completed_is_no_sign_of_recovery() {
        let mut journal = bare_journal(crate::types::ExperimentStatus::Completed);
        journal.analysis = Some(crate::types::AnalysisResult {
            estimate_accuracy: None,
            estimate_recovery_delta_s: None,
            trend: None,
            resilience_score: Some(1.0),
        });

        let signals = signals_of(&[journal]);

        assert!((signals.pass_rate() - 1.0).abs() < f64::EPSILON);
        assert_eq!(signals.recovery_watched, 0);
        assert_eq!(signals.recovery_compliance(DEFAULT_MTTR_TARGET_S), None);
    }

    #[test]
    fn the_recovery_line_says_the_share_and_the_runs_it_is_of() {
        let signals = mixed_runs();

        assert_eq!(
            signals.recovery_line(DEFAULT_MTTR_TARGET_S),
            "Recovery within 30s: 50.0% of the runs that watched for recovery (2 of 4)"
        );
        assert_eq!(
            signals.recovery_line(30.5),
            "Recovery within 30.5s: 75.0% of the runs that watched for recovery (3 of 4)"
        );
    }

    #[test]
    fn the_recovery_line_says_when_no_run_watched_for_recovery() {
        let signals = signals_of(&[bare_journal(crate::types::ExperimentStatus::Completed)]);

        assert_eq!(
            signals.recovery_line(DEFAULT_MTTR_TARGET_S),
            "Recovery within 30s: not measured, no run watched for recovery. \
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
        assert!(signals.mttrs.is_empty());
        assert_eq!(signals.recovery_watched, 0);
    }
}
