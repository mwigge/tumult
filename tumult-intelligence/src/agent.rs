//! Agent-CLI enhancement of the heuristic recommendations.
//!
//! Builds a single self-contained prompt from the deterministic heuristic
//! output, the plugin/action catalog, and the journal-derived signals, then
//! runs it through a [`tumult_agent_cli`] adapter (Claude Code, Codex, ...)
//! and splits the response back into recommendation text plus zero or more
//! proposed `.toon` experiment documents.
//!
//! This module is UI-free: it returns structured data and never prints.
//! Validation and file writing are the caller's responsibility (the CLI
//! gates every proposed experiment through
//! `tumult_core::engine::validate_experiment` before writing).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use tumult_agent_cli::{run_prompt, AgentCliAdapter, AgentCliError, PromptRequest};

use crate::render::render_text;
use crate::report::plugin_catalog;
use crate::sanitise;
use crate::types::RecommendationOutput;

/// Compact `.toon` experiment example embedded in the prompt so the model
/// mirrors the real experiment format (modeled on `examples/redis-chaos.toon`).
const TOON_FORMAT_EXAMPLE: &str = r#"title: Redis resilience — verify recovery after disruption
description: Check Redis is alive, inject a disruption, confirm it recovers

tags[2]: redis, resilience

steady_state_hypothesis:
  title: Redis responds to ping
  probes[1]:
    - name: redis-ping
      activity_type: probe
      provider:
        type: process
        path: sh
        arguments[2]: "-c", "docker exec docker-redis-1 redis-cli ping"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "PONG"

method[1]:
  - name: redis-restart
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "docker restart docker-redis-1"
      timeout_s: 30.0

rollbacks[0]:"#;

/// Options for one agent enhancement run.
#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// Optional model override passed through to the agent CLI.
    pub model: Option<String>,
    /// Deadline for the agent CLI subprocess.
    pub timeout: Duration,
    /// When true, the prompt asks for complete `.toon` experiment documents
    /// in `toon`-tagged fences alongside the enhanced recommendations.
    pub generate_experiments: bool,
    /// Working directory for the agent CLI subprocess. An empty path means
    /// the current directory.
    pub workspace: PathBuf,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            model: None,
            timeout: tumult_agent_cli::adapter::DEFAULT_TIMEOUT,
            generate_experiments: false,
            workspace: PathBuf::new(),
        }
    }
}

/// Structured result of an agent enhancement run.
#[derive(Debug, Clone)]
pub struct AgentEnhancement {
    /// Adapter name the response came from (e.g. `claude-code`).
    pub adapter: String,
    /// Model override used, when any.
    pub model: Option<String>,
    /// The agent's enhanced/re-ranked recommendation text (everything
    /// outside `toon`-tagged fences).
    pub recommendations: String,
    /// Raw contents of each `toon`-tagged fence, in response order. Unvalidated:
    /// callers must gate each through experiment parsing + validation.
    pub experiments: Vec<String>,
}

