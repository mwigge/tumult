//! The recovery trends of `tumult trend`: the RTO and the RPO each run
//! measured, and their margins to the targets the run declares.
//!
//! A run that did not recover has no measured RTO. Its point holds the
//! time it watched from the outage seen, a bound, and says that it did not
//! recover. A run whose probes saw no outage, and a run that declares a
//! target but measured nothing, are no point: they are counted.

use duckdb::params;

use crate::error::AnalyticsError;

use super::AnalyticsStore;

/// A metric of a recovery trend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryMetric {
    /// The RTO a run measured: smaller is better.
    RtoMeasured,
    /// The RTO target of a run minus the RTO it measured: negative is a
    /// breach, larger is better.
    RtoMargin,
    /// The RPO a run measured: smaller is better.
    RpoMeasured,
    /// The RPO target of a run minus the RPO it measured.
    RpoMargin,
}

impl RecoveryMetric {
    /// Every recovery metric, in the order `tumult trend` lists them.
    pub const ALL: [Self; 4] = [
        Self::RtoMeasured,
        Self::RtoMargin,
        Self::RpoMeasured,
        Self::RpoMargin,
    ];

    /// The name of the metric, as `tumult trend --metric` takes it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::RtoMeasured => "rto_measured_s",
            Self::RtoMargin => "rto_margin_s",
            Self::RpoMeasured => "rpo_measured_s",
            Self::RpoMargin => "rpo_margin_s",
        }
    }

    /// The metric named `name`. `None` when no recovery metric has it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|metric| metric.name() == name)
    }

    /// Whether the metric is a margin to a target, where larger is better.
    #[must_use]
    pub fn is_margin(self) -> bool {
        matches!(self, Self::RtoMargin | Self::RpoMargin)
    }

    /// Whether the metric is of the RTO.
    #[must_use]
    pub fn is_rto(self) -> bool {
        matches!(self, Self::RtoMeasured | Self::RtoMargin)
    }

    /// How the value of a run that did not recover bounds the one it would
    /// have measured.
    fn bound(self) -> &'static str {
        if self.is_margin() {
            "at most"
        } else {
            "at least"
        }
    }
}

// The query of the runs of a trend, oldest first: the run, what its point
// is (`measured`, `not_recovered`, `no_outage` or `not_measured`), its
// value and its target.
//
// The RTO runs are the runs that measured an RTO or declare a target. A
// run that did not recover has an `rto` verdict that says so, with the
// time it watched from the outage seen. Grouped, so that an objective of
// the run that is also named `rto` makes no second row.
const RTO_SQL: &str = "
WITH unrecovered AS (
    SELECT experiment_id, max(observed) AS watched_s
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto' AND recovered = false
    GROUP BY experiment_id
)
SELECT e.experiment_id, e.title, e.status, e.started_at_ns,
    CASE
        WHEN e.rto_measured_s IS NOT NULL THEN 'measured'
        WHEN u.watched_s IS NOT NULL THEN 'not_recovered'
        WHEN e.impact_observed = false THEN 'no_outage'
        ELSE 'not_measured'
    END AS point,
    coalesce(e.rto_measured_s, u.watched_s) AS value,
    e.rto_target_s AS target
FROM experiments AS e
LEFT JOIN unrecovered AS u ON u.experiment_id = e.experiment_id
WHERE e.rto_measured_s IS NOT NULL OR e.rto_target_s IS NOT NULL
ORDER BY e.started_at_ns, e.experiment_id";

// The RPO runs are the runs that measured an RPO, declare a target or have
// an `rpo` objective.
const RPO_SQL: &str = "
SELECT e.experiment_id, e.title, e.status, e.started_at_ns,
    CASE WHEN e.rpo_measured_s IS NOT NULL THEN 'measured' ELSE 'not_measured' END AS point,
    e.rpo_measured_s AS value,
    e.rpo_target_s AS target
FROM experiments AS e
WHERE e.rpo_measured_s IS NOT NULL
    OR e.rpo_target_s IS NOT NULL
    OR EXISTS (
        SELECT 1 FROM nfr_verdicts AS v
        WHERE v.experiment_id = e.experiment_id AND v.kind = 'rpo'
    )
ORDER BY e.started_at_ns, e.experiment_id";

/// A row of [`RTO_SQL`] or [`RPO_SQL`].
struct TrendRow {
    experiment_id: String,
    title: String,
    status: String,
    started_at_ns: i64,
    point: String,
    value: Option<f64>,
    target: Option<f64>,
}

/// One run of a recovery trend.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryPoint {
    pub experiment_id: String,
    pub title: String,
    pub status: String,
    pub started_at_ns: i64,
    /// The value the run measured. For a run that did not recover, the
    /// bound that [`bound`](Self::bound) names.
    pub value: f64,
    /// The RTO metrics only: whether the run recovered. `None` for the RPO
    /// metrics.
    pub recovered: Option<bool>,
    /// For a run that did not recover: `at least` for its RTO, which is the
    /// time it watched from the outage seen, and `at most` for its margin.
    /// `None` for a value the run measured.
    pub bound: Option<&'static str>,
}

/// A recovery trend: its points, oldest first, and the runs left out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecoveryTrend {
    pub points: Vec<RecoveryPoint>,
    /// The runs left out because their probes saw no outage.
    pub no_outage: usize,
    /// The runs left out because they declare the target and measured
    /// nothing, such as a run whose method failed or an RPO that no probe
    /// read back.
    pub not_measured: usize,
}

impl RecoveryTrend {
    /// Add the run of `row` to the trend of `metric`: as a point, or to
    /// the count of the runs left out. A margin is of the runs that declare
    /// a target only.
    fn add(&mut self, metric: RecoveryMetric, row: TrendRow) {
        let value = if metric.is_margin() {
            let Some(target) = row.target else {
                return;
            };
            row.value.map(|value| target - value)
        } else {
            row.value
        };
        let (value, recovered) = match (row.point.as_str(), value) {
            ("measured", Some(value)) => (value, true),
            ("not_recovered", Some(value)) => (value, false),
            ("no_outage", _) => {
                self.no_outage += 1;
                return;
            }
            _ => {
                self.not_measured += 1;
                return;
            }
        };
        self.points.push(RecoveryPoint {
            experiment_id: row.experiment_id,
            title: row.title,
            status: row.status,
            started_at_ns: row.started_at_ns,
            value,
            recovered: metric.is_rto().then_some(recovered),
            bound: (!recovered).then(|| metric.bound()),
        });
    }
}

impl AnalyticsStore {
    /// The trend of `metric` over every run of the store.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn recovery_trend(&self, metric: RecoveryMetric) -> Result<RecoveryTrend, AnalyticsError> {
        let sql = if metric.is_rto() { RTO_SQL } else { RPO_SQL };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![], |row| {
            Ok(TrendRow {
                experiment_id: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                started_at_ns: row.get(3)?,
                point: row.get(4)?,
                value: row.get(5)?,
                target: row.get(6)?,
            })
        })?;
        let mut trend = RecoveryTrend::default();
        for row in rows {
            trend.add(metric, row?);
        }
        Ok(trend)
    }
}

/// The statements of the recovery trend, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![RTO_SQL, RPO_SQL]
}
