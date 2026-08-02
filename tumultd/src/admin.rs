use std::path::PathBuf;

use anyhow::{Context, Result};
use tumult_ingest::{Config, ManualImporter};
use tumult_lake::{Store, TokenRow, UserRow};

pub(crate) fn import(file: PathBuf, label: Option<String>) -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    let store = Store::open(&config.db_path)
        .context("open store (stop the daemon first if it is running, or set TUMULT_LAKE_PATH)")?;
    let writer = store.writer()?;
    let summary = ManualImporter::new(&writer)
        .import_file(&file, label)
        .with_context(|| format!("import {}", file.display()))?;
    println!(
        "imported {} rows as {} (batch {})",
        summary.rows, summary.format, summary.batch_id
    );
    Ok(())
}

/// Current time as epoch nanoseconds (row timestamps).
fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// Insert a user row with `password` hashed via argon2id. `must_change`
/// marks a one-time bootstrap password that has to be rotated on first login.
fn insert_user(
    writer: &tumult_lake::Writer,
    username: &str,
    password: &str,
    role: &str,
    must_change: bool,
) -> Result<UserRow> {
    let user = UserRow {
        id: uuid::Uuid::new_v4().to_string(),
        username: username.to_string(),
        password_hash: tumult_auth::hash_password(password)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?,
        role: role.to_string(),
        must_change,
        disabled: false,
        created_at_ns: now_ns(),
    };
    writer.create_user(&user)?;
    Ok(user)
}

/// `tumultd create-admin`: generate a one-time password and create the admin
/// user. The password is printed to stdout exactly once and never logged.
pub(crate) fn create_admin(username: &str, db: Option<PathBuf>) -> Result<()> {
    let config = Config::from_env().map_err(anyhow::Error::msg)?;
    let db_path = db.unwrap_or(config.db_path);
    let store = Store::open(&db_path)
        .context("open store (stop the daemon first if it is running, or set TUMULT_LAKE_PATH)")?;
    if store
        .read_only()
        .context("open store read-only")?
        .user_by_username(username)
        .context("check existing users")?
        .is_some()
    {
        anyhow::bail!("user {username:?} already exists");
    }
    let writer = store.writer()?;
    let password = tumult_auth::new_password();
    insert_user(&writer, username, &password, "admin", true)?;
    println!("created admin user: {username}");
    println!("one-time password: {password}");
    println!("this password must be changed on first login");
    Ok(())
}

