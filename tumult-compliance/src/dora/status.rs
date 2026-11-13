//! Evidence status: what the evidence shows about a requirement.
//!
//! The scale is the audit distinction between a control that is designed,
//! one that exists, and one that has operated over time:
//!
//! | status      | rule                                                        |
//! |-------------|-------------------------------------------------------------|
//! | `na`        | the profile excludes the row                                |
//! | `unknown`   | the evidence class is not connected                         |
//! | `no_events` | the feed is connected and nothing happened                  |
//! | `none`      | observable, no declaration and no evidence                  |
//! | `designed`  | a target, plan, schedule or policy is declared              |
//! | `existing`  | `v` is at or above the Existing threshold today             |
//! | `operating` | `v` has held at or above it for the whole Operating window  |
//!
//! A status is a statement about evidence held against a house
//! expectation. It is never a statement about compliance.

use tumult_core::compliance::{Requirement, ARTICLES};

use super::coverage::{CellState, Matrix, MatrixCell, MatrixRow, RowState};
use super::params::HouseParams;

/// Evidence status of a cell or of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Na,
    Unknown,
    NoEvents,
    None,
    Designed,
    Existing,
    Operating,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Na => "na",
            Self::Unknown => "unknown",
            Self::NoEvents => "no_events",
            Self::None => "none",
            Self::Designed => "designed",
            Self::Existing => "existing",
            Self::Operating => "operating",
        }
    }

    /// Rank 0–3 of an observable status; `None` for the three states
    /// that are not on the scale.
    #[must_use]
    pub fn rank(self) -> Option<u8> {
        match self {
            Self::Na | Self::Unknown | Self::NoEvents => None,
            Self::None => Some(0),
            Self::Designed => Some(1),
            Self::Existing => Some(2),
            Self::Operating => Some(3),
        }
    }

    fn from_rank(rank: u8) -> Self {
        match rank {
            0 => Self::None,
            1 => Self::Designed,
            2 => Self::Existing,
            _ => Self::Operating,
        }
    }
}

/// Unbroken days before `today` on which the value held at or above the
/// Existing threshold, counted back from yesterday and capped at the
/// Operating window. The caller has already established that today's
/// value holds.
pub fn streak_days(value_on: impl Fn(i64) -> f64, today: i64, params: &HouseParams) -> u32 {
    let mut days = 0;
    while days < params.operating_days {
        let day = today - i64::from(days) - 1;
        if value_on(day) < params.existing_threshold {
            break;
        }
        days += 1;
    }
    days
}

/// Days until a cell that holds today reaches Operating, if nothing
/// breaks the streak.
#[must_use]
pub fn days_until_operating(streak_days: u32, params: &HouseParams) -> u32 {
    params.operating_days.saturating_sub(streak_days)
}

/// Status of one (function, requirement) cell.
#[must_use]
pub fn cell_status(cell: &MatrixCell<'_>, params: &HouseParams) -> Status {
    match cell.cell.state {
        CellState::Unknown => return Status::Unknown,
        CellState::NoEvents => return Status::NoEvents,
        CellState::Scored => {}
    }
    if cell.cell.v < params.existing_threshold {
        return if cell.declared {
            Status::Designed
        } else {
            Status::None
        };
    }
    if cell.streak_days >= params.operating_days {
        return Status::Operating;
    }
    Status::Existing
}

/// Status of a row over a scope of functions: the weakest scored cell,
/// except that the row is Operating when every cell is at least Existing
/// and the Operating share (by criticality weight) reaches the house
/// share.
#[must_use]
pub fn row_status(matrix: &Matrix<'_>, row: &MatrixRow<'_>, scope: Option<usize>) -> Status {
    match row.state(scope) {
        RowState::Na => return Status::Na,
        RowState::Unknown => return Status::Unknown,
        RowState::NoEvents => return Status::NoEvents,
        RowState::Known => {}
    }
    let params = matrix.ctx.params;
    let mut weakest = 3;
    let mut operating = 0.0;
    let mut total = 0.0;
    for cell in row.scoped(scope) {
        let Some(rank) = cell_status(cell, params).rank() else {
            continue;
        };
        let weight = matrix.weight(cell.function);
        weakest = weakest.min(rank);
        total += weight;
        if rank == 3 {
            operating += weight;
        }
    }
    if weakest == 2 && total > 0.0 && operating / total >= params.operating_share {
        return Status::Operating;
    }
    Status::from_rank(weakest)
}

