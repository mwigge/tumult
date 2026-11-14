//! The version a run records for its target: declared in the definition,
//! or asked of the target's telemetry once, when the run ends.

use super::*;
use crate::controls::ControlRegistry;
use std::sync::Mutex;

/// Answers with scripted versions, the last answer for every further
/// question, and remembers every question.
pub(super) struct ScriptedResolver {
    answers: Vec<Result<Vec<VersionShare>, String>>,
    settle: VersionSettle,
    pub(super) asked: Mutex<Vec<RunWindow>>,
}

fn share(version: &str, requests: u64) -> VersionShare {
    VersionShare {
        version: version.into(),
        requests,
    }
}

impl ScriptedResolver {
    fn scripted(
        answers: Vec<Result<Vec<VersionShare>, String>>,
        settle: VersionSettle,
    ) -> Arc<Self> {
        Arc::new(Self {
            answers,
            settle,
            asked: Mutex::new(Vec::new()),
        })
    }

    pub(super) fn answering(version: Option<&str>) -> Arc<Self> {
        let seen = version.map(|v| share(v, 10)).into_iter().collect();
        Self::scripted(vec![Ok(seen)], VersionSettle::ONCE)
    }

    fn failing(error: &str) -> Arc<Self> {
        Self::scripted(vec![Err(error.into())], VersionSettle::ONCE)
    }

    pub(super) fn questions(&self) -> Vec<RunWindow> {
        self.asked.lock().unwrap().clone()
    }
}

impl TargetVersionResolver for ScriptedResolver {
    fn resolve(&self, window: &RunWindow) -> Result<Vec<VersionShare>, String> {
        let mut asked = self.asked.lock().unwrap();
        asked.push(window.clone());
        let turn = (asked.len() - 1).min(self.answers.len() - 1);
        self.answers[turn].clone()
    }

    fn settle(&self) -> VersionSettle {
        self.settle
    }
}

const QUICK: VersionSettle = VersionSettle {
    pause: std::time::Duration::from_millis(1),
    deadline: std::time::Duration::from_secs(5),
};

#[test]
fn the_telemetry_is_asked_again_until_its_count_settles() {
    let mut exp = minimal_experiment();
    exp.target = Some(checkout(None));
    // The exporter is still flushing on the first two questions.
    let resolver = ScriptedResolver::scripted(
        vec![
            Ok(vec![]),
            Ok(vec![share("1.5.0", 6)]),
            Ok(vec![share("1.5.1", 94), share("1.5.0", 6)]),
            Ok(vec![share("1.5.1", 94), share("1.5.0", 6)]),
        ],
        QUICK,
    );

    let journal = run(&exp, &config_with(&resolver));

    assert_eq!(resolver.questions().len(), 4, "until two answers agree");
    let recorded = journal.target_version.expect("a version");
    assert_eq!(recorded.version, "1.5.1");
    assert_eq!(recorded.version_source, VersionSource::Mixed);
    assert_eq!(recorded.shares, vec![share("1.5.1", 94), share("1.5.0", 6)]);
}

/// A run that is told to stop while its version is being asked for does
/// not sit out the settle pauses: it records what the last answer said.
#[test]
fn a_run_told_to_stop_does_not_wait_for_the_telemetry_to_settle() {
    /// Counts ever more requests, and stops the run on the first question.
    struct Stopping(tokio_util::sync::CancellationToken, Mutex<u64>);
    impl TargetVersionResolver for Stopping {
        fn resolve(&self, _window: &RunWindow) -> Result<Vec<VersionShare>, String> {
            let mut asked = self.1.lock().unwrap();
            *asked += 1;
            self.0.cancel();
            Ok(vec![share("1.5.0", *asked)])
        }

        fn settle(&self) -> VersionSettle {
            VersionSettle {
                pause: std::time::Duration::from_millis(100),
                deadline: std::time::Duration::from_secs(1),
            }
        }
    }
    let mut exp = minimal_experiment();
    exp.target = Some(checkout(None));
    let token = tokio_util::sync::CancellationToken::new();
    let resolver = Arc::new(Stopping(token.clone(), Mutex::new(0)));
    let config = RunConfig {
        cancellation_token: Some(token),
        version_resolver: Some(resolver.clone()),
        ..RunConfig::default()
    };

    let journal = run(&exp, &config);

    assert_eq!(*resolver.1.lock().unwrap(), 1, "asked once, not again");
    assert_eq!(
        journal.target_version.map(|v| v.version),
        Some("1.5.0".into())
    );
}

