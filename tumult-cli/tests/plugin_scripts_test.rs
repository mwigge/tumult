//! Tests for the bundled script plugins.
//!
//! Validates that:
//! 1. Plugin manifests parse correctly as TOON
//! 2. Probe scripts execute and produce output on the current platform
//! 3. Scripts validate required environment variables
//! 4. Scripts that query a service fail when the query fails

mod lab;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use lab::{result, step, Lab};
use tumult_core::types::{ActivityStatus, ExperimentStatus};

/// Get workspace root (parent of tumult-cli/).
fn ws() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn plugin_path(relative: &str) -> String {
    ws().join("plugins")
        .join(relative)
        .to_str()
        .unwrap()
        .to_string()
}

/// Parse a plugin.toon manifest and verify it has expected fields.
fn assert_manifest_valid(plugin_dir: &str) {
    let path = plugin_path(&format!("{plugin_dir}/plugin.toon"));
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("manifest not found: {path}"));
    assert!(content.contains("name:"), "manifest missing name field");
    assert!(
        content.contains("version:"),
        "manifest missing version field"
    );
    assert!(
        content.contains("actions") || content.contains("probes"),
        "manifest missing actions/probes"
    );
}

fn assert_script_executable(relative: &str) {
    let path = plugin_path(relative);
    let metadata = std::fs::metadata(&path).unwrap_or_else(|_| panic!("script not found: {path}"));
    let mode = metadata.permissions().mode();
    assert!(
        mode & 0o111 != 0,
        "script not executable: {path} (mode: {mode:o})"
    );
}

