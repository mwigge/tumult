//! What `tumult gameday run` scores.
//!
//! The game days here are real: the binary runs each experiment with `sh`
//! steps on a target that goes down and heals itself after a number of
//! reads. The score is read from what the binary prints and from the
//! journal it writes.
#![cfg(unix)]

mod lab;

use lab::{count, indented, step, Lab};
use tumult_core::types::GameDayJournal;

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

#[test]
fn the_recovery_score_is_the_share_of_runs_that_came_back_within_the_target() {
    let lab = Lab::with_state("ok");
    // The run reads its target once when the method starts. The fault
    // comes half a second later, so every read of a target that is down
    // is a read of the recovery that follows the method.
    let down = |name: &str| {
        [
            step("settle", "action", "sleep 0.5"),
            step("fault", "action", &format!("echo down > {name}.state")),
        ]
        .concat()
    };
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
        "scoring:\n  pass_threshold: 0.75\n  mttr_target_s: 2.0\n  recovery_required: true\n",
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
        "\nResilience Score: 0.92\n",
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
            "\nResilience Score: 0.92\n  \
             Pass rate:    1.00  Recovery: 0.67  Load: 1.00  Compliance: 1.00\n  \
             Status: COMPLIANT\n"
        ),
        "{analysed}"
    );
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
    let lab = Lab::with_state("ok");
    // The one run takes under a second; the load is planned for three.
    let fault = step("fault", "action", "sleep 0.7");
    write_gameday(
        &lab,
        &[("stays-up", experiment("stays-up", &fault, 1))],
        "load:\n  tool: oha\n  script: \"http://127.0.0.1:9/health\"\n  vus: 2\n  duration_s: 3.0\n",
    );
    let oha = lab.path("oha-stand-in.sh");
    tumult_test_utils::write_executable(&oha, OHA_STAND_IN);

    let printed = gameday(
        &lab,
        "run",
        &[("TUMULT_OHA_BINARY", oha.to_str().expect("utf-8 path"))],
    );

    let journal = journal(&lab);
    assert!(
        (3.0..4.0).contains(&journal.duration_s),
        "the game day took {}s for a load of 3s",
        journal.duration_s
    );
    let load = journal.load_result.expect("the load left its result");
    assert_eq!(load.total_requests, 40);
    assert!(
        printed.contains("\nLoad (oha): 40 requests, p95=80ms, error_rate=0.0000\n"),
        "{printed}"
    );
}
