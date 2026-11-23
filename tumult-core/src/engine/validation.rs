//! Static validation of experiment definitions before execution.

use crate::types::{
    Activity, Experiment, LoadConfig, LoadProfile, LoadTool, Provider, TestKind, Tolerance,
};

use super::EngineError;

/// Validate an experiment definition before execution.
///
/// Checks: method is non-empty, regex patterns compile, hypothesis probes exist,
/// script providers name well-formed plugins/functions.
///
/// # Errors
///
/// Returns [`EngineError::UnsupportedVersion`] if the experiment version is not `"v1"`.
/// Returns [`EngineError::EmptyMethod`] if the method contains no steps.
/// Returns [`EngineError::EmptyHypothesisProbes`] if the hypothesis has no probes.
/// Returns [`EngineError::InvalidRegex`] if a regex tolerance pattern fails to compile.
/// Returns [`EngineError::InvalidToleranceBounds`] if a range tolerance has lower > upper.
/// Returns [`EngineError::InvalidMaxConcurrentFaults`] if `max_concurrent_faults` is 0.
/// Returns [`EngineError::InvalidScriptProvider`] if a script provider's plugin or
/// function is empty or contains path separators, `..`, null bytes, or whitespace.
/// This holds for every step: method, rollbacks, hypothesis probes, guards and controls.
/// Returns [`EngineError::InvalidSeconds`] if a timeout or a pause of any of those steps,
/// or the duration of a flat load, is not a number of seconds from 0 up to 2^64.
/// Returns [`EngineError::NfrInvalid`] if an objective names a metric no run can measure,
/// or a metric that does not measure the kind of the objective.
/// Returns [`EngineError::RecoveryInvalid`] if the `recovery` block asks for a window
/// that is not above 0 or longer than an hour, or for an interval longer than the window.
/// An RTO longer than the window is valid and logs `recovery.window.shorter_than_rto`.
/// Returns [`EngineError::LoadProfileInvalid`] if the load names a tool with no adapter,
/// an oha threshold the adapter does not check, or a profile that does not fit the run.
///
/// # Examples
///
/// ```
/// use tumult_core::engine::validate_experiment;
/// use tumult_core::types::*;
/// use std::collections::HashMap;
/// use indexmap::IndexMap;
///
/// let experiment = Experiment {
///     version: "v1".into(),
///     title: "validate-demo".into(),
///     description: None,
///     tags: vec![],
///     configuration: IndexMap::new(),
///     secrets: IndexMap::new(),
///     controls: vec![],
///     steady_state_hypothesis: None,
///     guards: vec![],
///     blast_radius: None,
///     max_concurrent_faults: None,
///     kind: Default::default(),
///     target: None,
///     nfr: vec![],
///     rto_target_s: None,
///     rpo_target_s: None,
///     recovery: None,
///     method: vec![Activity {
///         name: "action-1".into(),
///         activity_type: ActivityType::Action,
///         provider: Provider::Native {
///             plugin: "test".into(),
///             function: "noop".into(),
///             arguments: HashMap::new(),
///         },
///         tolerance: None,
///         pause_before_s: None,
///         pause_after_s: None,
///         background: false,
///         label_selector: None,
///     }],
///     rollbacks: vec![],
///     estimate: None,
///     baseline: None,
///     load: None,
///     regulatory: None,
/// };
///
/// assert!(validate_experiment(&experiment).is_ok());
///
/// // An experiment with no method steps fails validation
/// let empty = Experiment {
///     version: "v1".into(),
///     title: "empty".into(),
///     description: None,
///     tags: vec![],
///     configuration: IndexMap::new(),
///     secrets: IndexMap::new(),
///     controls: vec![],
///     steady_state_hypothesis: None,
///     guards: vec![],
///     blast_radius: None,
///     max_concurrent_faults: None,
///     kind: Default::default(),
///     target: None,
///     nfr: vec![],
///     rto_target_s: None,
///     rpo_target_s: None,
///     recovery: None,
///     method: vec![],
///     rollbacks: vec![],
///     estimate: None,
///     baseline: None,
///     load: None,
///     regulatory: None,
/// };
///
/// assert!(validate_experiment(&empty).is_err());
/// ```
pub fn validate_experiment(experiment: &Experiment) -> Result<(), EngineError> {
    // Version check — only "v1" is supported
    if experiment.version != "v1" {
        return Err(EngineError::UnsupportedVersion {
            version: experiment.version.clone(),
        });
    }

    if experiment.method.is_empty() {
        return Err(EngineError::EmptyMethod);
    }

    // A cap of 0 would make every background activity wait on the fault gate
    // forever; a set value must be at least 1.
    if experiment.max_concurrent_faults == Some(0) {
        return Err(EngineError::InvalidMaxConcurrentFaults);
    }

    validate_objectives(experiment)?;
    validate_recovery(experiment)?;
    validate_load(experiment)?;
    validate_target_version(experiment)?;

    // Validate hypothesis has probes if defined
    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        if hypothesis.probes.is_empty() {
            return Err(EngineError::EmptyHypothesisProbes {
                title: hypothesis.title.clone(),
            });
        }
    }

    // Validate guards: each guard's probe must carry a tolerance (the safe
    // condition), min_breaches must be at least 1, and any regex/range
    // tolerance must be well-formed.
    for guard in &experiment.guards {
        if guard.min_breaches == 0 {
            return Err(EngineError::GuardInvalidMinBreaches {
                guard: guard.name.clone(),
            });
        }
        match &guard.probe.tolerance {
            None => {
                return Err(EngineError::GuardMissingTolerance {
                    guard: guard.name.clone(),
                });
            }
            Some(Tolerance::Regex { pattern }) => {
                if regex_lite::Regex::new(pattern).is_err() {
                    return Err(EngineError::InvalidRegex {
                        activity: guard.name.clone(),
                        pattern: pattern.clone(),
                    });
                }
            }
            Some(Tolerance::Range { from, to }) if from > to => {
                return Err(EngineError::InvalidToleranceBounds {
                    activity: guard.name.clone(),
                    from: *from,
                    to: *to,
                });
            }
            Some(_) => {}
        }
        validate_script_provider(&guard.name, &guard.probe.provider)?;
        validate_step_seconds(&format!("guard '{}'", guard.name), &guard.probe)?;
    }

    for control in &experiment.controls {
        validate_script_provider(&control.name, &control.provider)?;
        validate_timeout(&format!("control '{}'", control.name), &control.provider)?;
    }

    // Validate all regex tolerance patterns compile, and every script
    // provider names a well-formed plugin and function.
    let all_activities = experiment
        .method
        .iter()
        .chain(experiment.rollbacks.iter())
        .chain(
            experiment
                .steady_state_hypothesis
                .as_ref()
                .map(|h| h.probes.iter())
                .into_iter()
                .flatten(),
        );
    for activity in all_activities {
        match &activity.tolerance {
            Some(Tolerance::Regex { pattern }) => {
                if regex_lite::Regex::new(pattern).is_err() {
                    return Err(EngineError::InvalidRegex {
                        activity: activity.name.clone(),
                        pattern: pattern.clone(),
                    });
                }
            }
            Some(Tolerance::Range { from, to }) if from > to => {
                return Err(EngineError::InvalidToleranceBounds {
                    activity: activity.name.clone(),
                    from: *from,
                    to: *to,
                });
            }
            _ => {}
        }

        validate_script_provider(&activity.name, &activity.provider)?;
        validate_step_seconds(&format!("activity '{}'", activity.name), activity)?;
    }

    Ok(())
}

