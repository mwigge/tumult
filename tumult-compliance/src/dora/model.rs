//! Inputs of the DORA coverage and maturity scores.
//!
//! Everything the scores read is gathered into [`Inputs`] first, so the
//! arithmetic is a pure function of one value: the lake loader fills it in
//! production and the tests build it by hand.

use std::collections::BTreeMap;

use tumult_core::compliance::EntityProfile;

use super::params::HouseParams;

/// Nanoseconds in one day.
pub const DAY_NS: i64 = 86_400_000_000_000;

/// Day number (days since 1970-01-01, UTC) of a timestamp.
#[must_use]
pub fn day_of(ns: i64) -> i64 {
    ns.div_euclid(DAY_NS)
}

/// Last nanosecond of a day.
#[must_use]
pub fn end_of_day(day: i64) -> i64 {
    (day + 1) * DAY_NS - 1
}

/// Day number of a civil date (proleptic Gregorian, UTC).
#[must_use]
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let shifted = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * shifted + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DD` of a day number.
#[must_use]
pub fn format_day(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Day number of a `YYYY-MM-DD` date; `None` when the text is not one or
/// names a day that does not exist.
#[must_use]
pub fn parse_day(text: &str) -> Option<i64> {
    let mut parts = text.trim().splitn(3, '-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.get(..2)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // A day the month does not have (30 February) lands in the next
    // month, so it does not read back as written.
    let number = days_from_civil(year, month, day);
    (format_day(number) == format!("{year:04}-{month:02}-{day:02}")).then_some(number)
}

/// Criticality tier of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Critical,
    Important,
    Other,
}

impl Tier {
    /// Parse the stored tier name; anything that is not `critical` or
    /// `important` is `other`.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "critical" => Self::Critical,
            "important" => Self::Important,
            _ => Self::Other,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::Important => "important",
            Self::Other => "other",
        }
    }

    /// Whether requirements scoped to critical and important functions
    /// bind this tier.
    #[must_use]
    pub fn is_critical_or_important(self) -> bool {
        self != Self::Other
    }

    /// Criticality weight `c_f`.
    #[must_use]
    pub fn weight(self, params: &HouseParams) -> f64 {
        match self {
            Self::Critical => params.criticality.critical,
            Self::Important => params.criticality.important,
            Self::Other => params.criticality.other,
        }
    }
}

/// One planned test of a function's own test plan (Art. 25(3)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTest {
    pub kind: String,
    pub interval_days: u32,
}

/// A registered function: the unit both scores are computed on.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The `ict.entity.id` the function is registered under.
    pub id: String,
    pub name: String,
    pub tier: Tier,
    pub rto_s: Option<f64>,
    pub rpo_s: Option<f64>,
    /// When the criticality assessment was last done.
    pub last_assessed_ns: Option<i64>,
    /// Org-unit path the function belongs to.
    pub org_unit: Option<String>,
    /// Requirement ids the owner has declared a policy, plan or target
    /// for (the "Designed" status).
    pub declared: Vec<String>,
    pub test_plan: Vec<PlannedTest>,
    /// A recurring run is scheduled against the function.
    pub scheduled: bool,
    /// False for the synthetic function that stands in for an
    /// `ict.entity.id` nobody registered.
    pub registered: bool,
    /// `F<n>` register codes, one per licensed activity.
    pub register_codes: Vec<String>,
}

impl Function {
    /// A registered function with nothing declared.
    #[must_use]
    pub fn new(id: &str, tier: Tier) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            tier,
            rto_s: None,
            rpo_s: None,
            last_assessed_ns: None,
            org_unit: None,
            declared: Vec::new(),
            test_plan: Vec::new(),
            scheduled: false,
            registered: true,
            register_codes: Vec::new(),
        }
    }
}

/// Where a piece of evidence came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An automated run.
    Run,
    /// A verified manual record.
    Manual,
}

impl Source {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Manual => "manual",
        }
    }
}

/// Outcome of a piece of evidence. Inconclusive evidence is dropped at
/// load time: it is not evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Partial,
    Failed,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }

    /// Outcome factor `o`.
    #[must_use]
    pub fn factor(self, params: &HouseParams) -> f64 {
        match self {
            Self::Passed => params.outcome.passed,
            Self::Partial => params.outcome.partial,
            Self::Failed => params.outcome.failed,
        }
    }
}

/// One dated piece of evidence: an automated run or a verified manual
/// record.
#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub id: String,
    pub source: Source,
    /// Functions the evidence names. A manual record naming none speaks
    /// for the legal entity as a whole (attestations only).
    pub functions: Vec<String>,
    /// Services the evidence targeted.
    pub services: Vec<String>,
    /// Workload node ids (`wl:…`) the evidence targeted.
    pub workloads: Vec<String>,
    pub system: Option<String>,
    pub component: Option<String>,
    pub env: Option<String>,
    /// Name of the experiment; a finding is closed by a later pass of the
    /// same experiment.
    pub experiment: String,
    /// Test discipline (`chaos`, `dr`, `load`, …); `None` for a record
    /// that is not a test.
    pub kind: Option<String>,
    /// Manual exercise type (`tabletop`, `drill`, …).
    pub exercise_type: Option<String>,
    /// Requirement ids a manual record cites.
    pub cited: Vec<String>,
    pub outcome: Outcome,
    pub at_ns: i64,
    pub rto_target_s: Option<f64>,
    pub rto_measured_s: Option<f64>,
    pub rpo_target_s: Option<f64>,
    pub rpo_measured_s: Option<f64>,
}

impl Evidence {
    /// A passed automated run of `kind` against one function.
    #[must_use]
    pub fn run(id: &str, function: &str, kind: &str, at_ns: i64) -> Self {
        Self {
            id: id.to_string(),
            source: Source::Run,
            functions: vec![function.to_string()],
            services: Vec::new(),
            workloads: Vec::new(),
            system: None,
            component: None,
            env: None,
            experiment: id.to_string(),
            kind: Some(kind.to_string()),
            exercise_type: None,
            cited: Vec::new(),
            outcome: Outcome::Passed,
            at_ns,
            rto_target_s: None,
            rto_measured_s: None,
            rpo_target_s: None,
            rpo_measured_s: None,
        }
    }

    /// A passed, verified manual record of `exercise_type`.
    #[must_use]
    pub fn manual(id: &str, functions: &[&str], exercise_type: &str, at_ns: i64) -> Self {
        Self {
            source: Source::Manual,
            functions: functions.iter().map(|f| (*f).to_string()).collect(),
            kind: None,
            exercise_type: Some(exercise_type.to_string()),
            ..Self::run(id, "", "", at_ns)
        }
    }

    /// Whether the evidence names `function`.
    #[must_use]
    pub fn names(&self, function: &str) -> bool {
        self.functions.iter().any(|f| f == function)
    }
}

/// One service, as seen in telemetry or declared in the register.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub name: String,
    /// Conformance level 0–4 of its telemetry resource.
    pub level: u8,
    /// Functions the service supports.
    pub functions: Vec<String>,
    /// Whether the service emitted telemetry inside the conformance
    /// window.
    pub observed: bool,
    /// First telemetry ever seen from the service.
    pub first_seen_ns: Option<i64>,
    pub last_seen_ns: Option<i64>,
    pub env: Option<String>,
    pub system: Option<String>,
    pub component: Option<String>,
}

impl Service {
    /// An observed service at `level` supporting `functions`.
    #[must_use]
    pub fn observed(name: &str, level: u8, functions: &[&str], last_seen_ns: i64) -> Self {
        Self {
            name: name.to_string(),
            level,
            functions: functions.iter().map(|f| (*f).to_string()).collect(),
            observed: true,
            first_seen_ns: Some(0),
            last_seen_ns: Some(last_seen_ns),
            env: None,
            system: None,
            component: None,
        }
    }

    /// A service the register declares that has emitted no telemetry.
    #[must_use]
    pub fn silent(name: &str, functions: &[&str]) -> Self {
        Self {
            observed: false,
            first_seen_ns: None,
            last_seen_ns: None,
            ..Self::observed(name, 0, functions, 0)
        }
    }
}

/// A workload from the deployment manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workload {
    /// `wl:<namespace>/<kind>/<name>`.
    pub id: String,
    /// The service whose telemetry joins the workload.
    pub service: Option<String>,
}

/// How much an SLO matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SloCriticality {
    Blocking,
    Major,
    Minor,
}

/// One SLO from the SLO registry.
#[derive(Debug, Clone, PartialEq)]
pub struct Slo {
    pub id: String,
    pub function: Option<String>,
    pub service: Option<String>,
    pub criticality: SloCriticality,
    pub met: bool,
    /// Share of the error budget left, 0–1.
    pub remaining: f64,
}

/// The SLO registry feed.
#[derive(Debug, Clone, PartialEq)]
pub enum SloFeed {
    NotConnected,
    Connected { slos: Vec<Slo>, history_days: u32 },
}

/// One major incident from the incident register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    pub id: String,
    /// Functions affected; none means the whole legal entity.
    pub functions: Vec<String>,
    pub at_ns: i64,
    /// Requirement ids the handling of this incident satisfied
    /// (classification, the three reports, the review).
    pub satisfied: Vec<String>,
    /// Review done and every test action verified by a passing run.
    pub closed: bool,
    /// The failure mode had been tested before the incident.
    pub previously_tested: bool,
}

/// The incident register feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncidentFeed {
    NotConnected,
    Connected(Vec<Incident>),
}

/// Deployment-wide facts the upper gates read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // one bool per gate condition
pub struct Facts {
    pub alert_rules_exported: bool,
    pub forecasts_with_provenance: bool,
    pub posture_delta_two_quarters: bool,
    pub register_connected: bool,
    /// When the register of information was last reconciled.
    pub register_reconciled_ns: Option<i64>,
    /// Whether any dependency topology has been observed.
    pub topology_observed: bool,
}

/// Stored daily values of `v(function, requirement)`, keyed by
/// `(function id, requirement id)` then day number: the rows the daily
/// job wrote on the day they describe. `None` is a day stored as unknown
/// or without events. A day with no entry was never stored. Neither
/// counts toward the Operating streak.
pub type Snapshots = BTreeMap<(String, String), BTreeMap<i64, Option<f64>>>;

/// Everything the scores read, for one legal entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Inputs {
    pub as_of_ns: i64,
    pub entity: EntityProfile,
    pub functions: Vec<Function>,
    pub evidence: Vec<Evidence>,
    pub services: Vec<Service>,
    pub workloads: Vec<Workload>,
    pub slos: SloFeed,
    pub incidents: IncidentFeed,
    pub facts: Facts,
    pub snapshots: Snapshots,
}

impl Inputs {
    /// Inputs with nothing connected and nothing registered.
    #[must_use]
    pub fn empty(as_of_ns: i64, entity: EntityProfile) -> Self {
        Self {
            as_of_ns,
            entity,
            functions: Vec::new(),
            evidence: Vec::new(),
            services: Vec::new(),
            workloads: Vec::new(),
            slos: SloFeed::NotConnected,
            incidents: IncidentFeed::NotConnected,
            facts: Facts::default(),
            snapshots: Snapshots::new(),
        }
    }

    /// Whether every function comes from the register.
    #[must_use]
    pub fn functions_registered(&self) -> bool {
        !self.functions.is_empty() && self.functions.iter().all(|f| f.registered)
    }

    /// The function with this id.
    #[must_use]
    pub fn function(&self, id: &str) -> Option<&Function> {
        self.functions.iter().find(|f| f.id == id)
    }

    /// Highest criticality weight among the functions a service supports;
    /// `None` when it maps to no function.
    #[must_use]
    pub fn service_weight(&self, service: &Service, params: &HouseParams) -> Option<f64> {
        service
            .functions
            .iter()
            .filter_map(|id| self.function(id))
            .map(|f| f.tier.weight(params))
            .fold(None, |best, w| Some(best.map_or(w, |b: f64| b.max(w))))
    }
}

/// Round to two decimals, half up, tolerant of binary representation
/// error (3.825 is stored as 3.82499…).
#[must_use]
pub fn round2(value: f64) -> f64 {
    (value * 100.0 + 1e-6).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip_through_day_numbers() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(format_day(0), "1970-01-01");
        let d = parse_day("2026-11-13").unwrap();
        assert_eq!(format_day(d), "2026-11-13");
        assert_eq!(format_day(d + 30), "2026-12-13");
        assert_eq!(d - parse_day("2026-06-16").unwrap(), 150);
        assert_eq!(d - parse_day("2026-02-10").unwrap(), 276);
        assert_eq!(d - parse_day("2025-06-20").unwrap(), 511);
        assert_eq!(
            parse_day("2024-02-29").map(format_day).unwrap(),
            "2024-02-29"
        );
        assert_eq!(parse_day("2026-11-13T10:00:00Z"), Some(d));
        assert_eq!(parse_day("yesterday"), None);
        assert_eq!(parse_day("2026-13-01"), None);
        // A day the month does not have is no date.
        for impossible in [
            "2026-02-30",
            "2026-02-29",
            "2026-04-31",
            "2025-11-31",
            "2026-00-10",
        ] {
            assert_eq!(parse_day(impossible), None, "{impossible}");
        }
        assert!(parse_day("2028-02-29").is_some());
        assert!(parse_day("2026-12-31").is_some());
    }

    #[test]
    fn day_of_floors_toward_the_past() {
        assert_eq!(day_of(0), 0);
        assert_eq!(day_of(DAY_NS - 1), 0);
        assert_eq!(day_of(DAY_NS), 1);
        assert_eq!(day_of(-1), -1);
        assert_eq!(day_of(end_of_day(7)), 7);
    }

    #[test]
    fn tiers_parse_and_weigh() {
        let p = HouseParams::default();
        assert_eq!(Tier::parse("Critical"), Tier::Critical);
        assert_eq!(Tier::parse("important"), Tier::Important);
        assert_eq!(Tier::parse("standard"), Tier::Other);
        assert_eq!(Tier::Critical.weight(&p), 3.0);
        assert_eq!(Tier::Other.weight(&p), 1.5);
        assert!(!Tier::Other.is_critical_or_important());
    }

    #[test]
    fn round2_rounds_ties_up() {
        assert_eq!(round2(3.825), 3.83);
        assert_eq!(round2(3.535), 3.54);
        assert_eq!(round2(4.417_333), 4.42);
        assert_eq!(round2(3.597), 3.6);
    }
}
