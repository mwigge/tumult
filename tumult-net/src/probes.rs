//! Read-only network probes used to validate steady state.

use std::time::{Duration, Instant};

use tokio::net::TcpStream;

use crate::error::NetError;

/// Default connect timeout for reachability and latency probes.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Report whether a TCP `host:port` accepts a connection within the timeout.
///
/// A refused, timed-out, or unresolvable endpoint is reported as `Ok(false)`
/// rather than an error, so the probe composes cleanly with tolerance checks.
///
/// # Errors
///
/// Returns [`NetError`] only for unexpected local failures; ordinary
/// unreachability is surfaced as `Ok(false)`.
#[tracing::instrument]
#[must_use = "a reachability probe result must be checked against a tolerance"]
pub async fn reachable(host: &str, port: u16) -> Result<bool, NetError> {
    let _span = crate::telemetry::begin_reachable(host, port);
    let target = format!("{host}:{port}");
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target.as_str())).await {
        Ok(Ok(_stream)) => Ok(true),
        // Connection refused / reset / DNS failure — the endpoint is not reachable.
        Ok(Err(_)) | Err(_) => Ok(false),
    }
}

/// Measure the TCP handshake latency to `host:port` in milliseconds.
///
/// # Errors
///
/// Returns [`NetError::Io`] if the connection cannot be established (refused,
/// timed out, or unresolvable) within the timeout.
#[tracing::instrument]
#[must_use = "a latency probe result must be checked against a tolerance"]
pub async fn measured_latency(host: &str, port: u16) -> Result<f64, NetError> {
    let _span = crate::telemetry::begin_measured_latency(host, port);
    let target = format!("{host}:{port}");
    let start = Instant::now();
    let connect = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target.as_str())).await;
    match connect {
        Ok(Ok(_stream)) => Ok(start.elapsed().as_secs_f64() * 1000.0),
        Ok(Err(e)) => Err(NetError::Io(e)),
        Err(_elapsed) => Err(NetError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("connect to {target} timed out after {CONNECT_TIMEOUT:?}"),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn reachable_is_true_for_a_live_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(reachable("127.0.0.1", addr.port()).await.unwrap());
    }

    #[tokio::test]
    async fn reachable_is_false_for_a_closed_port() {
        // Bind then drop to obtain an almost-certainly-free port.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(!reachable("127.0.0.1", port).await.unwrap());
    }

    #[tokio::test]
    async fn measured_latency_returns_a_finite_value() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ms = measured_latency("127.0.0.1", addr.port()).await.unwrap();
        assert!(ms.is_finite() && ms >= 0.0);
    }
}
