//! Pure decision logic for the Tumult autopilot: propose → gate → enact.
//!
//! This crate is the **decision layer** of the 2.15 autopilot. It owns the
//! policy model, the anti-hollow validator, the deterministic safety gate,
//! and the earned-autonomy ladder — and nothing else. There is no database,
//! no runner, no clock, and no I/O beyond parsing policy TOML text handed
//! in by the caller. The engine (triggers, decision store, journal, MCP
//! surface) lives elsewhere and calls down into these pure functions,
//! mirroring how `tumult-graph` computes over data that `tumult-analytics`
//! owns.
//!
//! # The pipeline
//!
//! ```text
//! trigger ──> recommender ──> validator ──> safety gate ──> runner
//!                (candidate)   (report)      (verdict)
//! ```
//!
//! 1. The caller joins a recommendation with its playbook and context into a
//!    [`Candidate`], and snapshots the environment into an
//!    [`AmbientContext`].
//! 2. [`validate`] rejects hollow candidates (nothing falsifiable) and
//!    records enactability blockers (no playbook, no rollback, multi-fault).
//! 3. [`evaluate`] — the gate — turns policy + candidate + ambient +
//!    autonomy + validator report into a [`Verdict`]: `Enact`, `Downgrade`
//!    (would enact, but a bounded condition failed — queued for a human
//!    with the exact blockers named), `Propose` (never enact-eligible), or
//!    `Veto` (hard safety violation, with the rule that fired).
//! 4. The ladder ([`autonomy_earned`], [`record_outcome`], [`reset`]) makes
//!    autonomy something a fault class *earns* from clean enacted runs —
//!    or is granted up-front via operator pretrust — never something
//!    configured from hope.
//!
//! # The reproducibility contract
//!
//! Every verdict is bit-reproducible from `(policy hash, inputs)`:
//!
//! * the policy hash ([`policy_hash`]) covers the raw TOML text, so any
//!   edit to the file is a new policy version;
//! * the gate reads no wall clock — "now"-shaped facts (business hours,
//!   cooldown age, runs today) are frozen into [`AmbientContext`] by the
//!   caller before evaluation;
//! * all rules are always evaluated in one fixed order, and
//!   [`GateDecision::rules_evaluated`] lists every rule with its outcome —
//!   that vector *is* the audit record.
//!
//! The replay corpus under `tests/corpus/` exercises exactly this contract
//! and is the seed of the offline scoring harness: any change to policy or
//! gate logic replays the corpus and surfaces verdict flips.

pub mod candidate;
pub mod gate;
pub mod ladder;
pub mod policy;
pub mod validator;

pub use candidate::{AmbientContext, AutonomyRecord, Candidate, ConfidenceTier, Trigger};
pub use gate::{evaluate, GateDecision, Verdict, RULE_ORDER};
pub use ladder::{autonomy_earned, class_key, record_outcome, reset};
pub use policy::{
    policy_hash, AutopilotPolicy, LoadedPolicy, PlaybookEntry, PolicyError, PretrustEntry,
};
pub use validator::{validate, ValidatorReport};
