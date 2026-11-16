//! HTTP client for the `tumult-forecastd` sidecar (`POST /forecast`), the
//! Timestral provider behind `TUMULTD_FORECAST_URL`. Asynchronous because
//! it runs inside the API's tokio runtime; the pure provider trait stays
//! synchronous and the handlers bridge the two through `forecast::produce`.
//!
//! The sidecar is another process and its answer is not trusted: the
//! client follows no redirects and uses no proxy, reads at most
//! [`MAX_RESPONSE_BYTES`], and accepts an answer only when its levels are
//! the ones asked for, its shape is one variate of `horizon` steps and
//! every value is finite. Anything else is a [`SidecarError`] and the
//! built-in provider answers instead.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tumult_forecast::{Covariates, Forecast, ForecastError, Provenance, MIN_HISTORY};

const TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Longest horizon the sidecar accepts (`MAX_HORIZON` in
/// `forecastd/forecastd/service.py`). A longer request is turned down
/// here, without a network call.
pub const MAX_HORIZON: usize = 4_096;
/// Longest context the sidecar accepts (`MAX_CONTEXT` in
/// `forecastd/forecastd/service.py`).
pub const MAX_CONTEXT: usize = 32_768;
/// Largest response body read: a full-horizon answer at every level the
/// model has is under two mebibytes.
pub const MAX_RESPONSE_BYTES: usize = 8 << 20;

/// What kind of failure kept the sidecar from answering. The code is all
/// a client of the API is told; the detail stays in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// No connection could be made.
    Unreachable,
    /// The sidecar did not answer in time, or said so itself (408).
    Timeout,
    /// A status that is not about this one request (5xx, and 401, 403,
    /// 404 or any other 4xx: a wrong URL, a proxy), a redirect, an
    /// oversized body or an answer that is not the forecast that was asked
    /// for.
    BadResponse,
    /// The sidecar is alive and turned this one request down (400, 413 or
    /// 422), or the request exceeds what it accepts.
    Rejected,
    /// The sidecar is alive and has no slot for this request right now
    /// (429, or 503 with `Retry-After`), or the daemon already has as many
    /// calls under way as the sidecar takes.
    Busy,
}

/// What a non-2xx status says. Only the statuses the sidecar gives a
/// request it will not take are a rejection, and only a throttle (429) or
/// a 503 that says when to come back (`retry_after`, which is how the
/// sidecar answers when every slot is taken) is a sidecar that is merely
/// busy. Anything else would be answered to every request alike, so it
/// counts against the sidecar.
fn status_kind(status: reqwest::StatusCode, retry_after: bool) -> FailureKind {
    match (status.as_u16(), retry_after) {
        (400 | 413 | 422, _) => FailureKind::Rejected,
        (408, _) => FailureKind::Timeout,
        (429, _) | (503, true) => FailureKind::Busy,
        _ => FailureKind::BadResponse,
    }
}

/// `Retry-After` as a number of whole seconds. The date form is not
/// read: the sidecar sends seconds, and a date would need the two clocks
/// to agree.
fn retry_after_seconds(value: &reqwest::header::HeaderValue) -> Option<Duration> {
    let seconds = value.to_str().ok()?.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds))
}

impl FailureKind {
    /// The coarse code reported as `fallback_reason`.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::BadResponse => "bad_response",
            Self::Rejected => "rejected",
            Self::Busy => "busy",
        }
    }

    /// Whether the sidecar itself is at fault and should be skipped for a
    /// while. A rejected request says nothing against the sidecar, and a
    /// busy one is working: skipping it for the cool-down would turn one
    /// full moment into thirty seconds without the model.
    #[must_use]
    pub fn trips_breaker(self) -> bool {
        !matches!(self, Self::Rejected | Self::Busy)
    }
}

/// Why the sidecar did not produce a forecast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarError {
    pub kind: FailureKind,
    /// What went wrong, for the log only.
    pub detail: String,
    /// How long a busy sidecar asked to be left alone: `Retry-After` in
    /// whole seconds. `None` when it gave no such number.
    pub retry_after: Option<Duration>,
}

impl SidecarError {
    fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            retry_after: None,
        }
    }

    fn transport(context: &str, error: &reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            FailureKind::Timeout
        } else if error.is_connect() || error.is_request() {
            FailureKind::Unreachable
        } else {
            FailureKind::BadResponse
        };
        Self::new(kind, format!("{context}: {error}"))
    }
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.code(), self.detail)
    }
}

