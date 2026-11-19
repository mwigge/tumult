//! A proxy is found again through the pidfile its daemon writes once it
//! serves. A second start on one listen address is refused, so the first
//! proxy can still be stopped.
//!
//! The process checks below are those of a Unix host.
#![cfg(unix)]

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tumult_net::actions::{start_proxy, stop_proxy};
use tumult_net::{FaultProfile, ProxySpec};
use tumult_test_utils::EnvGuard;

const PROXYD: &str = env!("CARGO_BIN_EXE_tumult-net-proxyd");

/// A directory of this test's own, removed with everything in it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "tumult-net-test-pidfile-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self(path)
    }

    /// Points the actions at the daemon of this build and keeps the
    /// pidfile and the log in this directory.
    fn isolate(&self, env: &mut EnvGuard) {
        env.set("TUMULT_NET_PROXYD", PROXYD)
            .set("TMPDIR", &self.0)
            .remove("XDG_RUNTIME_DIR");
    }

    fn pidfile(&self, listen: SocketAddr) -> PathBuf {
        self.0.join(format!("tumult-net-{}.pid", key(listen)))
    }

    fn log(&self, listen: SocketAddr) -> PathBuf {
        self.0
            .join(format!("tumult-net-{}.log.d", key(listen)))
            .join("stderr.log")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leftover only costs temp space.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The listen address as the pidfile name carries it.
fn key(listen: SocketAddr) -> String {
    listen
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A loopback address nothing is bound to (bound once, then released).
fn free_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener.local_addr().expect("local addr")
}

/// An upstream that sends back what it gets, on threads of its own.
fn echo_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the upstream");
    let addr = listener.local_addr().expect("upstream addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                return;
            };
            std::thread::spawn(move || {
                let mut buf = [0u8; 64];
                while let Ok(n @ 1..) = stream.read(&mut buf) {
                    if stream.write_all(&buf[..n]).is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// What comes back when `bytes` are sent through `listen`.
fn round_trip(listen: SocketAddr, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect_timeout(&listen, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(bytes)?;
    let mut back = vec![0u8; bytes.len()];
    stream.read_exact(&mut back)?;
    Ok(back)
}

/// Whether nothing accepts connections on `addr` any more, asked until it
/// is so or two seconds have passed.
fn goes_quiet(addr: SocketAddr) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
            return true;
        }
        // sleep: a killed daemon closes its socket a moment later.
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// The pid a pidfile names, if it names one.
fn pid_in_file(pidfile: &Path) -> Option<u32> {
    std::fs::read_to_string(pidfile).ok()?.trim().parse().ok()
}

/// The pid a message names as `(pid N)`.
fn pid_in_text(text: &str) -> Option<u32> {
    let (_, rest) = text.split_once("(pid ")?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Whether `pid` is a process of the host, a zombie included.
fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The pid of a process that has ended and is reaped.
fn dead_pid() -> u32 {
    let mut child = Command::new("true").spawn().expect("spawn true");
    let pid = child.id();
    child.wait().expect("reap true");
    pid
}

/// The file a path names, as device and inode.
fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

#[tokio::test]
async fn a_second_start_on_one_address_is_refused_and_the_first_proxy_can_still_be_stopped() {
    let mut env = EnvGuard::lock();
    let scratch = Scratch::new("second-start");
    scratch.isolate(&mut env);
    let upstream = echo_upstream();
    let listen = free_addr();
    let pidfile = scratch.pidfile(listen);
    let log = scratch.log(listen);

    let first = start_proxy(listen, upstream, FaultProfile::default())
        .await
        .expect("the first start");
    let first_pid = pid_in_file(&pidfile).expect("the first start leaves a pidfile");
    let first_log = file_id(&log);
    let slower = FaultProfile {
        delay_ms: 50,
        ..FaultProfile::default()
    };
    let second = start_proxy(listen, upstream, slower).await;

    // Looked at before the first proxy is stopped, asserted after: a failed
    // assertion must not leave a daemon running.
    let named_after = pid_in_file(&pidfile);
    let log_after = file_id(&log);
    let forwarded = round_trip(listen, b"still here");
    let stopped = stop_proxy(listen).await;
    let quiet = goes_quiet(listen);
    if !quiet {
        // What a lost proxy needs: the stop did not reach it.
        let _ = Command::new("kill").arg(first_pid.to_string()).status();
    }

    assert!(first.contains(&format!("(pid {first_pid})")), "{first}");
    let refused = second
        .expect_err("a second start on one address is refused")
        .to_string();
    assert!(
        refused.contains(&format!("(pid {first_pid})")),
        "the error names the running proxy: {refused}"
    );
    assert!(
        refused.contains(&listen.to_string()),
        "the error names the address: {refused}"
    );
    assert_eq!(
        named_after,
        Some(first_pid),
        "the pidfile still names the first proxy"
    );
    assert!(first_log.is_some(), "the first proxy has a log");
    assert_eq!(
        log_after, first_log,
        "the log of the first proxy is not replaced"
    );
    assert_eq!(
        forwarded.expect("the first proxy still forwards"),
        b"still here"
    );
    let stopped = stopped.expect("the stop");
    assert!(
        stopped.contains(&format!("stopped (pid {first_pid})")),
        "{stopped}"
    );
    assert!(quiet, "nothing listens on {listen} after the stop");
}

#[tokio::test]
async fn a_start_goes_on_over_a_pidfile_that_names_no_live_proxy() {
    let mut env = EnvGuard::lock();
    let scratch = Scratch::new("stale-pidfile");
    scratch.isolate(&mut env);
    let upstream = echo_upstream();
    // A pid that is gone, and on Linux also a live process that is no proxy
    // daemon: this test.
    let mut stale = vec![dead_pid()];
    if cfg!(target_os = "linux") {
        stale.push(std::process::id());
    }

    for stale_pid in stale {
        let listen = free_addr();
        let pidfile = scratch.pidfile(listen);
        std::fs::write(&pidfile, stale_pid.to_string()).expect("leave a stale pidfile");

        let started = start_proxy(listen, upstream, FaultProfile::default()).await;
        // Looked at before the proxy is stopped, asserted after: a failed
        // assertion must not leave a daemon running.
        let named = pid_in_file(&pidfile);
        let forwarded = round_trip(listen, b"fresh");
        let stopped = stop_proxy(listen).await;
        let quiet = goes_quiet(listen);

        let started = started.expect("a stale pidfile does not refuse the start");
        let pid = pid_in_text(&started).expect("the start names its pid");
        assert_ne!(pid, stale_pid, "{started}");
        assert_eq!(named, Some(pid), "the pidfile names the new proxy");
        assert_eq!(forwarded.expect("the new proxy forwards"), b"fresh");
        let stopped = stopped.expect("the stop");
        assert!(
            stopped.contains(&format!("stopped (pid {pid})")),
            "{stopped}"
        );
        assert!(quiet, "nothing listens on {listen} after the stop");
    }
}

#[tokio::test]
async fn a_start_on_a_port_another_program_holds_fails_and_leaves_nothing_behind() {
    let mut env = EnvGuard::lock();
    let scratch = Scratch::new("port-held");
    scratch.isolate(&mut env);
    let held = TcpListener::bind("127.0.0.1:0").expect("hold a port");
    let listen = held.local_addr().expect("held addr");

    let started = start_proxy(listen, free_addr(), FaultProfile::default()).await;
    let pidfile_left = scratch.pidfile(listen).exists();
    if started.is_ok() {
        // A start that should have failed: stop what it started, so a
        // failed assertion leaves no daemon behind.
        let _ = stop_proxy(listen).await;
    }

    let error = started
        .expect_err("a daemon that cannot bind fails the start")
        .to_string();
    assert!(error.contains("exited"), "{error}");
    assert!(
        error.contains("Address already in use"),
        "the daemon's own words are in the error: {error}"
    );
    assert!(!pidfile_left, "no pidfile is left");
    let pid = pid_in_text(&error).expect("the error names the daemon's pid");
    assert!(!process_exists(pid), "the daemon {pid} is reaped");
}

#[test]
fn the_daemon_names_itself_in_its_pidfile_once_it_serves() {
    // Held so the scratch directory is not made inside that of another
    // test, which points `TMPDIR` at its own and removes it.
    let _env = EnvGuard::lock();
    let scratch = Scratch::new("daemon-writes");
    let listen = free_addr();
    let spec = ProxySpec {
        listen,
        upstream: free_addr(),
        profile: FaultProfile::default(),
        pidfile: scratch.0.join("proxyd.pid"),
    };
    let mut daemon = Command::new(PROXYD)
        .args(spec.to_argv())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the daemon");
    let pid = daemon.id();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut named = pid_in_file(&spec.pidfile);
    while named != Some(pid) && Instant::now() < deadline {
        // sleep: interval of the bounded wait for the pidfile.
        std::thread::sleep(Duration::from_millis(20));
        named = pid_in_file(&spec.pidfile);
    }
    let serves = TcpStream::connect_timeout(&listen, Duration::from_secs(1)).is_ok();
    let mut beside: Vec<String> = std::fs::read_dir(&scratch.0)
        .expect("read the scratch directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    beside.sort();
    let _ = daemon.kill();
    let _ = daemon.wait();

    assert_eq!(named, Some(pid), "the pidfile names the daemon");
    assert!(serves, "the daemon serves {listen} by the time it is named");
    assert_eq!(beside, ["proxyd.pid"], "nothing else is left beside it");
}
