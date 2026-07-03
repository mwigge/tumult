//! Native dispatch for the `tumult-net` TCP chaos-proxy plugin.
//!
//! Routes `net.<function>` calls to `tumult_net::actions::*`, parsing the
//! JSON arguments into the socket addresses, timing, rate, and probability
//! parameters each userspace fault needs. Kept in a sibling module so
//! `exec.rs` stays focused; as a child module it reuses the private
//! `arg_str` / `arg_num` helpers from its parent.

use super::{arg_num, arg_str};

/// Dispatch to tumult-net TCP chaos-proxy functions.
pub(super) async fn dispatch_net(
    function: &str,
    args: &std::collections::HashMap<String, serde_json::Value>,
) -> Result<String, String> {
    let listen = arg_str(args, "listen")?
        .parse::<std::net::SocketAddr>()
        .map_err(|e| format!("invalid listen address: {e}"))?;

    // The rollback only needs the listen address.
    if function == "stop_proxy" {
        return tumult_net::actions::stop_proxy(listen)
            .await
            .map_err(|e| format!("{e}"));
    }

    let upstream = arg_str(args, "upstream")?
        .parse::<std::net::SocketAddr>()
        .map_err(|e| format!("invalid upstream address: {e}"))?;
    let seed = u64::from(arg_num::<u32>(args, "seed").unwrap_or(0));
    let as_usize =
        |key: &str| usize::try_from(arg_num::<u32>(args, key).unwrap_or(0)).unwrap_or(usize::MAX);
    let as_prob = |key: &str| {
        args.get(key)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
    };

    match function {
        "inject_latency" => {
            let delay_ms = u64::from(arg_num::<u32>(args, "delay_ms").unwrap_or(0));
            let jitter_ms = u64::from(arg_num::<u32>(args, "jitter_ms").unwrap_or(0));
            tumult_net::actions::inject_latency(listen, upstream, delay_ms, jitter_ms, seed)
                .await
                .map_err(|e| format!("{e}"))
        }
        "throttle_bandwidth" => {
            tumult_net::actions::throttle_bandwidth(listen, upstream, as_usize("rate_bps"))
                .await
                .map_err(|e| format!("{e}"))
        }
        "fragment_stream" => {
            tumult_net::actions::fragment_stream(listen, upstream, as_usize("slice_bytes"))
                .await
                .map_err(|e| format!("{e}"))
        }
        "corrupt_bytes" => {
            tumult_net::actions::corrupt_bytes(listen, upstream, as_prob("probability"), seed)
                .await
                .map_err(|e| format!("{e}"))
        }
        "terminate_connections" => tumult_net::actions::terminate_connections(
            listen,
            upstream,
            as_prob("probability"),
            seed,
        )
        .await
        .map_err(|e| format!("{e}")),
        "start_proxy" => {
            let profile = tumult_net::FaultProfile {
                delay_ms: u64::from(arg_num::<u32>(args, "delay_ms").unwrap_or(0)),
                jitter_ms: u64::from(arg_num::<u32>(args, "jitter_ms").unwrap_or(0)),
                rate_bps: as_usize("rate_bps"),
                slice_bytes: as_usize("slice_bytes"),
                corrupt_prob: as_prob("corrupt_prob"),
                terminate_prob: as_prob("terminate_prob"),
                seed,
            };
            tumult_net::actions::start_proxy(listen, upstream, profile)
                .await
                .map_err(|e| format!("{e}"))
        }
        _ => Err(format!("unknown tumult-net function: {function}")),
    }
}
