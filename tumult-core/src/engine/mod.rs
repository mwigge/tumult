//! Experiment engine — orchestrates the five-phase execution lifecycle.

mod config;
mod duplicate_keys;
mod error;
mod template;
mod tolerance;
mod validation;

pub use config::{
    build_config_env, build_secret_env, flatten_secrets, resolve_config, resolve_secrets,
};
pub use error::EngineError;
pub use template::{apply_template_vars, apply_vars, parse_experiment, validate_placeholders};
pub(crate) use tolerance::reading;
pub use tolerance::{conclude, determine_status, evaluate_tolerance, Conclusion, RunFacts};
pub use validation::validate_experiment;
