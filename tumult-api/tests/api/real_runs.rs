//! Runs over the API whose steps are real processes.
//!
//! The server here has the executor of the daemon, so a step is an `sh`
//! command and a stop has a process to end. The steps leave files in a
//! directory of the test, and the store is read after the run.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use tumult_lake::Store;

use crate::common::*;
use crate::runs::{await_terminal_run, enqueue_approved, register_toon};

/// A directory the steps of one run write to.
struct Lab(tempfile::TempDir);

impl Lab {
    fn new() -> Self {
        Self(tempfile::TempDir::new().expect("lab directory"))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    fn has(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// A step that runs `command` in the lab.
    fn step(&self, name: &str, command: &str, timeout_s: f64) -> String {
        format!(
            r#"  - name: {name}
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cd {lab} || exit 1; {command}"
      timeout_s: {timeout_s:.1}
"#,
            lab = self.0.path().display()
        )
    }

    /// A step that would run for a minute, a step after it, and a rollback.
    /// The long step leaves `started`, and the process id of its `sleep`.
    fn long_run(&self) -> String {
        format!(
            "title: a stop ends a real process\n\
             description: the first step runs for a minute\n\
             method[2]:\n{long}{next}rollbacks[1]:\n{undo}",
            long = self.step(
                "long-step",
                "sleep 60 & echo $! > sleep.pid; touch started; wait",
                90.0
            ),
            next = self.step("next-step", "touch next-step-ran", 5.0),
            undo = self.step("undo", "touch rolled-back", 5.0),
        )
    }

    /// Whether the `sleep` of the long step is still there.
    fn sleep_is_running(&self) -> bool {
        let pid = std::fs::read_to_string(self.path("sleep.pid")).expect("sleep.pid");
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("kill -0")
            .success()
    }
}

/// Wait until the run has left the file `path`. A run that does not get
/// that far fails the test with what the API says about it.
async fn wait_for(srv: &TestServer, run_id: &str, path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        if Instant::now() > deadline {
            let (_, detail) = get(&srv.base, &format!("/api/runs/{run_id}")).await;
            panic!("{} never came: {detail}", path.display());
        }
        // sleep: interval of the bounded poll for a file of another process.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The steps the store holds for the experiment of a run, in their order.
fn stored_steps(srv: &TestServer, experiment_id: &str) -> Vec<Value> {
    let reader = Store::at(&srv.db_path).read_only().expect("reader");
    reader
        .query_json_rows(&format!(
            "SELECT name, status, error, phase FROM activity_results \
             WHERE experiment_id = '{experiment_id}' ORDER BY started_at_ns"
        ))
        .expect("stored steps")
}

fn step<'a>(steps: &'a [Value], name: &str) -> &'a Value {
    steps
        .iter()
        .find(|step| step["name"] == name)
        .unwrap_or_else(|| panic!("no step {name} in {steps:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_over_the_api_ends_the_process_that_runs() {
    let lab = Lab::new();
    let definition = lab.long_run();
    let srv = spawn_server_with_real_executor(&[&definition]).await;
    let registry_id = register_toon(&srv.base, &definition).await;
    let run_id = enqueue_approved(&srv.base, &registry_id).await;
    wait_for(&srv, &run_id, &lab.path("started")).await;

    let stopped_at = Instant::now();
    let response = reqwest::Client::new()
        .post(format!("{}/api/runs/{run_id}/stop", srv.base))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status().as_u16(), 200);
    let detail = await_terminal_run(&srv.base, &run_id).await;

    assert!(
        stopped_at.elapsed() < Duration::from_secs(30),
        "the stop waited for the step: {:?}",
        stopped_at.elapsed()
    );
    assert_eq!(detail["run"]["state"], "aborted", "{detail}");
    assert_eq!(detail["run"]["rollback_status"], "completed", "{detail}");
    assert!(!lab.sleep_is_running(), "the process of the step lives on");
    assert!(lab.has("rolled-back"), "the rollback did not run");
    assert!(!lab.has("next-step-ran"), "the method went on");

    let experiment_id = detail["run"]["experiment_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the run has no experiment: {detail}"));
    let steps = stored_steps(&srv, experiment_id);
    let long = step(&steps, "long-step");
    assert_eq!(long["status"], "failed", "{steps:?}");
    assert_eq!(
        long["error"], "process 'sh' stopped before it finished",
        "{steps:?}"
    );
    assert_eq!(step(&steps, "undo")["status"], "succeeded", "{steps:?}");
    assert!(
        steps.iter().all(|step| step["name"] != "next-step"),
        "{steps:?}"
    );
}
