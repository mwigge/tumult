//! `tumult manual` against a real daemon API on loopback: the router of
//! the daemon over a lake in a temp dir, with its single writer. The
//! commands run as the real `tumult` binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tumult_lake::Store;

/// The daemon of a test: its API on a loopback port, over a lake of its own.
struct Daemon {
    base: String,
    dir: tempfile::TempDir,
    ingest: tumult_ingest::IngestWriter,
    runtime: tokio::runtime::Runtime,
}

/// The house of the test daemon: a failover needs two sign-offs.
const ORG_FILE: &str = "nodes:
  - {name: platform, kind: domain}
score:
  manual_required_signoffs: {failover: 2}
";

impl Daemon {
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("lake.duckdb");
        Store::open(&db_path).expect("a new lake");
        let org_file = dir.path().join("org.yaml");
        std::fs::write(&org_file, ORG_FILE).expect("org file");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        let (base, ingest) = runtime.block_on(serve(dir.path(), db_path, org_file));
        Self {
            base,
            dir,
            ingest,
            runtime,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `tumult` with `args` against this daemon, with a home of its own.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tumult"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env("HOME", self.dir.path())
            .env("TUMULT_LAKE_PATH", self.path("cli-lake.duckdb"))
            .env("TUMULT_DAEMON_URL", &self.base)
            .env_remove("TUMULT_DAEMON_TOKEN")
            .env_remove("RUST_LOG")
            .env_remove("TUMULT_LOG_FORMAT")
            .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT");
        command
    }

    fn tumult(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run tumult")
    }

    /// `tumult` with `args`, sending `token` to the daemon.
    fn tumult_as(&self, token: &str, args: &[&str]) -> Output {
        self.command(args)
            .env("TUMULT_DAEMON_TOKEN", token)
            .output()
            .expect("run tumult")
    }

    /// Run one write on the lake through the single writer of the daemon.
    fn write(&self, f: impl FnOnce(&tumult_lake::Writer) -> Result<(), String> + Send + 'static) {
        self.runtime
            .block_on(self.ingest.write(tumult_ingest::Batch::Exec(Box::new(f))))
            .expect("a write");
    }

    /// Change the lake behind the API, as someone with the file could.
    fn tamper(&self, sql: String) {
        self.write(move |w| w.execute(&sql, []).map(|_| ()).map_err(|e| e.to_string()));
    }

    /// A user with `role` and a `kro_` token for it; the token.
    fn user_with_token(&self, username: &str, role: &str) -> String {
        let user = tumult_lake::UserRow {
            id: format!("u-{username}"),
            username: username.into(),
            password_hash: tumult_auth::hash_password("a long test password").expect("a hash"),
            role: role.into(),
            must_change: false,
            disabled: false,
            created_at_ns: now_ns(),
        };
        let token = tumult_auth::new_token();
        let row = tumult_lake::TokenRow {
            id: format!("t-{username}"),
            user_id: user.id.clone(),
            name: username.into(),
            token_hash: tumult_auth::sha256_hex(&token),
            created_at_ns: now_ns(),
            last_used_at_ns: None,
            revoked: false,
            expires_at_ns: None,
        };
        self.write(move |w| {
            w.create_user(&user).map_err(|e| e.to_string())?;
            w.create_token(&row).map_err(|e| e.to_string())
        });
        token
    }
}

/// Serve the daemon API over the lake at `db_path`; its base URL and its
/// single writer.
async fn serve(
    dir: &Path,
    db_path: PathBuf,
    org_file: PathBuf,
) -> (String, tumult_ingest::IngestWriter) {
    let ingest =
        tumult_ingest::IngestWriter::spawn(Store::at(&db_path).writer().expect("a writer"), 16).0;
    let reports = dir.join("reports");
    std::fs::create_dir_all(&reports).expect("reports dir");
    let metrics = Path::new(env!("CARGO_MANIFEST_DIR")).join("../metrics");
    // No model is asked anything here: a closed port stands in for one.
    let llm = Arc::new(tumult_intelligence::llm::OpenAiCompatClient::new(
        "http://127.0.0.1:1".into(),
        None,
        "test-model".into(),
    ));
    let org = tumult_compliance::OrgTree::from_yaml(ORG_FILE).expect("an org tree");
    let score = tumult_api::score::ScoreRuntime::open(
        Some(org_file),
        None,
        std::time::Duration::ZERO,
        now_ns(),
    );
    let state = tumult_api::ApiState::new(
        db_path,
        metrics,
        reports,
        llm,
        org,
        Some(ingest.clone()),
        None,
        None,
        false,
    )
    .with_score_runtime(Arc::new(score));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let addr = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        axum::serve(listener, tumult_api::router(state))
            .await
            .expect("the API serves");
    });
    (format!("http://{addr}"), ingest)
}

