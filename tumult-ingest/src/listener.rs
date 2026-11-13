//! The gRPC listener's connection limits.
//!
//! tonic accepts every connection it is offered, and each one costs a task,
//! HTTP/2 state and buffers before a single request is authorised. The
//! listener here hands tonic at most [`ConnectionLimits::max_connections`]
//! connections at once and at most [`ConnectionLimits::max_per_peer`] from
//! one peer; a connection over either limit is closed as soon as it is
//! accepted. Everything served on the listener shares the limits: the unary
//! OTLP exports and the OTel-Arrow streams alike.
//!
//! A place under the limits is not kept for free:
//!
//! * a connection has [`ConnectionLimits::handshake_timeout`] to complete
//!   the TLS handshake (when TLS is on) and send the HTTP/2 preface. Neither
//!   tonic nor hyper puts a time limit on either, so without this one a peer
//!   that connects and says nothing would hold its place for ever;
//! * no connection lives longer than
//!   [`ConnectionLimits::max_connection_age`] plus
//!   [`ConnectionLimits::max_connection_age_grace`]; it is cut then, and
//!   the client reconnects. (tonic 0.14 sends no GOAWAY at the age when a
//!   grace is set, so the two add up to one limit; they stay two settings
//!   because that is how gRPC servers name them.) An idle peer that keeps
//!   answering pings therefore cannot hold a place for ever either;
//! * TCP keepalive is switched on for every accepted socket (tonic ignores
//!   its own `tcp_keepalive` setting for a listener it did not create), and
//!   the HTTP/2 keepalive ping of the server closes a connection whose peer
//!   stops answering.
//!
//! A peer is an IPv4 address or an IPv6 /64: one host usually has a whole
//! /64 to pick source addresses from. **Behind a proxy or load balancer
//! every client arrives from the proxy's address** and they all share one
//! peer's allowance; raise `KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER` (up to
//! `KRONIKA_GRPC_MAX_CONNECTIONS`) there and let the proxy limit its
//! clients.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv6Addr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{ready, Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Sleep;
use tonic::codegen::tokio_stream::Stream;
use tonic::transport::server::Connected;

use crate::log_guard::LogLimiter;

/// How many connections the gRPC listener keeps open, and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionLimits {
    /// Open connections in total (`KRONIKA_GRPC_MAX_CONNECTIONS`).
    pub max_connections: usize,
    /// Open connections from one peer, an IPv4 address or an IPv6 /64
    /// (`KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER`).
    pub max_per_peer: usize,
    /// How long a connection may take to complete the TLS handshake and
    /// send the HTTP/2 preface (`KRONIKA_GRPC_HANDSHAKE_TIMEOUT_SECS`).
    pub handshake_timeout: Duration,
    /// The age a connection may reach
    /// (`KRONIKA_GRPC_MAX_CONNECTION_AGE_SECS`).
    pub max_connection_age: Duration,
    /// How long after that the connection is cut, whatever is in flight
    /// (`KRONIKA_GRPC_MAX_CONNECTION_AGE_GRACE_SECS`).
    pub max_connection_age_grace: Duration,
}

