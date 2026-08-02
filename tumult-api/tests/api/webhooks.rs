//! Webhook CRUD tests (`/api/webhooks*`): admin-only management, the HMAC
//! secret is returned exactly once at creation and never listed, and the
//! SSRF guard rejects insecure/local URLs.

use crate::common::*;
use serde_json::json;

#[tokio::test]
async fn webhook_crud_admin_only_secret_shown_once() {
    let srv = spawn_server().await;
    add_user(&srv, "admin", "admin-password-1", "admin", false).await;
    add_user(&srv, "op", "op-password-1", "operator", false).await;
    let (admin, _) = add_token(&srv, "u-admin", "admin-token").await;
    let (op, _) = add_token(&srv, "u-op", "op-token").await;

    // Unauthenticated → 401; operator → 403 on both read and write.
    let resp = reqwest::Client::new()
        .get(format!("{}/api/webhooks", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let (status, _body) = get_auth(&srv.base, "/api/webhooks", &op).await;
    assert_eq!(status, 403, "operator may not manage webhooks");
    let (status, _body) = post_auth(
        &srv.base,
        "/api/webhooks",
        &op,
        json!({"name": "x", "url": "https://hooks.example.com/x"}),
    )
    .await;
    assert_eq!(status, 403);

    // Create: 201 with the signing secret in the response, exactly once.
    let (status, body) = post_auth(
        &srv.base,
        "/api/webhooks",
        &admin,
        json!({"name": "ci sink", "url": "https://hooks.example.com/x", "events": ["stop_requested"]}),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let secret = body["secret"]
        .as_str()
        .expect("secret returned at creation");
    assert_eq!(secret.len(), 64, "64-hex secret");
    assert_eq!(body["events"], json!(["stop_requested"]));
    assert_eq!(body["enabled"], true);

    // List: the webhook is there WITHOUT the secret.
    let (status, body) = get_auth(&srv.base, "/api/webhooks", &admin).await;
    assert_eq!(status, 200);
    let hooks = body["webhooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0]["name"], "ci sink");
    assert!(
        hooks[0].get("secret").is_none(),
        "never list secrets: {body}"
    );

    // SSRF guard: insecure scheme and loopback IPs are rejected.
    for bad_url in [
        "http://hooks.example.com/x",
        "https://127.0.0.1:8080/x",
        "https://169.254.169.254/latest",
        "https://192.168.1.10/x",
        "not-a-url",
    ] {
        let (status, body) = post_auth(
            &srv.base,
            "/api/webhooks",
            &admin,
            json!({"name": "bad", "url": bad_url}),
        )
        .await;
        assert_eq!(status, 400, "{bad_url}: {body}");
    }

    // Disable, delete; unknown ids 404.
    let (status, _body) = post_auth(
        &srv.base,
        &format!("/api/webhooks/{id}/enable"),
        &admin,
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _body) = post_auth(
        &srv.base,
        "/api/webhooks/w-nope/enable",
        &admin,
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _body) = post_auth(
        &srv.base,
        &format!("/api/webhooks/{id}/delete"),
        &admin,
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = get_auth(&srv.base, "/api/webhooks", &admin).await;
    assert_eq!(status, 200);
    assert_eq!(body["webhooks"], json!([]));
}
