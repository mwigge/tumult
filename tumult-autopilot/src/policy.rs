//! Autopilot policy: the operator's contract with the gate, parsed from
//! TOML (`autopilot.toml`) and hashed for reproducibility.
//!
//! Everything the gate may ever allow is bounded here, and a missing key
//! only ever *narrows* what is allowed: disabled by default, guard required
//! by default, no tier enact-eligible by default. Unknown keys are rejected
//! rather than ignored because in a safety policy a typo must fail loudly,
//! not deserialise into something more permissive.
//!
//! [`policy_hash`] — sha256 of the raw TOML *text*, not of the parsed
//! struct — is the reproducibility anchor: formatting and comments count,
//! so any edit to the file is a new policy version and every stored
//! decision can name exactly the policy that produced it. [`LoadedPolicy`]
//! carries the text, the parse result and the hash together so callers
//! persist all three with each decision.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The whole policy file: an `[autopilot]` table plus its sub-tables.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyDoc {
    #[serde(default)]
    autopilot: AutopilotPolicy,
}

/// The `[autopilot]` policy table. Limits are *enforced* by
/// [`crate::gate::evaluate`]; this type only carries them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutopilotPolicy {
    /// Master switch; `false` (the default) vetoes everything.
    #[serde(default)]
    pub enabled: bool,
    /// Hard daily run budget across all services; exhaustion is a veto.
    #[serde(default = "default_max_runs_per_day")]
    pub max_runs_per_day: u32,
    /// Per-service cooldown between autopilot runs, in hours.
    #[serde(default = "default_cooldown_hours")]
    pub cooldown_hours: u32,
    /// Default evidence freshness window, in days.
    #[serde(default = "default_evidence_ttl_days")]
    pub evidence_ttl_days: u32,
    /// Tiers where enact is ever allowed; anything else caps at propose.
    /// Free-form strings — they only need to match the topology's tier tags,
    /// so no vocabulary is validated here.
    #[serde(default)]
    pub enact_tiers: Vec<String>,
    /// When `true` (the default), an experiment without a guard caps at
    /// propose.
    #[serde(default = "default_true")]
    pub require_guard: bool,
    /// When `true`, enact is allowed only within business hours. The caller
    /// decides what those are — the gate never reads a clock.
    #[serde(default)]
    pub business_hours_only: bool,
    /// Precision a fault class needs to graduate to enact.
    #[serde(default = "default_autonomy_threshold")]
    pub autonomy_threshold: f64,
    /// Minimum enacted samples before precision means anything.
    #[serde(default = "default_autonomy_min_samples")]
    pub autonomy_min_samples: u32,
    /// Operator-granted initial trust; each entry names a fault class
    /// *exactly* — see [`AutopilotPolicy::is_pretrusted`].
    #[serde(default)]
    pub pretrusted: Vec<PretrustEntry>,
    /// Recommendation → experiment bindings; matched on
    /// `(plugin, action[, service])` by [`AutopilotPolicy::playbook_for`].
    /// Experiment paths are resolved by the engine, not validated here.
    #[serde(default)]
    pub playbook: Vec<PlaybookEntry>,
    /// Per-framework TTL overrides; key is the framework name (`DORA`, …).
    #[serde(default)]
    pub evidence_ttl_days_by_framework: BTreeMap<String, u32>,
}

/// One operator-granted trust bootstrap for a fault class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PretrustEntry {
    /// Plugin of the trusted class (e.g. `tumult-postgres`).
    pub plugin: String,
    /// Action of the trusted class (e.g. `kill-connections`).
    pub action: String,
    /// Tier of the trusted class — required, because pretrust names a class
    /// exactly and a class without a tier is a different class.
    pub tier: String,
}

/// One recommendation → experiment binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybookEntry {
    /// Plugin the binding applies to.
    pub plugin: String,
    /// Action the binding applies to.
    pub action: String,
    /// Optional service restriction; a service-specific entry beats a
    /// generic one for that service.
    #[serde(default)]
    pub service: Option<String>,
    /// Path of the experiment to run (resolved and read by the engine).
    pub experiment: String,
}

