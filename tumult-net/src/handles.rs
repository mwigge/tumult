//! Live, retunable fault knobs shared into the proxy adapter stack.
//!
//! Each handle is an `Arc<Dynamic*>` from `tokio-netem`. Cloning a handle is a
//! cheap atomic refcount bump, and calling `set` on it retunes every proxy pipe
//! that holds a clone — without tearing down live connections.

use std::sync::Arc;
use std::time::Duration;

use tokio_netem::delayer::DynamicDuration;
use tokio_netem::probability::DynamicProbability;
use tokio_netem::slicer::DynamicSize;
use tokio_netem::throttler::DynamicRate;

use crate::config::FaultProfile;
use crate::error::NetError;

/// The four runtime-adjustable fault knobs feeding one proxy's adapter stacks.
#[derive(Clone)]
pub struct FaultHandles {
    /// Per-write latency (a value of [`Duration::ZERO`] disables delay).
    pub delay: Arc<DynamicDuration>,
    /// Egress rate limit in bytes/second (`0` = unlimited).
    pub rate: Arc<DynamicRate>,
    /// Write-slice size in bytes (`0` = no slicing).
    pub slice: Arc<DynamicSize>,
    /// Per-byte corruption probability, `0.0..=1.0`.
    pub corrupt: Arc<DynamicProbability>,
}

impl FaultHandles {
    /// Build a handle set from a validated [`FaultProfile`].
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] if the profile is invalid or the corruption
    /// probability is rejected by `tokio-netem`.
    pub fn from_profile(profile: &FaultProfile) -> Result<Self, NetError> {
        profile.validate()?;
        Ok(Self {
            delay: DynamicDuration::new(profile.effective_delay()),
            rate: DynamicRate::new(profile.rate_bps),
            slice: DynamicSize::new(profile.slice_bytes),
            corrupt: DynamicProbability::new(profile.corrupt_prob)?,
        })
    }

    /// Retune the latency knob for every pipe holding a clone.
    pub fn set_delay(&self, delay: Duration) {
        self.delay.set(delay);
    }

    /// Retune the egress rate limit (bytes/second; `0` disables throttling).
    pub fn set_rate(&self, rate_bps: usize) {
        self.rate.set(rate_bps);
    }

    /// Retune the write-slice size (`0` disables slicing).
    pub fn set_slice(&self, slice_bytes: usize) {
        self.slice.set(slice_bytes);
    }

    /// Retune the corruption probability.
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] if `probability` is outside `0.0..=1.0`.
    pub fn set_corrupt(&self, probability: f64) -> Result<(), NetError> {
        self.corrupt.set(probability)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_profile_builds_and_retunes() {
        let profile = FaultProfile {
            delay_ms: 5,
            rate_bps: 2048,
            slice_bytes: 32,
            corrupt_prob: 0.0,
            ..FaultProfile::default()
        };
        let h = FaultHandles::from_profile(&profile).expect("handles");
        h.set_delay(Duration::from_millis(10));
        h.set_rate(4096);
        h.set_slice(16);
        assert!(h.set_corrupt(0.5).is_ok());
        assert!(h.set_corrupt(2.0).is_err());
    }
}
