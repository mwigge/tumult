//! Standard span attribute names following the resilience.* namespace.

// Experiment identity
/// Span attribute key for the experiment identifier.
pub const EXPERIMENT_ID: &str = "resilience.experiment.id";
/// Span attribute key for the experiment name.
pub const EXPERIMENT_NAME: &str = "resilience.experiment.name";
/// Span attribute key for the experiment run number.
pub const EXPERIMENT_RUN_NUMBER: &str = "resilience.experiment.run_number";

// Target
/// Span attribute key for the targeted system.
pub const TARGET_SYSTEM: &str = "resilience.target.system";
/// Span attribute key for the target's technology.
pub const TARGET_TECHNOLOGY: &str = "resilience.target.technology";
/// Span attribute key for the target component.
pub const TARGET_COMPONENT: &str = "resilience.target.component";
/// Span attribute key for the target environment.
pub const TARGET_ENVIRONMENT: &str = "resilience.target.environment";

// Fault
/// Span attribute key for the fault type.
pub const FAULT_TYPE: &str = "resilience.fault.type";
/// Span attribute key for the fault subtype.
pub const FAULT_SUBTYPE: &str = "resilience.fault.subtype";
/// Span attribute key for the fault severity.
pub const FAULT_SEVERITY: &str = "resilience.fault.severity";
/// Span attribute key for the fault blast radius.
pub const FAULT_BLAST_RADIUS: &str = "resilience.fault.blast_radius";

// Action / Probe
/// Span attribute key for the executed action name.
pub const ACTION_NAME: &str = "resilience.action.name";
/// Span attribute key for the executed probe name.
pub const PROBE_NAME: &str = "resilience.probe.name";
/// Span attribute key for the plugin name.
pub const PLUGIN_NAME: &str = "resilience.plugin.name";

// Outcome
/// Span attribute key for the outcome status (`success`/`failure`).
pub const OUTCOME: &str = "resilience.outcome.status";
/// Span attribute key recording whether the steady-state hypothesis held.
pub const HYPOTHESIS_MET: &str = "resilience.outcome.hypothesis_met";
/// Span attribute key for the recovery time, in seconds.
pub const RECOVERY_TIME_S: &str = "resilience.outcome.recovery_time_s";

// Execution
/// Span attribute key for the execution target.
pub const EXECUTION_TARGET: &str = "resilience.execution.target";
/// Span attribute key for a duration, in milliseconds.
pub const DURATION_MS: &str = "resilience.duration_ms";
