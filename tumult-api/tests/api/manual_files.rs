//! Files as evidence over the API: `POST /api/manual/experiments/{id}/attachments?name=`
//! takes the bytes of one file as an `application/octet-stream` body,
//! stores it once in the lake folder by its SHA-256, and
//! `GET .../attachments/{attachment_id}/file` gives it back with headers
//! that keep a browser from running it. A file may be attached after
//! sign-off without changing the seal, and the audit verify checks the
//! stored files and the attach rows.

use std::path::{Path, PathBuf};

use reqwest::Method;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::common::*;

/// A small PDF: the bytes start as every PDF does.
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n\
    trailer << /Root 1 0 R >>\n%%EOF\n";

/// The first bytes of an x86-64 ELF executable.
const ELF: &[u8] = b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\
    \x02\x00\x3e\x00\x01\x00\x00\x00\xb0\x10\x40\x00\x00\x00\x00\x00";

const BOUNDARY: &str = "tumult-test-boundary-7f3a";

/// The cap on one file: 25 MB.
const CAP: usize = 25 * 1024 * 1024;

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A multipart form with `added_by` and one file part, as a browser form
/// on any site can send it.
fn form(name: &str, bytes: &[u8], added_by: &str) -> Vec<u8> {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"added_by\"\r\n\r\n{added_by}\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
         Content-Type: application/pdf\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

/// The upload of `bytes` as the file `name` to record `id`, by `by`.
fn file_request(
    base: &str,
    id: &str,
    name: &str,
    bytes: &[u8],
    by: &str,
) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(format!("{base}/api/manual/experiments/{id}/attachments"))
        .query(&[("name", name), ("added_by", by)])
        .header("content-type", "application/octet-stream")
        .body(bytes.to_vec())
}

