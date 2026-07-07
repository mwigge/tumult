//! Deterministic, explainable injection recommendations.
//!
//! Given the lineage matrix, the declared dependency topology, and the plugin
//! action catalog, rank the next most valuable fault injections. Every score
//! is a product of transparent factors and every factor appends a
//! human-readable reason, because a recommendation an operator cannot audit
//! is a recommendation they will not run. Pure and deterministic — same
//! inputs, same output, byte for byte — so it is safe to expose over MCP
//! without a "why did it change?" support burden.
//!
//! # Scoring
//!
//! For each `(service, article)` from a Broken or Untested lineage cell,
//! paired with one catalog action:
//!
//! `score = state × strength × centrality × proximity × novelty`
//!
//! * **state** — broken 1.0, untested 0.6: fixing a demonstrated break
//!   outranks probing an untested control.
//! * **strength** — citation strength of the article (direct 1.0,
//!   supporting 0.7, indirect 0.4; unknown defaults to indirect).
//! * **centrality** — `1 + in_degree / max_in_degree`: services more
//!   depended-upon are worth testing first.
//! * **proximity** — `1 / (1 + d)` where `d` is the shortest undirected
//!   `depends_on` distance to the nearest broken service (0 when the service
//!   itself is broken; 0 for everyone when nothing is broken; 5 when
//!   unreachable): blast-radius neighbours of a known break come first.
//! * **novelty** — 1.25 when the action has never been tested.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::coverage::AvailableAction;
use crate::lineage::{ControlServiceStatus, LineageCell};

/// Undirected `depends_on` distance assigned when no broken service is
/// reachable from a candidate — far, but not a total veto.
const UNREACHABLE_DISTANCE: usize = 5;

/// Everything [`recommend`] needs; the caller reads it from storage/catalog.
pub struct RecommendationInput<'a> {
    /// The lineage matrix from [`crate::lineage::compute_lineage`].
    pub lineage: &'a [LineageCell],
    /// Declared `(src service id, dst service id)` dependency edges.
    pub depends_on: &'a [(String, String)],
    /// The plugin action catalog.
    pub available_actions: &'a [AvailableAction],
    /// Action names that have appeared in a tested run.
    pub tested_action_names: &'a HashSet<String>,
    /// Article id → citation strength (`direct`/`supporting`/`indirect`).
    pub article_strength: &'a HashMap<String, String>,
    /// Observed traffic criticality per service id (e.g. OTel span rate).
    /// Empty map = factor neutral. Values are relative; only the ratio to
    /// the maximum matters.
    pub criticality: &'a HashMap<String, f64>,
}

/// One ranked, explained injection recommendation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recommendation {
    /// The service to inject on.
    pub service_id: String,
    /// The plugin owning the recommended action.
    pub plugin: String,
    /// The recommended fault action.
    pub action: String,
    /// The compliance article the injection would inform.
    pub article_id: String,
    /// Citation strength used in scoring.
    pub strength: String,
    /// The composite score (product of the documented factors).
    pub score: f64,
    /// One human-readable reason per scoring factor.
    pub reasons: Vec<String>,
}

