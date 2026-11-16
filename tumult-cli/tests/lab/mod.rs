//! A real target for tests of the `tumult` binary.
//!
//! The target is a directory with a state file that says `ok` or `down`.
//! The steps of an experiment are real `sh` commands that run in that
//! directory: the fault writes `down`, a probe reads the file with `cat`,
//! and the rollback leaves a marker file.

// Each test binary uses its own part of this module.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};

use tumult_core::journal::read_journal;
use tumult_core::types::{ActivityResult, Journal};

/// A directory with the state file of the target.
pub struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    /// A target whose state file says `state`.
    pub fn with_state(state: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("state"), format!("{state}\n")).expect("state file");
        Self { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `tumult` with `args`, a clean environment and a private store.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tumult"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env("HOME", self.dir.path())
            .env("TUMULT_LAKE_PATH", self.path("lake.duckdb"))
            .env_remove("RUST_LOG")
            .env_remove("TUMULT_LOG_FORMAT")
            .env_remove("TUMULT_ENVIRONMENT")
            .env_remove("TUMULT_DAEMON_URL")
            .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT");
        command
    }

    pub fn tumult(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run tumult")
    }

    /// Write `definition` as the experiment of the lab; the command that
    /// runs it and writes `journal.toon`.
    pub fn run_command(&self, definition: &str) -> Command {
        std::fs::write(self.path("experiment.toon"), definition).expect("experiment");
        self.command(&[
            "run",
            "experiment.toon",
            "--journal-path",
            "journal.toon",
            "--no-ingest",
        ])
    }

    /// The journal the run behind `output` wrote.
    pub fn journal(&self, output: &Output) -> Journal {
        read_journal(&self.path("journal.toon")).unwrap_or_else(|e| {
            panic!(
                "no journal ({e}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
    }

    /// Run an experiment whose method is `method` and whose rollback leaves
    /// a marker; the exit code and the journal.
    pub fn run(&self, method: &str) -> (Option<i32>, Journal) {
        self.run_with(method, &leave_marker())
    }

    /// Run an experiment with this method and these rollbacks.
    pub fn run_with(&self, method: &str, rollbacks: &str) -> (Option<i32>, Journal) {
        let output = self
            .run_command(&experiment(method, rollbacks))
            .output()
            .expect("run tumult");
        (output.status.code(), self.journal(&output))
    }

    pub fn rolled_back(&self) -> bool {
        self.path("rolled-back").exists()
    }
}

/// A step that runs `command` in a shell, for five seconds at most.
pub fn step(name: &str, activity_type: &str, command: &str) -> String {
    step_within(name, activity_type, command, 5.0)
}

/// A step that runs `command` in a shell, for `timeout_s` at most.
pub fn step_within(name: &str, activity_type: &str, command: &str, timeout_s: f64) -> String {
    format!(
        r#"  - name: {name}
    activity_type: {activity_type}
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "{command}"
      timeout_s: {timeout_s:.1}
"#
    )
}

/// A probe that reads the state file and must read `expected`.
pub fn reads(name: &str, expected: &str) -> String {
    format!(
        r#"{}    tolerance:
      type: regex
      pattern: "^{expected}$"
"#,
        step(name, "probe", "cat state")
    )
}

/// `block`, two spaces deeper.
pub fn indented(block: &str) -> String {
    let lines: Vec<String> = block.lines().map(|line| format!("  {line}")).collect();
    lines.join("\n") + "\n"
}

/// How many steps a block of steps holds.
pub fn count(steps: &str) -> usize {
    steps.matches("  - name:").count()
}

/// The steady state is `ok` in the state file.
pub fn experiment(method: &str, rollbacks: &str) -> String {
    format!(
        r"title: Probe tolerance
description: The fault writes down to a state file and the probes read it

rto_target_s: 30.0

steady_state_hypothesis:
  title: The target says ok
  probes[1]:
{steady}
method[{steps}]:
{method}
rollbacks[{undo}]:
{rollbacks}",
        steady = indented(&reads("steady", "ok")),
        steps = count(method),
        undo = count(rollbacks),
    )
}

pub fn leave_marker() -> String {
    step("leave-marker", "action", "touch rolled-back")
}

pub fn inject() -> String {
    step("inject", "action", "echo down > state")
}

pub fn heal() -> String {
    step("heal", "action", "echo ok > state")
}

pub fn result(results: &[ActivityResult], name: &str) -> ActivityResult {
    results
        .iter()
        .find(|result| result.name == name)
        .cloned()
        .unwrap_or_else(|| panic!("no step {name}"))
}
