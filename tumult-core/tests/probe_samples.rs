//! The samples of the hypothesis probes, taken while the method runs and
//! after it.
//!
//! A sample is judged, traced and kept the way a step of the method is: a
//! sample that reads outside its tolerance has a span with an error that
//! says so, a provider that panics is a failed sample, and the probes keep
//! the order of the experiment file.
//!
//! Every probe is an `sh` process that reads the state file of a lab, so
//! the samples are real reads of a target that changes under the fault.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opentelemetry::trace::Status;
use opentelemetry_sdk::trace::SpanData;
use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{
    run_experiment_with_sampling, ActivityExecutor, ActivityOutcome, RunConfig, SamplingConfig,
};
use tumult_core::types::*;
use tumult_test_utils::{capture_spans, span_attribute};

const OUTSIDE: &str = "the output is outside the tolerance";

/// The target of the lab prints its state and counts each read in `reads`.
/// A state with a count, such as `down 3`, holds for that many reads, then
/// the target is `ok` again. In the state `refused` the target cannot be
/// reached.
const TARGET: &str = r#"read -r state left < state
echo "$state" >> reads
if test -n "$left" && test "$left" -gt 1; then sh set-state.sh "$state" $((left - 1))
elif test -n "$left"; then sh set-state.sh ok
fi
if test "$state" = refused; then echo "connection refused" >&2; exit 7; fi
echo "$state"
"#;

/// `sh set-state.sh down 2` makes `down 2` the state of the target. The
/// new state goes to a file of its own first and is then moved in place,
/// so a read at the same time sees the old state or the new one, never an
/// empty file.
const SET_STATE: &str = r#"echo "$*" > "state.$$"
mv "state.$$" state
"#;

/// Runs the process of a step in the directory of the lab. What it printed
/// is the output, and a step that exits with an error did not run: its
/// error is what it wrote to stderr. A plugin with a bug dies on an answer
/// it does not know: this one panics on a probe that printed `crash`.
struct InLab(tempfile::TempDir);

