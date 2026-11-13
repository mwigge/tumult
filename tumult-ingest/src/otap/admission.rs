//! Who holds what of the receiver: shares per connection and per peer.
//!
//! The limits in [`super::limits`] bound the receiver as a whole. Without
//! shares one client could take a whole limit for itself, and with it the
//! service from everyone else. A connection is named by the peer's socket
//! address, a peer by [`crate::listener::peer_key`] of its IP address (the
//! address for IPv4, the /64 for IPv6), the same key the listener counts
//! connections by. Behind a proxy every client has the proxy's address and
//! is one peer: raise the per-peer limits there.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};

use super::limits::{OtapLimits, RetainedPool};
use crate::listener::peer_key;

/// What one connection or one peer holds.
struct Holder {
    streams: usize,
    retained: Arc<RetainedPool>,
}

impl Holder {
    fn new(what: &'static str, limit: usize) -> Self {
        Self {
            streams: 0,
            retained: RetainedPool::share(what, limit),
        }
    }
}

#[derive(Default)]
struct Holders {
    connections: HashMap<SocketAddr, Holder>,
    peers: HashMap<IpAddr, Holder>,
}

/// The open streams of a receiver by connection and by peer.
pub(super) struct Shares {
    limits: OtapLimits,
    holders: Mutex<Holders>,
}

/// Why a stream got no place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refused {
    Connection,
}

impl Refused {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Connection => "too many otel-arrow streams on this connection",
        }
    }
}

/// One stream's place in the shares of its connection and its peer; given
/// back on drop.
pub(super) struct StreamSlot {
    shares: Arc<Shares>,
    connection: SocketAddr,
    peer: IpAddr,
    /// The retained-bytes accounts of the connection and the peer.
    retained: [Arc<RetainedPool>; 2],
}

impl Shares {
    pub fn new(limits: &OtapLimits) -> Arc<Self> {
        Arc::new(Self {
            limits: *limits,
            holders: Mutex::default(),
        })
    }

    /// A place for one more stream on the connection from `addr`.
    pub fn claim(self: &Arc<Self>, addr: SocketAddr) -> Result<StreamSlot, Refused> {
        let limits = &self.limits;
        let peer = peer_key(addr.ip());
        let mut holders = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        let Holders { connections, peers } = &mut *holders;
        let on_connection = connections.get(&addr).map_or(0, |held| held.streams);
        if on_connection >= limits.max_streams_per_connection {
            return Err(Refused::Connection);
        }
        let connection = connections.entry(addr).or_insert_with(|| {
            Holder::new(
                "retained bytes per connection",
                limits.max_connection_retained_bytes,
            )
        });
        let of_peer = peers.entry(peer).or_insert_with(|| {
            Holder::new("retained bytes per peer", limits.max_peer_retained_bytes)
        });
        connection.streams += 1;
        of_peer.streams += 1;
        Ok(StreamSlot {
            shares: self.clone(),
            connection: addr,
            peer,
            retained: [connection.retained.clone(), of_peer.retained.clone()],
        })
    }

    /// Connections and peers that hold a stream now.
    #[cfg(test)]
    pub fn holders(&self) -> (usize, usize) {
        let holders = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        (holders.connections.len(), holders.peers.len())
    }
}

impl StreamSlot {
    /// The accounts a stream of this connection charges what it keeps
    /// to, the narrowest first.
    pub fn retained(&self) -> impl Iterator<Item = Arc<RetainedPool>> + '_ {
        self.retained.iter().cloned()
    }
}

/// One stream less for `key`; the holder goes with its last stream.
fn leave<K: std::hash::Hash + Eq>(holders: &mut HashMap<K, Holder>, key: &K) {
    let Some(holder) = holders.get_mut(key) else {
        return;
    };
    holder.streams = holder.streams.saturating_sub(1);
    if holder.streams == 0 {
        holders.remove(key);
    }
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut holders = self
            .shares
            .holders
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        leave(&mut holders.connections, &self.connection);
        leave(&mut holders.peers, &self.peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn limits() -> OtapLimits {
        OtapLimits {
            max_streams_per_connection: 2,
            max_connection_retained_bytes: 100,
            max_peer_retained_bytes: 150,
            ..OtapLimits::default()
        }
    }

    #[test]
    fn a_connection_holds_no_more_streams_than_its_share() {
        let shares = Shares::new(&limits());
        let a = addr("192.0.2.1:1000");
        let first = shares.claim(a).ok().unwrap();
        let _second = shares.claim(a).ok().unwrap();
        assert_eq!(shares.claim(a).err(), Some(Refused::Connection));
        // Another connection of the same peer has a share of its own.
        let _other = shares.claim(addr("192.0.2.1:1001")).ok().unwrap();
        assert_eq!(shares.holders(), (2, 1));
        drop(first);
        let _again = shares.claim(a).ok().unwrap();
    }

    #[test]
    fn streams_of_one_connection_charge_one_account_and_peers_another() {
        let shares = Shares::new(&limits());
        let a1 = shares.claim(addr("192.0.2.1:1000")).ok().unwrap();
        let a2 = shares.claim(addr("192.0.2.1:1000")).ok().unwrap();
        let b = shares.claim(addr("192.0.2.1:2000")).ok().unwrap();
        let other = shares.claim(addr("192.0.2.2:1000")).ok().unwrap();
        let accounts = |slot: &StreamSlot| slot.retained().collect::<Vec<_>>();
        let (a1, a2, b, other) = (accounts(&a1), accounts(&a2), accounts(&b), accounts(&other));
        assert!(Arc::ptr_eq(&a1[0], &a2[0]), "one account per connection");
        assert!(!Arc::ptr_eq(&a1[0], &b[0]), "connections do not share it");
        assert!(Arc::ptr_eq(&a1[1], &b[1]), "one account per peer");
        assert!(!Arc::ptr_eq(&a1[1], &other[1]), "peers do not share it");
        assert_eq!((a1[0].limit(), a1[1].limit()), (100, 150));
    }

    #[test]
    fn an_ipv6_peer_is_its_64() {
        let shares = Shares::new(&limits());
        let one = shares.claim(addr("[2001:db8:1:2::1]:1000")).ok().unwrap();
        let two = shares.claim(addr("[2001:db8:1:2::2]:1000")).ok().unwrap();
        let far = shares.claim(addr("[2001:db8:1:3::1]:1000")).ok().unwrap();
        let peer = |slot: &StreamSlot| slot.retained().nth(1).unwrap();
        assert!(Arc::ptr_eq(&peer(&one), &peer(&two)));
        assert!(!Arc::ptr_eq(&peer(&one), &peer(&far)));
        assert_eq!(shares.holders(), (3, 2));
    }

    #[test]
    fn the_last_stream_takes_its_holders_with_it() {
        let shares = Shares::new(&limits());
        let a = shares.claim(addr("192.0.2.1:1000")).ok().unwrap();
        let b = shares.claim(addr("192.0.2.1:1001")).ok().unwrap();
        drop(a);
        assert_eq!(shares.holders(), (1, 1));
        drop(b);
        assert_eq!(shares.holders(), (0, 0));
    }
}
