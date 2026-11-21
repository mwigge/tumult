//! What `tumult gameday run` scores.
//!
//! The game days here are real: the binary runs each experiment with `sh`
//! steps on a target that goes down and heals itself after a number of
//! reads. The score is read from what the binary prints and from the
//! journal it writes.
#![cfg(unix)]

mod lab;

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lab::{count, fatal_error, indented, step, step_within, wait_for, Lab};
use serde_json::Value;
use tumult_core::types::{ExperimentStatus, GameDayJournal};

/// The target `$1` says `ok` until a fault writes `down` to its state file.
/// Once it is down it heals itself on read number `$2`.
const TARGET: &str = r#"state="$1.state"
test -f "$state" || echo ok > "$state"
cat "$state"
grep -q down "$state" || exit 0
echo read >> "$1.reads"
test "$(wc -l < "$1.reads")" -lt "$2" || echo ok > "$state"
"#;

/// An experiment on the target `name`: `method` is its method, and the
/// steady state is a target that says `ok`. The target heals on read
/// number `reads` after it went down.
fn experiment(name: &str, method: &str, reads: u32) -> String {
    let probe = format!(
        "{}    tolerance:\n      type: regex\n      pattern: \"^ok$\"\n",
        step("steady", "probe", &format!("sh target.sh {name} {reads}"))
    );
    format!(
        "title: {name}
description: The target {name} heals on read {reads} after it went down

steady_state_hypothesis:
  title: The target says ok
  probes[1]:
{steady}
method[{steps}]:
{method}",
        steady = indented(&probe),
        steps = count(method),
    )
}

/// Write the game day `lab.gameday.toon` of these experiments, with
/// `extra` as further top-level blocks, and the target script.
fn write_gameday(lab: &Lab, experiments: &[(&str, String)], extra: &str) {
    std::fs::write(lab.path("target.sh"), TARGET).expect("target script");
    for (name, definition) in experiments {
        std::fs::write(lab.path(&format!("{name}.toon")), definition).expect("experiment");
    }
    let paths: Vec<String> = experiments
        .iter()
        .map(|(name, _)| format!("  - path: {name}.toon"))
        .collect();
    let gameday = format!(
        "title: Lab game day
description: Real targets, one of them slow to come back

experiments[{count}]:
{paths}

{extra}",
        count = experiments.len(),
        paths = paths.join("\n"),
    );
    std::fs::write(lab.path("lab.gameday.toon"), gameday).expect("game day");
}

