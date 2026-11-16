//! The connections that are open, and who gives way when there is no
//! room.
//!
//! Every connection has a place under three counts: the total, its peer's
//! and, for IPv6, its site's. When an ingest token is configured a
//! connection is also *waiting* until its first authorised call. A
//! newcomer that finds a count full takes the place of the oldest waiting
//! connection under that count, if there is one; a connection that has
//! made an authorised call is never put out for one that has not.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Waker;

use super::limits::ConnectionLimits;
use crate::log_guard::LogLimiter;

/// Whether the services behind the listener ask their callers for a
/// token. Only then can the listener tell a client from someone who
/// merely connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// No ingest token is configured: every connection is as good as the
    /// next, none waits to authenticate and none is put out for another.
    Open,
    /// An ingest token is configured: a connection has
    /// [`ConnectionLimits::auth_timeout`] to make an authorised call, and
    /// until then gives way to a newcomer when there is no room.
    Token,
}

/// Bits of an IPv6 address that name a peer: a host is normally routed a
/// whole /64 and can connect from any address in it.
const PEER_BITS: u32 = 64;
/// Bits of an IPv6 address that name a site: the block a provider
/// usually assigns to one customer.
const SITE_BITS: u32 = 48;
/// Bits that mark an IPv6 address as link-local (`fe80::/10`).
const LINK_LOCAL_BITS: u32 = 10;

fn prefix(v6: Ipv6Addr, bits: u32) -> Ipv6Addr {
    Ipv6Addr::from(u128::from(v6) & (u128::MAX << (128 - bits)))
}

fn link_local(v6: Ipv6Addr) -> bool {
    prefix(v6, LINK_LOCAL_BITS) == Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)
}

/// What a per-peer share is counted by: the address itself for IPv4 (and
/// for IPv4 mapped into IPv6), the /64 for IPv6. A link-local IPv6
/// address (`fe80::/10`) is a peer of its own: every host on a link has
/// its address in the one `fe80::/64`, which therefore says nothing
/// about who is calling.
#[must_use]
pub fn peer_key(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else {
        return ip;
    };
    if let Some(v4) = v6.to_ipv4_mapped() {
        return IpAddr::V4(v4);
    }
    if link_local(v6) {
        return ip;
    }
    IpAddr::V6(prefix(v6, PEER_BITS))
}

/// What connections are counted by: a network, and for link-local
/// addresses the interface (scope id) they are on, since the same
/// address on two links is two hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    net: IpAddr,
    scope: u32,
}

/// The peer of `remote` and, for IPv6, its site: the /48, or the whole
/// link for a link-local address.
fn keys(remote: SocketAddr) -> (Key, Option<Key>) {
    let peer = peer_key(remote.ip());
    let IpAddr::V6(v6) = peer else {
        return (
            Key {
                net: peer,
                scope: 0,
            },
            None,
        );
    };
    if !link_local(v6) {
        let site = IpAddr::V6(prefix(v6, SITE_BITS));
        return (
            Key {
                net: peer,
                scope: 0,
            },
            Some(Key {
                net: site,
                scope: 0,
            }),
        );
    }
    let scope = match remote {
        SocketAddr::V6(v6) => v6.scope_id(),
        SocketAddr::V4(_) => 0,
    };
    let link = IpAddr::V6(prefix(v6, LINK_LOCAL_BITS));
    (Key { net: peer, scope }, Some(Key { net: link, scope }))
}

/// One open connection, as the gate and the services see it.
pub struct Conn {
    id: u64,
    remote: SocketAddr,
    peer: Key,
    site: Option<Key>,
    authenticated: AtomicBool,
    evicted: AtomicBool,
    /// Whether its place has been given back. Read and written under the
    /// gate's lock only.
    released: AtomicBool,
    /// Wakes the connection's task when it is put out.
    waker: Mutex<Option<Waker>>,
    gate: Gate,
}

impl Conn {
    /// Where the connection came from.
    pub(super) fn remote(&self) -> SocketAddr {
        self.remote
    }

    /// Whether the connection has made an authorised call (always, when
    /// no token is configured).
    pub(super) fn is_authenticated(&self) -> bool {
        self.authenticated.load(Ordering::Acquire)
    }

    /// Whether the connection lost its place to a newcomer.
    pub(super) fn is_evicted(&self) -> bool {
        self.evicted.load(Ordering::Acquire)
    }

    /// The connection made an authorised call: it no longer waits, and
    /// no newcomer takes its place. Too late for a connection that has
    /// been put out already.
    pub(super) fn authenticate(&self) {
        if self.is_authenticated() {
            return;
        }
        let mut now = self.gate.lock();
        if self.released.load(Ordering::Relaxed) {
            return;
        }
        self.authenticated.store(true, Ordering::Release);
        now.waiting.remove(&self.id);
    }

    /// Have `waker` woken when the connection is put out.
    pub(super) fn watch(&self, waker: &Waker) {
        let mut held = self.waker.lock().unwrap_or_else(PoisonError::into_inner);
        if held.as_ref().is_some_and(|known| known.will_wake(waker)) {
            return;
        }
        *held = Some(waker.clone());
    }

