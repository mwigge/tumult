//! Route table: the single source of truth for authorization.

use tumult_auth::Role;

// ---------------------------------------------------------------------------
// Route table: (method, path template, minimum role). The single source of
// truth for authorization; `{...}` segments match exactly one path segment.
// Anything not listed fails closed at Admin.

pub const ROUTE_TABLE: &[(&str, &str, Role)] = &[
    // Reads: every GET is Viewer, except the user list (Admin).
    ("GET", "/api/overview", Role::Viewer),
    ("GET", "/api/timeseries", Role::Viewer),
    ("GET", "/api/experiments", Role::Viewer),
    ("GET", "/api/experiments/windows", Role::Viewer),
    ("GET", "/api/experiments/{id}", Role::Viewer),
    ("GET", "/api/dimensions", Role::Viewer),
    ("GET", "/api/metrics", Role::Viewer),
    ("GET", "/api/logs", Role::Viewer),
    ("GET", "/api/logs/volume", Role::Viewer),
    ("GET", "/api/traces", Role::Viewer),
    ("GET", "/api/traces/durations", Role::Viewer),
    ("GET", "/api/traces/{id}", Role::Viewer),
    ("GET", "/api/metrics/catalog", Role::Viewer),
    ("GET", "/api/metrics/query", Role::Viewer),
    ("GET", "/api/topology", Role::Viewer),
    ("GET", "/api/scores", Role::Viewer),
    ("GET", "/api/scores/tree", Role::Viewer),
    ("GET", "/api/manual/experiments", Role::Viewer),
    ("GET", "/api/manual/experiments/{id}", Role::Viewer),
    ("GET", "/api/authoring/catalog", Role::Viewer),
    ("GET", "/api/registry", Role::Viewer),
    ("GET", "/api/registry/{id}", Role::Viewer),
    ("GET", "/api/runs", Role::Viewer),
    ("GET", "/api/runs/{id}", Role::Viewer),
    ("GET", "/api/runs/{id}/audit/verify", Role::Viewer),
    ("GET", "/api/schedules", Role::Viewer),
    ("GET", "/api/events", Role::Viewer),
    ("GET", "/api/gamedays", Role::Viewer),
    ("GET", "/api/gamedays/{id}", Role::Viewer),
    ("GET", "/api/lake/status", Role::Viewer),
    ("GET", "/api/reports", Role::Viewer),
    ("GET", "/api/reports/v2", Role::Viewer),
    ("GET", "/api/reports/v2/{id}/pdf", Role::Viewer),
    ("GET", "/api/reports/v2/{id}/html", Role::Viewer),
    ("GET", "/api/reports/{name}", Role::Viewer),
    ("GET", "/api/me", Role::Viewer),
    ("GET", "/api/users", Role::Admin),
    // Viewer-level writes (no fault injection, no state change).
    ("POST", "/api/ask", Role::Viewer),
    ("POST", "/api/authoring/scaffold", Role::Viewer),
    ("POST", "/api/runs/dry-run", Role::Viewer),
    ("POST", "/api/auth/login", Role::Viewer),
    ("POST", "/api/auth/logout", Role::Viewer),
    ("POST", "/api/auth/change-password", Role::Viewer),
    // Operator: run execution, imports, manual-evidence entry, reports.
    ("POST", "/api/runs", Role::Operator),
    ("POST", "/api/runs/stop-all", Role::Operator),
    ("POST", "/api/runs/{id}/stop", Role::Operator),
    ("POST", "/api/runs/validate", Role::Operator),
    ("POST", "/api/import/journal", Role::Operator),
    ("POST", "/api/manual/experiments", Role::Operator),
    ("PUT", "/api/manual/experiments/{id}", Role::Operator),
    (
        "POST",
        "/api/manual/experiments/{id}/submit",
        Role::Operator,
    ),
    (
        "POST",
        "/api/manual/experiments/{id}/attachments",
        Role::Operator,
    ),
    ("POST", "/api/manual/import", Role::Operator),
    ("POST", "/api/reports/generate", Role::Operator),
    ("POST", "/api/reports/v2/generate", Role::Operator),
    ("POST", "/api/lake/export", Role::Operator),
    ("POST", "/api/schedules", Role::Operator),
    ("POST", "/api/schedules/{id}/enable", Role::Operator),
    ("POST", "/api/schedules/{id}/delete", Role::Operator),
    ("POST", "/api/gamedays/validate", Role::Operator),
    ("POST", "/api/gamedays/{id}/runs", Role::Operator),
    // Approver: manual-evidence review.
    (
        "POST",
        "/api/manual/experiments/{id}/verify",
        Role::Approver,
    ),
    (
        "POST",
        "/api/manual/experiments/{id}/reject",
        Role::Approver,
    ),
    // Approvals: the queue is a read; decisions need the Approver role;
    // break-glass is Admin-only (ADR-013).
    ("GET", "/api/approvals", Role::Viewer),
    ("POST", "/api/runs/{id}/approve", Role::Approver),
    ("POST", "/api/runs/{id}/reject", Role::Approver),
    ("POST", "/api/runs/{id}/break-glass", Role::Admin),
    // Admin: user and token management.
    ("POST", "/api/users", Role::Admin),
    ("POST", "/api/users/{id}/role", Role::Admin),
    ("POST", "/api/users/{id}/password", Role::Admin),
    ("POST", "/api/users/{id}/disable", Role::Admin),
    ("POST", "/api/users/{id}/scopes", Role::Admin),
    ("GET", "/api/tokens", Role::Admin),
    ("POST", "/api/tokens", Role::Admin),
    ("POST", "/api/tokens/{id}/revoke", Role::Admin),
    ("GET", "/api/webhooks", Role::Admin),
    ("POST", "/api/webhooks", Role::Admin),
    ("POST", "/api/webhooks/{id}/enable", Role::Admin),
    ("POST", "/api/webhooks/{id}/delete", Role::Admin),
];

