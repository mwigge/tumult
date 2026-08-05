//! Daemon self-observability: process-wide counters and gauges rendered as
//! Prometheus text by `GET /metrics`.
//!
//! Hand-rolled atomics — the workspace carries no metrics facade and the
//! daemon SLIs are a handful of counters, so a new dependency is not
//! justified. Instrumentation lives at the source (run worker, webhook
//! dispatcher, schedule scheduler, GameDay supervisor); tumultd's ops
//! router only renders.

use std::sync::atomic::{AtomicU64, Ordering};

static RUNS_STARTED: AtomicU64 = AtomicU64::new(0);
static RUNS_COMPLETED: AtomicU64 = AtomicU64::new(0);
static RUNS_FAILED: AtomicU64 = AtomicU64::new(0);
static WEBHOOK_DELIVERED: AtomicU64 = AtomicU64::new(0);
static WEBHOOK_FAILED: AtomicU64 = AtomicU64::new(0);
static WEBHOOK_DEAD_LETTERED: AtomicU64 = AtomicU64::new(0);
static SCHEDULE_FIRES: AtomicU64 = AtomicU64::new(0);
static ACTIVE_CAMPAIGNS: AtomicU64 = AtomicU64::new(0);
static SUPERVISOR_TICK_NS: AtomicU64 = AtomicU64::new(0);

fn inc(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// A run began execution (the worker stamped `started`).
pub fn run_started() {
    inc(&RUNS_STARTED);
}

/// A run reached a completed terminal state (passed/deviated/aborted with a
/// journal).
pub fn run_completed() {
    inc(&RUNS_COMPLETED);
}

/// A run failed (validation, dispatch refusal, or the runner errored).
pub fn run_failed() {
    inc(&RUNS_FAILED);
}

/// One webhook event delivered successfully.
pub fn webhook_delivered() {
    inc(&WEBHOOK_DELIVERED);
}

/// One webhook event delivery failed (the event is retried next tick).
pub fn webhook_failed() {
    inc(&WEBHOOK_FAILED);
}

/// `n` webhook events moved to the dead-letter table (permanent loss,
/// recorded in `webhook_dead_letters`).
pub fn webhook_dead_lettered(n: u64) {
    WEBHOOK_DEAD_LETTERED.fetch_add(n, Ordering::Relaxed);
}

/// A schedule fired a run.
pub fn schedule_fired() {
    inc(&SCHEDULE_FIRES);
}

/// Snapshot of campaigns currently advancing (gauge, set each supervisor
/// tick).
pub fn set_active_campaigns(n: u64) {
    ACTIVE_CAMPAIGNS.store(n, Ordering::Relaxed);
}

/// Heartbeat: a supervisor tick ran. Readiness requires this to be non-zero
/// (a supervisor whose task died stops ticking).
pub fn supervisor_tick() {
    SUPERVISOR_TICK_NS.store(crate::now_ns() as u64, Ordering::Relaxed);
}

/// The last supervisor heartbeat (0 = no tick since boot).
#[must_use]
pub fn supervisor_last_tick_ns() -> u64 {
    SUPERVISOR_TICK_NS.load(Ordering::Relaxed)
}

fn push_counter(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
    ));
}

fn push_gauge(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n"
    ));
}

/// Render every daemon metric in Prometheus text exposition format.
#[must_use]
pub fn render_prometheus() -> String {
    let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
    let mut out = String::new();
    push_counter(
        &mut out,
        "tumultd_runs_started_total",
        "Runs that began execution.",
        load(&RUNS_STARTED),
    );
    push_counter(
        &mut out,
        "tumultd_runs_completed_total",
        "Runs that reached a completed terminal state.",
        load(&RUNS_COMPLETED),
    );
    push_counter(
        &mut out,
        "tumultd_runs_failed_total",
        "Runs that failed.",
        load(&RUNS_FAILED),
    );
    push_counter(
        &mut out,
        "tumultd_webhook_deliveries_succeeded_total",
        "Webhook events delivered successfully.",
        load(&WEBHOOK_DELIVERED),
    );
    push_counter(
        &mut out,
        "tumultd_webhook_deliveries_failed_total",
        "Webhook event deliveries that failed (retried until dead-lettered).",
        load(&WEBHOOK_FAILED),
    );
    push_counter(
        &mut out,
        "tumultd_webhook_dead_letters_total",
        "Webhook events abandoned to the dead-letter table.",
        load(&WEBHOOK_DEAD_LETTERED),
    );
    push_counter(
        &mut out,
        "tumultd_schedule_fires_total",
        "Schedule fires.",
        load(&SCHEDULE_FIRES),
    );
    push_gauge(
        &mut out,
        "tumultd_active_campaigns",
        "GameDay campaigns currently advancing.",
        load(&ACTIVE_CAMPAIGNS),
    );
    push_gauge(
        &mut out,
        "tumultd_supervisor_last_tick_ns",
        "Last supervisor tick (heartbeat), epoch nanoseconds.",
        load(&SUPERVISOR_TICK_NS),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_emits_prometheus_text_for_every_metric() {
        webhook_delivered();
        webhook_dead_lettered(2);
        set_active_campaigns(3);
        supervisor_tick();
        let text = render_prometheus();
        for name in [
            "tumultd_runs_started_total",
            "tumultd_runs_completed_total",
            "tumultd_runs_failed_total",
            "tumultd_webhook_deliveries_succeeded_total",
            "tumultd_webhook_deliveries_failed_total",
            "tumultd_webhook_dead_letters_total",
            "tumultd_schedule_fires_total",
            "tumultd_active_campaigns",
            "tumultd_supervisor_last_tick_ns",
        ] {
            assert!(text.contains(&format!("# TYPE {name}")), "{name}: {text}");
        }
        assert!(text.contains("tumultd_active_campaigns 3"), "{text}");
        assert!(
            text.contains("tumultd_webhook_dead_letters_total 2"),
            "{text}"
        );
        assert!(supervisor_last_tick_ns() > 0);
    }
}
