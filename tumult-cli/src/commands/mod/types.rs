//! Typed CLI enums and small argument-parsing helpers shared across commands.

// ── Typed CLI enums ───────────────────────────────────────────

/// Export format for journal files.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// Apache Parquet columnar format
    Parquet,
    /// Comma-separated values
    Csv,
    /// JSON
    Json,
}

/// Report output format.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportFormat {
    /// HTML report
    Html,
    /// PDF (generates HTML then prints instructions for conversion)
    Pdf,
    /// JSON (raw journal serialized via serde_json)
    Json,
    /// JUnit XML (one testcase per activity across all phases)
    Junit,
}

/// Regulatory compliance framework.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComplianceFramework {
    /// EU Digital Operational Resilience Act
    Dora,
    /// EU Network and Information Security Directive
    Nis2,
    /// Payment Card Industry Data Security Standard
    #[value(name = "pci-dss")]
    PciDss,
    /// ISO 22301 Business Continuity Management
    #[value(name = "iso-22301")]
    Iso22301,
    /// ISO 27001 Information Security Management
    #[value(name = "iso-27001")]
    Iso27001,
    /// SOC 2 Service Organization Control Type 2
    Soc2,
    /// Basel III / BCBS 239 Risk Data Aggregation
    #[value(name = "basel-iii")]
    BaselIii,
}

impl ComplianceFramework {
    /// Returns the canonical string identifier used in report output.
    #[must_use]
    pub fn as_report_str(&self) -> &'static str {
        match self {
            ComplianceFramework::Dora => "DORA",
            ComplianceFramework::Nis2 => "NIS2",
            ComplianceFramework::PciDss => "PCI-DSS",
            ComplianceFramework::Iso22301 => "ISO-22301",
            ComplianceFramework::Iso27001 => "ISO-27001",
            ComplianceFramework::Soc2 => "SOC2",
            ComplianceFramework::BaselIii => "Basel-III",
        }
    }
}

/// Load test tool selection.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadToolArg {
    /// k6 load testing tool
    K6,
    /// Apache `JMeter` load testing tool
    Jmeter,
    /// Explicitly disable load testing even if the experiment defines it
    None,
}

// ── CLI helper functions ──────────────────────────────────────

/// Parses a human duration like "30s", "5m", "1h" to seconds.
#[must_use]
pub fn parse_duration_str(s: &str) -> f64 {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('s') {
        num.parse().unwrap_or(30.0)
    } else if let Some(num) = s.strip_suffix('m') {
        num.parse::<f64>().unwrap_or(1.0) * 60.0
    } else if let Some(num) = s.strip_suffix('h') {
        num.parse::<f64>().unwrap_or(1.0) * 3600.0
    } else {
        s.parse().unwrap_or(30.0)
    }
}

/// Parses `--var KEY=VALUE` arguments into a `HashMap`.
///
/// # Errors
///
/// Returns an error if any argument does not contain `=`.
pub fn parse_var_args(
    vars: &[String],
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut map = std::collections::HashMap::new();
    for entry in vars {
        let (key, value) = entry.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("--var argument must be in KEY=VALUE format, got: {entry:?}")
        })?;
        map.insert(key.to_string(), value.to_string());
    }
    Ok(map)
}

/// Builds a `LoadConfig` override from CLI flags.
///
/// Returns `None` if `--load none` was specified (explicitly disable load).
/// Returns `None` if no `--load` flag was given at all (use experiment default).
/// Returns `Some(config)` if a real load tool was specified (override experiment).
#[must_use]
pub fn build_load_override(
    tool: Option<LoadToolArg>,
    script: Option<std::path::PathBuf>,
    vus: Option<u32>,
    duration: Option<String>,
) -> Option<tumult_core::types::LoadConfig> {
    // --load none explicitly disables
    if matches!(tool, Some(LoadToolArg::None)) {
        return None;
    }

    let tool = tool?; // No --load flag at all → no override
    let script = script.unwrap_or_else(|| std::path::PathBuf::from("load.js"));
    let duration_s = duration.map(|d| parse_duration_str(&d));

    let load_tool = match tool {
        LoadToolArg::K6 => tumult_core::types::LoadTool::K6,
        LoadToolArg::Jmeter => tumult_core::types::LoadTool::Jmeter,
        LoadToolArg::None => unreachable!(),
    };

    Some(tumult_core::types::LoadConfig {
        tool: load_tool,
        script,
        vus: Some(vus.unwrap_or(10)),
        duration_s: duration_s.or(Some(30.0)),
        thresholds: std::collections::HashMap::new(),
    })
}
