//! Parser tests for the `agentic`, `import`, and `store` subcommands.

use crate::cli::*;

use clap::Parser;
use std::path::PathBuf;

// ── Agentic ──────────────────────────────────────────────

#[test]
fn parse_agentic_list_packs() {
    let cli = Cli::try_parse_from(["tumult", "agentic", "list-packs"]).unwrap();
    let Commands::Agentic { action } = cli.command else {
        panic!("expected Agentic command");
    };
    assert!(matches!(action, AgenticAction::ListPacks));
}

#[test]
fn parse_agentic_smoke() {
    let cli = Cli::try_parse_from(["tumult", "agentic", "smoke"]).unwrap();
    let Commands::Agentic { action } = cli.command else {
        panic!("expected Agentic command");
    };
    let AgenticAction::Smoke { journal } = action else {
        panic!("expected Agentic smoke action");
    };
    assert_eq!(journal, PathBuf::from("target/agentic/smoke-journal.toon"));
}

#[test]
fn parse_agentic_run() {
    let cli = Cli::try_parse_from([
        "tumult",
        "agentic",
        "run",
        "--scenario",
        "tool-timeout-fallback",
        "--journal",
        "target/agentic/tool.toon",
    ])
    .unwrap();
    let Commands::Agentic { action } = cli.command else {
        panic!("expected Agentic command");
    };
    let AgenticAction::Run { scenario, journal } = action else {
        panic!("expected Agentic run action");
    };
    assert_eq!(scenario, "tool-timeout-fallback");
    assert_eq!(journal, PathBuf::from("target/agentic/tool.toon"));
}

#[test]
fn parse_agentic_trajectory() {
    let cli = Cli::try_parse_from([
        "tumult",
        "agentic",
        "trajectory",
        "--pack",
        "rag-grounding-failure",
        "--journal",
        "target/agentic/traj.toon",
    ])
    .unwrap();
    let Commands::Agentic { action } = cli.command else {
        panic!("expected Agentic command");
    };
    let AgenticAction::Trajectory { pack, journal } = action else {
        panic!("expected Agentic trajectory action");
    };
    assert_eq!(pack, "rag-grounding-failure");
    assert_eq!(journal, PathBuf::from("target/agentic/traj.toon"));
}

#[test]
fn parse_agentic_replay() {
    let cli = Cli::try_parse_from([
        "tumult",
        "agentic",
        "replay",
        "--fixture",
        "examples/agentic/malformed-json-recovery.fixture.json",
        "--journal",
        "target/agentic/replay.toon",
    ])
    .unwrap();
    let Commands::Agentic { action } = cli.command else {
        panic!("expected Agentic command");
    };
    let AgenticAction::Replay { fixture, journal } = action else {
        panic!("expected Agentic replay action");
    };
    assert_eq!(
        fixture,
        PathBuf::from("examples/agentic/malformed-json-recovery.fixture.json")
    );
    assert_eq!(journal, PathBuf::from("target/agentic/replay.toon"));
}

#[test]
fn parse_agentic_requires_subcommand() {
    let err = Cli::try_parse_from(["tumult", "agentic"]).unwrap_err();
    assert_eq!(
        err.kind(),
        clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
}

#[test]
fn parse_agentic_proxy() {
    let cli = Cli::try_parse_from([
        "tumult",
        "agentic",
        "proxy",
        "--listen",
        "127.0.0.1:9090",
        "--upstream",
        "https://api.openai.com",
        "--scenario",
        "concurrency-storm",
        "--journal",
        "target/agentic/proxy.jsonl",
        "--seed",
        "7",
        "--client",
        "codex",
    ])
    .unwrap();
    let Commands::Agentic { action } = cli.command else {
        panic!("expected Agentic command");
    };
    let AgenticAction::Proxy {
        listen,
        upstream,
        scenario,
        journal,
        seed,
        client,
    } = action
    else {
        panic!("expected Agentic proxy action");
    };
    assert_eq!(listen, "127.0.0.1:9090");
    assert_eq!(upstream, "https://api.openai.com");
    assert_eq!(scenario, "concurrency-storm");
    assert_eq!(journal, Some(PathBuf::from("target/agentic/proxy.jsonl")));
    assert_eq!(seed, 7);
    assert_eq!(client, "codex");
}

#[test]
fn parse_agentic_proxy_defaults() {
    let cli = Cli::try_parse_from(["tumult", "agentic", "proxy"]).unwrap();
    let Commands::Agentic { action } = cli.command else {
        panic!("expected Agentic command");
    };
    let AgenticAction::Proxy {
        listen,
        upstream,
        scenario,
        journal,
        seed,
        client,
    } = action
    else {
        panic!("expected Agentic proxy action");
    };
    assert_eq!(listen, "127.0.0.1:8080");
    assert_eq!(upstream, "https://api.anthropic.com");
    assert_eq!(scenario, "malformed-json-recovery");
    assert_eq!(journal, None);
    assert_eq!(seed, 1);
    assert_eq!(client, "unknown");
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
    let cli = Cli::try_parse_from(["tumult", "store", "backup", "--output", "my-backup"]).unwrap();
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
    let cli = Cli::try_parse_from(["tumult", "store", "purge", "--older-than-days", "90"]).unwrap();
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
