//! Auth storage (schema v6; v9 adds optional `tokens.expires_at_ns`): the
//! `users` identity table, `sessions` (opaque session ids stored only as
//! sha256 hashes), API `tokens` (kro_-prefixed, stored as sha256 hashes),
//! and per-user `user_env_scopes`.
//!
//! All four tables are index-free like the v5 run tables (see `schema.rs`):
//! uniqueness of username / session id hash / token hash is enforced in
//! code, not by the database.
//!
//! All mutations go through [`Writer`] (the daemon's single-writer channel
//! rides them, like every other write); reads go through [`Reader`] and back
//! the API auth middleware. `password_hash` leaves the store only through
//! the service layer — the API must never serialize it out.

use duckdb::params;

use crate::error::StoreError;
use crate::{Reader, Writer};

/// A user record (`users`).
#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub must_change: bool,
    pub disabled: bool,
    pub created_at_ns: i64,
}

/// A session record (`sessions`): the opaque cookie value is never stored,
/// only its sha256 hex.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id_hash: String,
    pub user_id: String,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
}

/// An API token record (`tokens`): the kro_-prefixed token itself is never
/// stored, only its sha256 hex. `expires_at_ns` is optional (schema v9):
/// `None` means the token never expires (the pre-v9 behaviour).
#[derive(Debug, Clone)]
pub struct TokenRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub token_hash: String,
    pub created_at_ns: i64,
    pub last_used_at_ns: Option<i64>,
    pub revoked: bool,
    pub expires_at_ns: Option<i64>,
}

impl Writer {
    /// Insert a user (callers enforce username uniqueness first).
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn create_user(&self, user: &UserRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO users VALUES (?,?,?,?,?,?,?)",
            params![
                user.id,
                user.username,
                user.password_hash,
                user.role,
                user.must_change,
                user.disabled,
                user.created_at_ns
            ],
        )?;
        Ok(())
    }

    /// Replace a user's password hash, clearing the one-time bootstrap
    /// `must_change` flag.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn set_user_password(&self, user_id: &str, password_hash: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE users SET password_hash = ?, must_change = false WHERE id = ?",
            params![password_hash, user_id],
        )?;
        Ok(())
    }

    /// Admin-driven password reset: set the hash and force a change at next
    /// login (`must_change = true`) — the inverse of
    /// [`Writer::set_user_password`], which clears the flag on self-service
    /// change.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn reset_user_password(
        &self,
        user_id: &str,
        password_hash: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE users SET password_hash = ?, must_change = true WHERE id = ?",
            params![password_hash, user_id],
        )?;
        Ok(())
    }

    /// Set a user's role (`viewer|operator|approver|admin`).
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn set_user_role(&self, user_id: &str, role: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE users SET role = ? WHERE id = ?",
            params![role, user_id],
        )?;
        Ok(())
    }

    /// Disable or re-enable a user (disabled users cannot authenticate).
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn set_user_disabled(&self, user_id: &str, disabled: bool) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE users SET disabled = ? WHERE id = ?",
            params![disabled, user_id],
        )?;
        Ok(())
    }

    /// Insert a session.
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn create_session(&self, session: &SessionRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO sessions VALUES (?,?,?,?)",
            params![
                session.id_hash,
                session.user_id,
                session.created_at_ns,
                session.expires_at_ns
            ],
        )?;
        Ok(())
    }

    /// Delete a session by id hash (logout).
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub fn delete_session(&self, id_hash: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM sessions WHERE id_hash = ?", params![id_hash])?;
        Ok(())
    }

    /// Delete every session owned by a user — any password change or reset
    /// invalidates all existing logins, the caller's own included.
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub fn delete_sessions_for_user(&self, user_id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM sessions WHERE user_id = ?", params![user_id])?;
        Ok(())
    }

    /// Insert an API token record.
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn create_token(&self, token: &TokenRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO tokens VALUES (?,?,?,?,?,?,?,?)",
            params![
                token.id,
                token.user_id,
                token.name,
                token.token_hash,
                token.created_at_ns,
                token.last_used_at_ns,
                token.revoked,
                token.expires_at_ns
            ],
        )?;
        Ok(())
    }

    /// Revoke a token by its record id (the revocation handle).
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn revoke_token(&self, id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("UPDATE tokens SET revoked = true WHERE id = ?", params![id])?;
        Ok(())
    }

    /// Revoke every token owned by a user — any password change or reset
    /// invalidates all existing API tokens for that user.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn revoke_tokens_for_user(&self, user_id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE tokens SET revoked = true WHERE user_id = ?",
            params![user_id],
        )?;
        Ok(())
    }

    /// Stamp a token's `last_used_at_ns` (looked up by token hash).
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn touch_token_last_used(&self, token_hash: &str, at_ns: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE tokens SET last_used_at_ns = ? WHERE token_hash = ?",
            params![at_ns, token_hash],
        )?;
        Ok(())
    }

    /// Replace a user's environment scopes atomically (delete + insert in
    /// one transaction). An empty set clears all rows, which means the user
    /// is allowed in every environment.
    ///
    /// # Errors
    /// Returns an error if the delete or any insert fails (the transaction
    /// is rolled back).
    pub fn set_user_env_scopes(
        &self,
        user_id: &str,
        environments: &[String],
    ) -> Result<(), StoreError> {
        crate::with_tx(&self.conn, || {
            self.conn.execute(
                "DELETE FROM user_env_scopes WHERE user_id = ?",
                params![user_id],
            )?;
            let mut stmt = self
                .conn
                .prepare("INSERT INTO user_env_scopes VALUES (?,?)")?;
            for env in environments {
                stmt.execute(params![user_id, env])?;
            }
            Ok(())
        })
    }
}