impl Default for AutopilotPolicy {
    /// The all-defaults policy: disabled and maximally conservative — the
    /// same shape an empty TOML document parses to.
    fn default() -> Self {
        Self {
            enabled: false,
            max_runs_per_day: default_max_runs_per_day(),
            cooldown_hours: default_cooldown_hours(),
            evidence_ttl_days: default_evidence_ttl_days(),
            enact_tiers: Vec::new(),
            require_guard: default_true(),
            business_hours_only: false,
            autonomy_threshold: default_autonomy_threshold(),
            autonomy_min_samples: default_autonomy_min_samples(),
            pretrusted: Vec::new(),
            playbook: Vec::new(),
            evidence_ttl_days_by_framework: BTreeMap::new(),
        }
    }
}

impl AutopilotPolicy {
    /// Whether the operator pretrusted this exact fault class.
    ///
    /// The match is exact on `(plugin, action, tier)`: a candidate without a
    /// tier never matches, because widening pretrust to "any tier" would
    /// grant autonomy the operator did not spell out.
    #[must_use]
    pub fn is_pretrusted(&self, plugin: &str, action: &str, tier: Option<&str>) -> bool {
        tier.is_some_and(|t| {
            self.pretrusted
                .iter()
                .any(|p| p.plugin == plugin && p.action == action && p.tier == t)
        })
    }

    /// Whether `tier` is one where enact is ever allowed. A candidate with
    /// no tier is never enact-eligible — an unknown blast radius cannot be
    /// bounded.
    #[must_use]
    pub fn tier_allows_enact(&self, tier: Option<&str>) -> bool {
        tier.is_some_and(|t| self.enact_tiers.iter().any(|allowed| allowed == t))
    }

    /// Resolve the playbook for `(plugin, action, service)`: a
    /// service-specific entry wins over a generic (`service`-less) one;
    /// among equally specific entries the first in file order wins, so
    /// resolution is deterministic for a fixed policy text.
    #[must_use]
    pub fn playbook_for(
        &self,
        plugin: &str,
        action: &str,
        service: Option<&str>,
    ) -> Option<&PlaybookEntry> {
        let mut generic = None;
        for entry in self
            .playbook
            .iter()
            .filter(|p| p.plugin == plugin && p.action == action)
        {
            match (entry.service.as_deref(), service) {
                (Some(bound), Some(wanted)) if bound == wanted => return Some(entry),
                (None, _) => generic = generic.or(Some(entry)),
                _ => {}
            }
        }
        generic
    }

    /// Evidence TTL for a compliance framework: the per-framework override
    /// when present, else the policy default.
    #[must_use]
    pub fn evidence_ttl_days_for(&self, framework: &str) -> u32 {
        self.evidence_ttl_days_by_framework
            .get(framework)
            .copied()
            .unwrap_or(self.evidence_ttl_days)
    }
}

/// A parsed policy together with the exact text and hash that produced it,
/// so callers persist all three with every decision row.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedPolicy {
    /// The validated policy.
    pub policy: AutopilotPolicy,
    /// The raw TOML text, byte for byte as handed in.
    pub raw_toml: String,
    /// Lowercase sha256 hex of `raw_toml` — the reproducibility anchor.
    pub hash: String,
}

impl LoadedPolicy {
    /// Parse and validate a policy TOML document, capturing text + hash.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError`] when the TOML fails to parse (including
    /// unknown keys — see the module docs for why those are fatal) or a
    /// value is out of its safe range.
    pub fn parse(toml_text: &str) -> Result<Self, PolicyError> {
        let doc: PolicyDoc =
            toml::from_str(toml_text).map_err(|err| PolicyError::Parse(err.to_string()))?;
        validate_policy(&doc.autopilot)?;
        Ok(Self {
            policy: doc.autopilot,
            raw_toml: toml_text.to_string(),
            hash: policy_hash(toml_text),
        })
    }