#[test]
fn telemetry_that_never_settles_is_read_at_the_deadline() {
    struct Growing(Mutex<u64>);
    impl TargetVersionResolver for Growing {
        fn resolve(&self, _window: &RunWindow) -> Result<Vec<VersionShare>, String> {
            let mut asked = self.0.lock().unwrap();
            *asked += 1;
            Ok(vec![share("1.5.1", *asked)])
        }

        fn settle(&self) -> VersionSettle {
            VersionSettle {
                pause: std::time::Duration::from_millis(5),
                deadline: std::time::Duration::from_millis(40),
            }
        }
    }
    let growing = Growing(Mutex::new(0));
    let window = RunWindow {
        system: "checkout".into(),
        environment: None,
        namespace: None,
        started_at_ns: 0,
        ended_at_ns: 1,
    };
    let started = std::time::Instant::now();
    let seen = settled_versions(&growing, &window).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let asked = *growing.0.lock().unwrap();
    assert!((2..=9).contains(&asked), "asked {asked} times");
    assert_eq!(seen, vec![share("1.5.1", asked)], "the last answer");
    // Silent telemetry settles on the second question.
    let silent = ScriptedResolver::scripted(vec![Ok(vec![])], QUICK);
    assert_eq!(settled_versions(silent.as_ref(), &window), Ok(vec![]));
    assert_eq!(silent.questions().len(), 2);
    // An error ends the asking.
    let broken = ScriptedResolver::scripted(vec![Ok(vec![]), Err("gone".into())], QUICK);
    assert_eq!(
        settled_versions(broken.as_ref(), &window),
        Err("gone".into())
    );
}

pub(super) fn checkout(version: Option<&str>) -> ExperimentTarget {
    ExperimentTarget {
        system: "checkout".into(),
        technology: None,
        environment: Some("demo".into()),
        criticality: None,
        namespace: None,
        version: version.map(str::to_string),
    }
}

pub(super) fn config_with(resolver: &Arc<ScriptedResolver>) -> RunConfig {
    RunConfig {
        version_resolver: Some(resolver.clone()),
        ..RunConfig::default()
    }
}

fn run(experiment: &Experiment, config: &RunConfig) -> Journal {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    let controls = Arc::new(ControlRegistry::new());
    run_experiment_with_sampling(experiment, &executor, &controls, config, &fast_sampling())
        .unwrap()
}

#[test]
fn a_declared_version_is_recorded_and_the_telemetry_is_not_asked() {
    let mut exp = minimal_experiment();
    exp.target = Some(checkout(Some("1.5.0")));
    let resolver = ScriptedResolver::answering(Some("9.9.9"));

    let journal = run(&exp, &config_with(&resolver));

    assert_eq!(
        journal.target_version,
        Some(TargetVersion {
            version: "1.5.0".into(),
            version_source: VersionSource::Declared,
            shares: Vec::new(),
        })
    );
    assert!(resolver.questions().is_empty(), "declared wins unasked");
}

#[test]
fn an_undeclared_version_is_observed_once_when_the_run_ends() {
    let mut exp = minimal_experiment();
    exp.target = Some(ExperimentTarget {
        namespace: Some("shop".into()),
        ..checkout(None)
    });
    let resolver = ScriptedResolver::answering(Some("1.5.1"));

    let journal = run(&exp, &config_with(&resolver));

    assert_eq!(
        journal.target_version,
        Some(TargetVersion {
            version: "1.5.1".into(),
            version_source: VersionSource::Observed,
            shares: Vec::new(),
        })
    );
    // Asked once, about this run's own window, environment and
    // namespace: telemetry that speaks inside the run is not padded.
    assert_eq!(
        resolver.questions(),
        vec![RunWindow {
            system: "checkout".into(),
            environment: Some("demo".into()),
            namespace: Some("shop".into()),
            started_at_ns: journal.started_at_ns,
            ended_at_ns: journal.ended_at_ns,
        }]
    );
}

/// Answers `inside` about a window that begins at or after `not_before`
/// (a run's own window) and `around` about one that begins earlier (the
/// window padded for clock skew).
struct ByWindow {
    not_before: i64,
    inside: Vec<VersionShare>,
    around: Vec<VersionShare>,
    asked: Mutex<Vec<RunWindow>>,
}

impl TargetVersionResolver for ByWindow {
    fn resolve(&self, window: &RunWindow) -> Result<Vec<VersionShare>, String> {
        self.asked.lock().unwrap().push(window.clone());
        if window.started_at_ns >= self.not_before {
            return Ok(self.inside.clone());
        }
        Ok(self.around.clone())
    }

    fn settle(&self) -> VersionSettle {
        VersionSettle::ONCE
    }
}