/// The most connections a configuration can ask for.
const MAX_CONNECTIONS_CEILING: usize = 65_536;
/// The longest handshake timeout a configuration can ask for, in seconds.
const MAX_HANDSHAKE_TIMEOUT_SECS: usize = 300;
/// The longest connection age a configuration can ask for: a week.
const MAX_CONNECTION_AGE_SECS: usize = 7 * 24 * 60 * 60;
/// The longest grace a configuration can ask for: an hour.
const MAX_CONNECTION_AGE_GRACE_SECS: usize = 60 * 60;

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            max_per_peer: 64,
            handshake_timeout: Duration::from_secs(10),
            max_connection_age: Duration::from_secs(30 * 60),
            max_connection_age_grace: Duration::from_secs(10 * 60),
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
        let count = |variable: &str, default: usize, max: usize| {
            bounded(lookup(variable), variable, default, max)
        };
        let seconds = |variable: &str, default: Duration, max: usize| {
            let default = usize::try_from(default.as_secs()).unwrap_or(max);
            let secs = bounded(lookup(variable), variable, default, max);
            Duration::from_secs(u64::try_from(secs).unwrap_or(u64::MAX))
        };
        let max_connections = count(
            "KRONIKA_GRPC_MAX_CONNECTIONS",
            d.max_connections,
            MAX_CONNECTIONS_CEILING,
        );
        Self {
            max_connections,
            max_per_peer: count(
                "KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER",
                d.max_per_peer.min(max_connections),
                max_connections,
            ),
            handshake_timeout: seconds(
                "KRONIKA_GRPC_HANDSHAKE_TIMEOUT_SECS",
                d.handshake_timeout,
                MAX_HANDSHAKE_TIMEOUT_SECS,
            ),
            max_connection_age: seconds(
                "KRONIKA_GRPC_MAX_CONNECTION_AGE_SECS",
                d.max_connection_age,
                MAX_CONNECTION_AGE_SECS,
            ),
            max_connection_age_grace: seconds(
                "KRONIKA_GRPC_MAX_CONNECTION_AGE_GRACE_SECS",
                d.max_connection_age_grace,
                MAX_CONNECTION_AGE_GRACE_SECS,
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

/// What a per-peer share is counted by: the address itself for IPv4 (and
/// for IPv4 mapped into IPv6), the /64 for IPv6. A host is normally routed
/// a whole /64 and can connect from any address in it.
#[must_use]
pub fn peer_key(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else {
        return ip;
    };
    if let Some(v4) = v6.to_ipv4_mapped() {
        return IpAddr::V4(v4);
    }
    IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64)))
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
        let peer = peer_key(peer);
        let mut now = open.lock().unwrap_or_else(PoisonError::into_inner);
        if now.total >= limits.max_connections {
            return None;
        }
        let held = now.peers.get(&peer).copied().unwrap_or(0);
        if held >= limits.max_per_peer {
            return None;
        }
        now.peers.insert(peer, held + 1);
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

/// The 24 bytes every HTTP/2 client sends first (RFC 9113, section 3.4).
const PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// TLS record content type of application data (and, in TLS 1.3, of every
/// encrypted record).
const TLS_APPLICATION_DATA: u8 = 0x17;
/// A TLS record header: content type, version, length.
const TLS_HEADER: usize = 5;
/// Complete application-data records after which a TLS connection counts
/// as established.
const TLS_RECORDS_NEEDED: u8 = 2;

/// How far a connection has come, judged from the bytes the client sent.
///
/// Without TLS the client's first 24 bytes must be the HTTP/2 preface.
/// With TLS the listener sees records, not their contents, and counts the
/// complete application-data records: in TLS 1.3 the first is the client's
/// `Finished` and the second carries the preface; in TLS 1.2 the first
/// carries the preface and the second the SETTINGS acknowledgement every
/// client owes. Only complete records count, and every complete record is
/// checked by the TLS or HTTP/2 layer above, which closes the connection
/// on anything that is not what the protocol expects. So a peer cannot get
/// past the deadline with bytes that merely look right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Progress {
    /// Nothing has arrived.
    Start,
    /// Plaintext: this much of the preface has arrived.
    Preface(usize),
    /// TLS records are arriving.
    Tls(TlsRecords),
    /// Bytes that are neither; the layer above closes the connection, and
    /// if it does not the deadline does.
    Neither,
    Established,
}

/// Where in the TLS record stream the client is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TlsRecords {
    header: [u8; TLS_HEADER],
    /// Header bytes of the current record that have arrived.
    have: usize,
    /// Body bytes of the current record still to come.
    body: usize,
    /// Complete application-data records so far.
    application: u8,
}

impl TlsRecords {
    /// Take what `bytes` holds of the current record; the rest is returned.
    fn take<'a>(&mut self, mut bytes: &'a [u8]) -> &'a [u8] {
        if self.have < TLS_HEADER {
            let n = (TLS_HEADER - self.have).min(bytes.len());
            self.header[self.have..self.have + n].copy_from_slice(&bytes[..n]);
            self.have += n;
            bytes = &bytes[n..];
            if self.have < TLS_HEADER {
                return bytes;
            }
            self.body = usize::from(u16::from_be_bytes([self.header[3], self.header[4]]));
        }
        let n = self.body.min(bytes.len());
        self.body -= n;
        if self.body > 0 {
            return &bytes[n..];
        }
        self.have = 0;
        if self.header[0] == TLS_APPLICATION_DATA {
            self.application = self.application.saturating_add(1);
        }
        &bytes[n..]
    }
}

