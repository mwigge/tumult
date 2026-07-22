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
    /// Apache Arrow IPC stream format
    Arrow,
}

/// Report output format.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportFormat {
    /// HTML report
    Html,
    /// PDF (generates HTML then prints instructions for conversion)
    Pdf,
    /// JSON (raw journal serialized via `serde_json`)
    Json,
    /// `JUnit` XML (one testcase per activity across all phases)
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
    /// Maps the clap value enum onto the shared domain enum in
    /// `tumult_core::compliance` (the single source of truth for report
    /// identifiers, full names, and verdict logic).
    #[must_use]
    pub fn to_core(self) -> tumult_core::compliance::ComplianceFramework {
        use tumult_core::compliance::ComplianceFramework as Core;
        match self {
            ComplianceFramework::Dora => Core::Dora,
            ComplianceFramework::Nis2 => Core::Nis2,
            ComplianceFramework::PciDss => Core::PciDss,
            ComplianceFramework::Iso22301 => Core::Iso22301,
            ComplianceFramework::Iso27001 => Core::Iso27001,
            ComplianceFramework::Soc2 => Core::Soc2,
            ComplianceFramework::BaselIii => Core::BaselIii,
        }
    }

    /// Returns the canonical string identifier used in report output.
    #[must_use]
    pub fn as_report_str(&self) -> &'static str {
        self.to_core().as_report_str()
    }
}

/// Load test tool selection.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadToolArg {
    /// k6 load testing tool
    K6,
    /// Explicitly disable load testing even if the experiment defines it
    None,
}

// ── CLI helper functions ──────────────────────────────────────

/// Parses a human duration like "30s", "5m", "1h" to seconds.
///
/// # Errors
///
/// Returns an error if the value is not a number with an optional `s`, `m`,
/// or `h` suffix — a typo must surface, not silently become a 30-second
/// default.
pub fn parse_duration_str(s: &str) -> anyhow::Result<f64> {
    let s = s.trim();
    let (num, multiplier) = if let Some(num) = s.strip_suffix('s') {
        (num, 1.0)
    } else if let Some(num) = s.strip_suffix('m') {
        (num, 60.0)
    } else if let Some(num) = s.strip_suffix('h') {
        (num, 3600.0)
    } else {
        (s, 1.0)
    };
    let value = num.parse::<f64>().map_err(|_| {
        anyhow::anyhow!(
            "invalid duration {s:?} — expected a number with an optional s/m/h suffix (e.g. 30s, 5m, 1h)"
        )
    })?;
    Ok(value * multiplier)
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
///
/// # Errors
///
/// Returns an error if the duration value is not a valid duration string.
pub fn build_load_override(
    tool: Option<LoadToolArg>,
    script: Option<std::path::PathBuf>,
    vus: Option<u32>,
    duration: Option<String>,
) -> anyhow::Result<Option<tumult_core::types::LoadConfig>> {
    // --load none explicitly disables
    if matches!(tool, Some(LoadToolArg::None)) {
        return Ok(None);
    }

    // No --load flag at all → no override
    let Some(tool) = tool else {
        return Ok(None);
    };
    let script = script.unwrap_or_else(|| std::path::PathBuf::from("load.js"));
    let duration_s = duration.map(|d| parse_duration_str(&d)).transpose()?;

    let load_tool = match tool {
        LoadToolArg::K6 => tumult_core::types::LoadTool::K6,
        LoadToolArg::None => unreachable!(),
    };

    Ok(Some(tumult_core::types::LoadConfig {
        tool: load_tool,
        script,
        vus: Some(vus.unwrap_or(10)),
        duration_s: duration_s.or(Some(30.0)),
        thresholds: std::collections::HashMap::new(),
    }))
}
