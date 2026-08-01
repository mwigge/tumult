use crate::common::*;
use serde_json::{json, Value};
use tumult_lake::{LogRow, MetricSumRow, SpanRow, Store};

// ---------------------------------------------------------------------------
// /api/approvals + /api/runs/{id}/approve|reject|break-glass (T10, ADR-013)

/// POST a JSON body with a `kro_` bearer token; returns (status, body).
async fn post_auth(base: &str, path: &str, token: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// GET with a `kro_` bearer token; returns (status, body).
async fn get_auth(base: &str, path: &str, token: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// The five roles involved in the approval workflow; returns
/// (admin, approver, approver2, viewer, operator) bearer tokens.
async fn approval_tokens(srv: &TestServer) -> (String, String, String, String, String) {
    for (name, role) in [
        ("root", "admin"),
        ("anna", "approver"),
        ("boris", "approver"),
        ("vicky", "viewer"),
        ("olga", "operator"),
    ] {
        add_user(srv, name, &format!("{name}-password-1"), role, false).await;
    }
    let (admin, _) = add_token(srv, "u-root", "t-admin").await;
    let (anna, _) = add_token(srv, "u-anna", "t-anna").await;
    let (boris, _) = add_token(srv, "u-boris", "t-boris").await;
    let (vicky, _) = add_token(srv, "u-vicky", "t-vicky").await;
    let (olga, _) = add_token(srv, "u-olga", "t-olga").await;
    (admin, anna, boris, vicky, olga)
}

/// Register RUN_TOON with a bearer token; returns its registry id.
async fn register_run_def_auth(base: &str, token: &str) -> String {
    let (status, body) =
        post_auth(base, "/api/runs/validate", token, json!({"toon": RUN_TOON})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["valid"], true, "{body}");
    body["registry_id"].as_str().unwrap().to_string()
}

/// Create a gated run as `token`; returns (run_id, tier).
async fn create_gated(base: &str, token: &str, registry_id: &str, env: &str) -> (String, String) {
    let (status, body) = post_auth(
        base,
        "/api/runs",
        token,
        json!({"registry_id": registry_id, "env": env}),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["state"], "pending_approval", "{body}");
    (
        body["run_id"].as_str().unwrap().to_string(),
        body["tier"].as_str().unwrap().to_string(),
    )
}

/// Poll a run's detail (authenticated) until it reaches a terminal state.
async fn await_terminal_run_auth(base: &str, token: &str, run_id: &str) -> Value {
    for _ in 0..200 {
        let (status, body) = get_auth(base, &format!("/api/runs/{run_id}"), token).await;
        assert_eq!(status, 200, "{body}");
        let state = body["run"]["state"].as_str().unwrap_or_default();
        if [
            "passed",
            "deviated",
            "failed",
            "aborted",
            "orphaned",
            "rollback_pending",
            "rejected",
            "expired",
        ]
        .contains(&state)
        {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("run {run_id} never reached a terminal state");
}

/// The full T1 flow: create gates, the queue lists it, the requester cannot
/// self-approve, a second approver dispatches, and the run detail carries
/// the whole approval chain.
#[tokio::test]
async fn approval_t1_flow_end_to_end() {
    let srv = spawn_server().await;
    let (admin, anna, _, vicky, _) = approval_tokens(&srv).await;
    let registry_id = register_run_def_auth(&srv.base, &admin).await;

    let (run_id, tier) = create_gated(&srv.base, &admin, &registry_id, "dev").await;
    assert_eq!(tier, "T1");

    // The queue lists the pending request (a viewer can read it).
    let (status, body) = get_auth(&srv.base, "/api/approvals", &vicky).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 1, "{body}");
    assert_eq!(body["queue"][0]["run_id"], json!(run_id));
    assert_eq!(body["queue"][0]["tier"], "T1");
    assert_eq!(body["queue"][0]["requested_by"], "root");

    // Run detail carries the approval chain: the request, no decisions yet.
    let (status, body) = get_auth(&srv.base, &format!("/api/runs/{run_id}"), &admin).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["approval"]["request"]["run_id"], json!(run_id));
    assert_eq!(body["approval"]["request"]["tier"], "T1");
    assert!(body["approval"]["request"]["consumed_at_ns"].is_null());
    assert_eq!(body["approval"]["decisions"], json!([]));

    // Segregation of duties: the requester (an admin, role-wise allowed to
    // approve) cannot approve their own run.
    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/runs/{run_id}/approve"),
        &admin,
        json!({}),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("self-approval"),
        "{body}"
    );

    // A second approver clears the quorum and dispatches.
    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/runs/{run_id}/approve"),
        &anna,
        json!({"note": "looks safe"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["state"], "queued", "{body}");

    // The run executes to a terminal state; the audit trail and the
    // approval chain tell the whole story.
    let detail = await_terminal_run_auth(&srv.base, &admin, &run_id).await;
    assert_eq!(detail["run"]["state"], "passed", "{detail}");
    let events: Vec<&str> = detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["event"].as_str())
        .collect();
    for event in ["requested", "approved", "dispatch_queued", "consumed"] {
        assert!(events.contains(&event), "{event} missing from {events:?}");
    }
    assert!(
        detail["approval"]["request"]["consumed_at_ns"].is_number(),
        "{detail}"
    );
    let decisions = detail["approval"]["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0]["approver"], "anna");
    assert_eq!(decisions[0]["decision"], "approved");
    assert_eq!(decisions[0]["note"], "looks safe");
}

/// Rejection by a second approver flips the run terminal `rejected`.
#[tokio::test]
async fn approval_reject_makes_run_terminal() {
    let srv = spawn_server().await;
    let (admin, anna, _, _, _) = approval_tokens(&srv).await;
    let registry_id = register_run_def_auth(&srv.base, &admin).await;
    let (run_id, _) = create_gated(&srv.base, &admin, &registry_id, "dev").await;

    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/runs/{run_id}/reject"),
        &anna,
        json!({"note": "too risky this week"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["state"], "rejected", "{body}");

    let detail = await_terminal_run_auth(&srv.base, &admin, &run_id).await;
    assert_eq!(detail["run"]["state"], "rejected", "{detail}");
    let events: Vec<&str> = detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["event"].as_str())
        .collect();
    assert!(events.contains(&"rejected"), "{events:?}");
    let decisions = detail["approval"]["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0]["decision"], "rejected");
    assert_eq!(decisions[0]["approver"], "anna");

    // A rejected run cannot be approved afterwards.
    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/runs/{run_id}/approve"),
        &anna,
        json!({}),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["state"], "rejected", "{body}");
}

/// RBAC on the approval endpoints: viewer and operator cannot decide,
/// break-glass is admin-only.
#[tokio::test]
async fn approval_endpoints_enforce_roles() {
    let srv = spawn_server().await;
    let (admin, anna, _, vicky, olga) = approval_tokens(&srv).await;
    let registry_id = register_run_def_auth(&srv.base, &admin).await;
    let (run_id, _) = create_gated(&srv.base, &admin, &registry_id, "dev").await;

    for (name, token) in [("viewer", &vicky), ("operator", &olga)] {
        let (status, _) = post_auth(
            &srv.base,
            &format!("/api/runs/{run_id}/approve"),
            token,
            json!({}),
        )
        .await;
        assert_eq!(status, 403, "{name} must not approve");
    }
    // Break-glass by a non-admin approver: 403 at the role gate.
    let (status, _) = post_auth(
        &srv.base,
        &format!("/api/runs/{run_id}/break-glass"),
        &anna,
        json!({"justification": "prod is down, need signal now"}),
    )
    .await;
    assert_eq!(status, 403, "approver must not break glass");
}

/// Break-glass: short justification 400s, a proper one dispatches and
/// leaves a retrospective manual-evidence draft as compliance debt.
#[tokio::test]
async fn break_glass_dispatches_and_opens_retrospective_debt() {
    let srv = spawn_server().await;
    let (admin, _, _, _, _) = approval_tokens(&srv).await;
    let registry_id = register_run_def_auth(&srv.base, &admin).await;
    let (run_id, _) = create_gated(&srv.base, &admin, &registry_id, "dev").await;

    let (status, _) = post_auth(
        &srv.base,
        &format!("/api/runs/{run_id}/break-glass"),
        &admin,
        json!({"justification": "short"}),
    )
    .await;
    assert_eq!(status, 400, "justification under 10 chars");

    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/runs/{run_id}/break-glass"),
        &admin,
        json!({"justification": "prod is down, need signal now"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["state"], "queued", "{body}");
    assert_eq!(body["break_glass"], true, "{body}");

    let detail = await_terminal_run_auth(&srv.base, &admin, &run_id).await;
    assert_eq!(detail["run"]["state"], "passed", "{detail}");
    let events: Vec<&str> = detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["event"].as_str())
        .collect();
    assert!(events.contains(&"overridden"), "{events:?}");
    assert_eq!(detail["approval"]["request"]["break_glass"], true);
    assert_eq!(detail["approval"]["request"]["break_glass_by"], "root");

    // The retrospective compliance-debt draft exists, entered by the admin.
    let reader = Store::at(&srv.db_path).read_only().unwrap();
    let drafts = reader.manual_experiments(Some("draft")).unwrap();
    let retro = drafts
        .iter()
        .find(|d| {
            d["experiment_name"] == json!(format!("Break-glass retrospective — run {run_id}"))
        })
        .unwrap_or_else(|| panic!("no retrospective draft in {drafts:?}"));
    assert_eq!(retro["entered_by"], "root");
    assert_eq!(retro["status"], "draft");
    assert_eq!(
        retro["action_items"],
        json!([format!(
            "Retrospective review of break-glass dispatch for run {run_id}"
        )])
    );
}

/// T3 without a configured autopilot policy fails closed: the approval is
/// refused 422 and the run stays pending.
#[tokio::test]
async fn t3_approval_fails_closed_without_policy() {
    let srv = spawn_server().await;
    let (admin, anna, boris, _, _) = approval_tokens(&srv).await;
    let registry_id = register_run_def_auth(&srv.base, &admin).await;

    let (run_id, tier) = create_gated(&srv.base, &admin, &registry_id, "prod").await;
    assert_eq!(tier, "T3");

    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/runs/{run_id}/approve"),
        &anna,
        json!({}),
    )
    .await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("autopilot gate unavailable"),
        "{body}"
    );

    // Still pending — a second approver hits the same wall, nothing leaked.
    let (status, body) = get_auth(&srv.base, &format!("/api/runs/{run_id}"), &boris).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["run"]["state"], "pending_approval", "{body}");
    assert_eq!(body["approval"]["decisions"], json!([]));
}

