//! Auth storage tests (moved out of `src/auth.rs`): user CRUD, session and
//! token lifecycles, env scopes, the v6 `legacy` backfill identity, and
//! run-audit actors.

#![cfg(feature = "duckdb")]

use tumult_lake::auth::{SessionRow, TokenRow, UserRow};
use tumult_lake::{Store, CURRENT_SCHEMA_VERSION};

fn fixture() -> (tempfile::TempDir, Store) {
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
            expires_at_ns: None,
        })
        .unwrap();

    let r = store.read_only().unwrap();
    let (t, u) = r.token_with_user("thash", 50).unwrap().unwrap();
    assert_eq!(t.id, "t-1");
    assert_eq!(t.name, "deploy script");
    assert!(t.last_used_at_ns.is_none());
    assert_eq!(u.username, "alice");
    assert!(r.token_with_user("nope", 50).unwrap().is_none());

    writer.touch_token_last_used("thash", 42).unwrap();
    let (t, _) = store
        .read_only()
        .unwrap()
        .token_with_user("thash", 50)
        .unwrap()
        .unwrap();
    assert_eq!(t.last_used_at_ns, Some(42));

    writer.revoke_token("t-1").unwrap();
    assert!(store
        .read_only()
        .unwrap()
        .token_with_user("thash", 50)
        .unwrap()
        .is_none());
}

#[test]
fn list_tokens_returns_all_newest_first_including_revoked() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.create_user(&user("u-1", "alice")).unwrap();
    let token = |id: &str, created_at_ns: i64| TokenRow {
        id: id.into(),
        user_id: "u-1".into(),
        name: format!("token {id}"),
        token_hash: format!("hash-{id}"),
        created_at_ns,
        last_used_at_ns: None,
        revoked: false,
        expires_at_ns: None,
    };
    writer.create_token(&token("t-old", 1)).unwrap();
    writer.create_token(&token("t-new", 3)).unwrap();
    writer.create_token(&token("t-mid", 2)).unwrap();
    writer.revoke_token("t-mid").unwrap();

    let tokens = store.read_only().unwrap().list_tokens().unwrap();
    let ids: Vec<&str> = tokens.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t-new", "t-mid", "t-old"], "newest first");
    assert!(tokens[1].revoked, "revoked rows are listed too");
    assert_eq!(tokens[0].name, "token t-new");
}

#[test]
fn token_expiry_is_enforced() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();
    writer.create_user(&user("u-1", "alice")).unwrap();
    let token = |id: &str, hash: &str, expires_at_ns: Option<i64>| TokenRow {
        id: id.into(),
        user_id: "u-1".into(),
        name: id.into(),
        token_hash: hash.into(),
        created_at_ns: 1,
        last_used_at_ns: None,
        revoked: false,
        expires_at_ns,
    };
    writer
        .create_token(&token("t-exp", "hexp", Some(10)))
        .unwrap();
    writer
        .create_token(&token("t-live", "hlive", Some(100)))
        .unwrap();
    writer
        .create_token(&token("t-open", "hopen", None))
        .unwrap();

    let r = store.read_only().unwrap();
    // Expired (expires_at_ns <= now) is excluded, exactly like revoked.
    assert!(r.token_with_user("hexp", 50).unwrap().is_none());
    // Unexpired and no-expiry tokens resolve; the expiry round-trips.
    let (t, _) = r.token_with_user("hlive", 50).unwrap().unwrap();
    assert_eq!(t.expires_at_ns, Some(100));
    let (t, _) = r.token_with_user("hopen", 50).unwrap().unwrap();
    assert_eq!(t.expires_at_ns, None);
    // Boundary: expires_at_ns == now is expired; the no-expiry token
    // resolves at any time.
    assert!(r.token_with_user("hlive", 100).unwrap().is_none());
    assert!(r.token_with_user("hopen", i64::MAX).unwrap().is_some());
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
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
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