/// What `tumult gameday <action> lab.gameday.toon` prints.
fn gameday(lab: &Lab, action: &str, envs: &[(&str, &str)]) -> String {
    let output = lab
        .command(&["gameday", action, "lab.gameday.toon"])
        .envs(envs.iter().copied())
        .output()
        .expect("run tumult");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// The journal the game day wrote next to its file.
fn journal(lab: &Lab) -> GameDayJournal {
    let toon = std::fs::read_to_string(lab.path("lab.gameday.journal.toon")).expect("journal");
    toon_format::decode_default(&toon).expect("the journal is read")
}

/// A method that takes the target `name` down. The run reads its target
/// once when the method starts. The fault comes half a second later, so
/// every read of a target that is down is a read of the recovery that
/// follows the method.
fn down(name: &str) -> String {
    [
        step("settle", "action", "sleep 0.5"),
        step("fault", "action", &format!("echo down > {name}.state")),
    ]
    .concat()
}

/// A game day passes when two thirds of its runs come back in two seconds.
const SCORING: &str =
    "scoring:\n  pass_threshold: 0.75\n  mttr_target_s: 2.0\n  recovery_required: true\n";

#[test]
fn the_recovery_score_is_the_share_of_runs_that_came_back_within_the_target() {
    let lab = Lab::with_state("ok");
    write_gameday(
        &lab,
        &[
            // The fault changes nothing: the first read after it says ok.
            (
                "stays-up",
                experiment("stays-up", &step("fault", "action", "true"), 1),
            ),
            // Down on the first read, back on the second, one second later.
            ("back-fast", experiment("back-fast", &down("back-fast"), 1)),
            // Down on three reads, back on the fourth, three seconds later.
            ("back-slow", experiment("back-slow", &down("back-slow"), 3)),
        ],
        SCORING,
    );

    let printed = gameday(&lab, "run", &[]);

    let journal = journal(&lab);
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
    let score = &journal.resilience_score;
    assert!(
        (score.recovery_compliance - 2.0 / 3.0).abs() < 1e-9,
        "{score:?}"
    );
    // 0.30 pass rate + 0.25 * 2/3 recovery + 0.25 load + 0.20 compliance.
    assert!(
        (score.overall - (0.75 + 0.25 * 2.0 / 3.0)).abs() < 1e-9,
        "{score:?}"
    );
    assert_eq!(journal.compliance_status, "COMPLIANT");
    for line in [
        "Status:  3/3 PASS (COMPLIANT)\n",
        "\nGameDay score: 0.92\n",
        "  Pass rate:        1.00\n",
        "  Recovery:         0.67\n",
        "  Load impact:      1.00\n",
        "  Compliance:       1.00\n",
    ] {
        assert!(printed.contains(line), "no {line:?} in:\n{printed}");
    }

    let analysed = gameday(&lab, "analyze", &[]);
    assert!(
        analysed.contains(
            "\nGameDay score: 0.92\n  \
             Pass rate:    1.00  Recovery: 0.67  Load: 1.00  Compliance: 1.00\n  \
             Status: COMPLIANT\n"
        ),
        "{analysed}"
    );
}

#[test]
fn a_game_day_that_is_not_compliant_ends_with_an_error_after_it_wrote_its_journal() {
    let lab = Lab::with_state("ok");
    // One of the two runs comes back within the target: half of the
    // recovery score, and 0.875 in all.
    write_gameday(
        &lab,
        &[
            (
                "stays-up",
                experiment("stays-up", &step("fault", "action", "true"), 1),
            ),
            ("back-slow", experiment("back-slow", &down("back-slow"), 3)),
        ],
        SCORING,
    );

    let output = lab
        .command(&["gameday", "run", "lab.gameday.toon"])
        .output()
        .expect("run tumult");

    let printed = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{printed}");
    assert_eq!(
        fatal_error(&output.stderr),
        "game day finished with status: PARTIAL"
    );
    assert!(
        printed.contains("Status:  2/2 PASS (PARTIAL)\n"),
        "{printed}"
    );
    assert!(
        printed.ends_with("\nJournal: lab.gameday.journal.toon\n"),
        "{printed}"
    );
    assert_eq!(journal(&lab).compliance_status, "PARTIAL");
}

/// A game day of one experiment whose step adds a line to the file `ran`
/// in the lab, so a test can count the runs of the step.
fn write_marking_gameday(lab: &Lab) {
    let mark = step("mark", "action", "echo run >> ran");
    write_gameday(lab, &[("marks", experiment("marks", &mark, 1))], SCORING);
}

/// The lines the step of the marking game day left: one per run.
fn marks(lab: &Lab) -> String {
    std::fs::read_to_string(lab.path("ran")).unwrap_or_default()
}

#[test]
fn a_second_game_day_run_keeps_the_journal_of_the_first_and_runs_nothing() {
    let lab = Lab::with_state("ok");
    write_marking_gameday(&lab);
    gameday(&lab, "run", &[]);
    let first = std::fs::read(lab.path("lab.gameday.journal.toon")).expect("journal");

    let output = lab
        .command(&["gameday", "run", "lab.gameday.toon"])
        .output()
        .expect("run tumult");

    let printed = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{printed}");
    assert_eq!(
        fatal_error(&output.stderr),
        "journal already exists: lab.gameday.journal.toon — pass --force to overwrite"
    );
    assert_eq!(
        std::fs::read(lab.path("lab.gameday.journal.toon")).expect("journal"),
        first
    );
    // The step ran in the first game day only: the second stopped before
    // its first experiment.
    assert_eq!(marks(&lab), "run\n");
}

#[test]
fn a_game_day_run_with_force_replaces_the_journal_of_the_first() {
    let lab = Lab::with_state("ok");
    write_marking_gameday(&lab);
    gameday(&lab, "run", &[]);
    let first = journal(&lab);

    let output = lab
        .command(&["gameday", "run", "lab.gameday.toon", "--force"])
        .output()
        .expect("run tumult");

    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}");
    assert_eq!(marks(&lab), "run\nrun\n");
    let second = journal(&lab);
    assert_ne!(second.gameday_id, first.gameday_id);
    assert!(
        printed.ends_with("\nJournal: lab.gameday.journal.toon\n"),
        "{printed}"
    );
}

