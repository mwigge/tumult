//! Behavior tests for `KubernetesExecutor::execute` dispatch.
//!
//! `execute` builds a `kube` client from ambient credentials before
//! dispatching, so these tests supply a `KUBECONFIG` pointing either at an
//! unreachable address (argument validation fails before any network I/O) or
//! at a hand-rolled HTTP/1.1 mock apiserver on a loopback socket. The mock
//! records the exact method/path/body the executor sent and replies with
//! scripted JSON, one scripted response per request.
//!
//! `KUBECONFIG` is process-global, so every test that touches it serializes
//! on `ENV_LOCK` (async-aware, since it is held across `.await` points) and
//! restores the variable on drop.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use tumult_kubernetes::KubernetesExecutor;
use tumult_plugin::native::{NativeArgs, NativeError, NativeExecutor};

// ── Environment plumbing ──────────────────────────────────────

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().await
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// This test binary links both `ring` and `aws-lc-rs` through the workspace
/// dependency tree, so rustls cannot pick a process-level default provider on
/// its own and panics when `kube` builds its TLS config. Install one
/// explicitly, mirroring `tumultd::serve`.
fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Write a syntactically valid kubeconfig pointing at `server` and return
/// its unique temp path.
fn write_kubeconfig(server: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tumult-native-dispatch-{}-{}.yaml",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let yaml = format!(
        "apiVersion: v1\n\
         kind: Config\n\
         clusters:\n\
         - cluster:\n\
         \x20   server: {server}\n\
         \x20 name: fake\n\
         contexts:\n\
         - context:\n\
         \x20   cluster: fake\n\
         \x20   user: fake\n\
         \x20 name: fake\n\
         current-context: fake\n\
         users:\n\
         - name: fake\n\
         \x20 user: {{}}\n"
    );
    std::fs::write(&path, yaml).expect("write kubeconfig");
    path
}

/// Sets `KUBECONFIG` for the lifetime of the guard, restoring the previous
/// value (or absence) and deleting the temp file on drop.
struct KubeconfigEnv {
    original: Option<std::ffi::OsString>,
    file: Option<PathBuf>,
}

impl KubeconfigEnv {
    fn point_at(path: &Path) -> Self {
        let original = std::env::var_os("KUBECONFIG");
        std::env::set_var("KUBECONFIG", path);
        Self {
            original,
            file: Some(path.to_path_buf()),
        }
    }
}

impl Drop for KubeconfigEnv {
    fn drop(&mut self) {
        match &self.original {
            Some(value) => std::env::set_var("KUBECONFIG", value),
            None => std::env::remove_var("KUBECONFIG"),
        }
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn native_args(pairs: &[(&str, Value)]) -> NativeArgs {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

// ── Mock apiserver ────────────────────────────────────────────

#[derive(Debug)]
struct Captured {
    method: String,
    path: String,
    body: Value,
}

/// A minimal HTTP/1.1 apiserver: serves one scripted JSON response per
/// request and records what was sent. Handles keep-alive connections and
/// multiple sequential connections (each `execute` call builds a client).
struct MockApiserver {
    addr: SocketAddr,
    captured: Arc<Mutex<Vec<Captured>>>,
}

impl MockApiserver {
    async fn start(responses: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock apiserver");
        let addr = listener.local_addr().expect("local addr");
        let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
        let scripted: Arc<Mutex<VecDeque<Value>>> =
            Arc::new(Mutex::new(responses.into_iter().collect()));

        let captured_task = Arc::clone(&captured);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let conn_captured = Arc::clone(&captured_task);
                let conn_scripted = Arc::clone(&scripted);
                tokio::spawn(handle_connection(stream, conn_captured, conn_scripted));
            }
        });

        Self { addr, captured }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn captured(&self) -> Vec<Captured> {
        std::mem::take(&mut *lock(&self.captured))
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn handle_connection(
    mut stream: TcpStream,
    captured: Arc<Mutex<Vec<Captured>>>,
    scripted: Arc<Mutex<VecDeque<Value>>>,
) {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let header_end = loop {
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                break pos;
            }
            let mut chunk = [0_u8; 4096];
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };

        let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap_or_default().to_owned();
        let path = request_parts.next().unwrap_or_default().to_owned();
        let content_length: usize = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse().ok())
            .unwrap_or(0);

        let body_start = header_end + 4;
        while buf.len() < body_start + content_length {
            let mut chunk = [0_u8; 4096];
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let body_bytes = buf[body_start..body_start + content_length].to_vec();
        buf.drain(..body_start + content_length);

        let body = if body_bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body_bytes).expect("request body is JSON")
        };
        lock(&captured).push(Captured { method, path, body });

