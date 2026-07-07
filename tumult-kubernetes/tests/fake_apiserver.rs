//! Hermetic behavior tests for actions and probes against a fake apiserver.
//!
//! Uses the kube-rs documented mock pattern: `kube::Client::new` wraps a
//! `tower_test::mock` service, so every request the crate would send to a
//! real apiserver is intercepted in-process. No network, no cluster.

use std::pin::pin;

use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;
use kube::client::Body;
use kube::Client;
use serde_json::{json, Value};
use tower_test::mock::{self, Handle};

use tumult_kubernetes::inject::{self, StressKind};
use tumult_kubernetes::{actions, discovery, probes, KubeError};

type MockHandle = Handle<Request<Body>, Response<Body>>;

/// Build a mock-backed client plus the handle used to script apiserver
/// behavior from the test body.
fn mock_client() -> (Client, MockHandle) {
    let (service, handle) = mock::pair::<Request<Body>, Response<Body>>();
    (Client::new(service, "default"), handle)
}

/// Split a captured request into parts and its JSON body (`Null` when empty).
async fn parts_and_json(request: Request<Body>) -> (http::request::Parts, Value) {
    let (parts, body) = request.into_parts();
    let bytes = body.collect().await.expect("collect body").to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("request body is JSON")
    };
    (parts, value)
}

fn json_response(body: &Value) -> Response<Body> {
    Response::new(Body::from(serde_json::to_vec(body).expect("encode body")))
}

fn status_response(code: StatusCode, reason: &str, message: &str) -> Response<Body> {
    let status = json!({
        "kind": "Status",
        "apiVersion": "v1",
        "metadata": {},
        "status": "Failure",
        "message": message,
        "reason": reason,
        "code": code.as_u16(),
    });
    Response::builder()
        .status(code)
        .body(Body::from(serde_json::to_vec(&status).expect("encode")))
        .expect("build response")
}

/// Minimal pod object; `ready` toggles the Ready condition.
fn pod_json(namespace: &str, name: &str, phase: &str, ready: bool, restarts: i32) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": name, "namespace": namespace },
        "spec": { "containers": [{ "name": "app", "image": "app:1" }], "nodeName": "worker-1" },
        "status": {
            "phase": phase,
            "conditions": [{ "type": "Ready", "status": if ready { "True" } else { "False" } }],
            "containerStatuses": [{
                "name": "app", "image": "app:1", "imageID": "", "ready": ready,
                "restartCount": restarts,
                "state": {},
            }],
        },
    })
}

// ── Pod-kill action ───────────────────────────────────────────

#[tokio::test]
async fn delete_pod_sends_delete_to_namespaced_path_with_grace_period() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, body) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::DELETE);
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/prod/pods/api-0");
        assert_eq!(
            body["gracePeriodSeconds"], 30,
            "grace period must be forwarded: {body}"
        );
        send.send_response(json_response(&pod_json(
            "prod", "api-0", "Running", true, 0,
        )));
    });

    let message = actions::delete_pod(client, "prod", "api-0", Some(30))
        .await
        .expect("delete succeeds");

    assert_eq!(message, "pod prod/api-0 deleted");
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn delete_pod_without_grace_period_sends_empty_delete_params() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, body) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::DELETE);
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/default/pods/cache-1");
        assert_eq!(
            body.get("gracePeriodSeconds"),
            None,
            "no grace period requested, none must be sent: {body}"
        );
        send.send_response(json_response(&pod_json(
            "default", "cache-1", "Running", true, 0,
        )));
    });

    actions::delete_pod(client, "default", "cache-1", None)
        .await
        .expect("delete succeeds");
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn delete_pod_404_yields_typed_api_error() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (_, send) = handle.next_request().await.expect("service not called");
        send.send_response(status_response(
            StatusCode::NOT_FOUND,
            "NotFound",
            "pods \"ghost\" not found",
        ));
    });

    let err = actions::delete_pod(client, "prod", "ghost", None)
        .await
        .expect_err("missing pod must fail");

    assert!(
        matches!(err, KubeError::Api(_)),
        "expected KubeError::Api, got: {err:?}"
    );
    assert!(
        err.to_string().contains("not found"),
        "unexpected message: {err}"
    );
    apiserver.await.expect("apiserver task");
}