fn now_ns() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .expect("a time in range")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Assert that `output` is a success and give its stdout.
fn ok(output: &Output) -> String {
    assert!(
        output.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout(output),
        stderr(output)
    );
    stdout(output)
}

fn json_of(output: &Output) -> Value {
    let text = ok(output);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// A failover of the payments database, as a TOON record body.
const FAILOVER_TOON: &str = r"experiment_name: Payments database failover
exercise_type: failover
executed_at_ns: 1795000000000000000
hypothesis: The replica takes over within five minutes
method: Stop the primary and promote the replica
outcome_status: passed
hypothesis_met: true
target_system: payments-db
target_environment: production
recovery_time_s: 240
rto_target_s: 300
rpo_target_s: 60
rpo_measured_s: 0
data_verified: true
attestation: I attest this record is the exercise as it was run.
participants[2]{name,role}:
  alice,dr_owner
  bob,dba
steps[2]{title,actor,result}:
  Stop the primary,bob,done
  Promote the replica,bob,done
actions[1]{title,owner}:
  Write the failover runbook down,carol
links[1]{kind,linked_id}:
  function,cif-0001
";

/// A restore test, as a JSON record body.
const RESTORE_JSON: &str = r#"{
  "experiment_name": "Ledger restore from backup",
  "exercise_type": "restore",
  "executed_at_ns": 1794000000000000000,
  "hypothesis": "Last night's backup restores in an hour",
  "method": "Restore the backup into a new instance",
  "outcome_status": "partial",
  "target_system": "ledger",
  "target_environment": "staging",
  "attestation": "I attest this record is the exercise as it was run."
}"#;

/// A small PDF: the bytes start as every PDF does.
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, b| {
            write!(hex, "{b:02x}").expect("a hex digit");
            hex
        })
}

/// Create a record from `body` written to `name`, entered by `by`; its id.
fn create(daemon: &Daemon, name: &str, body: &str, by: &str) -> String {
    std::fs::write(daemon.path(name), body).expect("record file");
    let text = ok(&daemon.tumult(&["manual", "create", "--file", name, "--by", by]));
    id_in(&text)
}

/// The id a write prints: the last word of its first line.
fn id_in(text: &str) -> String {
    text.lines()
        .next()
        .and_then(|line| line.split_whitespace().last())
        .unwrap_or_else(|| panic!("no id in {text:?}"))
        .to_string()
}

fn show(daemon: &Daemon, id: &str) -> Value {
    json_of(&daemon.tumult(&["manual", "show", id, "--format", "json"]))
}

#[test]
fn a_record_from_a_toon_file_is_a_draft_with_every_part_of_it() {
    let daemon = Daemon::start();
    let id = create(&daemon, "failover.toon", FAILOVER_TOON, "alice");

    let detail = show(&daemon, &id);
    let record = &detail["experiment"];
    assert_eq!(record["status"], "draft", "{detail}");
    assert_eq!(record["entered_by"], "alice");
    assert_eq!(record["exercise_type"], "failover");
    assert_eq!(record["rto_target_s"], 300.0);
    assert_eq!(detail["steps"].as_array().map(Vec::len), Some(2));
    assert_eq!(detail["actions"][0]["owner"], "carol");
    assert_eq!(detail["links"][0]["linked_id"], "cif-0001");
    assert_eq!(detail["signoffs_required"], 2);
    assert_eq!(detail["signoffs_needed"], 2);

    let text = ok(&daemon.tumult(&["manual", "show", &id]));
    for line in [
        "Payments database failover",
        "failover, draft, outcome passed",
        "payments-db in production",
        "RTO planned 300 s, actual 240 s",
        "RPO planned 60 s, actual 0 s",
        "1. Stop the primary (bob): done",
        "2. Promote the replica (bob): done",
        "1. Write the failover runbook down, owner carol, open",
        "function cif-0001",
        "sign-offs: 0 of 2, 2 still needed",
    ] {
        assert!(text.contains(line), "no {line:?} in:\n{text}");
    }
}