/// The seconds of a step: its timeout and its pauses. `owner` names the
/// step in the error.
fn validate_step_seconds(owner: &str, activity: &Activity) -> Result<(), EngineError> {
    validate_timeout(owner, &activity.provider)?;
    validate_seconds(owner, "pause_before_s", activity.pause_before_s)?;
    validate_seconds(owner, "pause_after_s", activity.pause_after_s)
}

/// The timeout of a process or a script provider. A native provider has none.
fn validate_timeout(owner: &str, provider: &Provider) -> Result<(), EngineError> {
    let (Provider::Process { timeout_s, .. } | Provider::Script { timeout_s, .. }) = provider
    else {
        return Ok(());
    };
    validate_seconds(owner, "timeout_s", *timeout_s)
}

/// A number of seconds the runner waits on must be one a `Duration` can
/// hold: zero or more, finite, and below 2^64. Where the runner turns any
/// other value into a `Duration`, it panics or the wait is dropped.
fn validate_seconds(owner: &str, field: &str, seconds: Option<f64>) -> Result<(), EngineError> {
    let Some(value) = seconds else {
        return Ok(());
    };
    if std::time::Duration::try_from_secs_f64(value).is_ok() {
        return Ok(());
    }
    Err(EngineError::InvalidSeconds {
        owner: owner.to_string(),
        field: field.to_string(),
        value,
    })
}

/// Check the plugin and function names of a script provider. `owner` is the
/// step, guard or control the provider belongs to. Other providers pass.
fn validate_script_provider(owner: &str, provider: &Provider) -> Result<(), EngineError> {
    let Provider::Script {
        plugin, function, ..
    } = provider
    else {
        return Ok(());
    };
    validate_script_name(owner, "plugin", plugin)?;
    validate_script_name(owner, "function", function)
}

