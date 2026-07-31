//! Error type produced by the experiment engine.

use thiserror::Error;

/// Errors produced while validating or preparing an experiment for execution.
#[derive(Error, Debug)]
pub enum EngineError {
    /// The experiment's method section contains no steps.
    #[error("experiment has no method steps")]
    EmptyMethod,
    /// A configuration key references an environment variable that is not set.
    #[error("configuration key '{key}' references env var '{env_key}' which is not set")]
    ConfigResolutionFailed { key: String, env_key: String },
    /// A secret references an environment variable that is not set.
    #[error("secret '{group}.{key}' references env var '{env_key}' which is not set")]
    SecretResolutionFailed {
        group: String,
        key: String,
        env_key: String,
    },
    /// A secret references a file that does not exist.
    #[error("secret '{group}.{key}' references file '{path}' which does not exist")]
    SecretFileNotFound {
        group: String,
        key: String,
        path: String,
    },
    /// The experiment file could not be parsed.
    #[error("experiment file parse error: {0}")]
    ParseError(String),
    /// A tolerance regex pattern failed to compile.
    #[error("invalid regex pattern in activity '{activity}': {pattern}")]
    InvalidRegex { activity: String, pattern: String },
    /// A range tolerance has a lower bound greater than its upper bound.
    #[error(
        "invalid tolerance range in activity '{activity}': lower ({from}) must be <= upper ({to})"
    )]
    InvalidToleranceBounds {
        activity: String,
        from: f64,
        to: f64,
    },
    /// A steady-state hypothesis declares no probes.
    #[error("hypothesis '{title}' has no probes defined")]
    EmptyHypothesisProbes { title: String },
    /// The experiment declares a schema version this engine does not support.
    #[error("unsupported experiment version '{version}' (supported: v1)")]
    UnsupportedVersion { version: String },
    /// A guard probe has no tolerance; the tolerance defines the safe
    /// condition whose breach halts the experiment.
    #[error(
        "guard '{guard}' has no tolerance; a guard's tolerance defines the safe \
         condition (breach ⇒ halt)"
    )]
    GuardMissingTolerance { guard: String },
    /// A guard's `min_breaches` is 0; it must be at least 1.
    #[error("guard '{guard}' has min_breaches = 0; it must be at least 1")]
    GuardInvalidMinBreaches { guard: String },
    /// `max_concurrent_faults` was set to 0, which would block every
    /// background activity forever.
    #[error(
        "max_concurrent_faults must be at least 1 when set (0 would block every \
         background activity forever)"
    )]
    InvalidMaxConcurrentFaults,
    /// The experiment template references variables that were never defined.
    #[error("experiment template references undefined variables: {names}")]
    UndefinedVars { names: String },
    /// A script provider reference is invalid (see `reason`).
    #[error("invalid script provider in activity '{activity}': {reason}")]
    InvalidScriptProvider { activity: String, reason: String },
}