#[test]
fn a_record_from_a_json_file_is_listed_and_the_filters_narrow_the_list() {
    let daemon = Daemon::start();
    let failover = create(&daemon, "failover.toon", FAILOVER_TOON, "alice");
    let restore = create(&daemon, "restore.json", RESTORE_JSON, "alice");

    let listed = |args: &[&str]| -> Vec<String> {
        let mut argv = vec!["manual", "list", "--format", "json"];
        argv.extend_from_slice(args);
        json_of(&daemon.tumult(&argv))["records"]
            .as_array()
            .expect("records")
            .iter()
            .map(|r| r["id"].as_str().expect("an id").to_string())
            .collect()
    };
    let mut all = listed(&[]);
    all.sort();
    let mut both = vec![failover.clone(), restore.clone()];
    both.sort();
    assert_eq!(all, both);
    assert_eq!(listed(&["--type", "restore"]), vec![restore.clone()]);
    assert_eq!(listed(&["--target", "payments-db"]), vec![failover.clone()]);
    assert_eq!(
        listed(&["--status", "draft", "--type", "failover"]),
        vec![failover.clone()]
    );
    assert!(listed(&["--status", "submitted"]).is_empty());
    // 1795000000 s is 2026-11-18 and 1794000000 s is 2026-11-06 (UTC).
    assert_eq!(listed(&["--from", "2026-11-10"]), vec![failover.clone()]);
    assert_eq!(listed(&["--to", "2026-11-06"]), vec![restore.clone()]);
    assert_eq!(
        listed(&["--from", "2026-11-18T00:00:00Z", "--to", "2026-11-19"]),
        vec![failover.clone()]
    );

    let text = ok(&daemon.tumult(&["manual", "list"]));
    assert!(text.starts_with("2 records\n"), "{text}");
    let row = text
        .lines()
        .find(|line| line.starts_with(&restore))
        .unwrap_or_else(|| panic!("no row of {restore} in:\n{text}"));
    for part in [
        "draft",
        "restore",
        "2026-11-06",
        "ledger in staging",
        "Ledger restore from backup",
    ] {
        assert!(row.contains(part), "no {part:?} in {row:?}");
    }
    let none = ok(&daemon.tumult(&["manual", "list", "--type", "tabletop"]));
    assert_eq!(none, "no manual records\n");

    let bad = daemon.tumult(&["manual", "list", "--from", "next week"]);
    assert_eq!(bad.status.code(), Some(1), "{}", stderr(&bad));
    assert!(stderr(&bad).contains("next week"), "{}", stderr(&bad));
}

#[test]
fn a_field_the_api_does_not_know_is_refused_and_named() {
    let daemon = Daemon::start();
    let body = FAILOVER_TOON.replace("rpo_measured_s: 0\n", "rpo_lost_s: 0\n");
    std::fs::write(daemon.path("typo.toon"), body).expect("record file");

    let refused = daemon.tumult(&["manual", "create", "--file", "typo.toon", "--by", "alice"]);

    assert_eq!(refused.status.code(), Some(1), "{}", stdout(&refused));
    assert!(
        stderr(&refused).contains("rpo_lost_s"),
        "{}",
        stderr(&refused)
    );
    assert!(stderr(&refused).contains("422"), "{}", stderr(&refused));
    let listed = json_of(&daemon.tumult(&["manual", "list", "--format", "json"]));
    assert_eq!(listed["records"], serde_json::json!([]), "{listed}");
}