/// Secure-by-default bind policy plus the demo bootstrap paths, run in
/// `serve` before any server binds (the store is open; the writer is the
/// same one the ingest channel then takes over). "Zero users" below means
/// zero *real* users — the v6 `legacy` backfill identity (disabled,
/// unverifiable) does not count, so an upgraded pre-auth store still
/// bootstraps instead of locking itself out:
///
/// * HTTP bind on a non-loopback host with zero users and no
///   `KRONIKA_BOOTSTRAP_ADMIN_PASSWORD` → refuse to start: the daemon will
///   not expose an unauthenticated API on a network interface.
/// * `KRONIKA_BOOTSTRAP_ADMIN_PASSWORD` set with zero users → create the
///   `admin` user with that exact password (`must_change = false`) — a loud
///   demo/dev path. Ignored (logged) when users already exist.
/// * `KRONIKA_BOOTSTRAP_TOKEN` set in that same zero-users bootstrap →
///   provision a `kro_`-prefixed API token (stored only as its sha256) for
///   the bootstrap admin. The value must start with `kro_`; anything else
///   refuses startup. When no bootstrap admin is created (no password set),
///   the token env var is ignored with a warning: there is no user to own it.
/// * Loopback bind with zero users → start unauthenticated (dev mode), with
///   a warning.
pub(crate) fn enforce_bind_guard(
    writer: &tumult_lake::Writer,
    store: &Store,
    config: &Config,
) -> Result<()> {
    let http_loopback = tumult_auth::host_is_loopback(&config.otlp_http_addr.ip().to_string());
    let bootstrap_password = std::env::var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD")
        .ok()
        .filter(|p| !p.is_empty());
    let bootstrap_token = std::env::var("KRONIKA_BOOTSTRAP_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let users_exist = store
        .read_only()
        .context("open store read-only")?
        .real_users_exist()
        .context("check for existing users")?;

    if users_exist {
        if bootstrap_password.is_some() {
            tracing::info!("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD ignored: users already exist");
        }
        if bootstrap_token.is_some() {
            tracing::info!("KRONIKA_BOOTSTRAP_TOKEN ignored: users already exist");
        }
        return Ok(());
    }

    if let Some(password) = bootstrap_password {
        // Validate the token before writing anything: a bad value must not
        // leave a half-provisioned bootstrap behind.
        if let Some(token) = bootstrap_token.as_deref() {
            if !token.starts_with("kro_") {
                anyhow::bail!(
                    "KRONIKA_BOOTSTRAP_TOKEN must start with \"kro_\"; refusing to start"
                );
            }
        }
        let admin = insert_user(writer, "admin", &password, "admin", false)?;
        tracing::warn!(
            "created bootstrap admin from KRONIKA_BOOTSTRAP_ADMIN_PASSWORD — this is a \
             demo/dev bootstrap path and must never be used in production"
        );
        if let Some(token) = bootstrap_token {
            writer.create_token(&TokenRow {
                id: uuid::Uuid::new_v4().to_string(),
                user_id: admin.id.clone(),
                name: "bootstrap".into(),
                token_hash: tumult_auth::sha256_hex(&token),
                created_at_ns: now_ns(),
                last_used_at_ns: None,
                revoked: false,
                expires_at_ns: None,
            })?;
            tracing::warn!(
                "provisioned bootstrap API token for the bootstrap admin (demo/dev path)"
            );
        }
        return Ok(());
    }

    if bootstrap_token.is_some() {
        tracing::warn!(
            "KRONIKA_BOOTSTRAP_TOKEN ignored: no bootstrap admin was created \
             (set KRONIKA_BOOTSTRAP_ADMIN_PASSWORD too)"
        );
    }
    if !http_loopback {
        anyhow::bail!(
            "refusing to serve the API on non-loopback address {} without authentication: \
             run `tumultd create-admin` (with the daemon stopped) or set \
             KRONIKA_BOOTSTRAP_ADMIN_PASSWORD for a demo bootstrap, or bind \
             KRONIKA_OTLP_HTTP_ADDR to 127.0.0.1 for local-only access",
            config.otlp_http_addr
        );
    }
    tracing::warn!(
        "no users exist: the API is running unauthenticated on loopback (dev mode); \
         run `tumultd create-admin` to enable authentication"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    /// One experiment span row in the manual-import CSV shape.
    const SPAN_CSV: &str =
        "ts_ns,span_name,service_name,experiment_name\n123,resilience.experiment,demo,exp-1\n";

    /// An initialised (empty) store inside a tempdir; no connection is held.
    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lake.duckdb");
        Store::open(&db_path).unwrap();
        (dir, db_path)
    }

    /// A store plus its single read-write connection, for the bind guard.
    fn temp_store() -> (tempfile::TempDir, Store, tumult_lake::Writer) {
        let (dir, db_path) = temp_db();
        let store = Store::open(&db_path).unwrap();
        let writer = store.writer().unwrap();
        (dir, store, writer)
    }

    /// Guard-test config: loopback gRPC, caller-chosen HTTP bind.
    fn config(http_addr: &str) -> Config {
        Config {
            db_path: PathBuf::from("/tmp/db.duckdb"),
            otlp_grpc_addr: "127.0.0.1:4317".parse().unwrap(),
            otlp_http_addr: http_addr.parse().unwrap(),
            metrics_dir: PathBuf::from("metrics"),
            ingest_token: None,
            tls_cert: None,
            tls_key: None,
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_bootstrap_env();
        guard
    }

    fn clear_bootstrap_env() {
        std::env::remove_var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD");
        std::env::remove_var("KRONIKA_BOOTSTRAP_TOKEN");
    }

    fn user_count(store: &Store) -> u64 {
        let rows = store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT count(*) AS c FROM users")
            .unwrap();
        rows[0]["c"].as_u64().unwrap()
    }

    // -- create_admin ----------------------------------------------------------

    #[test]
    fn create_admin_creates_the_user_and_refuses_duplicates() {
        let (_dir, db_path) = temp_db();
        create_admin("admin", Some(db_path.clone())).unwrap();

        let store = Store::at(&db_path);
        let user = store
            .read_only()
            .unwrap()
            .user_by_username("admin")
            .unwrap()
            .expect("admin row exists");
        assert_eq!(user.role, "admin");
        assert!(
            user.must_change,
            "the one-time password must be rotated on first login"
        );
        assert!(!user.disabled);
        assert!(!user.password_hash.is_empty());

        let err = create_admin("admin", Some(db_path)).unwrap_err();
        assert!(format!("{err:#}").contains("already exists"), "{err:#}");
    }

    #[test]
    fn create_admin_without_db_flag_uses_the_configured_store_path() {
        let _guard = env_lock();
        let (_dir, db_path) = temp_db();
        std::env::set_var("TUMULT_LAKE_PATH", &db_path);
        create_admin("boss", None).unwrap();
        std::env::remove_var("TUMULT_LAKE_PATH");

        let store = Store::at(&db_path);
        assert!(store
            .read_only()
            .unwrap()
            .user_by_username("boss")
            .unwrap()
            .is_some());
    }

    // -- import ----------------------------------------------------------------

    #[test]
    fn import_loads_a_csv_into_the_store() {
        let _guard = env_lock();
        let (dir, db_path) = temp_db();
        std::env::set_var("TUMULT_LAKE_PATH", &db_path);

        let csv = dir.path().join("spans.csv");
        std::fs::write(&csv, SPAN_CSV).unwrap();
        import(csv, Some("manual".to_string())).unwrap();
        std::env::remove_var("TUMULT_LAKE_PATH");

        let store = Store::at(&db_path);
        let reader = store.read_only().unwrap();
        let spans = reader
            .query_json_rows("SELECT count(*) AS c FROM spans")
            .unwrap();
        assert_eq!(spans[0]["c"].as_u64(), Some(1));
        let batches = reader
            .query_json_rows("SELECT count(*) AS c FROM import_batches")
            .unwrap();
        assert_eq!(batches[0]["c"].as_u64(), Some(1));
    }

    #[test]
    fn import_rejects_missing_and_unrecognised_files() {
        let _guard = env_lock();
        let (dir, db_path) = temp_db();
        std::env::set_var("TUMULT_LAKE_PATH", &db_path);

        let missing = import(dir.path().join("nope.csv"), None).unwrap_err();
        assert!(format!("{missing:#}").contains("import"), "{missing:#}");

        let garbage = dir.path().join("notes.txt");
        std::fs::write(&garbage, "plain text without commas\n").unwrap();
        assert!(import(garbage, None).is_err());

        std::env::remove_var("TUMULT_LAKE_PATH");
    }

    // -- enforce_bind_guard ----------------------------------------------------

    #[test]
    fn bind_guard_allows_unauthenticated_loopback_dev_mode() {
        let _guard = env_lock();
        let (_dir, store, writer) = temp_store();
        enforce_bind_guard(&writer, &store, &config("127.0.0.1:4318")).unwrap();
        assert!(!store.read_only().unwrap().real_users_exist().unwrap());
    }

    #[test]
    fn bind_guard_refuses_an_unauthenticated_network_bind() {
        let _guard = env_lock();
        let (_dir, store, writer) = temp_store();
        let err = enforce_bind_guard(&writer, &store, &config("0.0.0.0:4318")).unwrap_err();
        assert!(
            format!("{err:#}").contains("refusing to serve the API"),
            "{err:#}"
        );
    }

    #[test]
    fn bind_guard_bootstrap_password_creates_the_admin_on_a_network_bind() {
        let _guard = env_lock();
        let (_dir, store, writer) = temp_store();
        std::env::set_var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD", "s3cret");
        enforce_bind_guard(&writer, &store, &config("0.0.0.0:4318")).unwrap();
        clear_bootstrap_env();

        let user = store
            .read_only()
            .unwrap()
            .user_by_username("admin")
            .unwrap()
            .expect("bootstrap admin exists");
        assert_eq!(user.role, "admin");
        assert!(
            !user.must_change,
            "the bootstrap password is used as-is, not rotated"
        );
        assert!(tumult_auth::verify_password(&user.password_hash, "s3cret"));
    }

    #[test]
    fn bind_guard_rejects_a_token_without_the_kro_prefix_before_writing() {
        let _guard = env_lock();
        let (_dir, store, writer) = temp_store();
        std::env::set_var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD", "s3cret");
        std::env::set_var("KRONIKA_BOOTSTRAP_TOKEN", "not-a-kro-token");
        let err = enforce_bind_guard(&writer, &store, &config("0.0.0.0:4318")).unwrap_err();
        clear_bootstrap_env();
        assert!(format!("{err:#}").contains("kro_"), "{err:#}");
        assert_eq!(
            user_count(&store),
            0,
            "a rejected token must not leave a half-provisioned admin behind"
        );
    }

    #[test]
    fn bind_guard_provisions_the_bootstrap_token_for_the_bootstrap_admin() {
        let _guard = env_lock();
        let (_dir, store, writer) = temp_store();
        std::env::set_var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD", "s3cret");
        std::env::set_var("KRONIKA_BOOTSTRAP_TOKEN", "kro_dev_token");
        enforce_bind_guard(&writer, &store, &config("0.0.0.0:4318")).unwrap();
        clear_bootstrap_env();

        let admin = store
            .read_only()
            .unwrap()
            .user_by_username("admin")
            .unwrap()
            .expect("bootstrap admin exists");
        let tokens = store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT user_id, name, token_hash FROM tokens")
            .unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0]["user_id"].as_str().unwrap(), admin.id);
        assert_eq!(tokens[0]["name"].as_str().unwrap(), "bootstrap");
        assert_eq!(
            tokens[0]["token_hash"].as_str().unwrap(),
            tumult_auth::sha256_hex("kro_dev_token"),
            "only the sha256 of the token is stored"
        );
    }

    #[test]
    fn bind_guard_ignores_a_token_when_no_bootstrap_admin_is_created() {
        let _guard = env_lock();
        let (_dir, store, writer) = temp_store();
        std::env::set_var("KRONIKA_BOOTSTRAP_TOKEN", "kro_dev_token");

        // The network bind is still refused: a token alone authenticates no one.
        assert!(enforce_bind_guard(&writer, &store, &config("0.0.0.0:4318")).is_err());
        // Loopback dev mode starts, but no token is provisioned.
        enforce_bind_guard(&writer, &store, &config("127.0.0.1:4318")).unwrap();
        clear_bootstrap_env();

        let tokens = store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT count(*) AS c FROM tokens")
            .unwrap();
        assert_eq!(tokens[0]["c"].as_u64(), Some(0));
    }

    #[test]
    fn bind_guard_ignores_the_bootstrap_env_when_users_already_exist() {
        let _guard = env_lock();
        let (_dir, store, writer) = temp_store();
        insert_user(&writer, "admin", "existing-pw", "admin", true).unwrap();
        std::env::set_var("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD", "s3cret");
        std::env::set_var("KRONIKA_BOOTSTRAP_TOKEN", "kro_dev_token");
        enforce_bind_guard(&writer, &store, &config("0.0.0.0:4318")).unwrap();
        clear_bootstrap_env();

        assert_eq!(user_count(&store), 1, "no second admin may be created");
        let tokens = store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT count(*) AS c FROM tokens")
            .unwrap();
        assert_eq!(tokens[0]["c"].as_u64(), Some(0));
    }
}