/// Rank injection candidates; deterministic, truncated to `limit`.
///
/// One candidate per `(service, article)`: the first (sorted by
/// `plugin::action`) never-tested action is preferred — all non-novelty
/// factors are identical across actions for a fixed pair, so among untested
/// actions the sorted-first one is the canonical highest scorer; when every
/// action is tested the first sorted action is used. Final order is
/// `(score desc, service_id, plugin::action, article_id)`.
#[must_use]
pub fn recommend(input: &RecommendationInput<'_>, limit: usize) -> Vec<Recommendation> {
    let mut actions: Vec<&AvailableAction> = input.available_actions.iter().collect();
    actions.sort_by(|a, b| (&a.plugin, &a.action).cmp(&(&b.plugin, &b.action)));
    if actions.is_empty() {
        return Vec::new();
    }

    let (in_degree, max_in_degree) = in_degrees(input.depends_on);
    let broken_services: HashSet<&str> = input
        .lineage
        .iter()
        .filter(|cell| cell.status == ControlServiceStatus::Broken)
        .map(|cell| cell.service_id.as_str())
        .collect();
    let distances = broken_distances(input.depends_on, &broken_services);

    // BTreeMap dedupes (service, article) pairs and fixes iteration order.
    let mut candidates: BTreeMap<(&str, &str), ControlServiceStatus> = BTreeMap::new();
    for cell in input.lineage {
        if matches!(
            cell.status,
            ControlServiceStatus::Broken | ControlServiceStatus::Untested
        ) {
            candidates.insert(
                (cell.service_id.as_str(), cell.article_id.as_str()),
                cell.status,
            );
        }
    }

    let mut out: Vec<Recommendation> = Vec::new();
    for ((service, article), status) in candidates {
        let action = actions
            .iter()
            .find(|a| !input.tested_action_names.contains(&a.action))
            .or(actions.first())
            .copied();
        let Some(action) = action else { continue };

        let mut score = 1.0;
        let mut reasons = Vec::new();

        let (state_weight, state_word) = match status {
            ControlServiceStatus::Broken => (1.0, "broken"),
            _ => (0.6, "untested"),
        };
        score *= state_weight;
        reasons.push(format!("{article} is {state_word} on {service}"));

        let strength = input
            .article_strength
            .get(article)
            .map_or("indirect", String::as_str);
        score *= strength_weight(strength);
        reasons.push(format!("citation strength for {article} is {strength}"));

        let degree = in_degree.get(service).copied().unwrap_or(0);
        if max_in_degree > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                score *= 1.0 + degree as f64 / max_in_degree as f64;
            }
        }
        reasons.push(format!(
            "{degree} service{} depend on {service}",
            if degree == 1 { "" } else { "s" }
        ));

        let d = if broken_services.is_empty() {
            0
        } else {
            distances
                .get(service)
                .copied()
                .unwrap_or(UNREACHABLE_DISTANCE)
        };
        #[allow(clippy::cast_precision_loss)]
        {
            score *= 1.0 / (1.0 + d as f64);
        }
        reasons.push(match d {
            0 if broken_services.contains(service) => {
                format!("{service} itself has a broken control")
            }
            0 => "no broken services anywhere — proximity neutral".to_string(),
            _ => format!("{d} hop(s) from the nearest broken service"),
        });

        if !input.tested_action_names.contains(&action.action) {
            score *= 1.25;
            reasons.push(format!(
                "action {}::{} never tested",
                action.plugin, action.action
            ));
        }

        // Observed-traffic criticality (OTel-derived): services carrying
        // more real traffic score higher, up to 2x for the busiest. Absent
        // data is neutral — silence must not look like importance.
        let max_criticality = input
            .criticality
            .values()
            .fold(0.0_f64, |acc, v| acc.max(*v));
        if max_criticality > 0.0 {
            if let Some(rate) = input.criticality.get(service) {
                if *rate > 0.0 {
                    score *= 1.0 + rate / max_criticality;
                    reasons.push(format!(
                        "observed traffic rate {rate:.0} ({:.0}% of busiest service)",
                        100.0 * rate / max_criticality
                    ));
                }
            }
        }

        out.push(Recommendation {
            service_id: service.to_string(),
            plugin: action.plugin.clone(),
            action: action.action.clone(),
            article_id: article.to_string(),
            strength: strength.to_string(),
            score,
            reasons,
        });
    }

    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.service_id.cmp(&b.service_id))
            .then_with(|| (&a.plugin, &a.action).cmp(&(&b.plugin, &b.action)))
            .then_with(|| a.article_id.cmp(&b.article_id))
    });
    out.truncate(limit);
    out
}

/// Citation-strength factor; unknown labels degrade to the indirect weight
/// rather than failing, because scoring must be total.
fn strength_weight(strength: &str) -> f64 {
    match strength {
        "direct" => 1.0,
        "supporting" => 0.7,
        _ => 0.4,
    }
}

/// In-degree per service — how many *distinct* services depend on it — plus
/// the maximum across all services.
fn in_degrees(depends_on: &[(String, String)]) -> (HashMap<&str, usize>, usize) {
    let mut dependents: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (src, dst) in depends_on {
        dependents
            .entry(dst.as_str())
            .or_default()
            .insert(src.as_str());
    }
    let degrees: HashMap<&str, usize> = dependents
        .into_iter()
        .map(|(dst, srcs)| (dst, srcs.len()))
        .collect();
    let max = degrees.values().copied().max().unwrap_or(0);
    (degrees, max)
}

/// Multi-source BFS over the *undirected* `depends_on` graph from every
/// broken service: blast radius flows both ways (a broken dependency hurts
/// its dependents, and a broken dependent implicates what it leans on).
fn broken_distances<'a>(
    depends_on: &'a [(String, String)],
    broken: &HashSet<&'a str>,
) -> HashMap<&'a str, usize> {
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for (src, dst) in depends_on {
        adjacency
            .entry(src.as_str())
            .or_default()
            .push(dst.as_str());
        adjacency
            .entry(dst.as_str())
            .or_default()
            .push(src.as_str());
    }
    let mut dist: HashMap<&str, usize> = HashMap::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    let mut sources: Vec<&str> = broken.iter().copied().collect();
    sources.sort_unstable();
    for service in sources {
        dist.insert(service, 0);
        queue.push_back(service);
    }
    while let Some(current) = queue.pop_front() {
        let next_dist = dist.get(current).copied().unwrap_or(0) + 1;
        if let Some(neighbours) = adjacency.get(current) {
            for &neighbour in neighbours {
                if !dist.contains_key(neighbour) {
                    dist.insert(neighbour, next_dist);
                    queue.push_back(neighbour);
                }
            }
        }
    }
    dist
}

// Behaviour tests live in `tests/recommend.rs` (the fixture-heavy suite kept
// this module over the size budget); only pure helpers are tested here.
#[cfg(test)]
mod tests {
    use super::strength_weight;

    #[test]
    fn strength_weights_match_the_documented_table() {
        assert!((strength_weight("direct") - 1.0).abs() < 1e-12);
        assert!((strength_weight("supporting") - 0.7).abs() < 1e-12);
        assert!((strength_weight("indirect") - 0.4).abs() < 1e-12);
        assert!((strength_weight("banana") - 0.4).abs() < 1e-12); // total
    }
}