// ── Deployment / node actions ─────────────────────────────────

#[tokio::test]
async fn scale_deployment_merge_patches_replica_count() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, body) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::PATCH);
        assert_eq!(
            parts.uri.path(),
            "/apis/apps/v1/namespaces/prod/deployments/web"
        );
        assert_eq!(
            parts.headers[http::header::CONTENT_TYPE],
            "application/merge-patch+json"
        );
        assert_eq!(body, json!({ "spec": { "replicas": 0 } }));
        send.send_response(json_response(&json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": "web", "namespace": "prod" },
            "spec": { "replicas": 0 },
        })));
    });

    let message = actions::scale_deployment(client, "prod", "web", 0)
        .await
        .expect("scale succeeds");

    assert_eq!(message, "deployment prod/web scaled to 0 replicas");
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn cordon_node_patches_unschedulable_true_on_cluster_scoped_path() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, body) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::PATCH);
        assert_eq!(parts.uri.path(), "/api/v1/nodes/worker-1");
        assert_eq!(body, json!({ "spec": { "unschedulable": true } }));
        send.send_response(json_response(&json!({
            "apiVersion": "v1",
            "kind": "Node",
            "metadata": { "name": "worker-1" },
            "spec": { "unschedulable": true },
        })));
    });

    let message = actions::cordon_node(client, "worker-1")
        .await
        .expect("cordon succeeds");

    assert_eq!(message, "node worker-1 cordoned");
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn drain_node_cordons_lists_by_node_and_skips_daemonset_pods() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);

        // 1. Cordon patch.
        let (request, send) = handle.next_request().await.expect("cordon request");
        let (parts, body) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::PATCH);
        assert_eq!(parts.uri.path(), "/api/v1/nodes/worker-1");
        assert_eq!(body, json!({ "spec": { "unschedulable": true } }));
        send.send_response(json_response(&json!({
            "apiVersion": "v1",
            "kind": "Node",
            "metadata": { "name": "worker-1" },
        })));

        // 2. Cluster-wide pod list filtered to the drained node.
        let (request, send) = handle.next_request().await.expect("list request");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::GET);
        assert_eq!(parts.uri.path(), "/api/v1/pods");
        let query = parts.uri.query().unwrap_or_default();
        assert!(
            query.contains("fieldSelector=spec.nodeName%3Dworker-1"),
            "list must filter by node: {query}"
        );
        let mut daemon_pod = pod_json("kube-system", "logger-abc", "Running", true, 0);
        daemon_pod["metadata"]["ownerReferences"] = json!([{
            "apiVersion": "apps/v1",
            "kind": "DaemonSet",
            "name": "logger",
            "uid": "d81ff353-0000-0000-0000-000000000000",
        }]);
        send.send_response(json_response(&json!({
            "apiVersion": "v1",
            "kind": "PodList",
            "metadata": {},
            "items": [
                daemon_pod,
                pod_json("default", "app-a", "Running", true, 0),
                pod_json("prod", "app-b", "Running", true, 0),
            ],
        })));

        // 3. First eviction succeeds.
        let (request, send) = handle.next_request().await.expect("first delete");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::DELETE);
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/default/pods/app-a");
        send.send_response(json_response(&pod_json(
            "default", "app-a", "Running", true, 0,
        )));

        // 4. Second eviction is rejected by the apiserver.
        let (request, send) = handle.next_request().await.expect("second delete");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::DELETE);
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/prod/pods/app-b");
        send.send_response(status_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "etcd is on fire",
        ));
    });

    let result = actions::drain_node(client, "worker-1", None)
        .await
        .expect("drain returns a result even with partial failures");

    assert_eq!(result.node, "worker-1");
    assert_eq!(result.evicted, vec!["default/app-a".to_string()]);
    assert_eq!(result.failed.len(), 1);
    assert_eq!(result.failed[0].0, "prod/app-b");
    assert_eq!(result.skipped_daemonsets, 1);
    assert_eq!(
        result.to_string(),
        "node worker-1 drained: 1 evicted, 1 failed, 1 daemonset pods skipped"
    );
    apiserver.await.expect("apiserver task");
}

// ── Network-policy actions ────────────────────────────────────

