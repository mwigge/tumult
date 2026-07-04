//! Caller-identity tool — reports the authenticated principal's access role.
//!
//! `tumult_whoami` is the one tool whose output depends on *who is asking*: it
//! surfaces the [`crate::handler::Role`] the auth layer resolved for the
//! request so a client (the web UI, an agent) can adapt to its own
//! permissions. It is read-only and viewer-callable — a viewer must be able to
//! learn that it is a viewer.

use crate::tools::StructuredReport;

/// Report the caller's resolved role and whether the request was authenticated.
///
/// `role` is the canonical role name (`viewer` or `operator`) the auth layer
/// mapped this request's bearer token to. `authenticated` is `true` when a
/// configured token validated the request, and `false` in loopback open mode
/// (no auth configured) — where every caller has full operator access without
/// presenting a token.
#[must_use]
pub fn whoami(role: &str, authenticated: bool) -> StructuredReport {
    let mut structured = serde_json::Map::new();
    structured.insert("role".into(), serde_json::Value::String(role.to_string()));
    structured.insert(
        "authenticated".into(),
        serde_json::Value::Bool(authenticated),
    );
    let text = format!("role: {role}\nauthenticated: {authenticated}");
    StructuredReport { text, structured }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whoami_reports_viewer() {
        let report = whoami("viewer", true);
        assert_eq!(report.structured["role"], "viewer");
        assert_eq!(report.structured["authenticated"], true);
        assert!(report.text.contains("viewer"));
    }

    #[test]
    fn whoami_reports_operator() {
        let report = whoami("operator", true);
        assert_eq!(report.structured["role"], "operator");
        assert_eq!(report.structured["authenticated"], true);
        assert!(report.text.contains("operator"));
    }

    #[test]
    fn whoami_open_mode_is_unauthenticated_operator() {
        // Loopback dev: no token, but full access — reported as an
        // unauthenticated operator so the UI still shows every control.
        let report = whoami("operator", false);
        assert_eq!(report.structured["role"], "operator");
        assert_eq!(report.structured["authenticated"], false);
    }
}
