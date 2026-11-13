//! `GET /api/forecast` ([`metric`]) and `GET /api/runs/{id}/impact`
//! ([`run_impact`]) — the forecasting endpoints.
//!
//! Both pull a series from the lake, resample it onto the series contract
//! (regular step, `NaN` gaps, capped) and ask a provider for quantile
//! paths. The provider is the Timestral sidecar when `TUMULTD_FORECAST_URL`
//! is set and answers; otherwise — unset, unreachable, or a bad answer —
//! the built-in Holt-Winters provider, and the response's `provenance`
//! says which one it was. When the built-in provider stepped in for a
//! configured sidecar, `fallback_reason` is one coarse code:
//! `unreachable`, `timeout`, `bad_response`, `rejected` or
//! `circuit_open`. What exactly went wrong is in the daemon's log only.
//!
//! A failing sidecar opens a circuit breaker ([`breaker`]) so the requests
//! that follow skip it and get the fallback at once until a cool-down has
//! passed. Whoever made it, a band narrower than the series can resolve
//! is widened to [`tumult_forecast::band::floor`] and flagged
//! `degenerate_band`. Spans and counters are described in [`telemetry`].

pub mod breaker;
pub mod client;
mod metric;
mod run_impact;
mod telemetry;

use std::time::Instant;

use serde_json::{json, Value};
use tumult_forecast::resample::RegularSeries;
use tumult_forecast::{
    band, Covariates, Forecast, ForecastError, ForecastProvider, ForecastRequest, HoltWinters,
    DEFAULT_LEVELS,
};

use crate::ApiState;

pub(crate) use metric::forecast;
pub(crate) use run_impact::run_impact;

const SECOND_NS: i64 = 1_000_000_000;
const MS_NS: i64 = 1_000_000;
const BUILT_IN_MODEL: &str = "holt-winters";
/// `fallback_reason` while the sidecar is being skipped.
const CIRCUIT_OPEN: &str = "circuit_open";

/// A forecast plus where it came from.
pub(crate) struct Produced {
    pub forecast: Forecast,
    /// `None` when the chosen provider answered; the coarse code for why
    /// the built-in provider stepped in otherwise.
    pub fallback_reason: Option<&'static str>,
}

/// Ask the sidecar unless its circuit is open; the error is the coarse
/// reason the built-in provider has to step in.
async fn ask_sidecar(
    state: &ApiState,
    client: &client::TimestralClient,
    parent: &telemetry::Timed,
    request: &ForecastRequest<'_>,
) -> Result<Forecast, &'static str> {
    let breaker = state.forecast_breaker();
    let Some(permit) = breaker.admit(Instant::now()) else {
        return Err(CIRCUIT_OPEN);
    };
    let span = parent.child(telemetry::SIDECAR_CALL);
    let answer = client
        .forecast(
            request.context,
            request.horizon,
            request.levels,
            request.covariates,
        )
        .await;
    match answer {
        Ok(mut forecast) => {
            span.finish("ok");
            breaker.record_success(permit);
            forecast.apply_band_floor(band::floor(request.context));
            Ok(forecast)
        }
        Err(e) => {
            let code = e.kind.code();
            span.finish(code);
            tracing::warn!(
                reason = code,
                detail = %e.detail,
                "forecast sidecar failed; using the built-in provider"
            );
            if e.kind.trips_breaker() {
                breaker.record_failure(permit, Instant::now());
            } else {
                // It answered, so it is alive: a probe's rejection closes
                // the circuit just as its forecast would have.
                breaker.record_success(permit);
            }
            Err(code)
        }
    }
}

/// The built-in fit, off the async workers: a long context is CPU work.
async fn built_in(request: &ForecastRequest<'_>) -> Result<Forecast, ForecastError> {
    let context = request.context.to_vec();
    let covariates = request.covariates.cloned();
    let horizon = request.horizon;
    tokio::task::spawn_blocking(move || {
        HoltWinters::new().forecast(&ForecastRequest {
            context: &context,
            horizon,
            levels: &DEFAULT_LEVELS,
            covariates: covariates.as_ref(),
        })
    })
    .await
    .map_err(|e| ForecastError::Provider(format!("the built-in fit did not finish: {e}")))?
}

