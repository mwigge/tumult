//! Regulatory compliance domain logic shared by the CLI and MCP server.
//!
//! Owns the framework catalog (names and canonical report identifiers), the
//! journal-derived compliance signals (pass rate and recovery compliance),
//! and the COMPLIANT / PARTIAL / NON-COMPLIANT verdict thresholds — one
//! source of truth so `tumult compliance` and the `tumult_compliance` MCP
//! tool cannot drift apart.

mod citations;
mod dates;
mod evidence;
mod framework;
mod signals;

pub use citations::{
    Citation, CITATIONS, CITATION_MAX_AGE_MONTHS, EVIDENCE_DISCLAIMER, REGISTRY_VERSION,
};
pub use dates::{current_year_month, months_since_verified, parse_year_month, stale_citations};
pub use evidence::{EvidenceStrength, EvidenceType};
pub use framework::ComplianceFramework;
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

    #[test]
    fn signals_prefer_mttr_over_resilience_scores() {
        let signals = ComplianceSignals {
            total_journals: 4,
            completed_journals: 3,
            journals_with_regulatory: 0,
            mttrs: vec![10.0, 40.0],
            resilience_scores: vec![0.1],
        };
        assert!((signals.pass_rate() - 0.75).abs() < f64::EPSILON);
        // 1 of 2 MTTRs under target; resilience scores are ignored.
        assert_eq!(
            signals.recovery_compliance(DEFAULT_MTTR_TARGET_S),
            Some(0.5)
        );
    }

    #[test]
    fn signals_fall_back_to_resilience_scores_then_none() {
        let mut signals = ComplianceSignals {
            total_journals: 2,
            completed_journals: 2,
            journals_with_regulatory: 1,
            mttrs: vec![],
            resilience_scores: vec![0.8, 0.6],
        };
        let rc = signals
            .recovery_compliance(DEFAULT_MTTR_TARGET_S)
            .expect("resilience fallback");
        assert!((rc - 0.7).abs() < 1e-9);

        signals.resilience_scores.clear();
        assert_eq!(signals.recovery_compliance(DEFAULT_MTTR_TARGET_S), None);
    }

    #[test]
    fn empty_signals_have_zero_pass_rate() {
        let signals = ComplianceSignals::default();
        assert!(signals.pass_rate().abs() < f64::EPSILON);
    }

    fn bare_journal(status: crate::types::ExperimentStatus) -> Journal {
        Journal {
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
        assert!(signals.resilience_scores.is_empty());
    }
}
