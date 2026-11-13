//! The decisions behind a performance verdict, free of I/O: which version
//! a run measured, which environment a request is answered for, which two
//! versions are compared and what each side is made of.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use tumult_core::types::OBSERVED_VERSION_MIN_SHARE;
use tumult_lake::{version_cmp, VersionSighting, REDEPLOY_GAP};
use tumult_perf::Window;

/// An environment: `None` is the telemetry that names none.
pub(crate) type Env = Option<String>;

/// How old the target's last span before a run may be and still say what
/// served the run.
pub(crate) const STALE_NS: i64 = 3_600 * 1_000_000_000;
/// The share of its load windows a side may lose to silence and still be
/// compared.
pub(crate) const UNANSWERED_LIMIT: f64 = 0.10;
/// [`REDEPLOY_GAP`] in nanoseconds: how long a baseline may keep reporting
/// after the candidate came back and still have been replaced by it.
#[allow(clippy::cast_possible_wrap)]
pub(crate) const REDEPLOY_GAP_NS: i64 = REDEPLOY_GAP.as_secs() as i64 * 1_000_000_000;

/// `<n>` followed by `s`, `m`, `h`, `d` or `w` as seconds; `None` for
/// anything else, zero included.
pub(crate) fn parse_range(range: &str) -> Option<i64> {
    let per_unit = match range.chars().last()? {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        'w' => 7 * 86_400,
        _ => return None,
    };
    let digits = &range[..range.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    if n == 0 {
        return None;
    }
    n.checked_mul(per_unit)
}

/// Where a run's version comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attribution {
    /// The experiment or the command line named it.
    Declared,
    /// The runner read it from the target's spans when the run ended.
    Observed,
    /// Nothing was recorded: read from the target's spans now.
    Inferred,
}

impl Attribution {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Observed => "observed",
            Self::Inferred => "inferred",
        }
    }
}

/// Why a run has no version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Reason {
    /// The target never reported a version from the run's environment.
    NoTelemetryInEnvironment,
    /// It did, but neither inside the run's window nor shortly before.
    NoSpansNearRun,
    /// More than one version served the run's window and none of them
    /// nearly all of it: what the run measured is no single version.
    MixedVersions,
}

impl Reason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoTelemetryInEnvironment => "no_target_telemetry_in_environment",
            Self::NoSpansNearRun => "no_target_spans_near_run",
            Self::MixedVersions => "mixed_versions",
        }
    }
}

/// How a run's load was executed, as far as its result says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Execution {
    pub aborted: bool,
    pub truncated: bool,
    pub planned: Option<u64>,
    pub completed: Option<u64>,
    pub unanswered: u64,
    /// Windows skipped behind an unanswered one.
    pub lost: u64,
    /// The requests the load sent and the ones answered well; `None`
    /// when the tool counted none.
    pub requests_sent: Option<u64>,
    pub requests_ok: Option<u64>,
}

/// A run against the target.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Run {
    pub experiment_id: String,
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
    pub environment: Env,
    /// The version the run recorded, and whether it was declared (rather
    /// than observed by the runner).
    pub recorded: Option<(String, bool)>,
    /// The runner saw more than one version serve the run's window.
    pub mixed: bool,
    /// The journal's status, when the journal reached the lake.
    pub status: Option<String>,
    pub execution: Execution,
    /// `(version, requests)`: the requests the target served inside the
    /// run's window, in the run's environment. Read for runs that
    /// recorded no version.
    pub inside: Vec<(String, u64)>,
    /// `(version, last span)`: the target's spans before the run's start
    /// and no more than [`STALE_NS`] before it, in the run's environment.
    pub before: Vec<(String, i64)>,
}

/// A run and the version it measured, or why none is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attributed {
    pub run: Run,
    pub owner: Result<(String, Attribution), Reason>,
}

/// The version a run measured: the one it recorded; else the one that
/// served nearly all ([`OBSERVED_VERSION_MIN_SHARE`]) of the target's
/// requests inside its window; else, when the window is silent, the
/// version of the target's last span shortly before the run, ties to the
/// version deployed latest in the run's environment and then to the
/// greater version. A window two versions shared belongs to neither.
/// Only spans of exactly the run's environment count.
pub(crate) fn owner_of(
    sightings: &[VersionSighting],
    run: &Run,
) -> Result<(String, Attribution), Reason> {
    if run.mixed {
        return Err(Reason::MixedVersions);
    }
    if let Some((version, declared)) = &run.recorded {
        let how = if *declared {
            Attribution::Declared
        } else {
            Attribution::Observed
        };
        return Ok((version.clone(), how));
    }
    if let Some((version, served)) = run.inside.iter().max_by_key(|(_, served)| *served) {
        let all: u64 = run.inside.iter().map(|(_, served)| *served).sum();
        #[allow(clippy::cast_precision_loss)]
        let nearly_all = *served as f64 >= OBSERVED_VERSION_MIN_SHARE * all as f64;
        if !nearly_all {
            return Err(Reason::MixedVersions);
        }
        return Ok((version.clone(), Attribution::Inferred));
    }
    let deployed = |version: &str| {
        sightings
            .iter()
            .filter(|s| s.environment == run.environment && s.version == version)
            .map(|s| s.last_deployed_ns)
            .max()
            .unwrap_or(i64::MIN)
    };
    let last = run.before.iter().max_by(|(a, at_a), (b, at_b)| {
        at_a.cmp(at_b)
            .then_with(|| deployed(a).cmp(&deployed(b)))
            .then_with(|| version_cmp(a, b))
    });
    if let Some((version, _)) = last {
        return Ok((version.clone(), Attribution::Inferred));
    }
    if sightings.iter().any(|s| s.environment == run.environment) {
        return Err(Reason::NoSpansNearRun);
    }
    Err(Reason::NoTelemetryInEnvironment)
}