#[derive(Serialize)]
struct Request<'a> {
    context: Vec<Option<f64>>,
    horizon: usize,
    quantile_levels: &'a [f64],
    #[serde(skip_serializing_if = "Option::is_none")]
    past_covariates: Option<Vec<Vec<Option<f64>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    known_covariates: Option<Vec<Vec<Option<f64>>>>,
}

#[derive(Deserialize)]
struct ModelInfo {
    model: String,
    model_id: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    weights_hash: String,
}

/// `model_id@revision (model version)` when the sidecar names a weights
/// revision, `model_id@version` otherwise.
fn version_label(info: &ModelInfo) -> String {
    if info.revision.is_empty() {
        return format!("{}@{}", info.model_id, info.version);
    }
    format!(
        "{}@{} ({} {})",
        info.model_id, info.revision, info.model, info.version
    )
}

#[derive(Deserialize)]
struct Response {
    levels: Vec<f64>,
    quantiles: Vec<Vec<Vec<Option<f64>>>>,
    #[serde(default)]
    context_points: usize,
    model: ModelInfo,
}

fn nullable(values: &[f64]) -> Vec<Option<f64>> {
    values.iter().map(|v| v.is_finite().then_some(*v)).collect()
}

fn nullable_rows(rows: &[Vec<f64>]) -> Option<Vec<Vec<Option<f64>>>> {
    if rows.is_empty() {
        return None;
    }
    Some(rows.iter().map(|r| nullable(r)).collect())
}

fn provider(message: impl Into<String>) -> ForecastError {
    ForecastError::Provider(message.into())
}

/// Levels this close are the same level: a model may hold its grid in
/// single precision.
const LEVEL_TOLERANCE: f64 = 1e-6;

fn same_levels(answered: &[f64], requested: &[f64]) -> bool {
    answered.len() == requested.len()
        && answered
            .iter()
            .zip(requested)
            .all(|(a, r)| (a - r).abs() < LEVEL_TOLERANCE)
}

/// One step's quantiles: as many as there are levels, every one finite.
fn step_values(step: Vec<Option<f64>>, width: usize) -> Result<Vec<f64>, ForecastError> {
    if step.len() != width {
        return Err(provider("response quantiles do not match its levels"));
    }
    step.into_iter()
        .map(|v| {
            v.filter(|x| x.is_finite())
                .ok_or_else(|| provider("response holds a missing or non-finite quantile"))
        })
        .collect()
}

