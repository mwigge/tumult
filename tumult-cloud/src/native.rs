//! Native dispatch — [`NativeExecutor`] implementation for `tumult-cloud`.
//!
//! Routes `tumult-cloud.<function>` calls to the AWS, Azure, and GCP
//! connectors. Credentials are resolved from the standard provider environment
//! chains and validated *before* any network call, so a missing credential
//! fails fast with the exact variable name rather than a connection error.

use tumult_plugin::native::{arg_str, NativeArgs, NativeError, NativeExecutor};

use crate::aws::{Ec2Client, FisClient};
use crate::azure::ChaosClient;
use crate::creds::{azure_token_from_env, gcp_token_from_env, region_from_env, AwsCredentials};
use crate::gcp::ComputeClient;

/// Functions `tumult-cloud` provides to the experiment runner.
const FUNCTIONS: &[&str] = &[
    "aws_fis_start_experiment",
    "aws_fis_stop_experiment",
    "aws_fis_experiment_status",
    "aws_ec2_stop_instance",
    "aws_ec2_terminate_instance",
    "azure_chaos_start",
    "azure_chaos_cancel",
    "azure_chaos_status",
    "gcp_compute_stop_instance",
];

/// Read an optional string argument, returning `None` when absent or non-string.
fn opt_str<'a>(args: &'a NativeArgs, key: &str) -> Option<&'a str> {
    args.get(key).and_then(serde_json::Value::as_str)
}

/// Convert a connector [`CloudError`](crate::error::CloudError) into a
/// [`NativeError`].
fn exec_err(e: crate::error::CloudError) -> NativeError {
    NativeError::execution(e)
}

/// The FIS experiment this process started last.
///
/// A stop or a status step without `experiment_id` means that experiment,
/// so an experiment file does not have to guess an id before the run.
#[derive(Default)]
struct StartedFis(std::sync::Mutex<Option<String>>);

impl StartedFis {
    const fn new() -> Self {
        Self(std::sync::Mutex::new(None))
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keep the id of a start. A start that gave no id changes nothing.
    fn remember(&self, id: Option<String>) {
        let Some(id) = id else {
            return;
        };
        *self.slot() = Some(id);
    }

    /// The id a step means: the one it names, or the one started last.
    fn resolve(&self, args: &NativeArgs) -> Result<String, NativeError> {
        if let Some(id) = opt_str(args, "experiment_id") {
            return Ok(id.to_owned());
        }
        self.slot()
            .clone()
            .ok_or_else(|| NativeError::MissingArgument {
                argument: "experiment_id".into(),
            })
    }
}

static STARTED_FIS: StartedFis = StartedFis::new();

/// [`NativeExecutor`] for the `tumult-cloud` connectors.
pub struct CloudExecutor;

#[async_trait::async_trait(?Send)]
impl NativeExecutor for CloudExecutor {
    fn name(&self) -> &'static str {
        "tumult-cloud"
    }

    fn functions(&self) -> &'static [&'static str] {
        FUNCTIONS
    }

    async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError> {
        // Validate the function name before touching arguments or credentials,
        // so typos fail with the available-function list.
        if !FUNCTIONS.contains(&function) {
            return Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            ));
        }

        match function {
            "aws_fis_start_experiment" => {
                let template = arg_str(args, "experiment_template_id")?;
                let client = fis_client(args)?;
                let started = client.start(template).await.map_err(exec_err)?;
                STARTED_FIS.remember(started.id);
                Ok(started.summary)
            }
            "aws_fis_stop_experiment" => {
                let id = STARTED_FIS.resolve(args)?;
                let client = fis_client(args)?;
                client.stop_experiment(&id).await.map_err(exec_err)
            }
            "aws_fis_experiment_status" => {
                let id = STARTED_FIS.resolve(args)?;
                let client = fis_client(args)?;
                client.experiment_status(&id).await.map_err(exec_err)
            }
            "aws_ec2_stop_instance" => {
                let instance = arg_str(args, "instance_id")?;
                let client = ec2_client(args)?;
                client.stop_instance(instance).await.map_err(exec_err)
            }
            "aws_ec2_terminate_instance" => {
                let instance = arg_str(args, "instance_id")?;
                let client = ec2_client(args)?;
                client.terminate_instance(instance).await.map_err(exec_err)
            }
            "azure_chaos_start" => {
                let (sub, rg, exp) = azure_args(args)?;
                azure_client()?.start(sub, rg, exp).await.map_err(exec_err)
            }
            "azure_chaos_cancel" => {
                let (sub, rg, exp) = azure_args(args)?;
                azure_client()?.cancel(sub, rg, exp).await.map_err(exec_err)
            }
            "azure_chaos_status" => {
                let (sub, rg, exp) = azure_args(args)?;
                azure_client()?.status(sub, rg, exp).await.map_err(exec_err)
            }
            "gcp_compute_stop_instance" => {
                let project = arg_str(args, "project")?;
                let zone = arg_str(args, "zone")?;
                let instance = arg_str(args, "instance")?;
                let token = gcp_token_from_env().map_err(exec_err)?;
                ComputeClient::new(token)
                    .stop_instance(project, zone, instance)
                    .await
                    .map_err(exec_err)
            }
            _ => Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            )),
        }
    }
}

