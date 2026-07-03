//! The in-process TCP chaos proxy engine.
//!
//! [`Proxy`] binds a downstream listener, dials the upstream for every accepted
//! connection, and forwards both directions concurrently. Because the
//! `tokio-netem` adapters are directional, each socket is split with
//! [`TcpStream::into_split`] and the egress (write) half of each direction is
//! wrapped with the fault stack from [`crate::faults::wrap_writer`]. This is the
//! `copy_bidirectional` pattern expanded into two independently faulted
//! `tokio::io::copy` pipes driven under [`tokio::try_join`].

use std::net::SocketAddr;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use crate::config::FaultProfile;
use crate::error::NetError;
use crate::faults::wrap_writer;
use crate::handles::FaultHandles;

/// A faulted TCP forwarding proxy.
pub struct Proxy {
    listen: SocketAddr,
    upstream: SocketAddr,
    profile: FaultProfile,
}

impl Proxy {
    /// Create a proxy that forwards `listen` → `upstream` applying `profile`.
    #[must_use]
    pub fn new(listen: SocketAddr, upstream: SocketAddr, profile: FaultProfile) -> Self {
        Self {
            listen,
            upstream,
            profile,
        }
    }

    /// Bind the downstream listener, returning it so callers (and tests) can
    /// read back the OS-assigned port when binding to port 0.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] if the bind fails.
    pub async fn bind(&self) -> Result<TcpListener, NetError> {
        Ok(TcpListener::bind(self.listen).await?)
    }

    /// Accept and forward connections forever on an already-bound `listener`.
    ///
    /// Each accepted connection is handled on its own task, so a single slow or
    /// terminated connection never blocks the accept loop. The future only
    /// resolves on an unrecoverable accept error.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Io`] if `listener.accept()` fails fatally, or
    /// [`NetError`] if the fault handles cannot be built from the profile.
    pub async fn serve(&self, listener: &TcpListener) -> Result<(), NetError> {
        let upstream = self.upstream;
        let terminate_prob = self.profile.terminate_prob;
        let seed = self.profile.seed_bytes();

        loop {
            let (downstream, _peer) = listener.accept().await?;
            // Fresh handle set per connection so retuning one connection never
            // perturbs another, and per-connection state stays isolated.
            let handles = FaultHandles::from_profile(&self.profile)?;
            tokio::spawn(async move {
                if let Err(e) =
                    forward(downstream, upstream, handles, terminate_prob, seed).await
                {
                    tracing::debug!(error = %e, "proxied connection closed with error");
                }
            });
        }
    }

    /// Bind and serve in one call.
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] if binding or serving fails.
    pub async fn run(&self) -> Result<(), NetError> {
        let listener = self.bind().await?;
        self.serve(&listener).await
    }
}

/// Dial the upstream and pump both directions until either side closes.
async fn forward(
    downstream: TcpStream,
    upstream_addr: SocketAddr,
    handles: FaultHandles,
    terminate_prob: f64,
    seed: [u8; 32],
) -> Result<(), NetError> {
    let upstream = TcpStream::connect(upstream_addr).await?;

    let (mut down_r, down_w) = downstream.into_split();
    let (mut up_r, up_w) = upstream.into_split();

    let mut to_upstream = wrap_writer(up_w, &handles, terminate_prob, seed);
    let mut to_downstream = wrap_writer(down_w, &handles, terminate_prob, seed);

    let client_to_server = async {
        let n = tokio::io::copy(&mut down_r, &mut to_upstream).await?;
        to_upstream.shutdown().await.ok();
        Ok::<u64, std::io::Error>(n)
    };
    let server_to_client = async {
        let n = tokio::io::copy(&mut up_r, &mut to_downstream).await?;
        to_downstream.shutdown().await.ok();
        Ok::<u64, std::io::Error>(n)
    };

    tokio::try_join!(client_to_server, server_to_client)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Spawn a loopback echo server; return its address.
    async fn spawn_echo() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    loop {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if sock.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    async fn spawn_proxy(upstream: SocketAddr, profile: FaultProfile) -> SocketAddr {
        let proxy = Proxy::new("127.0.0.1:0".parse().unwrap(), upstream, profile);
        let listener = proxy.bind().await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = proxy.serve(&listener).await;
        });
        addr
    }

    #[tokio::test]
    async fn proxy_forwards_bytes_end_to_end() {
        let echo = spawn_echo().await;
        let proxy = spawn_proxy(echo, FaultProfile::default()).await;

        let mut client = TcpStream::connect(proxy).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[tokio::test]
    async fn latency_fault_delays_the_round_trip() {
        let echo = spawn_echo().await;
        let profile = FaultProfile {
            delay_ms: 200,
            ..FaultProfile::default()
        };
        let proxy = spawn_proxy(echo, profile).await;

        let mut client = TcpStream::connect(proxy).await.unwrap();
        let start = Instant::now();
        client.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(&buf, b"ping");
        // Delay is applied on both egress halves; a single one-way delay is a
        // safe lower bound that avoids scheduler-timing flakiness.
        assert!(
            elapsed >= Duration::from_millis(180),
            "round trip {elapsed:?} was not delayed"
        );
    }

    #[tokio::test]
    async fn fragmentation_fault_preserves_payload() {
        let echo = spawn_echo().await;
        let profile = FaultProfile {
            slice_bytes: 1,
            ..FaultProfile::default()
        };
        let proxy = spawn_proxy(echo, profile).await;

        let mut client = TcpStream::connect(proxy).await.unwrap();
        let payload = b"fragmented-payload";
        client.write_all(payload).await.unwrap();
        let mut buf = vec![0u8; payload.len()];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, payload);
    }
}
