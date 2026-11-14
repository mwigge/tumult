//! The gRPC listener's connection limits.
//!
//! tonic accepts every connection it is offered, and each one costs a task,
//! HTTP/2 state and buffers before a single request is authorised. The
//! listener here hands tonic at most [`ConnectionLimits::max_connections`]
//! connections at once, at most [`ConnectionLimits::max_per_peer`] from
//! one peer and at most [`ConnectionLimits::max_per_ipv6_site`] from one
//! IPv6 /48. Everything served on the listener shares the limits: the
//! unary OTLP exports and the OTel-Arrow streams alike.
//!
//! A place under the limits is not kept for free:
//!
//! * a connection has [`ConnectionLimits::handshake_timeout`] to complete
//!   the TLS handshake (when TLS is on) and send the HTTP/2 preface. The
//!   TLS handshake is done here, not by tonic, so the deadline ends when
//!   rustls says the handshake is complete and the 24 bytes of the
//!   preface have been read inside it, not when bytes that look like
//!   either have gone by;
//! * when an ingest token is configured ([`Access::Token`]), a connection
//!   has [`ConnectionLimits::auth_timeout`] from the moment it is
//!   accepted to make a call that carries the token. Until it has, it is
//!   the one that goes when a newcomer finds no room: the oldest such
//!   connection gives its place up, for the listener as a whole, for a
//!   peer and for a site alike. A connection that has made an authorised
//!   call is never put out for one that has not;
//! * no connection lives longer than
//!   [`ConnectionLimits::max_connection_age`] plus
//!   [`ConnectionLimits::max_connection_age_grace`]; it is cut then, and
//!   the client reconnects. (tonic 0.14 sends no GOAWAY at the age when a
//!   grace is set, so the two add up to one limit; they stay two settings
//!   because that is how gRPC servers name them.);
//! * TCP keepalive is switched on for every accepted socket (tonic ignores
//!   its own `tcp_keepalive` setting for a listener it did not create), and
//!   the HTTP/2 keepalive ping of the server closes a connection whose peer
//!   stops answering.
//!
//! **Without an ingest token** ([`Access::Open`]) nothing tells a client
//! from someone who merely connected: no connection waits to
//! authenticate, none is put out, and a newcomer that finds no room is
//! closed as soon as it is accepted. Whoever can reach the listener from
//! `max_connections / max_per_peer` addresses (16 with the defaults) can
//! then hold every place for as long as age and grace allow by answering
//! pings. Set `KRONIKA_INGEST_TOKEN` on a listener that strangers can
//! reach.
//!
//! A peer is an IPv4 address or an IPv6 /64: one host usually has a whole
//! /64 to pick source addresses from. A site is an IPv6 /48, the block a
//! provider usually gives one customer, who would otherwise have 65 536
//! peers' allowances. A link-local IPv6 address (`fe80::/10`) is a peer
//! by itself together with the interface it is on, and all of one link is
//! one site. **Behind a proxy or load balancer every client arrives from
//! the proxy's address** and they all share one peer's allowance; raise
//! `KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER` (up to
//! `KRONIKA_GRPC_MAX_CONNECTIONS`) there and let the proxy limit its
//! clients.

mod limits;
mod registry;
mod stream;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

use std::future::{poll_fn, Future};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time::Sleep;
use tokio_rustls::rustls;
use tokio_rustls::TlsAcceptor;
use tonic::codegen::tokio_stream::Stream;

use crate::log_guard::{self, LogLimiter};
use registry::Conn;
use stream::{Guard, MaybeTls};

pub use limits::ConnectionLimits;
pub use registry::{peer_key, Access, Gate};
pub use stream::LimitedStream;

/// The TLS settings of the listener; see [`tls_config`].
pub type TlsConfig = rustls::ServerConfig;

/// How long the listener leaves the socket alone after an accept error
/// that is not about one connection. Running out of file descriptors is
/// the usual one, and accepting again at once fails again at once.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(200);

