//! Background-task scaffolding shared by the daemon's periodic supervisors
//! (schedule scheduler, webhook dispatcher, GameDay supervisor): one
//! interval/select/log loop, so the shutdown contract — cancel the token,
//! await the task so its `IngestWriter` clone drops before the writer
//! drain — is honored by construction rather than by copying.

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// A tick interval from an env var of seconds (minimum 1s); unset or
/// invalid values fall back to `default`.
#[must_use]
pub fn tick_from_env(key: &str, default: Duration) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or(default, Duration::from_secs)
}

/// Spawn a periodic task: every `tick` run `on_tick`, logging failures,
/// until `shutdown` is cancelled. The task owns whatever `on_tick`
/// captures (typically an `IngestWriter` clone), so the daemon must cancel
/// the token and await the returned handle before draining the writer
/// channel (same contract as the lake scheduler).
pub fn spawn_ticker<F, Fut>(
    name: &'static str,
    tick: Duration,
    shutdown: CancellationToken,
    mut on_tick: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = on_tick().await {
                        tracing::warn!(error = %e, "{name} tick failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("{name} exiting (shutdown)");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tumult_test_utils::EnvGuard;

    use super::*;

    #[test]
    fn tick_from_env_parses_seconds_with_floor_and_fallback() {
        let key = "TUMULT_TEST_TICK_FROM_ENV_S";
        let fallback = Duration::from_secs(30);
        let mut env = EnvGuard::lock();

        env.remove(key);
        assert_eq!(tick_from_env(key, fallback), fallback);
        env.set(key, "5");
        assert_eq!(tick_from_env(key, fallback), Duration::from_secs(5));
        env.set(key, " 7 ");
        assert_eq!(tick_from_env(key, fallback), Duration::from_secs(7));
        env.set(key, "0");
        assert_eq!(tick_from_env(key, fallback), fallback);
        env.set(key, "garbage");
        assert_eq!(tick_from_env(key, fallback), fallback);
    }

    /// A ticker that counts its ticks and fails every one of them.
    fn failing_ticker(
        shutdown: &CancellationToken,
    ) -> (Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ticks);
        let task = spawn_ticker(
            "test ticker",
            Duration::from_millis(10),
            shutdown.clone(),
            move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err("tick failed".to_string())
                }
            },
        );
        (ticks, task)
    }

    #[tokio::test(start_paused = true)]
    async fn ticker_ticks_once_per_interval_until_shutdown() {
        let shutdown = CancellationToken::new();
        let (ticks, task) = failing_ticker(&shutdown);

        // Ticks land at 0, 10, 20, 30, 40 and 50 ms.
        tokio::time::sleep(Duration::from_millis(55)).await;
        assert_eq!(
            ticks.load(Ordering::SeqCst),
            6,
            "a failed tick does not stop the ticker"
        );

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the ticker ends on shutdown")
            .unwrap();
        assert_eq!(ticks.load(Ordering::SeqCst), 6, "no tick after shutdown");
    }
}