/// Every run with its owner.
pub(crate) fn attribute(sightings: &[VersionSighting], runs: Vec<Run>) -> Vec<Attributed> {
    runs.into_iter()
        .map(|run| Attributed {
            owner: owner_of(sightings, &run),
            run,
        })
        .collect()
}

/// A version of the target in one environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionSeen {
    pub version: String,
    pub first_seen_ns: i64,
    pub last_seen_ns: i64,
    /// When the version was last put into service: its first sight, or,
    /// when it came back after another version had taken over, the start
    /// of its latest return.
    pub last_deployed_ns: i64,
    /// Whether it came back after another version had taken over.
    pub redeployed: bool,
}

/// A version runs recorded in one environment, whenever they ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedVersion {
    pub environment: Env,
    pub version: String,
    /// The start of the earliest run that recorded it.
    pub first_ns: i64,
    /// The end of the latest.
    pub last_ns: i64,
}

/// Deployment order, latest first: by the latest deployment, then by
/// version.
fn deployment_order(a: &VersionSeen, b: &VersionSeen) -> Ordering {
    b.last_deployed_ns
        .cmp(&a.last_deployed_ns)
        .then_with(|| version_cmp(&b.version, &a.version))
}

fn as_seen(sighting: &VersionSighting) -> VersionSeen {
    let redeployed = sighting.deployments > 1;
    VersionSeen {
        version: sighting.version.clone(),
        first_seen_ns: sighting.first_seen_ns,
        last_seen_ns: sighting.last_seen_ns,
        last_deployed_ns: if redeployed {
            sighting.last_deployed_ns
        } else {
            sighting.first_seen_ns
        },
        redeployed,
    }
}

/// A version known only from a run: seen for the length of that run.
fn seen_for(version: &str, from_ns: i64, to_ns: i64) -> VersionSeen {
    VersionSeen {
        version: version.to_string(),
        first_seen_ns: from_ns,
        last_seen_ns: to_ns,
        last_deployed_ns: from_ns,
        redeployed: false,
    }
}

/// Stretch `seen` over a run that recorded its version. A version that
/// never came back was deployed when it was first seen, by a span or by
/// a run.
fn stretch(seen: &mut VersionSeen, from_ns: i64, to_ns: i64) {
    seen.first_seen_ns = seen.first_seen_ns.min(from_ns);
    seen.last_seen_ns = seen.last_seen_ns.max(to_ns);
    if !seen.redeployed {
        seen.last_deployed_ns = seen.first_seen_ns;
    }
}

/// The target's versions per environment, each list in deployment order,
/// latest first. A version is one the target's spans ever carried, one a
/// run ever recorded (`recorded`), or one a run of the lookback was read
/// to have measured; none of it depends on how far back the samples are
/// read. A version only runs know counts as seen for the length of those
/// runs.
pub(crate) fn versions_by_env(
    sightings: &[VersionSighting],
    recorded: &[RecordedVersion],
    runs: &[Attributed],
) -> BTreeMap<Env, Vec<VersionSeen>> {
    let mut seen: BTreeMap<(Env, String), VersionSeen> = BTreeMap::new();
    for s in sightings {
        seen.insert((s.environment.clone(), s.version.clone()), as_seen(s));
    }
    for r in recorded {
        seen.entry((r.environment.clone(), r.version.clone()))
            .and_modify(|entry| stretch(entry, r.first_ns, r.last_ns))
            .or_insert_with(|| seen_for(&r.version, r.first_ns, r.last_ns));
    }
    for a in runs {
        let Ok((version, how)) = &a.owner else {
            continue;
        };
        let (from_ns, to_ns) = (a.run.started_at_ns, a.run.ended_at_ns);
        let entry = seen
            .entry((a.run.environment.clone(), version.clone()))
            .or_insert_with(|| seen_for(version, from_ns, to_ns));
        if *how != Attribution::Inferred {
            stretch(entry, from_ns, to_ns);
        }
    }
    let mut by_env: BTreeMap<Env, Vec<VersionSeen>> = BTreeMap::new();
    for ((env, _), version) in seen {
        by_env.entry(env).or_default().push(version);
    }
    for versions in by_env.values_mut() {
        versions.sort_by(deployment_order);
    }
    by_env
}

/// Whether the order of `candidate` before `baseline` rests on a redeploy
/// that cannot be told from two builds running side by side: the
/// candidate came back, and the baseline kept reporting for longer than
/// [`REDEPLOY_GAP_NS`] after that.
pub(crate) fn redeploy_is_ambiguous(candidate: &VersionSeen, baseline: &VersionSeen) -> bool {
    candidate.redeployed
        && baseline.last_seen_ns > candidate.last_deployed_ns.saturating_add(REDEPLOY_GAP_NS)
}

/// Environments in a stable order: by name, the unnamed one last.
pub(crate) fn sorted_envs<'a>(envs: impl Iterator<Item = &'a Env>) -> Vec<Env> {
    let mut envs: Vec<Env> = envs.cloned().collect();
    envs.sort_by(|a, b| (a.is_none(), a).cmp(&(b.is_none(), b)));
    envs
}