#[test]
fn a_record_is_signed_off_by_two_other_users_and_its_trail_verifies() {
    let daemon = Daemon::start();
    let id = create(&daemon, "failover.toon", FAILOVER_TOON, "alice");
    let submitted = ok(&daemon.tumult(&["manual", "submit", &id, "--by", "alice"]));
    assert_eq!(submitted, format!("submitted {id}\n"));

    let own = daemon.tumult(&["manual", "sign-off", &id, "--approve", "--by", "alice"]);
    assert_eq!(own.status.code(), Some(1), "{}", stdout(&own));
    assert!(stderr(&own).contains("422"), "{}", stderr(&own));

    let first = ok(&daemon.tumult(&["manual", "sign-off", &id, "--approve", "--by", "bob"]));
    assert_eq!(
        first,
        format!("approved {id}: status submitted, 1 sign-off still needed\n")
    );
    let second = ok(&daemon.tumult(&[
        "manual",
        "sign-off",
        &id,
        "--approve",
        "--by",
        "carol",
        "--note",
        "Checked the log",
    ]));
    assert_eq!(second, format!("approved {id}: status verified\n"));

    let detail = show(&daemon, &id);
    assert_eq!(detail["experiment"]["status"], "verified", "{detail}");
    assert_eq!(detail["signoffs_needed"], 0);
    let text = ok(&daemon.tumult(&["manual", "show", &id]));
    assert!(text.contains("sign-offs: 2 of 2, 0 still needed"), "{text}");
    assert!(text.contains("approved by carol"), "{text}");
    assert!(text.contains("Checked the log"), "{text}");

    let verified = ok(&daemon.tumult(&["manual", "verify", &id]));
    assert!(verified.starts_with(&format!("{id}: ok")), "{verified}");
    let verdict = json_of(&daemon.tumult(&["manual", "verify", &id, "--format", "json"]));
    assert_eq!(verdict["ok"], true, "{verdict}");
}

#[test]
fn a_reject_needs_a_note_and_rejects_the_record() {
    let daemon = Daemon::start();
    let id = create(&daemon, "restore.json", RESTORE_JSON, "alice");
    ok(&daemon.tumult(&["manual", "submit", &id, "--by", "alice"]));

    let both = daemon.tumult(&[
        "manual",
        "sign-off",
        &id,
        "--approve",
        "--reject",
        "--by",
        "bob",
    ]);
    assert_eq!(both.status.code(), Some(2), "{}", stderr(&both));
    let bare = daemon.tumult(&["manual", "sign-off", &id, "--reject", "--by", "bob"]);
    assert_eq!(bare.status.code(), Some(2), "{}", stderr(&bare));
    assert!(stderr(&bare).contains("--note"), "{}", stderr(&bare));

    let rejected = ok(&daemon.tumult(&[
        "manual",
        "sign-off",
        &id,
        "--reject",
        "--note",
        "The restore was not checked",
        "--by",
        "bob",
    ]));
    assert_eq!(rejected, format!("rejected {id}: status rejected\n"));
    assert_eq!(show(&daemon, &id)["experiment"]["status"], "rejected");
}

#[test]
fn verify_exits_1_and_prints_each_problem_when_the_lake_was_changed() {
    let daemon = Daemon::start();
    let id = create(&daemon, "restore.json", RESTORE_JSON, "alice");
    ok(&daemon.tumult(&["manual", "submit", &id, "--by", "alice"]));
    ok(&daemon.tumult(&["manual", "sign-off", &id, "--approve", "--by", "bob"]));
    ok(&daemon.tumult(&["manual", "verify", &id]));

    daemon.tamper(format!(
        "UPDATE manual_experiments SET reviewed_by = 'mallory' WHERE id = '{id}'"
    ));

    let text = daemon.tumult(&["manual", "verify", &id]);
    assert_eq!(text.status.code(), Some(1), "{}", stderr(&text));
    let out = stdout(&text);
    assert!(out.starts_with(&format!("{id}: NOT ok")), "{out}");
    assert!(
        out.lines().any(|line| line.contains("manual_experiments")
            && line.contains("reviewed_by")
            && line.contains("mallory")),
        "{out}"
    );
    let json = daemon.tumult(&["manual", "verify", &id, "--format", "json"]);
    assert_eq!(json.status.code(), Some(1), "{}", stderr(&json));
    let verdict: Value = serde_json::from_str(&stdout(&json)).expect("JSON");
    assert_eq!(verdict["ok"], false, "{verdict}");
    assert!(!verdict["problems"].as_array().expect("problems").is_empty());
}

