//! `GET /api/forecast` (`metric`) and `GET /api/runs/{id}/impact`
//! (`run_impact`) — the forecasting endpoints.
//!
//! Both pull a series from the lake, resample it onto the series contract
//! (regular step, `NaN` gaps, capped) and ask a provider for quantile
//! paths. The provider is the Timestral sidecar when `TUMULTD_FORECAST_URL`
//! is set and answers; otherwise — unset, unreachable, or a bad answer —
//! the built-in Holt-Winters provider, and the response's `provenance`
//! says which one it was. When the built-in provider stepped in for a
//! configured sidecar, `fallback_reason` is one coarse code:
//! `unreachable`, `timeout`, `bad_response`, `rejected`, `busy` or
//! `circuit_open`. What exactly went wrong is in the daemon's log only.
//!
//! A failing sidecar opens a circuit breaker ([`breaker`]) so the requests
//! that follow skip it and get the fallback at once until a cool-down has
//! passed. Two codes do not: `rejected` is a request the sidecar turned
//! down with 400, 413 or 422, or one too large to send, and `busy` is a
//! sidecar with no slot to spare (429, or 503 with `Retry-After`) or a
//! daemon that already has [`SIDECAR_SLOTS`] calls under way. Both cost
//! the one request its model forecast and nothing else, with one
//! exception: a sidecar that is busy every time
//! ([`breaker::DEFAULT_BUSY_LIMIT`] answers in a row, or for longer than
//! the cool-down) opens the circuit after all. A busy answer's
//! `Retry-After` keeps that series from the sidecar for as long,
//! [`breaker::RETRY_AFTER_CAP`] at most. Every other
//! status (401, 403, 404, 408 and 5xx included) would meet every request
//! alike and opens the circuit.
//! Whoever made it, a band narrower than the series can resolve
//! is widened to [`tumult_forecast::band::floor`] and flagged
//! `degenerate_band`. Spans and counters are described in `telemetry`.

pub mod breaker;
pub mod client;
mod metric;
mod run_impact;
mod telemetry;

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
/// Calls to the sidecar under way at once, at most: its own default
/// (`DEFAULT_MAX_CONCURRENT` in `forecastd/forecastd/service.py`).
pub const SIDECAR_SLOTS: usize = 4;

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

/// Every slot is taken: say so on the request's span, and in the log at
/// most once per [`breaker::SLOT_EVENT_EVERY`]. Requests come in bursts,
/// and a line for each would bury the log exactly when it is needed.
fn note_slots_exhausted(breaker: &breaker::Breaker, parent: &telemetry::Timed) {
    parent.flag(telemetry::SLOTS_EXHAUSTED);
    let Some(suppressed) = breaker.slots_exhausted(breaker.now()) else {
        return;
    };
    tracing::warn!(
        forecast.sidecar.slots = SIDECAR_SLOTS,
        forecast.suppressed = suppressed,
        "every forecast sidecar slot is taken; using the built-in provider"
    );
}