/// The endpoint set for one AWS service, if any.
///
/// `AWS_ENDPOINT_URL_<SERVICE>` is the variable the AWS command line and the
/// AWS SDKs read: a VPC interface endpoint, or a local stand-in for the
/// service.
fn endpoint_override(service: &str) -> Option<String> {
    std::env::var(format!("AWS_ENDPOINT_URL_{service}")).ok()
}

/// Build an FIS client from the region argument/environment and AWS creds.
fn fis_client(args: &NativeArgs) -> Result<FisClient, NativeError> {
    let region = region_from_env(opt_str(args, "region")).map_err(exec_err)?;
    let creds = AwsCredentials::from_env().map_err(exec_err)?;
    let Some(endpoint) = endpoint_override("FIS") else {
        return Ok(FisClient::new(region, creds));
    };
    Ok(FisClient::with_endpoint(endpoint, region, creds))
}

/// Build an EC2 client from the region argument/environment and AWS creds.
fn ec2_client(args: &NativeArgs) -> Result<Ec2Client, NativeError> {
    let region = region_from_env(opt_str(args, "region")).map_err(exec_err)?;
    let creds = AwsCredentials::from_env().map_err(exec_err)?;
    let Some(endpoint) = endpoint_override("EC2") else {
        return Ok(Ec2Client::new(region, creds));
    };
    Ok(Ec2Client::with_endpoint(endpoint, region, creds))
}

/// Build an Azure Chaos Studio client from the ambient bearer token.
fn azure_client() -> Result<ChaosClient, NativeError> {
    let token = azure_token_from_env().map_err(exec_err)?;
    Ok(ChaosClient::new(token))
}

