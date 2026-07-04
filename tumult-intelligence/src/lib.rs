//! AI-assisted recommendation support shared by the Tumult CLI and MCP server.

mod agent;
mod context;
mod recommend;
mod render;
mod report;
mod types;
mod write;

#[cfg(test)]
mod model;

#[cfg(test)]
mod tests;

pub use agent::{build_agent_prompt, enhance, split_toon_blocks, AgentEnhancement, AgentOptions};
pub use recommend::{recommend, recommend_output, render};
pub use report::heuristic_report;
pub use types::{OutputFormat, RecommendOptions, RecommendationItem, RecommendationOutput};
pub use write::{
    json_with_agent, render_text_with_agent, write_validated_experiments, WriteOutcome,
};
