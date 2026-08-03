//! Native dispatch — [`NativeExecutor`] implementation for `tumult-net`.
//!
//! Routes `tumult-net.<function>` calls to [`crate::actions`], parsing the
//! JSON arguments into the socket addresses, timing, rate, and probability
//! parameters each userspace fault needs.

use std::net::{SocketAddr, ToSocketAddrs};

use tumult_plugin::native::{arg_num, arg_str, NativeArgs, NativeError, NativeExecutor};

use crate::actions;
use crate::config::FaultProfile;

/// Functions `tumult-net` provides to the experiment runner.
const FUNCTIONS: &[&str] = &[
    "start_proxy",
    "stop_proxy",
    "inject_latency",
    "throttle_bandwidth",
    "fragment_stream",
    "corrupt_bytes",
    "terminate_connections",
];

/// [`NativeExecutor`] for the `tumult-net` TCP chaos-proxy plugin.
pub struct NetExecutor;

#[async_trait::async_trait(?Send)]
impl NativeExecutor for NetExecutor {
    fn name(&self) -> &'static str {
        "tumult-net"
    }

    fn functions(&self) -> &'static [&'static str] {
        FUNCTIONS
    }

    async fn execute(&self, function: &str, args: &NativeArgs) -> Result<String, NativeError> {
        // Validate the function name before touching arguments, so typos
        // fail with the available-function list instead of argument errors.
        if !FUNCTIONS.contains(&function) {
            return Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            ));
        }

        let listen = arg_addr(args, "listen")?;

        // The rollback only needs the listen address.
        if function == "stop_proxy" {
            return actions::stop_proxy(listen)
                .await
                .map_err(NativeError::execution);
        }

        let upstream = arg_addr(args, "upstream")?;
        let seed = u64::from(arg_num::<u32>(args, "seed").unwrap_or(0));
        let as_usize = |key: &str| {
            usize::try_from(arg_num::<u32>(args, key).unwrap_or(0)).unwrap_or(usize::MAX)
        };
        let as_prob = |key: &str| {
            args.get(key)
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0)
        };

        match function {
            "inject_latency" => {
                let delay_ms = u64::from(arg_num::<u32>(args, "delay_ms").unwrap_or(0));
                let jitter_ms = u64::from(arg_num::<u32>(args, "jitter_ms").unwrap_or(0));
                actions::inject_latency(listen, upstream, delay_ms, jitter_ms, seed)
                    .await
                    .map_err(NativeError::execution)
            }
            "throttle_bandwidth" => {
                actions::throttle_bandwidth(listen, upstream, as_usize("rate_bps"))
                    .await
                    .map_err(NativeError::execution)
            }
            "fragment_stream" => {
                actions::fragment_stream(listen, upstream, as_usize("slice_bytes"))
                    .await
                    .map_err(NativeError::execution)
            }
            "corrupt_bytes" => {
                actions::corrupt_bytes(listen, upstream, as_prob("probability"), seed)
                    .await
                    .map_err(NativeError::execution)
            }
            "terminate_connections" => {
                actions::terminate_connections(listen, upstream, as_prob("probability"), seed)
                    .await
                    .map_err(NativeError::execution)
            }
            "start_proxy" => {
                let profile = FaultProfile {
                    delay_ms: u64::from(arg_num::<u32>(args, "delay_ms").unwrap_or(0)),
                    jitter_ms: u64::from(arg_num::<u32>(args, "jitter_ms").unwrap_or(0)),
                    rate_bps: as_usize("rate_bps"),
                    slice_bytes: as_usize("slice_bytes"),
                    corrupt_prob: as_prob("corrupt_prob"),
                    terminate_prob: as_prob("terminate_prob"),
                    seed,
                };
                actions::start_proxy(listen, upstream, profile)
                    .await
                    .map_err(NativeError::execution)
            }
            _ => Err(NativeError::unknown_function(
                self.name(),
                function,
                FUNCTIONS,
            )),
        }
    }
}

