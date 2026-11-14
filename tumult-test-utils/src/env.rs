//! Serialised, self-restoring changes to process environment variables.

use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// One lock for every test in the process that touches the environment.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Exclusive access to the process environment for the lifetime of the guard.
///
/// Environment variables are process-wide, and `cargo test` runs the tests of
/// one binary on several threads. A test that sets or removes a variable takes
/// this guard first: it blocks other guarded tests until it is dropped, and on
/// drop it puts back every variable it changed.
#[must_use = "the environment is restored when the guard is dropped"]
pub struct EnvGuard {
    /// Original value of each changed variable, in the order first changed.
    saved: Vec<(OsString, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Waits for exclusive access to the environment.
    ///
    /// The lock is not re-entrant: a thread that already holds a guard and
    /// asks for a second one waits for itself and never returns. Take one
    /// guard per test and pass `&mut EnvGuard` to helpers that change more
    /// variables.
    ///
    /// A test that panicked while holding the guard has already restored its
    /// changes on unwind, so a poisoned lock is safe to take over.
    pub fn lock() -> Self {
        Self {
            saved: Vec::new(),
            _lock: ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner),
        }
    }

    /// Sets `key` to `value` until the guard is dropped.
    pub fn set(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.remember(key.as_ref());
        std::env::set_var(key, value);
        self
    }

    /// Removes `key` until the guard is dropped.
    pub fn remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.remember(key.as_ref());
        std::env::remove_var(key);
        self
    }

    fn remember(&mut self, key: &OsStr) {
        if self.saved.iter().any(|(saved, _)| saved == key) {
            return;
        }
        self.saved.push((key.to_owned(), std::env::var_os(key)));
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, original) in self.saved.drain(..) {
            match original {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use super::*;

    // Each test owns its variable names, so reading them after the guard is
    // dropped cannot race with another test.
    const SET_BEFORE: &str = "TUMULT_TEST_UTILS_ENV_GUARD_SET_BEFORE";
    const UNSET_BEFORE: &str = "TUMULT_TEST_UTILS_ENV_GUARD_UNSET_BEFORE";

    #[test]
    fn changes_are_visible_while_held_and_undone_on_drop() {
        std::env::set_var(SET_BEFORE, "original");
        std::env::remove_var(UNSET_BEFORE);

        let mut env = EnvGuard::lock();
        env.set(SET_BEFORE, "first").set(UNSET_BEFORE, "temporary");
        // A second change to the same variable must not overwrite the saved
        // original with the intermediate value.
        env.set(SET_BEFORE, "second");
        assert_eq!(std::env::var(SET_BEFORE).as_deref(), Ok("second"));
        assert_eq!(std::env::var(UNSET_BEFORE).as_deref(), Ok("temporary"));
        env.remove(SET_BEFORE);
        assert_eq!(std::env::var_os(SET_BEFORE), None);
        drop(env);

        assert_eq!(std::env::var(SET_BEFORE).as_deref(), Ok("original"));
        assert_eq!(std::env::var_os(UNSET_BEFORE), None);
        std::env::remove_var(SET_BEFORE);
    }

    #[test]
    fn changes_are_undone_when_the_holder_panics() {
        const KEY: &str = "TUMULT_TEST_UTILS_ENV_GUARD_PANIC";
        let result = std::thread::spawn(|| {
            let mut env = EnvGuard::lock();
            env.set(KEY, "leaked");
            panic!("holder fails while the variable is set");
        })
        .join();
        assert!(result.is_err());

        // The lock is poisoned now; the next guard still gets in.
        let _env = EnvGuard::lock();
        assert_eq!(std::env::var_os(KEY), None);
    }

    #[test]
    fn a_second_guard_waits_until_the_first_is_dropped() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (about_to_lock, waiting) = mpsc::channel();

        let first = EnvGuard::lock();
        let second = {
            let events = Arc::clone(&events);
            std::thread::spawn(move || {
                about_to_lock.send(()).unwrap();
                let _env = EnvGuard::lock();
                events.lock().unwrap().push("second acquired");
            })
        };
        waiting.recv().unwrap();
        // Give the other thread every chance to get past a lock that does not
        // exclude. Yielding is not a wait: the order below holds regardless.
        for _ in 0..10_000 {
            std::thread::yield_now();
        }
        events.lock().unwrap().push("first released");
        drop(first);
        second.join().unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            ["first released", "second acquired"]
        );
    }
}
