//! Robustness verdicts through the runner and the oha adapter: the drift
//! of a soak and the recovery after a spike, judged from the windows the
//! adapter keeps. A fake `oha` prints one summary per window, with the
//! p95 and the answers each window is given.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{run_experiment, ActivityExecutor, ActivityOutcome, RunConfig};
use tumult_core::types::{
    Activity, ActivityType, Experiment, Journal, LoadConfig, LoadProfile, LoadTool, NfrCriticality,
    NfrKind, NfrObjective, NfrOperator, NfrState, NfrVerdict,
};
use tumult_test_utils::EnvGuard;

/// A fake oha that answers window `n` with line `n` of the file `windows`
/// next to it: the p95 in seconds, the answers with a 200 and those with
/// a 503. Every window ends at once and waits for the next slot.
const FAKE_OHA: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && exit 0
dir=$(dirname "$0")
echo x >> "$dir/calls"
n=$(wc -l < "$dir/calls")
line=$(sed -n "${n}p" "$dir/windows")
[ -z "$line" ] && line=$(tail -n 1 "$dir/windows")
set -- $line
printf '{"summary":{"total":0.1,"slowest":%s},"latencyPercentiles":{"p50":%s,"p95":%s,"p99":%s},"statusCodeDistribution":{"200":%s,"503":%s},"errorDistribution":{}}' "$1" "$1" "$1" "$1" "$2" "$3"
"#;

/// Point `TUMULT_OHA_BINARY` at the fake, which answers each window with
/// one of `windows`: (p95 in ms, answers with a 200, answers with a 503).
fn install_fake_oha(env: &mut EnvGuard, dir: &tempfile::TempDir, windows: &[(f64, u32, u32)]) {
    let lines: Vec<String> = windows
        .iter()
        .map(|(p95_ms, ok, failed)| format!("{} {ok} {failed}", p95_ms / 1000.0))
        .collect();
    std::fs::write(dir.path().join("windows"), lines.join("\n") + "\n").expect("windows");
    let path = dir.path().join("fake-oha");
    tumult_test_utils::write_executable(&path, FAKE_OHA);
    env.set("TUMULT_OHA_BINARY", &path);
}

/// The one step of the method holds the run while the load runs.
struct HoldsFor(Duration);

impl ActivityExecutor for HoldsFor {
    fn execute(&self, _activity: &Activity) -> ActivityOutcome {
        // sleep: the method is what keeps the load running to its end
        std::thread::sleep(self.0);
        ActivityOutcome {
            success: true,
            output: None,
            error: None,
            duration_ms: 0,
        }
    }
}

fn objective(id: &str, kind: NfrKind, metric: &str, threshold: f64) -> NfrObjective {
    NfrObjective {
        id: id.into(),
        kind,
        metric: metric.into(),
        operator: NfrOperator::Lte,
        threshold,
        window_s: None,
        criticality: NfrCriticality::Major,
        description: None,
    }
}

fn load(profile: LoadProfile, vus: u32, duration_s: f64) -> LoadConfig {
    LoadConfig {
        tool: LoadTool::Oha,
        script: PathBuf::from("http://127.0.0.1:1/checkout"),
        vus: Some(vus),
        duration_s: Some(duration_s),
        thresholds: HashMap::new(),
        profile: Some(profile),
    }
}

/// Run an experiment with `load` and `objectives` to the end of its load.
fn run(load: LoadConfig, objectives: Vec<NfrObjective>) -> Journal {
    let hold = Duration::from_secs_f64(load.duration_s.expect("a duration") + 1.0);
    let experiment = Experiment {
        title: "robustness".into(),
        method: vec![Activity {
            name: "hold".into(),
            activity_type: ActivityType::Action,
            ..Activity::default()
        }],
        load: Some(load),
        nfr: objectives,
        ..Experiment::default()
    };
    let executor: Arc<dyn ActivityExecutor> = Arc::new(HoldsFor(hold));
    run_experiment(
        &experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
    )
    .expect("the run ends")
}

fn verdict<'a>(journal: &'a Journal, id: &str) -> &'a NfrVerdict {
    journal
        .objectives
        .as_ref()
        .and_then(|outcome| outcome.verdicts.iter().find(|v| v.nfr_id == id))
        .unwrap_or_else(|| panic!("no verdict {id}: {:?}", journal.objectives))
}

fn soak(seconds: f64) -> LoadConfig {
    load(
        LoadProfile::Soak {
            sample_interval_s: 1.0,
        },
        4,
        seconds,
    )
}

fn drift_objectives() -> Vec<NfrObjective> {
    vec![
        objective("p95-drift", NfrKind::Latency, "load.p95_drift_pct", 20.0),
        objective(
            "error-drift",
            NfrKind::ErrorRate,
            "load.error_rate_drift",
            0.02,
        ),
    ]
}

