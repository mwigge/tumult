//! Journal import endpoint (`/api/import/journal`).
//!
//! The CLI's daemon-first auto-ingest (`TUMULT_DAEMON_URL`) POSTs a finished
//! experiment journal here instead of opening the `DuckDB` store itself —
//! a direct open would lose to the daemon's single-writer lock. The write
//! rides the daemon's single-writer channel via [`tumult_ingest::Batch::Exec`];
//! this handler never opens a write connection of its own.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_core::types::{Experiment, Journal};
use tumult_ingest::Batch;
use tumult_lake::Writer;

use crate::sql_util::internal;
use crate::ApiState;

/// JSON body: the journal plus its experiment definition when the caller has
/// it (the definition enriches the `ChaosGraph` with the full
/// fault/service model).
#[derive(Debug, Deserialize)]
pub struct ImportJournalRequest {
    journal: Journal,
    experiment: Option<Experiment>,
}

/// `POST /api/import/journal` — ingest one experiment journal into the
/// analytics tables. Idempotent: a known `experiment_id` is skipped as a
/// duplicate and reported as `{"ingested": false}`.
pub async fn import_journal(
    State(state): State<ApiState>,
    Json(req): Json<ImportJournalRequest>,
) -> Result<Json<Value>, Response> {
    let Some(ingest) = state.ingest_handle() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "journal import is not wired (no ingest handle)"})),
        )
            .into_response());
    };
    let experiment_id = req.journal.experiment_id.clone();
    let journal = req.journal;
    let experiment = req.experiment;

    // The `Batch::Exec` closure itself always "succeeds" so the channel ack
    // stays clean; the real outcome travels in `slot`.
    let slot = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            *slot2.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(writer.ingest_journal(&journal, experiment.as_ref()));
            Ok(())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;

    let result = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    match result {
        Some(Ok(ingested)) => Ok(Json(json!({
            "ingested": ingested,
            "experiment_id": experiment_id,
        }))),
        Some(Err(e)) => Err(internal(e.to_string())),
        None => Err(internal("journal import did not run".into())),
    }
}
