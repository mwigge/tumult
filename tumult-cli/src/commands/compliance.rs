//! Regulatory compliance reporting command handler.

use std::path::Path;

use anyhow::{bail, Result};

use super::ComplianceFramework;

// ── Compliance command ────────────────────────────────────────

/// # Errors
///
/// Returns an error if journals cannot be read or the analytics query fails.
#[must_use = "callers must handle compliance check errors"]
pub fn cmd_compliance(
    journals_path: Option<&Path>,
    framework: ComplianceFramework,
    sources: bool,
) -> Result<()> {
    use tumult_core::compliance::{ComplianceSignals, DEFAULT_MTTR_TARGET_S as MTTR_TARGET_S};
    use tumult_core::journal::read_journal;
    use tumult_lake::AnalyticsStore;

    let core_framework = framework.to_core();

    // `--sources`: print the dated, sourced citation registry and exit. No
    // journals required — this is the drift-audit surface.
    if sources {
        print_sources(core_framework);
        return Ok(());
    }

    let Some(journals_path) = journals_path else {
        bail!("a journals path is required (or pass --sources to list the citation registry)");
    };

    let store = AnalyticsStore::in_memory()?;
    let mut count = 0;

    // The signals are counted from the journals themselves, by
    // tumult_core::compliance::ComplianceSignals (the single source of truth
    // shared with the MCP server): the analytics `experiments` table has no
    // MTTR column. Recovery is counted from the runs that measured one, each
    // against its own RTO or the default target. A run that only completed
    // is in the pass rate and is no sign of recovery.
    let mut signals = ComplianceSignals::default();

    if journals_path.is_dir() {
        for entry in std::fs::read_dir(journals_path)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toon") {
                match read_journal(&path) {
                    Ok(journal) => {
                        signals.accumulate(&journal);
                        store.ingest_journal(&journal)?;
                        count += 1;
                    }
                    Err(e) => super::events::journal_skipped(&path, &e),
                }
            }
        }
    } else if journals_path.is_file() {
        let journal = read_journal(journals_path)?;
        signals.accumulate(&journal);
        store.ingest_journal(&journal)?;
        count = 1;
    } else {
        bail!("path does not exist: {}", journals_path.display());
    }

    let full_name = core_framework.full_name();
    crate::outln!("=== {full_name} ===\n");
    crate::outln!("{}\n", tumult_core::compliance::EVIDENCE_DISCLAIMER);
    crate::outln!("Journals analyzed: {count}");
    crate::outln!(
        "With regulatory tagging: {}\n",
        signals.journals_with_regulatory
    );

    // Overall status
    let rows = store.query(
        "SELECT status, count(*) as runs FROM experiments GROUP BY status ORDER BY runs DESC",
    )?;
    crate::outln!("Experiment Results:");
    for row in &rows {
        crate::outln!("  {}: {} run(s)", row[0], row[1]);
    }

    // Compliance verdict derived from the journal-level pass rate and the
    // share of the runs that recovered within their target (pass rate only
    // when no run measured a recovery). The verdict token is an EVIDENCE-strength
    // signal, not a compliance attestation — see EVIDENCE_DISCLAIMER above.
    let pass_rate = signals.pass_rate();
    let recovery_compliance: Option<f64> = signals.recovery_compliance(MTTR_TARGET_S);
    let verdict = compliance_verdict(pass_rate, recovery_compliance);

    crate::outln!("\nEvidence summary (NOT a compliance determination):");
    crate::outln!("  Pass rate: {:.1}%", pass_rate * 100.0);
    crate::outln!("  {}", signals.recovery_line(MTTR_TARGET_S));
    crate::outln!("  Evidence verdict: {verdict}");

    // Framework-specific control citations, rendered from the single sourced,
    // dated registry in `tumult_core::compliance` (no hardcoded article
    // strings — edit the registry to update every surface at once).
    crate::outln!("\nSource: {}", core_framework.source_url());
    crate::outln!("Mapped controls (evidence toward, not proof of, compliance):\n");
    print_citations(core_framework);

    crate::outln!("\n=== End Report ===");
    Ok(())
}

/// Print every registry citation for `framework` with its title, summary,
/// evidence note, evidence-strength grade, source URL and last-verified date.
fn print_citations(framework: tumult_core::compliance::ComplianceFramework) {
    for c in framework.citations() {
        crate::outln!("  {} — {}", c.control_id, c.title);
        crate::outln!("    Requires: {}", c.summary);
        crate::outln!(
            "    Evidence [{} / {}]: {}",
            c.strength.as_str(),
            c.evidence_type.as_str(),
            c.evidence_note
        );
        crate::outln!(
            "    Source: {} (last verified {})",
            c.source_url,
            c.last_verified
        );
    }
}

/// `--sources`: list the dated, sourced citation registry for `framework`.
fn print_sources(framework: tumult_core::compliance::ComplianceFramework) {
    use tumult_core::compliance::REGISTRY_VERSION;

    crate::outln!("=== {} — citation registry ===\n", framework.full_name());
    crate::outln!("Registry version: {REGISTRY_VERSION}");
    crate::outln!("{}\n", tumult_core::compliance::EVIDENCE_DISCLAIMER);
    print_citations(framework);
    crate::outln!("\n=== End registry ===");
}

// The recovery-aware compliance verdict is owned by
// `tumult_core::compliance` so the CLI and MCP server share one source of
// truth. Re-exported here for the existing test surface.
pub(crate) use tumult_core::compliance::compliance_verdict;