/// Whether one path template matches a concrete path: a `{...}` segment
/// matches exactly one (non-empty) segment, literals match verbatim.
fn template_matches(template: &str, path: &str) -> bool {
    let t: Vec<&str> = template.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    t.len() == p.len()
        && t.iter().zip(&p).all(|(t, p)| {
            if t.starts_with('{') && t.ends_with('}') {
                !p.is_empty()
            } else {
                t == p
            }
        })
}

/// Number of literal (non-`{...}`) segments — the specificity rank.
fn literal_segments(template: &str) -> usize {
    template
        .split('/')
        .filter(|s| !(s.starts_with('{') && s.ends_with('}')))
        .count()
}

/// The minimum role for `(method, path)`: the most specific matching table
/// entry (most literal segments, so `/api/runs/dry-run` beats
/// `/api/runs/{id}`), or [`Role::Admin`] when nothing matches — fail closed.
pub(crate) fn required_role(method: &str, path: &str) -> Role {
    ROUTE_TABLE
        .iter()
        .filter(|(m, t, _)| *m == method && template_matches(t, path))
        .max_by_key(|(_, t, _)| literal_segments(t))
        .map_or(Role::Admin, |(_, _, r)| *r)
}
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_and_validate_beat_the_runs_id_template() {
        // Literal-heavy templates win over `/api/runs/{id}`.
        assert_eq!(required_role("POST", "/api/runs/stop-all"), Role::Operator);
        assert_eq!(required_role("GET", "/api/schedules"), Role::Viewer);
        assert_eq!(required_role("GET", "/api/events"), Role::Viewer);
        assert_eq!(required_role("GET", "/api/webhooks"), Role::Admin);
        assert_eq!(
            required_role("POST", "/api/gamedays/validate"),
            Role::Operator
        );
        assert_eq!(required_role("GET", "/api/gamedays/x"), Role::Viewer);
        assert_eq!(
            required_role("POST", "/api/gamedays/x/runs"),
            Role::Operator
        );
        assert_eq!(required_role("POST", "/api/webhooks/x/delete"), Role::Admin);
        assert_eq!(
            required_role("POST", "/api/schedules/x/enable"),
            Role::Operator
        );
        assert_eq!(required_role("POST", "/api/runs/dry-run"), Role::Viewer);
        assert_eq!(required_role("POST", "/api/runs/validate"), Role::Operator);
        assert_eq!(required_role("GET", "/api/runs/some-id"), Role::Viewer);
        assert_eq!(
            required_role("POST", "/api/runs/some-id/stop"),
            Role::Operator
        );
    }

    #[test]
    fn authoring_routes_are_viewer_level() {
        // Catalog reads and scaffolding never mutate the store: same level
        // as `POST /api/runs/dry-run` and the MCP authoring tools.
        assert_eq!(required_role("GET", "/api/authoring/catalog"), Role::Viewer);
        assert_eq!(
            required_role("POST", "/api/authoring/scaffold"),
            Role::Viewer
        );
    }

    #[test]
    fn table_roles_match_the_design() {
        assert_eq!(required_role("GET", "/api/experiments"), Role::Viewer);
        assert_eq!(required_role("GET", "/api/users"), Role::Admin);
        assert_eq!(required_role("POST", "/api/runs"), Role::Operator);
        assert_eq!(
            required_role("POST", "/api/manual/experiments/x/verify"),
            Role::Approver
        );
        assert_eq!(
            required_role("POST", "/api/manual/experiments/x/reject"),
            Role::Approver
        );
        assert_eq!(required_role("POST", "/api/users"), Role::Admin);
        assert_eq!(required_role("GET", "/api/tokens"), Role::Admin);
        assert_eq!(required_role("POST", "/api/tokens/x/revoke"), Role::Admin);
    }

    #[test]
    fn unmatched_routes_fail_closed_at_admin() {
        assert_eq!(required_role("DELETE", "/api/runs"), Role::Admin);
        assert_eq!(required_role("POST", "/api/runs/some-id"), Role::Admin);
        assert_eq!(required_role("GET", "/api/nope"), Role::Admin);
        assert_eq!(required_role("POST", "/api/nope/nope"), Role::Admin);
    }

    #[test]
    fn template_matcher_segment_semantics() {
        assert!(template_matches("/api/runs/{id}", "/api/runs/abc"));
        assert!(!template_matches("/api/runs/{id}", "/api/runs/abc/stop"));
        assert!(!template_matches("/api/runs/{id}", "/api/runs"));
        assert!(template_matches(
            "/api/manual/experiments/{id}/verify",
            "/api/manual/experiments/x/verify"
        ));
    }
}
