//! The two worked examples the score is pinned to: a mid-size bank under
//! the full framework and a small investment firm under the simplified
//! one. Test fixtures only.

use tumult_core::compliance::EntityProfile;

use super::coverage::Matrix;
use super::model::{
    day_of, end_of_day, parse_day, Evidence, Facts, Function, Incident, IncidentFeed, Inputs,
    Outcome, PlannedTest, Service, Slo, SloCriticality, SloFeed, Tier, DAY_NS,
};
use super::params::HouseParams;
use super::status;

/// The inputs with the rows the daily job would have stored had it run
/// on each of the `days` days before the as-of day: the status of every
/// cell as of the end of that day. Fixtures use it to state "the job has
/// been running"; production reads stored rows and never does this.
pub fn with_stored_days(mut inputs: Inputs, days: i64) -> Inputs {
    let params = HouseParams::default();
    let today = day_of(inputs.as_of_ns);
    let mut past = inputs.clone();
    past.snapshots.clear();
    for day in (today - days)..today {
        past.as_of_ns = end_of_day(day);
        for row in status::daily(&Matrix::build(&past, &params)) {
            let key = (row.function_id, row.requirement_id.to_string());
            inputs
                .snapshots
                .entry(key)
                .or_default()
                .insert(day, row.value);
        }
    }
    inputs
}

/// Days of stored history the worked examples carry: past the Operating
/// window, so every streak is decided by the evidence, not by the store.
pub const STORED_DAYS: i64 = 200;

/// Noon UTC of a `YYYY-MM-DD` date, in nanoseconds.
pub fn ns(date: &str) -> i64 {
    let day = parse_day(date).unwrap_or_else(|| panic!("{date} is not a date"));
    day * DAY_NS + DAY_NS / 2
}

const AS_OF: &str = "2026-11-13";

/// Noon, `days` days before the as-of date.
fn ago(days: i64) -> i64 {
    ns(AS_OF) - days * DAY_NS
}

fn plan(kinds: &[&str]) -> Vec<PlannedTest> {
    kinds
        .iter()
        .map(|kind| PlannedTest {
            kind: (*kind).to_string(),
            interval_days: 365,
        })
        .collect()
}

fn failed(mut evidence: Evidence) -> Evidence {
    evidence.outcome = Outcome::Failed;
    evidence
}

fn partial(mut evidence: Evidence) -> Evidence {
    evidence.outcome = Outcome::Partial;
    evidence
}

/// A recovery run with a measured recovery time and recovery point.
fn recovered(mut evidence: Evidence, rto_s: f64, rpo_s: f64) -> Evidence {
    evidence.rto_measured_s = Some(rto_s);
    evidence.rpo_measured_s = Some(rpo_s);
    evidence
}

/// A failed run and, when `closed_ago` is given, the later passing run of
/// the same experiment that closes the finding.
fn finding(function: &str, kind: &str, failed_ago: i64, closed_ago: Option<i64>) -> Vec<Evidence> {
    let experiment = format!("{function}-finding");
    let mut fail = failed(Evidence::run(
        &format!("{experiment}-fail"),
        function,
        kind,
        ago(failed_ago),
    ));
    fail.experiment.clone_from(&experiment);
    let mut rows = vec![fail];
    if let Some(closed) = closed_ago {
        let mut pass = Evidence::run(&format!("{experiment}-pass"), function, kind, ago(closed));
        pass.experiment = experiment;
        rows.push(pass);
    }
    rows
}

/// A verified manual record for the whole legal entity citing `rows`.
fn attestation(id: &str, rows: &[&str], days_ago: i64) -> Evidence {
    let mut record = Evidence::manual(id, &[], "other", ago(days_ago));
    record.cited = rows.iter().map(|row| (*row).to_string()).collect();
    record
}

const BANK_KINDS: [&str; 7] = [
    "chaos",
    "dr",
    "load",
    "performance",
    "robustness",
    "security",
    "nfr",
];

/// (function, tier, disciplines never run, disciplines run with a partial
/// outcome, age in days of the baseline runs).
type BankFunction = (
    &'static str,
    Tier,
    &'static [&'static str],
    &'static [&'static str],
    i64,
);

