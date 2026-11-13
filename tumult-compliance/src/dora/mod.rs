//! DORA coverage and maturity scores.
//!
//! Two scores are computed from the same inputs:
//!
//! - **Coverage** ([`coverage`]): per article, how much of what the
//!   requirement registry asks for is backed by fresh evidence, always
//!   reported with the share of rows that cannot be observed.
//! - **Maturity** ([`maturity`], [`rollup`]): an internal 1–5 level from
//!   five pillars, gated per level, computed per node and rolled up the
//!   organisation with the weakest critical child capping its parent.
//!
//! [`status`] turns coverage into the evidence status of each requirement
//! (designed, existing, operating). Every tunable number lives in
//! [`params::HouseParams`]: those are house choices, not regulation.

pub mod coverage;
pub mod loader;
pub mod maturity;
pub mod model;
pub mod params;
pub mod register;
pub mod rollup;
pub mod status;
#[cfg(test)]
mod worked;

pub use params::HouseParams;
