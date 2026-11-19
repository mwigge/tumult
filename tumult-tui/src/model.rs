//! Typed row structs mapped from the analytics store's stringly-typed
//! [`tumult_lake::QueryRow`] results.
//!
//! Every field here corresponds to a real column in the `experiments` /
//! `activity_results` tables (see `tumult-lake/src/schema/analytics.rs`).
//! The mappers are total: a malformed or short row yields `None` rather than
//! panicking, so a partially-populated store can never crash the UI.

/// One experiment as shown in the history table.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentRow {
    pub id: String,
    pub title: String,
    /// Raw store status (`completed`, `deviated`, `aborted`, `failed`, …).
    pub status: String,
    /// Wall-clock start, nanoseconds since the Unix epoch.
    pub started_at_ns: i64,
    pub duration_ms: u64,
    /// Whether the hypothesis of the run was met; `None` when the run
    /// judged none.
    pub hypothesis_met: Option<bool>,
    pub steps: u64,
    /// Count of non-succeeded activities recorded for this experiment.
    pub deviations: u64,
}

impl ExperimentRow {
    /// Build a row from the history query's column order:
    /// `experiment_id, title, status, started_at_ns, duration_ms,
    /// hypothesis_met, method_step_count, deviations`.
    ///
    /// Returns `None` if the slice is too short to be an experiment row.
    #[must_use]
    pub fn from_columns(cols: &[String]) -> Option<Self> {
        if cols.len() < 8 {
            return None;
        }
        Some(Self {
            id: cols[0].clone(),
            title: cols[1].clone(),
            status: cols[2].clone(),
            started_at_ns: cols[3].parse().unwrap_or(0),
            duration_ms: cols[4].parse().unwrap_or(0),
            hypothesis_met: cols[5].parse().ok(),
            steps: cols[6].parse().unwrap_or(0),
            deviations: cols[7].parse().unwrap_or(0),
        })
    }
}

/// One activity row inside an experiment's timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityRow {
    pub name: String,
    pub activity_type: String,
    pub status: String,
    pub duration_ms: u64,
    pub phase: String,
    pub output: String,
}

impl ActivityRow {
    /// Build from the timeline query's column order:
    /// `name, activity_type, status, duration_ms, phase, output`.
    #[must_use]
    pub fn from_columns(cols: &[String]) -> Option<Self> {
        if cols.len() < 6 {
            return None;
        }
        let output = if cols[5] == "NULL" {
            String::new()
        } else {
            cols[5].clone()
        };
        Some(Self {
            name: cols[0].clone(),
            activity_type: cols[1].clone(),
            status: cols[2].clone(),
            duration_ms: cols[3].parse().unwrap_or(0),
            phase: cols[4].clone(),
            output,
        })
    }
}

/// A `ChaosGraph` node summary shown in the graph browser.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNodeRow {
    pub id: String,
    pub kind: String,
    pub label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn experiment_row_maps_all_columns() {
        let row = ExperimentRow::from_columns(&cols(&[
            "abc",
            "Latency drill",
            "deviated",
            "1783183432647939812",
            "57",
            "true",
            "3",
            "1",
        ]))
        .unwrap();
        assert_eq!(row.id, "abc");
        assert_eq!(row.title, "Latency drill");
        assert_eq!(row.status, "deviated");
        assert_eq!(row.started_at_ns, 1_783_183_432_647_939_812);
        assert_eq!(row.duration_ms, 57);
        assert_eq!(row.hypothesis_met, Some(true));
        assert_eq!(row.steps, 3);
        assert_eq!(row.deviations, 1);
    }

    #[test]
    fn experiment_row_handles_a_run_that_judged_no_hypothesis() {
        let row = ExperimentRow::from_columns(&cols(&[
            "abc",
            "t",
            "completed",
            "10",
            "20",
            "NULL",
            "1",
            "0",
        ]))
        .unwrap();
        assert_eq!(row.hypothesis_met, None);
    }

    #[test]
    fn experiment_row_rejects_short_slice() {
        assert!(ExperimentRow::from_columns(&cols(&["a", "b"])).is_none());
    }

    #[test]
    fn activity_row_blanks_null_output() {
        let row = ActivityRow::from_columns(&cols(&[
            "probe-1",
            "probe",
            "succeeded",
            "12",
            "hypothesis_before",
            "NULL",
        ]))
        .unwrap();
        assert_eq!(row.output, "");
        assert_eq!(row.activity_type, "probe");
    }
}