/// Ask the sidecar unless its circuit is open; the error is the coarse
/// reason the built-in provider has to step in.
///
/// A request the sidecar would never be sent is turned down before the
/// breaker is asked: it says nothing about the sidecar and must not take
/// the half-open probe. The same goes for a request that arrives while
/// [`SIDECAR_SLOTS`] calls are under way: the sidecar would answer it 503,
/// so it gets the fallback (`busy`) without the round trip. And for a
/// request for `series` while the sidecar's last `Retry-After` for that
/// series has not passed.
async fn ask_sidecar(
    state: &ApiState,
    client: &client::TimestralClient,
    parent: &telemetry::Timed,
    series: &str,
    request: &ForecastRequest<'_>,
) -> Result<Forecast, &'static str> {
    if let Err(e) = client::check_limits(request.context.len(), request.horizon) {
        log_sidecar_failure(&e);
        return Err(e.kind.code());
    }
    let breaker = state.forecast_breaker();
    // The sidecar asked this series to wait; sending it again before then
    // would be answered the same way. Like the slot below, this comes
    // before the breaker: a request that is not sent must not take the
    // half-open probe with it.
    if breaker.held_back(series, breaker.now()) {
        return Err(client::FailureKind::Busy.code());
    }
    let Ok(_slot) = state.forecast_slots().try_acquire() else {
        note_slots_exhausted(breaker, parent);
        return Err(client::FailureKind::Busy.code());
    };
    let Some(permit) = breaker.admit(breaker.now()) else {
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
            let now = breaker.now();
            match e.kind {
                client::FailureKind::Busy => {
                    breaker.record_busy(permit, now, series, e.retry_after);
                }
                kind if kind.trips_breaker() => breaker.record_failure(permit, now),
                _ => breaker.record_rejection(permit, now),
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
    series: &str,
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
        match ask_sidecar(state, client, &span, series, &request).await {
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
    let breaker = state.forecast_breaker();
    breaker.is_open(breaker.now()).then_some(CIRCUIT_OPEN)
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
    use std::time::Instant;

    use tumult_otel::logging::testing::LogCapture;

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

    /// A sidecar state whose breaker reads a clock the test moves: the
    /// returned function puts it forward by so many seconds.
    fn state_with_clock(url: &str, cooldown: std::time::Duration) -> (ApiState, impl Fn(u64)) {
        use std::sync::atomic::{AtomicU64, Ordering};
        let start = Instant::now();
        let ahead = std::sync::Arc::new(AtomicU64::new(0));
        let read = ahead.clone();
        let clock = breaker::Clock::new(move || {
            start + std::time::Duration::from_secs(read.load(Ordering::SeqCst))
        });
        let state = state_with_sidecar(url, cooldown)
            .with_forecast_breaker(breaker::Breaker::new(cooldown).with_clock(clock));
        (state, move |seconds| {
            ahead.fetch_add(seconds, Ordering::SeqCst);
        })
    }

    async fn ask_under(
        state: &ApiState,
        span: &telemetry::Timed,
        series: &str,
    ) -> Result<Forecast, &'static str> {
        let request = ForecastRequest {
            context: &[1.0, 2.0],
            horizon: 4,
            levels: &DEFAULT_LEVELS,
            covariates: None,
        };
        let client = state.forecast_client().unwrap();
        ask_sidecar(state, client, span, series, &request).await
    }

    /// Ask for series `series` with a short context.
    async fn ask_for(state: &ApiState, series: &str) -> Result<Forecast, &'static str> {
        let span = telemetry::Timed::start(telemetry::PRODUCE);
        ask_under(state, &span, series).await
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
        ask_sidecar(state, client, &span, "s", &request).await
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
        let cooldown = std::time::Duration::from_secs(5);
        let state = state_with_sidecar(&format!("http://{addr}"), cooldown);
        let breaker = state.forecast_breaker();
        // The failure that opened the circuit is two cooldowns old.
        let failed_at = Instant::now().checked_sub(cooldown * 2).unwrap();
        breaker.record_failure(breaker.admit(failed_at).unwrap(), failed_at);

        assert_eq!(ask(&state, &[1.0, 2.0], 4).await.err(), Some("rejected"));
        let later = Instant::now();
        assert!(breaker.admit(later).is_some(), "the next request probes");
        assert!(breaker.admit(later).is_none(), "and it is the only one");
    }

    /// A stand-in sidecar: counts the calls it gets and answers each with
    /// whatever `answer` makes.
    async fn sidecar<F>(answer: F) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>)
    where
        F: Fn() -> axum::response::Response + Clone + Send + Sync + 'static,
    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let app = axum::Router::new().route(
            "/forecast",
            axum::routing::post(move || {
                seen.fetch_add(1, Ordering::SeqCst);
                let response = answer();
                async move { response }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), calls)
    }

    fn busy_answer(status: u16, retry_after: bool) -> axum::response::Response {
        use axum::response::IntoResponse;
        let status = axum::http::StatusCode::from_u16(status).unwrap();
        if !retry_after {
            return (status, "slow down").into_response();
        }
        (status, [(axum::http::header::RETRY_AFTER, "1")], "busy").into_response()
    }

    /// The sidecar's own "every slot is taken" (503 with `Retry-After`)
    /// and a throttle (429) cost the one request its model forecast. The
    /// circuit stays closed: the next request is sent to the sidecar too.
    #[tokio::test]
    async fn a_busy_sidecar_is_a_fallback_for_one_request_and_keeps_the_circuit_closed() {
        use std::sync::atomic::Ordering;
        let cooldown = std::time::Duration::from_secs(600);
        for (status, retry_after) in [(503, true), (429, false)] {
            let (url, calls) = sidecar(move || busy_answer(status, retry_after)).await;
            let state = state_with_sidecar(&url, cooldown);
            assert_eq!(ask_for(&state, "a").await.err(), Some("busy"));
            assert!(
                !state.forecast_breaker().is_open(Instant::now()),
                "{status}"
            );
            assert_eq!(ask_for(&state, "b").await.err(), Some("busy"));
            assert_eq!(calls.load(Ordering::SeqCst), 2, "{status}: both were sent");
        }
    }

    /// Busy once is a full moment; busy every time is a sidecar that does
    /// no work. The eighth busy answer in a row opens the circuit, and
    /// the requests after it are not sent.
    #[tokio::test]
    async fn a_sidecar_that_is_busy_every_time_opens_the_circuit() {
        use std::sync::atomic::Ordering;
        let (url, calls) = sidecar(|| busy_answer(429, false)).await;
        let (state, advance) = state_with_clock(&url, std::time::Duration::from_secs(600));
        for nth in 1..=breaker::DEFAULT_BUSY_LIMIT {
            assert_eq!(ask_for(&state, "s").await.err(), Some("busy"), "{nth}");
        }
        assert_eq!(ask_for(&state, "s").await.err(), Some(CIRCUIT_OPEN));
        assert_eq!(ask_for(&state, "other").await.err(), Some(CIRCUIT_OPEN));
        assert_eq!(calls.load(Ordering::SeqCst), 8, "the ninth was not sent");
        advance(600);
        assert_eq!(ask_for(&state, "s").await.err(), Some("busy"), "the probe");
        assert_eq!(calls.load(Ordering::SeqCst), 9);
    }

    /// Few requests never reach eight in a row: busy answers that span
    /// more than the cool-down open the circuit as well.
    #[tokio::test]
    async fn a_sidecar_busy_for_longer_than_the_cooldown_opens_the_circuit() {
        use std::sync::atomic::Ordering;
        let (url, calls) = sidecar(|| busy_answer(429, false)).await;
        let (state, advance) = state_with_clock(&url, std::time::Duration::from_secs(30));
        assert_eq!(ask_for(&state, "s").await.err(), Some("busy"));
        advance(31);
        assert_eq!(ask_for(&state, "s").await.err(), Some("busy"));
        assert_eq!(ask_for(&state, "s").await.err(), Some(CIRCUIT_OPEN));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// The sidecar said when to come back. Until then the same series is
    /// answered by the fallback without a call; another series is sent.
    #[tokio::test]
    async fn retry_after_keeps_that_series_from_the_sidecar_until_it_has_passed() {
        use std::sync::atomic::Ordering;
        let (url, calls) = sidecar(|| busy_answer(503, true)).await;
        let (state, advance) = state_with_clock(&url, std::time::Duration::from_secs(600));
        let sent = || calls.load(Ordering::SeqCst);
        assert_eq!(ask_for(&state, "a").await.err(), Some("busy"));
        assert_eq!(sent(), 1);
        assert_eq!(ask_for(&state, "a").await.err(), Some("busy"));
        assert_eq!(sent(), 1, "a was asked to wait a second and was sent");
        assert_eq!(ask_for(&state, "b").await.err(), Some("busy"));
        assert_eq!(sent(), 2, "b was not asked to wait");
        advance(1);
        assert_eq!(ask_for(&state, "a").await.err(), Some("busy"));
        assert_eq!(sent(), 3, "the second has passed");
        let breaker = state.forecast_breaker();
        assert!(!breaker.is_open(breaker.now()), "three busy answers");
    }

    /// A series that is waiting out its hint does not take the half-open
    /// probe with it.
    #[tokio::test]
    async fn a_series_that_is_held_back_leaves_the_probe_alone() {
        use std::sync::atomic::Ordering;
        let (url, calls) = sidecar(|| busy_answer(503, true)).await;
        let (state, advance) = state_with_clock(&url, std::time::Duration::from_secs(600));
        assert_eq!(ask_for(&state, "a").await.err(), Some("busy"));
        let breaker = state.forecast_breaker();
        let failed = breaker.now();
        breaker.record_failure(breaker.admit(failed).unwrap(), failed);
        advance(600);
        // The hint is long past; put a fresh one on `a` through the probe.
        assert_eq!(ask_for(&state, "a").await.err(), Some("busy"));
        assert_eq!(ask_for(&state, "a").await.err(), Some("busy"));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one before, one probe");
        let now = breaker.now();
        assert!(breaker.admit(now).is_some(), "the probe is still to be had");
    }

    /// The slot events among `logs`: how many requests each stands for.
    fn suppressed_by_slot_events(logs: &LogCapture) -> Vec<serde_json::Value> {
        let slots = serde_json::json!(SIDECAR_SLOTS);
        let events = logs.snapshot().events;
        let about_slots = events
            .iter()
            .filter(|event| event["forecast.sidecar.slots"] == slots);
        about_slots
            .map(|event| event["forecast.suppressed"].clone())
            .collect()
    }

    /// A request that finds every slot taken says so on its span, and in
    /// the log at most once per interval: a burst is one event that
    /// counts the requests it stands for, not one line each.
    #[tokio::test]
    async fn a_request_that_finds_no_slot_is_flagged_and_logged_once_per_interval() {
        use opentelemetry::trace::TracerProvider;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        let (state, advance) =
            state_with_clock("http://127.0.0.1:1", std::time::Duration::from_secs(600));
        let slots = u32::try_from(SIDECAR_SLOTS).unwrap();
        let _taken = state.forecast_slots().acquire_many(slots).await.unwrap();
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("test");
        let logs = LogCapture::start(tracing::Level::INFO);

        let span =
            telemetry::Timed::start_in(&tracer, telemetry::PRODUCE, &opentelemetry::Context::new());
        for _ in 0..3 {
            assert_eq!(ask_under(&state, &span, "s").await.err(), Some("busy"));
        }
        span.finish("fallback");
        assert_eq!(suppressed_by_slot_events(&logs), [0]);
        advance(breaker::SLOT_EVENT_EVERY.as_secs());
        assert_eq!(ask_for(&state, "s").await.err(), Some("busy"));
        assert_eq!(suppressed_by_slot_events(&logs), [0, 2]);

        let spans = exporter.get_finished_spans().unwrap();
        let flagged = spans[0]
            .attributes
            .iter()
            .any(|kv| kv.key.as_str() == telemetry::SLOTS_EXHAUSTED && kv.value.as_str() == "true");
        assert!(flagged, "{:?}", spans[0].attributes);
    }

    /// A request that got a slot carries no such flag.
    #[tokio::test]
    async fn a_request_that_got_a_slot_is_not_flagged() {
        use opentelemetry::trace::TracerProvider;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        let (url, _calls) = sidecar(|| busy_answer(429, false)).await;
        let state = state_with_sidecar(&url, std::time::Duration::from_secs(600));
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let span = telemetry::Timed::start_in(
            &provider.tracer("test"),
            telemetry::PRODUCE,
            &opentelemetry::Context::new(),
        );
        assert_eq!(ask_under(&state, &span, "s").await.err(), Some("busy"));
        span.finish("fallback");
        let spans = exporter.get_finished_spans().unwrap();
        let produce = spans.iter().find(|s| s.name == telemetry::PRODUCE).unwrap();
        let flagged = produce
            .attributes
            .iter()
            .any(|kv| kv.key.as_str() == telemetry::SLOTS_EXHAUSTED);
        assert!(!flagged, "{:?}", produce.attributes);
    }

    /// A 503 that does not say when to come back is an outage.
    #[tokio::test]
    async fn a_503_without_retry_after_opens_the_circuit() {
        use std::sync::atomic::Ordering;
        let (url, calls) = sidecar(|| busy_answer(503, false)).await;
        let state = state_with_sidecar(&url, std::time::Duration::from_secs(600));
        assert_eq!(
            ask(&state, &[1.0, 2.0], 4).await.err(),
            Some("bad_response")
        );
        assert_eq!(ask(&state, &[1.0, 2.0], 4).await.err(), Some(CIRCUIT_OPEN));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// A busy answer to the half-open probe proves the sidecar is alive
    /// but not that it can forecast: the next request probes again.
    #[tokio::test]
    async fn a_busy_probe_hands_the_probe_on() {
        let (url, _calls) = sidecar(|| busy_answer(503, true)).await;
        let (state, advance) = state_with_clock(&url, std::time::Duration::from_secs(600));
        let breaker = state.forecast_breaker();
        let opened = breaker.now();
        breaker.record_failure(breaker.admit(opened).unwrap(), opened);
        advance(601);
        assert_eq!(ask(&state, &[1.0, 2.0], 4).await.err(), Some("busy"));
        let later = breaker.now();
        assert!(breaker.admit(later).is_some(), "the next request probes");
        assert!(breaker.admit(later).is_none(), "and it is the only one");
    }

    const GOOD: &str = r#"{"levels":[0.1,0.5,0.9],"context_points":12,
        "quantiles":[[[1.0,2.0,3.0]]],
        "model":{"model":"timestral","model_id":"m","version":"1","weights_hash":"abc"}}"#;

    /// The daemon never has more calls under way than the sidecar has
    /// slots: one more gets the fallback at once, without a network call
    /// that could only be answered 503, and a finished call frees its slot.
    #[tokio::test]
    async fn calls_beyond_the_sidecar_slots_fall_back_without_a_call() {
        use axum::response::IntoResponse;
        use std::sync::atomic::Ordering;
        let (arrived_tx, mut arrived) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (release, _) = tokio::sync::broadcast::channel::<()>(1);
        let held = release.clone();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let app = axum::Router::new().route(
            "/forecast",
            axum::routing::post(move || {
                let mut released = held.subscribe();
                seen.fetch_add(1, Ordering::SeqCst);
                let arrived = arrived_tx.clone();
                async move {
                    arrived.send(()).unwrap();
                    released.recv().await.unwrap();
                    GOOD.into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = state_with_sidecar(
            &format!("http://{addr}"),
            std::time::Duration::from_secs(600),
        );

        let mut under_way = tokio::task::JoinSet::new();
        for _ in 0..SIDECAR_SLOTS {
            let state = state.clone();
            under_way.spawn(async move { ask(&state, &[1.0, 2.0], 1).await.map(|_| ()) });
        }
        for _ in 0..SIDECAR_SLOTS {
            arrived.recv().await.unwrap();
        }
        // Without the slots the fifth call is sent and held like the other
        // four. The clock is paused so that the bound costs no time: it
        // runs out only if the call is waiting on the sidecar.
        tokio::time::pause();
        let bound = std::time::Duration::from_secs(1);
        let fifth = tokio::time::timeout(bound, ask(&state, &[1.0, 2.0], 1)).await;
        tokio::time::resume();
        let fifth = fifth.expect("the fifth call was sent and waits on the sidecar");
        assert_eq!(fifth.err(), Some("busy"));
        assert_eq!(calls.load(Ordering::SeqCst), SIDECAR_SLOTS, "not sent");
        assert!(!state.forecast_breaker().is_open(Instant::now()));

        release.send(()).unwrap();
        while let Some(done) = under_way.join_next().await {
            assert_eq!(done.unwrap(), Ok(()));
        }
        let (again, sent) = tokio::join!(ask(&state, &[1.0, 2.0], 1), async {
            arrived.recv().await.unwrap();
            release.send(()).unwrap();
        });
        let () = sent;
        assert!(again.is_ok(), "a finished call gives its slot back");
        assert_eq!(calls.load(Ordering::SeqCst), SIDECAR_SLOTS + 1);
    }

    /// More daemon-side slots than the sidecar has would only produce 503s.
    #[test]
    fn the_daemon_has_no_more_slots_than_the_sidecar() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../forecastd/forecastd/service.py"
        );
        let source = std::fs::read_to_string(path).unwrap();
        let line = source
            .lines()
            .find(|l| l.starts_with("DEFAULT_MAX_CONCURRENT = "))
            .expect("the sidecar's default slot count");
        let sidecar: usize = line.split('=').nth(1).unwrap().trim().parse().unwrap();
        assert!((1..=sidecar).contains(&SIDECAR_SLOTS), "{SIDECAR_SLOTS}");
    }

    /// Every `tracing` event in `source` as its level and its text, from
    /// the macro to its `);`.
    fn tracing_events(source: &str) -> Vec<(&str, &str)> {
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
                rest.find(");").map(|end| (name, &rest[..end]))
            })
            .collect()
    }

    /// What is wrong with one log event, if anything. An event is about
    /// an error when it is logged at that level or names either error
    /// field, and then it carries both; any other event needs neither.
    fn event_fault(level: &str, event: &str) -> Option<&'static str> {
        if event.contains(" error = ") {
            return Some("a bare error field");
        }
        let message = event.contains("error.message = ");
        let kind = event.contains("error.type = ");
        let about_an_error = level == "error" || message || kind;
        if about_an_error && !message {
            return Some("no error.message");
        }
        if about_an_error && !kind {
            return Some("no error.type");
        }
        let text = event.rsplit('"').nth(1).unwrap_or_default();
        text.contains('{').then_some("an interpolated message")
    }

    #[test]
    fn an_event_without_an_error_needs_no_error_fields() {
        let plain = "info!(\n    forecast.model = model,\n    \"forecast served\"\n";
        assert_eq!(event_fault("info", plain), None);
        assert_eq!(event_fault("debug", "debug!(\"circuit closed\""), None);
        assert_eq!(
            event_fault("warn", "warn!(slots = 4, \"sidecar busy\""),
            None
        );
    }

    #[test]
    fn an_event_about_an_error_names_its_type_and_message() {
        let whole = "warn!(error.type = \"x\", error.message = %e, \"lookup failed\"";
        assert_eq!(event_fault("warn", whole), None);
        let untyped = "warn!(error.message = %e, \"lookup failed\"";
        assert_eq!(event_fault("warn", untyped), Some("no error.type"));
        let silent = "warn!(error.type = \"x\", \"lookup failed\"";
        assert_eq!(event_fault("warn", silent), Some("no error.message"));
        // The error level is about an error whatever its fields say.
        let bare = "error!(run = %id, \"forecast failed\"";
        assert_eq!(event_fault("error", bare), Some("no error.message"));
        let loose = "warn!(run = %id, error = %e, \"lookup failed\"";
        assert_eq!(event_fault("warn", loose), Some("a bare error field"));
        let inline = "info!(\"served {model}\"";
        assert_eq!(event_fault("info", inline), Some("an interpolated message"));
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
            for (level, event) in tracing_events(&source) {
                let forecast_event = path.contains("/forecast/") || event.contains("FORECAST");
                if !forecast_event {
                    continue;
                }
                seen += 1;
                assert_eq!(event_fault(level, event), None, "{path}: {event}");
            }
        }
        assert!(seen >= 4, "only {seen} forecast events were found");
    }
}