impl Progress {
    /// Account for `bytes` read from the client.
    fn observe(&mut self, mut bytes: &[u8]) {
        while let Some(&first) = bytes.first() {
            match self {
                Self::Established | Self::Neither => return,
                Self::Start if first == PREFACE[0] => *self = Self::Preface(0),
                Self::Start => *self = Self::Tls(TlsRecords::default()),
                Self::Preface(matched) => {
                    let n = (PREFACE.len() - *matched).min(bytes.len());
                    if bytes[..n] != PREFACE[*matched..*matched + n] {
                        *self = Self::Neither;
                        return;
                    }
                    *matched += n;
                    bytes = &bytes[n..];
                    if *matched == PREFACE.len() {
                        *self = Self::Established;
                    }
                }
                Self::Tls(records) => {
                    bytes = records.take(bytes);
                    if records.application >= TLS_RECORDS_NEEDED {
                        *self = Self::Established;
                    }
                }
            }
        }
    }
}

/// Why a connection was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cut {
    Handshake,
    Age,
}

/// The two time limits of one connection.
struct Clock {
    progress: Progress,
    /// Runs until the connection is established.
    handshake: Option<Pin<Box<Sleep>>>,
    /// The connection's age limit plus its grace.
    end: Pin<Box<Sleep>>,
    cut: Option<Cut>,
}

impl Clock {
    fn start(limits: ConnectionLimits) -> Self {
        let end = limits
            .max_connection_age
            .saturating_add(limits.max_connection_age_grace);
        Self {
            progress: Progress::Start,
            handshake: Some(Box::pin(tokio::time::sleep(limits.handshake_timeout))),
            end: Box::pin(tokio::time::sleep(end)),
            cut: None,
        }
    }

    /// Whether a limit has run out. Polling the timers here also wakes the
    /// connection's task when one does, however idle the peer is.
    fn expired(&mut self, cx: &mut Context<'_>) -> Option<Cut> {
        if self.cut.is_some() {
            return self.cut;
        }
        if self.progress == Progress::Established {
            self.handshake = None;
        }
        if let Some(handshake) = self.handshake.as_mut() {
            if handshake.as_mut().poll(cx).is_ready() {
                self.cut = Some(Cut::Handshake);
                return self.cut;
            }
        }
        if self.end.as_mut().poll(cx).is_ready() {
            self.cut = Some(Cut::Age);
        }
        self.cut
    }
}

/// An accepted connection that counts against the limits until it is
/// dropped, and that fails with [`io::ErrorKind::TimedOut`] once it has
/// outlived its handshake timeout or its age.
pub struct LimitedStream<IO = TcpStream> {
    io: IO,
    clock: Clock,
    peer: IpAddr,
    gate: Gate,
    _slot: Slot,
}

impl<IO> LimitedStream<IO> {
    /// Fail once a time limit has run out; the server drops a connection
    /// whose reads and writes fail, and the slot goes back with it.
    fn check(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        let before = self.clock.cut;
        let Some(cut) = self.clock.expired(cx) else {
            return Ok(());
        };
        if before.is_none() {
            self.report(cut);
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            match cut {
                Cut::Handshake => "connection not established in time",
                Cut::Age => "connection older than its maximum age",
            },
        ))
    }

    fn report(&self, cut: Cut) {
        let limits = self.gate.limits;
        if cut == Cut::Age {
            tracing::debug!(peer = %self.peer, "gRPC connection closed at its maximum age");
            return;
        }
        if let Some(suppressed) = self.gate.cut.allow() {
            tracing::warn!(
                peer = %self.peer,
                timeout_secs = limits.handshake_timeout.as_secs(),
                suppressed,
                "gRPC connection closed: no TLS handshake and HTTP/2 preface in time"
            );
        }
    }
}

