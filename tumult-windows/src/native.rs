//! Native dispatch — [`NativeExecutor`] implementation for `tumult-windows`.
//!
//! Routes `tumult-windows.<function>` calls to [`crate::faults`], parsing the
//! JSON activity arguments into the process, CPU, and firewall parameters each
//! fault needs. Argument validation is host-independent and happens here; the
//! Windows-specific effects happen in [`crate::faults`] when the underlying
//! tools are actually present.

use std::time::Duration;

use tumult_plugin::native::{arg_num, NativeArgs, NativeError, NativeExecutor};

use crate::error::WindowsError;
use crate::{commands::BlackholeTarget, faults};

/// Functions `tumult-windows` provides to the experiment runner.
const FUNCTIONS: &[&str] = &["process_kill", "cpu_stress", "network_blackhole"];

/// Default CPU-stress duration, in seconds, when `duration_secs` is absent.
const DEFAULT_CPU_DURATION_SECS: u64 = 10;

/// [`NativeExecutor`] for the `tumult-windows` native-fault plugin.
pub struct WindowsExecutor;

/// Read an optional string argument, returning `None` when absent or non-string.
fn opt_str<'a>(args: &'a NativeArgs, key: &str) -> Option<&'a str> {
    args.get(key).and_then(serde_json::Value::as_str)
}

/// Convert a crate [`WindowsError`] into the plugin-facing [`NativeError`].
///
/// Argument problems stay typed as `InvalidArgument`; a non-zero tool exit is a
/// completed-but-failed run (`Failed`); an un-spawnable tool is an execution
/// error carrying the source.
fn to_native(err: WindowsError) -> NativeError {
    match err {
        WindowsError::InvalidArgument { argument, reason } => {
            NativeError::InvalidArgument { argument, reason }
        }
        WindowsError::CommandFailed { .. } => NativeError::Failed(err.to_string()),
        WindowsError::Spawn { .. } => NativeError::execution(err),
    }
}

#[async_trait::async_trait(?Send)]
impl NativeExecutor for WindowsExecutor {
    fn name(&self) -> &'static str {
        "tumult-windows"
    }

    fn functions(&self) -> &'static [&'static str] {
        FUNCTIONS
    }

    async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError> {
        // Validate the function name before touching arguments, so typos fail
        // with the available-function list instead of argument errors.
        if !FUNCTIONS.contains(&function) {
            return Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            ));
        }

        match function {
            "process_kill" => {
                let image = opt_str(args, "image");
                let pid = arg_num::<u32>(args, "pid");
                let report = faults::process_kill(image, pid).map_err(to_native)?;
                Ok(report.to_json().to_string())
            }
            "cpu_stress" => {
                let requested = arg_num::<u32>(args, "workers")
                    .and_then(|w| usize::try_from(w).ok())
                    .unwrap_or_else(faults::default_workers);
                // Clamp to a sane maximum — an unbounded worker count would
                // spawn that many busy-spin threads on the target host.
                let max = faults::max_workers();
                let clamped = requested > max;
                let workers = requested.min(max);
                let duration_secs = arg_num::<u32>(args, "duration_secs")
                    .map_or(DEFAULT_CPU_DURATION_SECS, u64::from);
                let report = faults::cpu_stress(workers, Duration::from_secs(duration_secs));
                let mut json = report.to_json();
                if clamped {
                    json["warning"] = serde_json::json!(format!(
                        "requested {requested} workers exceeds the maximum {max} \
                         (4x logical CPUs, hard cap 256); clamped to {workers}"
                    ));
                }
                Ok(json.to_string())
            }
            "network_blackhole" => {
                let port = arg_num::<u16>(args, "port");
                let remote_host = opt_str(args, "remote_host");
                let target = BlackholeTarget::from_args(port, remote_host).map_err(to_native)?;
                let report = faults::network_blackhole(&target).map_err(to_native)?;
                Ok(report.to_json().to_string())
            }
            _ => Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_names_plugin_and_functions() {
        let executor = WindowsExecutor;
        assert_eq!(executor.name(), "tumult-windows");
        assert_eq!(
            executor.functions(),
            &["process_kill", "cpu_stress", "network_blackhole"]
        );
    }

    #[tokio::test]
    async fn unknown_function_is_rejected() {
        let err = WindowsExecutor
            .execute("reboot", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::UnknownFunction { .. }),
            "expected UnknownFunction, got: {err:?}"
        );
        assert!(err.to_string().contains("process_kill"));
    }

    #[tokio::test]
    async fn process_kill_without_selector_is_invalid_argument() {
        let err = WindowsExecutor
            .execute("process_kill", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::InvalidArgument { .. }),
            "expected InvalidArgument, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn network_blackhole_without_target_is_invalid_argument() {
        let err = WindowsExecutor
            .execute("network_blackhole", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::InvalidArgument { .. }));
    }

    #[tokio::test]
    async fn cpu_stress_runs_and_reports_json() {
        // cpu_stress is pure Rust, so it executes on the Linux test host. Use a
        // short duration to keep the test fast.
        let args = NativeArgs::from([
            ("workers".into(), serde_json::json!(2)),
            ("duration_secs".into(), serde_json::json!(0)),
        ]);
        let output = WindowsExecutor.execute("cpu_stress", &args).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["workers"], 2);
        assert!(value.get("warning").is_none(), "no clamp, no warning");
    }

    #[tokio::test]
    async fn cpu_stress_clamps_absurd_worker_counts_with_a_warning() {
        let args = NativeArgs::from([
            ("workers".into(), serde_json::json!(u32::MAX)),
            ("duration_secs".into(), serde_json::json!(0)),
        ]);
        let output = WindowsExecutor.execute("cpu_stress", &args).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let max = faults::max_workers();
        assert_eq!(value["workers"], max, "workers must be clamped to the max");
        assert!(
            value["warning"]
                .as_str()
                .unwrap_or_default()
                .contains("clamped"),
            "a clamp must surface a warning: {output}"
        );
    }

    #[tokio::test]
    async fn process_kill_on_non_windows_surfaces_execution_error() {
        if cfg!(not(windows)) {
            let args = NativeArgs::from([("image".into(), serde_json::json!("notepad.exe"))]);
            let err = WindowsExecutor
                .execute("process_kill", &args)
                .await
                .unwrap_err();
            // taskkill cannot spawn on Linux → Execution, not a panic.
            assert!(
                matches!(err, NativeError::Execution { .. }),
                "expected Execution, got: {err:?}"
            );
        }
    }
}