        let Some(response) = lock(&scripted).pop_front() else {
            return;
        };
        let payload = serde_json::to_vec(&response).expect("encode response");
        let head_out = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            payload.len()
        );
        if stream.write_all(head_out.as_bytes()).await.is_err() {
            return;
        }
        if stream.write_all(&payload).await.is_err() {
            return;
        }
    }
}

// ── Fixtures (mirrors tests/fake_apiserver.rs) ────────────────

fn pod_json(namespace: &str, name: &str, phase: &str, ready: bool) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": name, "namespace": namespace },
        "spec": { "containers": [{ "name": "app", "image": "app:1" }], "nodeName": "worker-1" },
        "status": {
            "phase": phase,
            "conditions": [{ "type": "Ready", "status": if ready { "True" } else { "False" } }],
        },
    })
}

fn pod_list(items: &[Value]) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "PodList",
        "metadata": {},
        "items": items,
    })
}

fn node_json(name: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Node",
        "metadata": { "name": name },
    })
}

fn network_policy_json(namespace: &str, name: &str) -> Value {
    json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": { "name": name, "namespace": namespace },
    })
}

/// (method, path-without-query) pairs of the captured requests.
fn sent_requests(captured: &[Captured]) -> Vec<(&str, &str)> {
    captured
        .iter()
        .map(|r| {
            (
                r.method.as_str(),
                r.path.split('?').next().unwrap_or_default(),
            )
        })
        .collect()
}

// ── Argument validation (no API call expected) ────────────────