/// A script provider's `plugin`/`function` fields name a plugin directory
/// entry and a manifest action: they must be non-empty and carry no path
/// separators, `..` traversal, null bytes, or whitespace — anything else
/// could steer manifest lookup outside the plugin search paths.
fn validate_script_name(activity: &str, field: &str, value: &str) -> Result<(), EngineError> {
    let reject = |reason: &str| EngineError::InvalidScriptProvider {
        activity: activity.to_string(),
        reason: format!("{field} '{value}' {reason}"),
    };
    if value.is_empty() {
        return Err(EngineError::InvalidScriptProvider {
            activity: activity.to_string(),
            reason: format!("{field} must not be empty"),
        });
    }
    if value.contains('/') || value.contains('\\') {
        return Err(reject("must not contain path separators"));
    }
    if value.contains("..") {
        return Err(reject("must not contain '..'"));
    }
    if value.contains('\0') {
        return Err(reject("must not contain null bytes"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(reject("must not contain whitespace"));
    }
    Ok(())
}

/// Objectives and recovery targets: unique, well-formed objectives; finite
/// positive RTO/RPO; a `dr` run must declare its RTO.
fn validate_objectives(experiment: &Experiment) -> Result<(), EngineError> {
    let probes: Vec<&str> = experiment
        .steady_state_hypothesis
        .iter()
        .flat_map(|hypothesis| &hypothesis.probes)
        .map(|probe| probe.name.as_str())
        .collect();
    let mut seen = std::collections::HashSet::new();
    for o in &experiment.nfr {
        if o.id.trim().is_empty() {
            return Err(EngineError::NfrInvalid {
                id: o.id.clone(),
                reason: "id is empty".into(),
            });
        }
        if !seen.insert(o.id.as_str()) {
            return Err(EngineError::NfrDuplicateId { id: o.id.clone() });
        }
        if o.metric.trim().is_empty() {
            return Err(EngineError::NfrInvalid {
                id: o.id.clone(),
                reason: "metric is empty".into(),
            });
        }
        if let Some(reason) = crate::runner::nfr::metric_fault(&o.metric, &probes) {
            return Err(EngineError::NfrInvalid {
                id: o.id.clone(),
                reason,
            });
        }
        if let Some(reason) = crate::runner::nfr::kind_fault(o.kind, &o.metric) {
            return Err(EngineError::NfrInvalid {
                id: o.id.clone(),
                reason,
            });
        }
        if !o.threshold.is_finite() {
            return Err(EngineError::NfrInvalid {
                id: o.id.clone(),
                reason: "threshold must be finite".into(),
            });
        }
        if o.window_s.is_some_and(|w| !(w.is_finite() && w > 0.0)) {
            return Err(EngineError::NfrInvalid {
                id: o.id.clone(),
                reason: "window_s must be positive".into(),
            });
        }
    }
    for (field, value) in [
        ("rto_target_s", experiment.rto_target_s),
        ("rpo_target_s", experiment.rpo_target_s),
    ] {
        if value.is_some_and(|v| !(v.is_finite() && v > 0.0)) {
            return Err(EngineError::InvalidRecoveryTarget {
                field: field.into(),
            });
        }
    }
    if experiment.kind == TestKind::Dr && experiment.rto_target_s.is_none() {
        return Err(EngineError::DrRequiresRto);
    }
    Ok(())
}

/// The recovery window must be one the post phase can watch. An RTO longer
/// than the window is valid, but a run that recovers after the window and
/// within the RTO breaches it: validation logs a warning.
fn validate_recovery(experiment: &Experiment) -> Result<(), EngineError> {
    let recovery = experiment.recovery.unwrap_or_default();
    if let Some(reason) = recovery.invalid_reason() {
        return Err(EngineError::RecoveryInvalid { reason });
    }
    if let Some(rto_target_s) = experiment
        .rto_target_s
        .filter(|rto_target_s| *rto_target_s > recovery.window_s)
    {
        tracing::warn!(
            rto_target_s,
            window_s = recovery.window_s,
            "recovery.window.shorter_than_rto"
        );
    }
    Ok(())
}

/// A load profile must be realisable: it needs a duration, every length
/// must be finite and positive, and the shaped segments must fit inside
/// the duration. Checked here so an impossible shape fails at `validate`
/// rather than when the load tool starts.
/// A declared version is held to the bounds of every version string.
fn validate_target_version(experiment: &Experiment) -> Result<(), EngineError> {
    let Some(version) = experiment
        .target
        .as_ref()
        .and_then(|target| target.declared_version())
    else {
        return Ok(());
    };
    crate::types::check_version_text(version).map_err(|e| EngineError::TargetVersionInvalid {
        reason: e.to_string(),
    })
}

/// Check the regulatory block of an experiment as it is written: every
/// DORA requirement names a row or an article of the requirement registry
/// (in its registry id or an older form of it), and a tester has a name.
/// `tumult validate` runs it. A run does not: an experiment that names an
/// id the registry does not know still runs, as it did before the check.
///
/// # Errors
///
/// Returns [`EngineError::UnknownRequirement`] with the nearest registry id
/// if a DORA requirement is not in the registry, and
/// [`EngineError::RegulatoryInvalid`] if the tester has no name.
pub fn validate_regulatory(experiment: &Experiment) -> Result<(), EngineError> {
    let Some(mapping) = experiment.regulatory.as_ref() else {
        return Ok(());
    };
    for requirement in &mapping.requirements {
        crate::compliance::check_run_requirement(&mapping.frameworks, &requirement.id)
            .map_err(EngineError::UnknownRequirement)?;
    }
    if mapping
        .tester
        .as_ref()
        .is_some_and(|tester| tester.name.trim().is_empty())
    {
        return Err(EngineError::RegulatoryInvalid {
            reason: "tester.name must not be empty".into(),
        });
    }
    Ok(())
}

fn validate_load(experiment: &Experiment) -> Result<(), EngineError> {
    let Some(load) = experiment.load.as_ref() else {
        return Ok(());
    };
    // A tool without an adapter can never run: the runner's choice of
    // adapter decides it, and its reason is the one given here.
    if let Err(reason) = crate::runner::load_executor_for(&load.tool) {
        return Err(EngineError::LoadProfileInvalid { reason });
    }
    validate_thresholds(load)?;
    let invalid = |reason: &str| EngineError::LoadProfileInvalid {
        reason: reason.to_string(),
    };
    if load.vus == Some(0) {
        return Err(invalid("vus must be at least 1"));
    }
    let Some(profile) = load.profile.as_ref() else {
        return validate_seconds("load", "duration_s", load.duration_s);
    };
    let Some(duration_s) = load.duration_s else {
        return Err(invalid("a profile requires duration_s"));
    };
    if !(duration_s.is_finite() && duration_s > 0.0) {
        return Err(invalid("duration_s must be finite and positive"));
    }
    let positive = |name: &str, value: f64| -> Result<(), EngineError> {
        if value.is_finite() && value > 0.0 {
            return Ok(());
        }
        Err(invalid(&format!("{name} must be finite and positive")))
    };
    let vus = load.vus.unwrap_or(crate::runner::load_profile::DEFAULT_VUS);
    match profile {
        LoadProfile::Ramp {
            start_vus,
            ramp_up_s,
        } => {
            positive("ramp_up_s", *ramp_up_s)?;
            if *start_vus == 0 {
                return Err(invalid("start_vus must be at least 1"));
            }
            if *ramp_up_s > duration_s {
                return Err(invalid("ramp_up_s must not exceed duration_s"));
            }
            if *start_vus > vus {
                return Err(invalid("start_vus must not exceed vus"));
            }
        }
        LoadProfile::Spike {
            peak_vus,
            after_s,
            hold_s,
        } => {
            positive("hold_s", *hold_s)?;
            if *peak_vus == 0 {
                return Err(invalid("peak_vus must be at least 1"));
            }
            if !(after_s.is_finite() && *after_s >= 0.0) {
                return Err(invalid("after_s must be finite and non-negative"));
            }
            if after_s + hold_s > duration_s {
                return Err(invalid("after_s + hold_s must not exceed duration_s"));
            }
            if *peak_vus <= vus {
                return Err(invalid("peak_vus must exceed vus"));
            }
        }
        LoadProfile::Soak { sample_interval_s } => {
            positive("sample_interval_s", *sample_interval_s)?;
            if *sample_interval_s < crate::runner::load_profile::MIN_WINDOW_S {
                return Err(invalid(
                    "sample_interval_s must be at least 1 second: a shorter window measures too few requests",
                ));
            }
            if *sample_interval_s > duration_s {
                return Err(invalid("sample_interval_s must not exceed duration_s"));
            }
        }
    }
    Ok(())
}

/// k6 applies the thresholds of its script, not those of the file: they
/// stay valid and are said to be unused. oha checks the file's, and a run
/// with a name it does not check deviates, so such a name is refused.
fn validate_thresholds(load: &LoadConfig) -> Result<(), EngineError> {
    if load.thresholds.is_empty() {
        return Ok(());
    }
    if load.tool == LoadTool::K6 {
        tracing::warn!(
            tool = "k6",
            thresholds = load.thresholds.len(),
            "load.thresholds.not_applied"
        );
        return Ok(());
    }
    let Some(unknown) = load
        .thresholds
        .keys()
        .filter(|name| !crate::runner::oha::checks_threshold(name))
        .min()
    else {
        return Ok(());
    };
    Err(EngineError::LoadProfileInvalid {
        reason: format!("oha checks no threshold named '{unknown}'"),
    })
}

#[cfg(test)]
mod tests;
