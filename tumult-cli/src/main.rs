use tumult_cli::commands;

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "tumult",
    version,
    propagate_version = true,
    about = "Rust-native chaos engineering platform"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
enum RollbackStrategy {
    Always,
    Deviated,
    Never,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
enum ExportFormat {
    Parquet,
    Csv,
    Json,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
enum BaselineMode {
    /// Run full baseline then inject fault (default)
    Full,
    /// Skip baseline, use static tolerances
    Skip,
    /// Run baseline only, no fault injection
    Only,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
enum ComplianceFramework {
    Dora,
    Nis2,
    #[value(name = "pci-dss")]
    PciDss,
    #[value(name = "iso-22301")]
    Iso22301,
    #[value(name = "iso-27001")]
    Iso27001,
    Soc2,
    #[value(name = "basel-iii")]
    BaselIii,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Execute a chaos experiment
    Run {
        /// Path to experiment .toon file
        experiment: PathBuf,
        /// Output journal location
        #[arg(long, default_value = "journal.toon")]
        journal_path: PathBuf,
        /// Validate and show plan without executing
        #[arg(long)]
        dry_run: bool,
        /// Rollback strategy
        #[arg(long, default_value_t = RollbackStrategy::Deviated, value_enum)]
        rollback_strategy: RollbackStrategy,
        /// Baseline mode
        #[arg(long, default_value_t = BaselineMode::Full, value_enum)]
        baseline_mode: BaselineMode,
    },
    /// Validate experiment syntax and plugin references
    Validate {
        /// Path to experiment .toon file
        experiment: PathBuf,
    },
    /// List all discovered plugins, actions, and probes
    Discover {
        /// Show details for a specific plugin
        #[arg(long)]
        plugin: Option<String>,
    },
    /// SQL analytics over journal files
    Analyze {
        /// Directory containing journal files
        journals: PathBuf,
        /// SQL query to execute
        #[arg(long)]
        query: Option<String>,
    },
    /// Convert journal to other formats
    Export {
        /// Journal file to export
        journal: PathBuf,
        /// Output format
        #[arg(long, default_value_t = ExportFormat::Parquet, value_enum)]
        format: ExportFormat,
    },
    /// Regulatory compliance report
    Compliance {
        /// Directory containing journal files
        journals: PathBuf,
        /// Target regulatory framework
        #[arg(long, value_enum)]
        framework: ComplianceFramework,
    },
    /// Generate HTML report from journal
    Report {
        /// Journal file
        journal: PathBuf,
        /// Output path
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Interactive experiment creation
    Init {
        /// Start with a specific plugin
        #[arg(long)]
        plugin: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            experiment,
            journal_path,
            dry_run,
            rollback_strategy,
            baseline_mode: _,
        } => {
            let strategy = match rollback_strategy {
                RollbackStrategy::Always => tumult_core::execution::RollbackStrategy::Always,
                RollbackStrategy::Deviated => tumult_core::execution::RollbackStrategy::OnDeviation,
                RollbackStrategy::Never => tumult_core::execution::RollbackStrategy::Never,
            };
            commands::cmd_run(&experiment, &journal_path, dry_run, strategy)?;
        }
        Commands::Validate { experiment } => {
            commands::cmd_validate(&experiment)?;
        }
        Commands::Discover { plugin } => {
            commands::cmd_discover(plugin.as_deref())?;
        }
        Commands::Init { plugin } => {
            commands::cmd_init(plugin.as_deref())?;
        }
        Commands::Analyze { .. } => {
            anyhow::bail!("analyze command requires tumult-analytics (Phase 2)");
        }
        Commands::Export { .. } => {
            anyhow::bail!("export command requires tumult-analytics (Phase 2)");
        }
        Commands::Compliance { .. } => {
            anyhow::bail!("compliance command requires tumult-regulatory (Phase 2)");
        }
        Commands::Report { .. } => {
            anyhow::bail!("report command requires tumult-report (Phase 3)");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::CommandFactory;

    // ── CLI configuration ──────────────────────────────────────

    #[test]
    fn cli_debug_assert() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_subcommand_is_error() {
        assert!(Cli::try_parse_from(["tumult"]).is_err());
    }

    #[test]
    fn unknown_subcommand_is_error() {
        assert!(Cli::try_parse_from(["tumult", "destroy"]).is_err());
    }

    #[test]
    fn version_flag_is_recognized() {
        let err = Cli::try_parse_from(["tumult", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn help_flag_is_recognized() {
        let err = Cli::try_parse_from(["tumult", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    // ── Run ────────────────────────────────────────────────────

    #[test]
    fn parse_run_minimal() {
        let cli = Cli::try_parse_from(["tumult", "run", "experiment.toon"]).unwrap();
        let Commands::Run {
            experiment,
            journal_path,
            dry_run,
            rollback_strategy,
            baseline_mode,
        } = cli.command
        else {
            panic!("expected Run command");
        };
        assert_eq!(experiment, PathBuf::from("experiment.toon"));
        assert_eq!(journal_path, PathBuf::from("journal.toon"));
        assert!(!dry_run);
        assert_eq!(rollback_strategy, RollbackStrategy::Deviated);
        assert_eq!(baseline_mode, BaselineMode::Full);
    }

    #[test]
    fn parse_run_all_flags() {
        let cli = Cli::try_parse_from([
            "tumult",
            "run",
            "my-exp.toon",
            "--journal-path",
            "out.toon",
            "--dry-run",
            "--rollback-strategy",
            "always",
            "--baseline-mode",
            "skip",
        ])
        .unwrap();
        let Commands::Run {
            experiment,
            journal_path,
            dry_run,
            rollback_strategy,
            baseline_mode,
        } = cli.command
        else {
            panic!("expected Run command");
        };
        assert_eq!(experiment, PathBuf::from("my-exp.toon"));
        assert_eq!(journal_path, PathBuf::from("out.toon"));
        assert!(dry_run);
        assert_eq!(rollback_strategy, RollbackStrategy::Always);
        assert_eq!(baseline_mode, BaselineMode::Skip);
    }

    #[test]
    fn parse_run_baseline_only_mode() {
        let cli =
            Cli::try_parse_from(["tumult", "run", "exp.toon", "--baseline-mode", "only"]).unwrap();
        let Commands::Run { baseline_mode, .. } = cli.command else {
            panic!("expected Run command");
        };
        assert_eq!(baseline_mode, BaselineMode::Only);
    }

    #[test]
    fn parse_run_rollback_never() {
        let cli =
            Cli::try_parse_from(["tumult", "run", "exp.toon", "--rollback-strategy", "never"])
                .unwrap();
        let Commands::Run {
            rollback_strategy, ..
        } = cli.command
        else {
            panic!("expected Run command");
        };
        assert_eq!(rollback_strategy, RollbackStrategy::Never);
    }

    #[test]
    fn parse_run_invalid_rollback_strategy_is_error() {
        let result =
            Cli::try_parse_from(["tumult", "run", "exp.toon", "--rollback-strategy", "maybe"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_run_invalid_baseline_mode_is_error() {
        let result =
            Cli::try_parse_from(["tumult", "run", "exp.toon", "--baseline-mode", "partial"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_run_requires_experiment_path() {
        let err = Cli::try_parse_from(["tumult", "run"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn parse_run_unknown_flag_is_error() {
        let result = Cli::try_parse_from(["tumult", "run", "exp.toon", "--nonexistent"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_run_path_with_spaces() {
        let cli =
            Cli::try_parse_from(["tumult", "run", "path with spaces/experiment.toon"]).unwrap();
        let Commands::Run { experiment, .. } = cli.command else {
            panic!("expected Run command");
        };
        assert_eq!(
            experiment,
            PathBuf::from("path with spaces/experiment.toon")
        );
    }

    #[test]
    fn parse_run_path_with_unicode() {
        let cli =
            Cli::try_parse_from(["tumult", "run", "experiments/résilience-test.toon"]).unwrap();
        let Commands::Run { experiment, .. } = cli.command else {
            panic!("expected Run command");
        };
        assert_eq!(
            experiment,
            PathBuf::from("experiments/résilience-test.toon")
        );
    }

    #[test]
    fn parse_run_absolute_path() {
        let cli = Cli::try_parse_from(["tumult", "run", "/absolute/path/experiment.toon"]).unwrap();
        let Commands::Run { experiment, .. } = cli.command else {
            panic!("expected Run command");
        };
        assert_eq!(experiment, PathBuf::from("/absolute/path/experiment.toon"));
    }

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
    fn parse_analyze_minimal() {
        let cli = Cli::try_parse_from(["tumult", "analyze", "journals/"]).unwrap();
        let Commands::Analyze { journals, query } = cli.command else {
            panic!("expected Analyze command");
        };
        assert_eq!(journals, PathBuf::from("journals/"));
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
    fn parse_analyze_requires_journals_path() {
        let err = Cli::try_parse_from(["tumult", "analyze"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
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
        let Commands::Report { journal, output } = cli.command else {
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
}
