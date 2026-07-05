//! The supported regulatory framework catalog.

use super::citations::{Citation, CITATIONS};

/// A supported regulatory compliance framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComplianceFramework {
    /// EU Digital Operational Resilience Act (EU 2022/2554).
    Dora,
    /// EU Network and Information Security Directive (EU 2022/2555).
    Nis2,
    /// Payment Card Industry Data Security Standard 4.0.
    PciDss,
    /// ISO 22301 Business Continuity Management.
    Iso22301,
    /// ISO 27001 Information Security Management.
    Iso27001,
    /// SOC 2 Service Organization Control Type 2.
    Soc2,
    /// Basel III / BCBS 239 Risk Data Aggregation.
    BaselIii,
}

impl ComplianceFramework {
    /// Every supported framework, in display order.
    pub const ALL: [Self; 7] = [
        Self::Dora,
        Self::Nis2,
        Self::PciDss,
        Self::Iso22301,
        Self::Iso27001,
        Self::Soc2,
        Self::BaselIii,
    ];

    /// Parse a framework from its CLI value name (e.g. `dora`, `pci-dss`),
    /// case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns an error message naming the bad value and listing every valid
    /// value when `value` matches no framework.
    pub fn parse(value: &str) -> Result<Self, String> {
        let normalized = value.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|framework| framework.name() == normalized)
            .ok_or_else(|| {
                let valid: Vec<&str> = Self::ALL.iter().map(|f| f.name()).collect();
                format!(
                    "unknown framework '{value}'; valid values: {}",
                    valid.join(", ")
                )
            })
    }

    /// Stable lowercase identifier, matching the CLI's `--framework` values.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dora => "dora",
            Self::Nis2 => "nis2",
            Self::PciDss => "pci-dss",
            Self::Iso22301 => "iso-22301",
            Self::Iso27001 => "iso-27001",
            Self::Soc2 => "soc2",
            Self::BaselIii => "basel-iii",
        }
    }

    /// Canonical string identifier used in report output.
    #[must_use]
    pub fn as_report_str(self) -> &'static str {
        match self {
            Self::Dora => "DORA",
            Self::Nis2 => "NIS2",
            Self::PciDss => "PCI-DSS",
            Self::Iso22301 => "ISO-22301",
            Self::Iso27001 => "ISO-27001",
            Self::Soc2 => "SOC2",
            Self::BaselIii => "Basel-III",
        }
    }

    /// Full human-readable framework name with its source reference.
    #[must_use]
    pub fn full_name(self) -> &'static str {
        match self {
            Self::Dora => "DORA — Digital Operational Resilience Act (EU 2022/2554)",
            Self::Nis2 => "NIS2 — Network and Information Security Directive (EU 2022/2555)",
            Self::PciDss => "PCI-DSS 4.0 — Payment Card Industry Data Security Standard",
            Self::Iso22301 => "ISO 22301 — Business Continuity Management Systems",
            Self::Iso27001 => "ISO 27001 — Information Security Management Systems",
            Self::Soc2 => "SOC 2 — Service Organization Control Type 2 (TSC 2017)",
            Self::BaselIii => {
                "Basel Committee (BCBS) — operational resilience & risk data aggregation"
            }
        }
    }

    /// Primary official source URL for the framework (the canonical text an
    /// auditor would consult). Individual control citations carry their own
    /// [`Citation::source_url`], which may differ (e.g. the Basel operational
    /// resilience principles live in a different publication from BCBS 239).
    #[must_use]
    pub fn source_url(self) -> &'static str {
        match self {
            Self::Dora => "https://eur-lex.europa.eu/eli/reg/2022/2554/oj",
            Self::Nis2 => "https://eur-lex.europa.eu/eli/dir/2022/2555/oj",
            Self::PciDss => "https://www.pcisecuritystandards.org/document_library/",
            Self::Iso22301 => "https://www.iso.org/standard/75106.html",
            Self::Iso27001 => "https://www.iso.org/standard/27001",
            Self::Soc2 => {
                "https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2"
            }
            Self::BaselIii => "https://www.bis.org/bcbs/publ/d516.htm",
        }
    }

    /// All registry [`Citation`]s that map evidence to this framework, in
    /// registry order.
    #[must_use]
    pub fn citations(self) -> Vec<&'static Citation> {
        CITATIONS.iter().filter(|c| c.framework == self).collect()
    }
}
