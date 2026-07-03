use std::path::Path;

use anyhow::{Context, Result};

use super::ReportFormat;

mod escape;
mod html;
mod junit;

pub(crate) use html::generate_html_report;
pub(crate) use junit::generate_junit_report;

use junit::generate_json_report;

// ── Report command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the journal cannot be read or the report cannot be
/// written to disk.
#[must_use = "callers must handle report generation errors"]
pub fn cmd_report(
    journal_path: &Path,
    output: Option<&Path>,
    format: ReportFormat,
    trace_ui_base_arg: Option<&str>,
) -> Result<()> {
    use tumult_core::journal::read_journal;

    let journal = read_journal(journal_path)
        .with_context(|| format!("failed to read journal: {}", journal_path.display()))?;

    let stem = journal_path
        .file_stem()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("report");
    let ext = match format {
        ReportFormat::Pdf => "pdf",
        ReportFormat::Junit => "xml",
        ReportFormat::Json => "json",
        ReportFormat::Html => "html",
    };
    let out_path = output.map_or_else(
        || std::path::PathBuf::from(format!("{stem}.{ext}")),
        std::path::Path::to_path_buf,
    );

    match format {
        ReportFormat::Junit => {
            let xml = generate_junit_report(&journal);
            std::fs::write(&out_path, &xml)?;
            println!("Report generated: {}", out_path.display());
        }
        ReportFormat::Json => {
            let json = generate_json_report(&journal)?;
            std::fs::write(&out_path, &json)?;
            println!("Report generated: {}", out_path.display());
        }
        ReportFormat::Html | ReportFormat::Pdf => {
            // FIX 4: stable, non-cryptographic content hash of the raw journal bytes.
            // Uses std DefaultHasher (no crypto crate in the workspace) — acceptable for
            // provenance/tamper-evidence signalling, not a security guarantee.
            use std::hash::{Hash, Hasher};
            let raw = std::fs::read(journal_path).unwrap_or_default();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            raw.hash(&mut h);
            let source_hash = format!("{:016x}", h.finish());

            // FIX 2: trace UI base resolves flag > env var, off by default.
            // This is the trace UI, NOT the OTLP ingest endpoint.
            let trace_ui_base = trace_ui_base_arg
                .map(str::to_string)
                .or_else(|| std::env::var("TUMULT_TRACE_UI_BASE").ok())
                .filter(|s| !s.is_empty());

            let html = generate_html_report(&journal, trace_ui_base.as_deref(), &source_hash);

            if matches!(format, ReportFormat::Pdf) {
                // PDF: write HTML first, then note that wkhtmltopdf or browser print is needed
                std::fs::write(out_path.with_extension("html"), &html)?;
                println!(
                    "HTML generated: {}",
                    out_path.with_extension("html").display()
                );
                println!(
                    "To convert to PDF, use: wkhtmltopdf {} {}",
                    out_path.with_extension("html").display(),
                    out_path.display()
                );
                println!("Or open the HTML in a browser and print to PDF.");
            } else {
                std::fs::write(&out_path, &html)?;
                println!("Report generated: {}", out_path.display());
            }
        }
    }

    Ok(())
}
