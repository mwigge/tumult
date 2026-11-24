//! The recovery trends of `tumult trend`: the RTO and the RPO each run
//! measured, and their margins. A run that did not recover is a point
//! that says so, with the time it watched as a bound. A run that saw no
//! outage, and a run that declares a target but measured nothing, are no
//! point: they are counted.

#![cfg(feature = "duckdb")]

use tumult_core::types::{
    ExperimentStatus, Journal, NfrCriticality, NfrKind, NfrOperator, NfrState, NfrVerdict,
    ObjectiveOutcome, PostResult,
};
use tumult_lake::duckdb_store::sample_journal;
use tumult_lake::{AnalyticsStore, RecoveryMetric, RecoveryPoint, RecoveryTrend};

const S: i64 = 1_000_000_000;
const START_NS: i64 = 1_774_980_000_000_000_000;

/// What the post phase of a run saw: an outage when `outage` is set, and
/// a recovery `mttr_s` seconds after it when that is set.
fn post(outage: bool, mttr_s: Option<f64>) -> PostResult {
    PostResult {
        started_at_ns: START_NS,
        ended_at_ns: START_NS + 30 * S,
        duration_s: 30.0,
        samples: 30,
        probes: vec![],
        recovery_time_s: 29.1,
        full_recovery: mttr_s.is_some() || !outage,
        mttr_s,
        impact_observed: Some(outage),
        outage_started_at_ns: outage.then_some(START_NS - S),
        recovered_at_ns: None,
    }
}

/// A verdict of `kind` named `id` on `threshold`.
fn verdict(id: &str, kind: NfrKind, threshold: f64, observed: Option<f64>) -> NfrVerdict {
    NfrVerdict {
        nfr_id: id.into(),
        kind,
        metric: if kind == NfrKind::Rto {
            "recovery.mttr_s".into()
        } else {
            "probe.data.lost_s".into()
        },
        operator: NfrOperator::Lte,
        threshold,
        observed,
        state: NfrState::Inconclusive,
        margin: None,
        criticality: NfrCriticality::Blocking,
        recovered: None,
        reason: None,
    }
}

/// A run named `id` that started `second` seconds into the day, with the
/// objectives `objectives` and the post phase `post`.
fn run(id: &str, second: i64, objectives: ObjectiveOutcome, post: Option<PostResult>) -> Journal {
    let mut journal = sample_journal(id, ExperimentStatus::Completed);
    journal.started_at_ns = START_NS + second * S;
    journal.objectives = Some(objectives);
    journal.post_result = post;
    journal
}

/// A DR run with an RTO of 10 s that recovered in `mttr_s`.
fn recovered(id: &str, second: i64, mttr_s: f64) -> Journal {
    let verdict = NfrVerdict {
        state: NfrState::Met,
        margin: Some(10.0 - mttr_s),
        recovered: Some(true),
        ..verdict("rto", NfrKind::Rto, 10.0, Some(mttr_s))
    };
    let objectives = ObjectiveOutcome {
        rto_target_s: Some(10.0),
        rto_measured_s: Some(mttr_s),
        verdicts: vec![verdict],
        ..ObjectiveOutcome::default()
    };
    run(id, second, objectives, Some(post(true, Some(mttr_s))))
}

/// A DR run with an RTO of 10 s that did not recover in the 30.1 s it
/// watched from the outage seen.
fn never_back(id: &str, second: i64) -> Journal {
    let verdict = NfrVerdict {
        state: NfrState::Breached,
        recovered: Some(false),
        ..verdict("rto", NfrKind::Rto, 10.0, Some(30.1))
    };
    let objectives = ObjectiveOutcome {
        rto_target_s: Some(10.0),
        verdicts: vec![verdict],
        ..ObjectiveOutcome::default()
    };
    run(id, second, objectives, Some(post(true, None)))
}

/// A DR run with an RTO of 10 s whose probes saw no outage.
fn no_outage(id: &str, second: i64) -> Journal {
    let verdict = NfrVerdict {
        reason: Some(NfrVerdict::NO_OUTAGE_OBSERVED.into()),
        ..verdict("rto", NfrKind::Rto, 10.0, None)
    };
    let objectives = ObjectiveOutcome {
        rto_target_s: Some(10.0),
        verdicts: vec![verdict],
        ..ObjectiveOutcome::default()
    };
    run(id, second, objectives, Some(post(false, None)))
}