/// Turn the sidecar's answer into a [`Forecast`] for the one variate that
/// was asked for.
///
/// # Errors
/// [`ForecastError::Provider`] when the body is not the forecast that was
/// requested: unreadable, other levels, not exactly one variate of
/// `horizon` steps, a step without one value per level, or a value that
/// is missing or not finite.
pub(crate) fn parse_response(
    body: &str,
    horizon: usize,
    levels: &[f64],
) -> Result<Forecast, ForecastError> {
    let resp: Response =
        serde_json::from_str(body).map_err(|e| provider(format!("bad response: {e}")))?;
    if !same_levels(&resp.levels, levels) {
        return Err(provider(format!(
            "response levels {:?} are not the requested {levels:?}",
            resp.levels
        )));
    }
    if resp.quantiles.len() != 1 {
        return Err(provider(format!(
            "response has {} variates, requested 1",
            resp.quantiles.len()
        )));
    }
    let steps = resp.quantiles.into_iter().next().unwrap_or_default();
    if steps.len() != horizon {
        return Err(provider(format!(
            "response covers {} steps, requested {horizon}",
            steps.len()
        )));
    }
    let quantiles = steps
        .into_iter()
        .map(|step| step_values(step, levels.len()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Forecast {
        levels: levels.to_vec(),
        quantiles,
        context_points: resp.context_points,
        insufficient_history: resp.context_points < MIN_HISTORY,
        degenerate_band: false,
        provenance: Provenance {
            version: version_label(&resp.model),
            model: resp.model.model,
            weights_hash: resp.model.weights_hash,
        },
    })
}

/// `base` → the `POST /forecast` URL, when `base` is an http(s) URL.
fn endpoint(base: &str) -> Result<String, String> {
    let trimmed = base.trim();
    let url = reqwest::Url::parse(trimmed).map_err(|e| format!("not a URL ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "scheme {:?} is not allowed, use http or https",
            url.scheme()
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the URL names no host".into());
    }
    Ok(format!("{}/forecast", trimmed.trim_end_matches('/')))
}

/// The response body, read in chunks and refused past `cap` bytes.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Result<String, SidecarError> {
    let too_large = || SidecarError::new(FailureKind::BadResponse, "response body too large");
    let announced = resp.content_length().unwrap_or(0);
    if usize::try_from(announced).map_or(true, |n| n > cap) {
        return Err(too_large());
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| SidecarError::transport("sidecar body unreadable", &e))?
    {
        if body.len() + chunk.len() > cap {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body)
        .map_err(|_| SidecarError::new(FailureKind::BadResponse, "response body is not UTF-8"))
}

/// Whether a request of this size is one the sidecar accepts at all.
///
/// # Errors
/// A [`FailureKind::Rejected`] error naming the limits when it is not.
pub fn check_limits(context_points: usize, horizon: usize) -> Result<(), SidecarError> {
    if horizon <= MAX_HORIZON && context_points <= MAX_CONTEXT {
        return Ok(());
    }
    Err(SidecarError::new(
        FailureKind::Rejected,
        format!(
            "{context_points} context points and a horizon of {horizon} exceed the sidecar's \
             {MAX_CONTEXT} and {MAX_HORIZON}"
        ),
    ))
}

/// The sidecar client. Built once at start-up and shared.
#[derive(Debug, Clone)]
pub struct TimestralClient {
    base: String,
    endpoint: String,
    http: reqwest::Client,
}

impl TimestralClient {
    /// A client for the sidecar at `base` (e.g. `http://forecastd:8700`).
    ///
    /// # Errors
    /// A message naming the problem when `base` is not an `http` or
    /// `https` URL with a host, or the HTTP client cannot be built.
    pub fn new(base: &str) -> Result<Self, String> {
        Self::with_timeout(base, TIMEOUT)
    }

    fn with_timeout(base: &str, timeout: Duration) -> Result<Self, String> {
        let endpoint = endpoint(base)?;
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(CONNECT_TIMEOUT.min(timeout))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| format!("cannot build the HTTP client: {e}"))?;
        Ok(Self {
            base: base.trim().trim_end_matches('/').to_string(),
            endpoint,
            http,
        })
    }

    /// The base URL this client was built for.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// `POST /forecast` for one variate.
    ///
    /// # Errors
    /// [`SidecarError`] when the request exceeds what the sidecar accepts,
    /// the sidecar is unreachable or slow, answers a non-2xx status, or
    /// answers something other than the forecast that was asked for.
    pub async fn forecast(
        &self,
        context: &[f64],
        horizon: usize,
        levels: &[f64],
        covariates: Option<&Covariates>,
    ) -> Result<Forecast, SidecarError> {
        check_limits(context.len(), horizon)?;
        let request = Request {
            context: nullable(context),
            horizon,
            quantile_levels: levels,
            past_covariates: covariates.and_then(|c| nullable_rows(&c.past)),
            known_covariates: covariates.and_then(|c| nullable_rows(&c.known_future)),
        };
        let resp = self
            .http
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|e| SidecarError::transport("sidecar request failed", &e))?;
        let status = resp.status();
        let retry_after = resp.headers().get(reqwest::header::RETRY_AFTER).cloned();
        let body = read_capped(resp, MAX_RESPONSE_BYTES).await?;
        if !status.is_success() {
            let excerpt: String = body.chars().take(200).collect();
            let kind = status_kind(status, retry_after.is_some());
            let mut error =
                SidecarError::new(kind, format!("sidecar answered {status}: {excerpt}"));
            if kind == FailureKind::Busy {
                error.retry_after = retry_after.as_ref().and_then(retry_after_seconds);
            }
            return Err(error);
        }
        parse_response(&body, horizon, levels)
            .map_err(|e| SidecarError::new(FailureKind::BadResponse, e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;

    const GOOD: &str = r#"{"levels":[0.5],"context_points":12,"quantiles":[[[1.0]]],
        "model":{"model":"timestral","model_id":"mistralai/Timestral-5M",
        "version":"1.0.0","revision":"0123abcd","weights_hash":"abc"}}"#;

    #[test]
    fn response_parses_into_a_forecast_with_provenance() {
        let body = r#"{"levels":[0.1,0.5,0.9],"requested_levels":[0.1,0.5,0.9],"horizon":2,
            "context_points":12,"quantiles":[[[1.0,2.0,3.0],[1.5,2.5,3.5]]],
            "model":{"model":"timestral","model_id":"mistralai/Timestral-5M","size":"5M",
            "version":"0.1","weights_hash":"abc"}}"#;
        let f = parse_response(body, 2, &[0.1, 0.5, 0.9]).unwrap();
        assert_eq!(f.levels, vec![0.1, 0.5, 0.9]);
        assert_eq!(f.quantiles[0], vec![1.0, 2.0, 3.0]);
        assert_eq!(f.quantiles[1], vec![1.5, 2.5, 3.5]);
        assert_eq!(f.context_points, 12);
        assert!(!f.insufficient_history);
        assert!(!f.degenerate_band);
        assert_eq!(f.provenance.model, "timestral");
        assert_eq!(f.provenance.version, "mistralai/Timestral-5M@0.1");
        assert_eq!(f.provenance.weights_hash, "abc");
    }

    #[test]
    fn a_weights_revision_is_part_of_the_version() {
        let f = parse_response(GOOD, 1, &[0.5]).unwrap();
        assert_eq!(
            f.provenance.version,
            "mistralai/Timestral-5M@0123abcd (timestral 1.0.0)"
        );
    }

    fn body_with(levels: &str, quantiles: &str) -> String {
        format!(
            r#"{{"levels":{levels},"quantiles":{quantiles},"model":{{"model":"t","model_id":"x"}}}}"#
        )
    }

    #[test]
    fn bad_responses_are_provider_errors() {
        let requested = [0.1, 0.5, 0.9];
        let cases = [
            ("not json", "not json".to_string()),
            ("no variate", body_with("[0.1,0.5,0.9]", "[]")),
            (
                "two variates",
                body_with("[0.1,0.5,0.9]", "[[[1,2,3]],[[1,2,3]]]"),
            ),
            (
                "wrong horizon",
                body_with("[0.1,0.5,0.9]", "[[[1,2,3],[1,2,3]]]"),
            ),
            ("ragged step", body_with("[0.1,0.5,0.9]", "[[[1,2]]]")),
            ("a gap", body_with("[0.1,0.5,0.9]", "[[[1,null,3]]]")),
            (
                "an overflowing number",
                body_with("[0.1,0.5,0.9]", "[[[1,1e999,3]]]"),
            ),
        ];
        for (what, body) in cases {
            assert!(
                matches!(
                    parse_response(&body, 1, &requested),
                    Err(ForecastError::Provider(_))
                ),
                "{what}"
            );
        }
    }

    /// The model snapped the levels to its own grid, answered fewer, or
    /// answered them in another order: none of that is the band that was
    /// asked for.
    #[test]
    fn levels_other_than_the_requested_ones_are_refused() {
        let requested = [0.1, 0.5, 0.9];
        for levels in [
            "[0.2,0.5,0.8]",
            "[0.5]",
            "[0.9,0.5,0.1]",
            "[0.1,0.5,0.9,0.99]",
        ] {
            let width = levels.matches(',').count() + 1;
            let step = vec!["1"; width].join(",");
            let body = body_with(levels, &format!("[[[{step}]]]"));
            let err = parse_response(&body, 1, &requested).unwrap_err();
            assert!(err.to_string().contains("levels"), "{levels}: {err}");
        }
    }

    /// A grid held in single precision is still the grid that was asked
    /// for, and the forecast carries the requested levels exactly.
    #[test]
    fn single_precision_levels_match_and_are_normalised() {
        let body = body_with(
            "[0.10000000149011612,0.5,0.8999999761581421]",
            "[[[1,2,3]]]",
        );
        let f = parse_response(&body, 1, &[0.1, 0.5, 0.9]).unwrap();
        assert_eq!(f.levels, vec![0.1, 0.5, 0.9]);
        assert!(f.band(0.1, 0.9).is_some());
    }

    #[test]
    fn only_http_and_https_urls_are_accepted() {
        for good in [
            "http://forecastd:8700",
            " https://forecast.example/prefix/ ",
        ] {
            assert!(TimestralClient::new(good).is_ok(), "{good}");
        }
        let client = TimestralClient::new("http://forecastd:8700/").unwrap();
        assert_eq!(client.base(), "http://forecastd:8700");
        assert_eq!(client.endpoint, "http://forecastd:8700/forecast");
        for bad in [
            "",
            "forecastd:8700",
            "file:///etc/passwd",
            "ftp://forecastd/",
            "unix:/run/forecastd.sock",
            "http://",
            "not a url",
        ] {
            assert!(TimestralClient::new(bad).is_err(), "{bad:?}");
        }
    }

    async fn serve(app: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    async fn ask(client: &TimestralClient) -> Result<Forecast, SidecarError> {
        client.forecast(&[1.0, 2.0], 1, &[0.5], None).await
    }

    #[tokio::test]
    async fn an_unreachable_sidecar_is_unreachable() {
        let client = TimestralClient::new("http://127.0.0.1:1/").unwrap();
        let err = ask(&client).await.unwrap_err();
        assert_eq!(err.kind, FailureKind::Unreachable, "{err}");
        assert!(err.kind.trips_breaker());
        assert_eq!(err.kind.code(), "unreachable");
    }

    #[tokio::test]
    async fn a_good_answer_is_a_forecast() {
        let base = serve(axum::Router::new().route("/forecast", post(|| async { GOOD }))).await;
        let f = ask(&TimestralClient::new(&base).unwrap()).await.unwrap();
        assert_eq!(f.quantiles, vec![vec![1.0]]);
    }

    async fn answer_with(status: u16) -> SidecarError {
        answer(status, None).await
    }

    async fn answer(status: u16, retry_after: Option<&'static str>) -> SidecarError {
        let status = StatusCode::from_u16(status).unwrap();
        let app = axum::Router::new().route(
            "/forecast",
            post(move || async move {
                let mut response = (status, "context too short").into_response();
                if let Some(seconds) = retry_after {
                    let value = header::HeaderValue::from_static(seconds);
                    response.headers_mut().insert(header::RETRY_AFTER, value);
                }
                response
            }),
        );
        let base = serve(app).await;
        ask(&TimestralClient::new(&base).unwrap())
            .await
            .unwrap_err()
    }

    /// A sidecar that says "not now" is working: every slot is taken, or
    /// something in front of it is throttling. That is this request's
    /// problem, not a reason to skip the sidecar for the cool-down.
    #[tokio::test]
    async fn a_full_or_throttling_sidecar_is_busy_and_does_not_trip() {
        for (status, retry_after) in [(503, Some("1")), (429, None), (429, Some("5"))] {
            let err = answer(status, retry_after).await;
            assert_eq!(err.kind, FailureKind::Busy, "{status} {retry_after:?}");
            assert!(!err.kind.trips_breaker(), "{status}");
            assert_eq!(err.kind.code(), "busy");
        }
    }

    /// A busy answer carries the sidecar's hint in whole seconds. A date,
    /// or anything else that is not a number of seconds, still makes a
    /// 503 a busy one but is no hint.
    #[tokio::test]
    async fn a_busy_answer_carries_the_retry_after_seconds() {
        let secs = |n| Some(Duration::from_secs(n));
        assert_eq!(answer(503, Some("7")).await.retry_after, secs(7));
        assert_eq!(answer(429, Some(" 120 ")).await.retry_after, secs(120));
        assert_eq!(answer(429, None).await.retry_after, None);
        let dated = answer(503, Some("Wed, 21 Oct 2026 07:28:00 GMT")).await;
        assert_eq!(dated.kind, FailureKind::Busy);
        assert_eq!(dated.retry_after, None);
        assert_eq!(answer(503, Some("-3")).await.retry_after, None);
    }

    /// Only a busy answer has a hint: a 500 that names one is an outage.
    #[tokio::test]
    async fn a_failure_that_is_not_busy_has_no_hint() {
        let err = answer(500, Some("7")).await;
        assert_eq!(err.kind, FailureKind::BadResponse);
        assert_eq!(err.retry_after, None);
    }

    /// Without `Retry-After` a 503 is an outage, not a full house.
    #[tokio::test]
    async fn a_503_without_retry_after_is_the_sidecars_fault() {
        let err = answer(503, None).await;
        assert_eq!(err.kind, FailureKind::BadResponse);
        assert!(err.kind.trips_breaker());
    }

    /// Only a status that is about this one request is a rejection.
    #[tokio::test]
    async fn a_request_the_sidecar_turns_down_is_rejected() {
        for status in [400, 413, 422] {
            let err = answer_with(status).await;
            assert_eq!(err.kind, FailureKind::Rejected, "{status}");
            assert!(!err.kind.trips_breaker(), "{status}");
            assert!(err.detail.contains("context too short"), "{err}");
        }
    }

    /// A wrong URL (404), a sidecar behind something that wants
    /// credentials (401, 403) or one that is failing (5xx) would answer
    /// every request the same way: all of them trip.
    #[tokio::test]
    async fn every_other_status_is_the_sidecars_fault() {
        for status in [401, 403, 404, 405, 500, 502, 503, 504] {
            let err = answer_with(status).await;
            assert_eq!(err.kind, FailureKind::BadResponse, "{status}");
            assert!(err.kind.trips_breaker(), "{status}");
        }
        let slow = answer_with(408).await;
        assert_eq!(slow.kind, FailureKind::Timeout);
        assert!(slow.kind.trips_breaker());
    }

    /// A redirect could point the daemon at any address it can reach.
    #[tokio::test]
    async fn a_redirect_is_not_followed() {
        let app = axum::Router::new()
            .route(
                "/forecast",
                post(|| async {
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(header::LOCATION, "/elsewhere")],
                    )
                        .into_response()
                }),
            )
            .route("/elsewhere", post(|| async { GOOD }));
        let base = serve(app).await;
        let err = ask(&TimestralClient::new(&base).unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.kind, FailureKind::BadResponse, "{err}");
        assert!(err.detail.contains("307"), "{err}");
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused() {
        let app = axum::Router::new().route(
            "/forecast",
            post(|| async { "x".repeat(MAX_RESPONSE_BYTES + 1) }),
        );
        let base = serve(app).await;
        let err = ask(&TimestralClient::new(&base).unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.kind, FailureKind::BadResponse);
        assert!(err.detail.contains("too large"), "{err}");
    }

    #[tokio::test]
    async fn a_sidecar_that_never_answers_times_out() {
        let app =
            axum::Router::new().route("/forecast", post(std::future::pending::<&'static str>));
        let base = serve(app).await;
        let client = TimestralClient::with_timeout(&base, Duration::from_millis(300)).unwrap();
        let err = ask(&client).await.unwrap_err();
        assert_eq!(err.kind, FailureKind::Timeout, "{err}");
        assert_eq!(err.kind.code(), "timeout");
    }

    /// Nothing listens on port 1: a request that is turned down locally
    /// never gets as far as finding that out.
    #[tokio::test]
    async fn a_request_beyond_the_sidecar_limits_is_rejected_without_a_call() {
        let client = TimestralClient::new("http://127.0.0.1:1").unwrap();
        let long = client
            .forecast(&[1.0, 2.0], MAX_HORIZON + 1, &[0.5], None)
            .await
            .unwrap_err();
        assert_eq!(long.kind, FailureKind::Rejected, "{long}");
        let wide = client
            .forecast(&vec![1.0; MAX_CONTEXT + 1], 1, &[0.5], None)
            .await
            .unwrap_err();
        assert_eq!(wide.kind, FailureKind::Rejected, "{wide}");
        assert!(check_limits(MAX_CONTEXT, MAX_HORIZON).is_ok());
        let err = check_limits(MAX_CONTEXT + 1, 1).unwrap_err();
        assert_eq!(err.kind, FailureKind::Rejected);
        assert_eq!(
            check_limits(1, MAX_HORIZON + 1).unwrap_err().kind,
            FailureKind::Rejected
        );
    }

    /// The limits mirror the sidecar's; this reads them from its source.
    #[test]
    fn the_limits_match_the_sidecar_source() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../forecastd/forecastd/service.py"
        );
        let source = std::fs::read_to_string(path).unwrap();
        let constant = |name: &str| -> usize {
            let line = source
                .lines()
                .find(|l| l.starts_with(&format!("{name} = ")))
                .unwrap_or_else(|| panic!("{name} not found in {path}"));
            let value = line.split('=').nth(1).unwrap().trim().replace('_', "");
            value.parse().unwrap()
        };
        assert_eq!(constant("MAX_HORIZON"), MAX_HORIZON);
        assert_eq!(constant("MAX_CONTEXT"), MAX_CONTEXT);
    }

    #[test]
    fn request_serialises_gaps_as_null_and_omits_absent_covariates() {
        let r = Request {
            context: nullable(&[1.0, f64::NAN]),
            horizon: 1,
            quantile_levels: &[0.5],
            past_covariates: nullable_rows(&[]),
            known_covariates: nullable_rows(&[vec![0.0, 0.0, 0.0]]),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("[1.0,null]"));
        assert!(!json.contains("past_covariates"));
        assert!(json.contains("\"known_covariates\":[[0.0,0.0,0.0]]"));
    }
}
