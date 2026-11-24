//! The plan scenarios of Delegated Regulation (EU) 2024/1774.
//!
//! RTS Art. 26(2) lists the scenarios the ICT response and recovery plans
//! of an entity under the full framework take into account, points (a) to
//! (i). Art. 25(2) asks that the tests of the continuity plans always
//! include the scenarios the plans were built on; its point (b) names the
//! failure of an ICT third-party service provider, and its point (c) the
//! switchover to the redundant capacity, backups and redundant facilities
//! (not for a microenterprise). Under the simplified framework, Art. 39(1)
//! names a cyber-attack scenario and Art. 40(1) a yearly test of the
//! backup and restore procedures.
//!
//! A run (the `scenarios` of its regulatory block) and a manual record
//! name the scenarios they tested by the ids of [`SCENARIOS`], a closed
//! set: an id the set does not have is refused on write, with the nearest
//! id. The registry rows `rts1774.art26` and `rts1774.art39` are scored
//! from the scenarios they need ([`required_by`]). Reading Art. 25(2) with
//! Art. 26(2) as "a test per scenario" is the house's reading.

use std::fmt;

use super::refs::{distance, shared_start};
use super::requirements::{EntityProfile, Profile, Requirement};

/// The ELI of Delegated Regulation (EU) 2024/1774.
pub const RTS_1774_ELI: &str = "https://eur-lex.europa.eu/eli/reg_del/2024/1774/oj";

/// One scenario of the closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    /// Stable id, e.g. `power_outage`.
    pub id: &'static str,
    /// What the scenario is, in plain words.
    pub title: &'static str,
    /// Where the RTS names it.
    pub cite: &'static str,
    /// The registry rows that need a test of it.
    pub rows: &'static [&'static str],
    /// A microenterprise under the full framework need not test it
    /// (RTS Art. 25(2)(c)).
    pub micro_exempt: bool,
}

/// The full-framework row: RTS Art. 26 response and recovery plans.
const ART26: &str = "rts1774.art26";
/// The simplified-framework row: RTS Art. 39 business continuity plan.
const ART39: &str = "rts1774.art39";

/// Row template: a scenario the full-framework row needs.
const fn full(id: &'static str, title: &'static str, cite: &'static str) -> Scenario {
    Scenario {
        id,
        title,
        cite,
        rows: &[ART26],
        micro_exempt: false,
    }
}

/// The scenarios, in display order: Art. 26(2) point by point, with point
/// (a) split into the cyber attack and the three switchovers of
/// Art. 25(2)(c), and point (b) into a function that fails and a provider
/// that fails.
pub const SCENARIOS: &[Scenario] = &[
    Scenario {
        rows: &[ART26, ART39],
        ..full(
            "cyber_attack",
            "A cyber attack",
            "Art. 26(2)(a); Art. 39(1)",
        )
    },
    Scenario {
        micro_exempt: true,
        ..full(
            "switchover",
            "Switchover from the primary ICT infrastructure to the redundant capacity",
            "Art. 25(2)(c); Art. 26(2)(a)",
        )
    },
    Scenario {
        rows: &[ART26, ART39],
        micro_exempt: true,
        ..full(
            "backup_restore",
            "Restore from backups",
            "Art. 25(2)(c); Art. 26(2)(a); Art. 39(2)(g); Art. 40(1)",
        )
    },
    Scenario {
        micro_exempt: true,
        ..full(
            "redundant_facility",
            "Switchover to redundant facilities",
            "Art. 25(2)(c); Art. 26(2)(a)",
        )
    },
    full(
        "function_degraded",
        "A critical or important function degrades to an unacceptable level or fails",
        "Art. 26(2)(b)",
    ),
    full(
        "third_party_failure",
        "An ICT third-party service provider fails or becomes insolvent",
        "Art. 25(2)(b); Art. 26(2)(b); Art. 26(4)",
    ),
    full(
        "premises_loss",
        "Partial or total failure of premises: offices, business premises or data centres",
        "Art. 26(2)(c)",
    ),
    full(
        "ict_asset_failure",
        "Substantial failure of ICT assets or of the communication infrastructure",
        "Art. 26(2)(d)",
    ),
    full(
        "staff_unavailable",
        "A critical number of staff, or the staff who keep operations going, not available",
        "Art. 26(2)(e)",
    ),
    full(
        "physical_event",
        "Climate or environment events, natural disasters, pandemics or physical attacks",
        "Art. 26(2)(f)",
    ),
    full("insider_attack", "An insider attack", "Art. 26(2)(g)"),
    full(
        "political_instability",
        "Political or social instability, also where a provider is or where data is kept",
        "Art. 26(2)(h); Art. 25(2)(b)",
    ),
    full("power_outage", "A widespread power outage", "Art. 26(2)(i)"),
];