/// Why no environment could be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoEnvironment {
    /// The target has no version anywhere the request may look.
    NoVersions,
    /// Several environments could answer, or none of several can.
    Ambiguous(Vec<Env>),
}

/// The one environment a request is answered for: the one it names;
/// else the only one in which the target has versions; else the only one
/// that can answer — it has seen every version in `wanted`, or, with
/// none wanted, has two versions to compare. Never a mix of two.
pub(crate) fn choose_environment(
    requested: Option<&str>,
    by_env: &BTreeMap<Env, Vec<VersionSeen>>,
    wanted: &[&str],
) -> Result<Env, NoEnvironment> {
    if let Some(env) = requested {
        let env = Some(env.to_string());
        if by_env.contains_key(&env) {
            return Ok(env);
        }
        return Err(NoEnvironment::NoVersions);
    }
    let all = sorted_envs(by_env.keys());
    match all.as_slice() {
        [] => return Err(NoEnvironment::NoVersions),
        [only] => return Ok(only.clone()),
        _ => {}
    }
    let can_answer = |versions: &[VersionSeen]| {
        if wanted.is_empty() {
            return versions.len() >= 2;
        }
        wanted
            .iter()
            .all(|w| versions.iter().any(|v| v.version == *w))
    };
    let able: Vec<&Env> = all.iter().filter(|env| can_answer(&by_env[*env])).collect();
    match able.as_slice() {
        [only] => Ok((*only).clone()),
        _ => Err(NoEnvironment::Ambiguous(all)),
    }
}

/// `(candidate, baseline)` among `versions` (deployment order, latest
/// first, not empty). Unnamed, the candidate is the latest version and
/// the baseline the one deployed just before the candidate; a named
/// baseline alone takes the latest other version as its candidate.
/// `Err` carries a named version the list does not hold.
pub(crate) fn pick<'a>(
    versions: &'a [VersionSeen],
    baseline: Option<&str>,
    candidate: Option<&str>,
) -> Result<(&'a VersionSeen, Option<&'a VersionSeen>), String> {
    let index = |name: &str| {
        versions
            .iter()
            .position(|v| v.version == name)
            .ok_or_else(|| name.to_string())
    };
    let baseline = baseline.map(index).transpose()?;
    let candidate = candidate.map(index).transpose()?;
    let (c, b) = match (candidate, baseline) {
        (Some(c), Some(b)) => (c, Some(b)),
        (Some(c), None) => (c, (c + 1 < versions.len()).then_some(c + 1)),
        (None, None) => (0, (versions.len() > 1).then_some(1)),
        (None, Some(b)) => match (0..versions.len()).find(|i| *i != b) {
            Some(c) => (c, Some(b)),
            None => (b, None),
        },
    };
    Ok((&versions[c], b.map(|b| &versions[b])))
}

/// One run of a side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunDetail {
    pub experiment_id: String,
    pub attribution: Attribution,
    pub status: String,
    pub truncated: bool,
    pub aborted: bool,
    /// Whether the run's samples are part of the comparison.
    pub included: bool,
}

/// How many of a side's runs were not whole, and how many were left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Partial {
    pub truncated: usize,
    pub aborted: usize,
    pub halted: usize,
    pub deviated: usize,
    pub excluded: usize,
}

/// The load windows of a side's included runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Windows {
    pub planned: u64,
    pub completed: u64,
    pub unanswered: u64,
    /// Windows skipped behind an unanswered one.
    pub lost: u64,
    /// Windows whose sample is a request given up on at its timeout: the
    /// true value is that or more.
    pub censored: u64,
}

/// The requests of all of a side's runs, those left out of the samples
/// too: a load cut short measured other latencies than the plan would
/// have, but a request it failed did fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Requests {
    pub sent: u64,
    /// Sent and not answered well: timed out, refused, or answered with
    /// an error.
    pub failed: u64,
    /// Runs whose tool counted no requests; they are in neither number.
    pub runs_without_counts: usize,
}

impl Requests {
    /// The share of the counted requests that failed; `None` when none
    /// were counted.
    pub(crate) fn failure_share(&self) -> Option<f64> {
        if self.sent == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some(self.failed as f64 / self.sent as f64)
    }

    fn add(&mut self, execution: &Execution) {
        let (Some(sent), Some(ok)) = (execution.requests_sent, execution.requests_ok) else {
            self.runs_without_counts += 1;
            return;
        };
        self.sent += sent;
        self.failed += sent.saturating_sub(ok);
    }
}

/// One sample of the requested series.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sample {
    /// The run it belongs to.
    pub run: String,
    pub value: f64,
    /// The seconds of load behind it; 1 when the run does not say.
    pub weight: f64,
    /// The value is a request timeout: the truth is that or more.
    pub censored: bool,
}

impl Windows {
    /// The windows the load reached or lost to silence.
    pub(crate) fn reached(&self) -> u64 {
        self.completed + self.lost
    }

    /// The share of them in which nothing answered; `None` when the runs
    /// counted no windows.
    pub(crate) fn unanswered_share(&self) -> Option<f64> {
        if self.reached() == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some(self.unanswered as f64 / self.reached() as f64)
    }
}