/// Assemble the self-contained enhancement prompt.
///
/// The prompt carries (a) the deterministic heuristic recommendations,
/// (b) the journal-derived signals (coverage, failing and stale
/// experiments), and (c) the plugin/action catalog so the model proposes
/// only real actions. It ends with a strict response envelope —
/// recommendations section first, then zero or more `toon`-tagged fences — so the
/// response can be split mechanically.
#[must_use]
pub fn build_agent_prompt(
    heuristic: &RecommendationOutput,
    plugin_catalog: &str,
    generate_experiments: bool,
) -> String {
    // Operator- and journal-controlled fields are sanitised (invisible and
    // control characters stripped, length capped — prompt-injection hygiene)
    // before ANY interpolation: the goal is rendered both standalone and via
    // `render_text`, so sanitising a cloned output covers every site at once.
    let mut sanitised = heuristic.clone();
    sanitised.goal = sanitised.goal.as_deref().map(sanitise::goal);
    sanitised.heuristic_context = sanitise::journal_context(&heuristic.heuristic_context);
    let heuristic = &sanitised;

    let mut prompt = String::new();
    writeln!(
        prompt,
        "You are a chaos engineering advisor for Tumult, a Rust-native chaos \
         engineering platform. Below are deterministic heuristic \
         recommendations for the next chaos experiments, the signals they \
         were derived from, and the catalog of plugins and actions that \
         actually exist in this installation."
    )
    .ok();
    if let Some(goal) = &heuristic.goal {
        writeln!(prompt).ok();
        writeln!(prompt, "Operator goal: {goal}").ok();
    }

    writeln!(prompt).ok();
    writeln!(prompt, "## Heuristic recommendations (deterministic)").ok();
    writeln!(prompt).ok();
    writeln!(prompt, "{}", render_text(heuristic).trim_end()).ok();

    writeln!(prompt).ok();
    writeln!(prompt, "## Journal signals").ok();
    writeln!(prompt).ok();
    writeln!(prompt, "{}", heuristic.heuristic_context.trim_end()).ok();

    writeln!(prompt).ok();
    writeln!(prompt, "## Plugin catalog").ok();
    writeln!(prompt).ok();
    writeln!(
        prompt,
        "These are the ONLY plugins, actions, and probes available. Never \
         reference a plugin or action that is not listed here."
    )
    .ok();
    writeln!(prompt).ok();
    writeln!(prompt, "{}", plugin_catalog.trim_end()).ok();

    writeln!(prompt).ok();
    writeln!(prompt, "## Your task").ok();
    writeln!(prompt).ok();
    writeln!(
        prompt,
        "Enhance and re-rank the heuristic recommendations. For each \
         recommendation explain the reasoning: why it ranks where it does \
         given the coverage gaps, failure history, and staleness signals \
         above, what could go wrong, and what running it would teach."
    )
    .ok();
    if generate_experiments {
        writeln!(prompt).ok();
        writeln!(
            prompt,
            "For each proposed experiment, also emit a complete Tumult \
             experiment document in the TOON format, following exactly the \
             structure of this example:"
        )
        .ok();
        writeln!(prompt).ok();
        writeln!(prompt, "```toon\n{TOON_FORMAT_EXAMPLE}\n```").ok();
        writeln!(prompt).ok();
        writeln!(
            prompt,
            "Every experiment must have a unique, descriptive title, a \
             non-empty method, and use only plugins/actions from the catalog \
             or `type: process` providers."
        )
        .ok();
    }

    writeln!(prompt).ok();
    writeln!(prompt, "## Response format (strict)").ok();
    writeln!(prompt).ok();
    writeln!(
        prompt,
        "Respond with exactly the following, in this order, and nothing else:"
    )
    .ok();
    writeln!(
        prompt,
        "1. A section starting with the line `## Recommendations`, \
         containing the enhanced, re-ranked recommendations as a numbered \
         list with reasoning."
    )
    .ok();
    if generate_experiments {
        writeln!(
            prompt,
            "2. Zero or more fenced code blocks tagged `toon` (```toon ... \
             ```), each containing one complete experiment document. Do not \
             emit fenced code blocks with any other tag, and do not put \
             anything but the experiment document inside a fence."
        )
        .ok();
    } else {
        writeln!(
            prompt,
            "2. No fenced code blocks: do not emit experiment documents."
        )
        .ok();
    }
    prompt
}

/// Run the heuristic output through an agent CLI and split the response.
///
/// Builds the prompt via [`build_agent_prompt`] (attaching the live plugin
/// catalog), executes it with [`tumult_agent_cli::run_prompt`], and splits
/// the answer into recommendation text plus raw `toon`-fenced blocks.
///
/// # Errors
///
/// Propagates any [`AgentCliError`] from detection, invocation, or output
/// parsing (missing binary, auth failure, timeout, malformed CLI output).
pub fn enhance(
    heuristic: &RecommendationOutput,
    adapter: &dyn AgentCliAdapter,
    options: &AgentOptions,
) -> Result<AgentEnhancement, AgentCliError> {
    let catalog = plugin_catalog();
    let prompt = build_agent_prompt(heuristic, &catalog, options.generate_experiments);

    let mut request = PromptRequest::new(prompt, options.workspace.clone());
    request.model.clone_from(&options.model);
    request.timeout = options.timeout;

    let response = run_prompt(adapter, &request)?;
    let (recommendations, experiments) = split_toon_blocks(&response);
    Ok(AgentEnhancement {
        adapter: adapter.name().to_string(),
        model: options.model.clone(),
        recommendations,
        experiments,
    })
}

/// Split an agent response into recommendation text and `toon`-fenced blocks.
///
/// Everything outside `toon`-tagged fences (including fenced blocks with other
/// tags) stays in the recommendation text. An unterminated `toon` fence is
/// handled gracefully: its partial content is still returned as a block so
/// the caller's validation gate can reject it explicitly instead of it
/// vanishing silently.
#[must_use]
pub fn split_toon_blocks(response: &str) -> (String, Vec<String>) {
    let mut recommendations = String::new();
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;

    for line in response.lines() {
        let trimmed = line.trim();
        if let Some(block) = current.as_mut() {
            if trimmed == "```" {
                blocks.push(std::mem::take(block));
                current = None;
            } else {
                block.push_str(line);
                block.push('\n');
            }
        } else if trimmed == "```toon" {
            current = Some(String::new());
        } else {
            recommendations.push_str(line);
            recommendations.push('\n');
        }
    }
    // Unterminated fence: keep the partial block (see doc comment).
    if let Some(block) = current {
        blocks.push(block);
    }

    (recommendations.trim().to_string(), blocks)
}

#[cfg(test)]
mod tests {
    use super::{build_agent_prompt, split_toon_blocks};
    use crate::types::{RecommendationItem, RecommendationOutput};