impl InLab {
    fn new() -> Self {
        let lab = Self(tempfile::TempDir::new().expect("lab directory"));
        std::fs::write(lab.path().join("target.sh"), TARGET).expect("target script");
        std::fs::write(lab.path().join("set-state.sh"), SET_STATE).expect("state script");
        std::fs::write(lab.path().join("state"), "ok\n").expect("state file");
        lab
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

impl ActivityExecutor for InLab {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        let Provider::Process {
            path, arguments, ..
        } = &activity.provider
        else {
            panic!("the steps of this lab are processes");
        };
        let started = Instant::now();
        let ran = Command::new(path)
            .args(arguments)
            .current_dir(self.path())
            .output()
            .expect("the process starts");
        let printed = String::from_utf8_lossy(&ran.stdout).trim().to_string();
        assert_ne!(printed, "crash", "the plugin cannot read this answer");
        let error = (!ran.status.success())
            .then(|| String::from_utf8_lossy(&ran.stderr).trim().to_string());
        ActivityOutcome {
            success: ran.status.success(),
            output: Some(printed),
            error,
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

/// A probe that reads the target and holds while it says `ok`.
fn reads_ok(name: &str) -> Activity {
    Activity {
        tolerance: Some(Tolerance::Regex {
            pattern: "^ok$".into(),
        }),
        ..step(name, ActivityType::Probe, "sh target.sh")
    }
}

/// Shell that waits until the target was read `count` times in all (a
/// shell expression), for five seconds at most.
fn until_read(count: &str) -> String {
    format!(
        "i=0; until test \"$(wc -l < reads)\" -ge {count} || test $i -ge 500; \
         do sleep 0.01; i=$((i + 1)); done"
    )
}

/// An experiment whose hypothesis has `probes` and whose method is one
/// action that runs `fault`.
fn experiment(probes: Vec<Activity>, fault: &str) -> Experiment {
    Experiment {
        title: "probe samples".into(),
        steady_state_hypothesis: Some(Hypothesis {
            title: "the target answers ok".into(),
            probes,
        }),
        method: vec![step("fault", ActivityType::Action, fault)],
        ..Experiment::default()
    }
}

/// Sampling every 10 ms, at most `rounds` rounds while the method runs.
fn sampling(rounds: u32) -> SamplingConfig {
    SamplingConfig {
        interval: Duration::from_millis(10),
        max_during_samples: rounds,
        recovery_timeout: Duration::from_secs(5),
    }
}

fn run(experiment: &Experiment, lab: InLab, sampling: &SamplingConfig) -> Journal {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(lab);
    run_experiment_with_sampling(
        experiment,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default(),
        sampling,
    )
    .expect("run")
}

fn samples(journal: &Journal, phase: SamplePhase) -> Vec<&ProbeSample> {
    journal
        .probe_samples
        .iter()
        .filter(|sample| sample.phase == phase)
        .collect()
}

/// The names of the probes in `samples`, one per group of samples in a row.
fn groups(samples: &[&ProbeSample]) -> Vec<String> {
    let mut names: Vec<String> = samples.iter().map(|s| s.probe.clone()).collect();
    names.dedup();
    names
}

fn epoch_ns(at: SystemTime) -> i64 {
    let since = at.duration_since(UNIX_EPOCH).expect("after the epoch");
    i64::try_from(since.as_nanos()).expect("in range")
}

/// The span of `sample`: the one span of its probe that was open when the
/// sample started.
fn span_of<'a>(spans: &'a [SpanData], sample: &ProbeSample) -> &'a SpanData {
    let mut open = spans.iter().filter(|span| {
        span.name == "resilience.probe"
            && span_attribute(span, "resilience.action.name")
                .is_some_and(|name| name.as_str() == sample.probe.as_str())
            && epoch_ns(span.start_time) <= sample.ts_ns
            && sample.ts_ns <= epoch_ns(span.end_time)
    });
    let span = open
        .next()
        .unwrap_or_else(|| panic!("no span for {sample:?}"));
    assert!(open.next().is_none(), "two spans for {sample:?}");
    span
}

/// Every sample of `phase` has the span its result calls for: a failed
/// sample has a span with the error `failed_with`, and a sample that held
/// has a span with no error. Returns the count of failed samples.
fn assert_spans_agree(
    journal: &Journal,
    spans: &[SpanData],
    phase: SamplePhase,
    failed_with: &str,
) -> usize {
    let taken = samples(journal, phase);
    for sample in &taken {
        let expected = if sample.ok {
            Status::Unset
        } else {
            Status::error(failed_with.to_string())
        };
        assert_eq!(span_of(spans, sample).status, expected, "{sample:?}");
    }
    let failed = taken.iter().filter(|sample| !sample.ok).count();
    let error_spans = taken
        .iter()
        .filter(|sample| matches!(span_of(spans, sample).status, Status::Error { .. }))
        .count();
    assert_eq!(error_spans, failed);
    assert!(failed >= 1, "no sample failed: {taken:?}");
    assert!(failed < taken.len(), "no sample held: {taken:?}");
    failed
}

#[test]
fn a_sample_under_the_fault_outside_its_tolerance_has_a_span_that_says_so() {
    let lab = InLab::new();
    // The target is down for two reads, then up. The method waits for the
    // sampler to read it four times from the fault on.
    let fault = format!(
        "n=$(wc -l < reads); sh set-state.sh down 2; {}",
        until_read("$((n + 4))")
    );
    let experiment = experiment(vec![reads_ok("steady")], &fault);

    let (journal, spans) = capture_spans(|| run(&experiment, lab, &sampling(50)));

    let failed = assert_spans_agree(&journal, &spans, SamplePhase::During, OUTSIDE);
    let during = journal.during_result.expect("sampled during the method");
    assert_eq!(
        during.probes[0].breach_count,
        u32::try_from(failed).unwrap()
    );
}

#[test]
fn a_sample_after_the_method_outside_its_tolerance_has_a_span_that_says_so() {
    let lab = InLab::new();
    // One read before the method and one while it runs, then the target is
    // down for three reads after it.
    let fault = format!("{}; sh set-state.sh down 3", until_read("2"));
    let experiment = experiment(vec![reads_ok("steady")], &fault);

    let (journal, spans) = capture_spans(|| run(&experiment, lab, &sampling(1)));

    let failed = assert_spans_agree(&journal, &spans, SamplePhase::Post, OUTSIDE);
    assert_eq!(failed, 3);
    let post = journal.post_result.expect("sampled after the method");
    assert!(post.full_recovery, "{post:?}");
}

#[test]
fn a_sample_its_plugin_could_not_take_keeps_the_error_of_the_plugin() {
    let lab = InLab::new();
    let fault = format!("{}; sh set-state.sh refused 2", until_read("2"));
    let experiment = experiment(vec![reads_ok("steady")], &fault);

    let (journal, spans) = capture_spans(|| run(&experiment, lab, &sampling(1)));

    let failed = assert_spans_agree(&journal, &spans, SamplePhase::Post, "connection refused");
    assert_eq!(failed, 2);
}

#[test]
fn the_probes_of_a_hypothesis_keep_the_order_of_the_file() {
    let names = [
        "kilo", "delta", "zulu", "alpha", "mike", "echo", "bravo", "tango",
    ];
    let lab = InLab::new();
    // Eight reads before the method, and two rounds of eight while it runs.
    let fault = until_read("24");
    let experiment = experiment(names.into_iter().map(reads_ok).collect(), &fault);

    let journal = run(&experiment, lab, &sampling(50));

    let during = journal.during_result.as_ref().expect("sampled during");
    let during_names: Vec<&str> = during.probes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(during_names, names);
    let post = journal.post_result.as_ref().expect("sampled after");
    let post_names: Vec<&str> = post.probes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(post_names, names);

    // The journal keeps the samples of the method, then those after it,
    // each phase grouped by probe in the order of the file.
    let phases: Vec<SamplePhase> = journal.probe_samples.iter().map(|s| s.phase).collect();
    let first_post = phases
        .iter()
        .position(|phase| *phase == SamplePhase::Post)
        .expect("a sample after the method");
    assert!(phases[..first_post]
        .iter()
        .all(|p| *p == SamplePhase::During));
    assert!(phases[first_post..].iter().all(|p| *p == SamplePhase::Post));
    for phase in [SamplePhase::During, SamplePhase::Post] {
        let taken = samples(&journal, phase);
        assert_eq!(groups(&taken), names, "{phase:?}");
        for group in taken.chunk_by(|a, b| a.probe == b.probe) {
            assert!(group.is_sorted_by_key(|s| s.ts_ns), "{group:?}");
        }
    }
    assert!(during.probes.iter().all(|p| p.samples >= 2), "{during:?}");
}

#[test]
fn two_probes_with_one_name_are_one_series_that_holds_the_samples_of_both() {
    let lab = InLab::new();
    // Three reads before the method, and two rounds of three while it runs.
    let fault = until_read("9");
    let probes = vec![reads_ok("answer"), reads_ok("other"), reads_ok("answer")];
    let experiment = experiment(probes, &fault);

    let journal = run(&experiment, lab, &sampling(50));

    let during = journal.during_result.as_ref().expect("sampled during");
    let during_names: Vec<&str> = during.probes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(during_names, ["answer", "other"]);
    assert_eq!(during.probes[0].samples, 2 * during.probes[1].samples);
    let post = journal.post_result.as_ref().expect("sampled after");
    let post_names: Vec<&str> = post.probes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(post_names, ["answer", "other"]);
    for phase in [SamplePhase::During, SamplePhase::Post] {
        let taken = samples(&journal, phase);
        assert_eq!(groups(&taken), ["answer", "other"], "{phase:?}");
        let answers = taken.iter().filter(|s| s.probe == "answer").count();
        let others = taken.iter().filter(|s| s.probe == "other").count();
        assert_eq!(answers, 2 * others, "{phase:?}");
    }
}

#[test]
fn a_probe_that_panics_after_the_method_is_a_failed_sample_and_the_run_has_a_journal() {
    let lab = InLab::new();
    // One read before the method and one while it runs. The first read
    // after it makes the plugin panic, the next one reads ok.
    let fault = format!("{}; sh set-state.sh crash 1", until_read("2"));
    let experiment = experiment(vec![reads_ok("steady")], &fault);

    let ran = std::panic::catch_unwind(|| run(&experiment, lab, &sampling(1)));

    let journal = ran.expect("the panic of a sample stays in the runner");
    assert_eq!(journal.status, ExperimentStatus::Completed);
    let after: Vec<bool> = samples(&journal, SamplePhase::Post)
        .iter()
        .map(|s| s.ok)
        .collect();
    assert_eq!(after, [false, true]);
    assert!(journal.post_result.expect("sampled after").full_recovery);
}

#[test]
fn the_sampler_goes_on_after_a_probe_that_panicked() {
    let lab = InLab::new();
    // The read after the fault makes the plugin panic. The method waits for
    // the sampler to read the target three times from the fault on.
    let fault = format!(
        "n=$(wc -l < reads); sh set-state.sh crash 1; {}",
        until_read("$((n + 3))")
    );
    let experiment = experiment(vec![reads_ok("steady")], &fault);

    let journal = run(&experiment, lab, &sampling(50));

    let during: Vec<bool> = samples(&journal, SamplePhase::During)
        .iter()
        .map(|s| s.ok)
        .collect();
    let panicked = during
        .iter()
        .position(|ok| !ok)
        .unwrap_or_else(|| panic!("no failed sample: {during:?}"));
    assert!(during[panicked + 1..].contains(&true), "{during:?}");
    assert_eq!(
        journal.during_result.expect("sampled").probes[0].breach_count,
        1
    );
}

#[test]
fn a_probe_that_panics_in_the_check_before_the_method_ends_the_run_with_a_journal() {
    let lab = InLab::new();
    // The first read of the target makes the plugin panic.
    std::fs::write(lab.path().join("state"), "crash\n").expect("state file");
    let experiment = experiment(vec![reads_ok("steady")], "sh set-state.sh down");

    let journal = run(&experiment, lab, &sampling(1));

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert!(
        journal.method_results.is_empty(),
        "{:?}",
        journal.method_results
    );
    let before = journal.steady_state_before.expect("checked before");
    assert!(!before.met);
    let error = before.probe_results[0].error.as_deref().expect("an error");
    assert!(error.starts_with("activity panicked: "), "{error}");
    assert!(
        error.contains("the plugin cannot read this answer"),
        "{error}"
    );
}

#[test]
fn a_probe_that_panics_in_the_check_after_the_method_fails_the_check_and_the_rollback_runs() {
    let lab = InLab::new();
    // One read before the method and one while it runs. From then on every
    // read makes the plugin panic, until the rollback restores the target.
    let fault = format!("{}; sh set-state.sh crash", until_read("2"));
    let experiment = Experiment {
        rollbacks: vec![step(
            "restore",
            ActivityType::Action,
            "sh set-state.sh ok; echo restored",
        )],
        ..experiment(vec![reads_ok("steady")], &fault)
    };
    let sampling = SamplingConfig {
        recovery_timeout: Duration::from_millis(50),
        ..sampling(1)
    };

    let journal = run(&experiment, lab, &sampling);

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let after = journal.steady_state_after.expect("checked after");
    assert!(!after.met);
    let error = after.probe_results[0].error.as_deref().expect("an error");
    assert!(error.starts_with("activity panicked: "), "{error}");
    assert!(
        error.contains("the plugin cannot read this answer"),
        "{error}"
    );
    let rolled_back: Vec<(&str, bool, Option<&str>)> = journal
        .rollback_results
        .iter()
        .map(|step| {
            (
                step.name.as_str(),
                step.status == ActivityStatus::Succeeded,
                step.output.as_deref(),
            )
        })
        .collect();
    assert_eq!(rolled_back, [("restore", true, Some("restored"))]);
}
