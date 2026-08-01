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
