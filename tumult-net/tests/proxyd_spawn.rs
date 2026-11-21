//! The chaos proxy starts only when no other spawn runs.
//!
//! On macOS the standard library makes a pipe in two steps. A daemon that
//! starts between the two steps of a step that makes its pipes keeps a copy
//! of them, and that step then reads its output until the proxy is stopped.
#![cfg(unix)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use tumult_net::actions::start_proxy;
use tumult_net::FaultProfile;
use tumult_test_utils::EnvGuard;

/// A directory of this test's own, removed with everything in it.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leftover only costs temp space.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_proxy_starts_only_when_no_other_spawn_runs() {
    let mut env = EnvGuard::lock();
    let scratch =
        Scratch(std::env::temp_dir().join(format!("tumult-net-test-spawn-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).expect("create the scratch directory");
    let marker = scratch.0.join("started");
    let daemon = scratch.0.join("proxyd");
    let body = format!("#!/bin/sh\ntouch '{}'\n", marker.display());
    tumult_test_utils::write_executable(&daemon, &body);
    env.set("TUMULT_NET_PROXYD", &daemon)
        .set("TMPDIR", &scratch.0)
        .remove("XDG_RUNTIME_DIR");
    let listen: SocketAddr = "127.0.0.1:1".parse().expect("listen address");
    let upstream: SocketAddr = "127.0.0.1:9".parse().expect("upstream address");

    let started = tumult_spawn::alone(|| {
        let started = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            runtime.block_on(start_proxy(listen, upstream, FaultProfile::default()))
        });
        // sleep: there is no event for a file that must stay absent; wait
        // past the time the daemon would need to start.
        std::thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists(), "the proxy started during another spawn");
        started
    });

    let outcome = started.join().expect("the start ends");
    assert!(marker.exists(), "the proxy did not start");
    let error = outcome.expect_err("a daemon that exits at once does not serve");
    assert!(
        error.to_string().contains("exited before it served"),
        "{error}"
    );
}