    /// The sha256 hex of the raw policy text.
    #[must_use]
    pub fn policy_hash(&self) -> &str {
        &self.hash
    }
}

/// Range/shape checks that TOML types cannot express.
fn validate_policy(policy: &AutopilotPolicy) -> Result<(), PolicyError> {
    if !policy.autonomy_threshold.is_finite() || !(0.0..=1.0).contains(&policy.autonomy_threshold) {
        return Err(PolicyError::ThresholdOutOfRange(policy.autonomy_threshold));
    }
    // Zero would let a class graduate to enact with no evidence at all,
    // which contradicts "autonomy is earned".
    if policy.autonomy_min_samples == 0 {
        return Err(PolicyError::ZeroMinSamples);
    }
    for (index, entry) in policy.pretrusted.iter().enumerate() {
        if entry.plugin.is_empty() || entry.action.is_empty() || entry.tier.is_empty() {
            return Err(PolicyError::IncompletePretrust(index));
        }
    }
    for (index, entry) in policy.playbook.iter().enumerate() {
        if entry.plugin.is_empty() || entry.action.is_empty() || entry.experiment.is_empty() {
            return Err(PolicyError::IncompletePlaybook(index));
        }
    }
    Ok(())
}

/// Why a policy document was rejected.
#[derive(Debug)]
pub enum PolicyError {
    /// The TOML failed to parse (syntax error or unknown key).
    Parse(String),
    /// `autonomy_threshold` is not a finite value in `0.0..=1.0`.
    ThresholdOutOfRange(f64),
    /// `autonomy_min_samples` is zero — autonomy would be free.
    ZeroMinSamples,
    /// The `[[autopilot.pretrusted]]` entry at this index has an empty field.
    IncompletePretrust(usize),
    /// The `[[autopilot.playbook]]` entry at this index has an empty field.
    IncompletePlaybook(usize),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "autopilot policy TOML parse error: {err}"),
            Self::ThresholdOutOfRange(value) => {
                write!(f, "autonomy_threshold {value} must be within 0.0..=1.0")
            }
            Self::ZeroMinSamples => {
                write!(f, "autonomy_min_samples must be at least 1")
            }
            Self::IncompletePretrust(index) => {
                write!(
                    f,
                    "pretrusted entry {index} has an empty plugin/action/tier"
                )
            }
            Self::IncompletePlaybook(index) => write!(
                f,
                "playbook entry {index} has an empty plugin/action/experiment"
            ),
        }
    }
}

impl std::error::Error for PolicyError {}

/// Lowercase sha256 hex of the raw policy text — the reproducibility anchor
/// stored with every decision. Hashing text (not the parsed struct) means
/// comments and formatting count, so *any* edit is a new policy version.
#[must_use]
pub fn policy_hash(toml_text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(toml_text.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn default_max_runs_per_day() -> u32 {
    6
}
fn default_cooldown_hours() -> u32 {
    12
}
fn default_evidence_ttl_days() -> u32 {
    90
}
fn default_true() -> bool {
    true
}
fn default_autonomy_threshold() -> f64 {
    0.8
}
fn default_autonomy_min_samples() -> u32 {
    3
}

// Behaviour tests live in `tests/policy.rs`; only the hash primitive is
// tested here because its hex encoding is hand-rolled.
#[cfg(test)]
mod tests {
    use super::policy_hash;

    #[test]
    fn hash_of_empty_text_matches_the_known_sha256_vector() {
        assert_eq!(
            policy_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn any_textual_edit_changes_the_hash() {
        let a = policy_hash("[autopilot]\nenabled = true\n");
        let b = policy_hash("[autopilot]\nenabled = true # reviewed\n");
        assert_ne!(a, b);
        assert_eq!(a, policy_hash("[autopilot]\nenabled = true\n"));
    }
}
