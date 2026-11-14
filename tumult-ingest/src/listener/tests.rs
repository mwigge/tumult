//! The listener's limits, over in-memory connections and a paused clock:
//! nothing here waits on the wall clock.

use std::future::Future;
use std::sync::atomic::Ordering;

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tonic::codegen::tokio_stream::StreamExt;
use tonic::transport::server::Connected;

use super::limits::MAX_CONNECTIONS_CEILING;
use super::stream::PREFACE;
use super::testing::{
    client_tls, dialler, from, hello_offering_early_data, server_tls, undecryptable_record, Dial,
};
use super::*;

const PEER: &str = "192.0.2.1";

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn counts(max_connections: usize, max_per_peer: usize) -> ConnectionLimits {
    ConnectionLimits {
        max_connections,
        max_per_peer,
        max_per_ipv6_site: max_connections,
        ..ConnectionLimits::default()
    }
}

/// `event` happens, and well before any deadline of the listener: a
/// second of paused time. An event that never comes fails the test
/// instead of hanging it.
async fn soon<T>(event: impl Future<Output = T>) -> T {
    tokio::time::timeout(secs(1), event)
        .await
        .expect("did not happen")
}

/// The server closed `client` without sending anything.
async fn closed_unanswered(client: &mut DuplexStream) {
    let mut seen = Vec::new();
    let end = soon(client.read_to_end(&mut seen)).await;
    assert!(end.is_ok() && seen.is_empty(), "{end:?}, {seen:?}");
}

/// A listener over in-memory connections; what it lets in comes out of
/// `accepted` and stays open until dropped.
struct Listening {
    dial: Dial,
    gate: Gate,
    accepted: mpsc::UnboundedReceiver<LimitedStream<DuplexStream>>,
    polls: Arc<std::sync::atomic::AtomicUsize>,
    task: JoinHandle<()>,
}

fn listening(limits: ConnectionLimits, access: Access, tls: bool) -> Listening {
    let (dial, dialled) = dialler();
    let polls = dialled.polls();
    let mut incoming = Incoming::over(dialled, limits, access);
    if tls {
        incoming = incoming.with_tls(server_tls());
    }
    let gate = incoming.gate();
    let (tx, accepted) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        while let Some(next) = incoming.next().await {
            let Ok(stream) = next else {
                // What tonic does with an accept error: ask again.
                tokio::task::yield_now().await;
                continue;
            };
            if tx.send(stream).is_err() {
                return;
            }
        }
    });
    Listening {
        dial,
        gate,
        accepted,
        polls,
        task,
    }
}

impl Listening {
    async fn accepted(&mut self) -> LimitedStream<DuplexStream> {
        soon(self.accepted.recv()).await.expect("listener gone")
    }
}

#[tokio::test(start_paused = true)]
async fn connections_over_the_total_are_closed_and_slots_come_back() {
    let mut l = listening(counts(2, 2), Access::Open, false);
    let _first = l.dial.connect(from(PEER));
    let _second = l.dial.connect(from(PEER));
    let held_first = l.accepted().await;
    let _held_second = l.accepted().await;
    let mut third = l.dial.connect(from(PEER));
    closed_unanswered(&mut third).await;
    assert_eq!(l.gate.open(), 2, "a refused connection took a place");

    // A connection that ends gives its place to the next one.
    drop(held_first);
    let _fourth = l.dial.connect(from(PEER));
    let _held_fourth = l.accepted().await;
    assert_eq!(l.gate.open(), 2);
}

#[tokio::test(start_paused = true)]
async fn one_peer_cannot_take_every_connection() {
    let mut l = listening(counts(10, 1), Access::Open, false);
    let _first = l.dial.connect(from(PEER));
    let _held = l.accepted().await;
    let mut second = l.dial.connect(from(PEER));
    closed_unanswered(&mut second).await;
    let _other = l.dial.connect(from("192.0.2.2"));
    let _held_other = l.accepted().await;
    assert_eq!(l.gate.open(), 2);
}

#[tokio::test(start_paused = true)]
async fn refused_connections_are_logged_at_a_bounded_rate() {
    let (logs, _guard) = crate::test_logs::capture();
    let gate = Gate::new(counts(1, 1), Access::Open);
    let _held = gate.admit(tokio::io::empty(), from(PEER)).unwrap();
    for _ in 0..40 {
        assert!(gate.admit(tokio::io::empty(), from(PEER)).is_none());
    }
    let text = logs.text();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("gRPC connection refused"))
        .collect();
    assert!((1..=10).contains(&lines.len()), "{} lines", lines.len());
    assert!(lines[0].contains("peer=192.0.2.1"), "{}", lines[0]);
}

