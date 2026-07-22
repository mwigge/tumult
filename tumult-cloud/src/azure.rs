//! Azure Chaos Studio connector via the Azure Resource Manager (ARM) REST API.
//!
//! Authenticated with a bearer token from the standard Azure credential chain
//! (`AZURE_ACCESS_TOKEN`, obtainable from
//! `az account get-access-token --resource https://management.azure.com` or a
//! managed identity). [`ChaosClient::with_endpoint`] overrides the ARM base URL
//! for the hermetic mocked-HTTP tests.

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::error::CloudError;

/// ARM API version for the `Microsoft.Chaos` resource provider (stable GA).
const API_VERSION: &str = "2024-01-01";

/// Build a `reqwest::Method` from an uppercase verb.
fn method(verb: &str) -> reqwest::Method {
    match verb {
        "POST" => reqwest::Method::POST,
        _ => reqwest::Method::GET,
    }
}

/// The `{ "properties": { "provisioningState": … } }` shape of an ARM
/// experiment resource, used to summarize status.
#[derive(Debug, Deserialize)]
struct ExperimentResource {
    #[serde(default)]
    properties: ExperimentProperties,
}

#[derive(Debug, Default, Deserialize)]
struct ExperimentProperties {
    #[serde(rename = "provisioningState", default)]
    provisioning_state: String,
}

/// Connector for the Azure Chaos Studio experiment control plane.
pub struct ChaosClient {
    http: reqwest::Client,
    endpoint: String,
    /// Bearer token, held zeroized so it is scrubbed from memory on drop.
    token: Zeroizing<String>,
}

impl ChaosClient {
    /// Build a client against the public ARM endpoint
    /// (`https://management.azure.com`).
    #[must_use]
    pub fn new(token: impl Into<Zeroizing<String>>) -> Self {
        Self {
            http: crate::http::client(),
            endpoint: "https://management.azure.com".to_string(),
            token: token.into(),
        }
    }

    /// Build a client against an explicit ARM `endpoint` (mock server in
    /// tests, or a sovereign cloud).
    #[must_use]
    pub fn with_endpoint(endpoint: String, token: impl Into<Zeroizing<String>>) -> Self {
        Self {
            http: crate::http::client(),
            endpoint,
            token: token.into(),
        }
    }

    /// Resource path of a Chaos experiment.
    fn experiment_path(subscription: &str, resource_group: &str, experiment: &str) -> String {
        format!(
            "/subscriptions/{subscription}/resourceGroups/{resource_group}\
             /providers/Microsoft.Chaos/experiments/{experiment}"
        )
    }

    /// Send one bearer-authenticated request, returning the body text on 2xx
    /// or a typed [`CloudError`] otherwise.
    async fn send(&self, verb: &str, url: String) -> Result<String, CloudError> {
        let response = self
            .http
            .request(method(verb), url)
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
        if (200..300).contains(&status) {
            Ok(text)
        } else {
            Err(CloudError::from_status("azure", status, &text))
        }
    }

    /// Start a Chaos experiment (`POST …/start`).
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response.
    pub async fn start(
        &self,
        subscription: &str,
        resource_group: &str,
        experiment: &str,
    ) -> Result<String, CloudError> {
        let path = Self::experiment_path(subscription, resource_group, experiment);
        let url = format!("{}{path}/start?api-version={API_VERSION}", self.endpoint);
        self.send("POST", url).await?;
        Ok(format!("started Azure Chaos experiment {experiment}"))
    }

    /// Cancel a running Chaos experiment (`POST …/cancel`).
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response.
    pub async fn cancel(
        &self,
        subscription: &str,
        resource_group: &str,
        experiment: &str,
    ) -> Result<String, CloudError> {
        let path = Self::experiment_path(subscription, resource_group, experiment);
        let url = format!("{}{path}/cancel?api-version={API_VERSION}", self.endpoint);
        self.send("POST", url).await?;
        Ok(format!("cancelled Azure Chaos experiment {experiment}"))
    }

    /// Get a Chaos experiment's provisioning state (`GET …`).
    ///
    /// Note: this reports the experiment resource's `provisioningState`.
    /// Fine-grained run status lives under the `/executions` sub-API, which a
    /// thin connector does not track.
    ///
    /// # Errors
    ///
    /// Returns a typed [`CloudError`] on transport failure or a non-2xx
    /// response.
    pub async fn status(
        &self,
        subscription: &str,
        resource_group: &str,
        experiment: &str,
    ) -> Result<String, CloudError> {
        let path = Self::experiment_path(subscription, resource_group, experiment);
        let url = format!("{}{path}?api-version={API_VERSION}", self.endpoint);
        let text = self.send("GET", url).await?;
        let state = serde_json::from_str::<ExperimentResource>(&text)
            .map(|r| r.properties.provisioning_state)
            .unwrap_or_default();
        if state.is_empty() {
            Ok(format!("Azure Chaos experiment {experiment}: {text}"))
        } else {
            Ok(format!(
                "Azure Chaos experiment {experiment} (provisioningState: {state})"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experiment_path_is_arm_shaped() {
        let path = ChaosClient::experiment_path("sub1", "rg1", "exp1");
        assert_eq!(
            path,
            "/subscriptions/sub1/resourceGroups/rg1/providers/Microsoft.Chaos/experiments/exp1"
        );
    }

    #[test]
    fn status_parses_provisioning_state() {
        let text = r#"{"properties":{"provisioningState":"Succeeded"}}"#;
        let state = serde_json::from_str::<ExperimentResource>(text)
            .unwrap()
            .properties
            .provisioning_state;
        assert_eq!(state, "Succeeded");
    }
}