const BANK_FUNCTIONS: [BankFunction; 8] = [
    ("cif-0001", Tier::Critical, &["nfr"], &[], 200),
    (
        "cif-0002",
        Tier::Critical,
        &["security", "robustness"],
        &[],
        200,
    ),
    (
        "cif-0003",
        Tier::Critical,
        &["performance"],
        &["robustness"],
        200,
    ),
    ("cif-0011", Tier::Important, &["load"], &[], 200),
    (
        "cif-0012",
        Tier::Important,
        &["nfr", "performance"],
        &[],
        200,
    ),
    (
        "cif-0013",
        Tier::Important,
        &[],
        &["security", "robustness"],
        100,
    ),
    (
        "ops-0021",
        Tier::Other,
        &["load", "performance", "robustness", "security", "nfr"],
        &[],
        200,
    ),
    (
        "ops-0022",
        Tier::Other,
        &["load", "performance", "robustness", "security", "nfr"],
        &[],
        200,
    ),
];

fn bank_function(id: &str, tier: Tier) -> Function {
    Function {
        rto_s: Some(3600.0),
        rpo_s: Some(300.0),
        last_assessed_ns: Some(ago(60)),
        declared: vec!["dora.art28.8".to_string()],
        test_plan: plan(&["chaos", "dr"]),
        ..Function::new(id, tier)
    }
}

fn bank_baseline() -> Vec<Evidence> {
    let mut rows = Vec::new();
    for (function, _, never, partially, age) in BANK_FUNCTIONS {
        for kind in BANK_KINDS.iter().filter(|kind| !never.contains(kind)) {
            let mut run = Evidence::run(&format!("{function}-{kind}"), function, kind, ago(age));
            if *kind == "dr" {
                run = recovered(run, 1800.0, 60.0);
            }
            if partially.contains(kind) {
                run = partial(run);
            }
            rows.push(run);
        }
    }
    rows
}

/// (service count, conformance level) per function, in function order.
const BANK_SERVICES: [(&str, &[u8]); 8] = [
    ("cif-0001", &[4, 4]),
    ("cif-0002", &[4]),
    ("cif-0003", &[4]),
    ("cif-0011", &[4, 4, 3]),
    ("cif-0012", &[3, 3, 3]),
    ("cif-0013", &[3, 3]),
    ("ops-0021", &[3, 3, 3, 3]),
    ("ops-0022", &[2, 1, 1, 1]),
];

fn bank_services() -> Vec<Service> {
    let mut services = Vec::new();
    for (function, levels) in BANK_SERVICES {
        for (n, level) in levels.iter().enumerate() {
            services.push(Service::observed(
                &format!("{function}-svc-{n}"),
                *level,
                &[function],
                ago(1),
            ));
        }
    }
    services
}

fn bank_slos() -> Vec<Slo> {
    BANK_FUNCTIONS
        .iter()
        .map(|(function, ..)| Slo {
            id: format!("{function}-availability"),
            function: Some((*function).to_string()),
            service: None,
            criticality: SloCriticality::Blocking,
            met: true,
            remaining: if matches!(*function, "cif-0001" | "cif-0011") {
                0.2
            } else {
                0.6
            },
        })
        .collect()
}

const INCIDENT_ROWS: [&str; 6] = [
    "dora.art11.8",
    "dora.art13.3",
    "dora.art18.1",
    "dora.art19.4.a",
    "dora.art19.4.b",
    "dora.art19.4.c",
];

fn bank_incidents() -> Vec<Incident> {
    [
        (150, true, true),
        (100, true, true),
        (70, true, false),
        (20, false, false),
    ]
    .into_iter()
    .map(|(days_ago, closed, previously_tested)| Incident {
        id: format!("inc-{days_ago}"),
        functions: Vec::new(),
        at_ns: ago(days_ago),
        satisfied: INCIDENT_ROWS.iter().map(|id| (*id).to_string()).collect(),
        closed,
        previously_tested,
    })
    .collect()
}

fn bank_manual() -> Vec<Evidence> {
    let everyone: Vec<&str> = BANK_FUNCTIONS.iter().map(|(id, ..)| *id).collect();
    let mut drill = Evidence::manual("continuity-drill", &everyone, "drill", ago(60));
    drill.cited = vec!["rts1774.art25".to_string()];
    let tabletop = Evidence::manual("crisis-comms-tabletop", &everyone, "tabletop", ago(500));
    vec![
        attestation("ict-risk-review", &["dora.art6.5", "dora.art13.5"], 30),
        attestation("comms-plans", &["dora.art11.7"], 30),
        attestation("backup-policy", &["dora.art12.1", "dora.art12.3"], 30),
        attestation("third-party-strategy", &["dora.art28.2"], 30),
        // The assessment date of the register is a declaration; this
        // stored record is what the Art. 24(3) row scores.
        attestation("criticality-assessment", &["dora.art24.3"], 60),
        drill,
        tabletop,
    ]
}

