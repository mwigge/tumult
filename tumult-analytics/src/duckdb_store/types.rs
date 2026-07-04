//! Public analytics data types returned and accepted by [`AnalyticsStore`].
//!
//! [`AnalyticsStore`]: super::AnalyticsStore

// Re-exported from the lightweight `backend` module (where it now lives, so
// backends like `tumult-clickhouse` can use it without the DuckDB pipeline).
// Kept at this path for backwards compatibility.
pub use crate::backend::StoreStats;

#[derive(Debug, Clone, PartialEq)]
pub struct AgenticRunAnalytics {
    pub run_id: String,
    pub experiment_id: String,
    pub target_type: String,
    pub scenario: String,
    pub resilience_score: f64,
    pub trace_id: Option<String>,
    pub replay_id: Option<String>,
    pub contracts: Vec<AgenticContractAnalytics>,
    pub faults: Vec<AgenticFaultAnalytics>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgenticContractAnalytics {
    pub contract_type: String,
    pub scenario: String,
    pub passed: bool,
    pub reason: Option<String>,
    pub severity: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgenticFaultAnalytics {
    pub fault_type: String,
    pub scenario: String,
    pub applied: bool,
}