/// tonic sets keepalive only on sockets of a listener it created
/// itself; an accepted socket must have it all the same. A real socket,
/// and no clock: the test ends when the connection is accepted.
#[tokio::test]
async fn accepted_sockets_have_tcp_keepalive_and_no_delay() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut incoming = incoming(listener, ConnectionLimits::default(), Access::Open);
    let _client = TcpStream::connect(addr).await.unwrap();
    let held = incoming.next().await.unwrap().unwrap();
    let socket = socket2::SockRef::from(held.accepted());
    assert!(socket.keepalive().unwrap(), "SO_KEEPALIVE is off");
    assert_eq!(
        socket.tcp_keepalive_time().unwrap(),
        crate::grpc::TCP_KEEPALIVE
    );
    assert!(held.accepted().nodelay().unwrap());
    assert_eq!(held.connect_info().remote_addr().ip(), addr.ip());
}

#[test]
fn limits_come_from_the_environment_within_their_maxima() {
    let from = |total: Option<&str>, peer: Option<&str>, site: Option<&str>| {
        let owned = |v: Option<&str>| v.map(str::to_owned);
        let (total, peer, site) = (owned(total), owned(peer), owned(site));
        let l = ConnectionLimits::from_lookup(move |name| match name {
            "KRONIKA_GRPC_MAX_CONNECTIONS" => total.clone(),
            "KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER" => peer.clone(),
            "KRONIKA_GRPC_MAX_CONNECTIONS_PER_IPV6_SITE" => site.clone(),
            _ => None,
        });
        (l.max_connections, l.max_per_peer, l.max_per_ipv6_site)
    };
    assert_eq!(from(None, None, None), (1024, 64, 256));
    assert_eq!(
        ConnectionLimits::from_lookup(|_| None),
        ConnectionLimits::default()
    );
    assert_eq!(from(Some("200"), Some("8"), Some("16")), (200, 8, 16));
    // A site gets four peers' allowances unless it is told otherwise,
    // and never more than there are connections.
    assert_eq!(from(Some("200"), Some("8"), None), (200, 8, 32));
    assert_eq!(from(Some("200"), Some("80"), None), (200, 80, 200));
    // Not positive integers: the defaults.
    assert_eq!(from(Some("0"), Some("-1"), Some("x")), (1024, 64, 256));
    // Above the ceiling, and shares above the total.
    let ceiling = MAX_CONNECTIONS_CEILING;
    let huge = Some("99999999999");
    assert_eq!(from(huge, huge, huge), (ceiling, ceiling, ceiling));
    assert_eq!(from(Some("4"), None, None), (4, 4, 4));
    assert_eq!(from(Some("4"), Some("9"), Some("9")), (4, 4, 4));
}

#[test]
fn time_limits_come_from_the_environment_within_their_maxima() {
    let from = |value: &'static str| {
        ConnectionLimits::from_lookup(move |name| name.ends_with("_SECS").then(|| value.to_owned()))
    };
    let secs = |l: ConnectionLimits| {
        (
            l.handshake_timeout.as_secs(),
            l.auth_timeout.as_secs(),
            l.max_connection_age.as_secs(),
            l.max_connection_age_grace.as_secs(),
        )
    };
    assert_eq!(secs(ConnectionLimits::default()), (10, 10, 1800, 600));
    assert_eq!(secs(from("7")), (7, 7, 7, 7));
    // Zero would switch a limit off; it is not accepted.
    assert_eq!(secs(from("0")), (10, 10, 1800, 600));
    assert_eq!(secs(from("soon")), (10, 10, 1800, 600));
    assert_eq!(secs(from("99999999999999999999")), (10, 10, 1800, 600));
    assert_eq!(secs(from("99999999999")), (300, 300, 604_800, 3600));
    // The time to authenticate has a variable of its own.
    let auth = ConnectionLimits::from_lookup(|name| {
        (name == "KRONIKA_GRPC_AUTH_TIMEOUT_SECS").then(|| "45".to_owned())
    });
    assert_eq!(secs(auth), (10, 45, 1800, 600));
}

fn ip(text: &str) -> std::net::IpAddr {
    text.parse().unwrap()
}

