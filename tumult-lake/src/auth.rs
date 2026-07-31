//! Auth storage (schema v6): the `users` identity table, `sessions` (opaque
//! session ids stored only as sha256 hashes), API `tokens` (kro_-prefixed,
//! stored as sha256 hashes), and per-user `user_env_scopes`.
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
/// stored, only its sha256 hex.
#[derive(Debug, Clone)]
pub struct TokenRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub token_hash: String,
    pub created_at_ns: i64,
    pub last_used_at_ns: Option<i64>,
    pub revoked: bool,
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

    /// Insert an API token record.
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn create_token(&self, token: &TokenRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO tokens VALUES (?,?,?,?,?,?,?)",
            params![
                token.id,
                token.user_id,
                token.name,
                token.token_hash,
                token.created_at_ns,
                token.last_used_at_ns,
                token.revoked
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

    /// Fetch an unrevoked token by its hash, joined with its user, or
    /// `None`. The caller checks `disabled`.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn token_with_user(
        &self,
        token_hash: &str,
    ) -> Result<Option<(TokenRow, UserRow)>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT t.id, t.user_id, t.name, t.token_hash, t.created_at_ns, \
                    t.last_used_at_ns, t.revoked, \
                    u.id AS u_id, u.username AS u_username, \
                    u.password_hash AS u_password_hash, u.role AS u_role, \
                    u.must_change AS u_must_change, u.disabled AS u_disabled, \
                    u.created_at_ns AS u_created_at_ns \
             FROM tokens t JOIN users u ON u.id = t.user_id \
             WHERE t.token_hash = '{}' AND t.revoked = false",
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn fixture() -> (tempfile::TempDir, crate::Store) {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        (d, store)
    }

    fn user(id: &str, username: &str) -> UserRow {
        UserRow {
            id: id.into(),
            username: username.into(),
            password_hash: "phc".into(),
            role: "viewer".into(),
            must_change: true,
            disabled: false,
            created_at_ns: 1,
        }
    }

    #[test]
    fn user_crud_roundtrip() {
        let (_d, store) = fixture();
        let writer = store.writer().unwrap();
        // Fresh readers per check: read-only connections pin their snapshot
        // at open.
        assert!(!store.read_only().unwrap().users_exist().unwrap());

        writer.create_user(&user("u-1", "alice")).unwrap();
        let r = store.read_only().unwrap();
        assert!(r.users_exist().unwrap());
        let by_name = r.user_by_username("alice").unwrap().unwrap();
        assert_eq!(by_name.id, "u-1");
        assert!(by_name.must_change);
        let by_id = r.user_by_id("u-1").unwrap().unwrap();
        assert_eq!(by_id.username, "alice");
        assert!(r.user_by_username("nope").unwrap().is_none());
        assert!(r.user_by_id("nope").unwrap().is_none());

        writer.set_user_password("u-1", "newhash").unwrap();
        writer.set_user_role("u-1", "admin").unwrap();
        writer.set_user_disabled("u-1", true).unwrap();
        let u = store
            .read_only()
            .unwrap()
            .user_by_id("u-1")
            .unwrap()
            .unwrap();
        assert_eq!(u.password_hash, "newhash");
        assert!(!u.must_change, "set_user_password clears must_change");
        assert_eq!(u.role, "admin");
        assert!(u.disabled);

        writer.reset_user_password("u-1", "resethash").unwrap();
        let u = store
            .read_only()
            .unwrap()
            .user_by_id("u-1")
            .unwrap()
            .unwrap();
        assert_eq!(u.password_hash, "resethash");
        assert!(u.must_change, "reset_user_password forces must_change");

        writer.create_user(&user("u-2", "bob")).unwrap();
        let names: Vec<String> = store
            .read_only()
            .unwrap()
            .list_users()
            .unwrap()
            .into_iter()
            .map(|u| u.username)
            .collect();
        assert_eq!(names, ["alice", "bob"]);
    }

    #[test]
    fn session_lifecycle() {
        let (_d, store) = fixture();
        let writer = store.writer().unwrap();
        writer.create_user(&user("u-1", "alice")).unwrap();
        writer
            .create_session(&SessionRow {
                id_hash: "hash-1".into(),
                user_id: "u-1".into(),
                created_at_ns: 1,
                expires_at_ns: 100,
            })
            .unwrap();
        writer
            .create_session(&SessionRow {
                id_hash: "hash-expired".into(),
                user_id: "u-1".into(),
                created_at_ns: 1,
                expires_at_ns: 10,
            })
            .unwrap();

        let r = store.read_only().unwrap();
        let (s, u) = r.session_with_user("hash-1", 50).unwrap().unwrap();
        assert_eq!(s.user_id, "u-1");
        assert_eq!(s.expires_at_ns, 100);
        assert_eq!(u.username, "alice");
        // Expired (expires_at_ns <= now) and unknown sessions are excluded.
        assert!(r.session_with_user("hash-expired", 50).unwrap().is_none());
        assert!(r.session_with_user("hash-1", 100).unwrap().is_none());
        assert!(r.session_with_user("nope", 50).unwrap().is_none());

        writer.delete_session("hash-1").unwrap();
        assert!(store
            .read_only()
            .unwrap()
            .session_with_user("hash-1", 50)
            .unwrap()
            .is_none());
    }

    #[test]
    fn token_lifecycle() {
        let (_d, store) = fixture();
        let writer = store.writer().unwrap();
        writer.create_user(&user("u-1", "alice")).unwrap();
        writer
            .create_token(&TokenRow {
                id: "t-1".into(),
                user_id: "u-1".into(),
                name: "deploy script".into(),
                token_hash: "thash".into(),
                created_at_ns: 1,
                last_used_at_ns: None,
                revoked: false,
            })
            .unwrap();

        let r = store.read_only().unwrap();
        let (t, u) = r.token_with_user("thash").unwrap().unwrap();
        assert_eq!(t.id, "t-1");
        assert_eq!(t.name, "deploy script");
        assert!(t.last_used_at_ns.is_none());
        assert_eq!(u.username, "alice");
        assert!(r.token_with_user("nope").unwrap().is_none());

        writer.touch_token_last_used("thash", 42).unwrap();
        let (t, _) = store
            .read_only()
            .unwrap()
            .token_with_user("thash")
            .unwrap()
            .unwrap();
        assert_eq!(t.last_used_at_ns, Some(42));

        writer.revoke_token("t-1").unwrap();
        assert!(store
            .read_only()
            .unwrap()
            .token_with_user("thash")
            .unwrap()
            .is_none());
    }

    #[test]
    fn env_scopes_replace_and_clear() {
        let (_d, store) = fixture();
        let writer = store.writer().unwrap();
        writer.create_user(&user("u-1", "alice")).unwrap();

        writer
            .set_user_env_scopes("u-1", &["staging".to_string(), "prod".to_string()])
            .unwrap();
        assert_eq!(
            store.read_only().unwrap().user_env_scopes("u-1").unwrap(),
            ["prod", "staging"]
        );

        // Replace with a different set.
        writer
            .set_user_env_scopes("u-1", &["dev".to_string()])
            .unwrap();
        assert_eq!(
            store.read_only().unwrap().user_env_scopes("u-1").unwrap(),
            ["dev"]
        );

        // Empty clears: zero rows = all environments.
        writer.set_user_env_scopes("u-1", &[]).unwrap();
        assert!(store
            .read_only()
            .unwrap()
            .user_env_scopes("u-1")
            .unwrap()
            .is_empty());
    }

    /// A pre-v6 store gains the `legacy` backfill identity on migrate
    /// (disabled, role viewer, unverifiable password); a fresh store — no
    /// legacy rows to attribute — does not.
    #[test]
    fn v5_store_seeds_legacy_user_fresh_store_does_not() {
        let d = tempfile::TempDir::new().unwrap();
        let db = d.path().join("kronika.duckdb");
        // Build a v5-era store with a raw connection (Store::open would
        // migrate immediately). Minimal: the version marker alone, since
        // CREATE_TABLES is `IF NOT EXISTS` and fills in the rest.
        {
            let conn = duckdb::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_meta (key VARCHAR PRIMARY KEY, value BIGINT NOT NULL);
                 INSERT INTO schema_meta (key, value) VALUES ('version', 5);",
            )
            .unwrap();
        }

        let store = Store::open(&db).unwrap();
        let writer = store.writer().unwrap();
        assert_eq!(
            writer.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        let legacy = store
            .read_only()
            .unwrap()
            .user_by_id("legacy")
            .unwrap()
            .unwrap();
        assert_eq!(legacy.username, "legacy");
        assert_eq!(legacy.password_hash, "!");
        assert_eq!(legacy.role, "viewer");
        assert!(legacy.disabled);
        // The backfill identity alone does not count as configured auth.
        let r = store.read_only().unwrap();
        assert!(r.users_exist().unwrap());
        assert!(!r.real_users_exist().unwrap());

        let d2 = tempfile::TempDir::new().unwrap();
        let fresh = Store::open(&d2.path().join("kronika.duckdb")).unwrap();
        let r = fresh.read_only().unwrap();
        assert!(r.user_by_id("legacy").unwrap().is_none());
        assert!(!r.users_exist().unwrap());
    }

    #[test]
    fn run_audit_actor_roundtrips() {
        let (_d, store) = fixture();
        let writer = store.writer().unwrap();
        writer
            .insert_run_audit("run-1", "enqueued", None, Some("alice"))
            .unwrap();
        writer
            .insert_run_audit("run-1", "started", None, None)
            .unwrap();

        let trail = store.read_only().unwrap().run_audit_trail("run-1").unwrap();
        assert_eq!(trail.len(), 2);
        let by_event = |e: &str| trail.iter().find(|r| r["event"] == e).unwrap();
        assert_eq!(by_event("enqueued")["actor"], serde_json::json!("alice"));
        assert!(by_event("started")["actor"].is_null());
    }
}