/// A DR run with an RTO of 10 s whose method failed: the outage was seen,
/// and the watch does not count.
fn cut_short(id: &str, second: i64) -> Journal {
    let objectives = ObjectiveOutcome {
        rto_target_s: Some(10.0),
        verdicts: vec![verdict("rto", NfrKind::Rto, 10.0, None)],
        ..ObjectiveOutcome::default()
    };
    run(id, second, objectives, Some(post(true, None)))
}

/// A DR run with an RPO of 5 s that lost `lost_s`, or measured none.
fn with_rpo(id: &str, second: i64, lost_s: Option<f64>) -> Journal {
    let objectives = ObjectiveOutcome {
        rpo_target_s: Some(5.0),
        rpo_measured_s: lost_s,
        verdicts: vec![verdict("data-kept", NfrKind::Rpo, 5.0, lost_s)],
        ..ObjectiveOutcome::default()
    };
    run(id, second, objectives, Some(post(true, Some(1.0))))
}

fn trend_of(journals: &[Journal], metric: RecoveryMetric) -> RecoveryTrend {
    let store = AnalyticsStore::in_memory().unwrap();
    for journal in journals {
        store.ingest_journal(journal).unwrap();
    }
    store.recovery_trend(metric).unwrap()
}

/// The id, the value, whether the run recovered and the bound of each point.
fn points(trend: &RecoveryTrend) -> Vec<(&str, f64, Option<bool>, Option<&str>)> {
    trend
        .points
        .iter()
        .map(|p| (p.experiment_id.as_str(), p.value, p.recovered, p.bound))
        .collect()
}

/// Every kind of RTO run, not in the order they ran in, and a run that
/// declares no objective.
fn rto_runs() -> Vec<Journal> {
    let mut plain = sample_journal("plain", ExperimentStatus::Completed);
    plain.post_result = Some(post(true, Some(6.0)));
    vec![
        never_back("never-back", 3),
        recovered("fast", 1, 4.0),
        no_outage("no-outage", 2),
        cut_short("cut-short", 4),
        recovered("slow", 5, 7.5),
        plain,
    ]
}

#[test]
fn an_rto_trend_has_each_run_that_recovered_or_not_oldest_first_and_counts_the_others() {
    let trend = trend_of(&rto_runs(), RecoveryMetric::RtoMeasured);

    assert_eq!(
        points(&trend),
        [
            ("fast", 4.0, Some(true), None),
            ("never-back", 30.1, Some(false), Some("at least")),
            ("slow", 7.5, Some(true), None),
        ]
    );
    assert_eq!(trend.no_outage, 1, "{trend:?}");
    assert_eq!(trend.not_measured, 1, "{trend:?}");
}

#[test]
fn an_rto_margin_is_the_target_minus_the_rto_and_at_most_that_for_a_run_that_did_not_recover() {
    let trend = trend_of(&rto_runs(), RecoveryMetric::RtoMargin);

    let margins: Vec<_> = points(&trend)
        .into_iter()
        .map(|(id, value, recovered, bound)| (id, (value * 10.0).round() / 10.0, recovered, bound))
        .collect();
    assert_eq!(
        margins,
        [
            ("fast", 6.0, Some(true), None),
            ("never-back", -20.1, Some(false), Some("at most")),
            ("slow", 2.5, Some(true), None),
        ]
    );
    assert_eq!(trend.no_outage, 1, "{trend:?}");
    assert_eq!(trend.not_measured, 1, "{trend:?}");
}

#[test]
fn a_point_keeps_the_title_the_status_and_the_start_of_its_run() {
    let mut run = recovered("fast", 1, 4.0);
    run.experiment_title = "Orders failover".into();
    run.status = ExperimentStatus::Deviated;

    let trend = trend_of(&[run], RecoveryMetric::RtoMeasured);

    assert_eq!(
        trend.points,
        [RecoveryPoint {
            experiment_id: "fast".into(),
            title: "Orders failover".into(),
            status: "deviated".into(),
            started_at_ns: START_NS + S,
            value: 4.0,
            recovered: Some(true),
            bound: None,
        }]
    );
}

#[test]
fn a_run_with_an_objective_of_its_own_named_rto_is_one_point() {
    let mut run = never_back("never-back", 1);
    let objectives = run.objectives.as_mut().unwrap();
    let own = verdict("rto", NfrKind::Rto, 20.0, Some(30.1));
    objectives.verdicts.insert(0, own);

    for metric in [RecoveryMetric::RtoMeasured, RecoveryMetric::RtoMargin] {
        let trend = trend_of(std::slice::from_ref(&run), metric);
        assert_eq!(trend.points.len(), 1, "{metric:?}: {trend:?}");
        assert_eq!(trend.points[0].recovered, Some(false), "{metric:?}");
    }
}