// ---------------------------------------------------------------------------
// Auth: middleware, RBAC, sessions, tokens, env scoping

/// Create a user directly through the writer channel; returns the user id.
/// (The API's own `POST /api/users` is exercised separately; fixtures go
/// straight to the store.)
async fn add_user(
    srv: &TestServer,
    username: &str,
    password: &str,
    role: &str,
    must_change: bool,
) -> String {
    let hash = tumult_auth::hash_password(password).unwrap();
    let row = tumult_lake::UserRow {
        id: format!("u-{username}"),
        username: username.into(),
        password_hash: hash,
        role: role.into(),
        must_change,
        disabled: false,
        created_at_ns: now_ns(),
    };
    let id = row.id.clone();
    exec_write(srv, move |w| w.create_user(&row).map_err(|e| e.to_string())).await;
    id
}

/// Mint a `kro_` token for a user directly in the store; returns
/// `(plaintext_token, token_hash)`.
async fn add_token(srv: &TestServer, user_id: &str, name: &str) -> (String, String) {
    add_token_with_expiry(srv, user_id, name, None).await
}

/// Mint a `kro_` token with an explicit expiry (`None` = never expires).
async fn add_token_with_expiry(
    srv: &TestServer,
    user_id: &str,
    name: &str,
    expires_at_ns: Option<i64>,
) -> (String, String) {
    let token = tumult_auth::new_token();
    let row = tumult_lake::TokenRow {
        id: format!("t-{name}"),
        user_id: user_id.into(),
        name: name.into(),
        token_hash: tumult_auth::sha256_hex(&token),
        created_at_ns: now_ns(),
        last_used_at_ns: None,
        revoked: false,
        expires_at_ns,
    };
    let hash = row.token_hash.clone();
    exec_write(srv, move |w| {
        w.create_token(&row).map_err(|e| e.to_string())
    })
    .await;
    (token, hash)
}

