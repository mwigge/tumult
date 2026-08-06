//! The stateful webhook dispatcher: per-endpoint tasks, cross-tick
//! exponential backoff, and dead-lettering after bounded retries — see the
//! parent module docs for the delivery semantics.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use tumult_lake::{Store, WebhookDeadLetter, WebhookRow};

use super::policy::{hmac_sha256_hex, validate_webhook_url};
use super::{endpoint_budget_from_env, max_attempts_from_env};
use crate::{daemon_metrics, IngestWriter};

/// Per-endpoint retry/backoff state, held by the dispatcher across ticks.
#[derive(Clone, Copy, Default)]
struct EndpointBackoff {
    /// Consecutive failing ticks (drives the dead-letter threshold).
    failures: u32,
    /// Skip attempts until this tick (exponential backoff across ticks).
    next_attempt_tick: u64,
}

/// The stateful dispatcher: one shared `reqwest::Client` (connection pool
/// hoisted out of the tick) plus per-endpoint backoff. One instance lives
/// in the dispatcher task; tests drive it directly.
pub struct Dispatcher {
    client: reqwest::Client,
    endpoints: HashMap<String, EndpointBackoff>,
    tick: u64,
    max_attempts: u32,
    endpoint_budget: Duration,
}

impl Dispatcher {
    /// From the environment (`TUMULTD_WEBHOOK_MAX_ATTEMPTS`,
    /// `TUMULTD_WEBHOOK_ENDPOINT_BUDGET_S`) with a 2s per-request timeout.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be built.
    pub fn from_env() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self::new(
            client,
            max_attempts_from_env(),
            endpoint_budget_from_env(),
        ))
    }

    #[must_use]
    pub fn new(client: reqwest::Client, max_attempts: u32, endpoint_budget: Duration) -> Self {
        Self {
            client,
            endpoints: HashMap::new(),
            tick: 0,
            max_attempts,
            endpoint_budget,
        }
    }

    /// One dispatch tick: deliver due audit events to every enabled webhook,
    /// each in its own task under its own time budget. Returns the number of
    /// events delivered. A failure to *list* webhooks aborts the tick;
    /// per-endpoint failures are logged, counted and isolated.
    ///
    /// # Errors
    /// Returns an error when the store cannot be read at all.
    pub async fn dispatch_pending(
        &mut self,
        db_path: &Path,
        ingest: &IngestWriter,
    ) -> Result<usize, String> {
        let webhooks = Store::at(db_path)
            .read_only()
            .map_err(|e| e.to_string())?
            .enabled_webhooks()
            .map_err(|e| e.to_string())?;
        self.tick += 1;
        let tick = self.tick;
        let mut set = tokio::task::JoinSet::new();
        for hook in webhooks {
            let backoff = self.endpoints.get(&hook.id).copied().unwrap_or_default();
            if tick < backoff.next_attempt_tick {
                continue; // exponential backoff: not this endpoint's tick
            }
            let client = self.client.clone();
            let ingest = ingest.clone();
            let db_path = db_path.to_path_buf();
            let (budget, max_attempts, failures) =
                (self.endpoint_budget, self.max_attempts, backoff.failures);
            set.spawn(async move {
                let id = hook.id.clone();
                let outcome = match tokio::time::timeout(
                    budget,
                    dispatch_one(&db_path, &ingest, &client, &hook, failures, max_attempts),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(format!("endpoint budget {}s exceeded", budget.as_secs())),
                };
                (id, outcome)
            });
        }
        let mut delivered = 0usize;
        while let Some(joined) = set.join_next().await {
            let Ok((id, outcome)) = joined else {
                tracing::warn!("webhook endpoint task panicked; endpoint isolated");
                continue;
            };
            match outcome {
                Ok(outcome) => {
                    delivered += outcome.delivered;
                    self.record_outcome(&id, &outcome, tick);
                }
                Err(e) => {
                    tracing::warn!(webhook = %id, error = %e, "webhook dispatch failed; endpoint isolated");
                    self.record_failure(&id, tick);
                }
            }
        }
        Ok(delivered)
    }

    /// Backoff bookkeeping after a per-endpoint outcome: a clean tick resets
    /// the endpoint, a dead-lettering tick starts it fresh, a failing tick
    /// backs it off exponentially (1/2/4/8 ticks, capped).
    fn record_outcome(&mut self, id: &str, outcome: &DispatchOutcome, tick: u64) {
        if outcome.failed {
            if outcome.dead_lettered > 0 {
                self.endpoints.remove(id);
            } else {
                self.record_failure(id, tick);
            }
        } else {
            self.endpoints.remove(id);
        }
    }

    fn record_failure(&mut self, id: &str, tick: u64) {
        let failures = self.endpoints.get(id).map_or(0, |b| b.failures) + 1;
        let skip = (1u64 << failures.saturating_sub(1).min(3)).min(8);
        self.endpoints.insert(
            id.to_string(),
            EndpointBackoff {
                failures,
                next_attempt_tick: tick + skip,
            },
        );
    }
}

/// What one endpoint's tick did.
#[derive(Default)]
struct DispatchOutcome {
    delivered: usize,
    /// At least one delivery failed (drives backoff / dead-lettering).
    failed: bool,
    /// Events abandoned to the dead-letter table this tick.
    dead_lettered: u64,
}