/// Send `name` with `bytes` to record `id`; returns the status and the body.
async fn upload(base: &str, id: &str, name: &str, bytes: &[u8], by: &str) -> (u16, Value) {
    let resp = file_request(base, id, name, bytes, by)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn send(base: &str, method: Method, path: &str, body: &Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .request(method, format!("{base}{path}"))
        .json(body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

fn drill_body(by: &str) -> Value {
    json!({
        "experiment_name": "payments-db restore",
        "exercise_type": "restore",
        "executed_at_ns": now_ns() - 3600 * NS,
        "hypothesis": "Last night's backup restores within 900 s",
        "method": "Restore the backup on a spare host and read the orders back",
        "outcome_status": "passed",
        "target_system": "payments-db",
        "target_environment": "production",
        "recovery_time_s": 600.0,
        "entered_by": by,
        "attestation": "I attest this record reflects the exercise as executed."
    })
}

/// A draft entered by alice; returns its id.
async fn draft(base: &str) -> String {
    let (status, made) = send(
        base,
        Method::POST,
        "/api/manual/experiments",
        &drill_body("alice"),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    made["id"].as_str().unwrap().to_string()
}

/// A draft entered by alice, submitted by her and verified by bob.
async fn verified(base: &str) -> String {
    let id = draft(base).await;
    let path = format!("/api/manual/experiments/{id}/submit");
    let (status, body) = send(base, Method::POST, &path, &json!({"by": "alice"})).await;
    assert_eq!(status, 200, "{body}");
    let path = format!("/api/manual/experiments/{id}/verify");
    let (status, body) = send(base, Method::POST, &path, &json!({"reviewer": "bob"})).await;
    assert_eq!(status, 200, "{body}");
    id
}

async fn detail(base: &str, id: &str) -> Value {
    let (status, detail) = get(base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(status, 200, "{detail}");
    detail
}

async fn verdict(base: &str, id: &str) -> Value {
    let (status, verdict) = get(base, &format!("/api/manual/experiments/{id}/audit/verify")).await;
    assert_eq!(status, 200, "{verdict}");
    verdict
}

/// The attachments folder of the test lake.
fn attachments(srv: &TestServer) -> PathBuf {
    srv.db_path.parent().unwrap().join("attachments")
}

fn stored(srv: &TestServer, hash: &str) -> PathBuf {
    attachments(srv).join(&hash[..2]).join(hash)
}

/// Every file under `dir`, found by walking it.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn a_pdf_sent_as_its_bytes_is_stored_by_its_hash_and_downloads_byte_for_byte() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = draft(base).await;

    let (status, made) = upload(base, &id, "runbook.pdf", PDF, "alice").await;
    assert_eq!(status, 201, "{made}");
    let hash = sha256(PDF);
    assert_eq!(made["file_hash"], hash.as_str());
    assert_eq!(made["media_type"], "application/pdf");
    assert_eq!(made["size_bytes"], PDF.len());
    let attachment = made["id"].as_str().unwrap().to_string();

    let detail = detail(base, &id).await;
    let row = &detail["attachments"][0];
    assert_eq!(row["id"], attachment.as_str());
    assert_eq!(row["kind"], "file");
    assert_eq!(row["file_hash"], hash.as_str());
    assert_eq!(row["media_type"], "application/pdf");
    assert_eq!(row["size_bytes"], PDF.len());
    assert_eq!(row["label"], "runbook.pdf");
    assert_eq!(row["added_by"], "alice");
    assert_eq!(files_under(&attachments(&srv)), [stored(&srv, &hash)]);

    let resp = reqwest::get(format!("{base}{}", row["uri"].as_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    assert_eq!(header("content-type").as_deref(), Some("application/pdf"));
    assert_eq!(
        header("content-disposition").as_deref(),
        Some("attachment; filename=\"runbook.pdf\"")
    );
    assert_eq!(header("x-content-type-options").as_deref(), Some("nosniff"));
    assert_eq!(
        header("content-security-policy").as_deref(),
        Some("sandbox")
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), PDF);
}

#[tokio::test]
async fn a_file_one_byte_over_25_mb_is_413_and_leaves_nothing_in_the_attachments_folder() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = draft(base).await;

    let (status, body) = upload(base, &id, "big.log", &vec![b'a'; CAP + 1], "alice").await;
    assert_eq!(status, 413, "{body}");
    assert!(body["error"].as_str().unwrap().contains("25 MB"), "{body}");
    assert_eq!(files_under(&attachments(&srv)), Vec::<PathBuf>::new());
    assert_eq!(detail(base, &id).await["attachments"], json!([]));

    // A file of exactly 25 MB is taken.
    let (status, body) = upload(base, &id, "full.log", &vec![b'a'; CAP], "alice").await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["size_bytes"], CAP);
}

#[tokio::test]
async fn an_executable_named_pdf_is_415_and_names_the_allowed_types() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = draft(base).await;

    let (status, body) = upload(base, &id, "runbook.pdf", ELF, "alice").await;
    assert_eq!(status, 415, "{body}");
    assert_eq!(
        body["allowed"],
        json!([
            "application/pdf",
            "image/png",
            "image/jpeg",
            "text/plain",
            "text/csv",
            "application/json"
        ])
    );
    assert_eq!(files_under(&attachments(&srv)), Vec::<PathBuf>::new());
    assert_eq!(detail(base, &id).await["attachments"], json!([]));
}

#[tokio::test]
async fn a_body_a_page_on_another_site_can_send_without_asking_is_refused() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = draft(base).await;
    let path =
        format!("{base}/api/manual/experiments/{id}/attachments?name=runbook.pdf&added_by=alice");
    // A browser sends these content types across sites with no CORS
    // preflight; `application/octet-stream` and JSON it asks for first.
    for (content_type, body) in [
        (
            format!("multipart/form-data; boundary={BOUNDARY}"),
            form("runbook.pdf", PDF, "alice"),
        ),
        ("text/plain".to_string(), PDF.to_vec()),
        (
            "application/x-www-form-urlencoded".to_string(),
            PDF.to_vec(),
        ),
    ] {
        let resp = reqwest::Client::new()
            .post(&path)
            .header("content-type", &content_type)
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 415, "{content_type}");
        let answer: Value = resp.json().await.unwrap();
        let error = answer["error"].as_str().unwrap();
        assert!(error.contains("application/octet-stream"), "{error}");
    }
    let resp = reqwest::Client::new()
        .post(&path)
        .body(PDF.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 415, "no content type");
    assert_eq!(files_under(&attachments(&srv)), Vec::<PathBuf>::new());
    assert_eq!(detail(base, &id).await["attachments"], json!([]));

    // A file needs its name.
    let resp = reqwest::Client::new()
        .post(format!(
            "{base}/api/manual/experiments/{id}/attachments?added_by=alice"
        ))
        .header("content-type", "application/octet-stream")
        .body(PDF.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
}

#[tokio::test]
async fn the_same_file_sent_twice_is_stored_once() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = draft(base).await;

    let (status, first) = upload(base, &id, "runbook.pdf", PDF, "alice").await;
    assert_eq!(status, 201, "{first}");
    let (status, second) = upload(base, &id, "runbook-again.pdf", PDF, "alice").await;
    assert_eq!(status, 201, "{second}");
    assert_ne!(first["id"], second["id"]);
    assert_eq!(first["file_hash"], second["file_hash"]);
    let hash = sha256(PDF);
    assert_eq!(files_under(&attachments(&srv)), [stored(&srv, &hash)]);
    let rows = detail(base, &id).await["attachments"].clone();
    assert_eq!(rows.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_file_attached_after_verify_keeps_the_seal_and_adds_one_chained_audit_row() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = verified(base).await;
    let before = detail(base, &id).await;

    let (status, made) = upload(base, &id, "runbook.pdf", PDF, "carol").await;
    assert_eq!(status, 201, "{made}");

    let after = detail(base, &id).await;
    assert_eq!(after["experiment"], before["experiment"]);
    let (old, new) = (
        before["audit"].as_array().unwrap(),
        after["audit"].as_array().unwrap(),
    );
    assert_eq!(new.len(), old.len() + 1);
    assert_eq!(new[..old.len()], old[..]);
    let row = new.last().unwrap();
    let seal = &before["experiment"]["content_hash"];
    assert_eq!(row["action"], "attach");
    assert_eq!((&row["prev_hash"], &row["new_hash"]), (seal, seal));
    assert_eq!(row["link_hash"].as_str().map(str::len), Some(64));
    let diff = &row["diff"];
    assert_eq!(diff["attachment_id"], made["id"]);
    assert_eq!(diff["file_hash"], sha256(PDF).as_str());
    let verdict = verdict(base, &id).await;
    assert_eq!(verdict["ok"], true, "{verdict}");
}

/// Run `sql` straight on the lake, past every rule of the API.
async fn tamper(srv: &TestServer, sql: String) {
    exec_write(srv, move |w| {
        w.execute(&sql, []).map(|_| ()).map_err(|e| e.to_string())
    })
    .await;
}

/// Whether a problem of `verdict` names row `row` of `table` and says
/// `says`, with `field` when one is given.
fn names(verdict: &Value, table: &str, row: &str, field: Option<&str>, says: &str) -> bool {
    verdict["ok"] == false
        && verdict["problems"].as_array().unwrap().iter().any(|p| {
            p["table"] == table
                && p["row"] == row
                && field.is_none_or(|f| p["field"] == f)
                && p["problem"].as_str().unwrap().contains(says)
        })
}

#[tokio::test]
async fn verify_names_a_changed_file_a_deleted_file_and_a_deleted_attach_row() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = verified(base).await;
    let log = b"restored 1204 orders\n";
    let (_, pdf) = upload(base, &id, "runbook.pdf", PDF, "carol").await;
    let (_, text) = upload(base, &id, "restore.log", log, "carol").await;
    let (pdf, text) = (pdf["id"].as_str().unwrap(), text["id"].as_str().unwrap());
    assert_eq!(verdict(base, &id).await["ok"], true);
    const TABLE: &str = "evidence_attachments";

    // The stored PDF changed on disk.
    let path = stored(&srv, &sha256(PDF));
    std::fs::write(&path, b"%PDF-1.4 a different runbook").unwrap();
    let changed = verdict(base, &id).await;
    let says = "it was changed";
    assert!(
        names(&changed, TABLE, pdf, Some("file_hash"), says),
        "{changed}"
    );
    // A changed file is not served as the evidence it was.
    let file = format!("/api/manual/experiments/{id}/attachments/{pdf}/file");
    let (status, body) = get(base, &file).await;
    assert_eq!(status, 409, "{body}");
    std::fs::write(&path, PDF).unwrap();
    let resp = reqwest::get(format!("{base}{file}")).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // The stored log deleted.
    let path = stored(&srv, &sha256(log));
    std::fs::remove_file(&path).unwrap();
    let deleted = verdict(base, &id).await;
    let says = "the stored file is missing";
    assert!(
        names(&deleted, TABLE, text, Some("file_hash"), says),
        "{deleted}"
    );
    // The download says the file is missing; a file it cannot read is a
    // fault of the server, not a missing file.
    let file = format!("/api/manual/experiments/{id}/attachments/{text}/file");
    let (status, body) = get(base, &file).await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("missing"),
        "{body}"
    );
    std::fs::create_dir(&path).unwrap();
    let (status, body) = get(base, &file).await;
    assert_eq!(status, 500, "{body}");
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, log).unwrap();
    assert_eq!(verdict(base, &id).await["ok"], true);

    // The attach row of the log, the newest audit row, deleted.
    tamper(
        &srv,
        format!(
            "DELETE FROM manual_experiment_audit WHERE experiment_id = '{id}' \
             AND action = 'attach' AND diff->>'attachment_id' = '{text}'"
        ),
    )
    .await;
    let unnamed = verdict(base, &id).await;
    let says = "no audit row names this attachment";
    assert!(names(&unnamed, TABLE, text, None, says), "{unnamed}");
}

