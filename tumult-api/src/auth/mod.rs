//! Authentication, RBAC and per-user environment scoping.
//!
//! Auth is **enabled iff the store has any real users** (checked per
//! request, so bootstrapping the first user flips it on without a restart).
//! The `legacy` backfill identity seeded by the v6 migration does not count:
//! a store holding only that row is still open. While open, the middleware
//! injects a synthetic admin [`Principal`] and the whole API behaves exactly
//! as before auth existed.
//!
//! Credentials resolve in order: an `Authorization: Bearer kro_…` API token
//! (stored as its sha256, optionally expiring at `expires_at_ns`), then the
//! `kro_session` cookie (also stored as its sha256; sessions live 12 h).
//! Failure is always the same generic 401, and authorization then maps
//! `(method, path)` through [`ROUTE_TABLE`] — any route missing from the
//! table fails closed at [`Role::Admin`]. Failed logins are rate-limited
//! per `ip|username` (429 once the bucket is empty) and logged.
//!
//! All mutations ride the daemon's single-writer channel via
//! [`tumult_ingest::Batch::Exec`], like every other write endpoint; reads
//! run on a fresh read-only connection.

mod middleware;
mod rate_limit;
mod route_table;
mod session;
mod tokens;
mod users;

use tumult_auth::Role;

pub use middleware::auth_middleware;
pub use route_table::ROUTE_TABLE;
pub use session::{change_password, login, logout, me, ChangePasswordRequest, LoginRequest};
pub use tokens::{create_token, list_tokens, revoke_token, CreateTokenRequest};
pub use users::{
    create_user, list_users, reset_password, set_disabled, set_role, set_scopes, CreateUserRequest,
    ResetPasswordRequest, SetDisabledRequest, SetRoleRequest, SetScopesRequest,
};

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "kro_session";
/// Session lifetime: 12 hours, in nanoseconds (matches `Max-Age=43200`).
pub const SESSION_TTL_NS: i64 = 12 * 3600 * 1_000_000_000;

/// The authenticated identity behind a request, inserted into the request
/// extensions by [`auth_middleware`] and extracted by handlers via
/// `Extension<Principal>`.
#[derive(Clone, Debug)]
pub struct Principal {
    pub user_id: String,
    pub username: String,
    pub role: Role,
    /// Allowed environments; empty means every environment (also the case
    /// for the synthetic open-auth principal).
    pub env_scopes: Vec<String>,
    pub must_change: bool,
    /// `true` for the synthetic admin injected while auth is open (zero
    /// users): identity-bearing request fields (`entered_by` and friends)
    /// fall back to the request body, exactly as before auth existed. `false`
    /// for a real authenticated user, whose username then *overrides* those
    /// fields.
    pub synthetic: bool,
}

impl Principal {
    /// The open-auth stand-in: admin, all environments, username
    /// "anonymous".
    pub fn synthetic() -> Self {
        Self {
            user_id: String::new(),
            username: "anonymous".into(),
            role: Role::Admin,
            env_scopes: Vec::new(),
            must_change: false,
            synthetic: true,
        }
    }

    /// The username to record as actor, or `None` for the synthetic
    /// open-auth principal (system actor, as before auth existed).
    pub fn actor(&self) -> Option<String> {
        (!self.synthetic).then(|| self.username.clone())
    }

    /// Whether the principal may act on environment `env`: an empty scope
    /// list means every environment (same rule as the scoped reads).
    pub fn env_allowed(&self, env: &str) -> bool {
        self.env_scopes.is_empty() || self.env_scopes.iter().any(|s| s == env)
    }
}
