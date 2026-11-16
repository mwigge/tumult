//! Whose batch is decoded next.
//!
//! At most [`OtapLimits::max_concurrent_decodes`] batches are decoded,
//! converted and written at once; the rest wait. A queue in order of
//! arrival would let one client with many streams keep every other client
//! waiting behind all of its batches. Here the waiting batches take turns:
//! peer after peer, within a peer connection after connection, within a
//! connection in order of arrival. A client that waits is served after at
//! most one batch of every other waiting peer, however many streams those
//! have.
//!
//! [`OtapLimits::max_concurrent_decodes`]: super::limits::OtapLimits::max_concurrent_decodes

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::oneshot;

/// Whose stream asks. Streams without a peer address (an in-memory
/// transport) are one client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct Client {
    pub peer: Option<IpAddr>,
    pub connection: Option<SocketAddr>,
}

/// Something that hands out what waits in it, one at a time.
trait Turns {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
    fn is_empty(&self) -> bool;
    /// Forget the waiters that have given up.
    fn prune(&mut self);
    #[cfg(test)]
    fn len(&self) -> usize;
}

type Waiter = oneshot::Sender<DecodePermit>;

impl Turns for VecDeque<Waiter> {
    type Item = Waiter;

    fn next(&mut self) -> Option<Waiter> {
        self.pop_front()
    }

    fn is_empty(&self) -> bool {
        VecDeque::is_empty(self)
    }

    fn prune(&mut self) {
        self.retain(|waiter| !waiter.is_closed());
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        VecDeque::len(self)
    }
}

/// Queues that take turns: the one at the front hands out one item and
/// goes to the back if it has more.
struct Ring<K, Q> {
    order: VecDeque<K>,
    queues: HashMap<K, Q>,
}

impl<K, Q> Default for Ring<K, Q> {
    fn default() -> Self {
        Self {
            order: VecDeque::new(),
            queues: HashMap::new(),
        }
    }
}

impl<K: Copy + Eq + Hash, Q: Default> Ring<K, Q> {
    /// The queue of `key`; a new one takes its turn last.
    fn of(&mut self, key: K) -> &mut Q {
        self.queues.entry(key).or_insert_with(|| {
            self.order.push_back(key);
            Q::default()
        })
    }
}

impl<K: Copy + Eq + Hash, Q: Turns> Turns for Ring<K, Q> {
    type Item = Q::Item;

    fn next(&mut self) -> Option<Q::Item> {
        let key = self.order.pop_front()?;
        let queue = self.queues.get_mut(&key)?;
        let item = queue.next();
        if queue.is_empty() {
            self.queues.remove(&key);
        } else {
            self.order.push_back(key);
        }
        item
    }

    fn is_empty(&self) -> bool {
        self.queues.is_empty()
    }

    fn prune(&mut self) {
        self.queues.retain(|_, queue| {
            queue.prune();
            !queue.is_empty()
        });
        self.order.retain(|key| self.queues.contains_key(key));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.queues.values().map(Turns::len).sum()
    }
}

#[derive(Default)]
struct State {
    free: usize,
    waiting: Ring<Option<IpAddr>, Ring<Option<SocketAddr>, VecDeque<Waiter>>>,
}

/// The permits of a receiver and those who wait for one.
pub(super) struct DecodeGate {
    state: Mutex<State>,
}

/// The right to decode, convert and write one batch; passed on to whoever
/// is next when dropped.
pub(super) struct DecodePermit {
    /// `None` once the permit has been passed on.
    gate: Option<Arc<DecodeGate>>,
}

/// A place in the queue, taken when the ticket is made.
pub(super) struct Ticket {
    gate: Arc<DecodeGate>,
    permit: oneshot::Receiver<DecodePermit>,
}

