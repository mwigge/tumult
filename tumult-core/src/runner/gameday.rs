//! `run_gameday` — coordinated multi-experiment campaign runner and scoring.

use crate::controls::ControlRegistry;
use crate::types::{
    Experiment, ExperimentStatus, GameDay, GameDayExperiment, GameDayJournal, Journal,
    ResilienceScore,
};

use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;

use super::telemetry::epoch_nanos_now;
use super::{run_experiment, ActivityExecutor, RunConfig, RunnerError, TRACER_NAME};

/// Runs a `GameDay` — a coordinated campaign of experiments under shared load.
///
/// Iterates the provided experiments in sequence, optionally running a
/// shared load generator across all of them. Computes an aggregate
/// `ResilienceScore` and returns a `GameDayJournal`.
///
/// # Errors
///
/// Returns [`RunnerError::ExperimentCountMismatch`] if `experiments` does
/// not line up one-to-one with `gameday.experiments`, and [`RunnerError`]
/// if any experiment fails to execute (not if it deviates — deviation is a
/// valid outcome captured in the journal).
#[must_use = "the GameDayJournal contains the aggregate results"]
#[allow(clippy::too_many_lines)] // Orchestration function with OTel setup, load management, and scoring
pub fn run_gameday(
    gameday: &GameDay,
    experiments: &[Experiment],
    executor: &std::sync::Arc<dyn ActivityExecutor>,
    controls: &std::sync::Arc<ControlRegistry>,
    config: &RunConfig,
) -> Result<GameDayJournal, RunnerError> {
    // Journals are attributed to `gameday.experiments` entries by position
    // (e.g. for compliance coverage), so reject misaligned input up front
    // instead of trusting callers to keep the two lists in sync — a release
    // build would otherwise silently attribute coverage to the wrong
    // articles.
    if gameday.experiments.len() != experiments.len() {
        return Err(RunnerError::ExperimentCountMismatch {
            declared: gameday.experiments.len(),
            provided: experiments.len(),
        });
    }

    let gameday_id = uuid::Uuid::new_v4().to_string();
    let started = std::time::Instant::now();
    let started_at_ns = epoch_nanos_now();

    // Create root GameDay OTel span
    let tracer = opentelemetry::global::tracer(TRACER_NAME);
    let gd_span = tracer
        .span_builder("resilience.gameday")
        .with_attributes(vec![
            KeyValue::new("resilience.gameday.id", gameday_id.clone()),
            KeyValue::new("resilience.gameday.title", gameday.title.clone()),
            KeyValue::new(
                "resilience.gameday.experiment_count",
                i64::try_from(experiments.len()).unwrap_or(0),
            ),
        ])
        .start(&tracer);
    let gd_cx = opentelemetry::Context::current_with_span(gd_span);
    let _gd_guard = gd_cx.attach();

    // Start shared load (if configured)
    let load_handle = if let (Some(ref load_config), Some(ref load_exec)) =
        (&gameday.load, &config.load_executor)
    {
        match load_exec.start(load_config) {
            Ok(handle) => {
                tracing::info!(
                    tool = %load_config.tool,
                    "gameday load started"
                );
                Some(handle)
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to start gameday load");
                None
            }
        }
    } else {
        None
    };

    // Run each experiment with the GameDay span as parent context
    let mut journals = Vec::with_capacity(experiments.len());
    for experiment in experiments {
        let exp_config = RunConfig {
            rollback_strategy: config.rollback_strategy.clone(),
            cancellation_token: config.cancellation_token.clone(),
            parent_context: Some(opentelemetry::Context::current()),
            load_executor: None, // load is managed at GameDay level
            max_concurrent_faults: config.max_concurrent_faults,
        };
        match run_experiment(experiment, executor, controls, &exp_config) {
            Ok(journal) => journals.push(journal),
            Err(e) => {
                tracing::error!(error = %e, title = %experiment.title, "gameday experiment failed");
                return Err(e);
            }
        }
    }

    // Stop load and collect results
    let load_result =
        if let (Some(handle), Some(ref load_exec)) = (load_handle, &config.load_executor) {
            match load_exec.stop(handle) {
                Ok(result) => {
                    tracing::info!(
                        throughput_rps = result.throughput_rps,
                        "gameday load completed"
                    );
                    Some(result)
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to collect gameday load results");
                    None
                }
            }
        } else {
            None
        };

    // Compute resilience score
    let total = journals.len();
    let passed = journals
        .iter()
        .filter(|j| j.status == ExperimentStatus::Completed)
        .count();
    #[allow(clippy::cast_precision_loss)]
    let pass_rate = if total > 0 {
        passed as f64 / total as f64
    } else {
        0.0
    };

    // Recovery compliance: check MTTR against target
    let recovery = compute_recovery_compliance(&journals, gameday.scoring.mttr_target_s);

    // Load impact tolerance (1.0 if no load, otherwise based on error rate)
    let load_impact = load_result
        .as_ref()
        .map_or(1.0, |lr| (1.0 - lr.error_rate).max(0.0));

    // Compliance coverage: pair each declared experiment with its journal by
    // construction (same index, lengths validated above) and count mapped
    // articles that have passing experiments.
    let compliance_pairs: Vec<(&GameDayExperiment, &Journal)> =
        gameday.experiments.iter().zip(&journals).collect();
    let compliance = compute_compliance_coverage(&compliance_pairs);

    let score = ResilienceScore::compute(pass_rate, recovery, load_impact, compliance);
    let compliance_status = score.status().to_string();

    let ended_at_ns = epoch_nanos_now();
    #[allow(clippy::cast_precision_loss)]
    let duration_s = started.elapsed().as_secs_f64();

    Ok(GameDayJournal {
        gameday_id,
        title: gameday.title.clone(),
        started_at_ns,
        ended_at_ns,
        duration_s,
        experiment_journals: journals,
        load_result,
        resilience_score: score,
        compliance_status,
        regulatory: gameday.regulatory.clone(),
    })
}

