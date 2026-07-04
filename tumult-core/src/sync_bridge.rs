//! Bridge for driving async futures from synchronous code inside a Tokio
//! runtime.

use std::future::Future;

/// Run `fut` to completion from synchronous code that is executing inside a
/// running Tokio runtime (e.g. a sync trait method called from an async
/// context).
///
/// A bare `Handle::current().block_on(...)` panics (or deadlocks) when called
/// from inside an already-running Tokio task. `tokio::task::block_in_place`
/// moves the calling thread out of the async worker pool first, making the
/// nested `block_on` safe — but only on the **multi-threaded** runtime.
///
/// # Panics
///
/// Panics if called outside a Tokio runtime, or on the `current_thread`
/// scheduler (use `#[tokio::test(flavor = "multi_thread")]` in tests).
pub fn sync_await<F: Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}
