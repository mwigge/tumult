//! Parser tests for the non-agentic reporting/analytics subcommands.

use crate::cli::*;

use clap::Parser;
use std::path::PathBuf;

// ── Validate ───────────────────────────────────────────────

#[test]
fn parse_validate() {
    let cli = Cli::try_parse_from(["tumult", "validate", "test.toon"]).unwrap();
    let Commands::Validate { experiment } = cli.command else {
        panic!("expected Validate command");
    };
    assert_eq!(experiment, PathBuf::from("test.toon"));
}

#[test]
fn parse_validate_requires_path() {
    let err = Cli::try_parse_from(["tumult", "validate"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

// ── Discover ───────────────────────────────────────────────

#[test]
fn parse_discover_no_args() {
    let cli = Cli::try_parse_from(["tumult", "discover"]).unwrap();
    let Commands::Discover { plugin } = cli.command else {
        panic!("expected Discover command");
    };
    assert!(plugin.is_none());
}

#[test]
fn parse_discover_with_plugin() {
    let cli = Cli::try_parse_from(["tumult", "discover", "--plugin", "tumult-kafka"]).unwrap();
    let Commands::Discover { plugin } = cli.command else {
        panic!("expected Discover command");
    };
    assert_eq!(plugin.unwrap(), "tumult-kafka");
}

// ── Analyze ────────────────────────────────────────────────

#[test]
fn parse_analyze_with_path() {
    let cli = Cli::try_parse_from(["tumult", "analyze", "journals/"]).unwrap();
    let Commands::Analyze {
        journals, query, ..
    } = cli.command
    else {
        panic!("expected Analyze command");
    };
    assert_eq!(journals, Some(PathBuf::from("journals/")));
    assert!(query.is_none());
}

#[test]
fn parse_analyze_with_query() {
    let cli = Cli::try_parse_from([
        "tumult",
        "analyze",
        "journals/",
        "--query",
        "SELECT * FROM experiments",
    ])
    .unwrap();
    let Commands::Analyze { query, .. } = cli.command else {
        panic!("expected Analyze command");
    };
    assert_eq!(query.unwrap(), "SELECT * FROM experiments");
}

#[test]
fn parse_analyze_no_path_uses_persistent_store() {
    let cli = Cli::try_parse_from(["tumult", "analyze"]).unwrap();
    let Commands::Analyze { journals, .. } = cli.command else {
        panic!("expected Analyze command");
    };
    assert!(journals.is_none());
}

#[test]
fn parse_analyze_query_only() {
    let cli = Cli::try_parse_from([
        "tumult",
        "analyze",
        "--query",
        "SELECT count(*) FROM experiments",
    ])
    .unwrap();
    let Commands::Analyze {
        journals, query, ..
    } = cli.command
    else {
        panic!("expected Analyze command");
    };
    assert!(journals.is_none());
    assert!(query.is_some());
}

// ── Export ──────────────────────────────────────────────────

#[test]
fn parse_export_defaults_to_parquet() {
    let cli = Cli::try_parse_from(["tumult", "export", "journal.toon"]).unwrap();
    let Commands::Export { journal, format } = cli.command else {
        panic!("expected Export command");
    };
    assert_eq!(journal, PathBuf::from("journal.toon"));
    assert_eq!(format, ExportFormat::Parquet);
}

#[test]
fn parse_export_csv_format() {
    let cli =
        Cli::try_parse_from(["tumult", "export", "journal.toon", "--format", "csv"]).unwrap();
    let Commands::Export { format, .. } = cli.command else {
        panic!("expected Export command");
    };
    assert_eq!(format, ExportFormat::Csv);
}

#[test]
fn parse_export_json_format() {
    let cli =
        Cli::try_parse_from(["tumult", "export", "journal.toon", "--format", "json"]).unwrap();
    let Commands::Export { format, .. } = cli.command else {
        panic!("expected Export command");
    };
    assert_eq!(format, ExportFormat::Json);
}

#[test]
fn parse_export_invalid_format_is_error() {
    let result = Cli::try_parse_from(["tumult", "export", "journal.toon", "--format", "xml"]);
    assert!(result.is_err());
}

#[test]
fn parse_export_requires_journal_path() {
    let err = Cli::try_parse_from(["tumult", "export"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

// ── Compliance ─────────────────────────────────────────────

#[test]
fn parse_compliance_dora() {
    let cli = Cli::try_parse_from(["tumult", "compliance", "journals/", "--framework", "dora"])
        .unwrap();
    let Commands::Compliance {
        journals,
        framework,
    } = cli.command
    else {
        panic!("expected Compliance command");
    };
    assert_eq!(journals, PathBuf::from("journals/"));
    assert_eq!(framework, ComplianceFramework::Dora);
}

#[test]
fn parse_compliance_pci_dss() {
    let cli = Cli::try_parse_from([
        "tumult",
        "compliance",
        "journals/",
        "--framework",
        "pci-dss",
    ])
    .unwrap();
    let Commands::Compliance { framework, .. } = cli.command else {
        panic!("expected Compliance command");
    };
    assert_eq!(framework, ComplianceFramework::PciDss);
}

#[test]
fn parse_compliance_invalid_framework_is_error() {
    let result =
        Cli::try_parse_from(["tumult", "compliance", "journals/", "--framework", "hipaa"]);
    assert!(result.is_err());
}

#[test]
fn parse_compliance_requires_framework() {
    let err = Cli::try_parse_from(["tumult", "compliance", "journals/"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

#[test]
fn parse_compliance_requires_journals_path() {
    let err = Cli::try_parse_from(["tumult", "compliance", "--framework", "dora"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

// ── Report ─────────────────────────────────────────────────

#[test]
fn parse_report_minimal() {
    let cli = Cli::try_parse_from(["tumult", "report", "journal.toon"]).unwrap();
    let Commands::Report {
        journal, output, ..
    } = cli.command
    else {
        panic!("expected Report command");
    };
    assert_eq!(journal, PathBuf::from("journal.toon"));
    assert!(output.is_none());
}

#[test]
fn parse_report_with_output() {
    let cli = Cli::try_parse_from([
        "tumult",
        "report",
        "journal.toon",
        "--output",
        "report.html",
    ])
    .unwrap();
    let Commands::Report { output, .. } = cli.command else {
        panic!("expected Report command");
    };
    assert_eq!(output.unwrap(), PathBuf::from("report.html"));
}

#[test]
fn parse_report_requires_journal_path() {
    let err = Cli::try_parse_from(["tumult", "report"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

#[test]
fn parse_report_junit_format() {
    let cli =
        Cli::try_parse_from(["tumult", "report", "journal.toon", "--format", "junit"]).unwrap();
    let Commands::Report { format, .. } = cli.command else {
        panic!("expected Report command");
    };
    assert_eq!(format, ReportFormat::Junit);
}

#[test]
fn parse_report_json_format() {
    let cli =
        Cli::try_parse_from(["tumult", "report", "journal.toon", "--format", "json"]).unwrap();
    let Commands::Report { format, .. } = cli.command else {
        panic!("expected Report command");
    };
    assert_eq!(format, ReportFormat::Json);
}

#[test]
fn parse_report_trace_ui_base() {
    let cli = Cli::try_parse_from([
        "tumult",
        "report",
        "journal.toon",
        "--trace-ui-base",
        "https://tempo.example",
    ])
    .unwrap();
    let Commands::Report { trace_ui_base, .. } = cli.command else {
        panic!("expected Report command");
    };
    assert_eq!(trace_ui_base.as_deref(), Some("https://tempo.example"));
}

// ── Init ───────────────────────────────────────────────────

#[test]
fn parse_init_no_args() {
    let cli = Cli::try_parse_from(["tumult", "init"]).unwrap();
    let Commands::Init { plugin } = cli.command else {
        panic!("expected Init command");
    };
    assert!(plugin.is_none());
}

#[test]
fn parse_init_with_plugin() {
    let cli = Cli::try_parse_from(["tumult", "init", "--plugin", "tumult-db"]).unwrap();
    let Commands::Init { plugin } = cli.command else {
        panic!("expected Init command");
    };
    assert_eq!(plugin.unwrap(), "tumult-db");
}

// ── Recommend ─────────────────────────────────────────────

#[test]
fn parse_recommend_defaults() {
    let cli = Cli::try_parse_from(["tumult", "recommend"]).unwrap();
    let Commands::Recommend {
        goal,
        store_path,
        model,
        no_draft,
        format,
    } = cli.command
    else {
        panic!("expected Recommend command");
    };
    assert!(goal.is_none());
    assert!(store_path.is_none());
    assert!(model.is_none());
    assert!(!no_draft);
    assert_eq!(format, RecommendFormat::Text);
}

#[test]
fn parse_recommend_all_flags() {
    let cli = Cli::try_parse_from([
        "tumult",
        "recommend",
        "--goal",
        "test database failover",
        "--store-path",
        "analytics.duckdb",
        "--model",
        "qwen3",
        "--no-draft",
        "--format",
        "json",
    ])
    .unwrap();
    let Commands::Recommend {
        goal,
        store_path,
        model,
        no_draft,
        format,
    } = cli.command
    else {
        panic!("expected Recommend command");
    };
    assert_eq!(goal.as_deref(), Some("test database failover"));
    assert_eq!(store_path, Some(PathBuf::from("analytics.duckdb")));
    assert_eq!(model.as_deref(), Some("qwen3"));
    assert!(no_draft);
    assert_eq!(format, RecommendFormat::Json);
}

#[test]
fn parse_recommend_text_format() {
    let cli = Cli::try_parse_from(["tumult", "recommend", "--format", "text"]).unwrap();
    let Commands::Recommend { format, .. } = cli.command else {
        panic!("expected Recommend command");
    };
    assert_eq!(format, RecommendFormat::Text);
}

#[test]
fn parse_recommend_invalid_format_is_error() {
    let result = Cli::try_parse_from(["tumult", "recommend", "--format", "xml"]);
    assert!(result.is_err());
}
