//! Webhook CRUD endpoints (`/api/webhooks*`) — admin-managed outbound event
//! sinks (schema v11). The HMAC secret is returned exactly once, at
//! creation (one-time-password idiom); list rows never carry it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::WebhookRow;

use crate::auth::Principal;
use crate::error::{bad_request, not_found, unavailable};
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

/// One webhook as JSON *without* the secret.
fn webhook_json(w: &WebhookRow) -> Value {
    json!({
        "id": w.id,
        "name": w.name,
        "url": w.url,
        "events": w.events,
        "enabled": w.enabled,
        "created_by": w.created_by,
        "created_at_ns": w.created_at_ns,
    })
}

/// Fetch one webhook by id, or a 404 response.
async fn webhook_or_404(state: &ApiState, id: &str) -> Result<WebhookRow, Response> {
    if id.chars().count() > 100 {
        return Err(bad_request("webhook id too long"));
    }
    let lookup = id.to_string();
    let found = with_reader(&state.db_path, move |reader| {
        Ok(reader
            .list_webhooks()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|w| w.id == lookup))
    })
    .await?;
    found.ok_or_else(|| not_found("unknown webhook"))
}

/// `GET /api/webhooks` — every webhook (never the secrets), ordered by name.
pub async fn list(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let hooks = with_reader(&state.db_path, |reader| {
        Ok(reader
            .list_webhooks()
            .map_err(|e| e.to_string())?
            .iter()
            .map(webhook_json)
            .collect::<Vec<_>>())
    })
    .await?;
    Ok(Json(json!({"count": hooks.len(), "webhooks": hooks})))
}

/// JSON body for `POST /api/webhooks`.
#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    name: String,
    url: String,
    #[serde(default)]
    events: Vec<String>,
}

/// `POST /api/webhooks` — create an enabled webhook with a fresh HMAC
/// secret (returned exactly once, in this response). The delivery cursor
/// starts at creation time, so history is not replayed. The URL must pass
/// the SSRF policy (https; local/insecure only via daemon env flags).
pub async fn create(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("name must not be empty"));
    }
    if name.chars().count() > 100 {
        return Err(bad_request("name too long (max 100 chars)"));
    }
    if req.events.iter().any(|e| e.chars().count() > 100) {
        return Err(bad_request("event name too long (max 100 chars)"));
    }
    if let Err(reason) = tumult_ingest::webhooks::validate_webhook_url(&req.url) {
        return Err(bad_request(reason));
    }

    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "webhook storage is not wired (no ingest handle)",
        ));
    };
    let now = now_ns();
    let row = WebhookRow {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        url: req.url,
        secret: tumult_auth::new_session_id(),
        events: req.events,
        enabled: true,
        created_by: principal.actor(),
        created_at_ns: now,
    };
    let row2 = row.clone();
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.create_webhook(&row2).map_err(|e| e.to_string())?;
            // Start the cursor at creation: history is not replayed.
            writer
                .set_webhook_cursor(&row2.id, now)
                .map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let mut body = webhook_json(&row);
    body["secret"] = json!(row.secret);
    Ok((StatusCode::CREATED, Json(body)))
}

/// JSON body for `POST /api/webhooks/{id}/enable`.
#[derive(Debug, Deserialize)]
pub struct EnableWebhookRequest {
    enabled: bool,
}

/// `POST /api/webhooks/{id}/enable {enabled}` — flip a webhook on or off.
pub async fn set_enabled(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<EnableWebhookRequest>,
) -> Result<Json<Value>, Response> {
    webhook_or_404(&state, &id).await?;
    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "webhook storage is not wired (no ingest handle)",
        ));
    };
    let enabled = req.enabled;
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer
                .set_webhook_enabled(&id, enabled)
                .map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}

/// `POST /api/webhooks/{id}/delete` — remove a webhook and its delivery
/// cursor.
pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    webhook_or_404(&state, &id).await?;
    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "webhook storage is not wired (no ingest handle)",
        ));
    };
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.delete_webhook(&id).map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}
