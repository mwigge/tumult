//! `tumult-net-proxyd` — the detached TCP chaos-proxy daemon.
//!
//! The `tumult-net` actions spawn this binary so a proxy fault outlives the
//! short-lived CLI invocation that created it. It reconstructs its
//! configuration from the `--flag value` argv written by
//! [`tumult_net::config::ProxySpec::to_argv`], then forwards traffic until the
//! rollback action kills the process.

use std::process::ExitCode;

use tumult_net::config::ProxySpec;
use tumult_net::proxy::Proxy;
use tumult_otel::logging::{self, err, LogOptions};

/// `service.name` of the daemon's log events.
const SERVICE_NAME: &str = "tumult-net-proxyd";

#[tokio::main]
async fn main() -> ExitCode {
    // stderr is the startup log the spawning action reads back when the
    // daemon dies on arrival, so failures must be events on it.
    if let Err(error) = logging::init(LogOptions::new(SERVICE_NAME)) {
        logging::report_init_failure(SERVICE_NAME, &error);
        return ExitCode::FAILURE;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();

    let spec = match ProxySpec::from_argv(&args) {
        Ok(spec) => spec,
        Err(e) => {
            tracing::error!(
                name: "net.proxyd.args.invalid",
                { error.message = %err(&e) },
                "invalid arguments"
            );
            return ExitCode::FAILURE;
        }
    };

    let proxy = Proxy::new(spec.listen, spec.upstream, spec.profile);
    if let Err(e) = proxy.run().await {
        tracing::error!(
            name: "net.proxyd.failed",
            {
                server.address = %spec.listen.ip(),
                server.port = spec.listen.port(),
                error.message = %err(&e),
            },
            "chaos proxy failed"
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