fn run_script(relative: &str, env: &[(&str, &str)]) -> (i32, String, String) {
    let path = plugin_path(relative);
    let mut cmd = Command::new("sh");
    cmd.arg(&path);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to run {path}: {e}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// ═══════════════════════════════════════════════════════════════
// tumult-stress
// ═══════════════════════════════════════════════════════════════

#[test]
fn stress_manifest_parses() {
    assert_manifest_valid("tumult-stress");
}

#[test]
fn stress_scripts_are_executable() {
    for script in &[
        "tumult-stress/actions/cpu-stress.sh",
        "tumult-stress/actions/memory-stress.sh",
        "tumult-stress/actions/io-stress.sh",
        "tumult-stress/actions/combined-stress.sh",
        "tumult-stress/probes/cpu-utilization.sh",
        "tumult-stress/probes/memory-utilization.sh",
        "tumult-stress/probes/io-utilization.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn cpu_utilization_probe_produces_number() {
    let (code, stdout, _) = run_script("tumult-stress/probes/cpu-utilization.sh", &[]);
    assert_eq!(code, 0, "cpu-utilization probe failed");
    let value: f64 = stdout
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("cpu-utilization output not a number: '{}'", stdout.trim()));
    assert!(
        (0.0..=100.0).contains(&value),
        "cpu value out of range: {value}"
    );
}

#[test]
fn memory_utilization_probe_produces_number() {
    let (code, stdout, _) = run_script("tumult-stress/probes/memory-utilization.sh", &[]);
    assert_eq!(code, 0, "memory-utilization probe failed");
    let value: f64 = stdout.trim().parse().unwrap_or_else(|_| {
        panic!(
            "memory-utilization output not a number: '{}'",
            stdout.trim()
        )
    });
    assert!(
        (0.0..=100.0).contains(&value),
        "memory value out of range: {value}"
    );
}

#[test]
fn stress_action_fails_without_stressng() {
    let (code, _, stderr) = run_script(
        "tumult-stress/actions/cpu-stress.sh",
        &[("TUMULT_TIMEOUT", "1")],
    );
    if code != 0 {
        assert!(
            stderr.contains("stress-ng not found"),
            "expected stress-ng not found error, got: {stderr}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// tumult-containers
// ═══════════════════════════════════════════════════════════════

#[test]
fn containers_manifest_parses() {
    assert_manifest_valid("tumult-containers");
}

#[test]
fn containers_scripts_are_executable() {
    for script in &[
        "tumult-containers/actions/kill-container.sh",
        "tumult-containers/actions/stop-container.sh",
        "tumult-containers/actions/pause-container.sh",
        "tumult-containers/actions/unpause-container.sh",
        "tumult-containers/actions/limit-cpu.sh",
        "tumult-containers/actions/limit-memory.sh",
        "tumult-containers/probes/container-status.sh",
        "tumult-containers/probes/container-health.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn container_action_requires_container_id() {
    let (code, _, stderr) = run_script("tumult-containers/actions/kill-container.sh", &[]);
    assert_ne!(code, 0, "should fail without TUMULT_CONTAINER_ID");
    assert!(
        stderr.contains("TUMULT_CONTAINER_ID"),
        "error should mention missing var, got: {stderr}"
    );
}

// ═══════════════════════════════════════════════════════════════
// tumult-process
// ═══════════════════════════════════════════════════════════════

#[test]
fn process_manifest_parses() {
    assert_manifest_valid("tumult-process");
}

#[test]
fn process_scripts_are_executable() {
    for script in &[
        "tumult-process/actions/kill-process.sh",
        "tumult-process/actions/suspend-process.sh",
        "tumult-process/actions/resume-process.sh",
        "tumult-process/probes/process-exists.sh",
        "tumult-process/probes/process-resources.sh",
    ] {
        assert_script_executable(script);
    }
}

#[test]
fn process_exists_probe_detects_current_shell() {
    let pid = std::process::id();
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-exists.sh",
        &[("TUMULT_PID", &pid.to_string())],
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "true");
}

#[test]
fn process_exists_probe_detects_missing_pid() {
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-exists.sh",
        &[("TUMULT_PID", "999999999")],
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "false");
}

#[test]
fn process_resources_probe_returns_json() {
    let pid = std::process::id();
    let (code, stdout, _) = run_script(
        "tumult-process/probes/process-resources.sh",
        &[("TUMULT_PID", &pid.to_string())],
    );
    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("not valid JSON: '{}'", stdout.trim()));
    assert!(parsed.get("cpu_percent").is_some());
    assert!(parsed.get("mem_percent").is_some());
    assert!(parsed.get("running").is_some());
}

#[test]
fn process_action_requires_target() {
    let (code, _, stderr) = run_script("tumult-process/actions/kill-process.sh", &[]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("TUMULT_PID") || stderr.contains("required"),
        "error should mention missing target, got: {stderr}"
    );
}

// ═══════════════════════════════════════════════════════════════
// Scripts that call a service or a tool of the machine, run against stand-ins
//
// The service tools these scripts call (the Kafka tools, docker, mysql, psql
// and redis-cli), the tools that read or change the machine (tc, ping, top)
// and uname are stand-ins this file writes into a temp dir, and every test
// checks that its stand-ins ran. No such tool of the machine is started.
// ═══════════════════════════════════════════════════════════════

const CONSUMER_LAG: &str = "tumult-kafka/probes/consumer-lag.sh";
const UNDER_REPLICATED: &str = "tumult-kafka/probes/under-replicated.sh";
const BROKER_COUNT: &str = "tumult-kafka/probes/broker-count.sh";
const CLOCK_OFFSET: &str = "tumult-timewarp/probes/clock-offset.sh";
const REDIS_INFO: &str = "tumult-db-redis/probes/redis-info.sh";
const MYSQL_KILL_CONNECTIONS: &str = "tumult-db-mysql/actions/kill-connections.sh";
const POSTGRES_KILL_CONNECTIONS: &str = "tumult-db-postgres/actions/kill-connections.sh";

/// The service tools the scripts look for on `PATH`, and the tools of the
/// machine they read or change.
const SERVICE_TOOLS: [&str; 14] = [
    "docker",
    "mysql",
    "psql",
    "redis-cli",
    "kafka-topics",
    "kafka-topics.sh",
    "kafka-consumer-groups",
    "kafka-consumer-groups.sh",
    "kafka-broker-api-versions",
    "kafka-broker-api-versions.sh",
    "tc",
    "ping",
    "top",
    "uname",
];

/// What a service tool does until a test gives it an answer.
const NO_STAND_IN: &str = "echo \"$0: this test gave the tool no answer\" >&2\nexit 97";

/// Stand-ins for the tools of a script, in a temp dir. A stand-in writes
/// its name and its arguments to `calls` before it answers, so a test sees
/// that the stand-in ran and not a tool of the machine.
struct Stubs {
    dir: tempfile::TempDir,
}

impl Stubs {
    /// Every service tool starts as a stand-in that refuses, so a test that
    /// forgets one cannot start a tool of the machine.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(dir.path().join("bin")).expect("bin dir");
        let stubs = Self { dir };
        for name in SERVICE_TOOLS {
            stubs.tool(&format!("bin/{name}"), NO_STAND_IN);
        }
        stubs
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// A stand-in at `relative` that leaves its call in `calls`, then runs
    /// `answer`. Tools that a script finds on `PATH` go in `bin`, the Kafka
    /// tools in `kafka/bin`.
    fn tool(&self, relative: &str, answer: &str) {
        let path = self.path(relative);
        let dir = path.parent().expect("a tool has a dir");
        let name = path
            .file_name()
            .expect("a tool has a name")
            .to_string_lossy();
        std::fs::create_dir_all(dir).expect("tool dir");
        let calls = self.path("calls");
        let record = format!(r#"printf '%s\n' "{name} $*" >> '{}'"#, calls.display());
        tumult_test_utils::fake_bin(dir, &name, &format!("{record}\n{answer}"));
    }

    /// The calls of the stand-ins, one line each: the name and the arguments.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Run the plugin script at `relative` with `sh`: `bin` comes first on
    /// `PATH`, the Kafka tools are looked for in `kafka`, and the temp dir
    /// is `HOME` and `TMPDIR`.
    fn run(&self, relative: &str, env: &[(&str, &str)]) -> (i32, String, String) {
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let output = Command::new("sh")
            .arg(plugin_path(relative))
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("TMPDIR", self.dir.path())
            .env("TUMULT_KAFKA_DIR", self.path("kafka"))
            .envs(env.iter().copied())
            .output()
            .expect("run sh");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    }
}

/// The probe failed, printed no reading, and passed on `message`, the
/// line its tool wrote on stderr.
fn assert_failed_with(run: &(i32, String, String), message: &str) {
    let (code, stdout, stderr) = run;
    assert_ne!(*code, 0, "a query that failed is not a reading: {stdout}");
    assert_eq!(stdout, "", "no reading on stdout");
    assert!(
        stderr.contains(message),
        "the message of the tool: {stderr}"
    );
}

/// A script whose tool got no answer from the test reaches the refusing
/// stand-in, not a tool of the machine.
#[test]
fn a_tool_the_test_gave_no_answer_refuses_and_no_tool_of_the_machine_runs() {
    let stubs = Stubs::new();

    let run = stubs.run(REDIS_INFO, &[]);

    assert_eq!(stubs.calls(), ["redis-cli -h localhost -p 6379 INFO"]);
    assert_eq!(run.0, 97, "{}", run.2);
    assert_failed_with(&run, "this test gave the tool no answer");
}

// ── tumult-kafka ──────────────────────────────────────────────

/// What a Kafka tool writes when no broker answers.
const KAFKA_DOWN: &str =
    "org.apache.kafka.common.errors.TimeoutException: Timed out waiting for a node assignment.";

/// A Kafka tool that finds no broker.
fn kafka_down() -> String {
    format!("echo '{KAFKA_DOWN}' >&2\nexit 1")
}

const DESCRIBE_GROUP: &str =
    "kafka-consumer-groups.sh --bootstrap-server localhost:9092 --group orders --describe";
const DESCRIBE_UNDER_REPLICATED: &str =
    "kafka-topics.sh --bootstrap-server localhost:9092 --describe --under-replicated-partitions";

/// `kafka-consumer-groups --describe` for a group of two partitions whose
/// lags are `first` and `second`.
fn group_with_lag(first: u32, second: u32) -> String {
    format!(
        "cat <<'EOF'

GROUP   TOPIC   PARTITION  CURRENT-OFFSET  LOG-END-OFFSET  LAG  CONSUMER-ID  HOST       CLIENT-ID
orders  orders  0          120             {}             {first}    c-1          /10.0.0.4  c-1
orders  orders  1          300             {}             {second}    c-1          /10.0.0.4  c-1
EOF",
        120 + first,
        300 + second
    )
}

#[test]
fn consumer_lag_probe_fails_with_the_message_of_the_tool_when_the_query_fails() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-consumer-groups.sh", &kafka_down());

    let run = stubs.run(CONSUMER_LAG, &[("TUMULT_CONSUMER_GROUP", "orders")]);

    assert_eq!(stubs.calls(), [DESCRIBE_GROUP]);
    assert_failed_with(&run, KAFKA_DOWN);
}

#[test]
fn consumer_lag_probe_reads_zero_when_every_partition_is_caught_up() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-consumer-groups.sh", &group_with_lag(0, 0));

    let (code, stdout, stderr) = stubs.run(CONSUMER_LAG, &[("TUMULT_CONSUMER_GROUP", "orders")]);

    assert_eq!(stubs.calls(), [DESCRIBE_GROUP]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn consumer_lag_probe_sums_the_lag_of_every_partition() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-consumer-groups.sh", &group_with_lag(5, 7));

    let (code, stdout, stderr) = stubs.run(CONSUMER_LAG, &[("TUMULT_CONSUMER_GROUP", "orders")]);

    assert_eq!(stubs.calls(), [DESCRIBE_GROUP]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "12\n");
}

#[test]
fn under_replicated_probe_fails_with_the_message_of_the_tool_when_the_query_fails() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-topics.sh", &kafka_down());

    let run = stubs.run(UNDER_REPLICATED, &[]);

    assert_eq!(stubs.calls(), [DESCRIBE_UNDER_REPLICATED]);
    assert_failed_with(&run, KAFKA_DOWN);
}

#[test]
fn under_replicated_probe_prints_one_zero_for_a_healthy_cluster() {
    let stubs = Stubs::new();
    // A healthy cluster has no partition to list.
    stubs.tool("kafka/bin/kafka-topics.sh", "exit 0");

    let (code, stdout, stderr) = stubs.run(UNDER_REPLICATED, &[]);

    assert_eq!(stubs.calls(), [DESCRIBE_UNDER_REPLICATED]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn under_replicated_probe_counts_each_partition_that_lacks_a_replica() {
    let stubs = Stubs::new();
    stubs.tool(
        "kafka/bin/kafka-topics.sh",
        "cat <<'EOF'
\tTopic: orders\tPartition: 1\tLeader: 1\tReplicas: 1,2,3\tIsr: 1,3
\tTopic: orders\tPartition: 2\tLeader: 3\tReplicas: 3,1,2\tIsr: 3,1
EOF",
    );

    let (code, stdout, stderr) = stubs.run(UNDER_REPLICATED, &[]);

    assert_eq!(stubs.calls(), [DESCRIBE_UNDER_REPLICATED]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "2\n");
}

#[test]
fn broker_count_probe_fails_with_the_message_of_the_tool_when_the_query_fails() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-broker-api-versions.sh", &kafka_down());

    let run = stubs.run(BROKER_COUNT, &[]);

    assert_eq!(
        stubs.calls(),
        ["kafka-broker-api-versions.sh --bootstrap-server localhost:9092"]
    );
    assert_failed_with(&run, KAFKA_DOWN);
}

#[test]
fn broker_count_probe_prints_one_zero_when_no_broker_is_listed() {
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-broker-api-versions.sh", "exit 0");

    let (code, stdout, stderr) = stubs.run(BROKER_COUNT, &[]);

    assert_eq!(
        stubs.calls(),
        ["kafka-broker-api-versions.sh --bootstrap-server localhost:9092"]
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn broker_count_probe_counts_every_broker_of_the_cluster() {
    let stubs = Stubs::new();
    // Each broker is one line with its id, then the API versions it serves.
    stubs.tool(
        "kafka/bin/kafka-broker-api-versions.sh",
        "cat <<'EOF'
kafka-1:9092 (id: 1 rack: null) -> (
\tProduce(0): 0 to 11 [usable: 11],
\tFetch(1): 0 to 16 [usable: 16]
)
kafka-2:9092 (id: 2 rack: null) -> (
\tProduce(0): 0 to 11 [usable: 11],
\tFetch(1): 0 to 16 [usable: 16]
)
kafka-3:9092 (id: 3 rack: null) -> (
\tProduce(0): 0 to 11 [usable: 11],
\tFetch(1): 0 to 16 [usable: 16]
)
EOF",
    );

    let (code, stdout, stderr) =
        stubs.run(BROKER_COUNT, &[("TUMULT_KAFKA_BOOTSTRAP", "kafka-1:9092")]);

    assert_eq!(
        stubs.calls(),
        ["kafka-broker-api-versions.sh --bootstrap-server kafka-1:9092"]
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "3\n");
}

/// The check of the steady state in the example of
/// `docs/plugins/tumult-kafka.md`.
const DOCUMENTED_REPLICA_CHECK: &str = "    - name: check-replicas
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-kafka/probes/under-replicated.sh
        env:
          TUMULT_KAFKA_BOOTSTRAP: broker-1:9092
      tolerance:
        type: exact
        value: 0
";

/// The lag check of the example in `docs/blog/05-experiment-format.md`,
/// with its configuration filled in.
const DOCUMENTED_LAG_CHECK: &str = "    - name: consumer-lag
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-kafka/probes/consumer-lag.sh
        env:
          TUMULT_KAFKA_BOOTSTRAP: broker-1:9092
          TUMULT_CONSUMER_GROUP: orders
      tolerance:
        type: range
        from: 0
        to: 100
";

/// An experiment whose steady state is `probe` and whose method does
/// nothing. A `plugins` link in the lab lets the probe name its script by
/// the path the docs give.
fn guarded_by(lab: &Lab, probe: &str) -> String {
    std::os::unix::fs::symlink(ws().join("plugins"), lab.path("plugins"))
        .expect("link the plugins");
    format!(
        "title: A plugin probe guards the steady state
description: The method does nothing

steady_state_hypothesis:
  title: The cluster is healthy
  probes[1]:
{probe}
method[1]:
{}",
        step("no-op", "action", "true")
    )
}

/// Every call of the stand-ins is `expected`, and there was one at least.
fn assert_only_calls(stubs: &Stubs, expected: &str) {
    let calls = stubs.calls();
    assert!(!calls.is_empty(), "the stand-in never ran");
    assert!(calls.iter().all(|call| call == expected), "{calls:?}");
}

#[test]
fn the_documented_kafka_check_passes_on_a_healthy_cluster() {
    let lab = Lab::with_state("ok");
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-topics.sh", "exit 0");

    let output = lab
        .run_command(&guarded_by(&lab, DOCUMENTED_REPLICA_CHECK))
        .env("TUMULT_KAFKA_DIR", stubs.path("kafka"))
        .output()
        .expect("run tumult");

    assert_only_calls(
        &stubs,
        "kafka-topics.sh --bootstrap-server broker-1:9092 --describe --under-replicated-partitions",
    );
    let journal = lab.journal(&output);
    assert_eq!(
        journal.status,
        ExperimentStatus::Completed,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(0));
    let before = journal.steady_state_before.expect("the check ran");
    let check = result(&before.probe_results, "check-replicas");
    assert_eq!(check.output.as_deref(), Some("0"));
}

#[test]
fn a_consumer_lag_check_aborts_the_run_when_the_kafka_tool_is_down() {
    let lab = Lab::with_state("ok");
    let stubs = Stubs::new();
    stubs.tool("kafka/bin/kafka-consumer-groups.sh", &kafka_down());

    let output = lab
        .run_command(&guarded_by(&lab, DOCUMENTED_LAG_CHECK))
        .env("TUMULT_KAFKA_DIR", stubs.path("kafka"))
        .output()
        .expect("run tumult");

    assert_only_calls(
        &stubs,
        "kafka-consumer-groups.sh --bootstrap-server broker-1:9092 --group orders --describe",
    );
    let journal = lab.journal(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert!(journal.method_results.is_empty(), "the method did not run");
    let before = journal.steady_state_before.expect("the check ran");
    let lag = result(&before.probe_results, "consumer-lag");
    assert_eq!(lag.status, ActivityStatus::Failed);
    let error = lag.error.unwrap_or_default();
    assert!(error.contains(KAFKA_DOWN), "{error}");
}

// ── tumult-timewarp ───────────────────────────────────────────

/// The epoch the stand-in for `date` gives the runner.
const RUNNER_EPOCH: &str = "1700000000";
const DATE_CALL: &str = "date -u +%s";
const EXEC_CALL: &str = "docker exec app sh -c date -u +%s";

/// Stand-ins for `date`, which tells the runner `RUNNER_EPOCH`, and for
/// `docker`, which runs `answer` for the target.
fn clock_stubs(answer: &str) -> Stubs {
    let stubs = Stubs::new();
    stubs.tool("bin/date", &format!("echo {RUNNER_EPOCH}"));
    stubs.tool("bin/docker", answer);
    stubs
}

#[test]
fn clock_offset_probe_fails_with_the_message_of_docker_when_exec_fails() {
    let stubs =
        clock_stubs("echo 'Error response from daemon: No such container: app' >&2\nexit 1");

    let run = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "app")]);

    assert_eq!(stubs.calls(), [DATE_CALL, EXEC_CALL]);
    assert_failed_with(&run, "No such container: app");
}

#[test]
fn clock_offset_probe_fails_when_the_target_answers_no_number() {
    let stubs = clock_stubs("echo 'Thu Nov 19 12:00:00 UTC 2026'");

    let run = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "app")]);

    assert_eq!(stubs.calls(), [DATE_CALL, EXEC_CALL]);
    assert_failed_with(&run, "Thu Nov 19 12:00:00 UTC 2026");
}

