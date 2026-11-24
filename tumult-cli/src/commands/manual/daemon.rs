//! The daemon API as `tumult manual` talks to it: the URL of `--daemon-url`
//! or `TUMULT_DAEMON_URL`, the token of `TUMULT_DAEMON_TOKEN`, as for
//! `tumult gate` and `tumult perf`.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{Method, RequestBuilder};
use serde_json::Value;

/// Env var naming the daemon base URL.
const DAEMON_URL_ENV: &str = "TUMULT_DAEMON_URL";
/// Env var with the daemon API token (`kro_...`).
const DAEMON_TOKEN_ENV: &str = "TUMULT_DAEMON_TOKEN";
/// How long a request may take; a file upload may take longer.
const TIMEOUT: Duration = Duration::from_secs(30);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// A daemon to send requests to.
pub struct Daemon {
    base: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl Daemon {
    /// The daemon at `url`, else at `TUMULT_DAEMON_URL`.
    ///
    /// # Errors
    ///
    /// Returns an error when neither names a daemon.
    pub fn connect(url: Option<&str>) -> Result<Self> {
        let base = url
            .map(str::to_owned)
            .or_else(|| std::env::var(DAEMON_URL_ENV).ok())
            .filter(|s| !s.is_empty())
            .with_context(|| format!("no daemon URL: pass --daemon-url or set {DAEMON_URL_ENV}"))?;
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            token: std::env::var(DAEMON_TOKEN_ENV)
                .ok()
                .filter(|t| !t.is_empty()),
            client,
        })
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self.client.request(method, format!("{}{path}", self.base));
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// `GET path?query`, the JSON answer.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or does not
    /// answer with success and JSON.
    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        answer(self.request(Method::GET, path).query(query), path).await
    }

    /// `POST path` with `body` as JSON, the JSON answer.
    ///
    /// # Errors
    ///
    /// As [`Daemon::get`].
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        answer(self.request(Method::POST, path).json(body), path).await
    }

    /// `POST path?query` with `bytes` as an `application/octet-stream`
    /// body, the JSON answer.
    ///
    /// # Errors
    ///
    /// As [`Daemon::get`].
    pub async fn post_file(
        &self,
        path: &str,
        query: &[(&str, String)],
        bytes: Vec<u8>,
    ) -> Result<Value> {
        let request = self
            .request(Method::POST, path)
            .query(query)
            .timeout(UPLOAD_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes);
        answer(request, path).await
    }
}

/// Send `request` and read its answer: the JSON of a success, else an
/// error with the status and what the daemon said.
async fn answer(request: RequestBuilder, path: &str) -> Result<Value> {
    let resp = request
        .send()
        .await
        .with_context(|| format!("request to {path} failed"))?;
    let status = resp.status();
    let text = resp.text().await.with_context(|| {
        format!("{path} answered HTTP {status} with a body that could not be read")
    })?;
    if !status.is_success() {
        bail!("{path} answered HTTP {status}: {}", said(&text));
    }
    serde_json::from_str(&text)
        .with_context(|| format!("{path} answered HTTP {status} with a body that is not JSON"))
}

/// What an error answer says: its `error` field, or the text itself (a
/// body the API could not read is refused in plain text).
fn said(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_answer_says_its_error_field_or_its_text() {
        assert_eq!(
            said(r#"{"error": "plan 'x' not found"}"#),
            "plan 'x' not found"
        );
        assert_eq!(
            said("Failed to deserialize the JSON body: unknown field `rpo`\n"),
            "Failed to deserialize the JSON body: unknown field `rpo`"
        );
        assert_eq!(said(r#"{"ok": false}"#), r#"{"ok": false}"#);
    }
}
