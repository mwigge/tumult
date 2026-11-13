//! The gRPC listener's connection limits.
//!
//! tonic accepts every connection it is offered, and each one costs a task,
//! HTTP/2 state and buffers before a single request is authorised. The
//! listener here hands tonic at most [`ConnectionLimits::max_connections`]
//! connections at once and at most [`ConnectionLimits::max_per_peer`] from
//! one IP address; a connection over either limit is closed as soon as it is
//! accepted. Everything served on the listener shares the limits: the unary
//! OTLP exports and the OTel-Arrow streams alike.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{ready, Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tonic::codegen::tokio_stream::Stream;
use tonic::transport::server::{Connected, TcpConnectInfo};

use crate::log_guard::LogLimiter;

/// How many connections the gRPC listener keeps open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionLimits {
    /// Open connections in total (`KRONIKA_GRPC_MAX_CONNECTIONS`).
    pub max_connections: usize,
    /// Open connections from one IP address
    /// (`KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER`).
    pub max_per_peer: usize,
}

/// The most connections a configuration can ask for.
const MAX_CONNECTIONS_CEILING: usize = 65_536;

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            max_per_peer: 64,
        }
    }
}

impl ConnectionLimits {
    /// The defaults overridden by the `KRONIKA_GRPC_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] with the variable lookup injected.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let max_connections = bounded(
            lookup("KRONIKA_GRPC_MAX_CONNECTIONS"),
            "KRONIKA_GRPC_MAX_CONNECTIONS",
            d.max_connections,
            MAX_CONNECTIONS_CEILING,
        );
        Self {
            max_connections,
            max_per_peer: bounded(
                lookup("KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER"),
                "KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER",
                d.max_per_peer.min(max_connections),
                max_connections,
            ),
        }
    }
}

/// A positive integer of at most `max`, or `default` when `raw` is not one.
fn bounded(raw: Option<String>, variable: &str, default: usize, max: usize) -> usize {
    let Some(raw) = raw else {
        return default;
    };
    match raw.trim().parse::<usize>() {
        Ok(v) if v > max => {
            tracing::warn!(
                variable,
                max,
                "connection limit above its maximum, using the maximum"
            );
            max
        }
        Ok(v) if v > 0 => v,
        _ => {
            tracing::warn!(
                variable,
                default,
                "ignoring connection limit that is not a positive integer"
            );
            default
        }
    }
}

/// The connections that are open now.
#[derive(Default)]
struct Open {
    total: usize,
    peers: HashMap<IpAddr, usize>,
}

/// One connection's place under the limits; given back on drop.
struct Slot {
    open: Arc<Mutex<Open>>,
    peer: IpAddr,
}

impl Slot {
    /// A place for one more connection from `peer`, unless a limit is
    /// reached.
    fn claim(open: &Arc<Mutex<Open>>, peer: IpAddr, limits: ConnectionLimits) -> Option<Self> {
        let mut now = open.lock().unwrap_or_else(PoisonError::into_inner);
        if now.total >= limits.max_connections {
            return None;
        }
        let count = now.peers.entry(peer).or_insert(0);
        if *count >= limits.max_per_peer {
            return None;
        }
        *count += 1;
        now.total += 1;
        Some(Self {
            open: open.clone(),
            peer,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut now = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        now.total = now.total.saturating_sub(1);
        let left = now.peers.get(&self.peer).map_or(0, |n| n.saturating_sub(1));
        if left == 0 {
            now.peers.remove(&self.peer);
            return;
        }
        now.peers.insert(self.peer, left);
    }
}

/// An accepted connection that counts against the limits until it is
/// dropped.
pub struct LimitedStream {
    io: TcpStream,
    _slot: Slot,
}

impl Connected for LimitedStream {
    type ConnectInfo = TcpConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.io.connect_info()
    }
}

impl AsyncRead for LimitedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for LimitedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
}

/// The connections of `listener` that fit under `limits`, for
/// `serve_with_incoming`.
pub struct Incoming {
    listener: TcpListener,
    limits: ConnectionLimits,
    open: Arc<Mutex<Open>>,
    refused: LogLimiter,
}

/// Accept from `listener` under `limits`.
#[must_use]
pub fn incoming(listener: TcpListener, limits: ConnectionLimits) -> Incoming {
    Incoming {
        listener,
        limits,
        open: Arc::default(),
        refused: LogLimiter::warnings(),
    }
}

impl Incoming {
    /// Take `io` in under the limits, or close it.
    fn admit(&self, io: TcpStream, peer: SocketAddr) -> Option<LimitedStream> {
        let Some(slot) = Slot::claim(&self.open, peer.ip(), self.limits) else {
            if let Some(suppressed) = self.refused.allow() {
                tracing::warn!(
                    peer = %peer.ip(),
                    max_connections = self.limits.max_connections,
                    max_per_peer = self.limits.max_per_peer,
                    suppressed,
                    "gRPC connection refused: too many open connections"
                );
            }
            return None;
        };
        // What tonic's own listener sets.
        if let Err(e) = io.set_nodelay(true) {
            tracing::debug!(error = %e, "could not set TCP_NODELAY");
        }
        Some(LimitedStream { io, _slot: slot })
    }
}