/// Deliver one webhook's due events. On a failed delivery the cursor holds
/// at the last delivered event (the batch is retried under backoff); once
/// `failures + 1` reaches `max_attempts` the undelivered remainder is
/// dead-lettered and the cursor advances past it.
async fn dispatch_one(
    db_path: &Path,
    ingest: &IngestWriter,
    client: &reqwest::Client,
    hook: &WebhookRow,
    failures: u32,
    max_attempts: u32,
) -> Result<DispatchOutcome, String> {
    if let Err(reason) = validate_webhook_url(&hook.url) {
        tracing::warn!(webhook = %hook.id, error = %reason, "webhook URL fails the SSRF policy; skipping");
        return Ok(DispatchOutcome::default());
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
        return Ok(DispatchOutcome::default());
    }
    let wanted = |event: &serde_json::Value| {
        let name = event["event"].as_str().unwrap_or_default();
        hook.events.is_empty() || hook.events.iter().any(|e| e == name)
    };
    let batch_max = events
        .iter()
        .filter_map(|e| e["at_ns"].as_i64())
        .max()
        .unwrap_or(cursor);
    let mut outcome = DispatchOutcome::default();
    let mut delivered_up_to = cursor;
    let mut last_error = String::new();
    for event in &events {
        let at_ns = event["at_ns"].as_i64().unwrap_or(cursor);
        if !wanted(event) {
            delivered_up_to = delivered_up_to.max(at_ns);
            continue;
        }
        let body = serde_json::json!({
            "event": event["event"],
            "run_id": event["run_id"],
            "at_ns": at_ns,
            "actor": event["actor"],
            "detail": event["detail"],
        })
        .to_string();
        match post_signed(client, hook, &body).await {
            Ok(()) => {
                daemon_metrics::webhook_delivered();
                outcome.delivered += 1;
                delivered_up_to = delivered_up_to.max(at_ns);
            }
            Err(e) => {
                daemon_metrics::webhook_failed();
                outcome.failed = true;
                last_error = e;
                break; // the endpoint is likely down: retry the rest under backoff
            }
        }
    }
    let dead_letter = outcome.failed && failures + 1 >= max_attempts;
    let dead_letters: Vec<WebhookDeadLetter> = if dead_letter {
        events
            .iter()
            .filter(|e| e["at_ns"].as_i64().unwrap_or(cursor) > delivered_up_to)
            .filter(|e| wanted(e))
            .map(|e| WebhookDeadLetter {
                webhook_id: hook.id.clone(),
                run_id: e["run_id"].as_str().unwrap_or_default().to_string(),
                at_ns: e["at_ns"].as_i64().unwrap_or(cursor),
                event: e["event"].as_str().unwrap_or_default().to_string(),
                detail: e["detail"].as_str().map(str::to_string),
                actor: e["actor"].as_str().map(str::to_string),
                error: last_error.clone(),
                attempts: failures + 1,
                dead_at_ns: crate::now_ns(),
            })
            .collect()
    } else {
        Vec::new()
    };
    outcome.dead_lettered = dead_letters.len() as u64;
    let new_cursor = if dead_letter {
        batch_max
    } else {
        delivered_up_to
    };
    if outcome.dead_lettered > 0 {
        daemon_metrics::webhook_dead_lettered(outcome.dead_lettered);
        tracing::error!(
            webhook = %hook.id,
            dead_lettered = outcome.dead_lettered,
            attempts = failures + 1,
            "webhook deliveries permanently failed; events dead-lettered (replay from run_audit)"
        );
    }
    if new_cursor > cursor {
        let id = hook.id.clone();
        ingest
            .write(crate::Batch::Exec(Box::new(move |writer| {
                for letter in &dead_letters {
                    writer
                        .insert_webhook_dead_letter(letter)
                        .map_err(|e| e.to_string())?;
                }
                writer
                    .set_webhook_cursor(&id, new_cursor)
                    .map_err(|e| e.to_string())
            })))
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(outcome)
}

/// POST one signed payload; `Err(reason)` on rejection or transport failure
/// (already logged). Retries are the dispatcher's cross-tick backoff, not a
/// hot loop here.
///
/// Three headers ride along: `X-Tumult-Signature` stays the body-only HMAC
/// (receivers built before the timestamp scheme verify it unchanged), while
/// `X-Tumult-Timestamp` (unix seconds) and `X-Tumult-Signature-V2` (HMAC
/// over `"{timestamp}.{body}"`) are the additive replay protection —
/// receivers opt into freshness checks with
/// [`super::policy::verify_v2`].
async fn post_signed(
    client: &reqwest::Client,
    hook: &WebhookRow,
    body: &str,
) -> Result<(), String> {
    let timestamp_s = crate::now_ns() / 1_000_000_000;
    let signature = format!("sha256={}", hmac_sha256_hex(&hook.secret, body));
    let signature_v2 = format!(
        "sha256={}",
        hmac_sha256_hex(&hook.secret, &format!("{timestamp_s}.{body}"))
    );
    let result = client
        .post(&hook.url)
        .header("content-type", "application/json")
        .header("x-tumult-signature", &signature)
        .header("x-tumult-timestamp", timestamp_s)
        .header("x-tumult-signature-v2", &signature_v2)
        .body(body.to_string())
        .send()
        .await;
    match result {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) => {
            let reason = format!("rejected: HTTP {}", resp.status());
            tracing::warn!(webhook = %hook.id, status = %resp.status(), "webhook delivery rejected");
            Err(reason)
        }
        Err(e) => {
            tracing::warn!(webhook = %hook.id, error = %e, "webhook delivery failed");
            Err(e.to_string())
        }
    }
}