/// The status the house expects of a row: the configured override, else
/// the registry default for the entity's profile.
#[must_use]
pub fn expected_rank(req: &Requirement, matrix: &Matrix<'_>) -> u8 {
    matrix
        .ctx
        .params
        .expected_status
        .get(req.id)
        .copied()
        .unwrap_or_else(|| req.expected_status(&matrix.ctx.inputs.entity))
}

/// Name of an expected-status rank.
#[must_use]
pub fn rank_name(rank: u8) -> &'static str {
    Status::from_rank(rank).as_str()
}

/// How a row stands against the house expectation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// At or above the expected status.
    Meets,
    Gap,
    Unknown,
    NoEvents,
    Na,
}

impl Standing {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Meets => "meets",
            Self::Gap => "gap",
            Self::Unknown => "unknown",
            Self::NoEvents => "no_events",
            Self::Na => "na",
        }
    }
}

/// One cell's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellStatus {
    /// Index into `Inputs::functions`.
    pub function: usize,
    pub status: Status,
    pub streak_days: u32,
    /// Days until Operating, for a cell that is Existing.
    pub days_until_operating: Option<u32>,
}

/// One row's status against its expectation.
#[derive(Debug, Clone)]
pub struct RowStatus {
    pub req: &'static Requirement,
    pub status: Status,
    pub expected: u8,
    /// Ranks short of the expectation; 0 when met or not on the scale.
    pub gap: u8,
    pub standing: Standing,
    pub cells: Vec<CellStatus>,
}

/// Row counts of one article, and the same split weighted by the
/// criticality of the functions behind each row.
#[derive(Debug, Clone, PartialEq)]
pub struct ArticleStatus {
    pub article: &'static str,
    pub meets: usize,
    pub gap: usize,
    pub unknown: usize,
    pub no_events: usize,
    pub na: usize,
    /// Criticality-weighted shares of (row, function) cells that meet the
    /// expectation, fall short, or are unknown; they sum to 1 when the
    /// article has any observable or unknown cell.
    pub weighted_meets: f64,
    pub weighted_gap: f64,
    pub weighted_unknown: f64,
}

/// Status of every row of an entity.
#[derive(Debug, Clone)]
pub struct StatusReport {
    pub rows: Vec<RowStatus>,
    pub articles: Vec<ArticleStatus>,
}

impl StatusReport {
    /// Rows with this standing.
    #[must_use]
    pub fn count(&self, standing: Standing) -> usize {
        self.rows.iter().filter(|r| r.standing == standing).count()
    }

    /// The row with this requirement id.
    #[must_use]
    pub fn row(&self, id: &str) -> Option<&RowStatus> {
        self.rows.iter().find(|r| r.req.id == id)
    }
}

fn standing(status: Status, expected: u8) -> (Standing, u8) {
    match status {
        Status::Na => (Standing::Na, 0),
        Status::Unknown => (Standing::Unknown, 0),
        Status::NoEvents => (Standing::NoEvents, 0),
        _ => {
            let rank = status.rank().unwrap_or(0);
            if rank >= expected {
                return (Standing::Meets, 0);
            }
            (Standing::Gap, expected - rank)
        }
    }
}

