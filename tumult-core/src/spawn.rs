//! One spawn of a child process at a time.
//!
//! On macOS the standard library makes a pipe in two steps: it opens the
//! pipe, then marks it to close in a child. A child that another thread
//! spawns between the two steps keeps a copy of the pipe. While that child
//! lives, the reader of the pipe never sees its end: a probe whose output
//! pipe went to a load tool that runs for a minute hangs until its timeout.
//! Linux opens and marks a pipe in one step.
//!
//! The process steps and the load tools spawn their children through
//! [`alone`], so no spawn of one falls between the two steps of another.

use std::sync::{Mutex, PoisonError};

/// Held while a child process is spawned.
static SPAWN: Mutex<()> = Mutex::new(());

/// Run `spawn` while no other spawn of [`alone`] runs, and give its result.
/// `spawn` should only start the child: the next spawn waits for it.
pub fn alone<T>(spawn: impl FnOnce() -> T) -> T {
    let _alone = SPAWN.lock().unwrap_or_else(PoisonError::into_inner);
    spawn()
}