#[tokio::test]
async fn apply_network_policy_server_side_applies_named_policy() {
    let policy_json = json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": { "name": "deny-all", "namespace": "prod" },
        "spec": { "podSelector": {}, "policyTypes": ["Ingress", "Egress"] },
    });
    let policy = serde_json::from_value(policy_json.clone()).expect("valid policy");

    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, body) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::PATCH);
        assert_eq!(
            parts.uri.path(),
            "/apis/networking.k8s.io/v1/namespaces/prod/networkpolicies/deny-all"
        );
        assert_eq!(
            parts.headers[http::header::CONTENT_TYPE],
            "application/apply-patch+yaml"
        );
        let query = parts.uri.query().unwrap_or_default();
        assert!(
            query.contains("fieldManager=tumult"),
            "apply must set field manager: {query}"
        );
        assert_eq!(body["spec"]["policyTypes"], json!(["Ingress", "Egress"]));
        send.send_response(json_response(&policy_json));
    });

    let message = actions::apply_network_policy(client, "prod", policy)
        .await
        .expect("apply succeeds");

    assert_eq!(message, "network policy prod/deny-all applied");
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn delete_network_policy_deletes_by_namespace_and_name() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::DELETE);
        assert_eq!(
            parts.uri.path(),
            "/apis/networking.k8s.io/v1/namespaces/prod/networkpolicies/deny-all"
        );
        send.send_response(json_response(&json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": { "name": "deny-all", "namespace": "prod" },
        })));
    });

    let message = actions::delete_network_policy(client, "prod", "deny-all")
        .await
        .expect("delete succeeds");

    assert_eq!(message, "network policy prod/deny-all deleted");
    apiserver.await.expect("apiserver task");
}

// ── In-pod data-plane injection (ephemeral containers) ────────

#[tokio::test]
async fn pod_network_latency_patches_ephemeralcontainers_with_tc_netem() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, body) = parts_and_json(request).await;

        // Must hit the ephemeralcontainers subresource of the named pod.
        assert_eq!(parts.method, Method::PATCH);
        assert_eq!(
            parts.uri.path(),
            "/api/v1/namespaces/prod/pods/web-0/ephemeralcontainers"
        );
        // Strategic merge appends without clobbering existing ephemeral containers.
        assert_eq!(
            parts.headers[http::header::CONTENT_TYPE],
            "application/strategic-merge-patch+json"
        );

        let container = &body["spec"]["ephemeralContainers"][0];
        assert_eq!(container["image"], "tc-image:test");
        assert!(
            container["name"]
                .as_str()
                .expect("name is string")
                .starts_with("tumult-netem-"),
            "unexpected container name: {}",
            container["name"]
        );
        // NET_ADMIN is required for `tc` to touch the qdisc.
        assert_eq!(
            container["securityContext"]["capabilities"]["add"],
            json!(["NET_ADMIN"])
        );
        let cmd = container["command"][2]
            .as_str()
            .expect("command script is a string");
        assert!(
            cmd.contains("tc qdisc add dev eth0 root netem delay 250ms 25ms"),
            "cmd: {cmd}"
        );
        assert!(cmd.contains("sleep 45"), "must self-terminate: {cmd}");
        assert!(
            cmd.contains("tc qdisc del dev eth0 root netem"),
            "must restore: {cmd}"
        );

        send.send_response(json_response(&pod_json(
            "prod", "web-0", "Running", true, 0,
        )));
    });

    let message = inject::pod_network_latency(
        client,
        "prod",
        "web-0",
        250,
        25,
        45,
        "eth0",
        "tc-image:test",
    )
    .await
    .expect("latency injection succeeds");

    assert!(message.contains("250ms latency"), "message: {message}");
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn pod_network_latency_rejects_zero_delay_without_api_call() {
    let (client, _handle) = mock_client();
    // No apiserver task spawned: validation must fail before any request.
    let err = inject::pod_network_latency(client, "prod", "web-0", 0, 0, 30, "eth0", "img")
        .await
        .expect_err("zero delay must be rejected");
    assert!(
        matches!(
            err,
            KubeError::InvalidConfig {
                field: "delay_ms",
                ..
            }
        ),
        "expected InvalidConfig(delay_ms), got: {err:?}"
    );
}