#[test]
fn a_file_and_a_link_are_attached_as_evidence() {
    let daemon = Daemon::start();
    let id = create(&daemon, "failover.toon", FAILOVER_TOON, "alice");
    std::fs::write(daemon.path("runbook-log.pdf"), PDF).expect("pdf");

    let file = ok(&daemon.tumult(&["manual", "attach", &id, "runbook-log.pdf", "--by", "alice"]));
    assert!(file.starts_with("attached runbook-log.pdf"), "{file}");
    let link = ok(&daemon.tumult(&[
        "manual",
        "attach",
        &id,
        "--link",
        "https://wiki.example.com/failover",
        "--label",
        "write-up",
        "--by",
        "alice",
    ]));
    assert!(
        link.starts_with("attached https://wiki.example.com/failover"),
        "{link}"
    );

    let detail = show(&daemon, &id);
    let attachments = detail["attachments"].as_array().expect("attachments");
    assert_eq!(attachments.len(), 2, "{detail}");
    let hash = sha256(PDF);
    let stored = attachments
        .iter()
        .find(|a| a["kind"] == "file")
        .expect("a file");
    assert_eq!(stored["file_hash"], hash.as_str());
    assert_eq!(stored["media_type"], "application/pdf");
    assert_eq!(stored["label"], "runbook-log.pdf");
    let url = attachments
        .iter()
        .find(|a| a["kind"] == "url")
        .expect("a link");
    assert_eq!(url["uri"], "https://wiki.example.com/failover");
    assert_eq!(url["label"], "write-up");

    let text = ok(&daemon.tumult(&["manual", "show", &id]));
    assert!(
        text.contains(&format!(
            "file runbook-log.pdf, application/pdf, {} bytes, sha256 {hash}",
            PDF.len()
        )),
        "{text}"
    );
    assert!(
        text.contains("url https://wiki.example.com/failover (write-up)"),
        "{text}"
    );

    let neither = daemon.tumult(&["manual", "attach", &id, "--by", "alice"]);
    assert_eq!(neither.status.code(), Some(2), "{}", stderr(&neither));
    let wrong = daemon.tumult(&["manual", "attach", &id, "missing.pdf", "--by", "alice"]);
    assert_eq!(wrong.status.code(), Some(1), "{}", stderr(&wrong));
    assert!(stderr(&wrong).contains("missing.pdf"), "{}", stderr(&wrong));
}

/// A plan for the next failover drill of the payments database.
const PLAN_TOON: &str = "name: Payments failover next year
exercise_type: failover
target_system: payments-db
target_environment: production
owner: alice
due_at_ns: 1800000000000000000
rto_target_s: 300
";