#[test]
fn clock_offset_probe_reads_zero_when_the_clocks_agree() {
    let stubs = clock_stubs(&format!("echo {RUNNER_EPOCH}"));

    let (code, stdout, stderr) = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "app")]);

    assert_eq!(stubs.calls(), [DATE_CALL, EXEC_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn clock_offset_probe_reads_how_far_the_target_clock_is_ahead() {
    let stubs = clock_stubs("echo 1700003600");

    let (code, stdout, stderr) = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "app")]);

    assert_eq!(stubs.calls(), [DATE_CALL, EXEC_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "3600\n");
}

#[test]
fn clock_offset_probe_reads_zero_without_a_target_and_asks_no_container() {
    let stubs = clock_stubs("exit 1");

    let (code, stdout, stderr) = stubs.run(CLOCK_OFFSET, &[("TUMULT_TARGET", "")]);

    assert_eq!(stubs.calls(), [DATE_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0\n");
}

// ── tumult-db-redis ───────────────────────────────────────────

const REDIS_DOWN: &str = "Could not connect to Redis at localhost:6379: Connection refused";
const INFO_CALL: &str = "redis-cli -h localhost -p 6379 INFO";

#[test]
fn redis_info_probe_fails_with_the_message_of_redis_cli_when_it_fails() {
    let stubs = Stubs::new();
    stubs.tool("bin/redis-cli", &format!("echo '{REDIS_DOWN}' >&2\nexit 1"));

    let run = stubs.run(REDIS_INFO, &[]);

    assert_eq!(stubs.calls(), [INFO_CALL]);
    assert_failed_with(&run, REDIS_DOWN);
}

#[test]
fn redis_info_probe_fails_on_an_error_reply_that_exits_zero() {
    let stubs = Stubs::new();
    stubs.tool("bin/redis-cli", "echo 'NOAUTH Authentication required.'");

    let run = stubs.run(REDIS_INFO, &[]);

    assert_eq!(stubs.calls(), [INFO_CALL]);
    assert_failed_with(&run, "NOAUTH Authentication required.");
}

#[test]
fn redis_info_probe_reads_the_stats_of_a_healthy_server() {
    let stubs = Stubs::new();
    stubs.tool(
        "bin/redis-cli",
        r"printf '# Server\r\nuptime_in_seconds:3600\r\n\r\n# Clients\r\nconnected_clients:3\r\n'
printf '\r\n# Memory\r\nused_memory:1048576\r\nused_memory_human:1.00M\r\n'
printf '\r\n# Stats\r\ninstantaneous_ops_per_sec:12\r\nkeyspace_hits:40\r\nkeyspace_misses:2\r\n'",
    );

    let (code, stdout, stderr) = stubs.run(REDIS_INFO, &[]);

    assert_eq!(stubs.calls(), [INFO_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        r#"{"connected_clients": 3, "used_memory_bytes": 1048576, "used_memory_human": "1.00M", "ops_per_sec": 12, "keyspace_hits": 40, "keyspace_misses": 2, "uptime_seconds": 3600}"#
    );
}

// ── tumult-db-mysql ───────────────────────────────────────────

const PROCESSLIST_CALL: &str = "mysql -h localhost -P 3306 -u root -N -e SELECT id FROM information_schema.processlist WHERE db = 'app' AND id <> CONNECTION_ID();";

fn kill_call(id: u32) -> String {
    format!("mysql -h localhost -P 3306 -u root -e KILL {id};")
}

#[test]
fn mysql_kill_connections_counts_only_the_connections_it_killed() {
    let stubs = Stubs::new();
    // Connection 12 ends on its own between the list and its KILL.
    stubs.tool(
        "bin/mysql",
        r#"case "$*" in
    *SELECT*) printf '11\n12\n13\n' ;;
    *"KILL 12;"*) echo 'ERROR 1094 (HY000) at line 1: Unknown thread id: 12' >&2; exit 1 ;;
esac"#,
    );

    let (code, stdout, stderr) =
        stubs.run(MYSQL_KILL_CONNECTIONS, &[("TUMULT_MYSQL_DATABASE", "app")]);

    assert_eq!(
        stubs.calls(),
        [
            PROCESSLIST_CALL.to_string(),
            kill_call(11),
            kill_call(12),
            kill_call(13)
        ]
    );
    assert_eq!(
        code, 0,
        "a connection that went away is no failure: {stderr}"
    );
    assert_eq!(
        stdout,
        "killing connections to app\nkilled 2 connections to app\n"
    );
    assert!(stderr.contains("Unknown thread id: 12"), "{stderr}");
}

#[test]
fn mysql_kill_connections_fails_with_the_message_of_mysql_when_the_list_fails() {
    let stubs = Stubs::new();
    stubs.tool(
        "bin/mysql",
        r#"echo "ERROR 2003 (HY000): Can't connect to MySQL server on 'localhost:3306' (111)" >&2
exit 1"#,
    );

    let (code, stdout, stderr) =
        stubs.run(MYSQL_KILL_CONNECTIONS, &[("TUMULT_MYSQL_DATABASE", "app")]);

    assert_eq!(stubs.calls(), [PROCESSLIST_CALL]);
    assert_ne!(code, 0, "{stdout}");
    assert!(!stdout.contains("killed"), "{stdout}");
    assert!(stderr.contains("ERROR 2003 (HY000)"), "{stderr}");
}

// ── tumult-db-postgres ────────────────────────────────────────

const TERMINATE_CALL: &str = "psql -h localhost -p 5432 -U postgres -d postgres -t -c SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $$app$$ AND pid <> pg_backend_pid();";

#[test]
fn postgres_kill_connections_prints_one_zero_when_no_connection_is_open() {
    let stubs = Stubs::new();
    stubs.tool("bin/psql", "echo ''");

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_KILL_CONNECTIONS, &[("TUMULT_PG_DATABASE", "app")]);

    assert_eq!(stubs.calls(), [TERMINATE_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "terminated 0 connections to app\n");
}

#[test]
fn postgres_kill_connections_counts_only_the_rows_that_say_t() {
    let stubs = Stubs::new();
    // Backend 4242 ended on its own: the server warns and answers f.
    stubs.tool(
        "bin/psql",
        r"printf ' t\n t\n f\n\n'
echo 'WARNING:  PID 4242 is not a PostgreSQL backend process' >&2",
    );

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_KILL_CONNECTIONS, &[("TUMULT_PG_DATABASE", "app")]);

    assert_eq!(stubs.calls(), [TERMINATE_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "terminated 2 connections to app\n");
}

#[test]
fn postgres_kill_connections_fails_with_the_message_of_psql() {
    let stubs = Stubs::new();
    stubs.tool(
        "bin/psql",
        r#"echo 'psql: error: connection to server at "localhost" (::1), port 5432 failed: Connection refused' >&2
exit 2"#,
    );

    let (code, stdout, stderr) =
        stubs.run(POSTGRES_KILL_CONNECTIONS, &[("TUMULT_PG_DATABASE", "app")]);

    assert_eq!(stubs.calls(), [TERMINATE_CALL]);
    assert_ne!(code, 0, "{stdout}");
    assert!(!stdout.contains("terminated"), "{stdout}");
    assert!(
        stderr.contains("port 5432 failed: Connection refused"),
        "{stderr}"
    );
}

// ── tumult-network ────────────────────────────────────────────

const ADD_LATENCY: &str = "tumult-network/actions/add-latency.sh";
const PING_LATENCY: &str = "tumult-network/probes/ping-latency.sh";

/// Stand-ins for `uname`, which names `os`, and for `tool`, which runs
/// `answer`.
fn stubs_on(os: &str, tool: &str, answer: &str) -> Stubs {
    let stubs = Stubs::new();
    stubs.tool("bin/uname", &format!("echo {os}"));
    stubs.tool(&format!("bin/{tool}"), answer);
    stubs
}

#[test]
fn add_latency_delays_the_whole_interface_when_no_target_ip_is_given() {
    let stubs = stubs_on("Linux", "tc", "exit 0");

    let (code, stdout, stderr) = stubs.run(ADD_LATENCY, &[]);

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs.calls(),
        [
            "uname -s",
            "tc qdisc add dev eth0 root netem delay 100ms 10ms"
        ]
    );
    assert_eq!(
        stdout,
        "adding 100ms (±10ms) latency on eth0\nlatency injection active\n"
    );
}