#[tokio::test]
async fn pod_network_latency_404_pod_yields_typed_api_error() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (_, send) = handle.next_request().await.expect("service not called");
        send.send_response(status_response(
            StatusCode::NOT_FOUND,
            "NotFound",
            "pods \"ghost\" not found",
        ));
    });

    let err = inject::pod_network_latency(client, "prod", "ghost", 100, 0, 30, "eth0", "img")
        .await
        .expect_err("missing pod must fail");
    assert!(
        matches!(err, KubeError::Api(_)),
        "expected KubeError::Api, got: {err:?}"
    );
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn pod_stress_cpu_patches_ephemeralcontainers_with_stress_ng() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, body) = parts_and_json(request).await;

        assert_eq!(parts.method, Method::PATCH);
        assert_eq!(
            parts.uri.path(),
            "/api/v1/namespaces/prod/pods/api-7/ephemeralcontainers"
        );
        let container = &body["spec"]["ephemeralContainers"][0];
        assert_eq!(container["image"], "stress-image:test");
        assert!(
            container["name"]
                .as_str()
                .expect("name")
                .starts_with("tumult-stress-"),
            "name: {}",
            container["name"]
        );
        // Lands in the target container's process namespace.
        assert_eq!(container["targetContainerName"], "app");
        let cmd = container["command"][2].as_str().expect("script");
        assert!(cmd.contains("stress-ng --cpu 3"), "cmd: {cmd}");
        assert!(cmd.contains("--timeout 60s"), "self-terminating: {cmd}");

        send.send_response(json_response(&pod_json(
            "prod", "api-7", "Running", true, 0,
        )));
    });

    let message = inject::pod_stress(
        client,
        "prod",
        "api-7",
        StressKind::Cpu { workers: 3 },
        60,
        Some("app"),
        "stress-image:test",
    )
    .await
    .expect("stress injection succeeds");

    assert!(
        message.contains("3 CPU workers stress"),
        "message: {message}"
    );
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn pod_stress_memory_sets_vm_bytes_and_no_target_container() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, body) = parts_and_json(request).await;
        assert_eq!(
            parts.uri.path(),
            "/api/v1/namespaces/default/pods/cache-0/ephemeralcontainers"
        );
        let container = &body["spec"]["ephemeralContainers"][0];
        assert!(
            container.get("targetContainerName").is_none(),
            "no target container was requested: {container}"
        );
        let cmd = container["command"][2].as_str().expect("script");
        assert!(
            cmd.contains("--vm 1 --vm-bytes 268435456 --vm-keep"),
            "cmd: {cmd}"
        );
        send.send_response(json_response(&pod_json(
            "default", "cache-0", "Running", true, 0,
        )));
    });

    inject::pod_stress(
        client,
        "default",
        "cache-0",
        StressKind::Memory { bytes: 268_435_456 },
        30,
        None,
        "stress-image:test",
    )
    .await
    .expect("memory stress injection succeeds");
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn pod_stress_rejects_zero_cpu_workers_without_api_call() {
    let (client, _handle) = mock_client();
    let err = inject::pod_stress(
        client,
        "prod",
        "web-0",
        StressKind::Cpu { workers: 0 },
        30,
        None,
        "img",
    )
    .await
    .expect_err("zero workers must be rejected");
    assert!(
        matches!(
            err,
            KubeError::InvalidConfig {
                field: "cpu_workers",
                ..
            }
        ),
        "expected InvalidConfig(cpu_workers), got: {err:?}"
    );
}

#[tokio::test]
async fn resolve_target_pod_lists_by_label_and_picks_first_match() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::GET);
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/prod/pods");
        let query = parts.uri.query().unwrap_or_default();
        assert!(
            query.contains("labelSelector=app%3Dweb"),
            "selector must be forwarded: {query}"
        );
        send.send_response(json_response(&json!({
            "apiVersion": "v1",
            "kind": "PodList",
            "metadata": {},
            "items": [
                pod_json("prod", "web-abc", "Running", true, 0),
                pod_json("prod", "web-def", "Running", true, 0),
            ],
        })));
    });

    let pod = inject::resolve_target_pod(client, "prod", None, Some("app=web"))
        .await
        .expect("resolution succeeds");
    assert_eq!(pod, "web-abc");
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn resolve_target_pod_explicit_name_skips_api_call() {
    let (client, _handle) = mock_client();
    // No apiserver task: an explicit pod name must not trigger a list request.
    let pod = inject::resolve_target_pod(client, "prod", Some("chosen-0"), None)
        .await
        .expect("explicit name resolves without a call");
    assert_eq!(pod, "chosen-0");
}

