//! HTTP client for the `tumult-forecastd` sidecar (`POST /forecast`), the
//! Timestral provider behind `TUMULTD_FORECAST_URL`. Asynchronous because
//! it runs inside the API's tokio runtime; the pure provider trait stays
//! synchronous and the handlers bridge the two through `forecast::produce`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tumult_forecast::{Covariates, Forecast, ForecastError, Provenance, MIN_HISTORY};

const TIMEOUT: Duration = Duration::from_secs(15);

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
    weights_hash: String,
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

/// Turn the sidecar's answer into a [`Forecast`] for one variate.
pub(crate) fn parse_response(body: &str, horizon: usize) -> Result<Forecast, ForecastError> {
    let resp: Response = serde_json::from_str(body)
        .map_err(|e| ForecastError::Provider(format!("bad response: {e}")))?;
    let Some(first) = resp.quantiles.into_iter().next() else {
        return Err(ForecastError::Provider("response has no variate".into()));
    };
    if first.len() != horizon {
        return Err(ForecastError::Provider(format!(
            "response covers {} steps, requested {horizon}",
            first.len()
        )));
    }
    let width = resp.levels.len();
    let quantiles: Vec<Vec<f64>> = first
        .into_iter()
        .map(|step| {
            step.into_iter()
                .map(|v| v.unwrap_or(f64::NAN))
                .collect::<Vec<f64>>()
        })
        .collect();
    if quantiles.iter().any(|step| step.len() != width) {
        return Err(ForecastError::Provider(
            "response quantiles do not match its levels".into(),
        ));
    }
    Ok(Forecast {
        levels: resp.levels,
        quantiles,
        context_points: resp.context_points,
        insufficient_history: resp.context_points < MIN_HISTORY,
        provenance: Provenance {
            model: resp.model.model,
            version: format!("{}@{}", resp.model.model_id, resp.model.version),
            weights_hash: resp.model.weights_hash,
        },
    })
}

/// The sidecar client.
#[derive(Clone)]
pub struct TimestralClient {
    base: String,
    http: reqwest::Client,
}

impl TimestralClient {
    /// A client for the sidecar at `base` (e.g. `http://forecastd:8700`).
    #[must_use]
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    /// `POST /forecast` for one variate.
    ///
    /// # Errors
    /// [`ForecastError::Provider`] when the sidecar is unreachable, answers
    /// a non-2xx status or a body the client cannot read.
    pub async fn forecast(
        &self,
        context: &[f64],
        horizon: usize,
        levels: &[f64],
        covariates: Option<&Covariates>,
    ) -> Result<Forecast, ForecastError> {
        let request = Request {
            context: nullable(context),
            horizon,
            quantile_levels: levels,
            past_covariates: covariates.and_then(|c| nullable_rows(&c.past)),
            known_covariates: covariates.and_then(|c| nullable_rows(&c.known_future)),
        };
        let resp = self
            .http
            .post(format!("{}/forecast", self.base))
            .json(&request)
            .send()
            .await
            .map_err(|e| ForecastError::Provider(format!("sidecar unreachable: {e}")))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| ForecastError::Provider(format!("sidecar body unreadable: {e}")))?;
        if !status.is_success() {
            return Err(ForecastError::Provider(format!(
                "sidecar answered {status}: {}",
                body.chars().take(200).collect::<String>()
            )));
        }
        parse_response(&body, horizon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_parses_into_a_forecast_with_provenance() {
        let body = r#"{"levels":[0.1,0.5,0.9],"requested_levels":[0.1,0.5,0.9],"horizon":2,
            "context_points":12,"quantiles":[[[1.0,2.0,3.0],[1.5,null,3.5]]],
            "model":{"model":"timestral","model_id":"mistralai/Timestral-5M","size":"5M",
            "version":"0.1","weights_hash":"abc"}}"#;
        let f = parse_response(body, 2).unwrap();
        assert_eq!(f.levels, vec![0.1, 0.5, 0.9]);
        assert_eq!(f.quantiles[0], vec![1.0, 2.0, 3.0]);
        assert!(f.quantiles[1][1].is_nan());
        assert_eq!(f.context_points, 12);
        assert!(!f.insufficient_history);
        assert_eq!(f.provenance.model, "timestral");
        assert_eq!(f.provenance.version, "mistralai/Timestral-5M@0.1");
        assert_eq!(f.provenance.weights_hash, "abc");
    }

    #[test]
    fn bad_responses_are_provider_errors() {
        assert!(matches!(
            parse_response("not json", 1),
            Err(ForecastError::Provider(_))
        ));
        let no_variate = r#"{"levels":[0.5],"quantiles":[],"model":{"model":"t","model_id":"x"}}"#;
        assert!(matches!(
            parse_response(no_variate, 1),
            Err(ForecastError::Provider(_))
        ));
        let wrong_len =
            r#"{"levels":[0.5],"quantiles":[[[1.0]]],"model":{"model":"t","model_id":"x"}}"#;
        assert!(matches!(
            parse_response(wrong_len, 2),
            Err(ForecastError::Provider(_))
        ));
        let ragged =
            r#"{"levels":[0.5,0.9],"quantiles":[[[1.0]]],"model":{"model":"t","model_id":"x"}}"#;
        assert!(matches!(
            parse_response(ragged, 1),
            Err(ForecastError::Provider(_))
        ));
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

    #[tokio::test]
    async fn unreachable_sidecar_is_a_provider_error() {
        let client = TimestralClient::new("http://127.0.0.1:1/");
        let err = client
            .forecast(&[1.0, 2.0], 1, &[0.5], None)
            .await
            .unwrap_err();
        assert!(matches!(err, ForecastError::Provider(ref m) if m.contains("unreachable")));
    }
}
