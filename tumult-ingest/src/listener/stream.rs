//! One admitted connection: its place under the limits, its time limits
//! and the bytes that go through it.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::Sleep;
use tokio_rustls::server::TlsStream;
use tonic::transport::server::Connected;

use super::registry::{Conn, Gate};
use super::ConnInfo;

/// The 24 bytes every HTTP/2 client sends first (RFC 9113, section 3.4).
pub(super) const PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// How much of the HTTP/2 preface the client has sent. With TLS these
/// are the bytes inside it: nothing is read here before the handshake is
/// complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Progress {
    /// This much of the preface has arrived.
    Preface(usize),
    /// Bytes that are not the preface; the layer above closes the
    /// connection, and if it does not the deadline does.
    Neither,
    Established,
}

impl Progress {
    /// Account for `bytes` read from the client.
    fn observe(&mut self, bytes: &[u8]) {
        let Self::Preface(matched) = *self else {
            return;
        };
        let n = (PREFACE.len() - matched).min(bytes.len());
        if bytes[..n] != PREFACE[matched..matched + n] {
            *self = Self::Neither;
            return;
        }
        *self = match matched + n {
            done if done == PREFACE.len() => Self::Established,
            more => Self::Preface(more),
        };
    }
}

/// Why a connection was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cut {
    /// No TLS handshake and HTTP/2 preface in time.
    Handshake,
    /// No authorised call in time.
    Unauthenticated,
    /// Its place went to a newcomer before it made an authorised call.
    PutOut,
    Age,
}

impl Cut {
    fn error(self) -> io::Error {
        match self {
            Self::Handshake => timed_out("connection not established in time"),
            Self::Unauthenticated => timed_out("no authorised call in time"),
            Self::Age => timed_out("connection older than its maximum age"),
            Self::PutOut => io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "place given to a newer connection",
            ),
        }
    }
}

fn timed_out(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, why)
}

type Timer = Pin<Box<Sleep>>;

fn timer(after: std::time::Duration) -> Timer {
    Box::pin(tokio::time::sleep(after))
}

/// A connection's place under the limits, given back on drop, and the
/// time limits it has to keep while it holds the place.
pub(super) struct Guard {
    gate: Gate,
    conn: Arc<Conn>,
    /// Runs until the TLS handshake is complete and the preface is in.
    handshake: Option<Timer>,
    /// Runs until the first authorised call; not started when no token
    /// is configured.
    auth: Option<Timer>,
    /// The connection's age limit plus its grace.
    end: Timer,
    cut: Option<Cut>,
}

impl Guard {
    /// Start the time limits of `conn`. Must be called inside a tokio
    /// runtime.
    pub(super) fn start(gate: &Gate, conn: Arc<Conn>) -> Self {
        let limits = gate.limits();
        let end = limits
            .max_connection_age
            .saturating_add(limits.max_connection_age_grace);
        Self {
            gate: gate.clone(),
            handshake: Some(timer(limits.handshake_timeout)),
            auth: (!conn.is_authenticated()).then(|| timer(limits.auth_timeout)),
            end: timer(end),
            cut: None,
            conn,
        }
    }

    pub(super) fn conn(&self) -> &Arc<Conn> {
        &self.conn
    }

    /// Fail once a limit has run out or the place is gone; whoever
    /// drives the connection drops it when its reads and writes fail, and
    /// the place goes back with it. Polling the timers here also wakes
    /// the connection's task when one runs out, however idle the peer.
    pub(super) fn check(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        if let Some(cut) = self.cut {
            return Err(cut.error());
        }
        let Some(cut) = self.expired(cx) else {
            return Ok(());
        };
        self.cut = Some(cut);
        self.report(cut);
        Err(cut.error())
    }

    fn expired(&mut self, cx: &mut Context<'_>) -> Option<Cut> {
        if self.auth.is_some() && self.conn.is_authenticated() {
            self.auth = None;
        }
        if self.auth.is_some() {
            // Only a connection that has not authenticated is put out.
            // Watch first, look second: a newcomer that takes the place
            // in between finds the waker.
            self.conn.watch(cx.waker());
            if self.conn.is_evicted() {
                return Some(Cut::PutOut);
            }
        }
        if ran_out(self.handshake.as_mut(), cx) {
            return Some(Cut::Handshake);
        }
        if ran_out(self.auth.as_mut(), cx) {
            return Some(Cut::Unauthenticated);
        }
        ran_out(Some(&mut self.end), cx).then_some(Cut::Age)
    }