/// One side of the comparison.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Side {
    pub version: String,
    /// Every run that measured the version, in start order.
    pub details: Vec<RunDetail>,
    /// The samples of the included runs.
    pub samples: Vec<f64>,
    /// The same samples run by run, each with the seconds behind it; a
    /// run without samples is not listed.
    pub by_run: Vec<Vec<Window>>,
    pub partial: Partial,
    pub windows: Windows,
    pub requests: Requests,
    /// The least a censored sample reported: a quantile at or above it is
    /// a floor, not a measurement.
    pub censored_floor: Option<f64>,
}

impl Side {
    /// The runs whose samples are compared.
    pub(crate) fn runs(&self) -> Vec<&str> {
        self.details
            .iter()
            .filter(|d| d.included)
            .map(|d| d.experiment_id.as_str())
            .collect()
    }

    /// How many of the side's runs got their version that way.
    pub(crate) fn attributed(&self, how: Attribution) -> usize {
        self.details.iter().filter(|d| d.attribution == how).count()
    }
}

fn detail(a: &Attributed, how: Attribution, include_partial: bool) -> RunDetail {
    let execution = a.run.execution;
    RunDetail {
        experiment_id: a.run.experiment_id.clone(),
        attribution: how,
        status: a.run.status.clone().unwrap_or_else(|| "unknown".into()),
        truncated: execution.truncated,
        aborted: execution.aborted,
        included: include_partial || !(execution.truncated || execution.aborted),
    }
}

fn partial_of(details: &[RunDetail]) -> Partial {
    let count = |which: fn(&RunDetail) -> bool| details.iter().filter(|d| which(d)).count();
    Partial {
        truncated: count(|d| d.truncated),
        aborted: count(|d| d.aborted),
        halted: count(|d| d.status == "halted"),
        deviated: count(|d| d.status == "deviated"),
        excluded: count(|d| !d.included),
    }
}

fn add_windows(windows: &mut Windows, execution: &Execution) {
    windows.planned += execution.planned.unwrap_or(0);
    windows.completed += execution.completed.unwrap_or(0);
    windows.unanswered += execution.unanswered;
    windows.lost += execution.lost;
}

/// The samples of one run as windows, and what the censored ones say.
fn windows_of(id: &str, samples: &[Sample], side: &mut Side) {
    let mut run = Vec::new();
    for s in samples.iter().filter(|s| s.run == id) {
        run.push(Window {
            value: s.value,
            weight: s.weight,
        });
        side.samples.push(s.value);
        if !s.censored {
            continue;
        }
        side.windows.censored += 1;
        let floor = side.censored_floor.map_or(s.value, |f| f.min(s.value));
        side.censored_floor = Some(floor);
    }
    if !run.is_empty() {
        side.by_run.push(run);
    }
}

/// The side for `version` in `env`: the runs of that environment that
/// measured it. A run whose load was aborted or truncated is counted but
/// its samples and windows stay out, unless `include_partial`; its
/// requests are counted either way.
pub(crate) fn side(
    version: &str,
    env: &Env,
    runs: &[Attributed],
    samples: &[Sample],
    include_partial: bool,
) -> Side {
    let mut side = Side {
        version: version.to_string(),
        ..Side::default()
    };
    for a in runs.iter().filter(|a| &a.run.environment == env) {
        let Ok((owner, how)) = &a.owner else {
            continue;
        };
        if owner != version {
            continue;
        }
        let d = detail(a, *how, include_partial);
        side.requests.add(&a.run.execution);
        if d.included {
            add_windows(&mut side.windows, &a.run.execution);
            windows_of(&d.experiment_id, samples, &mut side);
        }
        side.details.push(d);
    }
    side.partial = partial_of(&side.details);
    side
}

