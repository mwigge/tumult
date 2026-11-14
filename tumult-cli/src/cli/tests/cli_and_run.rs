//! CLI configuration and `run` subcommand parser tests.

use crate::cli::*;

use clap::CommandFactory;
use clap::Parser;
use std::path::PathBuf;

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
        Cli::try_parse_from(["tumult", "run", "exp.toon", "--rollback-strategy", "never"]).unwrap();
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
    let result = Cli::try_parse_from(["tumult", "run", "exp.toon", "--rollback-strategy", "maybe"]);
    assert!(result.is_err());
}

#[test]
fn parse_run_invalid_baseline_mode_is_error() {
    let result = Cli::try_parse_from(["tumult", "run", "exp.toon", "--baseline-mode", "partial"]);
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
    let cli = Cli::try_parse_from(["tumult", "run", "path with spaces/experiment.toon"]).unwrap();
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
    let cli = Cli::try_parse_from(["tumult", "run", "experiments/résilience-test.toon"]).unwrap();
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

// ── Perf gate ─────────────────────────────────────────────

#[test]
fn perf_fails_on_regressed_and_inconclusive_unless_told_otherwise() {
    let cli = Cli::try_parse_from(["tumult", "perf", "checkout"]).unwrap();
    let Commands::Perf {
        fail_on,
        range,
        include,
        ..
    } = cli.command
    else {
        panic!("expected Perf command");
    };
    assert_eq!(
        fail_on,
        vec![PerfFailOnArg::Regressed, PerfFailOnArg::Inconclusive]
    );
    assert_eq!(range, None);
    assert_eq!(include, None);

    let cli = Cli::try_parse_from(["tumult", "perf", "checkout", "--fail-on", "regressed"]);
    let Commands::Perf { fail_on, .. } = cli.unwrap().command else {
        panic!("expected Perf command");
    };
    assert_eq!(fail_on, vec![PerfFailOnArg::Regressed]);
}

#[test]
fn perf_takes_a_range_and_can_include_partial_runs() {
    let cli = Cli::try_parse_from([
        "tumult",
        "perf",
        "checkout",
        "--range",
        "60d",
        "--include",
        "partial",
        "--fail-on",
        "regressed,inconclusive",
    ])
    .unwrap();
    let Commands::Perf {
        fail_on,
        range,
        include,
        ..
    } = cli.command
    else {
        panic!("expected Perf command");
    };
    assert_eq!(range.as_deref(), Some("60d"));
    assert_eq!(include, Some(PerfIncludeArg::Partial));
    assert_eq!(
        fail_on,
        vec![PerfFailOnArg::Regressed, PerfFailOnArg::Inconclusive]
    );
    assert!(Cli::try_parse_from(["tumult", "perf", "checkout", "--include", "all"]).is_err());
}

#[test]
fn perf_takes_between_1_and_1000_minimum_runs() {
    let min_runs = |value: &str| {
        Cli::try_parse_from(["tumult", "perf", "checkout", "--min-runs", value]).map(|cli| {
            let Commands::Perf { min_runs, .. } = cli.command else {
                panic!("expected Perf command");
            };
            min_runs
        })
    };
    assert_eq!(min_runs("1").unwrap(), Some(1));
    assert_eq!(min_runs("1000").unwrap(), Some(1000));
    for refused in ["0", "1001", "-1", "many"] {
        assert!(min_runs(refused).is_err(), "{refused}");
    }
}
