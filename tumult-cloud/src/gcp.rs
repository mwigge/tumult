//! Google Cloud connector — a single direct, high-signal Compute Engine fault.
//!
//! **GCP has no first-party managed chaos service.** Google's own guidance
//! points users at third-party tools (Chaos Toolkit, Gremlin, Litmus) rather
//! than a native equivalent of AWS FIS or Azure Chaos Studio. This connector
//! therefore exposes only a direct instance-stop via the Compute Engine REST
//! API and does not pretend a managed chaos API exists.
//!
//! Authenticated with a bearer token from the standard Google credential chain
//! (`GOOGLE_OAUTH_ACCESS_TOKEN`, obtainable from
//! `gcloud auth print-access-token`). [`ComputeClient::with_endpoint`]
//! overrides the base URL for the hermetic mocked-HTTP tests.

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::error::CloudError;

/// The `Operation` resource returned by an asynchronous Compute API call.
#[derive(Debug, Deserialize)]
struct Operation {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
}

/// Connector for direct Compute Engine instance faults.
pub struct ComputeClient {
    http: reqwest::Client,
    endpoint: String,
    /// Bearer token, held zeroized so it is scrubbed from memory on drop.
    token: Zeroizing<String>,
}

impl ComputeClient {
    /// Build a client against the public Compute API endpoint
    /// (`https://compute.googleapis.com`).
    #[must_use]
    pub fn new(token: impl Into<Zeroizing<String>>) -> Self {
        Self {
            http: crate::http::client(),
            endpoint: "https://compute.googleapis.com".to_string(),
            token: token.into(),
        }
    }

    /// Build a client against an explicit `endpoint` (mock server in tests).
    #[must_use]
    pub fn with_endpoint(endpoint: String, token: impl Into<Zeroizing<String>>) -> Self {
        Self {
            http: crate::http::client(),
            endpoint,
            token: token.into(),
        }
    }

    /// Stop a Compute Engine instance
    /// (`POST …/projects/{project}/zones/{zone}/instances/{instance}/stop`).
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response.
    pub async fn stop_instance(
        &self,
        project: &str,
        zone: &str,
        instance: &str,
    ) -> Result<String, CloudError> {
        let url = format!(
            "{}/compute/v1/projects/{project}/zones/{zone}/instances/{instance}/stop",
            self.endpoint
        );
        let response = self
            .http
            .post(url)
            .bearer_auth(self.token.as_str())
            .header("content-length", "0")
            .send()
            .await
            .map_err(|e| CloudError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| CloudError::Transport(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(CloudError::from_status("gcp", status, &text));
        }
        match serde_json::from_str::<Operation>(&text) {
            Ok(op) if !op.name.is_empty() => Ok(format!(
                "GCP stop instance {instance}: operation {} (status: {})",
                op.name, op.status
            )),
            _ => Ok(format!("GCP stop instance {instance}: accepted")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_parses_name_and_status() {
        let text = r#"{"name":"operation-123","status":"RUNNING"}"#;
        let op: Operation = serde_json::from_str(text).unwrap();
        assert_eq!(op.name, "operation-123");
        assert_eq!(op.status, "RUNNING");
    }
}