#[test]
fn a_soak_whose_p95_rises_by_half_breaches_a_drift_objective_of_twenty_percent() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    // The p95 grows from 100 to 150 ms, and the last window fails one
    // request in ten.
    install_fake_oha(
        &mut env,
        &dir,
        &[
            (100.0, 100, 0),
            (110.0, 100, 0),
            (130.0, 95, 5),
            (150.0, 90, 10),
        ],
    );

    let journal = run(soak(4.0), drift_objectives());

    let samples = &journal.load_result.as_ref().expect("a load").samples;
    assert_eq!(samples.len(), 4, "{samples:?}");
    let p95 = verdict(&journal, "p95-drift");
    assert_eq!(p95.state, NfrState::Breached, "{p95:?}");
    let grew = p95.observed.expect("a drift");
    assert!((grew - 50.0).abs() < 1e-6, "{grew}");
    let errors = verdict(&journal, "error-drift");
    assert_eq!(errors.state, NfrState::Breached, "{errors:?}");
    let rose = errors.observed.expect("a drift");
    assert!((rose - 0.1).abs() < 1e-9, "{rose}");
}

#[test]
fn a_soak_that_stays_flat_meets_its_drift_objectives() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&mut env, &dir, &[(100.0, 100, 0); 4]);

    let journal = run(soak(4.0), drift_objectives());

    for id in ["p95-drift", "error-drift"] {
        let flat = verdict(&journal, id);
        assert_eq!(flat.state, NfrState::Met, "{flat:?}");
        assert_eq!(flat.observed, Some(0.0), "{flat:?}");
        assert_eq!(flat.reason, None);
    }
}

#[test]
fn a_soak_of_three_windows_is_too_short_to_judge_and_says_so() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(
        &mut env,
        &dir,
        &[(100.0, 100, 0), (100.0, 100, 0), (900.0, 0, 100)],
    );

    let journal = run(soak(3.0), drift_objectives());

    for id in ["p95-drift", "error-drift"] {
        let short = verdict(&journal, id);
        assert_eq!(short.state, NfrState::Inconclusive, "{short:?}");
        assert_eq!(short.observed, None);
        assert_eq!(short.reason.as_deref(), Some(NfrVerdict::TOO_FEW_WINDOWS));
    }
}

/// One user, four for one second after two, then one again for five
/// seconds: one window a second.
fn spike(after_s: f64, duration_s: f64) -> LoadConfig {
    load(
        LoadProfile::Spike {
            peak_vus: 4,
            after_s,
            hold_s: 1.0,
            sample_interval_s: Some(1.0),
        },
        1,
        duration_s,
    )
}

fn recovery_objectives() -> Vec<NfrObjective> {
    vec![
        objective(
            "back-in-5s",
            NfrKind::Recovery,
            "load.spike_recovery_s",
            5.0,
        ),
        objective(
            "back-in-2s",
            NfrKind::Recovery,
            "load.spike_recovery_s",
            2.0,
        ),
    ]
}

#[test]
fn a_spike_whose_p95_is_back_in_the_band_after_three_seconds_meets_five_and_breaches_two() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    // Before the spike the p95 is 100 ms, so the band is up to 120 ms.
    // After it, the third window still reads 200 ms and the fourth 110.
    install_fake_oha(
        &mut env,
        &dir,
        &[
            (100.0, 100, 0),
            (100.0, 100, 0),
            (500.0, 100, 0),
            (400.0, 100, 0),
            (300.0, 100, 0),
            (200.0, 100, 0),
            (110.0, 100, 0),
            (100.0, 100, 0),
        ],
    );

    let journal = run(spike(2.0, 8.0), recovery_objectives());

    let samples = &journal.load_result.as_ref().expect("a load").samples;
    assert_eq!(samples.len(), 8, "{samples:?}");
    let met = verdict(&journal, "back-in-5s");
    assert_eq!(met.state, NfrState::Met, "{met:?}");
    let back = met.observed.expect("a recovery time");
    assert!((3.0..3.5).contains(&back), "{back}");
    let breached = verdict(&journal, "back-in-2s");
    assert_eq!(breached.state, NfrState::Breached, "{breached:?}");
    assert_eq!(breached.observed, Some(back));
}

#[test]
fn a_spike_with_no_window_before_it_is_too_short_to_judge_and_says_so() {
    let mut env = EnvGuard::lock();
    let dir = tempfile::tempdir().expect("tempdir");
    install_fake_oha(&mut env, &dir, &[(500.0, 100, 0), (100.0, 100, 0)]);

    let journal = run(spike(0.0, 2.0), recovery_objectives());

    let short = verdict(&journal, "back-in-5s");
    assert_eq!(short.state, NfrState::Inconclusive, "{short:?}");
    assert_eq!(short.observed, None);
    assert_eq!(short.reason.as_deref(), Some(NfrVerdict::TOO_FEW_WINDOWS));
}
