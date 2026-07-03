//! Directional adapter-stack builder over `tokio-netem`.
//!
//! The `tokio-netem` write adapters are directional (they implement only
//! `AsyncWrite`), so faults are applied on the egress half of each proxy pipe.
//! The stack is layered so that, from the socket outward, latency is applied
//! first (closest to the wire), then rate limiting, then fragmentation, then
//! byte corruption, with probabilistic termination outermost — a kill aborts
//! the whole stack.

use std::sync::Arc;

use tokio::io::AsyncWrite;
use tokio_netem::corrupter::Corrupter;
use tokio_netem::io::NetEmWriteExt;
use tokio_netem::terminator::Terminator;

use crate::handles::FaultHandles;

/// Wrap an egress writer with the full `delay → throttle → slice → corrupt →
/// terminate` adapter stack.
///
/// `terminate_prob` is a static per-write hard-close probability, and `seed`
/// makes the corruption and termination RNGs reproducible across runs.
pub(crate) fn wrap_writer<W>(
    writer: W,
    handles: &FaultHandles,
    terminate_prob: f64,
    seed: [u8; 32],
) -> impl AsyncWrite + Unpin + Send
where
    W: AsyncWrite + Unpin + Send,
{
    let delayed = writer.delay_writes_dyn(Arc::clone(&handles.delay));
    let throttled = delayed.throttle_writes_dyn(Arc::clone(&handles.rate));
    let sliced = throttled.slice_writes_dyn(Arc::clone(&handles.slice));
    let corrupted = Corrupter::from_seed(sliced, Arc::clone(&handles.corrupt), seed);
    Terminator::from_seed(corrupted, terminate_prob, seed)
}

#[cfg(test)]
mod tests {
    use super::wrap_writer;
    use crate::config::FaultProfile;
    use crate::handles::FaultHandles;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn passthrough_writer_forwards_bytes_unchanged() {
        let (mut client, server) = tokio::io::duplex(64);
        let profile = FaultProfile::default();
        let handles = FaultHandles::from_profile(&profile).expect("handles");

        let mut faulted = wrap_writer(server, &handles, 0.0, profile.seed_bytes());
        faulted.write_all(b"hello").await.expect("write");
        faulted.flush().await.expect("flush");
        drop(faulted);

        let mut buf = vec![0u8; 5];
        client.read_exact(&mut buf).await.expect("read");
        assert_eq!(&buf, b"hello");
    }
}
