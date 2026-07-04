//! Hermetic mocked-HTTP tests for the cloud connectors.
//!
//! A local `axum` server records the exact request each connector sends
//! (method, path, query, headers, body) and returns a canned provider
//! response. No real cloud account is contacted. These tests assert:
//!
//! * correct HTTP method / path / query,
//! * presence of the `SigV4` `Authorization` header (AWS) or `Bearer` token
//!   (Azure / GCP), and the correct signing scope / service,
//! * correct request body, and that success responses parse into typed
//!   outcomes,
//! * error paths (403 → auth, 404 → not-found, 429 / throttling → throttled)
//!   map to typed [`CloudError`] values, never a panic.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::Router;

use tumult_cloud::aws::{Ec2Client, FisClient};
use tumult_cloud::azure::ChaosClient;
use tumult_cloud::creds::AwsCredentials;
use tumult_cloud::error::CloudError;
use tumult_cloud::gcp::ComputeClient;

/// A single recorded inbound request.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
struct AppState {
    requests: Arc<Mutex<Vec<Recorded>>>,
    response: Arc<Mutex<(u16, String)>>,
}

async fn handler(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let recorded = Recorded {
        method: method.to_string(),
        path: uri.path().to_string(),
        query: uri.query().unwrap_or_default().to_string(),
        headers: headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        body: String::from_utf8_lossy(&body).to_string(),
    };
    state.requests.lock().unwrap().push(recorded);
    let (status, body) = state.response.lock().unwrap().clone();
    (StatusCode::from_u16(status).unwrap_or(StatusCode::OK), body)
}

/// A running mock HTTP server bound to an ephemeral localhost port.
struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    response: Arc<Mutex<(u16, String)>>,
}

impl MockServer {
    async fn start() -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = Arc::new(Mutex::new((200_u16, "{}".to_string())));
        let state = AppState {
            requests: requests.clone(),
            response: response.clone(),
        };
        let app = Router::new().fallback(handler).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base_url: format!("http://{addr}"),
            requests,
            response,
        }
    }

    fn set_response(&self, status: u16, body: &str) {
        *self.response.lock().unwrap() = (status, body.to_string());
    }

    fn last(&self) -> Recorded {
        self.requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a request was recorded")
    }
}

fn fake_aws_creds() -> AwsCredentials {
    AwsCredentials {
        access_key_id: "AKIDEXAMPLE".to_string(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
        session_token: None,
    }
}

// ── AWS FIS ────────────────────────────────────────────────────

#[tokio::test]
async fn fis_start_signs_and_posts_experiment() {
    let mock = MockServer::start().await;
    mock.set_response(
        200,
        r#"{"experiment":{"id":"EXPabc123","state":{"status":"initiating"}}}"#,
    );
    let client = FisClient::with_endpoint(
        mock.base_url.clone(),
        "us-east-1".to_string(),
        fake_aws_creds(),
    );

    let out = client.start_experiment("EXT-template-1").await.unwrap();
    assert!(out.contains("EXPabc123"));
    assert!(out.contains("initiating"));

    let req = mock.last();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/experiments");
    let auth = req.header("authorization").unwrap();
    assert!(auth.starts_with("AWS4-HMAC-SHA256 "), "auth: {auth}");
    assert!(
        auth.contains("us-east-1/fis/aws4_request"),
        "auth scope: {auth}"
    );
    assert!(req.header("x-amz-date").is_some());
    assert!(req.body.contains("EXT-template-1"));
    assert!(req.body.contains("clientToken"));
}

#[tokio::test]
async fn fis_stop_uses_delete_on_experiment_id() {
    let mock = MockServer::start().await;
    mock.set_response(
        200,
        r#"{"experiment":{"id":"EXP9","state":{"status":"stopping"}}}"#,
    );
    let client = FisClient::with_endpoint(
        mock.base_url.clone(),
        "eu-west-1".to_string(),
        fake_aws_creds(),
    );

    let out = client.stop_experiment("EXP9").await.unwrap();
    assert!(out.contains("stopping"));

    let req = mock.last();
    assert_eq!(req.method, "DELETE");
    assert_eq!(req.path, "/experiments/EXP9");
    assert!(req
        .header("authorization")
        .unwrap()
        .contains("eu-west-1/fis/aws4_request"));
}

#[tokio::test]
async fn fis_status_uses_get() {
    let mock = MockServer::start().await;
    mock.set_response(
        200,
        r#"{"experiment":{"id":"EXP9","state":{"status":"completed","reason":"all done"}}}"#,
    );
    let client = FisClient::with_endpoint(
        mock.base_url.clone(),
        "us-east-1".to_string(),
        fake_aws_creds(),
    );

    let out = client.experiment_status("EXP9").await.unwrap();
    assert!(out.contains("completed"));
    assert!(out.contains("all done"));
    assert_eq!(mock.last().method, "GET");
}

#[tokio::test]
async fn fis_403_maps_to_auth_error() {
    let mock = MockServer::start().await;
    mock.set_response(403, "{\"message\":\"not authorized\"}");
    let client = FisClient::with_endpoint(
        mock.base_url.clone(),
        "us-east-1".to_string(),
        fake_aws_creds(),
    );
    let err = client.start_experiment("EXT").await.unwrap_err();
    assert!(
        matches!(err, CloudError::Auth { status: 403, .. }),
        "got: {err:?}"
    );
}

#[tokio::test]
async fn fis_404_maps_to_not_found() {
    let mock = MockServer::start().await;
    mock.set_response(404, "{\"message\":\"no such experiment\"}");
    let client = FisClient::with_endpoint(
        mock.base_url.clone(),
        "us-east-1".to_string(),
        fake_aws_creds(),
    );
    let err = client.experiment_status("EXPmissing").await.unwrap_err();
    assert!(matches!(err, CloudError::NotFound { .. }), "got: {err:?}");
}

#[tokio::test]
async fn fis_throttling_maps_to_throttled() {
    let mock = MockServer::start().await;
    mock.set_response(400, "{\"__type\":\"ThrottlingException\"}");
    let client = FisClient::with_endpoint(
        mock.base_url.clone(),
        "us-east-1".to_string(),
        fake_aws_creds(),
    );
    let err = client.start_experiment("EXT").await.unwrap_err();
    assert!(matches!(err, CloudError::Throttled { .. }), "got: {err:?}");
}

// ── AWS EC2 ────────────────────────────────────────────────────

#[tokio::test]
async fn ec2_stop_signs_query_form_body() {
    let mock = MockServer::start().await;
    mock.set_response(
        200,
        "<StopInstancesResponse><instancesSet><item><instanceId>i-0abc</instanceId>\
         <currentState><code>64</code><name>stopping</name></currentState></item>\
         </instancesSet></StopInstancesResponse>",
    );
    let client = Ec2Client::with_endpoint(
        mock.base_url.clone(),
        "us-east-1".to_string(),
        fake_aws_creds(),
    );

    let out = client.stop_instance("i-0abc").await.unwrap();
    assert!(out.contains("stopping"), "out: {out}");

    let req = mock.last();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/");
    assert!(req.body.contains("Action=StopInstances"));
    assert!(req.body.contains("InstanceId.1=i-0abc"));
    assert!(req
        .header("authorization")
        .unwrap()
        .contains("us-east-1/ec2/aws4_request"));
}

#[tokio::test]
async fn ec2_terminate_uses_terminate_action() {
    let mock = MockServer::start().await;
    mock.set_response(200, "<TerminateInstancesResponse/>");
    let client = Ec2Client::with_endpoint(
        mock.base_url.clone(),
        "us-east-1".to_string(),
        fake_aws_creds(),
    );
    let _ = client.terminate_instance("i-0abc").await.unwrap();
    assert!(mock.last().body.contains("Action=TerminateInstances"));
}

// ── Azure Chaos Studio ─────────────────────────────────────────

#[tokio::test]
async fn azure_start_posts_bearer_authenticated() {
    let mock = MockServer::start().await;
    mock.set_response(202, "");
    let client = ChaosClient::with_endpoint(mock.base_url.clone(), "faketoken".to_string());

    let out = client.start("sub1", "rg1", "exp1").await.unwrap();
    assert!(out.contains("exp1"));

    let req = mock.last();
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.path,
        "/subscriptions/sub1/resourceGroups/rg1/providers/Microsoft.Chaos/experiments/exp1/start"
    );
    assert!(req.query.contains("api-version="));
    assert_eq!(req.header("authorization").unwrap(), "Bearer faketoken");
}