#[tokio::test]
async fn a_link_is_still_json_and_a_link_has_no_file_to_download() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = draft(base).await;
    let link = json!({"kind": "url", "uri": "https://wiki.example/restore", "added_by": "alice"});
    let path = format!("/api/manual/experiments/{id}/attachments");
    let (status, made) = send(base, Method::POST, &path, &link).await;
    assert_eq!(status, 201, "{made}");
    let file = json!({"kind": "file", "uri": "/etc/passwd", "added_by": "alice"});
    let (status, body) = send(base, Method::POST, &path, &file).await;
    assert_eq!(
        status, 400,
        "a file is sent as its bytes, not named in JSON: {body}"
    );

    let link = made["id"].as_str().unwrap();
    let (status, _) = get(base, &format!("{path}/{link}/file")).await;
    assert_eq!(status, 404);
    let (status, _) = get(base, &format!("{path}/no-such-attachment/file")).await;
    assert_eq!(status, 404);
    let (status, body) = upload(base, "no-such-record", "runbook.pdf", PDF, "alice").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(files_under(&attachments(&srv)), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn upload_and_download_keep_to_the_role_and_the_scopes() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let admin = add_scoped_token(&srv, "files-admin", "admin", &[]).await;
    let (status, made) = post_auth(
        base,
        "/api/manual/experiments",
        &admin,
        drill_body("files-admin"),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    let send_as = |token: String| {
        let id = id.clone();
        async move {
            let resp = file_request(base, &id, "runbook.pdf", PDF, "ignored")
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let (status, stored) = send_as(admin.clone()).await;
    assert_eq!(status, 201, "{stored}");
    let file = format!(
        "/api/manual/experiments/{id}/attachments/{}/file",
        stored["id"].as_str().unwrap()
    );

    // A viewer may download but not upload.
    let viewer = add_scoped_token(&srv, "files-viewer", "viewer", &[]).await;
    let resp = reqwest::Client::new()
        .get(format!("{base}{file}"))
        .header("Authorization", format!("Bearer {viewer}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), PDF);
    let (status, _) = send_as(viewer).await;
    assert_eq!(status, 403);

    // Outside the scopes the record does not exist.
    let staging = add_scoped_token(&srv, "files-staging", "operator", &["staging"]).await;
    let (status, _) = send_as(staging.clone()).await;
    assert_eq!(status, 404);
    let (status, _) = get_auth(base, &file, &staging).await;
    assert_eq!(status, 404);
    let (status, detail) = get_auth(base, &format!("/api/manual/experiments/{id}"), &admin).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["attachments"].as_array().unwrap().len(), 1);
}