#[test]
fn plans_are_created_listed_shown_and_cancelled() {
    let daemon = Daemon::start();
    std::fs::write(daemon.path("plan.toon"), PLAN_TOON).expect("plan file");

    let created = ok(&daemon.tumult(&[
        "manual",
        "plans",
        "create",
        "--file",
        "plan.toon",
        "--by",
        "alice",
    ]));
    assert!(created.starts_with("created plan "), "{created}");
    let id = id_in(&created);

    let text = ok(&daemon.tumult(&["manual", "plans", "list"]));
    assert!(text.starts_with("1 plan\n"), "{text}");
    let row = text.lines().nth(1).unwrap_or_default();
    for part in [
        id.as_str(),
        "planned",
        "failover",
        "due 2027-01-15",
        "payments-db in production",
        "Payments failover next year",
    ] {
        assert!(row.contains(part), "no {part:?} in {row:?}");
    }
    let plans = json_of(&daemon.tumult(&[
        "manual",
        "plans",
        "list",
        "--target",
        "payments-db",
        "--format",
        "json",
    ]));
    assert_eq!(plans["plans"][0]["id"], id.as_str(), "{plans}");
    assert_eq!(
        plans["plans"][0]["interval_days"], 365,
        "a DR plan comes back yearly"
    );

    let shown = ok(&daemon.tumult(&["manual", "plans", "show", &id]));
    assert!(shown.contains("owner alice"), "{shown}");
    assert!(shown.contains("every 365 days"), "{shown}");
    assert!(shown.contains("create by alice"), "{shown}");

    let cancelled = ok(&daemon.tumult(&[
        "manual",
        "plans",
        "cancel",
        &id,
        "--reason",
        "The database moves to another host",
        "--by",
        "alice",
    ]));
    assert_eq!(cancelled, format!("cancelled plan {id}\n"));
    let detail = json_of(&daemon.tumult(&["manual", "plans", "show", &id, "--format", "json"]));
    assert_eq!(detail["plan"]["status"], "cancelled", "{detail}");
    assert_eq!(detail["audit"].as_array().map(Vec::len), Some(2));
    let left = json_of(&daemon.tumult(&[
        "manual", "plans", "list", "--status", "planned", "--format", "json",
    ]));
    assert_eq!(left["plans"], serde_json::json!([]), "{left}");
}

#[test]
fn with_auth_on_the_token_says_who_acts_and_by_is_not_used() {
    let daemon = Daemon::start();
    let alice = daemon.user_with_token("alice", "operator");
    let bob = daemon.user_with_token("bob", "approver");
    let carol = daemon.user_with_token("carol", "approver");
    std::fs::write(daemon.path("failover.toon"), FAILOVER_TOON).expect("record file");

    let anonymous = daemon.tumult(&["manual", "list"]);
    assert_eq!(anonymous.status.code(), Some(1), "{}", stdout(&anonymous));
    assert!(stderr(&anonymous).contains("401"), "{}", stderr(&anonymous));

    let id = id_in(&ok(
        &daemon.tumult_as(&alice, &["manual", "create", "--file", "failover.toon"])
    ));
    ok(&daemon.tumult_as(&alice, &["manual", "submit", &id]));
    let not_allowed = daemon.tumult_as(&alice, &["manual", "sign-off", &id, "--approve"]);
    assert_eq!(
        not_allowed.status.code(),
        Some(1),
        "{}",
        stdout(&not_allowed)
    );
    assert!(
        stderr(&not_allowed).contains("403"),
        "{}",
        stderr(&not_allowed)
    );
    ok(&daemon.tumult_as(
        &bob,
        &["manual", "sign-off", &id, "--approve", "--by", "mallory"],
    ));
    ok(&daemon.tumult_as(&carol, &["manual", "sign-off", &id, "--approve"]));

    let detail = json_of(
        &daemon
            .command(&["manual", "show", &id, "--format", "json"])
            .env("TUMULT_DAEMON_TOKEN", &bob)
            .output()
            .expect("run tumult"),
    );
    assert_eq!(detail["experiment"]["entered_by"], "alice", "{detail}");
    assert_eq!(detail["experiment"]["status"], "verified");
    let signers: Vec<&str> = detail["signoffs"]
        .as_array()
        .expect("sign-offs")
        .iter()
        .filter_map(|s| s["signed_by"].as_str())
        .collect();
    assert_eq!(signers, ["bob", "carol"]);
}

#[test]
fn without_a_daemon_url_the_command_says_how_to_give_one() {
    let daemon = Daemon::start();
    let output = daemon
        .command(&["manual", "list"])
        .env_remove("TUMULT_DAEMON_URL")
        .output()
        .expect("run tumult");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("TUMULT_DAEMON_URL"),
        "{}",
        stderr(&output)
    );
    let given = ok(&daemon
        .command(&["manual", "list", "--daemon-url", &daemon.base])
        .env_remove("TUMULT_DAEMON_URL")
        .output()
        .expect("run tumult"));
    assert_eq!(given, "no manual records\n");
}
