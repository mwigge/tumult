//! Webhook storage tests: the v11 `webhooks` + `webhook_cursors` tables —
//! CRUD roundtrip, the enabled-only selection, and cursor upserts.

#![cfg(feature = "duckdb")]

use tumult_lake::{Store, WebhookRow, CURRENT_SCHEMA_VERSION};

fn fixture() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

fn webhook(id: &str, enabled: bool) -> WebhookRow {
    WebhookRow {
        id: id.into(),
        name: format!("hook {id}"),
        url: "https://hooks.example.com/endpoint".into(),
        secret: format!("secret-{id}"),
        events: vec![],
        enabled,
        created_by: Some("test".into()),
        created_at_ns: 1,
    }
}

#[test]
fn schema_is_v12() {
    let (_d, store) = fixture();
    assert_eq!(
        store.writer().unwrap().schema_version().unwrap(),
        CURRENT_SCHEMA_VERSION
    );
    assert_eq!(CURRENT_SCHEMA_VERSION, 12);
}

#[test]
fn webhook_crud_and_cursor() {
    let (_d, store) = fixture();
    let writer = store.writer().unwrap();

    let mut w = webhook("w-1", true);
    w.events = vec!["stop_requested".into(), "aborted".into()];
    writer.create_webhook(&w).unwrap();
    writer.create_webhook(&webhook("w-2", false)).unwrap();

    let all = store.read_only().unwrap().list_webhooks().unwrap();
    assert_eq!(all.len(), 2);
    let w1 = all.iter().find(|w| w.id == "w-1").unwrap();
    assert_eq!(w1.url, "https://hooks.example.com/endpoint");
    assert_eq!(w1.secret, "secret-w-1", "the store holds the secret");
    assert_eq!(w1.events, ["stop_requested", "aborted"]);
    assert!(w1.enabled);

    // The dispatcher's selection: enabled only.
    let enabled = store.read_only().unwrap().enabled_webhooks().unwrap();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].id, "w-1");

    // Cursor: unset until the first dispatch, then upserts forward.
    assert!(store
        .read_only()
        .unwrap()
        .webhook_cursor("w-1")
        .unwrap()
        .is_none());
    writer.set_webhook_cursor("w-1", 100).unwrap();
    writer.set_webhook_cursor("w-1", 200).unwrap();
    assert_eq!(
        store.read_only().unwrap().webhook_cursor("w-1").unwrap(),
        Some(200)
    );

    writer.set_webhook_enabled("w-2", true).unwrap();
    assert_eq!(
        store.read_only().unwrap().enabled_webhooks().unwrap().len(),
        2
    );
    writer.delete_webhook("w-1").unwrap();
    assert_eq!(store.read_only().unwrap().list_webhooks().unwrap().len(), 1);
}