impl Stream for Incoming {
    type Item = io::Result<LimitedStream>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let (io, peer) = match ready!(self.listener.poll_accept(cx)) {
                Ok(accepted) => accepted,
                Err(e) => return Poll::Ready(Some(Err(e))),
            };
            if let Some(admitted) = self.admit(io, peer) {
                return Poll::Ready(Some(Ok(admitted)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::AsyncReadExt;
    use tokio::sync::mpsc;
    use tonic::codegen::tokio_stream::StreamExt;

    use super::*;

    /// A listener under `limits` whose accepted connections come out of the
    /// returned channel (and stay open until dropped).
    async fn listen(limits: ConnectionLimits) -> (SocketAddr, mpsc::Receiver<LimitedStream>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            let mut incoming = incoming(listener, limits);
            while let Some(Ok(io)) = incoming.next().await {
                if tx.send(io).await.is_err() {
                    return;
                }
            }
        });
        (addr, rx)
    }

    /// Whether the server closed `client` (as opposed to leaving it open).
    async fn closed(client: &mut TcpStream) -> bool {
        let mut byte = [0u8; 1];
        match tokio::time::timeout(Duration::from_millis(300), client.read(&mut byte)).await {
            Ok(Ok(0) | Err(_)) => true,
            Ok(Ok(_)) => panic!("the server sent data"),
            Err(_) => false,
        }
    }

    #[tokio::test]
    async fn connections_over_the_total_are_closed_and_slots_come_back() {
        let limits = ConnectionLimits {
            max_connections: 2,
            max_per_peer: 2,
        };
        let (addr, mut accepted) = listen(limits).await;
        let mut first = TcpStream::connect(addr).await.unwrap();
        let mut second = TcpStream::connect(addr).await.unwrap();
        let held_first = accepted.recv().await.unwrap();
        let _held_second = accepted.recv().await.unwrap();
        let mut third = TcpStream::connect(addr).await.unwrap();
        assert!(closed(&mut third).await, "third connection stayed open");
        assert!(!closed(&mut first).await);
        assert!(!closed(&mut second).await);

        // A connection that ends gives its place to the next one.
        drop(held_first);
        let mut fourth = TcpStream::connect(addr).await.unwrap();
        let _held_fourth = accepted.recv().await.unwrap();
        assert!(!closed(&mut fourth).await, "freed slot was not reused");
    }

    #[tokio::test]
    async fn one_peer_cannot_take_every_connection() {
        let limits = ConnectionLimits {
            max_connections: 10,
            max_per_peer: 1,
        };
        let (addr, mut accepted) = listen(limits).await;
        let mut first = TcpStream::connect(addr).await.unwrap();
        let _held = accepted.recv().await.unwrap();
        let mut second = TcpStream::connect(addr).await.unwrap();
        assert!(closed(&mut second).await, "second connection stayed open");
        assert!(!closed(&mut first).await);
    }

    #[tokio::test]
    async fn refused_connections_are_logged_at_a_bounded_rate() {
        let limits = ConnectionLimits {
            max_connections: 1,
            max_per_peer: 1,
        };
        let (logs, _guard) = crate::test_logs::capture();
        let (addr, mut accepted) = listen(limits).await;
        let _first = TcpStream::connect(addr).await.unwrap();
        let _held = accepted.recv().await.unwrap();
        for _ in 0..40 {
            let mut extra = TcpStream::connect(addr).await.unwrap();
            assert!(closed(&mut extra).await);
        }
        let lines = logs
            .text()
            .lines()
            .filter(|l| l.contains("gRPC connection refused"))
            .count();
        assert!((1..=10).contains(&lines), "{lines} lines for 40 refusals");
    }

    #[test]
    fn limits_come_from_the_environment_within_their_maxima() {
        let from = |total: Option<&str>, peer: Option<&str>| {
            let (total, peer) = (total.map(str::to_owned), peer.map(str::to_owned));
            ConnectionLimits::from_lookup(move |name| match name {
                "KRONIKA_GRPC_MAX_CONNECTIONS" => total.clone(),
                "KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER" => peer.clone(),
                _ => None,
            })
        };
        assert_eq!(from(None, None), ConnectionLimits::default());
        let set = from(Some("200"), Some("8"));
        assert_eq!((set.max_connections, set.max_per_peer), (200, 8));
        // Not positive integers: the defaults.
        assert_eq!(from(Some("0"), Some("-1")), ConnectionLimits::default());
        // Above the ceiling, and a per-peer share above the total.
        let big = from(Some("99999999999"), Some("99999999999"));
        assert_eq!(big.max_connections, MAX_CONNECTIONS_CEILING);
        assert_eq!(big.max_per_peer, MAX_CONNECTIONS_CEILING);
        let small = from(Some("4"), None);
        assert_eq!((small.max_connections, small.max_per_peer), (4, 4));
        assert_eq!(from(Some("4"), Some("9")).max_per_peer, 4);
    }
}
