//! Files as evidence of a manual record.
//!
//! `POST /api/manual/experiments/{id}/attachments?name=<file name>` with
//! `Content-Type: application/octet-stream` takes the bytes of one file as
//! the body, and `added_by` as a query parameter (read only while auth is
//! open). The body is read as it arrives: the file goes to a temporary
//! file in the attachments folder of the lake, and the upload stops at
//! 25 MB (413). The type is read from the bytes (415 with the allowed
//! types). The record may be in any status: the attach is an audit row
//! that leaves the seal as it was. A browser on another site cannot send
//! this content type without asking first (a CORS preflight), so no page
//! can attach a file in the name of a signed-in user.
//!
//! `GET /api/manual/experiments/{id}/attachments/{attachment_id}/file`
//! gives the file back as a download, never as a page a browser runs:
//! `Content-Disposition: attachment`, `X-Content-Type-Options: nosniff`
//! and `Content-Security-Policy: sandbox`. A stored file that no longer
//! has the bytes of its hash is not served (409).

use std::pin::Pin;

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::{FileError, FileUpload, ManualError, StagedFile, MAX_FILE_BYTES, MEDIA_TYPES};

use crate::auth::Principal;
use crate::error::{bad_request, conflict, not_found, ApiError};
use crate::manual::{actor_or, exec_manual, in_scopes, manual_error};
use crate::sql_util::{internal, with_reader};
use crate::ApiState;

/// File bytes in flight between the body and the file on disk.
const IN_FLIGHT: usize = 4;
/// The most of a refused body that is still read, and dropped, so a client
/// that is still sending gets the answer and not a reset connection.
const DRAIN_LIMIT: u64 = MAX_FILE_BYTES;

/// What the body of an attach request is, by its `Content-Type`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    /// A link, as JSON.
    Link,
    /// The bytes of a file.
    File,
    /// Anything else, refused.
    Other,
}

/// What `headers` say the body of an attach request is.
pub(crate) fn sent(headers: &HeaderMap) -> Sent {
    let essence = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| t.split(';').next())
        .map(|t| t.trim().to_ascii_lowercase())
        .unwrap_or_default();
    match essence.as_str() {
        "application/octet-stream" => Sent::File,
        "application/json" => Sent::Link,
        t if t.starts_with("application/") && t.ends_with("+json") => Sent::Link,
        _ => Sent::Other,
    }
}

/// 415 for a body that is neither a link nor the bytes of a file. The body
/// is read and dropped first.
pub(crate) async fn not_a_link_or_a_file(request: Request) -> ApiError {
    drain(&mut request.into_body(), DRAIN_LIMIT).await;
    let body = json!({"error": "send a link as application/json, or the bytes of a file \
        as application/octet-stream with ?name=<file name>"});
    (StatusCode::UNSUPPORTED_MEDIA_TYPE, Json(body))
        .into_response()
        .into()
}

/// 413: the file is larger than a file may be.
fn too_large() -> ApiError {
    let body = json!({"error": FileError::TooLarge.to_string(), "limit_bytes": MAX_FILE_BYTES});
    (StatusCode::PAYLOAD_TOO_LARGE, Json(body))
        .into_response()
        .into()
}

/// Map a refused file to its answer: 413, 415 with the allowed types, 400
/// or 500.
fn file_error(err: &FileError) -> ApiError {
    match err {
        FileError::TooLarge => too_large(),
        FileError::Unsupported(_) => {
            let body = json!({"error": err.to_string(), "allowed": MEDIA_TYPES});
            (StatusCode::UNSUPPORTED_MEDIA_TYPE, Json(body))
                .into_response()
                .into()
        }
        FileError::Invalid(msg) => bad_request(msg.clone()),
        FileError::Io(e) => internal(e.to_string()),
    }
}