    /// Tell the connection's task that its place is gone.
    fn evict(&self) {
        self.evicted.store(true, Ordering::Release);
        let waker = self
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The count that has no room for a newcomer, narrowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Full {
    Peer,
    Site,
    Total,
}

/// The connections that are open now.
#[derive(Default)]
struct Open {
    next_id: u64,
    total: usize,
    peers: HashMap<Key, usize>,
    sites: HashMap<Key, usize>,
    /// The connections that have not made an authorised call, oldest
    /// first (ids are handed out in order).
    waiting: BTreeMap<u64, Arc<Conn>>,
}

fn held(counts: &HashMap<Key, usize>, key: Key) -> usize {
    counts.get(&key).copied().unwrap_or(0)
}

fn one_less(counts: &mut HashMap<Key, usize>, key: Key) {
    let left = held(counts, key).saturating_sub(1);
    if left == 0 {
        counts.remove(&key);
        return;
    }
    counts.insert(key, left);
}

impl Open {
    /// The narrowest count with no room for one more connection of
    /// `peer` and `site`.
    fn full(&self, limits: ConnectionLimits, peer: Key, site: Option<Key>) -> Option<Full> {
        if held(&self.peers, peer) >= limits.max_per_peer {
            return Some(Full::Peer);
        }
        if site.is_some_and(|site| held(&self.sites, site) >= limits.max_per_ipv6_site) {
            return Some(Full::Site);
        }
        (self.total >= limits.max_connections).then_some(Full::Total)
    }

    /// The oldest waiting connection counted under `full`. Its place is
    /// all a newcomer needs: counts never exceed their limits, a peer's
    /// connections are all in its site, and every connection is in the
    /// total.
    fn oldest_waiting(&self, full: Full, peer: Key, site: Option<Key>) -> Option<Arc<Conn>> {
        let counted = |conn: &&Arc<Conn>| match full {
            Full::Peer => conn.peer == peer,
            Full::Site => conn.site == site,
            Full::Total => true,
        };
        self.waiting.values().find(counted).cloned()
    }

    fn take(&mut self, peer: Key, site: Option<Key>) {
        self.total += 1;
        *self.peers.entry(peer).or_insert(0) += 1;
        if let Some(site) = site {
            *self.sites.entry(site).or_insert(0) += 1;
        }
    }

    /// Give the place of `conn` back, once.
    fn give_back(&mut self, conn: &Conn) {
        if conn.released.swap(true, Ordering::Relaxed) {
            return;
        }
        self.total = self.total.saturating_sub(1);
        one_less(&mut self.peers, conn.peer);
        if let Some(site) = conn.site {
            one_less(&mut self.sites, site);
        }
        self.waiting.remove(&conn.id);
    }
}

struct Shared {
    limits: ConnectionLimits,
    access: Access,
    open: Mutex<Open>,
    refused: LogLimiter,
    cut: LogLimiter,
}

/// The limits and the connections open under them.
#[derive(Clone)]
pub struct Gate {
    shared: Arc<Shared>,
}

impl Gate {
    #[must_use]
    pub fn new(limits: ConnectionLimits, access: Access) -> Self {
        Self {
            shared: Arc::new(Shared {
                limits,
                access,
                open: Mutex::default(),
                refused: LogLimiter::warnings(),
                cut: LogLimiter::warnings(),
            }),
        }
    }

    pub(super) fn limits(&self) -> ConnectionLimits {
        self.shared.limits
    }

    fn lock(&self) -> MutexGuard<'_, Open> {
        let open = &self.shared.open;
        open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Connections open now.
    #[must_use]
    pub fn open(&self) -> usize {
        self.lock().total
    }

    /// Open connections that have not made an authorised call (none
    /// without a token).
    #[must_use]
    pub fn unauthenticated(&self) -> usize {
        self.lock().waiting.len()
    }

    /// `Some(n)` when a connection cut for `reason` may be logged, `n`
    /// being how many such lines were left out since the last one.
    pub(super) fn may_log_cut(&self, reason: &'static str) -> Option<u64> {
        self.shared.cut.allow(reason)
    }

    /// A place for one more connection from `remote`, or `None` when
    /// there is none and nobody to put out for it.
    pub(super) fn claim(&self, remote: SocketAddr) -> Option<Arc<Conn>> {
        let (peer, site) = keys(remote);
        let token = self.shared.access == Access::Token;
        let mut now = self.lock();
        let mut put_out = None;
        if let Some(full) = now.full(self.shared.limits, peer, site) {
            let Some(victim) = now.oldest_waiting(full, peer, site) else {
                drop(now);
                self.log_refused(remote);
                return None;
            };
            now.give_back(&victim);
            put_out = Some(victim);
        }
        let conn = Arc::new(Conn {
            id: now.next_id,
            remote,
            peer,
            site,
            authenticated: AtomicBool::new(!token),
            evicted: AtomicBool::new(false),
            released: AtomicBool::new(false),
            waker: Mutex::new(None),
            gate: self.clone(),
        });
        now.next_id += 1;
        now.take(peer, site);
        if token {
            now.waiting.insert(conn.id, conn.clone());
        }
        drop(now);
        if let Some(victim) = put_out {
            victim.evict();
            self.log_put_out(&victim, remote);
        }
        Some(conn)
    }

    /// Give the place of `conn` back; a second call does nothing.
    pub(super) fn release(&self, conn: &Conn) {
        self.lock().give_back(conn);
    }

    fn log_refused(&self, remote: SocketAddr) {
        let Some(suppressed) = self.shared.refused.allow("refused") else {
            return;
        };
        let limits = self.shared.limits;
        tracing::warn!(
            peer = %remote.ip(),
            max_connections = limits.max_connections,
            max_per_peer = limits.max_per_peer,
            max_per_ipv6_site = limits.max_per_ipv6_site,
            suppressed,
            "gRPC connection refused: too many open connections"
        );
    }

    fn log_put_out(&self, victim: &Conn, newcomer: SocketAddr) {
        let Some(suppressed) = self.may_log_cut("put out") else {
            return;
        };
        tracing::warn!(
            peer = %victim.remote.ip(),
            newcomer = %newcomer.ip(),
            suppressed,
            "gRPC connection closed: it made no authorised call and its place was needed"
        );
    }
}