#[tokio::test]
async fn azure_cancel_hits_cancel_path() {
    let mock = MockServer::start().await;
    mock.set_response(202, "");
    let client = ChaosClient::with_endpoint(mock.base_url.clone(), "tok".to_string());
    client.cancel("s", "r", "e").await.unwrap();
    assert!(mock.last().path.ends_with("/cancel"));
}

#[tokio::test]
async fn azure_status_parses_provisioning_state() {
    let mock = MockServer::start().await;
    mock.set_response(200, r#"{"properties":{"provisioningState":"Succeeded"}}"#);
    let client = ChaosClient::with_endpoint(mock.base_url.clone(), "tok".to_string());
    let out = client.status("s", "r", "e").await.unwrap();
    assert!(out.contains("Succeeded"));
    assert_eq!(mock.last().method, "GET");
}

#[tokio::test]
async fn azure_401_maps_to_auth_error() {
    let mock = MockServer::start().await;
    mock.set_response(401, "{\"error\":\"expired token\"}");
    let client = ChaosClient::with_endpoint(mock.base_url.clone(), "tok".to_string());
    let err = client.start("s", "r", "e").await.unwrap_err();
    assert!(
        matches!(err, CloudError::Auth { status: 401, .. }),
        "got: {err:?}"
    );
}

// ── GCP Compute ────────────────────────────────────────────────

#[tokio::test]
async fn gcp_stop_posts_bearer_authenticated() {
    let mock = MockServer::start().await;
    mock.set_response(200, r#"{"name":"operation-42","status":"RUNNING"}"#);
    let client = ComputeClient::with_endpoint(mock.base_url.clone(), "gtoken".to_string());

    let out = client
        .stop_instance("proj", "us-central1-a", "vm1")
        .await
        .unwrap();
    assert!(out.contains("operation-42"));

    let req = mock.last();
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.path,
        "/compute/v1/projects/proj/zones/us-central1-a/instances/vm1/stop"
    );
    assert_eq!(req.header("authorization").unwrap(), "Bearer gtoken");
}

#[tokio::test]
async fn gcp_404_maps_to_not_found() {
    let mock = MockServer::start().await;
    mock.set_response(404, "{\"error\":\"instance not found\"}");
    let client = ComputeClient::with_endpoint(mock.base_url.clone(), "gtoken".to_string());
    let err = client
        .stop_instance("proj", "z", "missing")
        .await
        .unwrap_err();
    assert!(matches!(err, CloudError::NotFound { .. }), "got: {err:?}");
}