#[test]
fn add_latency_delays_only_the_traffic_to_the_target_ip() {
    let stubs = stubs_on("Linux", "tc", "exit 0");

    let (code, stdout, stderr) = stubs.run(
        ADD_LATENCY,
        &[
            ("TUMULT_TARGET_IP", "10.0.1.50"),
            ("TUMULT_INTERFACE", "ens3"),
            ("TUMULT_DELAY_MS", "250"),
            ("TUMULT_JITTER_MS", "0"),
        ],
    );

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stubs.calls(),
        [
            "uname -s",
            "tc qdisc add dev ens3 root handle 1: prio",
            "tc qdisc add dev ens3 parent 1:3 handle 30: netem delay 250ms 0ms",
            "tc filter add dev ens3 parent 1:0 protocol ip u32 match ip dst 10.0.1.50/32 flowid 1:3",
        ]
    );
    assert_eq!(
        stdout,
        "adding 250ms (±0ms) latency to 10.0.1.50 on ens3\nlatency injection active\n"
    );
}

const PING_CALL: &str = "ping -c 3 -q db";

/// What `ping -c 3 -q` prints on Linux for a host that answers.
const LINUX_PING: &str = "cat <<'EOF'
PING db (10.0.1.50) 56(84) bytes of data.

--- db ping statistics ---
3 packets transmitted, 3 received, 0% packet loss, time 2003ms
rtt min/avg/max/mdev = 0.045/0.052/0.061/0.007 ms
EOF";