/// The TLS settings for the certificate chain and private key in
/// `cert_pem` and `key_pem`: no client certificates, HTTP/2 only.
///
/// # Errors
///
/// When the PEM holds no certificate or no key, or the key does not fit
/// the certificate.
pub fn tls_config(cert_pem: &[u8], key_pem: &[u8]) -> io::Result<Arc<TlsConfig>> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    fn invalid(what: &str, why: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, format!("{what}: {why}"))
    }
    let chain = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| invalid("TLS certificate chain", e))?;
    if chain.is_empty() {
        return Err(invalid("TLS certificate chain", "no certificate in it"));
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| invalid("TLS private key", e))?;
    // Both rustls providers are compiled into the workspace; name one
    // rather than depend on a process-wide default.
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = TlsConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| invalid("TLS protocol versions", e))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| invalid("TLS certificate and key", e))?;
    config.alpn_protocols.push(b"h2".to_vec());
    Ok(Arc::new(config))
}

/// What a call's request says about the connection it arrived on, when
/// the connection came through [`Incoming`].
#[derive(Clone)]
pub struct ConnInfo {
    conn: Arc<Conn>,
}

impl ConnInfo {
    fn of(conn: Arc<Conn>) -> Self {
        Self { conn }
    }

    /// Where the connection came from.
    #[must_use]
    pub fn remote_addr(&self) -> SocketAddr {
        self.conn.remote()
    }

    /// The connection made an authorised call: it keeps its place.
    pub(crate) fn authenticated(&self) {
        self.conn.authenticate();
    }
}

/// Where `request` came from: its connection through [`Incoming`], or
/// what tonic knows of a connection it accepted itself.
pub(crate) fn remote_addr<T>(request: &tonic::Request<T>) -> Option<SocketAddr> {
    match request.extensions().get::<ConnInfo>() {
        Some(info) => Some(info.remote_addr()),
        None => request.remote_addr(),
    }
}

/// `request` was authorised: its connection keeps its place from now on.
pub(crate) fn authenticated<T>(request: &tonic::Request<T>) {
    if let Some(info) = request.extensions().get::<ConnInfo>() {
        info.authenticated();
    }
}

impl Gate {
    /// A place and the time limits for a connection from `remote`.
    fn enter(&self, remote: SocketAddr) -> Option<Guard> {
        let conn = self.claim(remote)?;
        Some(Guard::start(self, conn))
    }

    /// Take `io`, a connection from `remote` that speaks HTTP/2 without
    /// TLS, in under the limits, or drop it. Must be called inside a
    /// tokio runtime: the time limits start here.
    pub fn admit<IO>(&self, io: IO, remote: SocketAddr) -> Option<LimitedStream<IO>> {
        let guard = self.enter(remote)?;
        Some(LimitedStream::new(MaybeTls::Plain(io), guard))
    }
}

/// Where [`Incoming`] gets its connections from: a TCP listener, or a
/// test's in-memory one.
pub trait Accept {
    /// An accepted connection.
    type Io: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// The next connection and where it came from.
    fn poll_accept(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<(Self::Io, SocketAddr)>>;
}

impl Accept for TcpListener {
    type Io = TcpStream;