/// Computes recovery compliance score from MTTR data in journals.
fn compute_recovery_compliance(journals: &[Journal], mttr_target_s: f64) -> f64 {
    let mut total_recovery = 0;
    let mut compliant_recovery = 0;

    for journal in journals {
        if let Some(ref post) = journal.post_result {
            total_recovery += 1;
            if post.recovery_time_s <= mttr_target_s && post.full_recovery {
                compliant_recovery += 1;
            }
        }
    }

    if total_recovery == 0 {
        1.0 // No recovery data → assume compliant
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            f64::from(compliant_recovery) / f64::from(total_recovery)
        }
    }
}

/// Computes compliance coverage from article mappings.
///
/// Each pair holds a declared `GameDay` experiment and the journal produced
/// by running it, so coverage is attributed to the right articles by
/// construction rather than by positional trust across two parallel lists.
fn compute_compliance_coverage(pairs: &[(&GameDayExperiment, &Journal)]) -> f64 {
    // Collect all unique mapped articles
    let all_articles: std::collections::HashSet<&str> = pairs
        .iter()
        .flat_map(|(exp, _)| exp.compliance_maps.iter().map(String::as_str))
        .collect();

    if all_articles.is_empty() {
        return 1.0; // No articles mapped → full coverage by default
    }

    // An article is "met" if at least one experiment mapped to it completed
    let mut met = 0;
    for article in &all_articles {
        let has_passing = pairs.iter().any(|(exp, journal)| {
            exp.compliance_maps.iter().any(|a| a == article)
                && journal.status == ExperimentStatus::Completed
        });
        if has_passing {
            met += 1;
        }
    }

    #[allow(clippy::cast_precision_loss)]
    {
        f64::from(met) / all_articles.len() as f64
    }
}