/// What `ping -c 3 -q` prints on macOS for a host that answers.
const DARWIN_PING: &str = "cat <<'EOF'
PING db (10.0.1.50): 56 data bytes

--- db ping statistics ---
3 packets transmitted, 3 packets received, 0.0% packet loss
round-trip min/avg/max/stddev = 0.045/0.052/0.061/0.007 ms
EOF";

/// The line of the summary of Linux `ping` for a host that does not answer.
const NO_REPLY: &str = "3 packets transmitted, 0 received, 100% packet loss, time 2052ms";

#[test]
fn ping_latency_probe_reads_the_average_round_trip_on_linux() {
    let stubs = stubs_on("Linux", "ping", LINUX_PING);

    let (code, stdout, stderr) = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0.052\n");
}

#[test]
fn ping_latency_probe_fails_with_the_message_of_ping_when_the_host_does_not_answer() {
    // Linux ping writes its summary on stdout, also for a host that is down.
    let stubs = stubs_on(
        "Linux",
        "ping",
        &format!(
            "printf 'PING db (10.0.1.50) 56(84) bytes of data.\\n\\n--- db ping statistics ---\\n%s\\n' '{NO_REPLY}'\nexit 1"
        ),
    );

    let run = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_eq!(run.0, 1, "the status of ping: {}", run.2);
    assert_failed_with(&run, NO_REPLY);
}