#[test]
fn an_ipv6_peer_is_its_64_and_an_ipv4_peer_its_address() {
    assert_eq!(peer_key(ip("2001:db8:1:2::1")), ip("2001:db8:1:2::"));
    assert_eq!(
        peer_key(ip("2001:db8:1:2:ffff:ffff:ffff:9")),
        ip("2001:db8:1:2::")
    );
    assert_ne!(peer_key(ip("2001:db8:1:3::1")), ip("2001:db8:1:2::"));
    assert_eq!(peer_key(ip("192.0.2.7")), ip("192.0.2.7"));
    assert_eq!(peer_key(ip("::ffff:192.0.2.7")), ip("192.0.2.7"));
}

/// Every host on a link has its link-local address in `fe80::/64`: the
/// /64 is not a peer there, the address is.
#[test]
fn a_link_local_peer_is_its_address() {
    assert_eq!(peer_key(ip("fe80::1")), ip("fe80::1"));
    assert_eq!(peer_key(ip("fe80::2")), ip("fe80::2"));
    assert_eq!(peer_key(ip("febf::aa:2")), ip("febf::aa:2"));
    // Just outside fe80::/10.
    assert_eq!(peer_key(ip("fec0::1")), ip("fec0::"));
}

/// Two addresses of one /64 are one peer: with room for one connection
/// per peer the second is refused, a neighbouring /64 is not.
#[tokio::test(start_paused = true)]
async fn addresses_in_one_64_share_one_allowance() {
    let gate = Gate::new(counts(10, 1), Access::Open);
    let admit = |peer: &str| gate.admit(tokio::io::empty(), from(peer));
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

/// A /48 holds 65 536 peers; it does not get that many allowances.
#[tokio::test(start_paused = true)]
async fn the_64s_of_one_48_share_a_coarser_allowance() {
    let limits = ConnectionLimits {
        max_per_ipv6_site: 3,
        ..counts(100, 2)
    };
    let gate = Gate::new(limits, Access::Open);
    let admit = |peer: &str| gate.admit(tokio::io::empty(), from(peer));
    let held: Vec<_> = ["2001:db8:1:1::1", "2001:db8:1:2::1", "2001:db8:1:3::1"]
        .into_iter()
        .map(|peer| admit(peer).expect("under the site's allowance"))
        .collect();
    assert!(
        admit("2001:db8:1:4::1").is_none(),
        "a fourth /64 of the same /48 got in"
    );
    let _neighbour = admit("2001:db8:2:1::1").expect("another /48 is another site");
    // IPv4 has no site: more peers than a site's allowance get in.
    let _v4: Vec<_> = ["192.0.2.1", "192.0.2.2", "192.0.2.3", "192.0.2.4"]
        .into_iter()
        .map(|peer| admit(peer).expect("an IPv4 peer of its own"))
        .collect();
    assert_eq!(gate.open(), 8);

    drop(held);
    assert_eq!(gate.open(), 5);
    assert!(admit("2001:db8:1:4::1").is_some(), "places not returned");
}

fn link_local(address: &str, interface: u32) -> SocketAddr {
    let std::net::IpAddr::V6(v6) = ip(address) else {
        panic!("{address} is not IPv6");
    };
    SocketAddr::V6(std::net::SocketAddrV6::new(v6, 40_000, 0, interface))
}

/// Hosts on one link are told apart by their link-local address, and the
/// same address on another interface is another host. The link as a
/// whole is a site.
#[tokio::test(start_paused = true)]
async fn link_local_peers_are_their_address_and_interface() {
    let limits = ConnectionLimits {
        max_per_ipv6_site: 3,
        ..counts(100, 1)
    };
    let gate = Gate::new(limits, Access::Open);
    let admit = |remote: SocketAddr| gate.admit(tokio::io::empty(), remote);
    let _one = admit(link_local("fe80::1", 2)).expect("first host");
    let _two = admit(link_local("fe80::2", 2)).expect("a second host on the link");
    assert!(admit(link_local("fe80::1", 2)).is_none(), "one peer, twice");
    let _other_link = admit(link_local("fe80::1", 3)).expect("same address, another link");
    let _three = admit(link_local("fe80::3", 2)).expect("a third host on the link");
    assert!(
        admit(link_local("fe80::4", 2)).is_none(),
        "the link got more than a site's allowance"
    );
    assert_eq!(gate.open(), 4);
}

/// An admitted in-memory connection, treated as a server does: read
/// until the client ends it or a read fails, then drop it. `task` yields
/// how the connection ended.
struct Served {
    client: DuplexStream,
    info: ConnInfo,
    task: JoinHandle<io::Result<()>>,
}

fn serve<IO>(mut stream: LimitedStream<IO>) -> (ConnInfo, JoinHandle<io::Result<()>>)
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let info = stream.connect_info();
    let task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while stream.read(&mut buf).await? > 0 {}
        Ok(())
    });
    (info, task)
}

