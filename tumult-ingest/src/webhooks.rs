//! Outbound event notifications: the webhook dispatcher (schema v11
//! `webhooks` / `webhook_cursors`).
//!
//! Mirrors the schedule scheduler: every tick, each enabled webhook receives
//! the `run_audit` events newer than its delivery cursor as signed JSON
//! POSTs. Deliveries are **fire-and-log** (v1): one immediate retry on
//! failure, failures are logged via `tracing`, and the cursor advances
//! regardless — a down receiver misses events instead of blocking the
//! pipeline. Payloads are signed `X-Tumult-Signature: sha256=<hmac-sha256>`
//! with the webhook's secret.
//!
//! SSRF policy (conservative): URLs must be `https`; plain `http` requires
//! `TUMULTD_WEBHOOK_ALLOW_INSECURE=1`, and loopback / unspecified /
//! private / link-local IP literals require `TUMULTD_WEBHOOK_ALLOW_LOCAL=1`
//! (both exist for local demos and tests). Hostnames are not resolved at
//! validation time — DNS-level rebinding remains an accepted residual risk,
//! mitigated by the Admin-only management role.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hmac::{Hmac, Mac};
use tokio_util::sync::CancellationToken;
use tumult_lake::{Store, WebhookRow};

use crate::IngestWriter;

/// The dispatcher's tick interval from `TUMULTD_WEBHOOK_TICK_S` (default
/// 15s, minimum 1s); invalid values fall back to the default.
#[must_use]
pub fn tick_from_env() -> Duration {
    std::env::var("TUMULTD_WEBHOOK_TICK_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or_else(|| Duration::from_secs(15), Duration::from_secs)
}

fn env_flag(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Validate a webhook URL against the SSRF policy (env-flag driven; see the
/// module docs).
///
/// # Errors
/// Returns a human-readable reason when the URL is not acceptable.
pub fn validate_webhook_url(url: &str) -> Result<(), String> {
    validate_url_with(
        url,
        env_flag("TUMULTD_WEBHOOK_ALLOW_INSECURE"),
        env_flag("TUMULTD_WEBHOOK_ALLOW_LOCAL"),
    )
}

fn validate_url_with(url: &str, allow_insecure: bool, allow_local: bool) -> Result<(), String> {
    if url.chars().count() > 2_000 {
        return Err("url too long".into());
    }
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if allow_insecure => {}
        other => {
            return Err(format!(
                "scheme {other:?} not allowed: webhooks must be https (TUMULTD_WEBHOOK_ALLOW_INSECURE=1 permits http)"
            ));
        }
    }
    let host = parsed.host_str().ok_or("url must name a host")?;
    // url::Url keeps IPv6 brackets in host_str ("[::1]"); strip them before
    // the IP-literal check or v6 literals sail through as "hostnames".
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        if !allow_local && is_local(ip) {
            return Err(
                "loopback, private and link-local addresses are not allowed (TUMULTD_WEBHOOK_ALLOW_LOCAL=1 permits them)"
                    .into(),
            );
        }
    }
    Ok(())
}

/// Loopback, unspecified, private, or link-local address.
fn is_local(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_unspecified() || v4.is_private() || v4.is_link_local()
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            let seg = v6.segments()[0];
            // fe80::/10 link-local; fc00::/7 unique-local.
            (seg & 0xffc0) == 0xfe80 || (seg & 0xfe00) == 0xfc00
        }
    }
}

/// Lowercase hex HMAC-SHA256 of `msg` under `key` — the value behind
/// `X-Tumult-Signature: sha256=<hex>`.
#[must_use]
pub fn hmac_sha256_hex(key: &str, msg: &str) -> String {
    let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(key.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(msg.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from(b"0123456789abcdef"[usize::from(b >> 4)]));
        out.push(char::from(b"0123456789abcdef"[usize::from(b & 0x0f)]));
    }
    out
}

/// Spawn the webhook dispatcher (same shutdown contract as the schedule
/// scheduler: cancel the token and await before draining the writer).
pub fn spawn_webhook_dispatcher(
    db_path: PathBuf,
    ingest: IngestWriter,
    tick: Duration,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = dispatch_pending(&db_path, &ingest).await {
                        tracing::warn!(error = %e, "webhook dispatch tick failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("webhook dispatcher exiting (shutdown)");
                    break;
                }
            }
        }
    })
}