    fn report(&self, cut: Cut) {
        let limits = self.gate.limits();
        let peer = self.conn.remote().ip();
        let (reason, timeout, message) = match cut {
            Cut::Age => {
                tracing::debug!(%peer, "gRPC connection closed at its maximum age");
                return;
            }
            // Logged by the gate, which knows the newcomer.
            Cut::PutOut => return,
            Cut::Handshake => (
                "handshake",
                limits.handshake_timeout,
                "gRPC connection closed: no TLS handshake and HTTP/2 preface in time",
            ),
            Cut::Unauthenticated => (
                "unauthenticated",
                limits.auth_timeout,
                "gRPC connection closed: no authorised call in time",
            ),
        };
        let Some(suppressed) = self.gate.may_log_cut(reason) else {
            return;
        };
        tracing::warn!(
            %peer,
            timeout_secs = timeout.as_secs(),
            suppressed,
            "{message}"
        );
    }
}

fn ran_out(timer: Option<&mut Timer>, cx: &mut Context<'_>) -> bool {
    timer.is_some_and(|timer| timer.as_mut().poll(cx).is_ready())
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.gate.release(&self.conn);
    }
}

/// The bytes of a connection: as accepted, or inside TLS.
pub(super) enum MaybeTls<IO> {
    Plain(IO),
    Tls(Box<TlsStream<IO>>),
}

impl<IO: AsyncRead + AsyncWrite + Unpin> AsyncRead for MaybeTls<IO> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_read(cx, buf),
            Self::Tls(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}

impl<IO: AsyncRead + AsyncWrite + Unpin> AsyncWrite for MaybeTls<IO> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_write(cx, buf),
            Self::Tls(io) => Pin::new(io).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_flush(cx),
            Self::Tls(io) => Pin::new(io).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_shutdown(cx),
            Self::Tls(io) => Pin::new(io).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(io) => Pin::new(io).poll_write_vectored(cx, bufs),
            Self::Tls(io) => Pin::new(io).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(io) => io.is_write_vectored(),
            Self::Tls(io) => io.is_write_vectored(),
        }
    }
}

/// An accepted connection that counts against the limits until it is
/// dropped. It fails with [`io::ErrorKind::TimedOut`] once it has
/// outlived its handshake timeout, its time to authenticate or its age,
/// and with [`io::ErrorKind::ConnectionAborted`] once its place has gone
/// to a newcomer.
pub struct LimitedStream<IO = TcpStream> {
    io: MaybeTls<IO>,
    progress: Progress,
    guard: Guard,
}

impl<IO> LimitedStream<IO> {
    pub(super) fn new(io: MaybeTls<IO>, guard: Guard) -> Self {
        Self {
            io,
            progress: Progress::Preface(0),
            guard,
        }
    }

    /// The connection as it was accepted, under TLS or not.
    #[cfg(test)]
    pub(super) fn accepted(&self) -> &IO {
        match &self.io {
            MaybeTls::Plain(io) => io,
            MaybeTls::Tls(io) => io.get_ref().0,
        }
    }

    /// Account for what the client sent; the handshake deadline is over
    /// once that is the whole preface.
    fn observe(&mut self, bytes: &[u8]) {
        if self.guard.handshake.is_none() {
            return;
        }
        self.progress.observe(bytes);
        if self.progress == Progress::Established {
            self.guard.handshake = None;
        }
    }
}

impl<IO> Connected for LimitedStream<IO> {
    type ConnectInfo = ConnInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        ConnInfo::of(self.guard.conn().clone())
    }
}

impl<IO: AsyncRead + AsyncWrite + Unpin> AsyncRead for LimitedStream<IO> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.guard.check(cx)?;
        let before = buf.filled().len();
        let read = Pin::new(&mut this.io).poll_read(cx, buf);
        if matches!(read, Poll::Ready(Ok(()))) {
            this.observe(&buf.filled()[before..]);
        }
        read
    }
}

impl<IO: AsyncRead + AsyncWrite + Unpin> AsyncWrite for LimitedStream<IO> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.guard.check(cx)?;
        Pin::new(&mut this.io).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.guard.check(cx)?;
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
        this.guard.check(cx)?;
        Pin::new(&mut this.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
}
