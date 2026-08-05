//! Definition registration shared by run and GameDay validation: the
//! SHA-256 content-hash dedup (`reg-<first 12 hex>` id derivation) plus the
//! hash-lookup-then-insert flow over the daemon's single-writer channel.

use axum::response::Response;
use tumult_lake::RegisteredDefinition;

use crate::error::unavailable;
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

/// SHA-256 hex of a definition — its dedup key; the registry id derives
/// from it (`reg-<first 12 hex>`), so identical TOON always lands on the
/// same registry row.
pub(crate) fn content_hash(text: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The outcome of [`register_definition`]: the registry id and name, plus
/// whether this call inserted the row (`false` = dedup hit).
pub(crate) struct Registration {
    pub id: String,
    pub name: String,
    pub registered: bool,
}

/// Register one definition by content hash (dedup: an identical TOON lands
/// on the existing row, reported with `registered: false`). `gameday`
/// selects the registry `kind` (`'gameday'` vs the NULL experiment
/// default). The dedup lookup runs before the wired-ness check so a dedup
/// hit answers even when the API has no ingest handle; `not_wired` is the
/// 503 message for a genuine registration without one.
pub(crate) async fn register_definition(
    state: &ApiState,
    text: &str,
    name: &str,
    actor: Option<String>,
    gameday: bool,
    not_wired: &'static str,
) -> Result<Registration, Response> {
    let hash = content_hash(text);
    let lookup = hash.clone();
    let existing = with_reader(&state.db_path, move |reader| {
        reader.registry_by_hash(&lookup).map_err(|e| e.to_string())
    })
    .await?;
    if let Some(def) = existing {
        return Ok(Registration {
            id: def.id,
            name: def.name,
            registered: false,
        });
    }

    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(not_wired));
    };
    let def = RegisteredDefinition {
        id: format!("reg-{}", &hash[..12]),
        name: name.to_string(),
        definition_toon: text.to_string(),
        content_hash: hash,
        registered_at_ns: now_ns(),
        registered_by: actor,
    };
    let registration = Registration {
        id: def.id.clone(),
        name: def.name.clone(),
        registered: true,
    };
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            if gameday {
                writer
                    .register_gameday_definition(&def)
                    .map_err(|e| e.to_string())
            } else {
                writer.register_definition(&def).map_err(|e| e.to_string())
            }
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(registration)
}