/// Extract a socket-address argument.
fn arg_addr(args: &NativeArgs, key: &str) -> Result<SocketAddr, NativeError> {
    // Resolve via `to_socket_addrs` rather than a bare `SocketAddr` parse so a
    // DNS name works as well as a literal IP — `upstream: demo-app:8080` is the
    // norm on container/Kubernetes networks, not `10.0.0.5:8080`. A literal
    // `host:port` still resolves through this path unchanged.
    let value = arg_str(args, key)?;
    value
        .to_socket_addrs()
        .map_err(|e| NativeError::invalid_argument(key, e.to_string()))?
        .next()
        .ok_or_else(|| NativeError::invalid_argument(key, "resolved to no addresses"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_names_plugin_and_functions() {
        let executor = NetExecutor;
        assert_eq!(executor.name(), "tumult-net");
        assert!(executor.functions().contains(&"start_proxy"));
        assert!(executor.functions().contains(&"stop_proxy"));
    }

    #[tokio::test]
    async fn unknown_function_is_rejected() {
        let args = NativeArgs::from([("listen".into(), serde_json::json!("127.0.0.1:19999"))]);
        let err = NetExecutor.execute("drop_all", &args).await.unwrap_err();
        assert!(
            matches!(err, NativeError::UnknownFunction { .. }),
            "expected UnknownFunction, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn invalid_listen_address_is_typed_error() {
        let args = NativeArgs::from([("listen".into(), serde_json::json!("not-an-addr"))]);
        let err = NetExecutor
            .execute("inject_latency", &args)
            .await
            .unwrap_err();
        assert!(
            matches!(err, NativeError::InvalidArgument { .. }),
            "expected InvalidArgument, got: {err:?}"
        );
        assert!(err.to_string().contains("listen"));
    }

    #[tokio::test]
    async fn missing_listen_address_is_typed_error() {
        let err = NetExecutor
            .execute("stop_proxy", &NativeArgs::new())
            .await
            .unwrap_err();
        assert!(matches!(err, NativeError::MissingArgument { .. }));
    }

    #[tokio::test]
    async fn stop_proxy_without_running_proxy_is_idempotent() {
        let args = NativeArgs::from([("listen".into(), serde_json::json!("127.0.0.1:59997"))]);
        let output = NetExecutor.execute("stop_proxy", &args).await.unwrap();
        assert!(output.contains("no chaos proxy running"));
    }

    /// listen == upstream with every optional argument present: each arm's
    /// argument parsing runs, then the action's proxy-loop guard rejects the
    /// call deterministically without any proxyd daemon.
    fn proxy_loop_args() -> NativeArgs {
        NativeArgs::from([
            ("listen".into(), serde_json::json!("127.0.0.1:64123")),
            ("upstream".into(), serde_json::json!("127.0.0.1:64123")),
            ("delay_ms".into(), serde_json::json!(10)),
            ("jitter_ms".into(), serde_json::json!(5)),
            ("rate_bps".into(), serde_json::json!(1024)),
            ("slice_bytes".into(), serde_json::json!(64)),
            ("probability".into(), serde_json::json!(0.5)),
            ("corrupt_prob".into(), serde_json::json!(0.1)),
            ("terminate_prob".into(), serde_json::json!(0.2)),
            ("seed".into(), serde_json::json!(42)),
        ])
    }

    #[tokio::test]
    async fn every_fault_arm_rejects_a_proxy_loop() {
        for function in [
            "start_proxy",
            "inject_latency",
            "throttle_bandwidth",
            "fragment_stream",
            "corrupt_bytes",
            "terminate_connections",
        ] {
            let err = NetExecutor
                .execute(function, &proxy_loop_args())
                .await
                .unwrap_err();
            assert!(
                matches!(err, NativeError::Execution { .. }),
                "{function}: expected Execution, got: {err:?}"
            );
            assert!(
                err.to_string().contains("proxy loop"),
                "{function}: err: {err}"
            );
        }
    }

    #[tokio::test]
    async fn invalid_upstream_address_is_typed_error() {
        let args = NativeArgs::from([
            ("listen".into(), serde_json::json!("127.0.0.1:64124")),
            ("upstream".into(), serde_json::json!("not-an-addr")),
        ]);
        let err = NetExecutor.execute("start_proxy", &args).await.unwrap_err();
        assert!(
            matches!(err, NativeError::InvalidArgument { .. }),
            "expected InvalidArgument, got: {err:?}"
        );
        assert!(err.to_string().contains("upstream"));
    }
}