#[test]
fn a_game_day_whose_journal_cannot_be_written_runs_nothing() {
    let lab = Lab::with_state("ok");
    write_marking_gameday(&lab);
    // A directory holds the place of the journal.
    std::fs::create_dir(lab.path("lab.gameday.journal.toon")).expect("directory");

    let output = lab
        .command(&["gameday", "run", "lab.gameday.toon"])
        .output()
        .expect("run tumult");

    let printed = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{printed}");
    assert!(
        fatal_error(&output.stderr).contains("must be a regular file"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(marks(&lab), "");
}

/// A load block of two users for `duration_s`, with `profile` as its shape.
fn load_block(duration_s: f64, profile: &str) -> String {
    format!(
        "load:\n  tool: oha\n  script: \"http://127.0.0.1:9/health\"\n  vus: 2\n  \
         duration_s: {duration_s:.1}\n{profile}"
    )
}

/// `tumult gameday run lab.gameday.toon` with the stand-in in the place of
/// `oha`.
fn gameday_run(lab: &Lab) -> Command {
    let mut command = lab.command(&["gameday", "run", "lab.gameday.toon"]);
    command.env("TUMULT_OHA_BINARY", lab.oha_stand_in());
    command
}

#[test]
fn a_game_day_waits_for_its_load_and_no_longer() {
    let lab = Lab::with_state("ok");
    // The one run takes under a second. The load is planned for three: a
    // second of two users, a second of four, a second of two again. Each
    // second is a window of its own, and a load that is stopped starts no
    // further window.
    let fault = step("fault", "action", "sleep 0.7");
    let spike = "  profile:\n    type: spike\n    peak_vus: 4\n    after_s: 1.0\n    hold_s: 1.0\n";
    write_gameday(
        &lab,
        &[("stays-up", experiment("stays-up", &fault, 1))],
        &load_block(3.0, spike),
    );

    let output = gameday_run(&lab).output().expect("run tumult");

    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}");
    let journal = journal(&lab);
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
    assert!(
        printed.contains(
            "\n\nLoad (oha): 120 requests, 13.3 requests/s, p50 20.0 ms, p95 80.0 ms, \
             p99 100.0 ms, error rate 0.00 %, thresholds met\n\nJournal: "
        ),
        "{printed}"
    );
}

/// How the line about a load ends when the load was cut.
const CUT: &str = ", thresholds not met, cut before its planned end\n\nJournal: ";

/// Start the game day with the events of the runner on stderr.
fn start(lab: &Lab) -> Child {
    gameday_run(lab)
        .env("RUST_LOG", "tumult_core=info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start tumult")
}

fn send_sigint(child: &Child) {
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");
    assert!(sent.success(), "SIGINT was not sent");
}

/// Wait until the game day of `child` has left the file `name` in the lab.
/// If it never does, stop the binary and fail with what it printed and the
/// files of the lab.
fn wait_or_report(lab: &Lab, mut child: Child, name: &str) -> Child {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !lab.path(name).exists() {
        if Instant::now() < deadline {
            // sleep: interval of the bounded poll for a file of another process.
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        child.kill().expect("stop tumult");
        let output = child.wait_with_output().expect("wait for tumult");
        let mut files: Vec<String> = std::fs::read_dir(lab.path("."))
            .expect("lab directory")
            .map(|entry| {
                entry
                    .expect("lab entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        files.sort();
        panic!(
            "{name} never came\nfiles: {files:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    child
}

/// Whether the process whose id is in `file` still runs.
fn is_running(lab: &Lab, file: &str) -> bool {
    let pid = std::fs::read_to_string(lab.path(file)).expect("process id");
    Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(Stdio::null())
        .status()
        .expect("probe the process")
        .success()
}

/// How each run of the game day ended. The game day ended long before the
/// minute of its load: the load says that it was cut and what it had
/// measured by then, and its process is gone.
fn ended_with_its_load_cut(lab: &Lab, started: Instant) -> Vec<ExperimentStatus> {
    let took = started.elapsed();
    assert!(took < Duration::from_secs(30), "the game day took {took:?}");
    let journal = journal(lab);
    let load = journal.load_result.expect("the load left its result");
    assert!(load.execution.aborted, "{:?}", load.execution);
    assert_eq!(load.total_requests, 40);
    assert!(!is_running(lab, "load.pid"), "the load runs on");
    journal
        .experiment_journals
        .into_iter()
        .map(|run| run.status)
        .collect()
}

#[test]
fn a_game_day_that_gets_sigint_stops_its_step_and_cuts_its_load() {
    let lab = Lab::with_state("ok");
    // The step and the load would both run for a minute.
    let fault = step_within(
        "fault",
        "action",
        "sleep 60 & echo $! > sleep.pid; touch started; wait",
        90.0,
    );
    write_gameday(
        &lab,
        &[("long", experiment("long", &fault, 1))],
        &load_block(60.0, ""),
    );
    let started = Instant::now();
    let child = start(&lab);
    let child = wait_or_report(&lab, child, "started");
    wait_for(&lab.path("load.pid"));

    send_sigint(&child);

    let output = child.wait_with_output().expect("wait for tumult");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{printed}");
    assert_eq!(
        fatal_error(&output.stderr),
        "game day finished with status: NON-COMPLIANT"
    );
    let ended = ended_with_its_load_cut(&lab, started);
    assert_eq!(ended, [ExperimentStatus::Interrupted]);
    assert!(!is_running(&lab, "sleep.pid"), "the step runs on");
    assert!(
        printed.contains("Status:  0/1 PASS (NON-COMPLIANT)\n"),
        "{printed}"
    );
    assert!(printed.contains(CUT), "{printed}");
}

/// Read the events on `stderr` until the one whose message is `message`.
fn await_event(stderr: &mut impl BufRead, message: &str) {
    for line in stderr.lines() {
        let line = line.expect("a line of stderr");
        let event: Value = serde_json::from_str(&line).unwrap_or_default();
        if event["message"] == message {
            return;
        }
    }
    panic!("tumult ended without the event {message:?}");
}

#[test]
fn a_game_day_that_gets_sigint_while_it_waits_for_its_load_cuts_the_load() {
    let lab = Lab::with_state("ok");
    // The one run is over at once. The load is planned for a minute.
    write_gameday(
        &lab,
        &[(
            "stays-up",
            experiment("stays-up", &step("fault", "action", "true"), 1),
        )],
        &load_block(60.0, ""),
    );
    let started = Instant::now();
    let mut child = start(&lab);
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr"));
    await_event(&mut stderr, "gameday waits for its load to end");
    wait_for(&lab.path("load.pid"));

    send_sigint(&child);

    let mut rest = String::new();
    stderr
        .read_to_string(&mut rest)
        .expect("the rest of stderr");
    let output = child.wait_with_output().expect("wait for tumult");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}{rest}");
    let ended = ended_with_its_load_cut(&lab, started);
    assert_eq!(ended, [ExperimentStatus::Completed]);
    assert!(
        printed.contains("Status:  1/1 PASS (COMPLIANT)\n"),
        "{printed}"
    );
    assert!(printed.contains(CUT), "{printed}");
}