fn cell_statuses(
    row: &MatrixRow<'_>,
    scope: Option<usize>,
    params: &HouseParams,
) -> Vec<CellStatus> {
    row.scoped(scope)
        .map(|cell| {
            let status = cell_status(cell, params);
            CellStatus {
                function: cell.function,
                status,
                streak_days: cell.streak_days,
                days_until_operating: (status == Status::Existing)
                    .then(|| days_until_operating(cell.streak_days, params)),
            }
        })
        .collect()
}

/// Status of one row over a scope.
#[must_use]
pub fn row_report(matrix: &Matrix<'_>, row: &MatrixRow<'_>, scope: Option<usize>) -> RowStatus {
    let status = row_status(matrix, row, scope);
    let expected = expected_rank(row.req, matrix);
    let (standing, gap) = standing(status, expected);
    RowStatus {
        req: row.req,
        status,
        expected,
        gap,
        standing,
        cells: cell_statuses(row, scope, matrix.ctx.params),
    }
}

fn article_status(matrix: &Matrix<'_>, article: &'static str, rows: &[RowStatus]) -> ArticleStatus {
    let mut out = ArticleStatus {
        article,
        meets: 0,
        gap: 0,
        unknown: 0,
        no_events: 0,
        na: matrix.na.iter().filter(|r| r.article == article).count(),
        weighted_meets: 0.0,
        weighted_gap: 0.0,
        weighted_unknown: 0.0,
    };
    for row in rows.iter().filter(|r| r.req.article == article) {
        match row.standing {
            Standing::Meets => out.meets += 1,
            Standing::Gap => out.gap += 1,
            Standing::Unknown => out.unknown += 1,
            Standing::NoEvents => out.no_events += 1,
            Standing::Na => out.na += 1,
        }
        for cell in &row.cells {
            let weight = matrix.weight(cell.function);
            match standing(cell.status, row.expected).0 {
                Standing::Meets => out.weighted_meets += weight,
                Standing::Gap => out.weighted_gap += weight,
                Standing::Unknown => out.weighted_unknown += weight,
                Standing::NoEvents | Standing::Na => {}
            }
        }
    }
    let total = out.weighted_meets + out.weighted_gap + out.weighted_unknown;
    if total > 0.0 {
        out.weighted_meets /= total;
        out.weighted_gap /= total;
        out.weighted_unknown /= total;
    }
    out
}

/// Status of every applicable row over a scope, with the per-article
/// counts.
#[must_use]
pub fn report(matrix: &Matrix<'_>, scope: Option<usize>) -> StatusReport {
    let rows: Vec<RowStatus> = matrix
        .rows
        .iter()
        .map(|row| row_report(matrix, row, scope))
        .collect();
    let articles = ARTICLES
        .iter()
        .map(|(article, _)| article_status(matrix, article, &rows))
        .filter(|a| a.meets + a.gap + a.unknown + a.no_events + a.na > 0)
        .collect();
    StatusReport { rows, articles }
}

/// One stored daily status: `(function id, requirement id, status, v)`.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyStatus {
    pub function_id: String,
    pub requirement_id: &'static str,
    pub status: Status,
    /// `None` when the cell is unknown or has no events.
    pub value: Option<f64>,
}