#[test]
fn ping_latency_probe_fails_when_ping_prints_no_round_trip_time_on_linux() {
    let stubs = stubs_on(
        "Linux",
        "ping",
        "echo 'PING db (10.0.1.50) 56(84) bytes of data.'",
    );

    let run = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_failed_with(&run, "PING db (10.0.1.50) 56(84) bytes of data.");
}

#[test]
fn ping_latency_probe_reads_the_average_round_trip_on_macos() {
    let stubs = stubs_on("Darwin", "ping", DARWIN_PING);

    let (code, stdout, stderr) = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "0.052\n");
}

#[test]
fn ping_latency_probe_fails_with_the_message_of_ping_when_the_name_is_unknown_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "ping",
        "echo 'ping: cannot resolve db: Unknown host' >&2\nexit 68",
    );

    let run = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_eq!(run.0, 68, "the status of ping: {}", run.2);
    assert_failed_with(&run, "ping: cannot resolve db: Unknown host");
}

#[test]
fn ping_latency_probe_fails_when_ping_prints_no_round_trip_time_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "ping",
        "echo 'PING db (10.0.1.50): 56 data bytes'",
    );

    let run = stubs.run(PING_LATENCY, &[("TUMULT_TARGET_HOST", "db")]);

    assert_eq!(stubs.calls(), ["uname -s", PING_CALL]);
    assert_failed_with(&run, "PING db (10.0.1.50): 56 data bytes");
}

