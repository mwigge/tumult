//! Experiment engine — orchestrates the five-phase execution lifecycle.

mod config;
mod error;
mod template;
mod tolerance;
mod validation;

pub use config::{resolve_config, resolve_secrets};
pub use error::EngineError;
pub use template::{apply_vars, parse_experiment};
pub use tolerance::{determine_status, evaluate_tolerance};
pub use validation::validate_experiment;