    fn heuristic_fixture() -> RecommendationOutput {
        RecommendationOutput {
            source: "heuristic-fallback".to_string(),
            model: None,
            goal: Some("harden the cache tier".to_string()),
            recommendations: vec![RecommendationItem {
                rank: 1,
                title: "Exercise redis restart recovery".to_string(),
                rationale: "redis actions are untested".to_string(),
                plugins: vec!["tumult-redis".to_string()],
                actions: vec!["restart".to_string()],
                preconditions: vec!["staging only".to_string()],
                expected_learning: Some("recovery time".to_string()),
            }],
            draft_toon: None,
            draft_valid: None,
            draft_validation_error: None,
            notes: vec![],
            heuristic_context: "Coverage: 3/64 actions tested (5%)".to_string(),
        }
    }

    #[test]
    fn prompt_includes_heuristics_catalog_and_signals() {
        let catalog = "plugin: tumult-redis\n  actions:\n    - restart: restart the server\n";
        let prompt = build_agent_prompt(&heuristic_fixture(), catalog, false);

        assert!(prompt.contains("Exercise redis restart recovery"));
        assert!(prompt.contains("redis actions are untested"));
        assert!(prompt.contains("plugin: tumult-redis"));
        assert!(prompt.contains("restart: restart the server"));
        assert!(prompt.contains("Coverage: 3/64 actions tested (5%)"));
        assert!(prompt.contains("Operator goal: harden the cache tier"));
        assert!(prompt.contains("## Response format (strict)"));
        assert!(
            prompt.contains("do not emit experiment documents"),
            "without generation the prompt must forbid toon fences"
        );
        assert!(
            !prompt.contains("```toon"),
            "no toon fence instructions without generation"
        );
    }

    #[test]
    fn prompt_requests_toon_fences_when_generating() {
        let prompt = build_agent_prompt(&heuristic_fixture(), "catalog", true);

        assert!(prompt.contains("```toon"));
        assert!(
            prompt.contains("title: Redis resilience"),
            "prompt must embed the compact format example"
        );
        assert!(prompt.contains("steady_state_hypothesis:"));
        assert!(prompt.contains("Zero or more fenced code blocks tagged `toon`"));
    }

    #[test]
    fn prompt_sanitises_goal_and_journal_signals() {
        let mut fixture = heuristic_fixture();
        fixture.goal = Some("harden \u{202E}the\u{200B} cache tier".to_string());
        fixture.heuristic_context = "Coverage: 5%\u{0007}\u{FEFF}".to_string();
        let prompt = build_agent_prompt(&fixture, "catalog", false);

        assert!(prompt.contains("Operator goal: harden the cache tier"));
        assert!(prompt.contains("Coverage: 5%"));
        assert!(
            !prompt.contains('\u{202E}'),
            "bidi override must be stripped"
        );
        assert!(
            !prompt.contains('\u{200B}'),
            "zero-width space must be stripped"
        );
        assert!(
            !prompt.contains('\u{0007}'),
            "control char must be stripped"
        );
        assert!(!prompt.contains('\u{FEFF}'), "BOM must be stripped");
    }

    #[test]
    fn prompt_truncates_oversized_goal() {
        let mut fixture = heuristic_fixture();
        fixture.goal = Some("g".repeat(5000));
        let prompt = build_agent_prompt(&fixture, "catalog", false);

        assert!(prompt.contains("… [truncated]"));
        assert!(
            !prompt.contains(&"g".repeat(5000)),
            "the full oversized goal must not reach the prompt"
        );
    }

    #[test]
    fn split_handles_response_without_fences() {
        let (text, blocks) = split_toon_blocks("## Recommendations\n1. Do the thing.\n");
        assert_eq!(text, "## Recommendations\n1. Do the thing.");
        assert!(blocks.is_empty());
    }

    #[test]
    fn split_extracts_multiple_toon_blocks() {
        let response = "## Recommendations\n1. First.\n\n```toon\ntitle: one\n```\nbetween\n```toon\ntitle: two\nmethod[0]:\n```\nafter\n";
        let (text, blocks) = split_toon_blocks(response);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], "title: one\n");
        assert_eq!(blocks[1], "title: two\nmethod[0]:\n");
        assert!(text.contains("1. First."));
        assert!(text.contains("between"));
        assert!(text.contains("after"));
        assert!(!text.contains("title: one"));
    }

    #[test]
    fn split_keeps_unterminated_fence_as_partial_block() {
        let response = "intro\n```toon\ntitle: cut off";
        let (text, blocks) = split_toon_blocks(response);
        assert_eq!(text, "intro");
        assert_eq!(blocks, vec!["title: cut off\n".to_string()]);
    }

    #[test]
    fn split_ignores_non_toon_fences() {
        let response = "text\n```bash\necho hi\n```\ntail\n";
        let (text, blocks) = split_toon_blocks(response);
        assert!(blocks.is_empty());
        assert!(text.contains("echo hi"));
    }

    #[test]
    fn split_handles_empty_response() {
        let (text, blocks) = split_toon_blocks("");
        assert!(text.is_empty());
        assert!(blocks.is_empty());
    }
}
