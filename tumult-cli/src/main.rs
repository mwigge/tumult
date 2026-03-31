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

/// Maps to tumult_core::execution::RollbackStrategy
#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
enum RollbackStrategy {
    Always,
    #[value(alias = "deviated")]
    OnDeviation,
    Never,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
enum OutputFormat {
    /// Print journal as JSON to stdout
    Json,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
enum ExportFormat {
    Parquet,
    Csv,
    Json,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
enum ReportFormat {
    Html,
    Pdf,
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
        #[arg(long, default_value_t = RollbackStrategy::OnDeviation, value_enum)]
        rollback_strategy: RollbackStrategy,
        /// Baseline mode
        #[arg(long, default_value_t = BaselineMode::Full, value_enum)]
        baseline_mode: BaselineMode,
        /// Skip auto-ingestion into persistent analytics store
        #[arg(long)]
        no_ingest: bool,
        /// Output format for journal (human-readable summary or JSON to stdout)
        #[arg(long, value_enum)]
        output_format: Option<OutputFormat>,
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
        /// Directory containing journal files (omit to use persistent store)
        journals: Option<PathBuf>,
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
    /// Generate report from journal (HTML or PDF)
    Report {
        /// Journal file
        journal: PathBuf,
        /// Output path
        #[arg(long)]
        output: Option<PathBuf>,
        /// Report format
        #[arg(long, default_value_t = ReportFormat::Html, value_enum)]
        format: ReportFormat,
    },
    /// Cross-run trend analysis
    Trend {
        /// Directory containing journal files
        journals: PathBuf,
        /// Metric to track
        #[arg(long, default_value = "resilience_score")]
        metric: String,
        /// Time window (e.g., 30d, 90d)
        #[arg(long)]
        last: Option<String>,
        /// Filter by target technology (matches experiment title)
        #[arg(long)]
        target: Option<String>,
    },
    /// Interactive experiment creation
    Init {
        /// Start with a specific plugin
        #[arg(long)]
        plugin: Option<String>,
    },
    /// Import journals from Parquet backup
    Import {
        /// Directory containing Parquet backup files
        parquet_dir: PathBuf,
    },
    /// Persistent analytics store management
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
}

#[derive(clap::Subcommand, Debug)]
enum StoreAction {
    /// Show store statistics
    Stats,
    /// Export entire store to Parquet backup
    Backup {
        /// Output directory for backup files
        #[arg(long, default_value = "tumult-backup")]
        output: PathBuf,
    },
    /// Purge experiments older than N days
    Purge {
        /// Number of days to retain
        #[arg(long)]
        older_than_days: u32,
    },
    /// Show store file path
    Path,
    /// Migrate data from DuckDB to ClickHouse
    Migrate,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize OpenTelemetry from environment
    let otel_config = tumult_otel::config::TelemetryConfig::from_env();
    let telemetry = tumult_otel::telemetry::TumultTelemetry::new(otel_config);

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            experiment,
            journal_path,
            dry_run,
            rollback_strategy,
            baseline_mode: _,
            no_ingest,
            output_format,
        } => {
            let strategy = match rollback_strategy {
                RollbackStrategy::Always => tumult_core::execution::RollbackStrategy::Always,
                RollbackStrategy::OnDeviation => {
                    tumult_core::execution::RollbackStrategy::OnDeviation
                }
                RollbackStrategy::Never => tumult_core::execution::RollbackStrategy::Never,
            };
            commands::cmd_run(&experiment, &journal_path, dry_run, strategy, !no_ingest)?;
            // If --output-format json was specified, print the journal as JSON to stdout
            if matches!(output_format, Some(OutputFormat::Json)) {
                if let Ok(content) = std::fs::read_to_string(&journal_path) {
                    if let Ok(journal) =
                        toon_format::decode_default::<tumult_core::types::Journal>(&content)
                    {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&journal)
                                .unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}")),
                        );
                    }
                }
            }
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
        Commands::Analyze { journals, query } => {
            commands::cmd_analyze(journals.as_deref(), query.as_deref())?;
        }
        Commands::Export { journal, format } => {
            let fmt = match format {
                ExportFormat::Parquet => "parquet",
                ExportFormat::Csv => "csv",
                ExportFormat::Json => "json",
            };
            commands::cmd_export(&journal, fmt)?;
        }
        Commands::Compliance {
            journals,
            framework,
        } => {
            let fw = match framework {
                ComplianceFramework::Dora => "DORA",
                ComplianceFramework::Nis2 => "NIS2",
                ComplianceFramework::PciDss => "PCI-DSS",
                ComplianceFramework::Iso22301 => "ISO-22301",
                ComplianceFramework::Iso27001 => "ISO-27001",
                ComplianceFramework::Soc2 => "SOC2",
                ComplianceFramework::BaselIii => "Basel-III",
            };
            commands::cmd_compliance(&journals, fw)?;
        }
        Commands::Trend {
            journals,
            metric,
            last,
            target,
        } => {
            commands::cmd_trend(&journals, &metric, last.as_deref(), target.as_deref())?;
        }
        Commands::Report {
            journal,
            output,
            format,
        } => {
            let fmt = match format {
                ReportFormat::Html => "html",
                ReportFormat::Pdf => "pdf",
            };
            commands::cmd_report(&journal, output.as_deref(), fmt)?;
        }
        Commands::Import { parquet_dir } => {
            commands::cmd_import(&parquet_dir)?;
        }
        Commands::Store { action } => match action {
            StoreAction::Stats => commands::cmd_store_stats()?,
            StoreAction::Backup { output } => commands::cmd_store_backup(&output)?,
            StoreAction::Purge { older_than_days } => commands::cmd_store_purge(older_than_days)?,
            StoreAction::Path => commands::cmd_store_path()?,
            StoreAction::Migrate => commands::cmd_store_migrate()?,
        },
    }

    // Flush OTel spans before exit
    telemetry.shutdown();

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
            output_format,
            ..
        } = cli.command
        else {
            panic!("expected Run command");
        };
        assert_eq!(experiment, PathBuf::from("experiment.toon"));
        assert_eq!(journal_path, PathBuf::from("journal.toon"));
        assert!(!dry_run);
        assert_eq!(rollback_strategy, RollbackStrategy::OnDeviation);
        assert_eq!(baseline_mode, BaselineMode::Full);
        assert!(output_format.is_none());
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
            ..
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
    fn parse_analyze_with_path() {
        let cli = Cli::try_parse_from(["tumult", "analyze", "journals/"]).unwrap();
        let Commands::Analyze { journals, query } = cli.command else {
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
        let Commands::Analyze { journals, query } = cli.command else {
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

    // ── Run with --no-ingest ──────────────────────────────────

    #[test]
    fn parse_run_no_ingest_flag() {
        let cli = Cli::try_parse_from(["tumult", "run", "exp.toon", "--no-ingest"]).unwrap();
        let Commands::Run { no_ingest, .. } = cli.command else {
            panic!("expected Run command");
        };
        assert!(no_ingest);
    }

    #[test]
    fn parse_run_default_ingest_enabled() {
        let cli = Cli::try_parse_from(["tumult", "run", "exp.toon"]).unwrap();
        let Commands::Run { no_ingest, .. } = cli.command else {
            panic!("expected Run command");
        };
        assert!(!no_ingest);
    }

    #[test]
    fn parse_run_output_format_json() {
        let cli =
            Cli::try_parse_from(["tumult", "run", "exp.toon", "--output-format", "json"]).unwrap();
        let Commands::Run { output_format, .. } = cli.command else {
            panic!("expected Run command");
        };
        assert_eq!(output_format, Some(OutputFormat::Json));
    }

    #[test]
    fn parse_run_invalid_output_format_is_error() {
        let result = Cli::try_parse_from(["tumult", "run", "exp.toon", "--output-format", "xml"]);
        assert!(result.is_err());
    }

    // ── Import ────────────────────────────────────────────────

    #[test]
    fn parse_import() {
        let cli = Cli::try_parse_from(["tumult", "import", "backup/"]).unwrap();
        let Commands::Import { parquet_dir } = cli.command else {
            panic!("expected Import command");
        };
        assert_eq!(parquet_dir, PathBuf::from("backup/"));
    }

    #[test]
    fn parse_import_requires_path() {
        let err = Cli::try_parse_from(["tumult", "import"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    // ── Store ─────────────────────────────────────────────────

    #[test]
    fn parse_store_stats() {
        let cli = Cli::try_parse_from(["tumult", "store", "stats"]).unwrap();
        let Commands::Store { action } = cli.command else {
            panic!("expected Store command");
        };
        assert!(matches!(action, StoreAction::Stats));
    }

    #[test]
    fn parse_store_backup_default() {
        let cli = Cli::try_parse_from(["tumult", "store", "backup"]).unwrap();
        let Commands::Store { action } = cli.command else {
            panic!("expected Store command");
        };
        let StoreAction::Backup { output } = action else {
            panic!("expected Backup");
        };
        assert_eq!(output, PathBuf::from("tumult-backup"));
    }

    #[test]
    fn parse_store_backup_custom_output() {
        let cli =
            Cli::try_parse_from(["tumult", "store", "backup", "--output", "my-backup"]).unwrap();
        let Commands::Store { action } = cli.command else {
            panic!("expected Store command");
        };
        let StoreAction::Backup { output } = action else {
            panic!("expected Backup");
        };
        assert_eq!(output, PathBuf::from("my-backup"));
    }

    #[test]
    fn parse_store_purge() {
        let cli =
            Cli::try_parse_from(["tumult", "store", "purge", "--older-than-days", "90"]).unwrap();
        let Commands::Store { action } = cli.command else {
            panic!("expected Store command");
        };
        let StoreAction::Purge { older_than_days } = action else {
            panic!("expected Purge");
        };
        assert_eq!(older_than_days, 90);
    }

    #[test]
    fn parse_store_purge_requires_days() {
        let err = Cli::try_parse_from(["tumult", "store", "purge"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn parse_store_path() {
        let cli = Cli::try_parse_from(["tumult", "store", "path"]).unwrap();
        let Commands::Store { action } = cli.command else {
            panic!("expected Store command");
        };
        assert!(matches!(action, StoreAction::Path));
    }

    #[test]
    fn parse_store_requires_subcommand() {
        let err = Cli::try_parse_from(["tumult", "store"]).unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }
}