/// Extract the `(subscription, resource_group, experiment_id)` triple shared by
/// the Azure Chaos functions.
fn azure_args(args: &NativeArgs) -> Result<(&str, &str, &str), NativeError> {
    let sub = arg_str(args, "subscription")?;
    let rg = arg_str(args, "resource_group")?;
    let exp = arg_str(args, "experiment_id")?;
    Ok((sub, rg, exp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_names_plugin_and_functions() {
        let executor = CloudExecutor;
        assert_eq!(executor.name(), "tumult-cloud");
        assert!(executor.functions().contains(&"aws_fis_start_experiment"));
        assert!(executor.functions().contains(&"azure_chaos_start"));
        assert!(executor.functions().contains(&"gcp_compute_stop_instance"));
    }

    #[tokio::test]
    async fn unknown_function_is_rejected() {
        let err = CloudExecutor
            .execute("nuke_everything", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::UnknownFunction { .. }),
            "expected UnknownFunction, got: {err:?}"
        );
        assert!(err.to_string().contains("aws_fis_start_experiment"));
    }

    #[tokio::test]
    async fn missing_template_id_is_typed_error_before_network() {
        let err = CloudExecutor
            .execute("aws_fis_start_experiment", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::MissingArgument { .. }),
            "expected MissingArgument, got: {err:?}"
        );
        assert!(err.to_string().contains("experiment_template_id"));
    }

    #[tokio::test]
    async fn missing_instance_id_is_typed_error_before_network() {
        let err = CloudExecutor
            .execute("aws_ec2_stop_instance", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("instance_id"));
    }

    #[tokio::test]
    async fn azure_missing_subscription_is_typed_error_before_network() {
        let args = NativeArgs::from([("experiment_id".into(), serde_json::json!("exp"))]);
        let err = CloudExecutor
            .execute("azure_chaos_start", &args)
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("subscription"));
    }

    #[tokio::test]
    async fn azure_missing_resource_group_is_typed_error_for_all_functions() {
        let args = NativeArgs::from([
            ("subscription".into(), serde_json::json!("sub1")),
            ("experiment_id".into(), serde_json::json!("exp1")),
        ]);
        for function in [
            "azure_chaos_start",
            "azure_chaos_cancel",
            "azure_chaos_status",
        ] {
            let err = CloudExecutor.execute(function, &args).await.unwrap_err();
            assert!(
                matches!(err, NativeError::MissingArgument { .. }),
                "{function}: expected MissingArgument, got: {err:?}"
            );
            assert!(
                err.to_string().contains("resource_group"),
                "{function}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn azure_missing_experiment_id_is_typed_error_for_all_functions() {
        let args = NativeArgs::from([
            ("subscription".into(), serde_json::json!("sub1")),
            ("resource_group".into(), serde_json::json!("rg1")),
        ]);
        for function in [
            "azure_chaos_start",
            "azure_chaos_cancel",
            "azure_chaos_status",
        ] {
            let err = CloudExecutor.execute(function, &args).await.unwrap_err();
            assert!(
                matches!(err, NativeError::MissingArgument { .. }),
                "{function}: expected MissingArgument, got: {err:?}"
            );
            assert!(
                err.to_string().contains("experiment_id"),
                "{function}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn gcp_missing_project_is_typed_error_before_network() {
        let err = CloudExecutor
            .execute("gcp_compute_stop_instance", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("project"));
    }

    #[tokio::test]
    async fn gcp_missing_zone_is_typed_error_before_network() {
        let args = NativeArgs::from([("project".into(), serde_json::json!("proj"))]);
        let err = CloudExecutor
            .execute("gcp_compute_stop_instance", &args)
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("zone"));
    }

    #[tokio::test]
    async fn gcp_missing_instance_is_typed_error_before_network() {
        let args = NativeArgs::from([
            ("project".into(), serde_json::json!("proj")),
            ("zone".into(), serde_json::json!("us-central1-a")),
        ]);
        let err = CloudExecutor
            .execute("gcp_compute_stop_instance", &args)
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("instance"));
    }

    fn id_arg(id: &str) -> NativeArgs {
        NativeArgs::from([("experiment_id".into(), serde_json::json!(id))])
    }

    #[test]
    fn a_given_experiment_id_wins_over_the_started_one() {
        let started = StartedFis::default();
        started.remember(Some("EXPstarted".into()));
        assert_eq!(started.resolve(&id_arg("EXPgiven")).unwrap(), "EXPgiven");
    }

    #[test]
    fn the_started_experiment_is_used_when_no_id_is_given() {
        let started = StartedFis::default();
        started.remember(Some("EXPstarted".into()));
        assert_eq!(started.resolve(&NativeArgs::new()).unwrap(), "EXPstarted");
    }

    #[test]
    fn a_start_without_an_id_keeps_the_one_before() {
        let started = StartedFis::default();
        started.remember(Some("EXPfirst".into()));
        started.remember(None);
        assert_eq!(started.resolve(&NativeArgs::new()).unwrap(), "EXPfirst");
    }

    #[test]
    fn no_id_and_nothing_started_is_a_missing_argument() {
        let err = StartedFis::default()
            .resolve(&NativeArgs::new())
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("experiment_id"));
    }

    #[tokio::test]
    async fn fis_stop_missing_experiment_id_is_typed_error_before_network() {
        let err = CloudExecutor
            .execute("aws_fis_stop_experiment", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("experiment_id"));
    }

    #[tokio::test]
    async fn fis_status_missing_experiment_id_is_typed_error_before_network() {
        let err = CloudExecutor
            .execute("aws_fis_experiment_status", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("experiment_id"));
    }

    #[tokio::test]
    async fn ec2_terminate_missing_instance_id_is_typed_error_before_network() {
        let err = CloudExecutor
            .execute("aws_ec2_terminate_instance", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
        assert!(err.to_string().contains("instance_id"));
    }
}
