//! v6/v9 auth DDL. Same index-free rule as the v5 run tables: a daemon
//! killed mid-write can return with DuckDB's ART indexes desynced from the
//! table after WAL replay, and every subsequent UPDATE then dies fatally on
//! 'Failed to delete all rows from index'. These tables are tiny (a handful
//! of users, live sessions, and API tokens), sequential scans are free, and
//! uniqueness (username, session id hash, token hash) is enforced in code.

/// Auth tables (`users`, `sessions`, `tokens`, `user_env_scopes`) plus the
/// v9 optional token expiry (NULL = never expires; pre-v9 tokens keep
/// working). Additive and idempotent, like the v6/v7 ALTERs.
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS users (
    id              VARCHAR NOT NULL,   -- uuid; the bootstrap 'legacy' user uses id 'legacy'
    username        VARCHAR NOT NULL,   -- unique at code level
    password_hash   VARCHAR NOT NULL,   -- argon2id PHC string; '!' = can never verify (legacy)
    role            VARCHAR NOT NULL,   -- viewer|operator|approver|admin
    must_change     BOOLEAN NOT NULL,   -- one-time bootstrap password must be changed
    disabled        BOOLEAN NOT NULL,
    created_at_ns   BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    id_hash         VARCHAR NOT NULL,   -- sha256 hex of the opaque session id (cookie value never stored)
    user_id         VARCHAR NOT NULL,
    created_at_ns   BIGINT NOT NULL,
    expires_at_ns   BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS tokens (
    id              VARCHAR NOT NULL,   -- uuid of the token record (revocation handle)
    user_id         VARCHAR NOT NULL,
    name            VARCHAR NOT NULL,   -- human label, e.g. 'deploy script'
    token_hash      VARCHAR NOT NULL,   -- sha256 hex of the kro_-prefixed token
    created_at_ns   BIGINT NOT NULL,
    last_used_at_ns BIGINT,
    revoked         BOOLEAN NOT NULL
);
-- v9: optional token expiry (NULL = never expires; pre-v9 tokens keep
-- working). Additive and idempotent, like the v6/v7 ALTERs above.
ALTER TABLE tokens ADD COLUMN IF NOT EXISTS expires_at_ns BIGINT;
CREATE TABLE IF NOT EXISTS user_env_scopes (
    user_id         VARCHAR NOT NULL,
    environment     VARCHAR NOT NULL    -- one row per allowed env; zero rows = all environments
);
";
