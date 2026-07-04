//! AWS connectors: Fault Injection Service (FIS) and a couple of direct,
//! high-signal EC2 faults.
//!
//! Requests are signed with `SigV4` ([`crate::sigv4`]) and sent with a plain
//! `reqwest` client — no `aws-sdk-*` dependency. Endpoints resolve from the
//! region by default; [`FisClient::with_endpoint`] / [`Ec2Client::with_endpoint`]
//! override the base URL for VPC interface endpoints and for the hermetic
//! mocked-HTTP tests.

use chrono::Utc;
use serde::Deserialize;

use crate::creds::AwsCredentials;
use crate::error::CloudError;
use crate::sigv4::{sign, SignRequest};

/// EC2 Query-protocol API version.
const EC2_API_VERSION: &str = "2016-11-15";

/// Build a `reqwest::Method` from an uppercase verb, defaulting to `GET`.
fn method(verb: &str) -> reqwest::Method {
    match verb {
        "POST" => reqwest::Method::POST,
        "DELETE" => reqwest::Method::DELETE,
        _ => reqwest::Method::GET,
    }
}

/// Host authority (`host` or `host:port`) as `reqwest` will send it — the port
/// is included only when explicitly present (non-default), matching the mock
/// server and real service alike.
fn authority(url: &reqwest::Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => String::new(),
    }
}

/// A `SigV4`-signing HTTP sender bound to one endpoint, region, and set of
/// credentials. Shared by [`FisClient`] and [`Ec2Client`].
struct Signer {
    http: reqwest::Client,
    endpoint: String,
    region: String,
    creds: AwsCredentials,
}

impl Signer {
    fn new(endpoint: String, region: String, creds: AwsCredentials) -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint,
            region,
            creds,
        }
    }

    /// Sign and send one request, returning the response body text on 2xx or a
    /// typed [`CloudError`] otherwise.
    async fn send(
        &self,
        service: &str,
        verb: &str,
        path: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<String, CloudError> {
        let url_str = format!("{}{path}", self.endpoint);
        let url = reqwest::Url::parse(&url_str).map_err(|e| CloudError::InvalidConfig {
            field: "endpoint",
            reason: e.to_string(),
        })?;
        let host = authority(&url);

        let extra: Vec<(String, String)> = content_type
            .map(|ct| vec![("content-type".to_string(), ct.to_string())])
            .unwrap_or_default();

        let signed = sign(
            &SignRequest {
                method: verb,
                host: &host,
                path,
                query: url.query().unwrap_or_default(),
                body: &body,
                service,
                region: &self.region,
                extra_headers: &extra,
            },
            &self.creds,
            Utc::now(),
        );

        let mut request = self.http.request(method(verb), url);
        if let Some(ct) = content_type {
            request = request.header("content-type", ct);
        }
        for (name, value) in signed {
            request = request.header(name, value);
        }
        if !body.is_empty() {
            request = request.body(body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| CloudError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| CloudError::Transport(e.to_string()))?;
        if (200..300).contains(&status) {
            Ok(text)
        } else {
            Err(CloudError::from_status("aws", status, &text))
        }
    }
}

// ── FIS ────────────────────────────────────────────────────────

/// A summary of an FIS experiment's state, parsed from the API response.
#[derive(Debug, Deserialize)]
struct FisExperiment {
    id: String,
    #[serde(default)]
    state: FisState,
}

/// The `state` object of an FIS experiment.
#[derive(Debug, Default, Deserialize)]
struct FisState {
    #[serde(default)]
    status: String,
    #[serde(default)]
    reason: Option<String>,
}

/// The `{ "experiment": { … } }` envelope returned by FIS operations.
#[derive(Debug, Deserialize)]
struct FisEnvelope {
    experiment: FisExperiment,
}

/// Render a parsed FIS response into a stable, human- and machine-friendly
/// one-line summary.
fn summarize_fis(verb: &str, text: &str) -> String {
    match serde_json::from_str::<FisEnvelope>(text) {
        Ok(env) => {
            let reason = env
                .experiment
                .state
                .reason
                .filter(|r| !r.is_empty())
                .map(|r| format!(", reason: {r}"))
                .unwrap_or_default();
            format!(
                "{verb} FIS experiment {} (status: {}{reason})",
                env.experiment.id, env.experiment.state.status
            )
        }
        Err(_) => format!("{verb} FIS experiment: {text}"),
    }
}

/// Connector for the AWS Fault Injection Service control plane.
pub struct FisClient {
    signer: Signer,
}

impl FisClient {
    /// Build a client whose endpoint is derived from `region`
    /// (`https://fis.<region>.amazonaws.com`).
    #[must_use]
    pub fn new(region: String, creds: AwsCredentials) -> Self {
        let endpoint = format!("https://fis.{region}.amazonaws.com");
        Self {
            signer: Signer::new(endpoint, region, creds),
        }
    }

    /// Build a client against an explicit `endpoint` base URL (VPC interface
    /// endpoint, or a mock server in tests).
    #[must_use]
    pub fn with_endpoint(endpoint: String, region: String, creds: AwsCredentials) -> Self {
        Self {
            signer: Signer::new(endpoint, region, creds),
        }
    }

    /// Start an experiment from a template (`StartExperiment`,
    /// `POST /experiments`).
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response (auth / not-found / throttled / generic API error).
    pub async fn start_experiment(&self, template_id: &str) -> Result<String, CloudError> {
        // Idempotency token — unique per invocation, no `uuid` dependency.
        let client_token = format!(
            "tumult-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let body = serde_json::json!({
            "clientToken": client_token,
            "experimentTemplateId": template_id,
        });
        let payload = serde_json::to_vec(&body).map_err(|e| CloudError::InvalidConfig {
            field: "experiment_template_id",
            reason: e.to_string(),
        })?;
        let text = self
            .signer
            .send(
                "fis",
                "POST",
                "/experiments",
                payload,
                Some("application/json"),
            )
            .await?;
        Ok(summarize_fis("started", &text))
    }

    /// Stop a running experiment (`StopExperiment`,
    /// `DELETE /experiments/{id}`).
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response.
    pub async fn stop_experiment(&self, experiment_id: &str) -> Result<String, CloudError> {
        let path = format!("/experiments/{experiment_id}");
        let text = self
            .signer
            .send("fis", "DELETE", &path, Vec::new(), None)
            .await?;
        Ok(summarize_fis("stopped", &text))
    }

    /// Get an experiment's current state (`GetExperiment`,
    /// `GET /experiments/{id}`).
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response.
    pub async fn experiment_status(&self, experiment_id: &str) -> Result<String, CloudError> {
        let path = format!("/experiments/{experiment_id}");
        let text = self
            .signer
            .send("fis", "GET", &path, Vec::new(), None)
            .await?;
        Ok(summarize_fis("status of", &text))
    }
}

