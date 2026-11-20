//! `tumult-net-proxyd` — the detached TCP chaos-proxy daemon.
//!
//! The `tumult-net` actions spawn this binary so a proxy fault outlives the
//! short-lived CLI invocation that created it. It reconstructs its
//! configuration from the `--flag value` argv written by
//! [`tumult_net::config::ProxySpec::to_argv`], binds its listen address,
//! records its pid in the pidfile, then forwards traffic until the rollback
//! action kills the process.

use std::net::SocketAddr;
use std::process::ExitCode;

use tumult_net::config::ProxySpec;
use tumult_net::pidfile;
use tumult_net::proxy::Proxy;
use tumult_net::NetError;
use tumult_otel::logging::{self, err};

/// `service.name` of the daemon's log events.
const SERVICE_NAME: &str = "tumult-net-proxyd";

#[tokio::main]
async fn main() -> ExitCode {
    // stderr is the log file the spawning action reads back when the daemon
    // dies on arrival, so failures must be events on it. Nothing rotates
    // that file, so what goes to it is limited.
    if let Err(error) = logging::init(tumult_net::daemon_log_options(SERVICE_NAME)) {
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
    let listener = match proxy.bind().await {
        Ok(listener) => listener,
        Err(e) => return failed(spec.listen, &e),
    };
    // Recorded only once the address is bound: a daemon that could not
    // bind leaves the pidfile of the proxy that did alone.
    if let Err(e) = pidfile::write(&spec.pidfile, std::process::id()) {
        tracing::error!(
            name: "net.proxyd.pidfile.failed",
            {
                file.path = %spec.pidfile.display(),
                error.type = "io",
                error.message = %err(&e),
            },
            "the pid could not be recorded"
        );
        return ExitCode::FAILURE;
    }
    if let Err(e) = proxy.serve(&listener).await {
        return failed(spec.listen, &e);
    }
    ExitCode::SUCCESS
}

/// Logs why the proxy on `listen` failed, as the event the spawning action
/// reads back.
fn failed(listen: SocketAddr, error: &NetError) -> ExitCode {
    tracing::error!(
        name: "net.proxyd.failed",
        {
            server.address = %listen.ip(),
            server.port = listen.port(),
            error.message = %err(error),
        },
        "chaos proxy failed"
    );
    ExitCode::FAILURE
}