/// What a side that lost too many windows to silence says about it;
/// `None` when it lost no more than [`UNANSWERED_LIMIT`].
pub(crate) fn unanswered_reason(role: &str, side: &Side) -> Option<String> {
    let share = side.windows.unanswered_share()?;
    if share <= UNANSWERED_LIMIT {
        return None;
    }
    Some(format!(
        "{role} {}: {} of {} load windows went unanswered ({:.0}%), more than the {:.0}% a \
         side may lose; the latencies that were measured leave out the time nothing answered",
        side.version,
        side.windows.unanswered,
        side.windows.reached(),
        share * 100.0,
        UNANSWERED_LIMIT * 100.0
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn sighting(env: Option<&str>, version: &str, first: i64, last: i64) -> VersionSighting {
        VersionSighting {
            environment: env.map(str::to_string),
            version: version.into(),
            first_seen_ns: first,
            last_seen_ns: last,
            last_deployed_ns: first,
            deployments: 1,
            sanitized: false,
        }
    }

    fn demo(version: &str, first: i64, last: i64) -> VersionSighting {
        sighting(Some("demo"), version, first, last)
    }

    fn run(id: &str, env: Option<&str>) -> Run {
        Run {
            experiment_id: id.into(),
            environment: env.map(str::to_string),
            ..Run::default()
        }
    }

    fn served(id: &str, inside: &[(&str, u64)], before: &[(&str, i64)]) -> Run {
        Run {
            inside: inside.iter().map(|(v, n)| ((*v).to_string(), *n)).collect(),
            before: before.iter().map(|(v, t)| ((*v).to_string(), *t)).collect(),
            ..run(id, Some("demo"))
        }
    }

    fn seen(version: &str, first: i64) -> VersionSeen {
        VersionSeen {
            version: version.into(),
            first_seen_ns: first,
            last_seen_ns: first,
            last_deployed_ns: first,
            redeployed: false,
        }
    }

    fn recorded(version: &str, first: i64, last: i64) -> RecordedVersion {
        RecordedVersion {
            environment: Some("demo".into()),
            version: version.into(),
            first_ns: first,
            last_ns: last,
        }
    }

    fn sample(id: &str, value: f64) -> Sample {
        Sample {
            run: id.into(),
            value,
            weight: 1.0,
            censored: false,
        }
    }

    fn names(versions: &[VersionSeen]) -> Vec<&str> {
        versions.iter().map(|v| v.version.as_str()).collect()
    }

    #[test]
    fn a_range_is_a_positive_count_of_a_known_unit() {
        assert_eq!(parse_range("30d"), Some(30 * 86_400));
        assert_eq!(parse_range("24h"), Some(86_400));
        assert_eq!(parse_range("90s"), Some(90));
        assert_eq!(parse_range("15m"), Some(900));
        assert_eq!(parse_range("2w"), Some(14 * 86_400));
        for bad in [
            "", "d", "0d", "-1d", "1.5h", "7 d", "7y", "forever", "+7d", "7dd",
        ] {
            assert_eq!(parse_range(bad), None, "{bad}");
        }
        assert_eq!(parse_range("99999999999999999w"), None);
    }

    #[test]
    fn a_recorded_version_is_believed_before_any_span() {
        let sightings = [demo("1.0.0", 0, 100 * S)];
        let mut recorded = served("r", &[("1.0.0", 9)], &[("1.0.0", 5)]);
        recorded.recorded = Some(("1.1.0".into(), true));
        assert_eq!(
            owner_of(&sightings, &recorded),
            Ok(("1.1.0".into(), Attribution::Declared))
        );
        recorded.recorded = Some(("1.1.0".into(), false));
        assert_eq!(
            owner_of(&sightings, &recorded),
            Ok(("1.1.0".into(), Attribution::Observed))
        );
    }

    #[test]
    fn an_unrecorded_run_is_read_from_the_window_then_from_shortly_before() {
        let sightings = [demo("1.0.0", 0, 100 * S), demo("1.1.0", 50 * S, 90 * S)];
        let inferred = |version: &str| Ok((version.to_string(), Attribution::Inferred));
        // Nearly every request inside the window; what spoke before does
        // not matter.
        let most = served("a", &[("1.0.0", 97), ("1.1.0", 3)], &[("1.1.0", 40 * S)]);
        assert_eq!(owner_of(&sightings, &most), inferred("1.0.0"));
        // A silent window: the version that spoke last before the run.
        let quiet = served("c", &[], &[("1.0.0", 60 * S), ("1.1.0", 40 * S)]);
        assert_eq!(owner_of(&sightings, &quiet), inferred("1.0.0"));
        // The same instant: deployed latest.
        let tie = served("d", &[], &[("1.0.0", 60 * S), ("1.1.0", 60 * S)]);
        assert_eq!(owner_of(&sightings, &tie), inferred("1.1.0"));
    }

    #[test]
    fn a_window_two_versions_shared_belongs_to_neither() {
        let sightings = [demo("1.0.0", 0, 100 * S), demo("1.1.0", 50 * S, 90 * S)];
        // Seven requests in ten is most of them and still a mix: what the
        // run measured is neither version.
        for inside in [
            [("1.0.0", 7), ("1.1.0", 3)],
            [("1.0.0", 5), ("1.1.0", 5)],
            [("1.0.0", 94), ("1.1.0", 6)],
        ] {
            let shared = served("m", &inside, &[("1.0.0", 40 * S)]);
            assert_eq!(
                owner_of(&sightings, &shared),
                Err(Reason::MixedVersions),
                "{inside:?}"
            );
        }
        let nearly = served("n", &[("1.0.0", 95), ("1.1.0", 5)], &[]);
        assert_eq!(
            owner_of(&sightings, &nearly),
            Ok(("1.0.0".to_string(), Attribution::Inferred))
        );
        // A run whose runner saw the mix and recorded it stays out too,
        // whatever version it wrote beside it.
        let mut frozen = served("f", &[("1.1.0", 9)], &[]);
        frozen.recorded = Some(("1.1.0".into(), false));
        frozen.mixed = true;
        assert_eq!(owner_of(&sightings, &frozen), Err(Reason::MixedVersions));
        assert_eq!(Reason::MixedVersions.as_str(), "mixed_versions");
    }

    #[test]
    fn a_run_nothing_speaks_for_says_why() {
        let sightings = [demo("1.0.0", 0, 100 * S)];
        let silent = served("a", &[], &[]);
        assert_eq!(owner_of(&sightings, &silent), Err(Reason::NoSpansNearRun));
        // Demo's telemetry says nothing about a run in another environment,
        // or about one that names none.
        for env in [Some("prod"), None] {
            assert_eq!(
                owner_of(&sightings, &run("b", env)),
                Err(Reason::NoTelemetryInEnvironment),
                "{env:?}"
            );
        }
        let untagged = [sighting(None, "3.0.0", 0, S)];
        assert_eq!(
            owner_of(&untagged, &run("c", Some("demo"))),
            Err(Reason::NoTelemetryInEnvironment)
        );
        assert_eq!(
            owner_of(&untagged, &run("d", None)),
            Err(Reason::NoSpansNearRun)
        );
    }

    #[test]
    fn versions_are_listed_per_environment_in_deployment_order() {
        let sightings = [
            demo("1.8.0", 10 * S, 500 * S),
            demo("1.9.0", 20 * S, 500 * S),
            demo("1.10.0", 20 * S, 21 * S),
            demo("0.1.0", 0, 5 * S),
            sighting(Some("prod"), "1.8.0", 400 * S, 500 * S),
            sighting(None, "3.0.0", 0, 500 * S),
        ];
        let by_env = versions_by_env(&sightings, &[], &[]);
        // The same first sight: the greater version number is the later.
        // A version last seen long ago is still one that was deployed.
        assert_eq!(
            names(&by_env[&Some("demo".to_string())]),
            vec!["1.10.0", "1.9.0", "1.8.0", "0.1.0"]
        );
        assert_eq!(names(&by_env[&Some("prod".to_string())]), vec!["1.8.0"]);
        assert_eq!(names(&by_env[&None]), vec!["3.0.0"]);
        // The last sight of a version does not move it.
        let later = [
            demo("1.8.0", 10 * S, 9_000 * S),
            demo("1.9.0", 20 * S, 500 * S),
        ];
        let by_env = versions_by_env(&later, &[], &[]);
        assert_eq!(
            names(&by_env[&Some("demo".to_string())]),
            vec!["1.9.0", "1.8.0"]
        );
    }

    #[test]
    fn a_version_deployed_again_is_the_latest_and_says_so() {
        let rolled_back = VersionSighting {
            last_deployed_ns: 3_000 * S,
            deployments: 2,
            ..demo("1.8.0", 10 * S, 9_000 * S)
        };
        let sightings = [rolled_back, demo("1.9.0", 20 * S, 2_000 * S)];
        let by_env = versions_by_env(&sightings, &[], &[]);
        let versions = &by_env[&Some("demo".to_string())];
        assert_eq!(names(versions), vec!["1.8.0", "1.9.0"]);
        assert!(versions[0].redeployed);
        assert_eq!(versions[0].first_seen_ns, 10 * S);
        assert_eq!(versions[0].last_deployed_ns, 3_000 * S);
        assert!(!versions[1].redeployed);
        assert_eq!(versions[1].last_deployed_ns, versions[1].first_seen_ns);
        // The baseline went quiet before the rollback: the order is plain.
        assert!(!redeploy_is_ambiguous(&versions[0], &versions[1]));
    }

    #[test]
    fn a_redeployed_candidate_beside_a_live_baseline_cannot_be_ordered() {
        let again = VersionSeen {
            last_seen_ns: 9_000 * S,
            last_deployed_ns: 3_000 * S,
            redeployed: true,
            ..seen("1.8.0", 10 * S)
        };
        let live = VersionSeen {
            last_seen_ns: 9_000 * S,
            ..seen("1.9.0", 20 * S)
        };
        // Both versions kept reporting long after the old one came back:
        // two builds side by side, and no telling which is the newer.
        assert!(redeploy_is_ambiguous(&again, &live));
        // A baseline that trailed off as the candidate returned is not.
        let drained = VersionSeen {
            last_seen_ns: 3_000 * S + REDEPLOY_GAP_NS,
            ..live.clone()
        };
        assert!(!redeploy_is_ambiguous(&again, &drained));
        // Without a redeploy, two live versions are a canary in order.
        let fresh = VersionSeen {
            redeployed: false,
            ..again
        };
        assert!(!redeploy_is_ambiguous(&fresh, &live));
    }

    #[test]
    fn a_version_only_a_run_recorded_is_a_version_of_its_environment() {
        let sightings = [demo("1.0.0", 0, 100 * S), demo("0.9.0", 0, S)];
        // Whenever the runs were: the list does not follow the lookback.
        let declared = [
            recorded("2.0.0", 200 * S, 260 * S),
            recorded("0.5.0", -100 * S, -40 * S),
            // A run that declared 0.9.0 before any span of it was kept.
            recorded("0.9.0", -50 * S, 10 * S),
        ];
        let by_env = versions_by_env(&sightings, &declared, &[]);
        let demo_versions = &by_env[&Some("demo".to_string())];
        assert_eq!(
            names(demo_versions),
            vec!["2.0.0", "1.0.0", "0.9.0", "0.5.0"]
        );
        assert_eq!(demo_versions[0].first_seen_ns, 200 * S);
        assert_eq!(demo_versions[0].last_seen_ns, 260 * S);
        assert_eq!(demo_versions[2].first_seen_ns, -50 * S);
        assert_eq!(demo_versions[2].last_deployed_ns, -50 * S);
        assert_eq!(demo_versions[2].last_seen_ns, 10 * S);
        assert!(!demo_versions[2].redeployed);
        // A run read from spans the rollup does not hold yet keeps its
        // version in the list, for the length of the run.
        let mut early = served("o", &[("0.4.0", 4)], &[]);
        early.started_at_ns = 300 * S;
        early.ended_at_ns = 360 * S;
        let runs = attribute(&sightings, vec![early]);
        let by_env = versions_by_env(&sightings, &[], &runs);
        let demo_versions = &by_env[&Some("demo".to_string())];
        assert_eq!(names(demo_versions), vec!["0.4.0", "1.0.0", "0.9.0"]);
        assert_eq!(demo_versions[0].first_seen_ns, 300 * S);
    }

    #[test]
    fn a_request_is_answered_for_exactly_one_environment() {
        let demo_env = Some("demo".to_string());
        let prod_env = Some("prod".to_string());
        let mut by_env = BTreeMap::new();
        assert_eq!(
            choose_environment(None, &by_env, &[]),
            Err(NoEnvironment::NoVersions)
        );
        by_env.insert(demo_env.clone(), vec![seen("1.1.0", 2), seen("1.0.0", 1)]);
        assert_eq!(choose_environment(None, &by_env, &[]), Ok(demo_env.clone()));
        assert_eq!(
            choose_environment(Some("prod"), &by_env, &[]),
            Err(NoEnvironment::NoVersions)
        );
        // Prod has one version: only demo can compare.
        by_env.insert(prod_env.clone(), vec![seen("1.0.0", 1)]);
        assert_eq!(choose_environment(None, &by_env, &[]), Ok(demo_env.clone()));
        assert_eq!(
            choose_environment(Some("prod"), &by_env, &[]),
            Ok(prod_env.clone())
        );
        // Both can: the request has to say.
        by_env.insert(prod_env.clone(), vec![seen("1.2.0", 3), seen("1.0.0", 1)]);
        let both = NoEnvironment::Ambiguous(vec![demo_env.clone(), prod_env.clone()]);
        assert_eq!(choose_environment(None, &by_env, &[]), Err(both.clone()));
        assert_eq!(
            choose_environment(None, &by_env, &["1.0.0", "1.1.0"]),
            Ok(demo_env)
        );
        assert_eq!(choose_environment(None, &by_env, &["1.2.0"]), Ok(prod_env));
        assert_eq!(
            choose_environment(None, &by_env, &["1.0.0"]),
            Err(both.clone())
        );
        assert_eq!(choose_environment(None, &by_env, &["9.9.9"]), Err(both));
        // The unnamed environment is listed last.
        by_env.insert(None, vec![seen("3.0.0", 1)]);
        let Err(NoEnvironment::Ambiguous(all)) = choose_environment(None, &by_env, &[]) else {
            panic!("three environments, two of them comparable");
        };
        assert_eq!(all.last(), Some(&None));
    }

    #[test]
    fn candidate_and_baseline_follow_deployment_order_unless_named() {
        let versions = [seen("1.10.0", 3), seen("1.9.0", 2), seen("1.8.0", 1)];
        let picked = |baseline: Option<&str>, candidate: Option<&str>| {
            pick(&versions, baseline, candidate)
                .map(|(c, b)| (c.version.clone(), b.map(|b| b.version.clone())))
        };
        let pair = |c: &str, b: Option<&str>| Ok((c.to_string(), b.map(str::to_string)));
        assert_eq!(picked(None, None), pair("1.10.0", Some("1.9.0")));
        assert_eq!(picked(None, Some("1.9.0")), pair("1.9.0", Some("1.8.0")));
        assert_eq!(picked(None, Some("1.8.0")), pair("1.8.0", None));
        assert_eq!(picked(Some("1.8.0"), None), pair("1.10.0", Some("1.8.0")));
        assert_eq!(picked(Some("1.10.0"), None), pair("1.9.0", Some("1.10.0")));
        assert_eq!(
            picked(Some("1.10.0"), Some("1.8.0")),
            pair("1.8.0", Some("1.10.0"))
        );
        assert_eq!(picked(None, Some("2.0.0")), Err("2.0.0".to_string()));
        assert_eq!(picked(Some("0.1.0"), None), Err("0.1.0".to_string()));
        let one = [seen("1.0.0", 1)];
        let (candidate, baseline) = pick(&one, None, None).expect("one version");
        assert_eq!((candidate.version.as_str(), baseline), ("1.0.0", None));
        let (candidate, baseline) = pick(&one, Some("1.0.0"), None).expect("one version");
        assert_eq!((candidate.version.as_str(), baseline), ("1.0.0", None));
    }

    fn measured(id: &str, env: Option<&str>, version: &str, execution: Execution) -> Attributed {
        Attributed {
            run: Run {
                status: Some("completed".into()),
                execution,
                ..run(id, env)
            },
            owner: Ok((version.into(), Attribution::Inferred)),
        }
    }

    fn windows(planned: u64, completed: u64, unanswered: u64, lost: u64) -> Execution {
        Execution {
            planned: Some(planned),
            completed: Some(completed),
            unanswered,
            lost,
            truncated: completed + lost < planned,
            ..Execution::default()
        }
    }

    fn counted(sent: u64, ok: u64) -> Execution {
        Execution {
            requests_sent: Some(sent),
            requests_ok: Some(ok),
            ..windows(10, 10, 0, 0)
        }
    }

    #[test]
    fn a_side_holds_the_runs_of_its_version_in_its_environment_only() {
        let whole = windows(10, 10, 0, 0);
        let mut halted = measured("halted", Some("demo"), "1.1.0", windows(10, 4, 0, 0));
        halted.run.execution.aborted = true;
        halted.run.status = Some("halted".into());
        let mut deviated = measured("deviated", Some("demo"), "1.1.0", whole);
        deviated.run.status = Some("deviated".into());
        let runs = vec![
            measured("whole", Some("demo"), "1.1.0", whole),
            halted,
            deviated,
            measured("elsewhere", Some("prod"), "1.1.0", whole),
            measured("untagged", None, "1.1.0", whole),
            measured("other", Some("demo"), "1.0.0", whole),
            Attributed {
                run: run("unknown", Some("demo")),
                owner: Err(Reason::NoSpansNearRun),
            },
        ];
        let samples: Vec<Sample> = [
            ("whole", 1.0),
            ("halted", 2.0),
            ("elsewhere", 3.0),
            ("deviated", 4.0),
            ("other", 5.0),
            ("untagged", 6.0),
            ("unknown", 7.0),
            ("whole", 8.0),
        ]
        .iter()
        .map(|(id, value)| sample(id, *value))
        .collect();
        let demo_env = Some("demo".to_string());

        let s = side("1.1.0", &demo_env, &runs, &samples, false);
        assert_eq!(s.runs(), vec!["whole", "deviated"]);
        assert_eq!(s.samples, vec![1.0, 8.0, 4.0]);
        // The windows stay with their run, in the run's order.
        assert_eq!(
            s.by_run,
            vec![
                vec![Window::whole(1.0), Window::whole(8.0)],
                vec![Window::whole(4.0)],
            ]
        );
        assert_eq!(s.details.len(), 3);
        assert_eq!(
            s.partial,
            Partial {
                truncated: 1,
                aborted: 1,
                halted: 1,
                deviated: 1,
                excluded: 1
            }
        );
        assert_eq!(s.windows.planned, 20);
        assert_eq!(s.attributed(Attribution::Inferred), 3);
        assert_eq!(s.attributed(Attribution::Declared), 0);

        let s = side("1.1.0", &demo_env, &runs, &samples, true);
        assert_eq!(s.runs(), vec!["whole", "halted", "deviated"]);
        assert_eq!(s.samples, vec![1.0, 8.0, 2.0, 4.0]);
        assert_eq!(s.partial.excluded, 0);
        assert_eq!(s.windows.completed, 24);

        let s = side("1.1.0", &None, &runs, &samples, false);
        assert_eq!(s.runs(), vec!["untagged"]);
    }

    #[test]
    fn a_side_adds_up_its_requests_and_its_censored_windows() {
        let runs = [
            measured("a", None, "1.1.0", counted(1000, 900)),
            measured("b", None, "1.1.0", counted(3000, 2700)),
            // A tool that counted nothing: unknown, not zero failures.
            measured("silent", None, "1.1.0", windows(10, 10, 0, 0)),
        ];
        let samples = vec![
            sample("a", 20.0),
            Sample {
                censored: true,
                weight: 0.5,
                ..sample("a", 2000.0)
            },
            Sample {
                censored: true,
                ..sample("b", 1500.0)
            },
            sample("b", 30.0),
        ];
        let s = side("1.1.0", &None, &runs, &samples, false);
        assert_eq!(
            s.requests,
            Requests {
                sent: 4000,
                failed: 400,
                runs_without_counts: 1
            }
        );
        assert_eq!(s.requests.failure_share(), Some(0.1));
        assert_eq!(s.windows.censored, 2);
        // The least a censored window reported: every quantile at or
        // above it is a floor, not a measurement.
        assert_eq!(s.censored_floor, Some(1500.0));
        assert_eq!(
            s.by_run[0][1],
            Window {
                value: 2000.0,
                weight: 0.5
            }
        );
        let none = side("1.1.0", &None, &runs[2..3], &[], false);
        assert_eq!(none.requests.failure_share(), None);
        assert_eq!(none.censored_floor, None);
    }

    /// Requests that hang make the windows run over and the plan fall
    /// short. The run's latencies stay out; the requests it failed are
    /// the evidence it does give.
    #[test]
    fn a_run_cut_short_still_counts_its_requests() {
        let mut short = measured("short", None, "1.1.0", counted(900, 100));
        short.run.execution.truncated = true;
        let mut cut = measured("cut", None, "1.1.0", counted(100, 100));
        cut.run.execution.aborted = true;
        let runs = [measured("a", None, "1.1.0", counted(1000, 900)), short, cut];
        let samples = vec![sample("a", 20.0), sample("short", 30.0)];
        for include_partial in [false, true] {
            let s = side("1.1.0", &None, &runs, &samples, include_partial);
            assert_eq!(
                s.requests,
                Requests {
                    sent: 2000,
                    failed: 900,
                    runs_without_counts: 0
                }
            );
        }
        let s = side("1.1.0", &None, &runs, &samples, false);
        assert_eq!(s.runs(), vec!["a"]);
        assert_eq!(s.samples, vec![20.0]);
    }

    #[test]
    fn a_side_that_lost_too_many_windows_to_silence_is_not_comparable() {
        let side_of = |execution| {
            let runs = [measured("r", None, "1.1.0", execution)];
            side("1.1.0", &None, &runs, &[], false)
        };
        let quiet = side_of(windows(40, 36, 10, 4));
        assert_eq!(quiet.windows.unanswered_share(), Some(0.25));
        let reason = unanswered_reason("candidate", &quiet).expect("a quarter is too many");
        assert!(reason.contains("candidate 1.1.0"), "{reason}");
        assert!(reason.contains("10 of 40"), "{reason}");
        assert!(reason.contains("unanswered"), "{reason}");
        // One in ten is the limit, not over it.
        assert_eq!(
            unanswered_reason("baseline", &side_of(windows(40, 40, 4, 0))),
            None
        );
        assert!(unanswered_reason("baseline", &side_of(windows(40, 40, 5, 0))).is_some());
        // A tool that counts no windows says nothing either way.
        let uncounted = side_of(Execution::default());
        assert_eq!(uncounted.windows.unanswered_share(), None);
        assert_eq!(unanswered_reason("baseline", &uncounted), None);
    }
}
