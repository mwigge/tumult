//! Resource authorization shared by run reads, controls, approvals and events.
use super::Principal;
use crate::{
    error::not_found,
    sql_util::{sql_string, with_reader},
    ApiState,
};
use axum::response::Response;

/// Predicate for a query with runs aliased `r` and telemetry environment `e`.
/// Durable request context takes precedence over telemetry supplied by clients.
/// Legacy runs with no known environment fail closed for scoped principals.
pub(crate) fn run_scope_sql(scopes: &[String]) -> String {
    if scopes.is_empty() {
        return "TRUE".into();
    }
    let allowed = scopes
        .iter()
        .map(|s| sql_string(s))
        .collect::<Vec<_>>()
        .join(", ");
    format!("COALESCE(\
        (SELECT ar.env FROM approval_requests ar WHERE ar.run_id = r.id), \
        (SELECT json_extract_string(ra.detail, '$.env') FROM run_audit ra \
         WHERE ra.run_id = r.id AND ra.event = 'requested_context' ORDER BY ra.at_ns LIMIT 1), \
        (CASE WHEN EXISTS (SELECT 1 FROM run_registry rg WHERE rg.id = r.registry_id AND rg.kind = 'gameday') \
         THEN json_extract_string(r.params_json, '$.env') END), e.env) IN ({allowed})")
}

pub(crate) async fn authorize_run(
    state: &ApiState,
    principal: &Principal,
    id: &str,
) -> Result<(), Response> {
    if principal.env_scopes.is_empty() {
        return Ok(());
    }
    let predicate = run_scope_sql(&principal.env_scopes);
    let id = id.to_string();
    let allowed = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&format!("SELECT r.id FROM runs r \
            LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env FROM spans GROUP BY 1) e \
            ON e.experiment_id = r.experiment_id WHERE r.id = {} AND ({predicate})", sql_string(&id)))
            .map(|rows| !rows.is_empty()).map_err(|e| e.to_string())
    }).await?;
    if !allowed {
        return Err(not_found("unknown run"));
    }
    Ok(())
}