/// One dispatch tick: deliver due audit events to every enabled webhook.
/// Returns the number of events delivered. Fire-and-log: the cursor
/// advances past failures (one immediate retry each), so a down receiver
/// misses events rather than blocking the pipeline.
pub async fn dispatch_pending(db_path: &Path, ingest: &IngestWriter) -> Result<usize, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;
    let webhooks = Store::at(db_path)
        .read_only()
        .map_err(|e| e.to_string())?
        .enabled_webhooks()
        .map_err(|e| e.to_string())?;
    let mut delivered = 0usize;
    for hook in webhooks {
        delivered += dispatch_one(db_path, ingest, &client, &hook).await?;
    }
    Ok(delivered)
}

/// Deliver one webhook's due events; returns the delivered count.
async fn dispatch_one(
    db_path: &Path,
    ingest: &IngestWriter,
    client: &reqwest::Client,
    hook: &WebhookRow,
) -> Result<usize, String> {
    if let Err(reason) = validate_webhook_url(&hook.url) {
        tracing::warn!(webhook = %hook.id, error = %reason, "webhook URL fails the SSRF policy; skipping");
        return Ok(0);
    }
    let reader = Store::at(db_path).read_only().map_err(|e| e.to_string())?;
    let cursor = reader
        .webhook_cursor(&hook.id)
        .map_err(|e| e.to_string())?
        .unwrap_or(hook.created_at_ns);
    let events = reader
        .query_json_rows(&format!(
            "SELECT run_id, at_ns, event, detail, actor FROM run_audit \
             WHERE at_ns > {cursor} ORDER BY at_ns LIMIT 100"
        ))
        .map_err(|e| e.to_string())?;
    if events.is_empty() {
        return Ok(0);
    }
    let mut delivered = 0usize;
    let mut max_at_ns = cursor;
    for event in &events {
        let at_ns = event["at_ns"].as_i64().unwrap_or(cursor);
        max_at_ns = max_at_ns.max(at_ns);
        let name = event["event"].as_str().unwrap_or_default();
        if !hook.events.is_empty() && !hook.events.iter().any(|e| e == name) {
            continue;
        }
        let body = serde_json::json!({
            "event": name,
            "run_id": event["run_id"],
            "at_ns": at_ns,
            "actor": event["actor"],
            "detail": event["detail"],
        })
        .to_string();
        if post_signed(client, hook, &body).await {
            delivered += 1;
        }
    }
    let id = hook.id.clone();
    ingest
        .write(crate::Batch::Exec(Box::new(move |writer| {
            writer
                .set_webhook_cursor(&id, max_at_ns)
                .map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| e.to_string())?;
    Ok(delivered)
}

/// POST one signed payload with one immediate retry; `false` on failure
/// (already logged).
async fn post_signed(client: &reqwest::Client, hook: &WebhookRow, body: &str) -> bool {
    let signature = format!("sha256={}", hmac_sha256_hex(&hook.secret, body));
    for attempt in 1..=2 {
        let result = client
            .post(&hook.url)
            .header("content-type", "application/json")
            .header("x-tumult-signature", &signature)
            .body(body.to_string())
            .send()
            .await;
        match result {
            Ok(resp) if resp.status().is_success() => return true,
            Ok(resp) => {
                tracing::warn!(webhook = %hook.id, status = %resp.status(), attempt, "webhook delivery rejected");
            }
            Err(e) => {
                tracing::warn!(webhook = %hook.id, error = %e, attempt, "webhook delivery failed");
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_policy() {
        // https hostnames are always fine.
        assert!(validate_url_with("https://hooks.example.com/x", false, false).is_ok());
        // http needs the insecure opt-in.
        assert!(validate_url_with("http://hooks.example.com/x", false, false).is_err());
        assert!(validate_url_with("http://hooks.example.com/x", true, false).is_ok());
        // Other schemes are never allowed.
        assert!(validate_url_with("ftp://example.com/x", true, true).is_err());
        // Local IPs need the local opt-in.
        for local in [
            "https://127.0.0.1:8080/x",
            "https://[::1]/x",
            "https://169.254.169.254/latest",
            "https://192.168.1.10/x",
            "https://10.0.0.4/x",
            "https://[fe80::1]/x",
            "https://[fd00::1]/x",
        ] {
            assert!(validate_url_with(local, false, false).is_err(), "{local}");
            assert!(validate_url_with(local, false, true).is_ok(), "{local}");
        }
        // Garbage is rejected.
        assert!(validate_url_with("not-a-url", false, false).is_err());
    }

    #[test]
    fn hmac_matches_rfc4231_case2() {
        // RFC 4231 test case 2.
        assert_eq!(
            hmac_sha256_hex("Jefe", "what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
