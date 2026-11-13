//! Static validation of experiment definitions before execution.

use crate::types::{Experiment, LoadProfile, TestKind, Tolerance};

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
    validate_load(experiment)?;

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

        if let crate::types::Provider::Script {
            plugin, function, ..
        } = &activity.provider
        {
            validate_script_name(&activity.name, "plugin", plugin)?;
            validate_script_name(&activity.name, "function", function)?;
        }
    }

    Ok(())
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

/// A load profile must be realisable: it needs a duration, every length
/// must be finite and positive, and the shaped segments must fit inside
/// the duration. Checked here so an impossible shape fails at `validate`
/// rather than when the load tool starts.
fn validate_load(experiment: &Experiment) -> Result<(), EngineError> {
    let Some(load) = experiment.load.as_ref() else {
        return Ok(());
    };
    let invalid = |reason: &str| EngineError::LoadProfileInvalid {
        reason: reason.to_string(),
    };
    if load.vus == Some(0) {
        return Err(invalid("vus must be at least 1"));
    }
    let Some(profile) = load.profile.as_ref() else {
        return Ok(());
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
            if *sample_interval_s > duration_s {
                return Err(invalid("sample_interval_s must not exceed duration_s"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
