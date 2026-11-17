//! Behavior tests for the `script` provider dispatch: discovery failures,
//! action/probe resolution, argument-to-env mapping, and script exit-status
//! handling. A temporary plugin tree is published through
//! `TUMULT_PLUGIN_PATH` under the shared environment guard, which restores
//! the variable when the test ends.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;

use tumult_core::runner::ActivityExecutor;
use tumult_core::types::{Activity, ActivityType, Provider};
use tumult_exec::ProviderExecutor;

struct TempPluginTree(PathBuf);

impl TempPluginTree {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("tumult-exec-script-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

impl Drop for TempPluginTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn script_activity(
    plugin: &str,
    function: &str,
    arguments: HashMap<String, serde_json::Value>,
) -> Activity {
    Activity {
        name: "script-test".into(),
        activity_type: ActivityType::Action,
        provider: Provider::Script {
            plugin: plugin.into(),
            function: function.into(),
            arguments,
            timeout_s: Some(10.0),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
        label_selector: None,
    }
}

#[test]
fn script_provider_scenarios() {
    let tree = TempPluginTree::new();
    tree.write(
        "cov-test-plugin/plugin.toon",
        "name: cov-test-plugin\n\
         version: 0.1.0\n\
         description: Coverage test plugin\n\
         actions[3]:\n\
         \x20 - name: greet\n\
         \x20   script: actions/greet.sh\n\
         \x20   description: Echo the greeting argument\n\
         \x20 - name: fail-with-stderr\n\
         \x20   script: actions/fail-with-stderr.sh\n\
         \x20   description: Exit non-zero with stderr\n\
         \x20 - name: missing-script\n\
         \x20   script: actions/not-on-disk.sh\n\
         \x20   description: Points at a script that does not exist\n\
         probes[1]:\n\
         \x20 - name: health\n\
         \x20   script: probes/health.sh\n\
         \x20   description: Probe entry\n",
    );
    tree.write(
        "cov-test-plugin/actions/greet.sh",
        "sleep 0.08\necho \"$TUMULT_GREETING:$TUMULT_COUNT\"\n",
    );
    tree.write(
        "cov-test-plugin/actions/fail-with-stderr.sh",
        "echo script-broke >&2\nexit 2\n",
    );
    tree.write("cov-test-plugin/probes/health.sh", "echo probe-ok\n");
    tree.write(
        "cov-quiet-plugin/plugin.toon",
        "name: cov-quiet-plugin\n\
         version: 0.1.0\n\
         description: Plugin whose action fails silently\n\
         actions[1]:\n\
         \x20 - name: fail-quietly\n\
         \x20   script: actions/fail-quietly.sh\n\
         \x20   description: Exit non-zero without any output\n\
         probes[0]:\n",
    );
    tree.write("cov-quiet-plugin/actions/fail-quietly.sh", "exit 3\n");

    // Publish the tree through the custom search path.
    let mut env = tumult_test_utils::EnvGuard::lock();
    env.set("TUMULT_PLUGIN_PATH", &tree.0);
    let executor = ProviderExecutor::new();

    // Unknown plugin names what is available instead of failing silently.
    let outcome = executor.execute(&script_activity(
        "no-such-plugin-xyz",
        "greet",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("unknown script plugin: no-such-plugin-xyz"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("cov-test-plugin"),
        "error must list available plugins: {error}"
    );

    // Unknown function on a known plugin names the available entries.
    let outcome = executor.execute(&script_activity(
        "cov-test-plugin",
        "no-such-action",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("unknown cov-test-plugin action: no-such-action"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("greet") && error.contains("health"),
        "error must list available actions and probes: {error}"
    );

    // Arguments reach the script as TUMULT_* env vars; non-string values use
    // their JSON representation.
    let arguments = HashMap::from([
        (
            "greeting".to_string(),
            serde_json::Value::String("hi".to_string()),
        ),
        ("count".to_string(), serde_json::json!(3)),
    ]);
    let outcome = executor.execute(&script_activity("cov-test-plugin", "greet", arguments));
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.output.as_deref(), Some("hi:3"));
    // A lower bound only: the script sleeps 80ms and a sleep never returns
    // early, so this cannot fail on a slow machine.
    assert!(
        outcome.duration_ms >= 60,
        "script duration must include execution: {}",
        outcome.duration_ms
    );

    // A function missing from actions falls back to the manifest's probes.
    let outcome = executor.execute(&script_activity(
        "cov-test-plugin",
        "health",
        HashMap::new(),
    ));
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.output.as_deref(), Some("probe-ok"));

    // Non-zero exit with stderr surfaces the stderr text.
    let outcome = executor.execute(&script_activity(
        "cov-test-plugin",
        "fail-with-stderr",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    assert_eq!(outcome.error.as_deref(), Some("script-broke"));

    // Non-zero exit without stderr names the script and its exit code.
    let outcome = executor.execute(&script_activity(
        "cov-quiet-plugin",
        "fail-quietly",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("fail-quietly.sh") && error.contains("exit code 3"),
        "unexpected error: {error}"
    );

    // A manifest pointing at a missing script reports the executor error.
    let outcome = executor.execute(&script_activity(
        "cov-test-plugin",
        "missing-script",
        HashMap::new(),
    ));
    assert!(!outcome.success);
    let error = outcome.error.unwrap();
    assert!(
        error.contains("script not found"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_stop_ends_a_script_and_the_child_it_started() {
    let mut env = tumult_test_utils::EnvGuard::lock();
    let plugins = tempfile::TempDir::new().unwrap();
    let plugin = plugins.path().join("stop-test-plugin");
    std::fs::create_dir_all(plugin.join("actions")).unwrap();
    std::fs::write(
        plugin.join("plugin.toon"),
        "name: stop-test-plugin\n\
         version: 0.1.0\n\
         description: Plugin whose action runs for a long time\n\
         actions[1]:\n\
         \x20 - name: hang\n\
         \x20   script: actions/hang.sh\n\
         \x20   description: Start a child, then run for a long time\n\
         probes[0]:\n",
    )
    .unwrap();
    std::fs::write(
        plugin.join("actions/hang.sh"),
        "( sleep 0.6; touch \"$TUMULT_MARKER\" ) &\nsleep 30\n",
    )
    .unwrap();
    env.set("TUMULT_PLUGIN_PATH", plugins.path());
    let executor = ProviderExecutor::new();

    let marker = plugins.path().join("child-survived");
    let arguments = HashMap::from([(
        "marker".to_string(),
        serde_json::Value::String(marker.display().to_string()),
    )]);
    let stop = tokio_util::sync::CancellationToken::new();
    let trigger = stop.clone();
    let stopper = std::thread::spawn(move || {
        // sleep: the stop comes while the script runs, not before it.
        std::thread::sleep(std::time::Duration::from_millis(200));
        trigger.cancel();
    });
    let started = std::time::Instant::now();
    let outcome = executor.execute_until(
        &script_activity("stop-test-plugin", "hang", arguments),
        &stop,
    );
    stopper.join().unwrap();
    assert!(!outcome.success);
    assert_eq!(
        outcome.error.as_deref(),
        Some("script stopped before it finished")
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the stop waited for the script: {:?}",
        started.elapsed()
    );
    // sleep: there is no event for a file that must stay absent; wait past
    // the time the child would have written it.
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert!(!marker.exists(), "a child of the script outlived the stop");
}

#[test]
fn a_timeout_that_is_not_a_number_of_seconds_fails_the_step_without_a_panic() {
    let mut env = tumult_test_utils::EnvGuard::lock();
    let plugins = tempfile::TempDir::new().unwrap();
    let plugin = plugins.path().join("timeout-test-plugin");
    std::fs::create_dir_all(plugin.join("actions")).unwrap();
    std::fs::write(
        plugin.join("plugin.toon"),
        "name: timeout-test-plugin\n\
         version: 0.1.0\n\
         description: Plugin whose action leaves a mark\n\
         actions[1]:\n\
         \x20 - name: mark\n\
         \x20   script: actions/mark.sh\n\
         \x20   description: Write the marker file\n\
         probes[0]:\n",
    )
    .unwrap();
    std::fs::write(plugin.join("actions/mark.sh"), "touch \"$TUMULT_MARKER\"\n").unwrap();
    env.set("TUMULT_PLUGIN_PATH", plugins.path());
    env.set("HOME", plugins.path());
    let executor = ProviderExecutor::new();

    let marker = plugins.path().join("script-ran");
    let arguments = HashMap::from([(
        "marker".to_string(),
        serde_json::Value::String(marker.display().to_string()),
    )]);
    for seconds in [-1.0, f64::NAN, f64::INFINITY, 1e20] {
        let mut activity = script_activity("timeout-test-plugin", "mark", arguments.clone());
        let Provider::Script { timeout_s, .. } = &mut activity.provider else {
            unreachable!("script_activity builds a script step");
        };
        *timeout_s = Some(seconds);

        let outcome = executor.execute(&activity);

        assert!(!outcome.success, "timeout {seconds}: the step succeeded");
        let error = outcome.error.unwrap_or_default();
        assert!(
            error.starts_with("invalid timeout"),
            "timeout {seconds}: unexpected error: {error}"
        );
    }
    assert!(!marker.exists(), "a step with a bad timeout ran its script");
}
