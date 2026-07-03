//! Regulatory compliance reporting command handler.

use std::path::Path;

use anyhow::{bail, Result};

use super::ComplianceFramework;

// ── Compliance command ────────────────────────────────────────

/// # Errors
///
/// Returns an error if journals cannot be read or the analytics query fails.
#[allow(clippy::too_many_lines)] // Framework-specific output is intentionally verbose for audit clarity
#[must_use = "callers must handle compliance check errors"]
pub fn cmd_compliance(journals_path: &Path, framework: ComplianceFramework) -> Result<()> {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::journal::read_journal;
    use tumult_core::types::{ExperimentStatus, Journal};

    let store = AnalyticsStore::in_memory()?;
    let mut count = 0;
    let mut journals_with_regulatory = 0;

    // FIX 5: accumulate journal-level recovery/resilience signals directly from the
    // Journals. The analytics `experiments` table has no MTTR column, and a true
    // `ResilienceScore` (with recovery_compliance) is a GameDay-only aggregate — it is
    // NOT persisted on single-experiment Journals. So we derive an equivalent
    // recovery_compliance from PostResult.mttr_s, falling back to
    // AnalysisResult.resilience_score, then to pass-rate-only.
    let mut resilience_scores: Vec<f64> = Vec::new();
    let mut mttrs: Vec<f64> = Vec::new();
    let mut completed_journals = 0usize;
    let mut total_journals = 0usize;
    let mut accumulate = |journal: &Journal| {
        total_journals += 1;
        if matches!(journal.status, ExperimentStatus::Completed) {
            completed_journals += 1;
        }
        if let Some(s) = journal.analysis.as_ref().and_then(|a| a.resilience_score) {
            resilience_scores.push(s);
        }
        if let Some(m) = journal.post_result.as_ref().and_then(|p| p.mttr_s) {
            mttrs.push(m);
        }
    };

    if journals_path.is_dir() {
        for entry in std::fs::read_dir(journals_path)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&path) {
                    Ok(journal) => {
                        if journal.regulatory.is_some() {
                            journals_with_regulatory += 1;
                        }
                        accumulate(&journal);
                        store.ingest_journal(&journal)?;
                        count += 1;
                    }
                    Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
                }
            }
        }
    } else if journals_path.is_file() {
        let journal = read_journal(journals_path)?;
        if journal.regulatory.is_some() {
            journals_with_regulatory += 1;
        }
        accumulate(&journal);
        store.ingest_journal(&journal)?;
        count = 1;
    } else {
        bail!("path does not exist: {}", journals_path.display());
    }

    let fw = framework.as_report_str();
    let full_name = match framework {
        ComplianceFramework::Dora => "DORA — Digital Operational Resilience Act (EU 2022/2554)",
        ComplianceFramework::Nis2 => {
            "NIS2 — Network and Information Security Directive (EU 2022/2555)"
        }
        ComplianceFramework::PciDss => "PCI-DSS 4.0 — Payment Card Industry Data Security Standard",
        ComplianceFramework::Iso22301 => "ISO 22301 — Business Continuity Management Systems",
        ComplianceFramework::Iso27001 => "ISO 27001 — Information Security Management Systems",
        ComplianceFramework::Soc2 => "SOC 2 — Service Organization Control Type 2",
        ComplianceFramework::BaselIii => "Basel III — BCBS 239 Risk Data Aggregation",
    };
    println!("=== {full_name} ===\n");
    println!("Journals analyzed: {count}");
    println!("With regulatory tagging: {journals_with_regulatory}\n");

    // Overall status
    let rows = store.query(
        "SELECT status, count(*) as runs FROM experiments GROUP BY status ORDER BY runs DESC",
    )?;
    println!("Experiment Results:");
    for row in &rows {
        println!("  {}: {} run(s)", row[0], row[1]);
    }

    // FIX 5: compliance verdict derived from journal-level pass_rate + a recovery_compliance
    // proxy (MTTR-under-target, or avg resilience_score fallback, or pass-rate-only).
    #[allow(clippy::cast_precision_loss)]
    let pass_rate = if total_journals > 0 {
        completed_journals as f64 / total_journals as f64
    } else {
        0.0
    };
    // Kept so the framework-specific blocks below compile unchanged.
    let success_rate = pass_rate * 100.0;

    // Matches ScoringConfig::default_mttr_target (tumult-core types.rs).
    const MTTR_TARGET_S: f64 = 30.0;
    #[allow(clippy::cast_precision_loss)]
    let recovery_compliance: Option<f64> = if mttrs.is_empty() {
        if resilience_scores.is_empty() {
            None
        } else {
            Some(resilience_scores.iter().sum::<f64>() / resilience_scores.len() as f64)
        }
    } else {
        Some(mttrs.iter().filter(|m| **m <= MTTR_TARGET_S).count() as f64 / mttrs.len() as f64)
    };

    println!("\nCompliance Status:");
    println!("  Pass rate: {:.1}%", pass_rate * 100.0);
    match recovery_compliance {
        Some(rc) => println!(
            "  Recovery compliance: {:.1}% (MTTR<={MTTR_TARGET_S}s, or avg resilience proxy)",
            rc * 100.0
        ),
        None => {
            println!(
                "  Recovery compliance: N/A — no MTTR or resilience_score present in journals;"
            );
            println!("  verdict based on pass rate ONLY (reduced assurance).");
        }
    }
    println!("  Overall: {}", compliance_verdict(pass_rate, recovery_compliance));

    // Framework-specific requirements and evidence
    match fw {
        "DORA" => {
            println!("\nSource: https://eur-lex.europa.eu/eli/reg/2022/2554/oj");
            println!("Applies to EU financial entities. Mandates ICT resilience testing");
            println!("programmes with documented evidence and recovery time validation.\n");
            println!("Requirements:");
            println!("  Art. 24 — General requirements for ICT resilience testing");
            println!("    Testing programme: {count} experiment(s) executed");
            println!("  Art. 25 — Testing of ICT tools and systems");
            println!("    Scenario-based tests with documented results");
            println!("  Art. 26 — Advanced testing (TLPT)");
            println!("    Threat-led penetration testing (for systemically important entities)");
            println!("  Art. 11 — Response and recovery");
            println!("    Recovery procedures tested with measured recovery times");
        }
        "NIS2" => {
            println!("\nSource: https://eur-lex.europa.eu/eli/dir/2022/2555/oj");
            println!("Applies to EU essential/important entities across 18 sectors.");
            println!("Requires risk management measures including testing and audit.\n");
            println!("Requirements:");
            println!("  Art. 21(2)(c) — Business continuity and crisis management");
            println!("    Fault injection experiments with recovery measurement");
            println!("  Art. 21(2)(f) — Assessment of cybersecurity measures effectiveness");
            println!("    Baseline vs during-fault comparison proves control effectiveness");
            println!("  Art. 23 — Incident handling and reporting");
            println!("    Documented incident response procedures tested");
        }
        "PCI-DSS" => {
            println!("\nSource: https://www.pcisecuritystandards.org/document_library/");
            println!(
                "Applies to any entity storing, processing, or transmitting cardholder data.\n"
            );
            println!("Requirements:");
            println!("  Req. 11.4.1 — Penetration testing methodology defined");
            println!("    Experiment definitions with hypothesis, method, rollbacks");
            println!("  Req. 11.4.2 — Internal penetration testing at least annually");
            println!("    Journal timestamps prove execution: {count} run(s)");
            println!("  Req. 11.4.5 — Segmentation control testing");
            println!("    Network partition experiments with recovery verification");
            println!("  Req. 12.10.2 — Incident response plan tested annually");
            println!("    Experiments trigger and validate incident response procedures");
        }
        "ISO-22301" => {
            println!("\nSource: https://www.iso.org/standard/75106.html");
            println!("Business continuity management — requires exercising and testing.\n");
            println!("Requirements:");
            println!("  Clause 8.5 — Exercising and testing");
            println!("    Exercises consistent with BCMS scope: {count} experiment(s)");
            println!("    Based on appropriate scenarios with documented results");
            println!("    Formal post-exercise reports via `tumult report`");
            println!("    Results analysed via trend analysis and estimate accuracy");
        }
        "ISO-27001" => {
            println!("\nSource: https://www.iso.org/standard/27001");
            println!("Information security management — continuity controls.\n");
            println!("Requirements:");
            println!("  Annex A.17.1.3 — Verify and review IT service continuity controls");
            println!("    Experiment results prove controls function under fault conditions");
            println!("    Regular testing with journal frequency and trend data");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        "SOC2" => {
            println!("\nSource: https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2");
            println!("Service Organization Control — availability and processing integrity.\n");
            println!("Requirements:");
            println!("  CC7.5 — Recovery from identified disruptions");
            println!("    Recovery procedures tested with measured MTTR");
            println!("    Recovery meets defined objectives (RTO validation)");
            println!("  CC7.4 — Detection and monitoring");
            println!("    Observability data (OTel traces) proves monitoring coverage");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        "Basel-III" => {
            println!("\nSource: https://www.bis.org/publ/bcbs239.htm");
            println!("Risk data aggregation and reporting for global banking.\n");
            println!("Requirements:");
            println!("  Principle 6 — Adaptability");
            println!("    Systems function under stress conditions");
            println!("    Data aggregation and reporting during crisis validated");
            println!("    Recovery of reporting capability measured");
            println!("  Evidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
        _ => {
            println!("\nEvidence: {count} experiment(s), {success_rate:.1}% success rate");
        }
    }

    println!("\n=== End Report ===");
    Ok(())
}

/// FIX 5: recovery-aware compliance verdict.
///
/// The COMPLIANT / PARTIAL / NON-COMPLIANT verdict requires BOTH a pass rate and
/// a recovery signal. `recovery_compliance` is `None` when neither MTTR nor
/// `resilience_score` data is present in the journals, in which case the verdict
/// falls back to pass-rate-only thresholds (reduced assurance). Thresholds are
/// aligned with `ResilienceScore::status` (0.90 / 0.75).
#[must_use]
pub(crate) fn compliance_verdict(pass_rate: f64, recovery_compliance: Option<f64>) -> &'static str {
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