/// Today's status of every cell, as stored by the daily snapshot.
#[must_use]
pub fn daily(matrix: &Matrix<'_>) -> Vec<DailyStatus> {
    let params = matrix.ctx.params;
    let mut out = Vec::new();
    for row in &matrix.rows {
        for cell in &row.cells {
            out.push(DailyStatus {
                function_id: matrix.ctx.inputs.functions[cell.function].id.clone(),
                requirement_id: row.req.id,
                status: cell_status(cell, params),
                value: (cell.cell.state == CellState::Scored).then_some(cell.cell.v),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use tumult_core::compliance::EntityProfile;

    use super::super::model::{
        format_day, parse_day, Evidence, Function, Inputs, PlannedTest, Tier, DAY_NS,
    };
    use super::*;

    fn day(date: &str) -> i64 {
        parse_day(date).unwrap()
    }

    fn noon(day: i64) -> i64 {
        day * DAY_NS + DAY_NS / 2
    }

    /// One critical function with a passed restore test `age` days before
    /// the synthetic clock's "today".
    fn restore_tested(today: i64, age: i64) -> Inputs {
        let mut inputs = Inputs::empty(noon(today), EntityProfile::full());
        inputs.functions = vec![Function::new("cif-0042", Tier::Critical)];
        inputs.evidence = vec![Evidence::run(
            "run-restore",
            "cif-0042",
            "dr",
            noon(today - age),
        )];
        inputs
    }

    fn restore_status(inputs: &Inputs) -> CellStatus {
        let params = HouseParams::default();
        let matrix = Matrix::build(inputs, &params);
        let row = matrix.row("dora.art12.2").unwrap();
        row_report(&matrix, row, None).cells[0]
    }

    #[test]
    fn evidence_179_days_old_is_existing_and_180_days_old_is_operating() {
        let today = day("2026-11-13");
        let at_179 = restore_status(&restore_tested(today, 179));
        assert_eq!(at_179.status, Status::Existing);
        assert_eq!(at_179.streak_days, 179);
        assert_eq!(at_179.days_until_operating, Some(1));

        let at_180 = restore_status(&restore_tested(today, 180));
        assert_eq!(at_180.status, Status::Operating);
        assert_eq!(at_180.days_until_operating, None);
    }

    #[test]
    fn the_clock_advancing_one_day_flips_existing_to_operating() {
        let start = day("2026-05-17");
        let mut inputs = restore_tested(start, 0);
        inputs.as_of_ns = noon(start + 179);
        assert_eq!(restore_status(&inputs).status, Status::Existing);
        inputs.as_of_ns = noon(start + 180);
        assert_eq!(restore_status(&inputs).status, Status::Operating);
    }

    #[test]
    fn a_one_day_gap_in_the_stored_history_resets_the_streak() {
        let today = day("2026-11-13");
        let mut inputs = restore_tested(today, 300);
        assert_eq!(restore_status(&inputs).status, Status::Operating);

        let key = ("cif-0042".to_string(), "dora.art12.2".to_string());
        inputs
            .snapshots
            .entry(key)
            .or_default()
            .insert(today - 10, 0.0);
        let broken = restore_status(&inputs);
        assert_eq!(broken.status, Status::Existing);
        assert_eq!(
            broken.streak_days, 9,
            "only the nine days after the gap count"
        );
        assert_eq!(broken.days_until_operating, Some(171));
    }

    #[test]
    fn a_gap_between_two_tests_resets_the_streak() {
        // Telemetry-class rows have a 90-day window, so a service silent
        // for longer than that breaks the streak when it returns.
        let today = day("2026-11-13");
        let mut inputs = restore_tested(today, 400);
        // Chaos evidence is not restore evidence: 12(2) has only the old
        // failed-then-fresh pair below.
        inputs.evidence.clear();
        let mut old = Evidence::run("run-old", "cif-0042", "dr", noon(today - 900));
        old.experiment = "restore".into();
        let mut new = Evidence::run("run-new", "cif-0042", "dr", noon(today - 20));
        new.experiment = "restore".into();
        inputs.evidence = vec![old, new];
        let status = restore_status(&inputs);
        assert_eq!(status.status, Status::Existing);
        assert_eq!(status.streak_days, 20);
    }

    #[test]
    fn the_date_180_days_of_evidence_is_reached_follows_the_streak() {
        let today = day("2026-11-13");
        let status = restore_status(&restore_tested(today, 150));
        let reached = today + i64::from(status.days_until_operating.unwrap());
        assert_eq!(format_day(reached), "2026-12-13");
    }

    #[test]
    fn a_declared_target_without_evidence_is_designed_and_nothing_is_none() {
        let today = day("2026-11-13");
        let mut inputs = restore_tested(today, 10);
        inputs.evidence.clear();
        assert_eq!(restore_status(&inputs).status, Status::None);
        inputs.functions[0].test_plan = vec![PlannedTest {
            kind: "dr".into(),
            interval_days: 365,
        }];
        assert_eq!(restore_status(&inputs).status, Status::Designed);
    }

    #[test]
    fn expected_status_follows_weight_and_can_be_overridden() {
        let today = day("2026-11-13");
        let inputs = restore_tested(today, 30);
        let mut params = HouseParams::default();
        {
            let matrix = Matrix::build(&inputs, &params);
            let row = row_report(&matrix, matrix.row("dora.art12.2").unwrap(), None);
            assert_eq!(row.expected, 3, "weight 3 under full expects Operating");
            assert_eq!(row.status, Status::Existing);
            assert_eq!((row.standing, row.gap), (Standing::Gap, 1));
        }
        params.expected_status.insert("dora.art12.2".into(), 2);
        let matrix = Matrix::build(&inputs, &params);
        let row = row_report(&matrix, matrix.row("dora.art12.2").unwrap(), None);
        assert_eq!((row.standing, row.gap), (Standing::Meets, 0));
    }

    #[test]
    fn a_row_is_operating_when_the_house_share_of_its_functions_is() {
        let today = day("2026-11-13");
        let mut inputs = restore_tested(today, 200);
        // Critical 3 + critical 3 + important 2 Operating; important 2 at
        // 100 days: 8 of 10 by weight.
        for (id, tier, age) in [
            ("cif-0043", Tier::Critical, 200),
            ("cif-0017", Tier::Important, 200),
            ("cif-0018", Tier::Important, 100),
        ] {
            inputs.functions.push(Function::new(id, tier));
            inputs.evidence.push(Evidence::run(
                &format!("run-{id}"),
                id,
                "dr",
                noon(today - age),
            ));
        }
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let row = matrix.row("dora.art12.2").unwrap();
        assert_eq!(row_status(&matrix, row, None), Status::Operating);
        assert_eq!(row_status(&matrix, row, Some(3)), Status::Existing);

        let strict = HouseParams {
            operating_share: 0.9,
            ..HouseParams::default()
        };
        let matrix = Matrix::build(&inputs, &strict);
        let row = matrix.row("dora.art12.2").unwrap();
        assert_eq!(row_status(&matrix, row, None), Status::Existing);
    }

    #[test]
    fn article_counts_split_meets_gap_unknown_and_na() {
        let today = day("2026-11-13");
        let inputs = restore_tested(today, 200);
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let report = report(&matrix, None);
        let art12 = report.articles.iter().find(|a| a.article == "12").unwrap();
        // 12(2) Operating and 12(4) Operating meet; 12(6) and 12(7) have no
        // measured recovery; 12(1) and 12(3) await an attestation; the two
        // sector rows are n/a.
        assert_eq!(art12.meets, 2);
        assert_eq!(art12.gap, 2);
        assert_eq!(art12.unknown, 2);
        assert_eq!(art12.na, 2);
        let shares = art12.weighted_meets + art12.weighted_gap + art12.weighted_unknown;
        assert!((shares - 1.0).abs() < 1e-9);
        assert_eq!(
            report.row("dora.art12.1").unwrap().standing,
            Standing::Unknown
        );
    }

    #[test]
    fn the_daily_snapshot_carries_one_row_per_cell() {
        let today = day("2026-11-13");
        let inputs = restore_tested(today, 200);
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let rows = daily(&matrix);
        let cells: usize = matrix.rows.iter().map(|r| r.cells.len()).sum();
        assert_eq!(rows.len(), cells);
        let restore = rows
            .iter()
            .find(|r| r.requirement_id == "dora.art12.2")
            .unwrap();
        assert_eq!(restore.status, Status::Operating);
        assert_eq!(restore.value, Some(1.0));
        let attest = rows
            .iter()
            .find(|r| r.requirement_id == "dora.art12.1")
            .unwrap();
        assert_eq!((attest.status, attest.value), (Status::Unknown, None));
    }
}