/// The scenario with this id.
#[must_use]
pub fn scenario(id: &str) -> Option<&'static Scenario> {
    SCENARIOS.iter().find(|s| s.id == id)
}

/// The scenarios a registry row needs a test of, for this entity, in
/// display order. Empty for a row that is not scored from scenarios. The
/// carve-out of Art. 25(2)(c) spares a microenterprise under the full
/// framework only; Arts. 39 and 40 carve none out.
pub fn required_by<'a>(
    req: &'a Requirement,
    entity: &'a EntityProfile,
) -> impl Iterator<Item = &'static Scenario> + 'a {
    let spared = entity.profile == Profile::Full && entity.flags.microenterprise;
    SCENARIOS
        .iter()
        .filter(move |s| s.rows.contains(&req.id) && !(spared && s.micro_exempt))
}

/// A scenario id the closed set does not have, with the nearest id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownScenario {
    /// The id as it was written.
    pub id: String,
    /// The nearest id of the set.
    pub suggestion: Option<&'static str>,
}

impl fmt::Display for UnknownScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' is not a scenario id of RTS 2024/1774 Arts. 25, 26 and 39",
            self.id
        )?;
        match self.suggestion {
            Some(id) => write!(f, "; did you mean '{id}'?"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for UnknownScenario {}

/// The write check of one scenario id of a run or a record.
///
/// # Errors
///
/// [`UnknownScenario`] with the nearest id, for an id the set does not
/// have.
pub fn check_scenario(id: &str) -> Result<(), UnknownScenario> {
    if scenario(id).is_some() {
        return Ok(());
    }
    Err(UnknownScenario {
        id: id.to_owned(),
        suggestion: nearest(id),
    })
}

/// The id of the set nearest to `id`, when it is within half the length
/// of that id; on a tie, the one that starts with more of it, then the
/// first in display order.
fn nearest(id: &str) -> Option<&'static str> {
    let want = id.trim().to_ascii_lowercase();
    SCENARIOS
        .iter()
        .map(|s| (distance(&want, s.id), s.id))
        .filter(|(far, known)| *far <= known.len() / 2)
        .min_by_key(|(far, known)| (*far, std::cmp::Reverse(shared_start(&want, known))))
        .map(|(_, known)| known)
}

#[cfg(test)]
mod tests {
    use super::super::requirements::{requirement, REQUIREMENTS};
    use super::*;

    fn entity(profile: &str, flags: &[&str]) -> EntityProfile {
        let flags: Vec<String> = flags.iter().map(|f| (*f).to_string()).collect();
        EntityProfile::parse(profile, &flags).expect("valid profile")
    }

    fn ids_required_by(row: &str, e: &EntityProfile) -> Vec<&'static str> {
        let req = requirement(row).unwrap_or_else(|| panic!("no row {row}"));
        required_by(req, e).map(|s| s.id).collect()
    }

    /// The closed set: one id per scenario Art. 26(2) points (a) to (i)
    /// and Art. 25(2)(c) name, point (a) split in its four scenarios and
    /// point (b) in its two.
    #[test]
    fn the_set_holds_every_scenario_the_rts_names_once() {
        let ids: Vec<&str> = SCENARIOS.iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            [
                "cyber_attack",
                "switchover",
                "backup_restore",
                "redundant_facility",
                "function_degraded",
                "third_party_failure",
                "premises_loss",
                "ict_asset_failure",
                "staff_unavailable",
                "physical_event",
                "insider_attack",
                "political_instability",
                "power_outage",
            ]
        );
        for s in SCENARIOS {
            assert!(
                s.id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{} is not snake case",
                s.id
            );
            assert!(!s.title.is_empty(), "{}", s.id);
            assert!(s.cite.starts_with("Art. 2") || s.cite.starts_with("Art. 39"));
            for row in s.rows {
                assert!(requirement(row).is_some(), "{} needs a row {row}", s.id);
            }
            assert_eq!(scenario(s.id), Some(s));
        }
        assert_eq!(scenario("fire"), None);
    }

    /// Every point of Art. 26(2) is cited by a scenario.
    #[test]
    fn each_point_of_art_26_2_has_a_scenario() {
        for point in ["a", "b", "c", "d", "e", "f", "g", "h", "i"] {
            let cite = format!("Art. 26(2)({point})");
            assert!(
                SCENARIOS.iter().any(|s| s.cite.contains(&cite)),
                "no scenario cites {cite}"
            );
        }
        let cite = |id: &str| scenario(id).unwrap().cite;
        assert!(cite("power_outage").contains("Art. 26(2)(i)"));
        assert!(cite("premises_loss").contains("Art. 26(2)(c)"));
        assert!(cite("third_party_failure").contains("Art. 25(2)(b)"));
        assert!(cite("switchover").contains("Art. 25(2)(c)"));
        assert!(cite("cyber_attack").contains("Art. 39(1)"));
        assert!(cite("backup_restore").contains("Art. 40(1)"));
    }

    /// A full-framework entity tests all 13; a microenterprise under it is
    /// spared switchover, backups and redundant facilities (Art. 25(2)(c)).
    #[test]
    fn the_full_framework_row_needs_every_scenario_but_the_micro_carve_out() {
        let all: Vec<&str> = SCENARIOS.iter().map(|s| s.id).collect();
        assert_eq!(ids_required_by("rts1774.art26", &entity("full", &[])), all);
        let micro = ids_required_by("rts1774.art26", &entity("full", &["microenterprise"]));
        assert_eq!(micro.len(), 10, "{micro:?}");
        for spared in ["switchover", "backup_restore", "redundant_facility"] {
            assert!(!micro.contains(&spared), "{spared}: {micro:?}");
        }
        let exempt: Vec<&str> = SCENARIOS
            .iter()
            .filter(|s| s.micro_exempt)
            .map(|s| s.id)
            .collect();
        assert_eq!(
            exempt,
            ["switchover", "backup_restore", "redundant_facility"]
        );
    }

    /// The simplified plan names a cyber attack (Art. 39(1)) and is tested
    /// yearly for its backup and restore procedures (Art. 40(1)), also for
    /// a microenterprise: Arts. 39 and 40 carve none out.
    #[test]
    fn the_simplified_row_needs_a_cyber_attack_and_a_restore() {
        for flags in [&[][..], &["microenterprise"][..]] {
            assert_eq!(
                ids_required_by("rts1774.art39", &entity("simplified", flags)),
                ["cyber_attack", "backup_restore"]
            );
        }
    }

    #[test]
    fn rows_not_scored_from_scenarios_need_none() {
        let full = entity("full", &[]);
        for req in REQUIREMENTS {
            if matches!(req.id, "rts1774.art26" | "rts1774.art39") {
                continue;
            }
            assert_eq!(required_by(req, &full).count(), 0, "{}", req.id);
        }
    }

    #[test]
    fn an_id_of_the_set_passes_and_any_other_is_refused_with_the_nearest() {
        for s in SCENARIOS {
            assert_eq!(check_scenario(s.id), Ok(()), "{}", s.id);
        }
        let err = check_scenario("power_outages").unwrap_err();
        assert_eq!(err.suggestion, Some("power_outage"));
        assert_eq!(
            err.to_string(),
            "'power_outages' is not a scenario id of RTS 2024/1774 Arts. 25, 26 and 39; \
             did you mean 'power_outage'?"
        );
        // The set is lower case: an id is not matched loosely.
        let err = check_scenario("Power_Outage").unwrap_err();
        assert_eq!(err.suggestion, Some("power_outage"));
        // On a tie the id that starts with more of the text wins.
        assert_eq!(
            check_scenario("switch").unwrap_err().suggestion,
            Some("switchover")
        );
        // Far from every id: no guess.
        let far = check_scenario("the building burned down").unwrap_err();
        assert_eq!(far.suggestion, None);
        assert_eq!(
            far.to_string(),
            "'the building burned down' is not a scenario id of RTS 2024/1774 Arts. 25, 26 and 39"
        );
    }
}
