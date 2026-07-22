//! Credential resolution from the standard provider environment chains.
//!
//! Nothing here ever hardcodes a secret. Every resolver is written against a
//! `lookup` closure so it can be unit-tested with an empty environment (proving
//! the fail-fast path) without mutating the real process environment. The
//! `*_from_env` wrappers bind the closure to [`std::env::var`].

use zeroize::Zeroizing;

use crate::error::CloudError;

/// AWS access credentials resolved from the standard environment variables.
///
/// The session token is optional and only present for temporary
/// (STS / instance-profile) credentials. Secret material is wrapped in
/// [`Zeroizing`] so it is scrubbed from memory on drop.
#[derive(Clone)]
pub struct AwsCredentials {
    /// `AWS_ACCESS_KEY_ID`.
    pub access_key_id: String,
    /// `AWS_SECRET_ACCESS_KEY`.
    pub secret_access_key: Zeroizing<String>,
    /// `AWS_SESSION_TOKEN`, when using temporary credentials.
    pub session_token: Option<Zeroizing<String>>,
}

// A manual, redacting `Debug` impl so credentials never leak into logs, panic
// messages, or test output. `access_key_id` is not secret, so it is shown.
impl std::fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl AwsCredentials {
    /// Resolve credentials from a `lookup` closure.
    ///
    /// # Errors
    ///
    /// Returns [`CloudError::MissingCredential`] naming the first absent
    /// required variable (`AWS_ACCESS_KEY_ID` or `AWS_SECRET_ACCESS_KEY`).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, CloudError> {
        // Only environment variables are ever read — there is no instance
        // profile / IMDS lookup — so the context line must say so.
        let access_key_id = lookup("AWS_ACCESS_KEY_ID").ok_or(CloudError::MissingCredential {
            var: "AWS_ACCESS_KEY_ID",
            context: "read from environment variables only (no instance-profile lookup)",
        })?;
        let secret_access_key =
            lookup("AWS_SECRET_ACCESS_KEY").ok_or(CloudError::MissingCredential {
                var: "AWS_SECRET_ACCESS_KEY",
                context: "read from environment variables only (no instance-profile lookup)",
            })?;
        Ok(Self {
            access_key_id,
            secret_access_key: Zeroizing::new(secret_access_key),
            session_token: lookup("AWS_SESSION_TOKEN").map(Zeroizing::new),
        })
    }

    /// Resolve credentials from the real process environment.
    ///
    /// # Errors
    ///
    /// Returns [`CloudError::MissingCredential`] if either required variable
    /// is unset — before any network call is made.
    pub fn from_env() -> Result<Self, CloudError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }
}