impl DecodeGate {
    pub fn new(permits: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                free: permits,
                waiting: Ring::default(),
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ask for a permit for a stream of `client`.
    pub fn request(self: &Arc<Self>, client: Client) -> Ticket {
        let (waiter, permit) = oneshot::channel();
        let mut state = self.state();
        let free = state.free > 0;
        if free {
            state.free -= 1;
        } else {
            state
                .waiting
                .of(client.peer)
                .of(client.connection)
                .push_back(waiter);
            return Ticket {
                gate: self.clone(),
                permit,
            };
        }
        drop(state);
        // The receiver is right here: this cannot fail, and if it did the
        // permit would go back through its drop.
        let _ = waiter.send(self.permit());
        Ticket {
            gate: self.clone(),
            permit,
        }
    }

    fn permit(self: &Arc<Self>) -> DecodePermit {
        DecodePermit {
            gate: Some(self.clone()),
        }
    }

    /// Pass a permit on to whoever's turn it is, or keep it for later.
    fn release(self: &Arc<Self>) {
        loop {
            let waiter = {
                let mut state = self.state();
                let Some(waiter) = state.waiting.next() else {
                    state.free += 1;
                    return;
                };
                waiter
            };
            // Handed over outside the lock: a refused permit must not be
            // dropped (and released again) while the lock is held.
            let Err(mut refused) = waiter.send(self.permit()) else {
                return;
            };
            // The waiter gave up; the permit is still ours to pass on.
            refused.gate = None;
        }
    }

    /// Permits nobody holds.
    #[cfg(test)]
    pub fn free(&self) -> usize {
        self.state().free
    }

    /// Requests that wait for a permit.
    #[cfg(test)]
    pub fn waiting(&self) -> usize {
        self.state().waiting.len()
    }
}

impl Ticket {
    /// Wait for the turn. `None` when the receiver is gone.
    pub async fn permit(mut self) -> Option<DecodePermit> {
        (&mut self.permit).await.ok()
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.permit.close();
        // A permit that was handed over but never taken goes back through
        // its own drop, here.
        if self.permit.try_recv().is_ok() {
            return;
        }
        // Given up while waiting, or served: leave no dead waiter behind.
        self.gate.state().waiting.prune();
    }
}

impl Drop for DecodePermit {
    fn drop(&mut self) {
        if let Some(gate) = self.gate.take() {
            gate.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    use super::*;

    type Waiting = Pin<Box<dyn Future<Output = Option<DecodePermit>>>>;

    fn client(peer: u8, connection: u16) -> Client {
        let ip = IpAddr::from([192, 0, 2, peer]);
        Client {
            peer: Some(ip),
            connection: Some(SocketAddr::new(ip, connection)),
        }
    }

    fn ask(gate: &Arc<DecodeGate>, who: Client) -> Waiting {
        Box::pin(gate.request(who).permit())
    }

    /// The permit, if it is this one's turn.
    fn turn(waiting: &mut Waiting) -> Option<DecodePermit> {
        let mut context = Context::from_waker(Waker::noop());
        match waiting.as_mut().poll(&mut context) {
            Poll::Ready(permit) => Some(permit.expect("the gate is alive")),
            Poll::Pending => None,
        }
    }

    /// Hand the gate's one permit round until nobody waits; who got it, in
    /// order, by the names given.
    fn order(gate: &Arc<DecodeGate>, mut queue: Vec<(&'static str, Waiting)>) -> Vec<&'static str> {
        let mut served = Vec::new();
        while !queue.is_empty() {
            let (at, permit) = (0..queue.len())
                .find_map(|at| turn(&mut queue[at].1).map(|permit| (at, permit)))
                .expect("a free permit and nobody's turn");
            served.push(queue.remove(at).0);
            let others = queue.iter_mut().filter_map(|(_, waiting)| turn(waiting));
            assert_eq!(others.count(), 0, "one permit, one turn: {served:?}");
            drop(permit);
        }
        assert_eq!(gate.waiting(), 0);
        served
    }

    #[test]
    fn free_permits_are_handed_out_at_once_and_no_more_than_there_are() {
        let gate = DecodeGate::new(2);
        let mut first = ask(&gate, client(1, 1));
        let mut second = ask(&gate, client(1, 1));
        let mut third = ask(&gate, client(1, 1));
        let one = turn(&mut first).unwrap();
        let _two = turn(&mut second).unwrap();
        assert!(turn(&mut third).is_none());
        assert_eq!((gate.free(), gate.waiting()), (0, 1));
        drop(one);
        let three = turn(&mut third).unwrap();
        assert_eq!((gate.free(), gate.waiting()), (0, 0));
        drop(three);
        assert_eq!(gate.free(), 1);
    }

    /// The case the gate exists for: 64 queued batches of one peer, on two
    /// connections, and then one batch of another peer.
    #[test]
    fn a_peer_with_many_queued_streams_does_not_stall_another() {
        let gate = DecodeGate::new(1);
        let mut holder = ask(&gate, client(9, 1));
        let held = turn(&mut holder).unwrap();
        let mut crowd: Vec<Waiting> = (0..64)
            .map(|stream| ask(&gate, client(1, 1000 + stream % 2)))
            .collect();
        let mut other = ask(&gate, client(2, 1));
        assert_eq!(gate.waiting(), 65);

        drop(held);
        // The crowd came first and gets the first turn ...
        assert!(turn(&mut other).is_none());
        let first = turn(&mut crowd[0]).expect("first come, first turn");
        drop(first);
        // ... and the other peer the second, not the sixty-fifth.
        let second = turn(&mut other).expect("the other peer's turn");
        assert_eq!(gate.waiting(), 63);
        assert!(crowd[1..].iter_mut().all(|waiting| turn(waiting).is_none()));
        drop(second);
        assert!(turn(&mut crowd[1]).is_some());
    }

    #[test]
    fn peers_take_turns_and_so_do_the_connections_of_a_peer() {
        let gate = DecodeGate::new(1);
        let mut holder = ask(&gate, client(9, 1));
        let held = turn(&mut holder).unwrap();
        let queue = vec![
            ("a1", ask(&gate, client(1, 1))),
            ("a1", ask(&gate, client(1, 1))),
            ("a1", ask(&gate, client(1, 1))),
            ("a2", ask(&gate, client(1, 2))),
            ("b1", ask(&gate, client(2, 1))),
            ("b1", ask(&gate, client(2, 1))),
            ("c1", ask(&gate, client(3, 1))),
        ];
        drop(held);
        assert_eq!(
            order(&gate, queue),
            ["a1", "b1", "c1", "a2", "b1", "a1", "a1"]
        );
        assert_eq!(gate.free(), 1);
    }

    #[test]
    fn streams_without_an_address_are_one_client_among_the_others() {
        let gate = DecodeGate::new(1);
        let mut holder = ask(&gate, Client::default());
        let held = turn(&mut holder).unwrap();
        let queue = vec![
            ("local", ask(&gate, Client::default())),
            ("local", ask(&gate, Client::default())),
            ("a", ask(&gate, client(1, 1))),
        ];
        drop(held);
        assert_eq!(order(&gate, queue), ["local", "a", "local"]);
    }

    #[test]
    fn a_waiter_that_gives_up_is_forgotten_and_passed_over() {
        let gate = DecodeGate::new(1);
        let mut holder = ask(&gate, client(1, 1));
        let held = turn(&mut holder).unwrap();
        let gone = ask(&gate, client(2, 1));
        let mut patient = ask(&gate, client(3, 1));
        assert_eq!(gate.waiting(), 2);
        drop(gone);
        assert_eq!(gate.waiting(), 1, "no dead waiter is kept");
        drop(held);
        let permit = turn(&mut patient).expect("the next live waiter");
        drop(permit);
        assert_eq!((gate.free(), gate.waiting()), (1, 0));
    }

    #[test]
    fn a_permit_handed_to_a_waiter_that_never_takes_it_comes_back() {
        let gate = DecodeGate::new(1);
        let mut holder = ask(&gate, client(1, 1));
        let held = turn(&mut holder).unwrap();
        let unread = ask(&gate, client(2, 1));
        let mut next = ask(&gate, client(3, 1));
        // The permit is handed to `unread`, which is dropped unpolled.
        drop(held);
        assert_eq!(gate.free(), 0);
        assert!(turn(&mut next).is_none());
        drop(unread);
        assert!(turn(&mut next).is_some(), "the permit was passed on");
        assert_eq!(gate.free(), 1);
    }
}
