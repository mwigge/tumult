//! The score and the load wait of a game day whose steps are real.
//!
//! Every step is an `sh` process on a target that goes down and heals itself
//! after a number of reads, so the recovery times come from the clock and
//! not from a scripted answer. The same game day through the binary is in
//! `tumult-cli/tests/gameday_run.rs`.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
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

/// Runs the process of a step in the directory of the lab. It spawns
/// through `tumult_core::spawn::alone`, as the process steps do.
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
        let mut command = Command::new(path);
        command
            .args(arguments)
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = tumult_core::spawn::alone(|| command.spawn())
            .expect("the process starts")
            .wait_with_output()
            .expect("the process ends");
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
    run_in(lab.path(), experiments, load, mttr_target_s, None)
}

/// Run `experiments` as one game day in `lab`. `token` cancels it.
fn run_in(
    lab: &Path,
    experiments: &[Experiment],
    load: Option<LoadConfig>,
    mttr_target_s: f64,
    token: Option<CancellationToken>,
) -> GameDayJournal {
    std::fs::write(lab.join("target.sh"), TARGET).expect("target script");
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
    let executor: Arc<dyn ActivityExecutor> = Arc::new(InLab(lab.to_path_buf()));
    let config = RunConfig {
        load_executor: load.map(|load| load_executor_for(&load.tool).expect("load executor")),
        cancellation_token: token,
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
        // The fault changes nothing: no read sees an outage, so the run
        // measures no recovery.
        experiment("stays-up", &["true"], 1),
        // Down on the first read, back on the second, one second later.
        experiment("back-fast", &[&fast[0], &fast[1]], 1),
        // Down on three reads, back on the fourth, three seconds later.
        experiment("back-slow", &[&slow[0], &slow[1]], 3),
    ];

    let journal = run(&experiments, None, 2.0);

    let recoveries: Vec<Option<f64>> = journal
        .experiment_journals
        .iter()
        .map(Journal::recovery_time_s)
        .collect();
    assert_eq!(recoveries[0], None, "stays-up: {recoveries:?}");
    assert!(
        recoveries[1].is_some_and(|s| (0.5..1.5).contains(&s)),
        "back-fast: {recoveries:?}"
    );
    assert!(
        recoveries[2].is_some_and(|s| (2.5..10.0).contains(&s)),
        "back-slow: {recoveries:?}"
    );
    // Of the two runs that saw an outage, one came back within the two
    // seconds of the target. Stays-up saw nothing to recover from, so it
    // is not in the share.
    let score = journal.resilience_score;
    assert!((score.recovery_compliance - 0.5).abs() < 1e-9, "{score:?}");
    assert!((score.pass_rate - 1.0).abs() < 1e-9, "{score:?}");
}

/// A stand-in for the `oha` binary, which the machines that run these tests
/// do not have: it runs for the time `-z` asks for and prints the summary of
/// 40 answered requests. As `oha` does, it takes a time in `s` or in `ms`,
/// and prints that summary at once when it is interrupted. The adapter, its
/// process and the wait of the game day are the real ones.
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
case "$length" in
  *ms) sleep "$(awk "BEGIN { print ${length%ms} / 1000 }")" ;;
  *) sleep "${length%s}" ;;
esac
summary
"#;

/// Put the stand-in in the place of `oha` for as long as the result lives.
fn oha_stand_in() -> (tempfile::TempDir, EnvGuard) {
    let tools = tempfile::TempDir::new().expect("tools directory");
    let oha = tools.path().join("oha-stand-in.sh");
    tumult_test_utils::write_executable(&oha, OHA_STAND_IN);
    let mut env = EnvGuard::lock();
    env.set("TUMULT_OHA_BINARY", &oha);
    (tools, env)
}

/// A load of two users for `duration_s`.
fn load(duration_s: f64, profile: Option<LoadProfile>) -> LoadConfig {
    LoadConfig {
        tool: LoadTool::Oha,
        script: PathBuf::from("http://127.0.0.1:9/health"),
        vus: Some(2),
        duration_s: Some(duration_s),
        thresholds: HashMap::new(),
        profile,
    }
}

#[test]
fn a_game_day_waits_for_its_load_and_no_longer() {
    let _oha = oha_stand_in();
    // The one run takes under a second. The load is planned for three: a
    // second of two users, a second of four, a second of two again. Each
    // second is a window of its own, and a load that is stopped starts no
    // further window.
    let spike = LoadProfile::Spike {
        peak_vus: 4,
        after_s: 1.0,
        hold_s: 1.0,
    };

    let journal = run(
        &[experiment("stays-up", &["sleep 0.7"], 1)],
        Some(load(3.0, Some(spike))),
        30.0,
    );

    assert!(
        (3.0..4.0).contains(&journal.duration_s),
        "the game day took {}s for a load of 3s",
        journal.duration_s
    );
    let load = journal.load_result.expect("the load left its result");
    assert_eq!(load.execution.planned_windows, Some(3));
    assert_eq!(load.execution.completed_windows, Some(3));
    assert!(!load.execution.aborted, "{:?}", load.execution);
    assert_eq!(load.total_requests, 120);
}

