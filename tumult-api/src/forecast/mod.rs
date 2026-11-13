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
//! passed. `rejected` is the one code that does not: it is a request the
//! sidecar turned down with 400, 413 or 422, or one too large to send.
//! Every other status (401, 403, 404, 408, 429 and 5xx included) would
//! meet every request alike and opens the circuit.
//! Whoever made it, a band narrower than the series can resolve
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

/// Log why the sidecar did not answer. The detail is the sidecar's own
/// words or a transport error; it goes nowhere but here.
fn log_sidecar_failure(error: &client::SidecarError) {
    tracing::warn!(
        error.type = error.kind.code(),
        error.message = %error.detail,
        "forecast sidecar failed; using the built-in provider"
    );
}

/// Ask the sidecar unless its circuit is open; the error is the coarse
/// reason the built-in provider has to step in.
///
/// A request the sidecar would never be sent is turned down before the
/// breaker is asked: it says nothing about the sidecar and must not take
/// the half-open probe.
async fn ask_sidecar(
    state: &ApiState,
    client: &client::TimestralClient,
    parent: &telemetry::Timed,
    request: &ForecastRequest<'_>,
) -> Result<Forecast, &'static str> {
    if let Err(e) = client::check_limits(request.context.len(), request.horizon) {
        log_sidecar_failure(&e);
        return Err(e.kind.code());
    }
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
            log_sidecar_failure(&e);
            if e.kind.trips_breaker() {
                breaker.record_failure(permit, Instant::now());
            } else {
                breaker.record_rejection(permit, Instant::now());
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

    fn state_with_sidecar(url: &str, cooldown: std::time::Duration) -> ApiState {
        ApiState::new(
            "unused.duckdb".into(),
            "unused".into(),
            "unused".into(),
            std::sync::Arc::new(tumult_intelligence::llm::OpenAiCompatClient::from_env()),
            tumult_compliance::OrgTree::empty(),
            None,
            None,
            None,
            false,
        )
        .with_forecast_cooldown(cooldown)
        .with_forecast_url(Some(url.into()))
        .unwrap()
    }

    async fn ask(
        state: &ApiState,
        context: &[f64],
        horizon: usize,
    ) -> Result<Forecast, &'static str> {
        let span = telemetry::Timed::start(telemetry::PRODUCE);
        let request = ForecastRequest {
            context,
            horizon,
            levels: &DEFAULT_LEVELS,
            covariates: None,
        };
        let client = state.forecast_client().unwrap();
        ask_sidecar(state, client, &span, &request).await
    }

    /// A request the sidecar would never be sent says nothing about the
    /// sidecar: it is turned down before the breaker is asked, so it
    /// neither takes the half-open probe nor reads as `circuit_open`.
    #[tokio::test]
    async fn an_oversized_request_is_turned_down_before_the_breaker_is_asked() {
        let cooldown = std::time::Duration::from_secs(600);
        let state = state_with_sidecar("http://127.0.0.1:1", cooldown);
        let breaker = state.forecast_breaker();
        let now = Instant::now();
        breaker.record_failure(breaker.admit(now).unwrap(), now);
        let horizon = client::MAX_HORIZON + 1;
        assert_eq!(
            ask(&state, &[1.0, 2.0], horizon).await.err(),
            Some("rejected")
        );
        assert_eq!(ask(&state, &[1.0, 2.0], 4).await.err(), Some(CIRCUIT_OPEN));
    }

    /// A half-open probe that is answered 422 has not shown the sidecar
    /// can forecast. The circuit stays half open for the next request.
    #[tokio::test]
    async fn a_rejected_probe_does_not_close_the_circuit() {
        use axum::http::StatusCode;
        let app = axum::Router::new().route(
            "/forecast",
            axum::routing::post(|| async { (StatusCode::UNPROCESSABLE_ENTITY, "no") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cooldown = std::time::Duration::from_millis(40);
        let state = state_with_sidecar(&format!("http://{addr}"), cooldown);
        let breaker = state.forecast_breaker();
        let now = Instant::now();
        breaker.record_failure(breaker.admit(now).unwrap(), now);
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        assert_eq!(ask(&state, &[1.0, 2.0], 4).await.err(), Some("rejected"));
        let later = Instant::now();
        assert!(breaker.admit(later).is_some(), "the next request probes");
        assert!(breaker.admit(later).is_none(), "and it is the only one");
    }

    /// Every `tracing` event in `source`, from the macro to its `);`.
    fn tracing_events(source: &str) -> Vec<&str> {
        let marker = concat!("tracing", "::");
        source
            .match_indices(marker)
            .filter_map(|(at, _)| {
                let rest = &source[at..];
                let open = rest.find("!(")?;
                let name = &rest[marker.len()..open];
                if !["warn", "error", "info", "debug", "trace"].contains(&name) {
                    return None;
                }
                rest.find(");").map(|end| &rest[..end])
            })
            .collect()
    }

    /// Logs are structured: what varies is a field, named the way the
    /// rest of the daemon names it, and the message is a constant.
    #[test]
    fn forecast_log_events_carry_fields_and_a_constant_message() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/forecast");
        let mut sources = vec![concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs").to_string()];
        for entry in std::fs::read_dir(dir).unwrap() {
            sources.push(entry.unwrap().path().to_string_lossy().into_owned());
        }
        let mut seen = 0;
        for path in sources {
            let source = std::fs::read_to_string(&path).unwrap();
            for event in tracing_events(&source) {
                let forecast_event = path.contains("/forecast/") || event.contains("FORECAST");
                if !forecast_event {
                    continue;
                }
                seen += 1;
                assert!(event.contains("error.message = "), "{path}: {event}");
                assert!(event.contains("error.type = "), "{path}: {event}");
                assert!(!event.contains(" error = "), "{path}: {event}");
                let message = event.rsplit('"').nth(1).unwrap_or_default();
                assert!(!message.contains('{'), "{path}: interpolated: {event}");
            }
        }
        assert!(seen >= 4, "only {seen} forecast events were found");
    }
}