enum Expected {
    Missing(&'static str),
    Invalid(&'static str),
}

struct Case {
    function: &'static str,
    args: NativeArgs,
    expected: Expected,
}

async fn run_cases(cases: Vec<Case>) {
    install_crypto_provider();
    let _lock = env_lock().await;
    let kubeconfig = write_kubeconfig("http://127.0.0.1:1"); // unreachable; never dialed
    let _env = KubeconfigEnv::point_at(&kubeconfig);
    let executor = KubernetesExecutor;

    for case in cases {
        let err = executor
            .execute(case.function, &case.args)
            .await
            .expect_err("invalid arguments must fail before any API call");
        match (&case.expected, &err) {
            (Expected::Missing(argument), NativeError::MissingArgument { argument: actual }) => {
                assert_eq!(
                    actual, argument,
                    "{}: wrong missing argument reported",
                    case.function
                );
            }
            (
                Expected::Invalid(argument),
                NativeError::InvalidArgument {
                    argument: actual, ..
                },
            ) => {
                assert_eq!(
                    actual, argument,
                    "{}: wrong invalid argument reported",
                    case.function
                );
            }
            (expected, other) => {
                let wanted = match expected {
                    Expected::Missing(name) => format!("MissingArgument({name})"),
                    Expected::Invalid(name) => format!("InvalidArgument({name})"),
                };
                panic!("{}: expected {wanted}, got: {other:?}", case.function);
            }
        }
        let argument = match &case.expected {
            Expected::Missing(name) | Expected::Invalid(name) => name,
        };
        assert!(
            err.to_string().contains(argument),
            "{}: error message must name the argument: {err}",
            case.function
        );
    }
}

fn case(function: &'static str, pairs: &[(&str, Value)], expected: Expected) -> Case {
    Case {
        function,
        args: native_args(pairs),
        expected,
    }
}

#[tokio::test]
async fn dispatch_validates_action_arguments_before_any_api_call() {
    let cases = vec![
        case("delete_pod", &[], Expected::Missing("namespace")),
        case(
            "delete_pod",
            &[("namespace", json!("prod"))],
            Expected::Missing("name"),
        ),
        case("scale_deployment", &[], Expected::Missing("namespace")),
        case(
            "scale_deployment",
            &[("namespace", json!("prod"))],
            Expected::Missing("name"),
        ),
        case(
            "scale_deployment",
            &[("namespace", json!("prod")), ("name", json!("web"))],
            Expected::Missing("replicas"),
        ),
        case("cordon_node", &[], Expected::Missing("name")),
        case("uncordon_node", &[], Expected::Missing("name")),
        case("drain_node", &[], Expected::Missing("name")),
        case("apply_network_policy", &[], Expected::Missing("namespace")),
        case(
            "apply_network_policy",
            &[("namespace", json!("prod"))],
            Expected::Missing("policy"),
        ),
        case(
            "apply_network_policy",
            &[("namespace", json!("prod")), ("policy", json!(42))],
            Expected::Invalid("policy"),
        ),
        case("delete_network_policy", &[], Expected::Missing("namespace")),
        case(
            "delete_network_policy",
            &[("namespace", json!("prod"))],
            Expected::Missing("name"),
        ),
        case(
            "pod_network_latency",
            &[("namespace", json!("prod"))],
            Expected::Missing("pod (or label_selector)"),
        ),
        case(
            "pod_network_latency",
            &[("namespace", json!("prod")), ("pod", json!("web-0"))],
            Expected::Missing("delay_ms"),
        ),
        case(
            "pod_stress",
            &[("namespace", json!("prod"))],
            Expected::Missing("pod (or label_selector)"),
        ),
        case(
            "pod_stress",
            &[("namespace", json!("prod")), ("pod", json!("web-0"))],
            Expected::Missing("cpu_workers (or mem_bytes)"),
        ),
        case(
            "pod_stress",
            &[
                ("namespace", json!("prod")),
                ("pod", json!("web-0")),
                ("cpu_workers", json!(2)),
                ("mem_bytes", json!(1024)),
            ],
            Expected::Invalid("cpu_workers"),
        ),
    ];
    run_cases(cases).await;
}

#[tokio::test]
async fn dispatch_validates_probe_arguments_before_any_api_call() {
    let cases = vec![
        case("pod_is_ready", &[], Expected::Missing("namespace")),
        case(
            "pod_is_ready",
            &[("namespace", json!("prod"))],
            Expected::Missing("name"),
        ),
        case("deployment_is_ready", &[], Expected::Missing("namespace")),
        case(
            "deployment_is_ready",
            &[("namespace", json!("prod"))],
            Expected::Missing("name"),
        ),
        case("all_pods_ready", &[], Expected::Missing("namespace")),
        case(
            "all_pods_ready",
            &[("namespace", json!("prod"))],
            Expected::Missing("label_selector"),
        ),
        case("node_status", &[], Expected::Missing("name")),
        case("service_has_endpoints", &[], Expected::Missing("namespace")),
        case(
            "service_has_endpoints",
            &[("namespace", json!("prod"))],
            Expected::Missing("name"),
        ),
        case("count_pods_in_phase", &[], Expected::Missing("namespace")),
        case(
            "count_pods_in_phase",
            &[("namespace", json!("prod"))],
            Expected::Missing("label_selector"),
        ),
        case(
            "count_pods_in_phase",
            &[
                ("namespace", json!("prod")),
                ("label_selector", json!("app=web")),
            ],
            Expected::Missing("phase"),
        ),
    ];
    run_cases(cases).await;
}

// ── Client-init failure ───────────────────────────────────────

#[tokio::test]
async fn dispatch_reports_client_init_failure_for_missing_kubeconfig() {
    install_crypto_provider();
    let _lock = env_lock().await;
    let missing = std::env::temp_dir().join(format!(
        "tumult-no-such-kubeconfig-{}.yaml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&missing);
    let _env = KubeconfigEnv::point_at(&missing);

    let err = KubernetesExecutor
        .execute(
            "delete_pod",
            &native_args(&[("namespace", json!("prod")), ("name", json!("api-0"))]),
        )
        .await
        .expect_err("an unreadable kubeconfig must fail client init");

    assert!(
        matches!(err, NativeError::Execution { .. }),
        "expected Execution, got: {err:?}"
    );
    assert!(
        err.to_string().contains("kubernetes client init failed"),
        "message must identify the client init stage: {err}"
    );
}

// ── Full dispatch against the mock apiserver ──────────────────

#[tokio::test]
async fn dispatch_executes_pod_and_node_actions_against_apiserver() {
    install_crypto_provider();
    let server = MockApiserver::start(vec![
        pod_json("prod", "api-0", "Running", true), // delete_pod
        json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": "web", "namespace": "prod" },
            "spec": { "replicas": 0 },
        }),
        node_json("worker-1"), // cordon_node
        node_json("worker-1"), // uncordon_node
    ])
    .await;

    let _lock = env_lock().await;
    let kubeconfig = write_kubeconfig(&server.url());
    let _env = KubeconfigEnv::point_at(&kubeconfig);
    let executor = KubernetesExecutor;

    let message = executor
        .execute(
            "delete_pod",
            &native_args(&[
                ("namespace", json!("prod")),
                ("name", json!("api-0")),
                ("grace_period_seconds", json!(30)),
            ]),
        )
        .await
        .expect("delete_pod dispatch");
    assert_eq!(message, "pod prod/api-0 deleted");

    let message = executor
        .execute(
            "scale_deployment",
            &native_args(&[
                ("namespace", json!("prod")),
                ("name", json!("web")),
                ("replicas", json!(0)),
            ]),
        )
        .await
        .expect("scale_deployment dispatch");
    assert_eq!(message, "deployment prod/web scaled to 0 replicas");

    let message = executor
        .execute("cordon_node", &native_args(&[("name", json!("worker-1"))]))
        .await
        .expect("cordon_node dispatch");
    assert_eq!(message, "node worker-1 cordoned");

    let message = executor
        .execute(
            "uncordon_node",
            &native_args(&[("name", json!("worker-1"))]),
        )
        .await
        .expect("uncordon_node dispatch");
    assert_eq!(message, "node worker-1 uncordoned");

    let requests = server.captured();
    assert_eq!(
        sent_requests(&requests),
        vec![
            ("DELETE", "/api/v1/namespaces/prod/pods/api-0"),
            ("PATCH", "/apis/apps/v1/namespaces/prod/deployments/web"),
            ("PATCH", "/api/v1/nodes/worker-1"),
            ("PATCH", "/api/v1/nodes/worker-1"),
        ],
    );
    assert_eq!(
        requests[0].body["gracePeriodSeconds"], 30,
        "grace period must be forwarded: {}",
        requests[0].body
    );
    assert_eq!(
        requests[1].body["spec"]["replicas"], 0,
        "replica count must be forwarded: {}",
        requests[1].body
    );
    assert_eq!(requests[2].body["spec"]["unschedulable"], true);
    assert_eq!(requests[3].body["spec"]["unschedulable"], false);
}

#[tokio::test]
async fn dispatch_executes_drain_and_network_policies_against_apiserver() {
    install_crypto_provider();
    let mut daemon_pod = pod_json("kube-system", "logger-abc", "Running", true);
    daemon_pod["metadata"]["ownerReferences"] = json!([{
        "apiVersion": "apps/v1",
        "kind": "DaemonSet",
        "name": "logger",
        "uid": "d81ff353-0000-0000-0000-000000000000",
    }]);

    let server = MockApiserver::start(vec![
        // drain_node: cordon, pod list, one eviction
        node_json("worker-1"),
        pod_list(&[daemon_pod, pod_json("default", "app-a", "Running", true)]),
        json!({
            "apiVersion": "v1",
            "kind": "Status",
            "status": "Success",
            "code": 201,
        }),
        network_policy_json("prod", "deny-all"), // apply_network_policy
        network_policy_json("prod", "deny-all"), // delete_network_policy
    ])
    .await;

    let _lock = env_lock().await;
    let kubeconfig = write_kubeconfig(&server.url());
    let _env = KubeconfigEnv::point_at(&kubeconfig);
    let executor = KubernetesExecutor;

    let message = executor
        .execute("drain_node", &native_args(&[("name", json!("worker-1"))]))
        .await
        .expect("drain_node dispatch");
    assert_eq!(
        message,
        "node worker-1 drained: 1 evicted, 0 failed, 1 daemonset pods skipped"
    );

    let policy = json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": { "name": "deny-all", "namespace": "prod" },
        "spec": { "podSelector": {}, "policyTypes": ["Ingress", "Egress"] },
    });
    let message = executor
        .execute(
            "apply_network_policy",
            &native_args(&[("namespace", json!("prod")), ("policy", policy)]),
        )
        .await
        .expect("apply_network_policy dispatch");
    assert_eq!(message, "network policy prod/deny-all applied");