/// Ask the configured provider, falling back to Holt-Winters.
pub(crate) async fn produce(
    state: &ApiState,
    context: &[f64],
    horizon: usize,
    covariates: Option<&Covariates>,
) -> Result<Produced, ForecastError> {
    let span = telemetry::Timed::start(telemetry::PRODUCE);
    let request = ForecastRequest {
        context,
        horizon,
        levels: &DEFAULT_LEVELS,
        covariates,
    };
    let mut fallback_reason = None;
    if let Some(client) = state.forecast_client() {
        match ask_sidecar(state, client, &span, &request).await {
            Ok(forecast) => {
                span.finish("sidecar");
                return Ok(Produced {
                    forecast,
                    fallback_reason: None,
                });
            }
            Err(reason) => {
                telemetry::fallback(reason);
                fallback_reason = Some(reason);
            }
        }
    }
    match built_in(&request).await {
        Ok(forecast) => {
            span.finish(if fallback_reason.is_some() {
                "fallback"
            } else {
                "built_in"
            });
            Ok(Produced {
                forecast,
                fallback_reason,
            })
        }
        Err(e) => {
            span.finish("error");
            Err(e)
        }
    }
}

/// Why the configured sidecar is being skipped right now, if it is.
/// Looking does not start a half-open probe.
fn sidecar_skip_reason(state: &ApiState) -> Option<&'static str> {
    state.forecast_client()?;
    state
        .forecast_breaker()
        .is_open(Instant::now())
        .then_some(CIRCUIT_OPEN)
}

fn number(v: f64) -> Value {
    if v.is_finite() {
        json!(v)
    } else {
        Value::Null
    }
}

fn nullable(values: &[f64]) -> Vec<Value> {
    values.iter().map(|v| number(*v)).collect()
}

fn provenance_json(forecast: &Forecast, fallback_reason: Option<&str>) -> Value {
    json!({
        "model": forecast.provenance.model,
        "version": forecast.provenance.version,
        "weights_hash": forecast.provenance.weights_hash,
        "fallback_reason": fallback_reason,
    })
}

/// Observed context followed by the horizon as one chart-ready series:
/// `ts` (ms) across both, `observed` null over the horizon, `median` and the
/// 10–90 band null over the context.
fn chart_series(series: &RegularSeries, forecast: &Forecast, offset: usize) -> Value {
    let n = series.len();
    let horizon = forecast.horizon();
    let ts: Vec<i64> = (0..offset + horizon)
        .map(|i| series.ts_ns(i) / MS_NS)
        .collect();
    let mut observed = nullable(&series.values[..offset.min(n)]);
    observed.resize(offset + horizon, Value::Null);
    let pad = |values: Vec<f64>| -> Vec<Value> {
        let mut out = vec![Value::Null; offset];
        out.extend(nullable(&values));
        out
    };
    let (lower, upper) = forecast.band(0.1, 0.9).unwrap_or_default();
    json!({
        "ts": ts,
        "observed": observed,
        "median": pad(forecast.median()),
        "bands": [{"level": 0.8, "lower": pad(lower), "upper": pad(upper)}],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_series_pads_context_and_horizon() {
        let series = RegularSeries {
            start_ns: 0,
            step_ns: SECOND_NS,
            values: vec![1.0, f64::NAN],
        };
        let forecast = HoltWinters::new()
            .forecast(&ForecastRequest::new(&series.values, 2))
            .unwrap();
        let v = chart_series(&series, &forecast, 2);
        assert_eq!(v["ts"].as_array().unwrap().len(), 4);
        assert_eq!(v["observed"][0], 1.0);
        assert!(v["observed"][1].is_null() && v["observed"][2].is_null());
        assert!(v["median"][0].is_null());
        assert!(v["median"][2].is_number());
        assert_eq!(v["bands"][0]["level"], 0.8);
    }

    #[test]
    fn numbers_that_are_not_finite_become_null() {
        assert_eq!(number(1.5), json!(1.5));
        assert!(number(f64::NAN).is_null());
        assert!(number(f64::INFINITY).is_null());
        assert_eq!(nullable(&[1.0, f64::NAN]), vec![json!(1.0), Value::Null]);
    }
}