#[test]
fn a_run_that_measured_an_rto_without_a_target_has_an_rto_and_no_margin() {
    let mut run = recovered("no-target", 1, 4.0);
    let objectives = run.objectives.as_mut().unwrap();
    objectives.rto_target_s = None;
    objectives.verdicts.clear();

    let rto = trend_of(std::slice::from_ref(&run), RecoveryMetric::RtoMeasured);
    assert_eq!(points(&rto), [("no-target", 4.0, Some(true), None)]);

    let margin = trend_of(&[run], RecoveryMetric::RtoMargin);
    assert_eq!(margin, RecoveryTrend::default());
}

/// A run with an RPO objective of 2 s of its own and no RPO target, that
/// lost `lost_s`, or read none back.
fn own_threshold(id: &str, second: i64, lost_s: Option<f64>) -> Journal {
    let mut run = with_rpo(id, second, lost_s);
    let objectives = run.objectives.as_mut().unwrap();
    objectives.rpo_target_s = None;
    objectives.verdicts[0].threshold = 2.0;
    run
}

/// RPO runs: two that measured a loss, one that measured none, two with
/// an objective of their own and no target, and an RTO run.
fn rpo_runs() -> Vec<Journal> {
    vec![
        with_rpo("lost-more", 3, Some(4.0)),
        with_rpo("lost-little", 1, Some(2.0)),
        with_rpo("none-read", 2, None),
        own_threshold("own-threshold", 4, Some(0.5)),
        own_threshold("own-none-read", 6, None),
        recovered("rto-only", 5, 4.0),
    ]
}

#[test]
fn an_rpo_trend_has_each_run_that_measured_one_and_counts_the_others() {
    let trend = trend_of(&rpo_runs(), RecoveryMetric::RpoMeasured);

    assert_eq!(
        points(&trend),
        [
            ("lost-little", 2.0, None, None),
            ("lost-more", 4.0, None, None),
            ("own-threshold", 0.5, None, None),
        ]
    );
    assert_eq!(trend.not_measured, 2, "{trend:?}");
    assert_eq!(trend.no_outage, 0, "{trend:?}");
}

#[test]
fn an_rpo_margin_is_the_target_minus_the_rpo_of_each_run_that_declares_a_target() {
    let trend = trend_of(&rpo_runs(), RecoveryMetric::RpoMargin);

    assert_eq!(
        points(&trend),
        [
            ("lost-little", 3.0, None, None),
            ("lost-more", 1.0, None, None)
        ]
    );
    assert_eq!(trend.not_measured, 1, "{trend:?}");
}

#[test]
fn an_rpo_target_that_no_objective_measures_is_a_run_that_measured_no_rpo() {
    let objectives = ObjectiveOutcome {
        rpo_target_s: Some(5.0),
        verdicts: vec![NfrVerdict {
            reason: Some(NfrVerdict::NO_RPO_OBJECTIVE.into()),
            ..verdict("rpo", NfrKind::Rpo, 5.0, None)
        }],
        ..ObjectiveOutcome::default()
    };
    let journal = run("target-only", 1, objectives, None);

    for metric in [RecoveryMetric::RpoMeasured, RecoveryMetric::RpoMargin] {
        let trend = trend_of(std::slice::from_ref(&journal), metric);
        assert!(trend.points.is_empty(), "{metric:?}: {trend:?}");
        assert_eq!(trend.not_measured, 1, "{metric:?}");
    }
}

#[test]
fn a_recovery_metric_is_found_by_its_name() {
    let names: Vec<&str> = RecoveryMetric::ALL.iter().map(|m| m.name()).collect();
    assert_eq!(
        names,
        [
            "rto_measured_s",
            "rto_margin_s",
            "rpo_measured_s",
            "rpo_margin_s"
        ]
    );
    for metric in RecoveryMetric::ALL {
        assert_eq!(RecoveryMetric::from_name(metric.name()), Some(metric));
    }
    assert_eq!(RecoveryMetric::from_name("duration_ms"), None);
    assert!(RecoveryMetric::RtoMargin.is_margin());
    assert!(RecoveryMetric::RpoMargin.is_margin());
    assert!(!RecoveryMetric::RtoMeasured.is_margin());
    assert!(!RecoveryMetric::RpoMeasured.is_margin());
}
