//! The score and the load wait of a game day whose steps are real.
//!
//! Every step is an `sh` process on a target that goes down and heals itself
//! after a number of reads, so the recovery times come from the clock and
//! not from a scripted answer. The same game day through the binary is in
//! `tumult-cli/tests/gameday_run.rs`.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    load_executor_for, run_gameday, ActivityExecutor, ActivityOutcome, RunConfig,
};
use tumult_core::types::*;
use tumult_test_utils::EnvGuard;

/// The target `$1` says `ok` until a fault writes `down` to its state file.
/// Once it is down it heals itself on read number `$2`.
const TARGET: &str = r#"state="$1.state"
test -f "$state" || echo ok > "$state"
cat "$state"
grep -q down "$state" || exit 0
echo read >> "$1.reads"
test "$(wc -l < "$1.reads")" -lt "$2" || echo ok > "$state"
"#;

/// Runs the process of a step in the directory of the lab.
struct InLab(PathBuf);

impl ActivityExecutor for InLab {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let Provider::Process {
            path, arguments, ..
        } = &activity.provider
        else {
            panic!("the steps of this lab are processes");
        };
        let started = Instant::now();
        let output = Command::new(path)
            .args(arguments)
            .current_dir(&self.0)
            .output()
            .expect("the process starts");
        ActivityOutcome {
            success: output.status.success(),
            output: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            error: (!output.status.success()).then(|| format!("{}", output.status)),
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// A step that runs `command` in `sh`.
fn step(name: &str, activity_type: ActivityType, command: &str) -> Activity {
    Activity {
        name: name.into(),
        activity_type,
        provider: Provider::Process {
            path: "sh".into(),
            arguments: vec!["-c".into(), command.into()],
            env: HashMap::new(),
            timeout_s: Some(10.0),
        },
        ..Default::default()
    }
}

/// An experiment on the target `name`, which heals on read number `reads`
/// after it went down. The steady state is a target that says `ok`.
fn experiment(name: &str, method: &[&str], reads: u32) -> Experiment {
    let mut probe = step(
        "steady",
        ActivityType::Probe,
        &format!("sh target.sh {name} {reads}"),
    );
    probe.tolerance = Some(Tolerance::Regex {
        pattern: "^ok$".into(),
    });
    Experiment {
        title: name.into(),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target says ok".into(),
            probes: vec![probe],
        }),
        method: method
            .iter()
            .map(|command| step("step", ActivityType::Action, command))
            .collect(),
        ..Experiment::default()
    }
}

/// Run `experiments` as one game day in a new lab.
fn run(experiments: &[Experiment], load: Option<LoadConfig>, mttr_target_s: f64) -> GameDayJournal {
    let lab = tempfile::TempDir::new().expect("lab directory");
    std::fs::write(lab.path().join("target.sh"), TARGET).expect("target script");
    let gameday = GameDay {
        title: "Lab game day".into(),
        description: None,
        tags: vec![],
        regulatory: None,
        experiments: experiments
            .iter()
            .map(|experiment| GameDayExperiment {
                path: format!("{}.toon", experiment.title).into(),
                compliance_maps: vec![],
            })
            .collect(),
        scoring: ScoringConfig {
            mttr_target_s,
            ..ScoringConfig::default()
        },
        load: load.clone(),
    };
    let executor: Arc<dyn ActivityExecutor> = Arc::new(InLab(lab.path().to_path_buf()));
    let config = RunConfig {
        load_executor: load.map(|load| load_executor_for(&load.tool).expect("load executor")),
        ..RunConfig::default()
    };
    run_gameday(
        &gameday,
        experiments,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &config,
    )
    .expect("the game day runs")
}

#[test]
fn the_recovery_score_is_the_share_of_runs_that_came_back_within_the_target() {
    // A run reads its target once when the method starts. The fault comes
    // half a second later, so every read of a target that is down is a read
    // of the recovery that follows the method.
    let down = |name: &str| ["sleep 0.5".to_string(), format!("echo down > {name}.state")];
    let fast = down("back-fast");
    let slow = down("back-slow");
    let experiments = [
        // The fault changes nothing: the first read after it says ok.
        experiment("stays-up", &["true"], 1),
        // Down on the first read, back on the second, one second later.
        experiment("back-fast", &[&fast[0], &fast[1]], 1),
        // Down on three reads, back on the fourth, three seconds later.
        experiment("back-slow", &[&slow[0], &slow[1]], 3),
    ];

    let journal = run(&experiments, None, 2.0);

    let recoveries: Vec<f64> = journal
        .experiment_journals
        .iter()
        .map(|run| run.recovery_time_s().expect("the target came back"))
        .collect();
    assert!(recoveries[0] < 0.5, "stays-up: {recoveries:?}");
    assert!(
        (0.5..1.5).contains(&recoveries[1]),
        "back-fast: {recoveries:?}"
    );
    assert!(
        (2.5..10.0).contains(&recoveries[2]),
        "back-slow: {recoveries:?}"
    );
    // Two of three runs came back within the two seconds of the target.
    let score = journal.resilience_score;
    assert!(
        (score.recovery_compliance - 2.0 / 3.0).abs() < 1e-9,
        "{score:?}"
    );
    assert!((score.pass_rate - 1.0).abs() < 1e-9, "{score:?}");
}

/// A stand-in for the `oha` binary, which the machines that run these tests
/// do not have: it runs for the time `-z` asks for and prints the summary of
/// 40 answered requests. The adapter, its process and the wait of the game
/// day are the real ones.
const OHA_STAND_IN: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && exit 0
while [ $# -gt 0 ]; do
  if [ "$1" = "-z" ]; then shift; length="$1"; fi
  shift
done
sleep "${length%s}"
printf '%s' '{"summary":{"successRate":1.0,"total":3.0,"slowest":0.2,"fastest":0.01,"average":0.05,"requestsPerSec":13.3,"totalData":1,"sizePerRequest":1,"sizePerSec":1},"responseTimeHistogram":{},"latencyPercentiles":{"p50":0.02,"p95":0.08,"p99":0.1},"rps":{},"details":{},"statusCodeDistribution":{"200":40},"errorDistribution":{}}'
"#;

#[test]
fn a_game_day_waits_for_its_load_and_no_longer() {
    let tools = tempfile::TempDir::new().expect("tools directory");
    let oha = tools.path().join("oha-stand-in.sh");
    tumult_test_utils::write_executable(&oha, OHA_STAND_IN);
    let mut env = EnvGuard::lock();
    env.set("TUMULT_OHA_BINARY", &oha);
    // The one run takes under a second; the load is planned for three.
    let load = LoadConfig {
        tool: LoadTool::Oha,
        script: PathBuf::from("http://127.0.0.1:9/health"),
        vus: Some(2),
        duration_s: Some(3.0),
        thresholds: HashMap::new(),
        profile: None,
    };

    let journal = run(
        &[experiment("stays-up", &["sleep 0.7"], 1)],
        Some(load),
        30.0,
    );

    assert!(
        (3.0..4.0).contains(&journal.duration_s),
        "the game day took {}s for a load of 3s",
        journal.duration_s
    );
    let load = journal.load_result.expect("the load left its result");
    assert_eq!(load.total_requests, 40);
}