fn observed_by(inside: Vec<VersionShare>, around: Vec<VersionShare>) -> (Journal, Vec<RunWindow>) {
    let mut exp = minimal_experiment();
    exp.target = Some(checkout(None));
    let resolver = Arc::new(ByWindow {
        not_before: chrono::Utc::now().timestamp_nanos_opt().unwrap(),
        inside,
        around,
        asked: Mutex::new(Vec::new()),
    });
    let config = RunConfig {
        version_resolver: Some(resolver.clone()),
        ..RunConfig::default()
    };
    let journal = run(&exp, &config);
    let asked = resolver.asked.lock().unwrap().clone();
    (journal, asked)
}

#[test]
fn a_request_of_the_old_build_in_the_padding_does_not_mix_the_run() {
    // Ten requests of 1.5.1 inside the run; one of 1.5.0 a second before.
    let (journal, asked) = observed_by(
        vec![share("1.5.1", 10)],
        vec![share("1.5.1", 10), share("1.5.0", 1)],
    );
    assert_eq!(
        journal.target_version,
        Some(TargetVersion {
            version: "1.5.1".into(),
            version_source: VersionSource::Observed,
            shares: Vec::new(),
        })
    );
    assert_eq!(asked.len(), 1, "{asked:?}");
}

#[test]
fn the_padding_is_read_when_the_run_s_own_window_is_silent() {
    let skew_ns = i64::try_from(VERSION_WINDOW_SKEW.as_nanos()).unwrap();
    assert_eq!(skew_ns, 2_000_000_000);
    // A blank version and a version without requests name nothing.
    let nothing = vec![share(" ", 4), share("1.4.0", 0)];
    let (journal, asked) = observed_by(nothing, vec![share("1.5.0", 3)]);
    assert_eq!(
        journal
            .target_version
            .map(|v| (v.version, v.version_source)),
        Some(("1.5.0".into(), VersionSource::Observed))
    );
    let windows: Vec<(i64, i64)> = asked
        .iter()
        .map(|w| (w.started_at_ns, w.ended_at_ns))
        .collect();
    assert_eq!(
        windows,
        [
            (journal.started_at_ns, journal.ended_at_ns),
            (
                journal.started_at_ns - skew_ns,
                journal.ended_at_ns + skew_ns
            ),
        ]
    );
}

#[test]
fn a_version_seen_in_a_sliver_of_the_load_s_requests_is_not_recorded() {
    use super::load_tests::{load_result, ScriptedLoad};
    // The telemetry counted ten requests (see `answering`).
    for (sent, recorded) in [(4000, false), (1000, true), (100, true)] {
        let mut exp = super::load_tests::experiment_with_load();
        exp.target = Some(checkout(None));
        let resolver = ScriptedResolver::answering(Some("1.4.0"));
        let (load, _events) = ScriptedLoad::answering(Ok(load_result(sent, true)));
        let config = RunConfig {
            load_executor: Some(Arc::new(load)),
            ..config_with(&resolver)
        };
        let journal = run(&exp, &config);
        assert_eq!(journal.load_result.as_ref().unwrap().total_requests, sent);
        assert_eq!(journal.target_version.is_some(), recorded, "{sent} sent");
    }
}

#[test]
fn a_run_records_no_version_when_nothing_knows_it() {
    let mut exp = minimal_experiment();
    exp.target = Some(checkout(None));

    // Silent telemetry.
    let silent = ScriptedResolver::answering(None);
    assert_eq!(run(&exp, &config_with(&silent)).target_version, None);
    // The run's own window, then the padded one.
    assert_eq!(silent.questions().len(), 2);

    // A blank answer is no answer.
    let blank = ScriptedResolver::answering(Some("  "));
    assert_eq!(run(&exp, &config_with(&blank)).target_version, None);

    // Telemetry that cannot be read costs the version, not the run.
    let broken = ScriptedResolver::failing("lake unreachable");
    let journal = run(&exp, &config_with(&broken));
    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert_eq!(journal.target_version, None);

    // No resolver at all.
    assert_eq!(run(&exp, &RunConfig::default()).target_version, None);
}

#[test]
fn a_run_without_a_target_never_asks_for_a_version() {
    let exp = minimal_experiment();
    let resolver = ScriptedResolver::answering(Some("1.5.1"));

    let journal = run(&exp, &config_with(&resolver));

    assert_eq!(journal.target_version, None);
    assert!(resolver.questions().is_empty());
}

#[test]
fn a_run_aborted_by_its_hypothesis_still_observes_its_version() {
    let mut exp = experiment_with_hypothesis();
    exp.target = Some(checkout(None));
    let resolver = ScriptedResolver::answering(Some("1.5.1"));
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_fail());
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &controls,
        &config_with(&resolver),
        &fast_sampling(),
    )
    .unwrap();

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert_eq!(resolver.questions().len(), 1);
    assert_eq!(
        journal.target_version.map(|v| v.version_source),
        Some(VersionSource::Observed)
    );
}
