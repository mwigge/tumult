//! The statements of users, sessions, API tokens and environment scopes
//! (`auth.rs`).

pub(crate) const INSERT_USER: &str = "INSERT INTO users VALUES (?,?,?,?,?,?,?)";

pub(crate) const SET_PASSWORD: &str =
    "UPDATE users SET password_hash = ?, must_change = false WHERE id = ?";

pub(crate) const RESET_PASSWORD: &str =
    "UPDATE users SET password_hash = ?, must_change = true WHERE id = ?";

pub(crate) const SET_ROLE: &str = "UPDATE users SET role = ? WHERE id = ?";

pub(crate) const SET_DISABLED: &str = "UPDATE users SET disabled = ? WHERE id = ?";

pub(crate) const INSERT_SESSION: &str = "INSERT INTO sessions VALUES (?,?,?,?)";

pub(crate) const DELETE_SESSION: &str = "DELETE FROM sessions WHERE id_hash = ?";

pub(crate) const DELETE_USER_SESSIONS: &str = "DELETE FROM sessions WHERE user_id = ?";

pub(crate) const INSERT_TOKEN: &str = "INSERT INTO tokens VALUES (?,?,?,?,?,?,?,?)";

pub(crate) const REVOKE_TOKEN: &str = "UPDATE tokens SET revoked = true WHERE id = ?";

pub(crate) const REVOKE_USER_TOKENS: &str = "UPDATE tokens SET revoked = true WHERE user_id = ?";

pub(crate) const TOUCH_TOKEN: &str = "UPDATE tokens SET last_used_at_ns = ? WHERE token_hash = ?";

pub(crate) const DELETE_ENV_SCOPES: &str = "DELETE FROM user_env_scopes WHERE user_id = ?";

pub(crate) const INSERT_ENV_SCOPE: &str = "INSERT INTO user_env_scopes VALUES (?,?)";

pub(crate) const COUNT_USERS: &str = "SELECT count(*) AS c FROM users";

pub(crate) const COUNT_REAL_USERS: &str = "SELECT count(*) AS c FROM users WHERE id != 'legacy'";

pub(crate) const USERS: &str = "SELECT * FROM users ORDER BY username";

pub(crate) const TOKENS: &str = "SELECT * FROM tokens ORDER BY created_at_ns DESC, id DESC";

/// The user with the name `username`.
pub(crate) fn user_by_username(username: &str) -> String {
    format!(
        "SELECT * FROM users WHERE username = '{}'",
        username.replace('\'', "''")
    )
}

/// The user `id`.
pub(crate) fn user_by_id(id: &str) -> String {
    format!(
        "SELECT * FROM users WHERE id = '{}'",
        id.replace('\'', "''")
    )
}

/// The session with the hash `id_hash` that is open at `now_ns`, with its
/// user.
pub(crate) fn session_with_user(id_hash: &str, now_ns: i64) -> String {
    format!(
        "SELECT s.id_hash, s.user_id, s.created_at_ns, s.expires_at_ns, \
                u.id AS u_id, u.username AS u_username, \
                u.password_hash AS u_password_hash, u.role AS u_role, \
                u.must_change AS u_must_change, u.disabled AS u_disabled, \
                u.created_at_ns AS u_created_at_ns \
         FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.id_hash = '{}' AND s.expires_at_ns > {now_ns}",
        id_hash.replace('\'', "''")
    )
}

/// The token with the hash `token_hash` that is valid at `now_ns`, with
/// its user.
pub(crate) fn token_with_user(token_hash: &str, now_ns: i64) -> String {
    format!(
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
    )
}

/// The environments of the user `user_id`.
pub(crate) fn env_scopes(user_id: &str) -> String {
    format!(
        "SELECT environment FROM user_env_scopes WHERE user_id = '{}' \
         ORDER BY environment",
        user_id.replace('\'', "''")
    )
}

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        INSERT_USER,
        SET_PASSWORD,
        RESET_PASSWORD,
        SET_ROLE,
        SET_DISABLED,
        INSERT_SESSION,
        DELETE_SESSION,
        DELETE_USER_SESSIONS,
        INSERT_TOKEN,
        REVOKE_TOKEN,
        REVOKE_USER_TOKENS,
        TOUCH_TOKEN,
        DELETE_ENV_SCOPES,
        INSERT_ENV_SCOPE,
        COUNT_USERS,
        COUNT_REAL_USERS,
        USERS,
        TOKENS,
        user_by_username("ada"),
        user_by_id("user-1"),
        session_with_user("hash", 1_774_980_000_000_000_000),
        token_with_user("hash", 1_774_980_000_000_000_000),
        env_scopes("user-1")
    ]
}