impl Reader {
    /// Whether any user exists at all (bootstrap check).
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn users_exist(&self) -> Result<bool, StoreError> {
        let rows = self.query_json_rows("SELECT count(*) AS c FROM users")?;
        Ok(rows.first().and_then(|r| r["c"].as_u64()).unwrap_or(0) > 0)
    }

    /// Whether any *real* (non-backfill) user exists — the check that decides
    /// "is auth configured". The `legacy` row seeded by the v6 migration is a
    /// backfill identity for pre-auth free-text actors, not a credential: a
    /// store holding only that row must behave as unauthenticated, or every
    /// upgraded pre-auth store would lock itself out on first open.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn real_users_exist(&self) -> Result<bool, StoreError> {
        let rows = self.query_json_rows("SELECT count(*) AS c FROM users WHERE id != 'legacy'")?;
        Ok(rows.first().and_then(|r| r["c"].as_u64()).unwrap_or(0) > 0)
    }

    /// Fetch a user by username, or `None`.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn user_by_username(&self, username: &str) -> Result<Option<UserRow>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT * FROM users WHERE username = '{}'",
            username.replace('\'', "''")
        ))?;
        Ok(rows.first().map(row_to_user))
    }

    /// Fetch a user by id, or `None`.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn user_by_id(&self, id: &str) -> Result<Option<UserRow>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT * FROM users WHERE id = '{}'",
            id.replace('\'', "''")
        ))?;
        Ok(rows.first().map(row_to_user))
    }

    /// List all users, ordered by username. Includes `password_hash` — the
    /// service layer decides what to expose; the API must never serialize
    /// it out.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn list_users(&self) -> Result<Vec<UserRow>, StoreError> {
        let rows = self.query_json_rows("SELECT * FROM users ORDER BY username")?;
        Ok(rows.iter().map(row_to_user).collect())
    }

    /// List all API tokens, newest first, including revoked ones (the admin
    /// list needs the revocation state). Includes `token_hash` — the service
    /// layer decides what to expose; the API must never serialize it out.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn list_tokens(&self) -> Result<Vec<TokenRow>, StoreError> {
        let rows =
            self.query_json_rows("SELECT * FROM tokens ORDER BY created_at_ns DESC, id DESC")?;
        Ok(rows.iter().map(row_to_token).collect())
    }

    /// Fetch an unexpired session (`expires_at_ns > now_ns`) joined with its
    /// user, or `None`. The caller checks `disabled`.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn session_with_user(
        &self,
        id_hash: &str,
        now_ns: i64,
    ) -> Result<Option<(SessionRow, UserRow)>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT s.id_hash, s.user_id, s.created_at_ns, s.expires_at_ns, \
                    u.id AS u_id, u.username AS u_username, \
                    u.password_hash AS u_password_hash, u.role AS u_role, \
                    u.must_change AS u_must_change, u.disabled AS u_disabled, \
                    u.created_at_ns AS u_created_at_ns \
             FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.id_hash = '{}' AND s.expires_at_ns > {now_ns}",
            id_hash.replace('\'', "''")
        ))?;
        Ok(rows
            .first()
            .map(|r| (row_to_session(r), row_to_user_joined(r))))
    }

    /// Fetch an unrevoked, unexpired token by its hash, joined with its
    /// user, or `None`. A token with `expires_at_ns <= now_ns` authenticates
    /// exactly like a revoked one; `NULL` expiry never expires. The caller
    /// checks `disabled`.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn token_with_user(
        &self,
        token_hash: &str,
        now_ns: i64,
    ) -> Result<Option<(TokenRow, UserRow)>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT t.id, t.user_id, t.name, t.token_hash, t.created_at_ns, \
                    t.last_used_at_ns, t.revoked, t.expires_at_ns, \
                    u.id AS u_id, u.username AS u_username, \
                    u.password_hash AS u_password_hash, u.role AS u_role, \
                    u.must_change AS u_must_change, u.disabled AS u_disabled, \
                    u.created_at_ns AS u_created_at_ns \
             FROM tokens t JOIN users u ON u.id = t.user_id \
             WHERE t.token_hash = '{}' AND t.revoked = false \
               AND (t.expires_at_ns IS NULL OR t.expires_at_ns > {now_ns})",
            token_hash.replace('\'', "''")
        ))?;
        Ok(rows
            .first()
            .map(|r| (row_to_token(r), row_to_user_joined(r))))
    }

    /// A user's allowed environments (one row each); empty means the user
    /// is allowed in every environment.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn user_env_scopes(&self, user_id: &str) -> Result<Vec<String>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT environment FROM user_env_scopes WHERE user_id = '{}' \
             ORDER BY environment",
            user_id.replace('\'', "''")
        ))?;
        Ok(rows
            .iter()
            .filter_map(|r| r["environment"].as_str().map(str::to_string))
            .collect())
    }
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