/// POST /api/auth/login; returns (status, body, session cookie value).
async fn login(base: &str, username: &str, password: &str) -> (u16, Value, Option<String>) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/auth/login"))
        .json(&json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("kro_session="))
        .map(|v| v.split(';').next().unwrap().to_string())
        .filter(|c| !c.is_empty());
    let body: Value = resp.json().await.unwrap();
    (status, body, cookie)
}

/// One root experiment span in a specific target environment.
fn env_root(id: &str, name: &str, env: &str, ts: i64) -> SpanRow {
    SpanRow {
        ts_ns: ts,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        parent_span_id: None,
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 5 * NS,
        status_code: "Unset".into(),
        status_message: String::new(),
        service_name: "tumult".into(),
        service_version: None,
        experiment_id: Some(id.into()),
        experiment_name: Some(name.into()),
        outcome_status: None,
        fault_type: None,
        fault_subtype: None,
        fault_severity: None,
        blast_radius: None,
        target_system: Some("database".into()),
        target_technology: None,
        target_environment: Some(env.into()),
        plugin_name: None,
        hypothesis_met: None,
        recovery_time_s: None,
        span_attrs: vec![],
        resource_attrs: vec![],
        events: "[]".into(),
    }
}

/// Every non-GET route in the table (plus the admin user list) rejects a
/// request without credentials — the table is fail-closed, so a mutating
/// route missing from the table still 401s (at Admin).
#[tokio::test]
async fn route_table_sweep_rejects_unauthenticated() {
    let srv = spawn_server().await;
    add_user(&srv, "admin", "admin-password-1", "admin", false).await;
    let client = reqwest::Client::new();
    let mut swept = 0;
    for (method, template, _role) in tumult_api::auth::ROUTE_TABLE {
        if *method == "GET" && *template != "/api/users" {
            continue;
        }
        if *template == "/api/auth/login" || *template == "/api/me" {
            continue;
        }
        // Concrete path: "x" for each {…} placeholder segment.
        let path = template
            .split('/')
            .map(|s| if s.starts_with('{') { "x" } else { s })
            .collect::<Vec<_>>()
            .join("/");
        let builder = client.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{path}", srv.base),
        );
        let builder = if *method == "GET" {
            builder
        } else {
            builder.json(&json!({}))
        };
        let resp = builder.send().await.unwrap();
        assert_eq!(
            resp.status().as_u16(),
            401,
            "{method} {path} must 401 without credentials"
        );
        swept += 1;
    }
    assert!(swept >= 25, "sweep covered {swept} routes");
}

