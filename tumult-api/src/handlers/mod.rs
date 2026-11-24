//! HTTP handlers for the read-only query API, one module per domain.

pub mod conformance;
pub mod dora;
pub mod dr;
pub mod entities;
pub(crate) mod evidence;
pub mod experiments;
pub mod gate;
pub mod logs;
pub mod metrics;
pub mod overview;
pub mod perf;
pub mod recommend;
pub mod reports;
pub mod risks;
pub mod starmap;
pub(crate) mod starmap_cache;
pub mod topology;
pub mod traces;