    fn poll_accept(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<(TcpStream, SocketAddr)>> {
        let accepted = ready!(TcpListener::poll_accept(self, cx));
        if let Ok((io, _)) = &accepted {
            prepare(io);
        }
        Poll::Ready(accepted)
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

/// The connections of a listener that fit under the limits, for
/// `serve_with_incoming`. It never yields an error: tonic's accept loop
/// asks again at once after one, which with no file descriptors left is a
/// busy loop.
pub struct Incoming<A: Accept = TcpListener> {
    acceptor: A,
    gate: Gate,
    tls: Option<TlsAcceptor>,
    /// TLS handshakes in progress; each holds its connection's place.
    handshakes: JoinSet<Option<LimitedStream<A::Io>>>,
    /// Set after an accept error: no accept until it has run out.
    backoff: Option<Pin<Box<Sleep>>>,
    errors: LogLimiter,
}

/// Accept from `listener` under `limits`, without TLS.
#[must_use]
pub fn incoming(listener: TcpListener, limits: ConnectionLimits, access: Access) -> Incoming {
    Incoming::over(listener, limits, access)
}

impl<A: Accept> Incoming<A> {
    /// Accept from `acceptor` under `limits`, without TLS.
    #[must_use]
    pub fn over(acceptor: A, limits: ConnectionLimits, access: Access) -> Self {
        Self {
            acceptor,
            gate: Gate::new(limits, access),
            tls: None,
            handshakes: JoinSet::new(),
            backoff: None,
            errors: LogLimiter::warnings(),
        }
    }

    /// Do the TLS handshake of `config` on every connection before it is
    /// handed on.
    #[must_use]
    pub fn with_tls(mut self, config: Arc<TlsConfig>) -> Self {
        self.tls = Some(TlsAcceptor::from(config));
        self
    }

    /// The limits and the connections open under them.
    #[must_use]
    pub fn gate(&self) -> Gate {
        self.gate.clone()
    }

    /// A handshake that has completed, if any has.
    fn poll_handshaken(&mut self, cx: &mut Context<'_>) -> Option<LimitedStream<A::Io>> {
        while let Poll::Ready(Some(done)) = self.handshakes.poll_join_next(cx) {
            match done {
                Ok(Some(stream)) => return Some(stream),
                Ok(None) => {}
                Err(e) => tracing::error!(error = %e, "TLS handshake task failed"),
            }
        }
        None
    }

    /// Whether accepting is on hold after an error.
    fn backing_off(&mut self, cx: &mut Context<'_>) -> bool {
        let Some(backoff) = self.backoff.as_mut() else {
            return false;
        };
        if backoff.as_mut().poll(cx).is_pending() {
            return true;
        }
        self.backoff = None;
        false
    }

    fn accept_failed(&mut self, error: &io::Error) {
        use io::ErrorKind::{ConnectionAborted, ConnectionRefused, ConnectionReset};
        // About the one connection, which is gone; the next is fine.
        if matches!(
            error.kind(),
            ConnectionAborted | ConnectionRefused | ConnectionReset
        ) {
            return;
        }
        self.backoff = Some(Box::pin(tokio::time::sleep(ACCEPT_BACKOFF)));
        let Some(suppressed) = self.errors.allow("accept") else {
            return;
        };
        tracing::warn!(
            error = %error,
            backoff_ms = ACCEPT_BACKOFF.as_millis(),
            suppressed,
            "gRPC listener cannot accept connections, backing off"
        );
    }
}

/// The TLS handshake of one admitted connection, under its time limits.
async fn handshake<IO>(tls: TlsAcceptor, io: IO, mut guard: Guard) -> Option<LimitedStream<IO>>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    let mut accept = tls.accept(io);
    let done = poll_fn(|cx| {
        guard.check(cx)?;
        Pin::new(&mut accept).poll(cx)
    })
    .await;
    match done {
        Ok(stream) => Some(LimitedStream::new(MaybeTls::Tls(Box::new(stream)), guard)),
        Err(e) => {
            tracing::debug!(
                peer = %guard.conn().remote().ip(),
                error = %log_guard::clean_display(&e),
                "TLS handshake failed"
            );
            None
        }
    }
}

impl<A: Accept + Unpin> Stream for Incoming<A> {
    type Item = io::Result<LimitedStream<A::Io>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(stream) = this.poll_handshaken(cx) {
                return Poll::Ready(Some(Ok(stream)));
            }
            if this.backing_off(cx) {
                return Poll::Pending;
            }
            let (io, remote) = match ready!(this.acceptor.poll_accept(cx)) {
                Ok(accepted) => accepted,
                Err(e) => {
                    this.accept_failed(&e);
                    continue;
                }
            };
            let Some(guard) = this.gate.enter(remote) else {
                continue;
            };
            let Some(tls) = this.tls.clone() else {
                let stream = LimitedStream::new(MaybeTls::Plain(io), guard);
                return Poll::Ready(Some(Ok(stream)));
            };
            this.handshakes.spawn(handshake(tls, io, guard));
        }
    }
}
