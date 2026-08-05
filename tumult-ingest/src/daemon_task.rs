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
    use super::*;

    #[test]
    fn tick_from_env_parses_seconds_with_floor_and_fallback() {
        let key = "TUMULT_TEST_TICK_FROM_ENV_S";
        std::env::remove_var(key);
        assert_eq!(
            tick_from_env(key, Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        std::env::set_var(key, "5");
        assert_eq!(
            tick_from_env(key, Duration::from_secs(30)),
            Duration::from_secs(5)
        );
        std::env::set_var(key, "0");
        assert_eq!(
            tick_from_env(key, Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        std::env::set_var(key, "garbage");
        assert_eq!(
            tick_from_env(key, Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        std::env::remove_var(key);
    }

    #[tokio::test]
    async fn ticker_ticks_until_shutdown() {
        let ticks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let shutdown = CancellationToken::new();
        let counter = std::sync::Arc::clone(&ticks);
        let task = spawn_ticker(
            "test ticker",
            Duration::from_millis(10),
            shutdown.clone(),
            move || {
                let counter = std::sync::Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }
            },
        );
        tokio::time::sleep(Duration::from_millis(55)).await;
        shutdown.cancel();
        task.await.unwrap();
        assert!(
            ticks.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            "expected several ticks, got {}",
            ticks.load(std::sync::atomic::Ordering::SeqCst)
        );
    }
}
