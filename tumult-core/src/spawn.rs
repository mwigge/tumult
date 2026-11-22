//! One spawn of a child process at a time.
//!
//! The lock lives in `tumult-spawn`, so the crates that do not depend on
//! this one take the same lock. See [`alone`].

pub use tumult_spawn::alone;