impl<IO: Connected> Connected for LimitedStream<IO> {
    type ConnectInfo = IO::ConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.io.connect_info()
    }
}

impl<IO: AsyncRead + Unpin> AsyncRead for LimitedStream<IO> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.check(cx)?;
        let before = buf.filled().len();
        let read = Pin::new(&mut this.io).poll_read(cx, buf);
        if matches!(read, Poll::Ready(Ok(()))) {
            this.clock.progress.observe(&buf.filled()[before..]);
        }
        read
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for LimitedStream<IO> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.check(cx)?;
        Pin::new(&mut this.io).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.check(cx)?;
        Pin::new(&mut this.io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.check(cx)?;
        Pin::new(&mut this.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
}

/// The limits and the connections open under them.
#[derive(Clone)]
pub struct Gate {
    limits: ConnectionLimits,
    open: Arc<Mutex<Open>>,
    refused: Arc<LogLimiter>,
    cut: Arc<LogLimiter>,
}

impl Gate {
    #[must_use]
    pub fn new(limits: ConnectionLimits) -> Self {
        Self {
            limits,
            open: Arc::default(),
            refused: Arc::new(LogLimiter::warnings()),
            cut: Arc::new(LogLimiter::warnings()),
        }
    }

    /// Connections open now.
    #[must_use]
    pub fn open(&self) -> usize {
        let now = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        now.total
    }

    /// Take `io`, a connection from `peer`, in under the limits, or drop
    /// it. Must be called inside a tokio runtime: the time limits start
    /// here.
    pub fn admit<IO>(&self, io: IO, peer: IpAddr) -> Option<LimitedStream<IO>> {
        let Some(slot) = Slot::claim(&self.open, peer, self.limits) else {
            if let Some(suppressed) = self.refused.allow() {
                tracing::warn!(
                    %peer,
                    max_connections = self.limits.max_connections,
                    max_per_peer = self.limits.max_per_peer,
                    suppressed,
                    "gRPC connection refused: too many open connections"
                );
            }
            return None;
        };
        Some(LimitedStream {
            io,
            clock: Clock::start(self.limits),
            peer,
            gate: self.clone(),
            _slot: slot,
        })
    }
}

/// The connections of `listener` that fit under `limits`, for
/// `serve_with_incoming`.
pub struct Incoming {
    listener: TcpListener,
    gate: Gate,
}

/// Accept from `listener` under `limits`.
#[must_use]
pub fn incoming(listener: TcpListener, limits: ConnectionLimits) -> Incoming {
    Incoming {
        listener,
        gate: Gate::new(limits),
    }
}

/// What tonic's own listener would set on an accepted socket, had it
/// created the listener: no Nagle delay, and TCP keepalive so a peer that
/// vanished without a FIN or RST is noticed even while the connection is
/// idle.
fn prepare(io: &TcpStream) {
    if let Err(e) = io.set_nodelay(true) {
        tracing::debug!(error = %e, "could not set TCP_NODELAY");
    }
    let keepalive = socket2::TcpKeepalive::new().with_time(crate::grpc::TCP_KEEPALIVE);
    if let Err(e) = socket2::SockRef::from(io).set_tcp_keepalive(&keepalive) {
        tracing::debug!(error = %e, "could not set TCP keepalive");
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
            let Some(admitted) = self.gate.admit(io, peer.ip()) else {
                continue;
            };
            prepare(&admitted.io);
            return Poll::Ready(Some(Ok(admitted)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;
    use tokio::time::Instant;
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

    fn counts(max_connections: usize, max_per_peer: usize) -> ConnectionLimits {
        ConnectionLimits {
            max_connections,
            max_per_peer,
            ..ConnectionLimits::default()
        }
    }

    #[tokio::test]
    async fn connections_over_the_total_are_closed_and_slots_come_back() {
        let (addr, mut accepted) = listen(counts(2, 2)).await;
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
        let (addr, mut accepted) = listen(counts(10, 1)).await;
        let mut first = TcpStream::connect(addr).await.unwrap();
        let _held = accepted.recv().await.unwrap();
        let mut second = TcpStream::connect(addr).await.unwrap();
        assert!(closed(&mut second).await, "second connection stayed open");
        assert!(!closed(&mut first).await);
    }

    #[tokio::test]
    async fn refused_connections_are_logged_at_a_bounded_rate() {
        let (logs, _guard) = crate::test_logs::capture();
        let (addr, mut accepted) = listen(counts(1, 1)).await;
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

    /// tonic sets keepalive only on sockets of a listener it created
    /// itself; an accepted socket must have it all the same.
    #[tokio::test]
    async fn accepted_sockets_have_tcp_keepalive_and_no_delay() {
        let (addr, mut accepted) = listen(ConnectionLimits::default()).await;
        let _client = TcpStream::connect(addr).await.unwrap();
        let held = accepted.recv().await.unwrap();
        let socket = socket2::SockRef::from(&held.io);
        assert!(socket.keepalive().unwrap(), "SO_KEEPALIVE is off");
        assert_eq!(
            socket.tcp_keepalive_time().unwrap(),
            crate::grpc::TCP_KEEPALIVE
        );
        assert!(held.io.nodelay().unwrap());
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

    #[test]
    fn time_limits_come_from_the_environment_within_their_maxima() {
        let from = |value: &'static str| {
            ConnectionLimits::from_lookup(move |name| {
                name.ends_with("_SECS").then(|| value.to_owned())
            })
        };
        let secs = |l: ConnectionLimits| {
            (
                l.handshake_timeout.as_secs(),
                l.max_connection_age.as_secs(),
                l.max_connection_age_grace.as_secs(),
            )
        };
        assert_eq!(secs(ConnectionLimits::default()), (10, 1800, 600));
        assert_eq!(secs(from("7")), (7, 7, 7));
        // Zero would switch a limit off; it is not accepted.
        assert_eq!(secs(from("0")), (10, 1800, 600));
        assert_eq!(secs(from("soon")), (10, 1800, 600));
        assert_eq!(secs(from("99999999999999999999")), (10, 1800, 600));
        assert_eq!(secs(from("99999999999")), (300, 604_800, 3600));
    }

    fn v6(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn an_ipv6_peer_is_its_64_and_an_ipv4_peer_its_address() {
        assert_eq!(peer_key(v6("2001:db8:1:2::1")), v6("2001:db8:1:2::"));
        assert_eq!(
            peer_key(v6("2001:db8:1:2:ffff:ffff:ffff:9")),
            v6("2001:db8:1:2::")
        );
        assert_ne!(peer_key(v6("2001:db8:1:3::1")), v6("2001:db8:1:2::"));
        assert_eq!(peer_key(v6("192.0.2.7")), v6("192.0.2.7"));
        assert_eq!(peer_key(v6("::ffff:192.0.2.7")), v6("192.0.2.7"));
    }

    /// Two addresses of one /64 are one peer: with room for one connection
    /// per peer the second is refused, a neighbouring /64 is not.
    #[tokio::test]
    async fn addresses_in_one_64_share_one_allowance() {
        let gate = Gate::new(counts(10, 1));
        let admit = |peer: &str| gate.admit(tokio::io::empty(), v6(peer));
        let first = admit("2001:db8:1:2::1");
        assert!(first.is_some());
        assert!(
            admit("2001:db8:1:2:ffff::9").is_none(),
            "another address of the same /64 got a second allowance"
        );
        let _other = admit("2001:db8:1:3::1").expect("a different /64 is another peer");
        let _mapped = admit("::ffff:192.0.2.7").expect("an IPv4 peer of its own");
        assert!(admit("192.0.2.7").is_none(), "mapped and plain IPv4 differ");
        assert_eq!(gate.open(), 3);

        drop(first);
        assert!(
            admit("2001:db8:1:2:ffff::9").is_some(),
            "place not returned"
        );
    }

    const PEER: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));

    /// Admit an in-memory connection and treat it as a server does: read
    /// until the client ends it or a read fails, then drop it. The task
    /// yields how the connection ended.
    fn served(gate: &Gate) -> (DuplexStream, JoinHandle<io::Result<()>>) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let mut stream = gate.admit(server, PEER).expect("under the limits");
        let task = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while stream.read(&mut buf).await? > 0 {}
            Ok(())
        });
        (client, task)
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// The connection is cut with `TimedOut` after `expected` of (paused)
    /// time.
    async fn cut_after(started: Instant, task: JoinHandle<io::Result<()>>, expected: Duration) {
        // A day of paused time: a connection nothing cuts fails the test
        // instead of hanging it.
        let ended = tokio::time::timeout(secs(24 * 60 * 60), task).await;
        let error = ended
            .expect("the connection was never cut")
            .unwrap()
            .expect_err("the connection was not cut");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        let took = started.elapsed();
        assert!(
            took >= expected && took < expected + secs(1),
            "cut after {took:?}, expected {expected:?}"
        );
    }

    /// [`cut_after`], and the connection's slot is free again.
    async fn assert_cut_after(
        gate: &Gate,
        started: Instant,
        task: JoinHandle<io::Result<()>>,
        expected: Duration,
    ) {
        cut_after(started, task, expected).await;
        assert_eq!(gate.open(), 0, "the slot did not come back");
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_connection_is_cut_at_the_deadline_and_its_slot_returns() {
        let gate = Gate::new(ConnectionLimits::default());
        let started = Instant::now();
        let (mut client, task) = served(&gate);
        assert_eq!(gate.open(), 1);
        assert_cut_after(&gate, started, task, secs(10)).await;
        // The client sees the connection end.
        assert_eq!(client.read(&mut [0u8; 1]).await.unwrap(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn many_silent_connections_do_not_keep_the_listener_full() {
        let gate = Gate::new(counts(8, 8));
        let started = Instant::now();
        let held: Vec<_> = (0..8).map(|_| served(&gate)).collect();
        assert!(gate.admit(tokio::io::empty(), PEER).is_none(), "full");
        for (_client, task) in held {
            cut_after(started, task, secs(10)).await;
        }
        assert_eq!(gate.open(), 0);
        assert!(gate.admit(tokio::io::empty(), PEER).is_some());
    }

    /// One byte a second never completes the preface in time.
    #[tokio::test(start_paused = true)]
    async fn a_client_that_dribbles_the_preface_is_cut_at_the_deadline() {
        let gate = Gate::new(ConnectionLimits::default());
        let started = Instant::now();
        let (mut client, task) = served(&gate);
        let dribble = tokio::spawn(async move {
            for byte in PREFACE {
                if client.write_all(&[*byte]).await.is_err() {
                    return;
                }
                tokio::time::sleep(secs(1)).await;
            }
        });
        assert_cut_after(&gate, started, task, secs(10)).await;
        dribble.await.unwrap();
    }

    /// Slow is fine as long as the preface is complete before the
    /// deadline; after that the handshake deadline no longer applies.
    #[tokio::test(start_paused = true)]
    async fn a_slow_client_that_completes_the_preface_in_time_is_not_cut() {
        let gate = Gate::new(ConnectionLimits::default());
        let (mut client, task) = served(&gate);
        for piece in PREFACE.chunks(8) {
            tokio::time::sleep(secs(3)).await;
            client.write_all(piece).await.unwrap();
        }
        tokio::time::sleep(secs(600)).await;
        assert!(!task.is_finished(), "an established connection was cut");
        assert_eq!(gate.open(), 1);

        drop(client);
        task.await
            .unwrap()
            .expect("ended by the client, not by a cut");
        assert_eq!(gate.open(), 0);
    }

    /// Twenty-four bytes are not enough; they must be the preface.
    #[tokio::test(start_paused = true)]
    async fn bytes_that_are_not_the_preface_do_not_stop_the_deadline() {
        let gate = Gate::new(ConnectionLimits::default());
        let started = Instant::now();
        let (mut client, task) = served(&gate);
        client
            .write_all(b"PRI * HTTP/1.1\r\n\r\nSM\r\n\r\n and more to follow")
            .await
            .unwrap();
        assert_cut_after(&gate, started, task, secs(10)).await;
    }

    fn tls_record(content_type: u8, len: usize) -> Vec<u8> {
        let mut record = vec![content_type, 3, 3];
        record.extend_from_slice(&u16::try_from(len).unwrap().to_be_bytes());
        record.resize(TLS_HEADER + len, 0xab);
        record
    }

    /// A `ClientHello` and then nothing: the TLS handshake never finishes.
    #[tokio::test(start_paused = true)]
    async fn a_tls_client_that_stops_after_its_hello_is_cut_at_the_deadline() {
        let gate = Gate::new(ConnectionLimits::default());
        let started = Instant::now();
        let (mut client, task) = served(&gate);
        client.write_all(&tls_record(0x16, 512)).await.unwrap();
        assert_cut_after(&gate, started, task, secs(10)).await;
    }

    /// One encrypted record is the client's `Finished` in TLS 1.3: the
    /// handshake is done but no HTTP/2 preface has been sent.
    #[tokio::test(start_paused = true)]
    async fn a_tls_client_that_stops_after_the_handshake_is_cut_at_the_deadline() {
        let gate = Gate::new(ConnectionLimits::default());
        let started = Instant::now();
        let (mut client, task) = served(&gate);
        let mut flight = tls_record(0x16, 512);
        flight.extend(tls_record(0x14, 1));
        flight.extend(tls_record(TLS_APPLICATION_DATA, 53));
        // The start of a second record does not count either.
        flight.extend(&tls_record(TLS_APPLICATION_DATA, 60)[..40]);
        client.write_all(&flight).await.unwrap();
        assert_cut_after(&gate, started, task, secs(10)).await;
    }

    /// The TLS 1.3 client flight (hello, compatibility change-cipher-spec,
    /// `Finished`, then the preface), arriving in pieces that do not line
    /// up with the records.
    #[tokio::test(start_paused = true)]
    async fn a_tls_client_that_sends_the_preface_record_is_not_cut() {
        let gate = Gate::new(ConnectionLimits::default());
        let (mut client, task) = served(&gate);
        let mut flight = tls_record(0x16, 512);
        flight.extend(tls_record(0x14, 1));
        flight.extend(tls_record(TLS_APPLICATION_DATA, 53));
        flight.extend(tls_record(TLS_APPLICATION_DATA, 60));
        for piece in flight.chunks(7) {
            client.write_all(piece).await.unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(secs(600)).await;
        assert!(!task.is_finished(), "an established TLS connection was cut");
        assert_eq!(gate.open(), 1);
    }

    /// What is still open when age and grace are over is cut, however
    /// lively the peer.
    #[tokio::test(start_paused = true)]
    async fn an_established_connection_is_cut_when_its_age_and_grace_are_over() {
        let limits = ConnectionLimits {
            max_connection_age: secs(100),
            max_connection_age_grace: secs(20),
            ..ConnectionLimits::default()
        };
        let gate = Gate::new(limits);
        let started = Instant::now();
        let (mut client, task) = served(&gate);
        client.write_all(PREFACE).await.unwrap();
        tokio::time::sleep(secs(119)).await;
        assert!(!task.is_finished(), "cut before the grace was over");
        assert_cut_after(&gate, started, task, secs(120)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_cut_connection_also_refuses_writes() {
        let gate = Gate::new(ConnectionLimits::default());
        let (_client, server) = tokio::io::duplex(1024);
        let mut stream = gate.admit(server, PEER).unwrap();
        tokio::time::sleep(secs(11)).await;
        let error = stream.write_all(b"settings").await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let error = stream.flush().await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn cut_connections_are_logged_at_a_bounded_rate_with_the_peer() {
        let (logs, _guard) = crate::test_logs::capture();
        let gate = Gate::new(ConnectionLimits::default());
        let started = Instant::now();
        let held: Vec<_> = (0..40).map(|_| served(&gate)).collect();
        for (_client, task) in held {
            cut_after(started, task, secs(10)).await;
        }
        let text = logs.text();
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("gRPC connection closed"))
            .collect();
        assert!((1..=10).contains(&lines.len()), "{} lines", lines.len());
        assert!(lines[0].contains("192.0.2.1"), "{}", lines[0]);
        assert!(lines[0].contains("timeout_secs=10"), "{}", lines[0]);
    }
}