#[tokio::test]
async fn resolve_target_pod_empty_selector_match_is_typed_error() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (_, send) = handle.next_request().await.expect("service not called");
        send.send_response(json_response(&json!({
            "apiVersion": "v1",
            "kind": "PodList",
            "metadata": {},
            "items": [],
        })));
    });

    let err = inject::resolve_target_pod(client, "prod", None, Some("app=ghost"))
        .await
        .expect_err("no match must fail");
    assert!(
        matches!(
            err,
            KubeError::InvalidConfig {
                field: "label_selector",
                ..
            }
        ),
        "expected InvalidConfig(label_selector), got: {err:?}"
    );
    apiserver.await.expect("apiserver task");
}

// ── Topology discovery ────────────────────────────────────────

/// Minimal Service object with optional labels and selector.
fn service_json(namespace: &str, name: &str, labels: Value, selector: Value) -> Value {
    let mut svc = json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": { "name": name, "namespace": namespace },
        "spec": { "ports": [{ "port": 80 }] },
    });
    if !labels.is_null() {
        svc["metadata"]["labels"] = labels;
    }
    if !selector.is_null() {
        svc["spec"]["selector"] = selector;
    }
    svc
}

fn service_list(items: &[Value]) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "ServiceList",
        "metadata": {},
        "items": items,
    })
}

#[tokio::test]
async fn discover_services_lists_each_requested_namespace_and_sorts_output() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);

        // One list per requested namespace, in argument order.
        let (request, send) = handle.next_request().await.expect("prod list");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::GET);
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/prod/services");
        send.send_response(json_response(&service_list(&[
            // Unsorted on purpose: output order must not depend on apiserver order.
            service_json("prod", "web", Value::Null, json!({ "app": "web-app" })),
            service_json(
                "prod",
                "api",
                json!({ "tumult.io/tier": "service", "tumult.io/owner": "team-core" }),
                json!({ "app.kubernetes.io/name": "api-chart" }),
            ),
        ])));

        let (request, send) = handle.next_request().await.expect("staging list");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/staging/services");
        send.send_response(json_response(&service_list(&[service_json(
            "staging",
            "cache",
            json!({ "app.kubernetes.io/component": "cache" }),
            Value::Null,
        )])));
    });

    let services = discovery::discover_services(
        client,
        &["prod".to_string(), "staging".to_string()],
    )
    .await
    .expect("discovery succeeds");

    let names: Vec<(&str, &str)> = services
        .iter()
        .map(|s| (s.namespace.as_str(), s.name.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![("prod", "api"), ("prod", "web"), ("staging", "cache")],
        "deterministic (namespace, name) order"
    );
    assert_eq!(services[0].tier.as_deref(), Some("service"));
    assert_eq!(services[0].owner.as_deref(), Some("team-core"));
    assert_eq!(services[0].selector_apps, vec!["api-chart"]);
    assert_eq!(services[2].tier.as_deref(), Some("cache"));
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn discover_services_all_namespaces_skips_kube_system_and_apiserver_service() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("cluster-wide list");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::GET);
        assert_eq!(
            parts.uri.path(),
            "/api/v1/services",
            "empty namespace list must scan the whole cluster"
        );
        send.send_response(json_response(&service_list(&[
            service_json("default", "kubernetes", Value::Null, Value::Null),
            service_json("kube-system", "kube-dns", Value::Null, Value::Null),
            service_json("default", "shop", Value::Null, json!({ "app": "shop" })),
        ])));
    });

    let services = discovery::discover_services(client, &[])
        .await
        .expect("discovery succeeds");

    assert_eq!(services.len(), 1, "plumbing services are skipped: {services:?}");
    assert_eq!(services[0].name, "shop");
    assert_eq!(services[0].namespace, "default");
    apiserver.await.expect("apiserver task");
}

// ── Probes ────────────────────────────────────────────────────