fn served_from(gate: &Gate, peer: &str) -> Served {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let stream = gate.admit(server, from(peer)).expect("under the limits");
    let (info, task) = serve(stream);
    Served { client, info, task }
}

fn served(gate: &Gate) -> Served {
    served_from(gate, PEER)
}

/// The connection ends with an error of `kind` after `expected` of
/// (paused) time.
async fn ends_after(
    started: Instant,
    task: JoinHandle<io::Result<()>>,
    expected: Duration,
    kind: io::ErrorKind,
) {
    // A day of paused time: a connection nothing cuts fails the test
    // instead of hanging it.
    let ended = tokio::time::timeout(secs(24 * 60 * 60), task).await;
    let error = ended
        .expect("the connection was never cut")
        .unwrap()
        .expect_err("the connection was not cut");
    assert_eq!(error.kind(), kind, "{error}");
    let took = started.elapsed();
    assert!(
        took >= expected && took < expected + secs(1),
        "cut after {took:?}, expected {expected:?}"
    );
}

/// The connection is cut with `TimedOut` after `expected`.
async fn cut_after(started: Instant, task: JoinHandle<io::Result<()>>, expected: Duration) {
    ends_after(started, task, expected, io::ErrorKind::TimedOut).await;
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

fn open_gate() -> Gate {
    Gate::new(ConnectionLimits::default(), Access::Open)
}

#[tokio::test(start_paused = true)]
async fn a_silent_connection_is_cut_at_the_deadline_and_its_slot_returns() {
    let gate = open_gate();
    let started = Instant::now();
    let Served {
        mut client, task, ..
    } = served(&gate);
    assert_eq!(gate.open(), 1);
    assert_cut_after(&gate, started, task, secs(10)).await;
    // The client sees the connection end.
    assert_eq!(client.read(&mut [0u8; 1]).await.unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn many_silent_connections_do_not_keep_the_listener_full() {
    let gate = Gate::new(counts(8, 8), Access::Open);
    let started = Instant::now();
    let held: Vec<_> = (0..8).map(|_| served(&gate)).collect();
    assert!(gate.admit(tokio::io::empty(), from(PEER)).is_none(), "full");
    for connection in held {
        cut_after(started, connection.task, secs(10)).await;
    }
    assert_eq!(gate.open(), 0);
    assert!(gate.admit(tokio::io::empty(), from(PEER)).is_some());
}

/// One byte a second never completes the preface in time.
#[tokio::test(start_paused = true)]
async fn a_client_that_dribbles_the_preface_is_cut_at_the_deadline() {
    let gate = open_gate();
    let started = Instant::now();
    let Served {
        mut client, task, ..
    } = served(&gate);
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
    let gate = open_gate();
    let Served {
        mut client, task, ..
    } = served(&gate);
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
    let gate = open_gate();
    let started = Instant::now();
    let Served {
        mut client, task, ..
    } = served(&gate);
    client
        .write_all(b"PRI * HTTP/1.1\r\n\r\nSM\r\n\r\n and more to follow")
        .await
        .unwrap();
    assert_cut_after(&gate, started, task, secs(10)).await;
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
    let gate = Gate::new(limits, Access::Open);
    let started = Instant::now();
    let Served {
        mut client, task, ..
    } = served(&gate);
    client.write_all(PREFACE).await.unwrap();
    tokio::time::sleep(secs(119)).await;
    assert!(!task.is_finished(), "cut before the grace was over");
    assert_cut_after(&gate, started, task, secs(120)).await;
}

#[tokio::test(start_paused = true)]
async fn a_cut_connection_also_refuses_writes() {
    let gate = open_gate();
    let (_client, server) = tokio::io::duplex(1024);
    let mut stream = gate.admit(server, from(PEER)).unwrap();
    tokio::time::sleep(secs(11)).await;
    let error = stream.write_all(b"settings").await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    let error = stream.flush().await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}

#[tokio::test(start_paused = true)]
async fn cut_connections_are_logged_at_a_bounded_rate_with_the_peer() {
    let (logs, _guard) = crate::test_logs::capture();
    let gate = open_gate();
    let started = Instant::now();
    let held: Vec<_> = (0..40).map(|_| served(&gate)).collect();
    for connection in held {
        cut_after(started, connection.task, secs(10)).await;
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

/// Limits under which the time to authenticate is not the handshake
/// deadline, so a test sees which of the two cut a connection.
fn token_limits(max_connections: usize, max_per_peer: usize) -> ConnectionLimits {
    ConnectionLimits {
        auth_timeout: secs(30),
        ..counts(max_connections, max_per_peer)
    }
}

fn token_gate(max_connections: usize, max_per_peer: usize) -> Gate {
    Gate::new(token_limits(max_connections, max_per_peer), Access::Token)
}

/// The preface is all an HTTP/2 client needs to stay connected: with a
/// token configured it also needs an authorised call.
#[tokio::test(start_paused = true)]
async fn a_connection_that_never_authenticates_is_cut_when_its_time_is_up() {
    let (logs, _guard) = crate::test_logs::capture();
    let gate = token_gate(8, 8);
    let started = Instant::now();
    let Served {
        mut client, task, ..
    } = served(&gate);
    client.write_all(PREFACE).await.unwrap();
    assert_eq!((gate.open(), gate.unauthenticated()), (1, 1));
    assert_cut_after(&gate, started, task, secs(30)).await;
    assert_eq!(gate.unauthenticated(), 0);
    assert_eq!(client.read(&mut [0u8; 1]).await.unwrap(), 0);
    let text = logs.text();
    assert!(
        text.contains("no authorised call in time") && text.contains("timeout_secs=30"),
        "{text}"
    );
}

#[tokio::test(start_paused = true)]
async fn an_authenticated_connection_is_not_cut_when_that_time_is_up() {
    let gate = token_gate(8, 8);
    let Served {
        mut client,
        info,
        task,
    } = served(&gate);
    client.write_all(PREFACE).await.unwrap();
    tokio::time::sleep(secs(29)).await;
    info.authenticated();
    assert_eq!((gate.open(), gate.unauthenticated()), (1, 0));
    tokio::time::sleep(secs(600)).await;
    assert!(!task.is_finished(), "an authenticated connection was cut");
    assert_eq!(gate.open(), 1);

    drop(client);
    task.await.unwrap().expect("ended by the client");
    assert_eq!(gate.open(), 0);
}

/// Without a token there is nothing to wait for.
#[tokio::test(start_paused = true)]
async fn without_a_token_no_connection_waits_to_authenticate() {
    let gate = Gate::new(token_limits(8, 8), Access::Open);
    let Served {
        mut client, task, ..
    } = served(&gate);
    client.write_all(PREFACE).await.unwrap();
    assert_eq!((gate.open(), gate.unauthenticated()), (1, 0));
    tokio::time::sleep(secs(600)).await;
    assert!(!task.is_finished(), "cut without a token to ask for");
}

/// The newcomer gets the place of the oldest connection that has not
/// authenticated, at once: nobody waits for a deadline.
#[tokio::test(start_paused = true)]
async fn a_full_listener_lets_a_newcomer_in_for_the_oldest_unauthenticated() {
    let (logs, _guard) = crate::test_logs::capture();
    let gate = token_gate(3, 3);
    let started = Instant::now();
    let first = served(&gate);
    let mut second = served(&gate);
    let third = served(&gate);
    first.info.authenticated();
    assert_eq!((gate.open(), gate.unauthenticated()), (3, 2));

    let newcomer = served(&gate);
    assert_eq!((gate.open(), gate.unauthenticated()), (3, 2));
    ends_after(
        started,
        second.task,
        secs(0),
        io::ErrorKind::ConnectionAborted,
    )
    .await;
    assert_eq!(second.client.read(&mut [0u8; 1]).await.unwrap(), 0);
    assert_eq!(gate.open(), 3, "the place was given back twice");
    for kept in [&first.task, &third.task, &newcomer.task] {
        assert!(!kept.is_finished(), "the wrong connection was put out");
    }
    let text = logs.text();
    assert!(
        text.contains("made no authorised call and its place was needed"),
        "{text}"
    );
}

/// A connection that has made an authorised call is never put out for
/// one that has not.
#[tokio::test(start_paused = true)]
async fn a_listener_full_of_authenticated_connections_refuses_the_newcomer() {
    let gate = token_gate(3, 3);
    let held: Vec<_> = (0..3).map(|_| served(&gate)).collect();
    for connection in &held {
        connection.info.authenticated();
    }
    let (_client, server) = tokio::io::duplex(1024);
    assert!(
        gate.admit(server, from(PEER)).is_none(),
        "someone was put out"
    );
    assert_eq!((gate.open(), gate.unauthenticated()), (3, 0));
    tokio::time::sleep(secs(5)).await;
    for connection in &held {
        assert!(!connection.task.is_finished(), "put out for a newcomer");
    }
}

/// A peer at its allowance makes room among its own connections, not
/// among another peer's, however old those are.
#[tokio::test(start_paused = true)]
async fn a_peer_at_its_allowance_puts_out_its_own_oldest() {
    let gate = token_gate(10, 2);
    let started = Instant::now();
    let bystander = served_from(&gate, "192.0.2.9");
    let own_oldest = served(&gate);
    let own_newer = served(&gate);
    let newcomer = served(&gate);
    ends_after(
        started,
        own_oldest.task,
        secs(0),
        io::ErrorKind::ConnectionAborted,
    )
    .await;
    for kept in [&bystander.task, &own_newer.task, &newcomer.task] {
        assert!(!kept.is_finished(), "the wrong connection was put out");
    }
    assert_eq!(gate.open(), 3);

    // With the peer's connections all authenticated the peer is full.
    own_newer.info.authenticated();
    newcomer.info.authenticated();
    assert!(gate.admit(tokio::io::empty(), from(PEER)).is_none());
    assert!(!bystander.task.is_finished(), "another peer paid for it");
}

/// Likewise for a site: room is made among the site's connections.
#[tokio::test(start_paused = true)]
async fn a_site_at_its_allowance_puts_out_its_own_oldest() {
    let limits = ConnectionLimits {
        max_per_ipv6_site: 2,
        ..token_limits(10, 2)
    };
    let gate = Gate::new(limits, Access::Token);
    let started = Instant::now();
    let bystander = served_from(&gate, "2001:db8:2:1::1");
    let own_oldest = served_from(&gate, "2001:db8:1:1::1");
    let own_newer = served_from(&gate, "2001:db8:1:2::1");
    let newcomer = served_from(&gate, "2001:db8:1:3::1");
    ends_after(
        started,
        own_oldest.task,
        secs(0),
        io::ErrorKind::ConnectionAborted,
    )
    .await;
    for kept in [&bystander.task, &own_newer.task, &newcomer.task] {
        assert!(!kept.is_finished(), "the wrong connection was put out");
    }
    assert_eq!(gate.open(), 3);
}

/// Every way a connection ends gives its place back exactly once.
#[tokio::test(start_paused = true)]
async fn a_place_comes_back_once_however_the_connection_ends() {
    let gate = token_gate(2, 2);
    // Put out, then dropped by whoever held it.
    let (_c1, s1) = tokio::io::duplex(1024);
    let (_c2, s2) = tokio::io::duplex(1024);
    let (_c3, s3) = tokio::io::duplex(1024);
    let put_out = gate.admit(s1, from(PEER)).unwrap();
    let kept = gate.admit(s2, from(PEER)).unwrap();
    let newcomer = gate.admit(s3, from(PEER)).unwrap();
    assert_eq!((gate.open(), gate.unauthenticated()), (2, 2));
    let info = put_out.connect_info();
    drop(put_out);
    assert_eq!(gate.open(), 2, "a place came back twice");

    // An authorised call that arrives after the place is gone does not
    // bring it back, and does not count the connection as authenticated.
    info.authenticated();
    assert_eq!((gate.open(), gate.unauthenticated()), (2, 2));

    // Ended by its owner, authenticated or not.
    kept.connect_info().authenticated();
    assert_eq!((gate.open(), gate.unauthenticated()), (2, 1));
    drop(kept);
    assert_eq!((gate.open(), gate.unauthenticated()), (1, 1));
    drop(newcomer);
    assert_eq!((gate.open(), gate.unauthenticated()), (0, 0));

    // Refused: nothing was taken.
    let full = Gate::new(counts(1, 1), Access::Open);
    let _held = full.admit(tokio::io::empty(), from(PEER)).unwrap();
    assert!(full.admit(tokio::io::empty(), from(PEER)).is_none());
    assert_eq!(full.open(), 1);
}

/// A connection that is put out before anything polled it finds out on
/// its first read.
#[tokio::test(start_paused = true)]
async fn a_connection_put_out_before_it_was_read_fails_its_first_read() {
    let gate = token_gate(1, 1);
    let (_c1, s1) = tokio::io::duplex(1024);
    let mut first = gate.admit(s1, from(PEER)).unwrap();
    let _newcomer = served(&gate);
    let error = first.read(&mut [0u8; 1]).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    let error = first.write_all(b"x").await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
}

fn tls_listening(limits: ConnectionLimits, access: Access) -> Listening {
    listening(limits, access, true)
}

/// A real TLS client on `io`, through the whole handshake.
async fn tls_client(io: DuplexStream) -> tokio_rustls::client::TlsStream<DuplexStream> {
    let name = "localhost".try_into().unwrap();
    let connect = TlsConnector::from(client_tls()).connect(name, io);
    soon(connect).await.expect("TLS handshake")
}

/// The header of a 512-byte TLS handshake record and a few of its bytes.
const STALLED_HELLO: &[u8] = &[0x16, 3, 1, 2, 0, 1, 0, 1, 252];

/// The client's end is closed after `expected` of (paused) time.
async fn closed_after(started: Instant, client: &mut DuplexStream, expected: Duration) {
    let mut seen = Vec::new();
    let read = tokio::time::timeout(secs(24 * 60 * 60), client.read_to_end(&mut seen));
    // The read ends or fails, depending on how the server lets go.
    let _ = read.await.expect("never closed");
    let took = started.elapsed();
    assert!(
        took >= expected && took < expected + secs(1),
        "closed after {took:?}, expected {expected:?}"
    );
}

/// A connection is handed to the server when its TLS handshake is
/// complete and not before: a stalled one is cut here.
#[tokio::test(start_paused = true)]
async fn a_stalled_tls_handshake_is_cut_and_never_reaches_the_server() {
    let mut l = tls_listening(ConnectionLimits::default(), Access::Open);
    let started = Instant::now();
    let mut client = l.dial.connect(from(PEER));
    client.write_all(STALLED_HELLO).await.unwrap();
    closed_after(started, &mut client, secs(10)).await;
    assert_eq!(l.gate.open(), 0, "the slot did not come back");
    assert!(l.accepted.try_recv().is_err(), "handed to the server");
}

/// The `ClientHello` offers early data and what follows decrypts to
/// nothing. rustls skips such records without a word, so the handshake
/// neither completes nor fails; bytes going by do not end the deadline.
#[tokio::test(start_paused = true)]
async fn early_data_that_never_becomes_a_handshake_is_cut_at_the_deadline() {
    let mut l = tls_listening(ConnectionLimits::default(), Access::Open);
    let started = Instant::now();
    let mut client = l.dial.connect(from(PEER));
    client
        .write_all(&hello_offering_early_data())
        .await
        .unwrap();
    for _ in 0..3 {
        client.write_all(&undecryptable_record(64)).await.unwrap();
    }
    tokio::time::sleep(secs(9)).await;
    assert_eq!(l.gate.open(), 1, "rustls gave the connection up");
    assert!(l.accepted.try_recv().is_err(), "handed to the server");
    closed_after(started, &mut client, secs(10)).await;
    assert_eq!(l.gate.open(), 0, "the slot did not come back");
    assert!(l.accepted.try_recv().is_err(), "handed to the server");
}

/// The TLS handshake is not the end of the deadline: the preface is.
#[tokio::test(start_paused = true)]
async fn a_tls_client_that_sends_no_preface_is_cut_at_the_deadline() {
    let mut l = tls_listening(ConnectionLimits::default(), Access::Open);
    let started = Instant::now();
    let _client = tls_client(l.dial.connect(from(PEER))).await;
    let (_info, task) = serve(l.accepted().await);
    assert_cut_after(&l.gate, started, task, secs(10)).await;
}

#[tokio::test(start_paused = true)]
async fn a_tls_client_that_sends_the_preface_is_not_cut() {
    let mut l = tls_listening(ConnectionLimits::default(), Access::Open);
    let mut client = tls_client(l.dial.connect(from(PEER))).await;
    let (info, task) = serve(l.accepted().await);
    assert_eq!(info.remote_addr(), from(PEER));
    for piece in PREFACE.chunks(7) {
        tokio::time::sleep(secs(1)).await;
        client.write_all(piece).await.unwrap();
        client.flush().await.unwrap();
    }
    tokio::time::sleep(secs(600)).await;
    assert!(!task.is_finished(), "an established TLS connection was cut");
    assert_eq!(l.gate.open(), 1);
}

/// The time to authenticate runs from the accept, through the TLS
/// handshake: a long handshake deadline does not hide a connection
/// from it.
#[tokio::test(start_paused = true)]
async fn the_time_to_authenticate_covers_the_tls_handshake() {
    let limits = ConnectionLimits {
        handshake_timeout: secs(300),
        auth_timeout: secs(10),
        ..ConnectionLimits::default()
    };
    let mut l = tls_listening(limits, Access::Token);
    let started = Instant::now();
    let mut client = l.dial.connect(from(PEER));
    client
        .write_all(&hello_offering_early_data())
        .await
        .unwrap();
    client.write_all(&undecryptable_record(64)).await.unwrap();
    closed_after(started, &mut client, secs(10)).await;
    assert_eq!((l.gate.open(), l.gate.unauthenticated()), (0, 0));
    assert!(l.accepted.try_recv().is_err(), "handed to the server");
}

#[tokio::test(start_paused = true)]
async fn a_failed_tls_handshake_gives_its_place_back_at_once() {
    let mut l = tls_listening(ConnectionLimits::default(), Access::Token);
    let started = Instant::now();
    let mut client = l.dial.connect(from(PEER));
    client.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    closed_after(started, &mut client, secs(0)).await;
    assert_eq!((l.gate.open(), l.gate.unauthenticated()), (0, 0));
    assert!(l.accepted.try_recv().is_err(), "handed to the server");
}

/// A handshake in progress holds a place like any connection that has
/// not authenticated, and gives it up like one.
#[tokio::test(start_paused = true)]
async fn a_tls_handshake_in_progress_gives_way_to_a_newcomer() {
    let mut l = tls_listening(token_limits(1, 1), Access::Token);
    let started = Instant::now();
    let mut stalled = l.dial.connect(from(PEER));
    stalled.write_all(STALLED_HELLO).await.unwrap();
    let _newcomer = tls_client(l.dial.connect(from(PEER))).await;
    let _held = l.accepted().await;
    closed_after(started, &mut stalled, secs(0)).await;
    assert_eq!(l.gate.open(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_listener_that_goes_away_gives_back_the_places_of_its_handshakes() {
    let l = tls_listening(ConnectionLimits::default(), Access::Open);
    let mut client = l.dial.connect(from(PEER));
    client.write_all(STALLED_HELLO).await.unwrap();
    soon(async {
        while l.gate.open() < 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    l.task.abort();
    let started = Instant::now();
    closed_after(started, &mut client, secs(0)).await;
    assert_eq!(l.gate.open(), 0);
}

fn no_descriptors() -> io::Error {
    // EMFILE, as accept(2) reports it.
    io::Error::from_raw_os_error(24)
}

/// With no file descriptors left every accept fails at once. The
/// listener waits before it asks again instead of spinning, and still
/// accepts when there is something to accept.
#[tokio::test(start_paused = true)]
async fn accept_errors_make_the_listener_back_off() {
    let (logs, _guard) = crate::test_logs::capture();
    let mut l = listening(ConnectionLimits::default(), Access::Open, false);
    let started = Instant::now();
    for _ in 0..50 {
        l.dial.fail(no_descriptors());
    }
    let _client = l.dial.connect(from(PEER));
    let _held = tokio::time::timeout(secs(60), l.accepted.recv())
        .await
        .expect("never accepted again")
        .unwrap();
    assert_eq!(started.elapsed(), ACCEPT_BACKOFF * 50, "one wait an error");
    // Fifty errors, the connection and the wait for the next one: the
    // socket was not asked while the listener backed off.
    assert_eq!(l.polls.load(Ordering::Relaxed), 52);
    let lines = logs
        .text()
        .lines()
        .filter(|l| l.contains("cannot accept connections"))
        .count();
    assert!((1..=10).contains(&lines), "{lines} lines for 50 errors");
}

/// An error about one connection (the peer gave up while it waited to be
/// accepted) says nothing about the next.
#[tokio::test(start_paused = true)]
async fn an_accept_error_about_one_connection_does_not_delay_the_next() {
    let mut l = listening(ConnectionLimits::default(), Access::Open, false);
    let started = Instant::now();
    for _ in 0..50 {
        l.dial.fail(io::ErrorKind::ConnectionAborted.into());
    }
    let _client = l.dial.connect(from(PEER));
    let _held = l.accepted().await;
    assert_eq!(started.elapsed(), secs(0));
}

#[test]
fn tls_settings_need_a_certificate_and_its_key() {
    use super::testing::{CERT_PEM, KEY_PEM};

    let (cert, key) = (CERT_PEM.as_bytes(), KEY_PEM.as_bytes());
    let config = tls_config(cert, key).expect("a matching pair");
    assert_eq!(config.alpn_protocols, [b"h2".to_vec()]);
    for (cert, key) in [(b"".as_slice(), key), (cert, b"".as_slice()), (key, cert)] {
        let error = tls_config(cert, key).expect_err("not a pair");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    }
}
