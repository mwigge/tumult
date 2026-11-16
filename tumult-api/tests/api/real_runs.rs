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

/// The target of a run: it says what its state file says. A target that
/// was read as down three times comes back on its own.
const TARGET: &str = r#"cat state
grep -q down state || exit 0
echo read >> read-down
test "$(wc -l < read-down)" -lt 3 || echo ok > state
"#;

/// A directory the steps of one run write to. Its file `state` says `ok`
/// while the target is healthy.
struct Lab(tempfile::TempDir);

impl Lab {
    fn new() -> Self {
        let lab = Self(tempfile::TempDir::new().expect("lab directory"));
        std::fs::write(lab.path("state"), "ok\n").expect("state file");
        std::fs::write(lab.path("target.sh"), TARGET).expect("target script");
        lab
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

    /// A probe that runs `command` in the lab and holds while it prints `ok`.
    fn probe(&self, name: &str, command: &str) -> String {
        let action = self.step(name, command, 5.0);
        let probe = action.replace("activity_type: action", "activity_type: probe");
        format!("{probe}    tolerance:\n      type: regex\n      pattern: \"^ok$\"\n")
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

    /// A steady state that holds while the target says `ok`, a method of a
    /// fault and a check, and a rollback that heals the target.
    fn experiment(&self, ending: &Ending) -> String {
        let hypothesis = self.probe("steady", "sh target.sh");
        format!(
            "title: {title}\n\
             description: every step is a real process\n\
             steady_state_hypothesis:\n  title: the target says ok\n  probes[1]:\n{steady}\
             method[2]:\n{fault}{check}rollbacks[1]:\n{heal}",
            title = ending.title,
            steady = hypothesis
                .replace("\n  ", "\n    ")
                .replacen("  - ", "    - ", 1),
            fault = self.step("fault", ending.fault, 5.0),
            check = self.probe("check", &format!("echo {}", ending.check_says)),
            heal = self.step("heal", "echo ok > state; touch rolled-back", 5.0),
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

/// The rows of the store for one experiment, in the order of `order`.
fn stored(srv: &TestServer, select: &str, experiment_id: &str, order: &str) -> Vec<Value> {
    let reader = Store::at(&srv.db_path).read_only().expect("reader");
    reader
        .query_json_rows(&format!(
            "{select} WHERE experiment_id = '{experiment_id}' ORDER BY {order}"
        ))
        .expect("stored rows")
}

/// The steps the store holds for the experiment of a run, in their order.
fn stored_steps(srv: &TestServer, experiment_id: &str) -> Vec<Value> {
    stored(
        srv,
        "SELECT name, status, error, phase FROM activity_results",
        experiment_id,
        "started_at_ns",
    )
}

/// What the samples of the steady state said once the method was over:
/// one `true` for every sample within its tolerance.
fn samples_after_the_method(srv: &TestServer, experiment_id: &str) -> Vec<bool> {
    let rows = stored(
        srv,
        "SELECT ok FROM (SELECT * FROM probe_samples WHERE phase = 'post')",
        experiment_id,
        "ts_ns",
    );
    rows.iter()
        .map(|row| row["ok"].as_bool().expect("ok"))
        .collect()
}

fn step<'a>(steps: &'a [Value], name: &str) -> &'a Value {
    steps
        .iter()
        .find(|step| step["name"] == name)
        .unwrap_or_else(|| panic!("no step {name} in {steps:?}"))
}

/// The id of the experiment the detail of a run names.
fn experiment_of(detail: &Value) -> &str {
    detail["run"]["experiment_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the run has no experiment: {detail}"))
}

/// Run `definition` over the API until it ends, and give back the detail.
async fn run_to_its_end(srv: &TestServer, definition: &str) -> Value {
    let registry_id = register_toon(&srv.base, definition).await;
    let run_id = enqueue_approved(&srv.base, &registry_id).await;
    await_terminal_run(&srv.base, &run_id).await
}

/// The ids of the runs the experiments list has for `filter`.
async fn listed(srv: &TestServer, filter: &str) -> Vec<String> {
    let (status, body) = get(&srv.base, &format!("/api/experiments?{filter}")).await;
    assert_eq!(status, 200, "{filter}: {body}");
    let rows = body["experiments"].as_array().expect("experiments");
    rows.iter()
        .map(|row| row["id"].as_str().expect("id").to_string())
        .collect()
}

/// The value of the experiments card of the overview.
async fn runs_on_the_overview(srv: &TestServer) -> f64 {
    let (status, body) = get(&srv.base, "/api/overview?range=24h").await;
    assert_eq!(status, 200, "{body}");
    let cards = body["kpis"].as_array().expect("kpis");
    let card = cards
        .iter()
        .find(|card| card["name"] == "experiments")
        .unwrap_or_else(|| panic!("no experiments card: {body}"));
    card["value"].as_f64().expect("value")
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

    let steps = stored_steps(&srv, experiment_of(&detail));
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

/// A run of real steps: what its method does, how it ends and how it is
/// read back.
struct Ending {
    title: &'static str,
    /// What the fault of the method runs.
    fault: &'static str,
    /// What the check after the fault prints. It holds on `ok`.
    check_says: &'static str,
    /// The state of the run in the queue.
    state: &'static str,
    /// The status of the run on every page that lists it.
    status: &'static str,
    /// The status of the fault and of the check in the store.
    steps: [&'static str; 2],
    /// Whether the target was down when the method was over.
    went_down: bool,
    rolled_back: bool,
}

const ENDINGS: [Ending; 4] = [
    Ending {
        title: "real steps, the target stays up",
        fault: "touch acted",
        check_says: "ok",
        state: "passed",
        status: "completed",
        steps: ["succeeded", "succeeded"],
        went_down: false,
        rolled_back: false,
    },
    Ending {
        title: "real steps, the target goes down and comes back",
        fault: "echo down > state",
        check_says: "ok",
        state: "passed",
        status: "completed",
        steps: ["succeeded", "succeeded"],
        went_down: true,
        rolled_back: false,
    },
    Ending {
        title: "real steps, the check after the fault reads down",
        fault: "touch acted",
        check_says: "down",
        state: "deviated",
        status: "deviated",
        steps: ["succeeded", "failed"],
        went_down: false,
        rolled_back: true,
    },
    Ending {
        title: "real steps, the fault itself fails",
        fault: "touch acted; exit 3",
        check_says: "ok",
        state: "failed",
        status: "failed",
        steps: ["failed", "succeeded"],
        went_down: false,
        rolled_back: true,
    },
];

/// The run of one ending is read back as it ended: from the queue, from the
/// store, from its page and from the list.
async fn read_back(srv: &TestServer, ending: &Ending, lab: &Lab, definition: &str) {
    let title = ending.title;
    let detail = run_to_its_end(srv, definition).await;
    assert_eq!(detail["run"]["state"], ending.state, "{title}: {detail}");
    assert_eq!(lab.has("rolled-back"), ending.rolled_back, "{title}");

    let id = experiment_of(&detail);
    let steps = stored_steps(srv, id);
    let statuses = ["fault", "check"].map(|name| step(&steps, name)["status"].clone());
    assert_eq!(statuses, ending.steps, "{title}: {steps:?}");
    let samples = samples_after_the_method(srv, id);
    assert_eq!(samples.first(), Some(&!ending.went_down), "{title}");
    assert_eq!(samples.last(), Some(&true), "{title}: {samples:?}");

    let (code, page) = get(&srv.base, &format!("/api/experiments/{id}")).await;
    assert_eq!(code, 200, "{title}: {page}");
    assert_eq!(page["experiment"]["status"], ending.status, "{title}");
    assert_eq!(page["experiment"]["name"], title, "{page}");
    let by_name = format!("name={}", title.replace(' ', "%20"));
    assert_eq!(listed(srv, &by_name).await, [id], "{title}");
    let by_both = format!("{by_name}&outcome={}", ending.status);
    assert_eq!(listed(srv, &by_both).await, [id], "{title}");
    let by_another = format!("{by_name}&outcome=aborted");
    assert!(listed(srv, &by_another).await.is_empty(), "{title}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_of_real_steps_is_read_back_as_it_ended() {
    let labs: Vec<Lab> = ENDINGS.iter().map(|_| Lab::new()).collect();
    let definitions: Vec<String> = ENDINGS
        .iter()
        .zip(&labs)
        .map(|(ending, lab)| lab.experiment(ending))
        .collect();
    let named: Vec<&str> = definitions.iter().map(String::as_str).collect();
    let srv = spawn_server_with_real_executor(&named).await;
    let runs_before = runs_on_the_overview(&srv).await;

    for ((ending, lab), definition) in ENDINGS.iter().zip(&labs).zip(&definitions) {
        read_back(&srv, ending, lab, definition).await;
    }

    let runs_after = runs_on_the_overview(&srv).await;
    assert!(
        (runs_after - runs_before - 4.0).abs() < f64::EPSILON,
        "the overview went from {runs_before} to {runs_after} runs"
    );
}
