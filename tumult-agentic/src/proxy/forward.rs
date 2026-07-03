//! Upstream request forwarding and hop-by-hop header handling.

use std::collections::HashMap;

use axum::body::Bytes;

use crate::model::AgenticError;

pub(crate) struct UpstreamResponse {
    pub(crate) status: u16,
    pub(crate) content_type: Option<String>,
    pub(crate) body: String,
}

pub(crate) async fn forward(
    client: &reqwest::Client,
    parts: &axum::http::request::Parts,
    url: &str,
    body: Bytes,
    trace: &opentelemetry::Context,
) -> Result<UpstreamResponse, AgenticError> {
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .map_err(|err| AgenticError::Adapter(format!("bad method: {err}")))?;
    let mut builder = client.request(method, url);
    for (name, value) in &parts.headers {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        if let (Ok(rname), Ok(rvalue)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            builder = builder.header(rname, rvalue);
        }
    }
    // Propagate this proxy span's W3C trace context upstream so a compliant
    // intermediary or server continues the same distributed trace.
    let mut trace_headers: HashMap<String, String> = HashMap::new();
    tumult_otel::propagation::inject_traceparent(trace, &mut trace_headers);
    for (name, value) in trace_headers {
        if let (Ok(rname), Ok(rvalue)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            builder = builder.header(rname, rvalue);
        }
    }
    let response = builder
        .body(body.to_vec())
        .send()
        .await
        .map_err(|err| AgenticError::Adapter(format!("upstream request failed: {err}")))?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response
        .bytes()
        .await
        .map_err(|err| AgenticError::Adapter(format!("upstream body read failed: {err}")))?;
    let body = String::from_utf8_lossy(&bytes).into_owned();

    Ok(UpstreamResponse {
        status,
        content_type,
        body,
    })
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            // Force identity encoding so we can read and mutate the body.
            | "accept-encoding"
            // Managed by the proxy span: strip inbound W3C context so we inject
            // our own (avoids duplicate traceparent on the upstream request).
            | "traceparent"
            | "tracestate"
            | "connection"
            | "transfer-encoding"
            | "keep-alive"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "upgrade"
    )
}
