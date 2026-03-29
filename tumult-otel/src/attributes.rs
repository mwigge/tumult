//! Standard span attribute names following the resilience.* namespace.

// Experiment identity
pub const EXPERIMENT_ID: &str = "resilience.experiment.id";
pub const EXPERIMENT_NAME: &str = "resilience.experiment.name";
pub const EXPERIMENT_RUN_NUMBER: &str = "resilience.experiment.run_number";

// Target
pub const TARGET_SYSTEM: &str = "resilience.target.system";
pub const TARGET_TECHNOLOGY: &str = "resilience.target.technology";
pub const TARGET_COMPONENT: &str = "resilience.target.component";
pub const TARGET_ENVIRONMENT: &str = "resilience.target.environment";

// Fault
pub const FAULT_TYPE: &str = "resilience.fault.type";
pub const FAULT_SUBTYPE: &str = "resilience.fault.subtype";
pub const FAULT_SEVERITY: &str = "resilience.fault.severity";
pub const FAULT_BLAST_RADIUS: &str = "resilience.fault.blast_radius";

// Action / Probe
pub const ACTION_NAME: &str = "resilience.action.name";
pub const PROBE_NAME: &str = "resilience.probe.name";
pub const PLUGIN_NAME: &str = "resilience.plugin.name";

// Outcome
pub const OUTCOME: &str = "resilience.outcome.status";
pub const HYPOTHESIS_MET: &str = "resilience.outcome.hypothesis_met";
pub const RECOVERY_TIME_S: &str = "resilience.outcome.recovery_time_s";

// Execution
pub const EXECUTION_TARGET: &str = "resilience.execution.target";
pub const DURATION_MS: &str = "resilience.duration_ms";
