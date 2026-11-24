//! What the DR reports and the evidence pack say of a DR test: the RTO and
//! the RPO it planned and measured, and the evidence of a manual record.

use serde_json::Value;
use tumult_lake::ManualDetail;

use super::cell;
use crate::html::fmt_datetime;
use crate::model::Cell;

/// What a report says of the journal hash of a run: the lake does not keep
/// it.
pub(super) const JOURNAL_HASH_NOT_KEPT: &str =
    "not kept in the lake: tumult run prints it when it writes the journal";

/// Seconds as a report gives them: `4.0 s`.
pub(super) fn seconds(value: f64) -> String {
    format!("{value:.1} s")
}

/// The planned value of an objective, or `not set`.
pub(super) fn planned(target: Option<f64>) -> String {
    target.map_or_else(|| "not set".into(), seconds)
}

/// The number `key` of a row.
pub(super) fn number(row: &Value, key: &str) -> Option<f64> {
    row.get(key).and_then(Value::as_f64)
}

/// The boolean `key` of a row.
pub(super) fn flag(row: &Value, key: &str) -> Option<bool> {
    row.get(key).and_then(Value::as_bool)
}

/// The time `key` of a row, in unix nanoseconds.
pub(super) fn time(row: &Value, key: &str) -> Option<i64> {
    row.get(key).and_then(Value::as_i64)
}

/// What a DR test planned and measured of its RTO and its RPO, read from a
/// row with the columns of the lake view `dr_tests`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Objectives {
    rto_target_s: Option<f64>,
    rto_measured_s: Option<f64>,
    recovered: Option<bool>,
    no_outage: Option<bool>,
    rpo_target_s: Option<f64>,
    rpo_measured_s: Option<f64>,
}

impl Objectives {
    pub(super) fn of(row: &Value) -> Self {
        Self {
            rto_target_s: number(row, "rto_target_s"),
            rto_measured_s: number(row, "rto_measured_s"),
            recovered: flag(row, "recovered"),
            no_outage: flag(row, "no_outage"),
            rpo_target_s: number(row, "rpo_target_s"),
            rpo_measured_s: number(row, "rpo_measured_s"),
        }
    }

    pub(super) fn rto_planned(&self) -> String {
        planned(self.rto_target_s)
    }

    pub(super) fn rpo_planned(&self) -> String {
        planned(self.rpo_target_s)
    }

    /// What the test measured of its RTO: `not recovered` when the target
    /// did not come back while it was watched, `no outage observed` when
    /// the probes of a run saw none, else the seconds or `not measured`.
    pub(super) fn rto_actual(&self) -> String {
        if self.recovered == Some(false) {
            return "not recovered".into();
        }
        if self.no_outage == Some(true) {
            return "no outage observed".into();
        }
        self.rto_measured_s
            .map_or_else(|| "not measured".into(), seconds)
    }

    /// What the test measured of its RPO: the seconds, or `not measured`.
    pub(super) fn rpo_actual(&self) -> String {
        self.rpo_measured_s
            .map_or_else(|| "not measured".into(), seconds)
    }

    /// The RTO as `planned / actual`.
    pub(super) fn rto(&self) -> String {
        format!("{} / {}", self.rto_planned(), self.rto_actual())
    }

    /// The RPO as `planned / actual`.
    pub(super) fn rpo(&self) -> String {
        format!("{} / {}", self.rpo_planned(), self.rpo_actual())
    }
}

/// The verdict of a row of `dr_tests`, in words. A run whose probes saw no
/// outage and that measured nothing says `no outage observed`.
pub(super) fn verdict(row: &Value) -> String {
    let verdict = cell(row, "verdict").unwrap_or("not_measured");
    if verdict == "not_measured" && flag(row, "no_outage") == Some(true) {
        return "no outage observed".into();
    }
    verdict.replace('_', " ")
}

/// Whether the data was checked after the recovery, in words.
pub(super) fn data_verified(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes, complete",
        Some(false) => "no, loss or corruption found",
        None => "not checked",
    }
}

/// An attachment of a record in one line: a stored file with its type and
/// its size, or a link with where it points. The SHA-256 of a file goes on
/// a line of its own: it does not break.
pub(super) fn attachment(row: &Value) -> String {
    let kind = cell(row, "kind").unwrap_or("link");
    let label = cell(row, "label");
    if kind == "file" {
        return format!(
            "{}, {}, {} bytes",
            label.unwrap_or("unnamed file"),
            cell(row, "media_type").unwrap_or("unknown type"),
            time(row, "size_bytes").map_or_else(|| "unknown".into(), |n| n.to_string()),
        );
    }
    let mut parts = vec![format!(
        "{kind} {}",
        cell(row, "uri").unwrap_or("with no address")
    )];
    parts.extend(label.map(str::to_owned));
    parts.join(", ")
}

/// A sign-off in one line: who signed, in which role, what and when.
pub(super) fn signed(row: &Value) -> String {
    format!(
        "{}, {}, {} on {}",
        cell(row, "signed_by").unwrap_or("unknown"),
        cell(row, "role").unwrap_or("no role"),
        cell(row, "decision").unwrap_or("no decision"),
        time(row, "signed_at_ns").map_or_else(|| "an unknown time".into(), fmt_datetime),
    )
}

/// The evidence of a manual record as rows of a key-value block: each
/// attachment with the SHA-256 of its file on a row of its own, then each
/// sign-off, oldest first.
pub(super) fn record_evidence(detail: &ManualDetail) -> Vec<(String, Cell)> {
    let mut rows = Vec::new();
    for row in &detail.attachments {
        rows.push(("Attachment".to_string(), Cell::text(attachment(row))));
        if let Some(hash) = cell(row, "file_hash") {
            rows.push(("SHA-256".to_string(), Cell::text(hash)));
        }
    }
    rows.extend(
        detail
            .signoffs
            .iter()
            .map(|row| ("Sign-off".to_string(), Cell::text(signed(row)))),
    );
    rows
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_target_that_did_not_recover_says_so_before_anything_it_measured() {
        let objectives = Objectives::of(&json!({
            "rto_target_s": 10.0, "rto_measured_s": 30.1, "recovered": false, "no_outage": true
        }));
        assert_eq!(objectives.rto(), "10.0 s / not recovered");
    }

    #[test]
    fn a_run_that_saw_no_outage_measured_no_rto() {
        let objectives = Objectives::of(&json!({"rto_target_s": 10.0, "no_outage": true}));
        assert_eq!(objectives.rto(), "10.0 s / no outage observed");
        assert_eq!(
            verdict(&json!({"verdict": "not_measured", "no_outage": true})),
            "no outage observed"
        );
        assert_eq!(
            verdict(&json!({"verdict": "breached", "no_outage": true})),
            "breached",
            "an RPO it breached is still breached"
        );
    }

    #[test]
    fn an_rpo_nobody_measured_is_not_measured() {
        let objectives = Objectives::of(&json!({"rpo_target_s": 60.0}));
        assert_eq!(objectives.rpo(), "60.0 s / not measured");
        assert_eq!(Objectives::default().rpo(), "not set / not measured");
    }

    #[test]
    fn a_link_says_where_it_points_and_a_file_gives_its_type_and_size() {
        let file = json!({
            "kind": "file", "label": "restore.log", "media_type": "text/plain",
            "size_bytes": 26, "file_hash": "ab12"
        });
        assert_eq!(attachment(&file), "restore.log, text/plain, 26 bytes");
        let link = json!({"kind": "ticket", "uri": "OPS-1", "label": "the follow-up"});
        assert_eq!(attachment(&link), "ticket OPS-1, the follow-up");
    }
}
