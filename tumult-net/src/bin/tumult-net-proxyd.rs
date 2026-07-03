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

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let spec = match ProxySpec::from_argv(&args) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("tumult-net-proxyd: {e}");
            return ExitCode::FAILURE;
        }
    };

    let proxy = Proxy::new(spec.listen, spec.upstream, spec.profile);
    if let Err(e) = proxy.run().await {
        eprintln!("tumult-net-proxyd: proxy error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