// ── tumult-stress ─────────────────────────────────────────────

const CPU_UTILIZATION: &str = "tumult-stress/probes/cpu-utilization.sh";
const TOP_CALL: &str = "top -l 1 -n 0";

#[test]
fn cpu_utilization_probe_reads_the_busy_share_from_top_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "top",
        "cat <<'EOF'
Processes: 512 total, 3 running, 509 sleeping, 2390 threads
2026/11/19 12:00:00
Load Avg: 1.80, 1.86, 1.89
CPU usage: 5.26% user, 10.52% sys, 84.21% idle
SharedLibs: 371M resident, 61M data, 25M linkedit.
EOF",
    );

    let (code, stdout, stderr) = stubs.run(CPU_UTILIZATION, &[]);

    assert_eq!(stubs.calls(), ["uname -s", TOP_CALL]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "15.79\n");
}

#[test]
fn cpu_utilization_probe_fails_with_the_message_of_top_when_top_fails_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "top",
        "echo 'top: sysctl: Operation not permitted' >&2\nexit 1",
    );

    let run = stubs.run(CPU_UTILIZATION, &[]);

    assert_eq!(stubs.calls(), ["uname -s", TOP_CALL]);
    assert_failed_with(&run, "top: sysctl: Operation not permitted");
}

#[test]
fn cpu_utilization_probe_fails_when_top_prints_no_cpu_usage_on_macos() {
    let stubs = stubs_on(
        "Darwin",
        "top",
        "echo 'Processes: 512 total, 3 running, 509 sleeping, 2390 threads'",
    );

    let run = stubs.run(CPU_UTILIZATION, &[]);

    assert_eq!(stubs.calls(), ["uname -s", TOP_CALL]);
    assert_failed_with(&run, "Processes: 512 total");
}