// ── EC2 (direct faults) ────────────────────────────────────────

/// Extract the text of the first `<tag>…</tag>` element, if present.
fn extract_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(&xml[start..end])
}

/// Connector for direct EC2 instance faults via the Query protocol.
pub struct Ec2Client {
    signer: Signer,
}

impl Ec2Client {
    /// Build a client whose endpoint is derived from `region`
    /// (`https://ec2.<region>.amazonaws.com`).
    #[must_use]
    pub fn new(region: String, creds: AwsCredentials) -> Self {
        let endpoint = format!("https://ec2.{region}.amazonaws.com");
        Self {
            signer: Signer::new(endpoint, region, creds),
        }
    }

    /// Build a client against an explicit `endpoint` base URL (mock server in
    /// tests).
    #[must_use]
    pub fn with_endpoint(endpoint: String, region: String, creds: AwsCredentials) -> Self {
        Self {
            signer: Signer::new(endpoint, region, creds),
        }
    }

    /// Stop an instance (`StopInstances`).
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response.
    pub async fn stop_instance(&self, instance_id: &str) -> Result<String, CloudError> {
        self.instance_action("StopInstances", instance_id).await
    }

    /// Terminate an instance (`TerminateInstances`). Irreversible.
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response.
    pub async fn terminate_instance(&self, instance_id: &str) -> Result<String, CloudError> {
        self.instance_action("TerminateInstances", instance_id)
            .await
    }

    /// Send a single-instance EC2 Query action against `POST /`.
    async fn instance_action(&self, action: &str, instance_id: &str) -> Result<String, CloudError> {
        // EC2 Query protocol: parameters go in the x-www-form-urlencoded body,
        // which is what gets signed as the payload.
        let body = format!(
            "Action={action}&Version={EC2_API_VERSION}&InstanceId.1={}",
            crate::sigv4::encode_query_value(instance_id)
        );
        let text = self
            .signer
            .send(
                "ec2",
                "POST",
                "/",
                body.into_bytes(),
                Some("application/x-www-form-urlencoded"),
            )
            .await?;
        let state = extract_tag(&text, "name").unwrap_or("accepted");
        Ok(format!("EC2 {action} on {instance_id}: {state}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_includes_explicit_port() {
        let url = reqwest::Url::parse("http://127.0.0.1:8080/x").unwrap();
        assert_eq!(authority(&url), "127.0.0.1:8080");
        let url = reqwest::Url::parse("https://fis.us-east-1.amazonaws.com/x").unwrap();
        assert_eq!(authority(&url), "fis.us-east-1.amazonaws.com");
    }

    #[test]
    fn extract_tag_pulls_first_element() {
        let xml = "<a><currentState><code>64</code><name>stopping</name></currentState></a>";
        assert_eq!(extract_tag(xml, "name"), Some("stopping"));
        assert_eq!(extract_tag(xml, "missing"), None);
    }

    #[test]
    fn summarize_fis_parses_envelope() {
        let text = r#"{"experiment":{"id":"EXPabc","state":{"status":"running"}}}"#;
        let summary = summarize_fis("started", text);
        assert!(summary.contains("EXPabc"));
        assert!(summary.contains("running"));
    }

    #[test]
    fn summarize_fis_falls_back_on_unexpected_body() {
        assert!(summarize_fis("started", "not json").contains("not json"));
    }
}