    let message = executor
        .execute(
            "delete_network_policy",
            &native_args(&[("namespace", json!("prod")), ("name", json!("deny-all"))]),
        )
        .await
        .expect("delete_network_policy dispatch");
    assert_eq!(message, "network policy prod/deny-all deleted");

    let requests = server.captured();
    assert_eq!(
        sent_requests(&requests),
        vec![
            ("PATCH", "/api/v1/nodes/worker-1"),
            ("GET", "/api/v1/pods"),
            ("POST", "/api/v1/namespaces/default/pods/app-a/eviction"),
            (
                "PATCH",
                "/apis/networking.k8s.io/v1/namespaces/prod/networkpolicies/deny-all"
            ),
            (
                "DELETE",
                "/apis/networking.k8s.io/v1/namespaces/prod/networkpolicies/deny-all"
            ),
        ],
    );
    assert!(
        requests[1]
            .path
            .contains("fieldSelector=spec.nodeName%3Dworker-1"),
        "drain must list pods on the node: {}",
        requests[1].path
    );
    assert_eq!(
        requests[3].body["spec"]["policyTypes"],
        json!(["Ingress", "Egress"]),
        "policy spec must be forwarded: {}",
        requests[3].body
    );
}

#[tokio::test]
async fn dispatch_executes_probes_against_apiserver() {
    install_crypto_provider();
    let server = MockApiserver::start(vec![
        pod_json("prod", "web-0", "Running", true), // pod_is_ready
        json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": "web", "namespace": "prod" },
            "spec": { "replicas": 3 },
            "status": { "readyReplicas": 2, "availableReplicas": 2, "updatedReplicas": 3 },
        }),
        pod_list(&[
            // all_pods_ready
            pod_json("prod", "web-0", "Running", true),
            pod_json("prod", "web-1", "Running", false),
            pod_json("prod", "web-2", "Running", true),
        ]),
        json!({
            // node_status
            "apiVersion": "v1",
            "kind": "Node",
            "metadata": { "name": "worker-1" },
            "spec": { "unschedulable": true },
            "status": {
                "conditions": [
                    { "type": "Ready", "status": "True" },
                    { "type": "MemoryPressure", "status": "False" },
                ],
            },
        }),
        json!({
            // service_has_endpoints
            "apiVersion": "v1",
            "kind": "Endpoints",
            "metadata": { "name": "web", "namespace": "prod" },
            "subsets": [{ "addresses": [{ "ip": "10.0.0.1" }], "ports": [{ "port": 80 }] }],
        }),
        pod_list(&[
            // count_pods_in_phase
            pod_json("prod", "web-0", "Running", true),
            pod_json("prod", "web-1", "Pending", false),
            pod_json("prod", "web-2", "Running", true),
        ]),
    ])
    .await;

    let _lock = env_lock().await;
    let kubeconfig = write_kubeconfig(&server.url());
    let _env = KubeconfigEnv::point_at(&kubeconfig);
    let executor = KubernetesExecutor;

    let ready = executor
        .execute(
            "pod_is_ready",
            &native_args(&[("namespace", json!("prod")), ("name", json!("web-0"))]),
        )
        .await
        .expect("pod_is_ready dispatch");
    assert_eq!(ready, "true");

    let status = executor
        .execute(
            "deployment_is_ready",
            &native_args(&[("namespace", json!("prod")), ("name", json!("web"))]),
        )
        .await
        .expect("deployment_is_ready dispatch");
    let status: Value = serde_json::from_str(&status).expect("deployment status is JSON");
    assert_eq!(status["desired"], 3);
    assert_eq!(status["ready"], 2);
    assert_eq!(status["available"], 2);
    assert_eq!(status["up_to_date"], 3);

    let counts = executor
        .execute(
            "all_pods_ready",
            &native_args(&[
                ("namespace", json!("prod")),
                ("label_selector", json!("app=web")),
            ]),
        )
        .await
        .expect("all_pods_ready dispatch");
    assert_eq!(counts, "{\"total\":3,\"ready\":2}");

    let node = executor
        .execute("node_status", &native_args(&[("name", json!("worker-1"))]))
        .await
        .expect("node_status dispatch");
    let node: Value = serde_json::from_str(&node).expect("node status is JSON");
    assert_eq!(node["ready"], true);
    assert_eq!(node["schedulable"], false);
    assert_eq!(node["conditions"].as_array().expect("conditions").len(), 2);

    let has_endpoints = executor
        .execute(
            "service_has_endpoints",
            &native_args(&[("namespace", json!("prod")), ("name", json!("web"))]),
        )
        .await
        .expect("service_has_endpoints dispatch");
    assert_eq!(has_endpoints, "true");

    let running = executor
        .execute(
            "count_pods_in_phase",
            &native_args(&[
                ("namespace", json!("prod")),
                ("label_selector", json!("app=web")),
                ("phase", json!("Running")),
            ]),
        )
        .await
        .expect("count_pods_in_phase dispatch");
    assert_eq!(running, "2");

    let requests = server.captured();
    let sent = sent_requests(&requests);
    assert!(
        sent.iter().all(|(method, _)| *method == "GET"),
        "probes must be read-only: {sent:?}"
    );
    let paths: Vec<&str> = sent.iter().map(|(_, path)| *path).collect();
    assert_eq!(
        paths,
        vec![
            "/api/v1/namespaces/prod/pods/web-0",
            "/apis/apps/v1/namespaces/prod/deployments/web",
            "/api/v1/namespaces/prod/pods",
            "/api/v1/nodes/worker-1",
            "/api/v1/namespaces/prod/endpoints/web",
            "/api/v1/namespaces/prod/pods",
        ],
        "executor must query exactly these paths: {paths:?}"
    );
    assert!(
        requests[2].path.contains("labelSelector=app%3Dweb"),
        "label selector must be forwarded: {}",
        requests[2].path
    );
    assert!(
        requests[5].path.contains("labelSelector=app%3Dweb"),
        "label selector must be forwarded: {}",
        requests[5].path
    );
}
