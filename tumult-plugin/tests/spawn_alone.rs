//! A script step starts only when no other spawn runs.
//!
//! On macOS the standard library makes a pipe in two steps: it opens the
//! pipe, then marks it to close in a child. A child that another thread
//! spawns between the two steps keeps a copy of the pipe. The step then
//! reads its output until that child ends, and a load window runs for a
//! minute. Linux makes a pipe in one step, so there the race shows only
//! when `pipe2` is made to work as on macOS.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tumult_plugin::executor::{execute_script_until, ExecutorError, ScriptResult};

/// Run the script at `script` of the plugin in `root` to its end.
fn run_script(
    runtime: &tokio::runtime::Runtime,
    script: &Path,
    root: &Path,
) -> Result<ScriptResult, ExecutorError> {
    runtime.block_on(execute_script_until(
        script,
        root,
        &HashMap::<String, String>::new(),
        Some(Duration::from_secs(5)),
        std::future::pending(),
    ))
}

#[test]
fn a_script_starts_only_when_no_other_spawn_runs() {
    let dir = tempfile::tempdir().expect("temp dir");
    let marker = dir.path().join("started");
    let script = dir.path().join("step.sh");
    let body = format!("#!/bin/sh\ntouch '{}'\n", marker.display());
    tumult_test_utils::write_executable(&script, &body);
    let root = dir.path().to_path_buf();

    let ran = tumult_spawn::alone(|| {
        let ran = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            run_script(&runtime, &script, &root)
        });
        // sleep: there is no event for a file that must stay absent; wait
        // past the time the script would need to start.
        std::thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists(), "the script started during another spawn");
        ran
    });

    let result = ran.join().expect("the step ends").expect("the script runs");
    assert!(result.succeeded(), "{result:?}");
    assert!(marker.exists(), "the script did not run");
}

/// The stand-in of a load window runs for 20 s and gets the arguments of
/// an oha window. It starts while the script step starts.
#[test]
fn a_script_does_not_wait_for_a_window_that_starts_at_the_same_time() {
    let dir = tempfile::tempdir().expect("temp dir");
    let oha = dir.path().join("oha");
    tumult_test_utils::write_executable(&oha, "#!/bin/sh\nexec sleep 20\n");
    let script = dir.path().join("step.sh");
    tumult_test_utils::write_executable(&script, "#!/bin/sh\necho done\n");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    let (starting, started) = std::sync::mpsc::channel();
    let load = std::thread::spawn(move || {
        starting.send(()).expect("the test waits for the window");
        tumult_spawn::alone(|| {
            Command::new(&oha)
                .arg("--no-tui")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        })
    });
    started.recv().expect("the window starts");
    let began = Instant::now();
    let result = run_script(&runtime, &script, dir.path());
    let took = began.elapsed();

    let mut window = load.join().expect("the load ends").expect("oha starts");
    let _ = window.kill();
    let _ = window.wait();
    assert_eq!(result.expect("the script runs").stdout.trim(), "done");
    assert!(
        took < Duration::from_secs(10),
        "the step waited {took:?} for a window it did not start"
    );
}
