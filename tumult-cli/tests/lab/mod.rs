//! A real target for tests of the `tumult` binary.
//!
//! The target is a directory with a state file that says `ok` or `down`.
//! The steps of an experiment are real `sh` commands that run in that
//! directory: the fault writes `down`, a probe reads the file with `cat`,
//! and the rollback leaves a marker file.

// Each test binary uses its own part of this module.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use tumult_core::journal::read_journal;
use tumult_core::types::{ActivityResult, Journal};

/// A stand-in for the `oha` binary, which the machines that run these tests
/// do not have: it runs for the time `-z` asks for and prints the summary of
/// 40 answered requests. As `oha` does, it takes a time in `s` or in `ms`,
/// and prints that summary at once when it is interrupted. It leaves its
/// process id in `load.pid`. The adapter and its process are the real ones.
const OHA_STAND_IN: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && exit 0
while [ $# -gt 0 ]; do
  if [ "$1" = "-z" ]; then shift; length="$1"; fi
  shift
done
summary() {
  printf '%s' '{"summary":{"successRate":1.0,"total":3.0,"slowest":0.2,"fastest":0.01,"average":0.05,"requestsPerSec":13.3,"totalData":1,"sizePerRequest":1,"sizePerSec":1},"responseTimeHistogram":{},"latencyPercentiles":{"p50":0.02,"p95":0.08,"p99":0.1},"rps":{},"details":{},"statusCodeDistribution":{"200":40},"errorDistribution":{}}'
}
trap 'summary; exit 0' INT
echo $$ > load.pid
case "$length" in
  *ms) sleep "$(awk "BEGIN { print ${length%ms} / 1000 }")" ;;
  *) sleep "${length%s}" ;;
esac
summary
"#;

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

    /// Put the stand-in for `oha` in the lab; its path, for
    /// `TUMULT_OHA_BINARY`.
    pub fn oha_stand_in(&self) -> PathBuf {
        let oha = self.path("oha-stand-in.sh");
        tumult_test_utils::write_executable(&oha, OHA_STAND_IN);
        oha
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

/// `definition` with `block` among the keys of the file, before the steady
/// state.
pub fn with_block(definition: &str, block: &str) -> String {
    let steady = "steady_state_hypothesis:";
    assert!(definition.contains(steady), "{definition}");
    definition.replacen(steady, &format!("{block}\n{steady}"), 1)
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

/// The error the binary ended with: the `error.message` of its last log
/// line. Empty when it logged nothing.
pub fn fatal_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let Some(last) = text.lines().last() else {
        return String::new();
    };
    let log: serde_json::Value =
        serde_json::from_str(last).unwrap_or_else(|e| panic!("no log line ({e}): {text}"));
    log["error.message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Wait until another process has left the file `path`.
pub fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{} never came", path.display());
        // sleep: interval of the bounded poll for a file of another process.
        std::thread::sleep(Duration::from_millis(20));
    }
}
