// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! `/api/ask` with a scripted LLM: a valid generated query runs and returns
//! rows, guard-rejected SQL is a 422, and a non-connectivity LLM failure is
//! a 502 (never a panic or an empty 200).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tumult_intelligence::llm::{AiError, Llm, Message};

use crate::common;

/// Answers every chat with a fixed reply (or failure).
struct StubLlm(std::sync::Mutex<Option<Result<String, AiError>>>);

impl StubLlm {
    fn replying(sql: &str) -> Self {
        Self(std::sync::Mutex::new(Some(Ok(sql.to_string()))))
    }
    fn failing(err: AiError) -> Self {
        Self(std::sync::Mutex::new(Some(Err(err))))
    }
}

#[async_trait::async_trait]
impl Llm for StubLlm {
    async fn chat(&self, _messages: &[Message]) -> Result<String, AiError> {
        self.0
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Ok("SELECT 1".into()))
    }
}

/// Answers every chat with the same reply.
struct SameReply(String);

#[async_trait::async_trait]
impl Llm for SameReply {
    async fn chat(&self, _messages: &[Message]) -> Result<String, AiError> {
        Ok(self.0.clone())
    }
}

/// Add a viewer limited to `scopes` (none: every environment) to the lake
/// at `db`; returns its bearer token. The server reads the lake per
/// request, so the writer is closed before the first one.
fn add_viewer(db: &std::path::Path, username: &str, scopes: &[&str]) -> String {
    let writer = tumult_lake::Store::at(db).writer().unwrap();
    let user_id = format!("u-{username}");
    writer
        .create_user(&tumult_lake::UserRow {
            id: user_id.clone(),
            username: username.into(),
            password_hash: tumult_auth::hash_password("viewer-password").unwrap(),
            role: "viewer".into(),
            must_change: false,
            disabled: false,
            created_at_ns: common::now_ns(),
        })
        .unwrap();
    let scopes: Vec<String> = scopes.iter().map(|s| (*s).to_string()).collect();
    writer.set_user_env_scopes(&user_id, &scopes).unwrap();
    let token = tumult_auth::new_token();
    writer
        .create_token(&tumult_lake::TokenRow {
            id: format!("t-{username}"),
            user_id,
            name: username.into(),
            token_hash: tumult_auth::sha256_hex(&token),
            created_at_ns: common::now_ns(),
            last_used_at_ns: None,
            revoked: false,
            expires_at_ns: None,
        })
        .unwrap();
    token
}

async fn spawn_with_llm(llm: Arc<dyn Llm>) -> (String, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("k.duckdb");
    common::seed(&db_path);
    let metrics_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../metrics")
        .canonicalize()
        .unwrap();
    let state = tumult_api::ApiState::new(
        db_path,
        metrics_dir,
        tmp.path().join("reports"),
        llm,
        tumult_compliance::OrgTree::empty(),
        None,
        None,
        None,
        false,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, tumult_api::router(state))
            .await
            .unwrap();
    });
    (format!("http://{addr}"), tmp)
}

async fn ask(base: &str, question: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/ask"))
        .json(&json!({"question": question}))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

async fn ask_as(base: &str, token: &str, question: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/ask"))
        .bearer_auth(token)
        .json(&json!({"question": question}))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// A viewer's question whose generated SQL lists `users` after an allowed
/// table is refused the way naming `users` alone is, for a viewer of every
/// environment and for one limited to an environment, and no row of
/// `users` comes back.
#[tokio::test]
async fn generated_sql_that_lists_users_after_an_allowed_table_is_refused_like_users_alone() {
    let mut errors = Vec::new();
    for sql in [
        "SELECT u.username FROM run_roots r, users u",
        "SELECT u.username FROM users u",
    ] {
        let (base, tmp) = spawn_with_llm(Arc::new(SameReply(sql.into()))).await;
        let db = tmp.path().join("k.duckdb");
        let tokens = [
            add_viewer(&db, "viewer-everywhere", &[]),
            add_viewer(&db, "viewer-in-demo", &["demo"]),
        ];
        for token in &tokens {
            let (status, body) = ask_as(&base, token, "who can sign in").await;
            assert_eq!(status, 422, "{sql}: {body}");
            assert!(body.get("rows").is_none(), "{sql}: {body}");
            assert!(!body.to_string().contains("viewer-"), "{sql}: {body}");
            errors.push(body["error"].as_str().unwrap().to_string());
        }
    }
    assert!(errors[0].contains("rejected by the guard"), "{errors:?}");
    assert!(errors.iter().all(|e| *e == errors[0]), "{errors:?}");
}

#[tokio::test]
async fn llm_generated_sql_runs_and_returns_rows() {
    let (base, _tmp) = spawn_with_llm(Arc::new(StubLlm::replying(
        "```sql\nSELECT COUNT(*) AS experiments FROM spans WHERE span_name = 'resilience.experiment';\n```",
    )))
    .await;
    let (status, body) = ask(&base, "count my experiments please").await;
    assert_eq!(status, 200);
    assert_eq!(body["source"], json!("llm"));
    // Fences and the trailing semicolon are stripped before execution.
    assert_eq!(body["rows"][0]["experiments"], json!(2));
}

#[tokio::test]
async fn llm_sql_failing_the_guard_is_a_422_with_the_sql() {
    let (base, _tmp) = spawn_with_llm(Arc::new(StubLlm::replying("DROP TABLE spans"))).await;
    let (status, body) = ask(&base, "delete everything").await;
    assert_eq!(status, 422);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("rejected by the guard"));
    assert_eq!(body["sql"], json!("DROP TABLE spans"));
}

#[tokio::test]
async fn llm_sql_that_cannot_execute_is_a_500_with_the_sql() {
    let (base, _tmp) = spawn_with_llm(Arc::new(StubLlm::replying(
        "SELECT no_such_column FROM spans",
    )))
    .await;
    let (status, body) = ask(&base, "show me the frobnicate").await;
    assert_eq!(status, 500);
    assert!(body["error"].as_str().unwrap().contains("query failed"));
    assert_eq!(
        body["sql"],
        json!("SELECT no_such_column FROM spans LIMIT 500")
    );
}

#[tokio::test]
async fn llm_config_errors_are_bad_gateway() {
    let (base, _tmp) = spawn_with_llm(Arc::new(StubLlm::failing(AiError::Config(
        "missing KRONIKA_LLM_API_KEY".into(),
    ))))
    .await;
    let (status, body) = ask(&base, "anything at all").await;
    assert_eq!(status, 502);
    assert_eq!(body["configured"], json!(true));
    assert!(body["error"].as_str().unwrap().contains("LLM call failed"));
}