/// Map a `users` JSON row (bare column names) to the typed user.
fn row_to_user(v: &serde_json::Value) -> UserRow {
    UserRow {
        id: json_str(v, "id"),
        username: json_str(v, "username"),
        password_hash: json_str(v, "password_hash"),
        role: json_str(v, "role"),
        must_change: v["must_change"].as_bool().unwrap_or(false),
        disabled: v["disabled"].as_bool().unwrap_or(false),
        created_at_ns: v["created_at_ns"].as_i64().unwrap_or(0),
    }
}

/// Map the `u_*`-aliased user columns of an auth join to the typed user.
fn row_to_user_joined(v: &serde_json::Value) -> UserRow {
    UserRow {
        id: json_str(v, "u_id"),
        username: json_str(v, "u_username"),
        password_hash: json_str(v, "u_password_hash"),
        role: json_str(v, "u_role"),
        must_change: v["u_must_change"].as_bool().unwrap_or(false),
        disabled: v["u_disabled"].as_bool().unwrap_or(false),
        created_at_ns: v["u_created_at_ns"].as_i64().unwrap_or(0),
    }
}

/// Map a `sessions` JSON row to the typed session.
fn row_to_session(v: &serde_json::Value) -> SessionRow {
    SessionRow {
        id_hash: json_str(v, "id_hash"),
        user_id: json_str(v, "user_id"),
        created_at_ns: v["created_at_ns"].as_i64().unwrap_or(0),
        expires_at_ns: v["expires_at_ns"].as_i64().unwrap_or(0),
    }
}

/// Map a `tokens` JSON row to the typed token.
fn row_to_token(v: &serde_json::Value) -> TokenRow {
    TokenRow {
        id: json_str(v, "id"),
        user_id: json_str(v, "user_id"),
        name: json_str(v, "name"),
        token_hash: json_str(v, "token_hash"),
        created_at_ns: v["created_at_ns"].as_i64().unwrap_or(0),
        last_used_at_ns: v["last_used_at_ns"].as_i64(),
        revoked: v["revoked"].as_bool().unwrap_or(false),
        expires_at_ns: v["expires_at_ns"].as_i64(),
    }
}
