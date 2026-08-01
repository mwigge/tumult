//! `gameday` subcommand arguments.

use std::path::PathBuf;

use super::{ComplianceFramework, LoadToolArg};

// GameDay is a product name rendered verbatim in `--help`; backticks would
// leak into the help text, so silence the doc-markdown lint for this enum.
#[allow(clippy::doc_markdown)]
#[derive(clap::Subcommand, Debug)]
pub(crate) enum GameDayAction {
    /// Create a `.gameday.toon` file from experiment paths
    Create {
        /// Name for the GameDay
        name: String,
        /// Experiment `.toon` files (comma-separated)
        #[arg(long, value_delimiter = ',')]
        experiments: Vec<PathBuf>,
        /// Load tool to run during the GameDay
        #[arg(long, value_enum)]
        load: Option<LoadToolArg>,
        /// Path to load test script
        #[arg(long)]
        load_script: Option<PathBuf>,
        /// Number of virtual users
        #[arg(long)]
        load_vus: Option<u32>,
        /// Compliance framework to map
        #[arg(long)]
        framework: Option<ComplianceFramework>,
    },
    /// Run all experiments in a GameDay under shared load
    Run {
        /// Path to `.gameday.toon` file
        gameday: PathBuf,
    },
    /// Show aggregate analysis of a completed GameDay
    Analyze {
        /// Path to `.gameday.toon` file (uses journals from same directory)
        gameday: PathBuf,
    },
}
