//! Bridge for driving async futures from synchronous code, whether or not the
//! caller is inside a Tokio runtime.

use std::future::Future;

/// Run `fut` to completion from synchronous code.
///
/// Native plugin executors are async, but the experiment runner drives probes
/// and actions from synchronous code that may be:
///   1. inside a multi-threaded Tokio runtime (e.g. the MCP server, or the
///      CLI's `#[tokio::main]` thread), or
///   2. on a plain OS thread with no runtime at all (the runner executes
///      activities inside `std::thread::scope`, whose threads do not inherit
///      the parent's runtime).
///
/// Case 1 needs `block_in_place` to move off the async worker before the nested
/// `block_on`; case 2 has no runtime to borrow, so a bare `Handle::current()`
/// would panic. This helper handles both: it uses the current runtime handle
/// when one exists, and otherwise spins up a temporary current-thread runtime.
///
/// # Panics
///
/// Panics only if a temporary Tokio runtime cannot be constructed (case 2),
/// which in practice means the OS denied a thread/timer resource.
pub fn sync_await<F: Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        // Inside a runtime: block_in_place parks this worker so the nested
        // block_on is safe on the multi-threaded scheduler.
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        // No runtime on this thread (e.g. a scoped std::thread in the runner):
        // drive the future on a throwaway current-thread runtime.
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build temporary Tokio runtime")
            .block_on(fut),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn answer() -> u32 {
        tokio::task::yield_now().await;
        42
    }

    #[test]
    fn drives_future_with_no_ambient_runtime() {
        // A plain test thread has no runtime — the runner's scoped-thread case.
        assert_eq!(sync_await(answer()), 42);
    }

    #[test]
    fn drives_future_inside_multi_thread_runtime() {
        // The MCP-server / CLI case: a sync call nested inside a running
        // multi-threaded runtime must not panic.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let out = rt.block_on(async {
            tokio::task::spawn_blocking(|| sync_await(answer()))
                .await
                .unwrap()
        });
        assert_eq!(out, 42);
    }
}