/// The next bytes of `body`, `None` at its end.
async fn next_bytes(body: &mut Body) -> Result<Option<Bytes>, ApiError> {
    loop {
        let frame = std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await;
        let Some(frame) = frame else {
            return Ok(None);
        };
        let frame = frame.map_err(|e| bad_request(format!("the body could not be read: {e}")))?;
        if let Ok(data) = frame.into_data() {
            return Ok(Some(data));
        }
    }
}

/// Read the rest of `body` and drop it, at most `limit` bytes.
async fn drain(body: &mut Body, limit: u64) {
    let mut drained = 0;
    while drained < limit {
        match next_bytes(body).await {
            Ok(Some(data)) => drained += data.len() as u64,
            Ok(None) | Err(_) => return,
        }
    }
}

/// Receive `body` as the file `name`, into a temporary file in `dir`. The
/// bytes go to the disk on the blocking pool as they arrive.
async fn receive(
    body: &mut Body,
    dir: std::path::PathBuf,
    name: String,
) -> Result<StagedFile, ApiError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(IN_FLIGHT);
    let writer = tokio::task::spawn_blocking(move || {
        let mut upload = FileUpload::begin(&dir)?;
        while let Some(chunk) = rx.blocking_recv() {
            upload.write(&chunk)?;
        }
        upload.finish(&name)
    });
    let streamed = async {
        while let Some(chunk) = next_bytes(body).await? {
            // A closed channel: the writer stopped, and says why below.
            if tx.send(chunk).await.is_err() {
                break;
            }
        }
        Ok::<(), ApiError>(())
    }
    .await;
    drop(tx);
    let written = writer
        .await
        .map_err(|e| internal(format!("the file task failed: {e}")))?;
    // The writer's refusal (too large, not allowed) comes first: the body
    // was not read to its end after it.
    let staged = written.map_err(|e| file_error(&e))?;
    streamed?;
    Ok(staged)
}

/// 404 unless record `id` is there and in the principal's scopes, checked
/// before the body is read; the write checks it again.
async fn check_record(state: &ApiState, principal: &Principal, id: &str) -> Result<(), ApiError> {
    let scopes = principal.env_scopes.clone();
    let lookup = id.to_string();
    let found = with_reader(&state.db_path, move |reader| {
        let detail = reader
            .manual_experiment_detail(&lookup)
            .map_err(|e| e.to_string())?;
        Ok(detail.is_some_and(|d| {
            scopes.is_empty() || in_scopes(d.experiment["target_environment"].as_str(), &scopes)
        }))
    })
    .await?;
    if !found {
        return Err(manual_error(&ManualError::NotFound(id.to_string())));
    }
    Ok(())
}

/// The query of a file upload.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadQuery {
    /// The original file name, kept as the label.
    name: Option<String>,
    /// Who attaches the file, read only while auth is open.
    added_by: Option<String>,
}

/// `POST /api/manual/experiments/{id}/attachments?name=` with the bytes of
/// one file as the body: store it and attach it to the record. A refused
/// body is read and dropped before the answer.
pub(crate) async fn upload(
    state: ApiState,
    principal: Principal,
    id: String,
    request: Request,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (parts, mut body) = request.into_parts();
    let stored = store(&state, &principal, id, &parts.uri, &mut body).await;
    if stored.is_err() {
        drain(&mut body, DRAIN_LIMIT).await;
    }
    stored
}

/// Receive `body` as the file the query of `uri` names and attach it to
/// record `id`.
async fn store(
    state: &ApiState,
    principal: &Principal,
    id: String,
    uri: &Uri,
    body: &mut Body,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let Query(query) = Query::<UploadQuery>::try_from_uri(uri)
        .map_err(|e| bad_request(format!("invalid query: {e}")))?;
    let name = query
        .name
        .ok_or_else(|| bad_request("send the file name as ?name=<file name>"))?;
    let added_by = actor_or(principal, query.added_by, "added_by").map_err(|e| manual_error(&e))?;
    check_record(state, principal, &id).await?;
    let dir = tumult_lake::attachments_dir(&state.db_path);
    let staged = receive(body, dir, name).await?;
    let mut answer = json!({
        "kind": "file",
        "file_hash": staged.hash(),
        "media_type": staged.media_type(),
        "size_bytes": staged.size_bytes(),
        "label": staged.name(),
    });
    let resource_id = id.clone();
    let attachment_id = exec_manual(state, principal, Some(&resource_id), move |w| {
        w.add_manual_file(&id, staged, &added_by)
    })
    .await?;
    answer["id"] = json!(attachment_id);
    Ok((StatusCode::CREATED, Json(answer)))
}

