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
    /// The sidecar did not answer in time.
    Timeout,
    /// A 5xx, a redirect, an oversized body or an answer that is not the
    /// forecast that was asked for.
    BadResponse,
    /// The sidecar is alive and turned this one request down (4xx), or the
    /// request exceeds what it accepts.
    Rejected,
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
        }
    }

    /// Whether the sidecar itself is at fault and should be skipped for a
    /// while. A rejected request says nothing against the sidecar.
    #[must_use]
    pub fn trips_breaker(self) -> bool {
        self != Self::Rejected
    }
}

/// Why the sidecar did not produce a forecast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarError {
    pub kind: FailureKind,
    /// What went wrong, for the log only.
    pub detail: String,
}

impl SidecarError {
    fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
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
        if horizon > MAX_HORIZON || context.len() > MAX_CONTEXT {
            return Err(SidecarError::new(
                FailureKind::Rejected,
                format!(
                    "{} context points and a horizon of {horizon} exceed the sidecar's \
                     {MAX_CONTEXT} and {MAX_HORIZON}",
                    context.len()
                ),
            ));
        }
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
        let body = read_capped(resp, MAX_RESPONSE_BYTES).await?;
        if !status.is_success() {
            let kind = if status.is_client_error() {
                FailureKind::Rejected
            } else {
                FailureKind::BadResponse
            };
            let excerpt: String = body.chars().take(200).collect();
            return Err(SidecarError::new(
                kind,
                format!("sidecar answered {status}: {excerpt}"),
            ));
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

    #[tokio::test]
    async fn statuses_map_to_coarse_kinds() {
        let app = axum::Router::new()
            .route(
                "/a/forecast",
                post(|| async { (StatusCode::BAD_REQUEST, "context too short") }),
            )
            .route(
                "/b/forecast",
                post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
            );
        let base = serve(app).await;
        let rejected = ask(&TimestralClient::new(&format!("{base}/a")).unwrap())
            .await
            .unwrap_err();
        assert_eq!(rejected.kind, FailureKind::Rejected);
        assert!(!rejected.kind.trips_breaker());
        assert!(rejected.detail.contains("context too short"), "{rejected}");
        let broken = ask(&TimestralClient::new(&format!("{base}/b")).unwrap())
            .await
            .unwrap_err();
        assert_eq!(broken.kind, FailureKind::BadResponse);
        assert!(broken.kind.trips_breaker());
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
    async fn a_slow_sidecar_times_out() {
        let app = axum::Router::new().route(
            "/forecast",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                GOOD
            }),
        );
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
