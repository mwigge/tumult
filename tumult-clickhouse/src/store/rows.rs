//! Typed Row structs for safe insert/select (no SQL interpolation).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, clickhouse::Row)]
pub(crate) struct ExperimentRow {
    pub(crate) experiment_id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) started_at_ns: i64,
    pub(crate) ended_at_ns: i64,
    pub(crate) duration_ms: u64,
    pub(crate) method_step_count: i64,
    pub(crate) rollback_count: i64,
    pub(crate) hypothesis_before_met: Option<u8>,
    pub(crate) hypothesis_after_met: Option<u8>,
    pub(crate) estimate_accuracy: Option<f64>,
    pub(crate) resilience_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, clickhouse::Row)]
pub(crate) struct ActivityRow {
    pub(crate) experiment_id: String,
    pub(crate) name: String,
    pub(crate) activity_type: String,
    pub(crate) status: String,
    pub(crate) started_at_ns: i64,
    pub(crate) duration_ms: u64,
    pub(crate) output: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) phase: String,
}

#[derive(Debug, Deserialize, clickhouse::Row)]
pub(crate) struct CountRow {
    pub(crate) count: u64,
}

#[derive(Debug, Deserialize, clickhouse::Row)]
pub(crate) struct ValueRow {
    pub(crate) value: String,
}