#[tokio::test]
async fn pods_by_label_sends_selector_and_parses_statuses() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::GET);
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/prod/pods");
        let query = parts.uri.query().unwrap_or_default();
        assert!(
            query.contains("labelSelector=app%3Dweb"),
            "label selector must be forwarded: {query}"
        );
        send.send_response(json_response(&json!({
            "apiVersion": "v1",
            "kind": "PodList",
            "metadata": {},
            "items": [
                pod_json("prod", "web-0", "Running", true, 2),
                pod_json("prod", "web-1", "Pending", false, 0),
            ],
        })));
    });

    let statuses = probes::pods_by_label(client, "prod", "app=web")
        .await
        .expect("list succeeds");

    assert_eq!(statuses.len(), 2);
    assert_eq!(statuses[0].name, "web-0");
    assert_eq!(statuses[0].namespace, "prod");
    assert_eq!(statuses[0].phase, "Running");
    assert!(statuses[0].ready);
    assert_eq!(statuses[0].restarts, 2);
    assert_eq!(statuses[0].node, "worker-1");
    assert_eq!(statuses[1].phase, "Pending");
    assert!(!statuses[1].ready);
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn all_pods_ready_counts_ready_versus_total() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (_, send) = handle.next_request().await.expect("service not called");
        send.send_response(json_response(&json!({
            "apiVersion": "v1",
            "kind": "PodList",
            "metadata": {},
            "items": [
                pod_json("prod", "web-0", "Running", true, 0),
                pod_json("prod", "web-1", "Running", false, 1),
                pod_json("prod", "web-2", "Running", true, 0),
            ],
        })));
    });

    let (total, ready) = probes::all_pods_ready(client, "prod", "app=web")
        .await
        .expect("probe succeeds");

    assert_eq!((total, ready), (3, 2));
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn pod_is_ready_reports_false_without_ready_condition() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.method, Method::GET);
        assert_eq!(parts.uri.path(), "/api/v1/namespaces/prod/pods/web-0");
        // Pod with no status conditions at all (e.g. just scheduled).
        send.send_response(json_response(&json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": { "name": "web-0", "namespace": "prod" },
            "spec": { "containers": [{ "name": "app", "image": "app:1" }] },
        })));
    });

    let ready = probes::pod_is_ready(client, "prod", "web-0")
        .await
        .expect("probe succeeds");

    assert!(
        !ready,
        "pod without Ready condition must not count as ready"
    );
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn pod_is_ready_apiserver_500_yields_typed_error() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (_, send) = handle.next_request().await.expect("service not called");
        send.send_response(status_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "apiserver overloaded",
        ));
    });

    let err = probes::pod_is_ready(client, "prod", "web-0")
        .await
        .expect_err("500 must surface as an error");

    assert!(
        matches!(err, KubeError::Api(_)),
        "expected KubeError::Api, got: {err:?}"
    );
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn deployment_is_ready_parses_replica_counts() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(
            parts.uri.path(),
            "/apis/apps/v1/namespaces/prod/deployments/web"
        );
        send.send_response(json_response(&json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": "web", "namespace": "prod" },
            "spec": { "replicas": 3 },
            "status": { "readyReplicas": 2, "availableReplicas": 2, "updatedReplicas": 3 },
        })));
    });

    let status = probes::deployment_is_ready(client, "prod", "web")
        .await
        .expect("probe succeeds");

    assert_eq!(status.desired, 3);
    assert_eq!(status.ready, 2);
    assert_eq!(status.available, 2);
    assert_eq!(status.up_to_date, 3);
    apiserver.await.expect("apiserver task");
}

#[tokio::test]
async fn node_status_reports_cordoned_ready_node() {
    let (client, handle) = mock_client();
    let apiserver = tokio::spawn(async move {
        let mut handle = pin!(handle);
        let (request, send) = handle.next_request().await.expect("service not called");
        let (parts, _) = parts_and_json(request).await;
        assert_eq!(parts.uri.path(), "/api/v1/nodes/worker-1");
        send.send_response(json_response(&json!({
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
        })));
    });

    let status = probes::node_status(client, "worker-1")
        .await
        .expect("probe succeeds");

    assert!(status.ready, "Ready=True condition must map to ready");
    assert!(
        !status.schedulable,
        "unschedulable=true must map to schedulable=false"
    );
    assert_eq!(status.conditions.len(), 2);
    apiserver.await.expect("apiserver task");
}