/// Worked example A: a mid-size bank, profile `full`, not designated for
/// TLPT. Eight functions, twenty services, as of 2026-11-13.
pub fn bank_a() -> Inputs {
    let mut inputs = Inputs::empty(ns(AS_OF), EntityProfile::full());
    inputs.functions = BANK_FUNCTIONS
        .iter()
        .map(|(id, tier, ..)| bank_function(id, *tier))
        .collect();
    inputs.evidence = bank_baseline();
    // One important function breached its recovery time a month ago.
    inputs.evidence.push(recovered(
        Evidence::run("cif-0012-dr-latest", "cif-0012", "dr", ago(30)),
        7200.0,
        60.0,
    ));
    for (function, kind) in [
        ("cif-0001", "load"),
        ("cif-0002", "chaos"),
        ("cif-0003", "load"),
        ("cif-0011", "chaos"),
        ("cif-0012", "chaos"),
    ] {
        inputs
            .evidence
            .extend(finding(function, kind, 120, Some(90)));
    }
    inputs
        .evidence
        .extend(finding("ops-0021", "load", 40, None));
    inputs.evidence.extend(bank_manual());
    inputs.services = bank_services();
    inputs.slos = SloFeed::Connected {
        slos: bank_slos(),
        history_days: 425,
    };
    inputs.incidents = IncidentFeed::Connected(bank_incidents());
    inputs.facts = Facts {
        alert_rules_exported: true,
        topology_observed: true,
        register_connected: true,
        register_reconciled_ns: Some(ago(20)),
        ..Facts::default()
    };
    with_stored_days(inputs, STORED_DAYS)
}

/// Worked example B: a small investment firm, profile `simplified`, not
/// a microenterprise. Three functions, five services, as of 2026-11-13.
pub fn firm_b() -> Inputs {
    let entity = EntityProfile::parse("simplified", &[])
        .unwrap_or_else(|e| panic!("simplified is a profile: {e}"));
    let mut inputs = Inputs::empty(ns(AS_OF), entity);
    let assessed = Some(ago(60));
    inputs.functions = vec![
        Function {
            name: "Payments".into(),
            rto_s: Some(14_400.0),
            last_assessed_ns: assessed,
            test_plan: plan(&["dr"]),
            ..Function::new("cif-0001", Tier::Critical)
        },
        Function {
            name: "Reporting".into(),
            rto_s: Some(28_800.0),
            last_assessed_ns: assessed,
            test_plan: plan(&["dr"]),
            ..Function::new("cif-0002", Tier::Important)
        },
        Function {
            name: "Intranet".into(),
            last_assessed_ns: assessed,
            ..Function::new("ops-0003", Tier::Other)
        },
    ];
    let restore = recovered(
        Evidence::run("payments-restore", "cif-0001", "dr", ns("2026-06-16")),
        9000.0,
        0.0,
    );
    let mut manual_restore = Evidence::manual(
        "reporting-restore",
        &["cif-0002"],
        "drill",
        ns("2026-02-10"),
    );
    manual_restore.kind = Some("dr".into());
    inputs.evidence = vec![restore, manual_restore];
    inputs
        .evidence
        .extend(finding("cif-0001", "chaos", 80, Some(50)));
    inputs
        .evidence
        .extend(finding("cif-0002", "load", 60, None));
    inputs.services = vec![
        Service::observed("payments-api", 2, &["cif-0001"], ago(1)),
        Service::observed("payments-ledger", 2, &["cif-0001"], ago(1)),
        Service::observed("reporting-api", 1, &["cif-0002"], ago(1)),
        Service::observed("reporting-batch", 1, &["cif-0002"], ago(1)),
        Service::observed("intranet", 0, &["ops-0003"], ago(1)),
    ];
    inputs.incidents = IncidentFeed::Connected(Vec::new());
    with_stored_days(inputs, STORED_DAYS)
}