/// Resolve the AWS region: the explicit argument wins, otherwise
/// `AWS_REGION`, otherwise `AWS_DEFAULT_REGION`.
///
/// # Errors
///
/// Returns [`CloudError::MissingCredential`] if no region can be determined.
pub fn resolve_region(
    explicit: Option<&str>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, CloudError> {
    if let Some(region) = explicit {
        if !region.is_empty() {
            return Ok(region.to_string());
        }
    }
    lookup("AWS_REGION")
        .or_else(|| lookup("AWS_DEFAULT_REGION"))
        .ok_or(CloudError::MissingCredential {
            var: "AWS_REGION",
            context: "no `region` argument and neither AWS_REGION nor AWS_DEFAULT_REGION set",
        })
}

/// Convenience wrapper for [`resolve_region`] against the real environment.
///
/// # Errors
///
/// Returns [`CloudError::MissingCredential`] if no region can be determined.
pub fn region_from_env(explicit: Option<&str>) -> Result<String, CloudError> {
    resolve_region(explicit, |key| std::env::var(key).ok())
}

/// Resolve an Azure Resource Manager bearer token from a `lookup` closure.
///
/// The token is returned wrapped in [`Zeroizing`] so it is scrubbed from
/// memory on drop.
///
/// # Errors
///
/// Returns [`CloudError::MissingCredential`] naming `AZURE_ACCESS_TOKEN`.
pub fn azure_token(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Zeroizing<String>, CloudError> {
    lookup("AZURE_ACCESS_TOKEN")
        .map(Zeroizing::new)
        .ok_or(CloudError::MissingCredential {
            var: "AZURE_ACCESS_TOKEN",
            context:
                "obtain via `az account get-access-token --resource https://management.azure.com` \
                      or a managed-identity token",
        })
}

/// Convenience wrapper for [`azure_token`] against the real environment.
///
/// # Errors
///
/// Returns [`CloudError::MissingCredential`] if the token is unset.
pub fn azure_token_from_env() -> Result<Zeroizing<String>, CloudError> {
    azure_token(|key| std::env::var(key).ok())
}

/// Resolve a Google Cloud OAuth access token from a `lookup` closure.
///
/// The token is returned wrapped in [`Zeroizing`] so it is scrubbed from
/// memory on drop.
///
/// # Errors
///
/// Returns [`CloudError::MissingCredential`] naming `GOOGLE_OAUTH_ACCESS_TOKEN`.
pub fn gcp_token(lookup: impl Fn(&str) -> Option<String>) -> Result<Zeroizing<String>, CloudError> {
    lookup("GOOGLE_OAUTH_ACCESS_TOKEN")
        .or_else(|| lookup("CLOUDSDK_AUTH_ACCESS_TOKEN"))
        .map(Zeroizing::new)
        .ok_or(CloudError::MissingCredential {
            var: "GOOGLE_OAUTH_ACCESS_TOKEN",
            context: "obtain via `gcloud auth print-access-token`",
        })
}

/// Convenience wrapper for [`gcp_token`] against the real environment.
///
/// # Errors
///
/// Returns [`CloudError::MissingCredential`] if no token is set.
pub fn gcp_token_from_env() -> Result<Zeroizing<String>, CloudError> {
    gcp_token(|key| std::env::var(key).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CloudError;

    fn empty(_key: &str) -> Option<String> {
        None
    }

    #[test]
    fn aws_missing_access_key_fails_fast_naming_var() {
        let err = AwsCredentials::from_lookup(empty).unwrap_err();
        assert!(matches!(
            err,
            CloudError::MissingCredential {
                var: "AWS_ACCESS_KEY_ID",
                ..
            }
        ));
        assert!(err.to_string().contains("AWS_ACCESS_KEY_ID"));
    }

    #[test]
    fn aws_missing_secret_key_fails_fast() {
        let lookup = |key: &str| (key == "AWS_ACCESS_KEY_ID").then(|| "AKIA".to_string());
        let err = AwsCredentials::from_lookup(lookup).unwrap_err();
        assert!(matches!(
            err,
            CloudError::MissingCredential {
                var: "AWS_SECRET_ACCESS_KEY",
                ..
            }
        ));
    }

    #[test]
    fn aws_full_credentials_resolve() {
        let lookup = |key: &str| match key {
            "AWS_ACCESS_KEY_ID" => Some("AKIA".to_string()),
            "AWS_SECRET_ACCESS_KEY" => Some("secret".to_string()),
            "AWS_SESSION_TOKEN" => Some("token".to_string()),
            _ => None,
        };
        let creds = AwsCredentials::from_lookup(lookup).unwrap();
        assert_eq!(creds.access_key_id, "AKIA");
        assert_eq!(
            creds.session_token.as_deref().map(String::as_str),
            Some("token")
        );
    }

    #[test]
    fn region_prefers_explicit_then_env() {
        assert_eq!(
            resolve_region(Some("eu-west-1"), empty).unwrap(),
            "eu-west-1"
        );
        let lookup = |key: &str| (key == "AWS_REGION").then(|| "us-east-1".to_string());
        assert_eq!(resolve_region(None, lookup).unwrap(), "us-east-1");
        let fallback = |key: &str| (key == "AWS_DEFAULT_REGION").then(|| "ap-south-1".to_string());
        assert_eq!(resolve_region(Some(""), fallback).unwrap(), "ap-south-1");
    }

    #[test]
    fn region_absent_is_typed_error() {
        let err = resolve_region(None, empty).unwrap_err();
        assert!(matches!(
            err,
            CloudError::MissingCredential {
                var: "AWS_REGION",
                ..
            }
        ));
    }

    #[test]
    fn azure_and_gcp_tokens_fail_fast() {
        assert!(matches!(
            azure_token(empty).unwrap_err(),
            CloudError::MissingCredential {
                var: "AZURE_ACCESS_TOKEN",
                ..
            }
        ));
        assert!(matches!(
            gcp_token(empty).unwrap_err(),
            CloudError::MissingCredential {
                var: "GOOGLE_OAUTH_ACCESS_TOKEN",
                ..
            }
        ));
    }
}
