use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "tumult",
    version,
    about = "Rust-native chaos engineering platform"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(clap::Subcommand, Debug)]
pub enum Commands {
    /// Execute a chaos experiment
    Run {
        /// Path to experiment .toon file
        experiment: std::path::PathBuf,
        /// Output journal location
        #[arg(long, default_value = "journal.toon")]
        journal_path: std::path::PathBuf,
        /// Validate and show plan without executing
        #[arg(long)]
        dry_run: bool,
        /// Rollback strategy
        #[arg(long, default_value = "deviated")]
        rollback_strategy: String,
        /// Skip baseline acquisition (use static tolerances)
        #[arg(long)]
        skip_baseline: bool,
        /// Run baseline only, no fault injection
        #[arg(long)]
        baseline_only: bool,
    },
    /// Validate experiment syntax and plugin references
    Validate {
        /// Path to experiment .toon file
        experiment: std::path::PathBuf,
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
        journals: std::path::PathBuf,
        /// SQL query to execute
        #[arg(long)]
        query: Option<String>,
    },
    /// Convert journal to other formats
    Export {
        /// Journal file to export
        journal: std::path::PathBuf,
        /// Output format
        #[arg(long, default_value = "parquet")]
        format: String,
    },
    /// Regulatory compliance report
    Compliance {
        /// Directory containing journal files
        journals: std::path::PathBuf,
        /// Target regulatory framework
        #[arg(long)]
        framework: String,
    },
    /// Generate HTML report from journal
    Report {
        /// Journal file
        journal: std::path::PathBuf,
        /// Output path
        #[arg(long)]
        output: Option<std::path::PathBuf>,
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
    let _cli = Cli::parse();
    // Command dispatch will be implemented as each subcommand is built (TDD per feature)
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_debug_assert() {
        // Validates the clap derive configuration is consistent
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_run_minimal() {
        let cli = Cli::try_parse_from(["tumult", "run", "experiment.toon"]).unwrap();
        match cli.command {
            Commands::Run {
                experiment,
                journal_path,
                dry_run,
                rollback_strategy,
                skip_baseline,
                baseline_only,
            } => {
                assert_eq!(experiment.to_str().unwrap(), "experiment.toon");
                assert_eq!(journal_path.to_str().unwrap(), "journal.toon");
                assert!(!dry_run);
                assert_eq!(rollback_strategy, "deviated");
                assert!(!skip_baseline);
                assert!(!baseline_only);
            }
            _ => panic!("expected Run command"),
        }
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
            "--skip-baseline",
            "--baseline-only",
        ])
        .unwrap();
        match cli.command {
            Commands::Run {
                experiment,
                journal_path,
                dry_run,
                rollback_strategy,
                skip_baseline,
                baseline_only,
            } => {
                assert_eq!(experiment.to_str().unwrap(), "my-exp.toon");
                assert_eq!(journal_path.to_str().unwrap(), "out.toon");
                assert!(dry_run);
                assert_eq!(rollback_strategy, "always");
                assert!(skip_baseline);
                assert!(baseline_only);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_requires_experiment_path() {
        let result = Cli::try_parse_from(["tumult", "run"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_validate() {
        let cli = Cli::try_parse_from(["tumult", "validate", "test.toon"]).unwrap();
        match cli.command {
            Commands::Validate { experiment } => {
                assert_eq!(experiment.to_str().unwrap(), "test.toon");
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_validate_requires_path() {
        let result = Cli::try_parse_from(["tumult", "validate"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_discover_no_args() {
        let cli = Cli::try_parse_from(["tumult", "discover"]).unwrap();
        match cli.command {
            Commands::Discover { plugin } => {
                assert!(plugin.is_none());
            }
            _ => panic!("expected Discover command"),
        }
    }

    #[test]
    fn parse_discover_with_plugin() {
        let cli = Cli::try_parse_from(["tumult", "discover", "--plugin", "tumult-kafka"]).unwrap();
        match cli.command {
            Commands::Discover { plugin } => {
                assert_eq!(plugin.unwrap(), "tumult-kafka");
            }
            _ => panic!("expected Discover command"),
        }
    }

    #[test]
    fn parse_analyze_minimal() {
        let cli = Cli::try_parse_from(["tumult", "analyze", "journals/"]).unwrap();
        match cli.command {
            Commands::Analyze { journals, query } => {
                assert_eq!(journals.to_str().unwrap(), "journals/");
                assert!(query.is_none());
            }
            _ => panic!("expected Analyze command"),
        }
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
        match cli.command {
            Commands::Analyze { query, .. } => {
                assert_eq!(query.unwrap(), "SELECT * FROM experiments");
            }
            _ => panic!("expected Analyze command"),
        }
    }

    #[test]
    fn parse_export_defaults_to_parquet() {
        let cli = Cli::try_parse_from(["tumult", "export", "journal.toon"]).unwrap();
        match cli.command {
            Commands::Export { journal, format } => {
                assert_eq!(journal.to_str().unwrap(), "journal.toon");
                assert_eq!(format, "parquet");
            }
            _ => panic!("expected Export command"),
        }
    }

    #[test]
    fn parse_export_custom_format() {
        let cli =
            Cli::try_parse_from(["tumult", "export", "journal.toon", "--format", "csv"]).unwrap();
        match cli.command {
            Commands::Export { format, .. } => {
                assert_eq!(format, "csv");
            }
            _ => panic!("expected Export command"),
        }
    }

    #[test]
    fn parse_compliance() {
        let cli = Cli::try_parse_from(["tumult", "compliance", "journals/", "--framework", "DORA"])
            .unwrap();
        match cli.command {
            Commands::Compliance {
                journals,
                framework,
            } => {
                assert_eq!(journals.to_str().unwrap(), "journals/");
                assert_eq!(framework, "DORA");
            }
            _ => panic!("expected Compliance command"),
        }
    }

    #[test]
    fn parse_compliance_requires_framework() {
        let result = Cli::try_parse_from(["tumult", "compliance", "journals/"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_report_minimal() {
        let cli = Cli::try_parse_from(["tumult", "report", "journal.toon"]).unwrap();
        match cli.command {
            Commands::Report { journal, output } => {
                assert_eq!(journal.to_str().unwrap(), "journal.toon");
                assert!(output.is_none());
            }
            _ => panic!("expected Report command"),
        }
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
        match cli.command {
            Commands::Report { output, .. } => {
                assert_eq!(output.unwrap().to_str().unwrap(), "report.html");
            }
            _ => panic!("expected Report command"),
        }
    }

    #[test]
    fn parse_init_no_args() {
        let cli = Cli::try_parse_from(["tumult", "init"]).unwrap();
        match cli.command {
            Commands::Init { plugin } => {
                assert!(plugin.is_none());
            }
            _ => panic!("expected Init command"),
        }
    }

    #[test]
    fn parse_init_with_plugin() {
        let cli = Cli::try_parse_from(["tumult", "init", "--plugin", "tumult-db"]).unwrap();
        match cli.command {
            Commands::Init { plugin } => {
                assert_eq!(plugin.unwrap(), "tumult-db");
            }
            _ => panic!("expected Init command"),
        }
    }

    #[test]
    fn no_subcommand_is_error() {
        let result = Cli::try_parse_from(["tumult"]);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_subcommand_is_error() {
        let result = Cli::try_parse_from(["tumult", "destroy"]);
        assert!(result.is_err());
    }
}