/// A file name that is safe in a quoted `Content-Disposition` value.
fn download_name(label: &str) -> String {
    let safe: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " ._-()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim();
    if safe.is_empty() {
        return "attachment".into();
    }
    safe.into()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `GET /api/manual/experiments/{id}/attachments/{attachment_id}/file`:
/// the stored file of a file attachment, as a download. 404 for a record
/// outside the principal's scopes, an unknown attachment or a link.
pub async fn download(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path((id, attachment_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let scopes = principal.env_scopes.clone();
    let attachment = with_reader(&state.db_path, move |reader| {
        let Some(detail) = reader
            .manual_experiment_detail(&id)
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let env = detail.experiment["target_environment"].as_str();
        if !scopes.is_empty() && !in_scopes(env, &scopes) {
            return Ok(None);
        }
        Ok(detail
            .attachments
            .into_iter()
            .find(|a| a["id"] == attachment_id.as_str()))
    })
    .await?;
    let Some(attachment) = attachment.filter(|a| a["kind"] == "file") else {
        return Err(not_found("no file attachment with this id"));
    };
    let hash = attachment["file_hash"].as_str().unwrap_or_default();
    let dir = tumult_lake::attachments_dir(&state.db_path);
    let Some(path) = tumult_lake::stored_file(&dir, hash) else {
        return Err(conflict(
            "the attachment names no stored file: run the audit verify",
        ));
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(conflict("the stored file is missing: run the audit verify"));
        }
        Err(e) => return Err(internal(e.to_string())),
    };
    if sha256_hex(&bytes) != hash {
        return Err(conflict(
            "the stored file does not match its hash: run the audit verify",
        ));
    }
    let stored_type = attachment["media_type"].as_str().unwrap_or_default();
    let media_type = MEDIA_TYPES
        .iter()
        .find(|t| **t == stored_type)
        .copied()
        .unwrap_or("application/octet-stream");
    let label = download_name(attachment["label"].as_str().unwrap_or_default());
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{label}\""))
        .map_err(|e| internal(e.to_string()))?;
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(media_type)),
            (header::CONTENT_DISPOSITION, disposition),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("sandbox"),
            ),
        ],
        Body::from(Bytes::from(bytes)),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_name_keeps_only_safe_characters() {
        assert_eq!(download_name("runbook.pdf"), "runbook.pdf");
        assert_eq!(download_name("a\"b\r\nc;d.pdf"), "a_b__c_d.pdf");
        assert_eq!(download_name("återställning.log"), "_terst_llning.log");
        assert_eq!(download_name(""), "attachment");
    }

    #[test]
    fn the_content_type_says_a_link_a_file_or_neither() {
        let with = |t: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(t).unwrap());
            sent(&headers)
        };
        assert_eq!(with("application/json"), Sent::Link);
        assert_eq!(with("Application/JSON; charset=utf-8"), Sent::Link);
        assert_eq!(with("application/merge-patch+json"), Sent::Link);
        assert_eq!(with("application/octet-stream"), Sent::File);
        for other in [
            "multipart/form-data; boundary=x",
            "text/plain",
            "application/pdf",
        ] {
            assert_eq!(with(other), Sent::Other, "{other}");
        }
        assert_eq!(sent(&HeaderMap::new()), Sent::Other);
    }
}
