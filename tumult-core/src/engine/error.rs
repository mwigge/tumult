//! Error type produced by the experiment engine.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("experiment has no method steps")]
    EmptyMethod,
    #[error("configuration key '{key}' references env var '{env_key}' which is not set")]
    ConfigResolutionFailed { key: String, env_key: String },
    #[error("secret '{group}.{key}' references env var '{env_key}' which is not set")]
    SecretResolutionFailed {
        group: String,
        key: String,
        env_key: String,
    },
    #[error("secret '{group}.{key}' references file '{path}' which does not exist")]
    SecretFileNotFound {
        group: String,
        key: String,
        path: String,
    },
    #[error("experiment file parse error: {0}")]
    ParseError(String),
    #[error("invalid regex pattern in activity '{activity}': {pattern}")]
    InvalidRegex { activity: String, pattern: String },
    #[error(
        "invalid tolerance range in activity '{activity}': lower ({from}) must be <= upper ({to})"
    )]
    InvalidToleranceBounds {
        activity: String,
        from: f64,
        to: f64,
    },
    #[error("hypothesis '{title}' has no probes defined")]
    EmptyHypothesisProbes { title: String },
    #[error("unsupported experiment version '{version}' (supported: v1)")]
    UnsupportedVersion { version: String },
    #[error(
        "guard '{guard}' has no tolerance; a guard's tolerance defines the safe \
         condition (breach ⇒ halt)"
    )]
    GuardMissingTolerance { guard: String },
    #[error("guard '{guard}' has min_breaches = 0; it must be at least 1")]
    GuardInvalidMinBreaches { guard: String },
    #[error("experiment template references undefined variable '${{{{ {name} }}}}'")]
    UndefinedVar { name: String },
}