/// The role ladder: each role can do its own level and is 403'd above it.
#[tokio::test]
async fn route_role_matrix() {
    let srv = spawn_server().await;
    for (name, role) in [
        ("vicky", "viewer"),
        ("olga", "operator"),
        ("anna", "approver"),
        ("root", "admin"),
    ] {
        add_user(&srv, name, &format!("{name}-password-1"), role, false).await;
    }
    let client = reqwest::Client::new();
    let base = srv.base.clone();
    let cookie_of = |name: &str| {
        let base = base.clone();
        let name = name.to_string();
        async move {
            let (status, _, cookie) = login(&base, &name, &format!("{name}-password-1")).await;
            assert_eq!(status, 200, "login {name}");
            cookie.unwrap()
        }
    };
    let with_cookie = |method: &str, path: &str, cookie: &str, body: Option<Value>| {
        let b = client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
                format!("{base}{path}"),
            )
            .header("Cookie", format!("kro_session={cookie}"));
        match body {
            Some(v) => b.json(&v),
            None => b,
        }
    };

    let viewer = cookie_of("vicky").await;
    let operator = cookie_of("olga").await;
    let approver = cookie_of("anna").await;
    let admin = cookie_of("root").await;

    // Viewer: reads pass, execution is 403.
    let resp = with_cookie("GET", "/api/experiments", &viewer, None)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let resp = with_cookie(
        "POST",
        "/api/runs",
        &viewer,
        Some(json!({"registry_id": "reg-x"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    // …and admin territory is 403 too.
    let resp = with_cookie("GET", "/api/users", &viewer, None)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // Operator: validate passes, approval and user admin are 403.
    let resp = with_cookie(
        "POST",
        "/api/runs/validate",
        &operator,
        Some(json!({"toon": "title: x"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "{}",
        resp.text().await.unwrap()
    );
    let resp = with_cookie(
        "POST",
        "/api/manual/experiments/nope/verify",
        &operator,
        Some(json!({"reviewer": "x"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    let resp = with_cookie(
        "POST",
        "/api/users",
        &operator,
        Some(json!({"username": "mallory", "role": "viewer"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // Approver: verify passes the role gate (404: no such record), user
    // admin is still 403.
    let resp = with_cookie(
        "POST",
        "/api/manual/experiments/nope/verify",
        &approver,
        Some(json!({"reviewer": "x"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let resp = with_cookie(
        "POST",
        "/api/users",
        &approver,
        Some(json!({"username": "mallory", "role": "viewer"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // Admin: user management works — create (one-time password, must_change
    // always), list without hashes, duplicate 409, self-disable 400, token
    // mint + revoke.
    let resp = with_cookie(
        "POST",
        "/api/users",
        &admin,
        Some(json!({"username": "carol", "role": "viewer", "env_scopes": ["staging"]})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["username"], "carol");
    assert_eq!(body["must_change"], true);
    assert!(body["one_time_password"].as_str().unwrap().len() >= 12);
    let carol_id = body["id"].as_str().unwrap().to_string();

    let resp = with_cookie("GET", "/api/users", &admin, None)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let carol = body["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "carol")
        .unwrap();
    assert_eq!(carol["role"], "viewer");
    assert_eq!(carol["env_scopes"], json!(["staging"]));
    assert!(
        carol.get("password_hash").is_none(),
        "never serialize hashes"
    );

    let resp = with_cookie(
        "POST",
        "/api/users",
        &admin,
        Some(json!({"username": "carol", "role": "viewer"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 409);

    let resp = with_cookie(
        "POST",
        "/api/users/u-root/disable",
        &admin,
        Some(json!({"disabled": true})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 400, "cannot disable yourself");

    let resp = with_cookie(
        "POST",
        &format!("/api/users/{carol_id}/role"),
        &admin,
        Some(json!({"role": "operator"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let resp = with_cookie(
        "POST",
        &format!("/api/users/{carol_id}/role"),
        &admin,
        Some(json!({"role": "superuser"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let resp = with_cookie(
        "POST",
        "/api/users/u-nope/role",
        &admin,
        Some(json!({"role": "viewer"})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    let resp = with_cookie("POST", "/api/tokens", &admin, Some(json!({"name": "ci"})))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let body: Value = resp.json().await.unwrap();
    assert!(body["token"].as_str().unwrap().starts_with("kro_"));
    let token_id = body["id"].as_str().unwrap().to_string();
    let resp = with_cookie(
        "POST",
        &format!("/api/tokens/{token_id}/revoke"),
        &admin,
        Some(json!({})),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

/// Login → cookie → authenticated call; failure modes share one generic 401.
#[tokio::test]
async fn login_cookie_flow_and_failures() {
    let srv = spawn_server().await;
    add_user(&srv, "alice", "alice-password-1", "operator", false).await;
    add_user(&srv, "bob", "bob-password-123", "viewer", false).await;
    let client = reqwest::Client::new();

    // Wrong password and unknown user: identical generic 401 (no enumeration).
    let (s1, b1, _) = login(&srv.base, "alice", "not-the-password").await;
    assert_eq!(s1, 401);
    let (s2, b2, _) = login(&srv.base, "no-such-user", "not-the-password").await;
    assert_eq!(s2, 401);
    assert_eq!(b1, b2, "{b1} vs {b2}");
    assert_eq!(b1["error"], "invalid credentials");

    // Disabled user: same generic 401.
    exec_write(&srv, move |w| {
        w.set_user_disabled("u-bob", true)
            .map_err(|e| e.to_string())
    })
    .await;
    let (s3, b3, _) = login(&srv.base, "bob", "bob-password-123").await;
    assert_eq!(s3, 401);
    assert_eq!(b3, b1);

    // Good login: identity payload + cookie attributes (no Secure — the
    // harness runs plain HTTP).
    let resp = client
        .post(format!("{}/api/auth/login", srv.base))
        .json(&json!({"username": "alice", "password": "alice-password-1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.starts_with("kro_session="), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Strict"), "{set_cookie}");
    assert!(set_cookie.contains("Max-Age=43200"), "{set_cookie}");
    assert!(!set_cookie.contains("Secure"), "{set_cookie}");
    let cookie = set_cookie
        .strip_prefix("kro_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["username"], "alice");
    assert_eq!(body["role"], "operator");
    assert_eq!(body["must_change"], false);

    // The session cookie authenticates a mutating call.
    let resp = client
        .post(format!("{}/api/runs/validate", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .json(&json!({"toon": "title: x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // Logout expires the cookie and deletes the session server-side.
    let resp = client
        .post(format!("{}/api/auth/logout", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let cleared = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    let resp = client
        .post(format!("{}/api/runs/validate", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .json(&json!({"toon": "title: x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401, "session is gone after logout");
}

/// `kro_` bearer tokens authenticate at the owner's role; revocation kills
/// them; use stamps `last_used_at_ns`.
#[tokio::test]
async fn bearer_token_flow() {
    let srv = spawn_server().await;
    add_user(&srv, "alice", "alice-password-1", "viewer", false).await;
    add_user(&srv, "olga", "olga-password-12", "operator", false).await;
    let (viewer_token, viewer_hash) = add_token(&srv, "u-alice", "ci-viewer").await;
    let (operator_token, _) = add_token(&srv, "u-olga", "ci-operator").await;
    let client = reqwest::Client::new();

    let bearer = |token: &str| format!("Bearer {token}");
    let resp = client
        .get(format!("{}/api/experiments", srv.base))
        .header("Authorization", bearer(&viewer_token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    // Viewer token may not execute.
    let resp = client
        .post(format!("{}/api/runs/validate", srv.base))
        .header("Authorization", bearer(&viewer_token))
        .json(&json!({"toon": "title: x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    // Operator token may.
    let resp = client
        .post(format!("{}/api/runs/validate", srv.base))
        .header("Authorization", bearer(&operator_token))
        .json(&json!({"toon": "title: x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // Usage was stamped (best-effort, on the writer channel).
    let (token_row, _) = Store::at(&srv.db_path)
        .read_only()
        .unwrap()
        .token_with_user(&viewer_hash, now_ns())
        .unwrap()
        .unwrap();
    assert!(token_row.last_used_at_ns.is_some());

    // Revoked → 401.
    exec_write(&srv, move |w| {
        w.revoke_token("t-ci-viewer").map_err(|e| e.to_string())
    })
    .await;
    let resp = client
        .get(format!("{}/api/experiments", srv.base))
        .header("Authorization", bearer(&viewer_token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

/// A `must_change` principal is 403'd everywhere except logout /
/// change-password / me — until it changes the password.
#[tokio::test]
async fn must_change_gates_until_password_change() {
    let srv = spawn_server().await;
    add_user(&srv, "carol", "carol-password-1", "viewer", true).await;
    let client = reqwest::Client::new();

    let (status, body, cookie) = login(&srv.base, "carol", "carol-password-1").await;
    assert_eq!(status, 200);
    assert_eq!(body["must_change"], true);
    let cookie = cookie.unwrap();

    let resp = client
        .get(format!("{}/api/experiments", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "password_change_required");

    // Too short → 400; wrong current → 401.
    let resp = client
        .post(format!("{}/api/auth/change-password", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .json(&json!({"current_password": "carol-password-1", "new_password": "short"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let resp = client
        .post(format!("{}/api/auth/change-password", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .json(&json!({"current_password": "nope-nope-nope", "new_password": "carol-new-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);

    let resp = client
        .post(format!("{}/api/auth/change-password", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .json(
            &json!({"current_password": "carol-password-1", "new_password": "carol-new-password"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["changed"], true);

    // The same session is ungated now, and the new password logs in.
    let resp = client
        .get(format!("{}/api/experiments", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let (status, body, _) = login(&srv.base, "carol", "carol-new-password").await;
    assert_eq!(status, 200);
    assert_eq!(body["must_change"], false);
}

/// Admin password reset: a user locked out (password unknown — the demo
/// seed's bob-drift case) gets a one-time password forced through the
/// must_change flow, then changes to a permanent one.
#[tokio::test]
async fn admin_reset_password_recovers_a_locked_out_user() {
    let srv = spawn_server().await;
    add_user(&srv, "root", "root-password-1", "admin", false).await;
    let (root_token, _) = add_token(&srv, "u-root", "root").await;
    add_user(&srv, "dave", "dave-lost-password", "approver", false).await;
    let client = reqwest::Client::new();

    // Too short → 400; unknown user → 404.
    let (status, _body) = post_auth(
        &srv.base,
        "/api/users/u-dave/password",
        &root_token,
        json!({"password": "short"}),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _body) = post_auth(
        &srv.base,
        "/api/users/u-nope/password",
        &root_token,
        json!({"password": "dave-one-time-pw"}),
    )
    .await;
    assert_eq!(status, 404);

    let (status, body) = post_auth(
        &srv.base,
        "/api/users/u-dave/password",
        &root_token,
        json!({"password": "dave-one-time-pw"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["must_change"], true);

    // The old password is dead; the one-time password logs in but is gated
    // until changed.
    let (status, _body, _) = login(&srv.base, "dave", "dave-lost-password").await;
    assert_eq!(status, 401);
    let (status, body, cookie) = login(&srv.base, "dave", "dave-one-time-pw").await;
    assert_eq!(status, 200);
    assert_eq!(body["must_change"], true);
    let cookie = cookie.unwrap();
    let resp = client
        .get(format!("{}/api/experiments", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // Change to the permanent password: the same session is ungated and the
    // new password logs in clean.
    let resp = client
        .post(format!("{}/api/auth/change-password", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .json(&json!({"current_password": "dave-one-time-pw", "new_password": "dave-permanent-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let resp = client
        .get(format!("{}/api/experiments", srv.base))
        .header("Cookie", format!("kro_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let (status, body, _) = login(&srv.base, "dave", "dave-permanent-pw").await;
    assert_eq!(status, 200);
    assert_eq!(body["must_change"], false);
}

/// Environment scopes filter the experiment reads: list, detail.
#[tokio::test]
async fn env_scoping_filters_experiment_reads() {
    let srv = spawn_server().await;
    add_user(&srv, "scoped", "scoped-password", "viewer", false).await;
    exec_write(&srv, move |w| {
        w.set_user_env_scopes("u-scoped", &["staging".to_string()])
            .map_err(|e| e.to_string())
    })
    .await;
    let now = now_ns();
    exec_write(&srv, move |w| {
        w.insert_spans(&[
            env_root("exp-staging", "stg-exp", "staging", now - 100 * NS),
            env_root("exp-prod", "prod-exp", "prod", now - 90 * NS),
        ])
        .map_err(|e| e.to_string())
    })
    .await;
    let client = reqwest::Client::new();

    let (_, _, cookie) = login(&srv.base, "scoped", "scoped-password").await;
    let cookie = cookie.unwrap();
    let scoped_get = |path: &str| {
        client
            .get(format!("{}{path}", srv.base))
            .header("Cookie", format!("kro_session={cookie}"))
            .send()
    };

    // The list shows only the staging experiment (the demo-env seed and the
    // prod experiment are outside scope).
    let resp = scoped_get("/api/experiments").await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let rows = body["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["id"], "exp-staging");

    // Detail of an out-of-scope experiment 404s; in-scope resolves.
    let resp = scoped_get("/api/experiments/exp-prod").await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let resp = scoped_get("/api/experiments/exp-staging").await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // Runs without an experiment stay visible to a scoped user (queued);
    // a run linked to an out-of-scope experiment is hidden.
    let registry = tumult_lake::RegisteredDefinition {
        id: "reg-scope".into(),
        name: "scope test".into(),
        definition_toon: "title: scope test".into(),
        content_hash: "scope-hash".into(),
        registered_at_ns: 1,
        registered_by: None,
    };
    exec_write(&srv, move |w| {
        w.register_definition(&registry)
            .map_err(|e| e.to_string())?;
        w.insert_run(&tumult_lake::NewRun {
            id: "run-queued".into(),
            registry_id: "reg-scope".into(),
            params_json: None,
            queued_at_ns: 10,
            actor: None,
        })
        .map_err(|e| e.to_string())?;
        w.insert_run(&tumult_lake::NewRun {
            id: "run-prod".into(),
            registry_id: "reg-scope".into(),
            params_json: None,
            queued_at_ns: 11,
            actor: None,
        })
        .map_err(|e| e.to_string())?;
        w.mark_run_started("run-prod", Some("exp-prod"))
            .map_err(|e| e.to_string())
    })
    .await;
    let resp = scoped_get("/api/runs").await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(ids, ["run-queued"], "{ids:?}");
    let resp = scoped_get("/api/runs/run-prod").await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let resp = scoped_get("/api/runs/run-queued").await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // An unscoped viewer sees everything: the two seed experiments plus the
    // two env experiments.
    add_user(&srv, "full", "full-password-1", "viewer", false).await;
    let (_, _, full_cookie) = login(&srv.base, "full", "full-password-1").await;
    let resp = client
        .get(format!("{}/api/experiments", srv.base))
        .header("Cookie", format!("kro_session={}", full_cookie.unwrap()))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["experiments"].as_array().unwrap().len(), 4);
}

/// `GET /api/me` reports the auth state in all three modes.
#[tokio::test]
async fn me_reports_auth_state() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();

    // Open (no users): not required, not authenticated.
    let (status, body) = get(&srv.base, "/api/me").await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({"auth_required": false, "authenticated": false})
    );

    // Users exist but no credential: required, not authenticated.
    add_user(&srv, "admin", "admin-password-1", "admin", false).await;
    let (status, body) = get(&srv.base, "/api/me").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"auth_required": true, "authenticated": false}));

    // Valid session: full identity.
    let (_, _, cookie) = login(&srv.base, "admin", "admin-password-1").await;
    let resp = client
        .get(format!("{}/api/me", srv.base))
        .header("Cookie", format!("kro_session={}", cookie.unwrap()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        json!({
            "auth_required": true,
            "authenticated": true,
            "username": "admin",
            "role": "admin",
            "must_change": false,
            "env_scopes": [],
        })
    );
}

/// A burst of failed logins for one `ip|username` key is throttled (429,
/// generic body); other keys keep their own bucket, so a legit login on a
/// different account still works. (The test server wires no `ConnectInfo`,
/// so every request shares the "unknown" ip bucket component and the key
/// differs only by username.)
#[tokio::test]
async fn login_rate_limit_throttles_failed_burst() {
    let srv = spawn_server().await;
    add_user(&srv, "rl-target", "rl-target-password", "admin", false).await;
    add_user(&srv, "rl-other", "rl-other-password", "viewer", false).await;

    // Burst of failures (limiter capacity is 5): all generic 401s.
    for attempt in 1..=5 {
        let (status, body, _) = login(&srv.base, "rl-target", "not-the-password").await;
        assert_eq!(status, 401, "attempt {attempt}");
        assert_eq!(body["error"], "invalid credentials");
    }
    // Bucket exhausted: throttled, even with the right password.
    let (status, body, _) = login(&srv.base, "rl-target", "not-the-password").await;
    assert_eq!(status, 429);
    assert_eq!(body["error"], "too many attempts; slow down");
    let (status, _, _) = login(&srv.base, "rl-target", "rl-target-password").await;
    assert_eq!(status, 429, "throttled until the bucket refills");

    // A different username has its own bucket: legit login still works.
    let (status, _, cookie) = login(&srv.base, "rl-other", "rl-other-password").await;
    assert_eq!(status, 200);
    assert!(cookie.is_some());
}

/// Expired `kro_` tokens authenticate exactly like revoked ones (401);
/// unexpired and no-expiry tokens keep working.
#[tokio::test]
async fn expired_tokens_are_rejected_unexpired_and_no_expiry_work() {
    let srv = spawn_server().await;
    let uid = add_user(&srv, "tok-admin", "tok-admin-password", "admin", false).await;
    let (expired, _) = add_token_with_expiry(&srv, &uid, "expired", Some(1)).await;
    let (live, _) = add_token_with_expiry(&srv, &uid, "live", Some(now_ns() + 3600 * NS)).await;
    let (forever, _) = add_token(&srv, &uid, "forever").await;

    let client = reqwest::Client::new();
    for (token, expected) in [(&expired, 401), (&live, 200), (&forever, 200)] {
        let resp = client
            .get(format!("{}/api/users", srv.base))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), expected, "token {token}");
    }
}

/// `POST /api/tokens` accepts an optional `expires_at_ns` (must be in the
/// future); the minted token then stops authenticating once expired, while
/// a token minted without expiry keeps working.
#[tokio::test]
async fn create_token_with_optional_expiry() {
    let srv = spawn_server().await;
    add_user(&srv, "minter", "minter-password", "admin", false).await;
    let (_, _, cookie) = login(&srv.base, "minter", "minter-password").await;
    let cookie = cookie.unwrap();
    let client = reqwest::Client::new();
    let mint = |body: Value| {
        client
            .post(format!("{}/api/tokens", srv.base))
            .header("Cookie", format!("kro_session={cookie}"))
            .json(&body)
    };

    // Past expiry → 400.
    let resp = mint(json!({"name": "past", "expires_at_ns": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);

    // Future expiry → 201, expiry echoed, token works now.
    let expires = now_ns() + 3600 * NS;
    let resp = mint(json!({"name": "expiring", "expires_at_ns": expires}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["expires_at_ns"], json!(expires));
    let token = body["token"].as_str().unwrap();
    let resp = client
        .get(format!("{}/api/users", srv.base))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // No expiry → null in the response, token works.
    let resp = mint(json!({"name": "forever"})).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["expires_at_ns"], Value::Null);
}

/// Environment scopes filter the telemetry reads — logs, traces, metrics,
/// timeseries, scores — and the game-day report path hides experiments
/// outside the principal's environments.
#[tokio::test]
async fn env_scoping_filters_telemetry_reads() {
    let srv = spawn_server().await;
    add_user(&srv, "tele", "tele-password-1", "viewer", false).await;
    exec_write(&srv, move |w| {
        w.set_user_env_scopes("u-tele", &["staging".to_string()])
            .map_err(|e| e.to_string())
    })
    .await;
    let now = now_ns();
    exec_write(&srv, move |w| {
        w.insert_spans(&[
            env_root("exp-stg", "stg-exp", "staging", now - 100 * NS),
            env_root("exp-prd", "prd-exp", "prod", now - 90 * NS),
        ])
        .map_err(|e| e.to_string())?;
        w.insert_logs(&[
            LogRow {
                ts_ns: now - 99 * NS,
                severity_text: "INFO".into(),
                body: "staging log body".into(),
                trace_id: Some("trace-exp-stg".into()),
                span_id: None,
                service_name: "tumult".into(),
                log_attrs: vec![("experiment_id".to_string(), "exp-stg".to_string())],
                resource_attrs: vec![],
            },
            LogRow {
                ts_ns: now - 89 * NS,
                severity_text: "INFO".into(),
                body: "prod log body with secret".into(),
                trace_id: Some("trace-exp-prd".into()),
                span_id: None,
                service_name: "tumult".into(),
                log_attrs: vec![("experiment_id".to_string(), "exp-prd".to_string())],
                resource_attrs: vec![],
            },
        ])
        .map_err(|e| e.to_string())?;
        w.insert_metric_sums(&[
            MetricSumRow {
                ts_ns: now - 98 * NS,
                metric_name: "demo.env.requests".into(),
                value: 1.0,
                experiment_name: Some("stg-exp".into()),
                ..MetricSumRow::default()
            },
            MetricSumRow {
                ts_ns: now - 88 * NS,
                metric_name: "demo.env.requests".into(),
                value: 100.0,
                experiment_name: Some("prd-exp".into()),
                ..MetricSumRow::default()
            },
        ])
        .map_err(|e| e.to_string())
    })
    .await;

    let (_, _, cookie) = login(&srv.base, "tele", "tele-password-1").await;
    let cookie = cookie.unwrap();
    let client = reqwest::Client::new();
    let scoped_get = |path: &str| {
        client
            .get(format!("{}{path}", srv.base))
            .header("Cookie", format!("kro_session={cookie}"))
            .send()
    };

    // Logs: only the staging log body is visible.
    let resp = scoped_get("/api/logs?range=24h").await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let bodies: Vec<&str> = body["logs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l["body"].as_str())
        .collect();
    assert_eq!(bodies, ["staging log body"], "{bodies:?}");

    // Log volume aggregates the same scoped rows (one log, one bucket).
    let resp = scoped_get("/api/logs/volume?range=24h&interval=1h")
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let total: f64 = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["count"].as_f64())
        .sum();
    assert_eq!(total, 1.0, "{body}");

    // Traces: only the staging trace is listed; the durations scatter too.
    let resp = scoped_get("/api/traces").await.unwrap();
    let body: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = body["traces"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["trace_id"].as_str())
        .collect();
    assert_eq!(ids, ["trace-exp-stg"], "{ids:?}");
    let resp = scoped_get("/api/traces/durations").await.unwrap();
    let body: Value = resp.json().await.unwrap();
    let points = body["points"].as_array().unwrap();
    assert_eq!(points.len(), 1, "{points:?}");
    assert_eq!(points[0]["trace_id"], "trace-exp-stg");

    // Trace detail: in-scope resolves, out-of-scope 404s (no existence leak).
    let resp = scoped_get("/api/traces/trace-exp-stg").await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let resp = scoped_get("/api/traces/trace-exp-prd").await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // Metrics query: only the staging experiment's points come back.
    let resp = scoped_get("/api/metrics/query?name=demo.env.requests&interval=1d")
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let total: f64 = body["series"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["points"].as_array().unwrap().iter())
        .filter_map(|p| p["v"].as_f64())
        .sum();
    assert_eq!(total, 1.0, "{body}");

    // Timeseries (spans-sourced definition): only the staging experiment
    // is counted.
    let resp = scoped_get("/api/timeseries?metric=experiment_count&interval=1h&range=24h")
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let total: f64 = body["points"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["value"].as_f64())
        .sum();
    assert_eq!(total, 1.0, "{body}");

    // Scores: the scorecard holds only the staging experiment.
    let resp = scoped_get("/api/scores").await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let names: Vec<&str> = body["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(names, ["stg-exp"], "{names:?}");

    // Scores tree: only the in-scope leaf rolls up.
    let resp = scoped_get("/api/scores/tree").await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["scored"], 1, "{body}");
    assert_eq!(body["expected"], 1, "{body}");

    // Reports: a scoped operator cannot render a game-day report for an
    // out-of-scope experiment (404 — no existence leak).
    add_user(&srv, "tele-op", "tele-op-password", "operator", false).await;
    exec_write(&srv, move |w| {
        w.set_user_env_scopes("u-tele-op", &["staging".to_string()])
            .map_err(|e| e.to_string())
    })
    .await;
    let (_, _, op_cookie) = login(&srv.base, "tele-op", "tele-op-password").await;
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .header("Cookie", format!("kro_session={}", op_cookie.unwrap()))
        .json(&json!({"type": "game-day", "experiment_id": "exp-prd"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // An unscoped viewer still sees everything: both env logs plus the seed
    // telemetry.
    add_user(&srv, "tele-full", "tele-full-password", "viewer", false).await;
    let (_, _, full_cookie) = login(&srv.base, "tele-full", "tele-full-password").await;
    let resp = client
        .get(format!("{}/api/logs?range=24h", srv.base))
        .header("Cookie", format!("kro_session={}", full_cookie.unwrap()))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["body"] == "prod log body with secret"),
        "{body}"
    );
}

/// Scoped report generation is confined to the principal's environments, and
/// pre-rendered artifacts carry generation-time coverage metadata that the
/// list/serve endpoints enforce: global and legacy (no-metadata) artifacts
/// fail closed for scoped principals (hidden, 404) while unscoped
/// principals see everything.
#[tokio::test]
async fn env_scoping_confines_report_generation_and_artifacts() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    let now = now_ns();
    // One experiment in each of staging and prod (the seed's two are env
    // "demo"); the completion log gives the staging run a green outcome.
    exec_write(&srv, move |w| {
        w.insert_spans(&[
            env_root("exp-stg", "stg-exp", "staging", now - 100 * NS),
            env_root("exp-prd", "prd-exp", "prod", now - 90 * NS),
        ])
        .map_err(|e| e.to_string())?;
        w.insert_logs(&[LogRow {
            ts_ns: now - 99 * NS,
            severity_text: "INFO".into(),
            body: "experiment.completed".into(),
            trace_id: Some("trace-exp-stg".into()),
            span_id: None,
            service_name: "tumult".into(),
            log_attrs: vec![
                ("experiment_id".to_string(), "exp-stg".to_string()),
                ("status".to_string(), "Completed".to_string()),
            ],
            resource_attrs: vec![],
        }])
        .map_err(|e| e.to_string())
    })
    .await;

    // A scoped operator (staging only) and an unscoped viewer.
    add_user(&srv, "rep-op", "rep-op-password", "operator", false).await;
    exec_write(&srv, move |w| {
        w.set_user_env_scopes("u-rep-op", &["staging".to_string()])
            .map_err(|e| e.to_string())
    })
    .await;
    let (_, _, op_cookie) = login(&srv.base, "rep-op", "rep-op-password").await;
    let op_cookie = op_cookie.unwrap();
    add_user(&srv, "rep-full", "rep-full-password", "viewer", false).await;
    let (_, _, full_cookie) = login(&srv.base, "rep-full", "rep-full-password").await;
    let full_cookie = full_cookie.unwrap();
    // An unscoped operator for the global generation (generate is
    // Operator-gated).
    add_user(&srv, "rep-admin", "rep-admin-password", "operator", false).await;
    let (_, _, admin_cookie) = login(&srv.base, "rep-admin", "rep-admin-password").await;
    let admin_cookie = admin_cookie.unwrap();
    let authed = |method: &str, path: &str, cookie: &str, body: Option<Value>| {
        let req = client
            .request(method.parse().unwrap(), format!("{}{path}", srv.base))
            .header("Cookie", format!("kro_session={cookie}"));
        match body {
            Some(b) => req.json(&b),
            None => req,
        }
        .send()
    };

    // --- v2 generation is confined -------------------------------------
    let resp = authed(
        "POST",
        "/api/reports/v2/generate",
        &op_cookie,
        Some(json!({"type": "executive-digest", "period": "7d"})),
    )
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let meta: Value = resp.json().await.unwrap();
    assert_eq!(meta["env_scopes"], json!(["staging"]), "{meta}");
    let scoped_id = meta["doc_id"].as_str().unwrap().to_string();
    let resp = authed(
        "GET",
        &format!("/api/reports/v2/{scoped_id}/html"),
        &op_cookie,
        None,
    )
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let html = resp.text().await.unwrap();
    assert!(html.contains("stg-exp"), "{html}");
    assert!(!html.contains("prd-exp"), "{html}");

    // An unscoped generation covers everything and records null coverage.
    let resp = authed(
        "POST",
        "/api/reports/v2/generate",
        &admin_cookie,
        Some(json!({"type": "executive-digest", "period": "7d"})),
    )
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // --- v2 list + artifact fetch honour coverage ----------------------
    // Find the global (null-coverage) report in the unscoped list.
    let resp = authed("GET", "/api/reports/v2", &full_cookie, None)
        .await
        .unwrap();
    let list: Value = resp.json().await.unwrap();
    let metas = list["reports"].as_array().unwrap();
    let global_id = metas
        .iter()
        .find(|m| m["env_scopes"].is_null())
        .and_then(|m| m["doc_id"].as_str())
        .expect("a global (null-coverage) report exists")
        .to_string();
    assert!(metas.iter().any(|m| m["doc_id"] == scoped_id));

    // The scoped principal's list hides the global report.
    let resp = authed("GET", "/api/reports/v2", &op_cookie, None)
        .await
        .unwrap();
    let list: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = list["reports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["doc_id"].as_str())
        .collect();
    assert_eq!(ids, [scoped_id.as_str()], "{ids:?}");

    // Scoped fetch of the global artifact 404s (pdf and html); unscoped 200s.
    for (cookie, want) in [(&op_cookie, 404), (&full_cookie, 200)] {
        let resp = authed(
            "GET",
            &format!("/api/reports/v2/{global_id}/pdf"),
            cookie,
            None,
        )
        .await
        .unwrap();
        assert_eq!(resp.status().as_u16(), want, "pdf as scoped/unscoped");
        let resp = authed(
            "GET",
            &format!("/api/reports/v2/{global_id}/html"),
            cookie,
            None,
        )
        .await
        .unwrap();
        assert_eq!(resp.status().as_u16(), want, "html as scoped/unscoped");
    }

    // A legacy v2 report (meta without env_scopes, as written before this
    // change) fails closed for the scoped principal.
    let v2_dir = srv.reports_dir.join("v2");
    std::fs::write(
        v2_dir.join("KRK-R1-20200101-1egacy.html"),
        "<html>legacy</html>",
    )
    .unwrap();
    std::fs::write(
        v2_dir.join("KRK-R1-20200101-1egacy.json"),
        r#"{"doc_id":"KRK-R1-20200101-1egacy","type":"executive-digest","created_ns":1}"#,
    )
    .unwrap();
    let resp = authed(
        "GET",
        "/api/reports/v2/KRK-R1-20200101-1egacy/html",
        &op_cookie,
        None,
    )
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 404, "legacy hidden from scoped");
    let resp = authed(
        "GET",
        "/api/reports/v2/KRK-R1-20200101-1egacy/html",
        &full_cookie,
        None,
    )
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 200, "legacy visible to unscoped");

    // --- v1 digest generation is confined --------------------------------
    let resp = authed(
        "POST",
        "/api/reports/generate",
        &op_cookie,
        Some(json!({"metric": "experiment_count"})),
    )
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let name = body["name"].as_str().unwrap().to_string();
    let resp = authed("GET", &format!("/api/reports/{name}"), &op_cookie, None)
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let html = resp.text().await.unwrap();
    // Only the staging experiment counts (global would be 4: 2 seed + 2 new).
    assert!(html.contains(r#"<div class="kpi-value">1</div>"#), "{html}");

    // The scoped principal's v1 list shows its own digest but not the
    // pre-seeded legacy digest (no sidecar); the unscoped list shows both.
    let resp = authed("GET", "/api/reports", &op_cookie, None)
        .await
        .unwrap();
    let list: Value = resp.json().await.unwrap();
    let names: Vec<&str> = list["reports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(names, [name.as_str()], "{names:?}");
    let resp = authed("GET", "/api/reports", &full_cookie, None)
        .await
        .unwrap();
    let list: Value = resp.json().await.unwrap();
    let names: Vec<&str> = list["reports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert!(names.contains(&name.as_str()), "{names:?}");
    assert!(names.contains(&"2026-01-01T00-00_digest.html"), "{names:?}");

    // The legacy digest 404s for the scoped principal, 200s unscoped.
    let resp = authed(
        "GET",
        "/api/reports/2026-01-01T00-00_digest.html",
        &op_cookie,
        None,
    )
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let resp = authed(
        "GET",
        "/api/reports/2026-01-01T00-00_digest.html",
        &full_cookie,
        None,
    )
    .await
    .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}