/// The game day ended long before its load of 30 seconds would have. The
/// load says that it was cut, and what it had measured by then.
fn assert_the_load_was_cut(journal: &GameDayJournal) {
    assert!(
        journal.duration_s < 10.0,
        "the game day took {}s",
        journal.duration_s
    );
    let load = journal
        .load_result
        .as_ref()
        .expect("the load left its result");
    assert!(load.execution.aborted, "{:?}", load.execution);
    assert_eq!(load.total_requests, 40);
}

#[test]
fn a_game_day_stopped_by_an_error_cuts_its_load_and_keeps_what_it_measured() {
    let _oha = oha_stand_in();
    // The second run has no method, which is an error of the run. It stops
    // the game day when the load has more than 29 seconds left.
    let experiments = [
        experiment("stays-up", &["sleep 0.5"], 1),
        experiment("no-method", &[], 1),
    ];

    let journal = run(&experiments, Some(load(30.0, None)), 30.0);

    let ran: Vec<(&str, &ExperimentStatus)> = journal
        .experiment_journals
        .iter()
        .map(|run| (run.experiment_title.as_str(), &run.status))
        .collect();
    assert_eq!(ran, [("stays-up", &ExperimentStatus::Completed)]);
    assert_the_load_was_cut(&journal);
}

#[test]
fn a_game_day_cancelled_before_it_starts_runs_no_step_and_cuts_its_load() {
    let _oha = oha_stand_in();
    let lab = tempfile::TempDir::new().expect("lab directory");
    let token = CancellationToken::new();
    token.cancel();
    let experiments = [
        experiment("first", &["touch ran"], 1),
        experiment("second", &["touch ran"], 1),
    ];

    let journal = run_in(
        lab.path(),
        &experiments,
        Some(load(30.0, None)),
        30.0,
        Some(token),
    );

    assert!(
        journal.experiment_journals.is_empty(),
        "{} runs started",
        journal.experiment_journals.len()
    );
    assert!(!lab.path().join("ran").exists(), "a step ran");
    assert!(
        journal.duration_s < 10.0,
        "the game day took {}s",
        journal.duration_s
    );
    // The load is cut as it starts, so it may have measured nothing yet.
    let load = journal.load_result.expect("the load left its result");
    assert!(load.execution.aborted, "{:?}", load.execution);
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{} never came", path.display());
        // sleep: interval of the bounded poll for a file of another process.
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_cancelled_game_day_cuts_its_load_and_keeps_what_it_measured() {
    let _oha = oha_stand_in();
    let lab = tempfile::TempDir::new().expect("lab directory");
    let token = CancellationToken::new();
    // The game day is cancelled while its one step runs.
    let canceller = {
        let started = lab.path().join("started");
        let token = token.clone();
        std::thread::spawn(move || {
            wait_for(&started);
            token.cancel();
        })
    };

    let journal = run_in(
        lab.path(),
        &[experiment("cancelled", &["touch started; sleep 1"], 1)],
        Some(load(30.0, None)),
        30.0,
        Some(token),
    );

    canceller.join().expect("the canceller");
    let ended: Vec<&ExperimentStatus> = journal
        .experiment_journals
        .iter()
        .map(|run| &run.status)
        .collect();
    assert_eq!(ended, [&ExperimentStatus::Interrupted]);
    assert_the_load_was_cut(&journal);
}

/// On macOS a load window that starts while a step makes its pipes keeps a
/// copy of them, and the step waits for the load to end. So the lab starts
/// a step only when no other spawn runs, as the process steps do.
#[test]
fn a_step_of_the_lab_starts_only_when_no_other_spawn_runs() {
    // The load tests take this lock too, so none of their windows waits
    // while this test holds the spawn lock.
    let _env = EnvGuard::lock();
    let lab = tempfile::TempDir::new().expect("lab directory");
    let started = lab.path().join("started");
    let in_lab = InLab(lab.path().to_path_buf());
    let (ready, is_ready) = std::sync::mpsc::channel();

    let ran = tumult_core::spawn::alone(|| {
        let ran = std::thread::spawn(move || {
            ready.send(()).expect("the test waits for the step");
            in_lab.execute(&step("start", ActivityType::Action, "touch started"))
        });
        is_ready.recv().expect("the step is about to start");
        // sleep: there is no event for a file that must stay absent; wait
        // past the time the step would need to start.
        std::thread::sleep(Duration::from_millis(100));
        assert!(!started.exists(), "the step started during another spawn");
        ran
    });

    let outcome = ran.join().expect("the step ends");
    assert!(outcome.success, "{:?}", outcome.error);
    assert!(started.exists(), "the step did not run");
}
