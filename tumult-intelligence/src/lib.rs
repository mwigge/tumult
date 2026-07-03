//! AI-assisted recommendation support shared by the Tumult CLI and MCP server.

#[path = "lib/types.rs"]
mod types;

#[path = "lib/report.rs"]
mod report;

#[path = "lib/context.rs"]
mod context;

#[path = "lib/render.rs"]
mod render;

#[path = "lib/recommend.rs"]
mod recommend;

#[cfg(test)]
#[path = "lib/model.rs"]
mod model;

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;

pub use recommend::recommend;
pub use report::heuristic_report;
pub use types::{OutputFormat, RecommendOptions, RecommendationItem, RecommendationOutput};
