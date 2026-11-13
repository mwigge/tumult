//! Paragraph-level DORA requirement registry.
//!
//! Each row names one obligation of Regulation (EU) 2022/2554 (or of the
//! simplified-framework RTS, Delegated Regulation (EU) 2024/1774), the class
//! of evidence that can speak to it, the window in which that evidence
//! stays fresh, the functions it binds and the entity profiles it applies
//! to. The coverage score, the per-requirement evidence status and the
//! maturity gates all read this table; nothing else carries requirement
//! ids, weights or windows.
//!
//! The weights, the evidence selectors and the expected-status defaults
//! are house choices, not regulatory text. The registry is therefore
//! signed off by the compliance owner per deployment (`signed_by` and
//! `signed_at`, recorded with the legal-entity register) and versioned by
//! [`REQUIREMENTS_VERSION`].

use super::evidence::EvidenceStrength;

/// Version of the requirement registry. Bump on any row change so a score
/// can be traced to the rule set that produced it.
pub const REQUIREMENTS_VERSION: &str = "2026.11";

/// Evidence status code: a declaration exists but no qualifying evidence.
pub const EXPECTED_STATUS_DESIGNED: u8 = 1;
/// Evidence status code: qualifying evidence exists today.
pub const EXPECTED_STATUS_EXISTING: u8 = 2;
/// Evidence status code: qualifying evidence held without a gap for the
/// whole operating window.
pub const EXPECTED_STATUS_OPERATING: u8 = 3;

/// House default: rows at or above this weight are "core" rows, which a
/// full-framework entity is expected to hold at Operating.
pub const CORE_WEIGHT_MIN: u8 = 3;
/// Weight of a gating row (the level-3 gate reads these).
pub const GATING_WEIGHT: u8 = 5;

/// Yearly window, in days.
const YEAR: u32 = 365;
/// Telemetry-class window, in days.
const TELEMETRY: u32 = 90;
/// SLO-class window, in days.
const SLO: u32 = 30;
/// Threat-led penetration testing window: three years, in days.
const THREE_YEARS: u32 = 1095;

/// Which ICT risk management framework binds the legal entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Profile {
    /// Entities under Arts. 5–15.
    Full,
    /// Art. 16(1) entities under the simplified framework.
    Simplified,
}

impl Profile {
    /// Stable lower-case name, as stored on `ict_legal_entities`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Simplified => "simplified",
        }
    }
}

/// Entity flags that switch registry rows on or off. Set by the compliance
/// owner on the legal entity; never inferred from telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // one bool per regulatory flag
pub struct EntityFlags {
    pub microenterprise: bool,
    pub tlpt_designated: bool,
    pub csd: bool,
    pub ccp: bool,
    pub trading_venue: bool,
    pub credit_institution: bool,
    pub nis2: bool,
}

/// Every flag name accepted on a legal entity, in display order.
pub const FLAG_NAMES: [&str; 7] = [
    "microenterprise",
    "tlpt_designated",
    "csd",
    "ccp",
    "trading_venue",
    "credit_institution",
    "nis2",
];

impl EntityFlags {
    fn slot(&mut self, name: &str) -> Option<&mut bool> {
        Some(match name {
            "microenterprise" => &mut self.microenterprise,
            "tlpt_designated" => &mut self.tlpt_designated,
            "csd" => &mut self.csd,
            "ccp" => &mut self.ccp,
            "trading_venue" => &mut self.trading_venue,
            "credit_institution" => &mut self.credit_institution,
            "nis2" => &mut self.nis2,
            _ => return None,
        })
    }

    fn get(&self, name: &str) -> bool {
        let mut copy = *self;
        copy.slot(name).is_some_and(|flag| *flag)
    }
}

/// The profile and flags of one legal entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityProfile {
    pub profile: Profile,
    pub flags: EntityFlags,
}

impl EntityProfile {
    /// A full-framework entity with no flags set.
    #[must_use]
    pub fn full() -> Self {
        Self {
            profile: Profile::Full,
            flags: EntityFlags::default(),
        }
    }

    /// Parse the stored profile name and flag list.
    ///
    /// # Errors
    /// Names the offending value when the profile or a flag is unknown.
    pub fn parse(profile: &str, flags: &[String]) -> Result<Self, String> {
        let profile = match profile {
            "full" => Profile::Full,
            "simplified" => Profile::Simplified,
            other => return Err(format!("unknown profile '{other}' (full|simplified)")),
        };
        let mut parsed = EntityFlags::default();
        for name in flags {
            let Some(slot) = parsed.slot(name) else {
                return Err(format!(
                    "unknown entity flag '{name}' ({})",
                    FLAG_NAMES.join("|")
                ));
            };
            *slot = true;
        }
        Ok(Self {
            profile,
            flags: parsed,
        })
    }

    /// Names of the flags that are set, in display order.
    #[must_use]
    pub fn flag_names(&self) -> Vec<&'static str> {
        FLAG_NAMES
            .into_iter()
            .filter(|name| self.flags.get(name))
            .collect()
    }

    /// The label printed next to every level: `full`, `simplified`, or
    /// either followed by `, microenterprise`.
    #[must_use]
    pub fn label(&self) -> String {
        if self.flags.microenterprise {
            return format!("{}, microenterprise", self.profile.as_str());
        }
        self.profile.as_str().to_string()
    }

    /// Whether the reduced (simplified or microenterprise) rule set applies.
    #[must_use]
    pub fn is_reduced(&self) -> bool {
        self.profile == Profile::Simplified || self.flags.microenterprise
    }
}

/// The class of evidence a requirement can be observed through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceClass {
    /// Executed tests: automated runs and verified manual exercises.
    Test,
    /// Signals derived from ingested telemetry.
    Telemetry,
    /// Signed manual records (attestations, reviews, tabletop exercises).
    Manual,
    /// The incident register.
    Incident,
}

impl EvidenceClass {
    /// Stable lower-case name used in API payloads.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Telemetry => "telemetry",
            Self::Manual => "manual",
            Self::Incident => "incident",
        }
    }
}

/// Which functions a requirement binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionScope {
    /// Every registered function.
    All,
    /// Functions assessed as critical or important.
    CriticalImportant,
}

impl FunctionScope {
    /// Stable name used in API payloads.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::CriticalImportant => "critical,important",
        }
    }
}

/// Which framework profiles a row applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSet {
    Full,
    Simplified,
    Any,
}

/// How the microenterprise flag changes a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroRule {
    /// Applies regardless of the flag.
    Any,
    /// Carved out for microenterprises.
    Exempt,
    /// Applies to microenterprises only.
    Only,
}

/// Sector or designation flag a row requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagRule {
    None,
    Csd,
    Ccp,
    CsdOrCcp,
    TlptDesignated,
}

/// An external feed whose connection decides whether a row is observable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// The incident register (Arts. 17–19).
    Incident,
    /// The SLO registry.
    Slo,
    /// The register-of-information export and reconciliation.
    Register,
}

impl Feed {
    /// Name used in the legal entity's `feeds` list.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incident => "incident",
            Self::Slo => "slo",
            Self::Register => "register",
        }
    }
}

/// How qualifying evidence for a row is selected. The selector is the
/// house reading of what evidences the paragraph; it is data, so the
/// compliance owner can review every row in one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selector {
    /// A run (or verified manual exercise) of one of these disciplines
    /// against the function; an empty list accepts every discipline.
    Run { kinds: &'static [&'static str] },
    /// Runs in at least `min` distinct disciplines inside the window.
    Disciplines { min: usize },
    /// Every failed test is followed by a passing run of the same test.
    Remediation,
    /// Recovery time (and recovery point, when measured) within target.
    RecoveryMet,
    /// A measured recovery point within its target.
    RecoveryPoint,
    /// A verified manual exercise of one of these types that names the
    /// function; with `cited`, the record must also cite the row id.
    Exercise {
        types: &'static [&'static str],
        cited: bool,
    },
    /// A verified manual record that cites the row id. Unobservable until
    /// a record citing the row exists at all.
    Attestation,
    /// The function is registered with a criticality assessment date.
    FunctionAssessed,
    /// A test plan or schedule is declared for the function.
    Programme,
    /// The share of the function's planned tests executed in their interval.
    PlanExecuted,
    /// Telemetry from the function's services inside the window.
    Telemetry,
    /// The function's dependencies are observable, so single points of
    /// failure can be identified.
    Topology,
    /// Observable only through the named feed.
    Feed(Feed),
}

/// One row of the requirement registry.
#[derive(Debug, Clone, Copy)]
pub struct Requirement {
    /// Stable id, e.g. `dora.art24.6`.
    pub id: &'static str,
    /// Article key the row is grouped under (see [`ARTICLES`]).
    pub article: &'static str,
    /// Human citation, e.g. `Art. 24(6)`.
    pub cite: &'static str,
    /// What the row checks, in the house's words.
    pub title: &'static str,
    /// 1 minor, 2 normal, 3 core, 5 gating.
    pub weight: u8,
    /// Class of evidence the row is observed through.
    pub evidence: EvidenceClass,
    /// Days in which evidence counts at full freshness.
    pub window_days: u32,
    /// Functions the row binds.
    pub scope: FunctionScope,
    /// Strength cap applied to automated evidence for this row.
    pub strength: EvidenceStrength,
    /// Framework profiles the row applies to.
    pub profiles: ProfileSet,
    /// Microenterprise carve-out.
    pub micro: MicroRule,
    /// Sector or designation flag the row needs.
    pub flag: FlagRule,
    /// How qualifying evidence is selected.
    pub selector: Selector,
}

impl Requirement {
    /// Whether the row applies to an entity with this profile and flags.
    /// Rows that do not apply are `na`: dropped from every ratio.
    #[must_use]
    pub fn applies(&self, entity: &EntityProfile) -> bool {
        self.profile_matches(entity.profile)
            && self.micro_matches(entity.flags.microenterprise)
            && self.flag_matches(&entity.flags)
    }

    fn profile_matches(&self, profile: Profile) -> bool {
        match self.profiles {
            ProfileSet::Any => true,
            ProfileSet::Full => profile == Profile::Full,
            ProfileSet::Simplified => profile == Profile::Simplified,
        }
    }

    fn micro_matches(&self, microenterprise: bool) -> bool {
        match self.micro {
            MicroRule::Any => true,
            MicroRule::Exempt => !microenterprise,
            MicroRule::Only => microenterprise,
        }
    }

    fn flag_matches(&self, flags: &EntityFlags) -> bool {
        match self.flag {
            FlagRule::None => true,
            FlagRule::Csd => flags.csd,
            FlagRule::Ccp => flags.ccp,
            FlagRule::CsdOrCcp => flags.csd || flags.ccp,
            FlagRule::TlptDesignated => flags.tlpt_designated,
        }
    }

    /// Whether the row binds a function of this tier.
    #[must_use]
    pub fn binds(&self, critical_or_important: bool) -> bool {
        self.scope == FunctionScope::All || critical_or_important
    }

    /// Registry default for the expected evidence status: core and gating
    /// rows expect Operating under the full framework; every other row,
    /// and every row under the simplified framework or for a
    /// microenterprise, expects Existing.
    #[must_use]
    pub fn expected_status(&self, entity: &EntityProfile) -> u8 {
        if entity.is_reduced() || self.weight < CORE_WEIGHT_MIN {
            return EXPECTED_STATUS_EXISTING;
        }
        EXPECTED_STATUS_OPERATING
    }
}

/// Article keys in display order, with their titles.
pub const ARTICLES: &[(&str, &str)] = &[
    ("5", "Governance and organisation"),
    ("6", "ICT risk management framework"),
    ("9", "Protection and prevention"),
    ("10", "Detection"),
    ("11", "Response and recovery"),
    (
        "12",
        "Backup policies and procedures, restoration and recovery procedures and methods",
    ),
    ("13", "Learning and evolving"),
    ("16", "Simplified ICT risk management framework"),
    ("17", "ICT-related incident management process"),
    (
        "18",
        "Classification of ICT-related incidents and cyber threats",
    ),
    ("19", "Reporting of major ICT-related incidents"),
    (
        "24",
        "General requirements for digital operational resilience testing",
    ),
    ("25", "Testing of ICT tools and systems"),
    ("26", "Advanced testing based on TLPT"),
    ("28", "ICT third-party risk: general principles"),
    (
        "rts1774",
        "Delegated Regulation (EU) 2024/1774 (ICT risk management RTS)",
    ),
];

/// Title of an article key, `None` when the key is not in [`ARTICLES`].
#[must_use]
pub fn article_title(key: &str) -> Option<&'static str> {
    ARTICLES
        .iter()
        .find(|(article, _)| *article == key)
        .map(|(_, title)| *title)
}

/// The row with this id.
#[must_use]
pub fn requirement(id: &str) -> Option<&'static Requirement> {
    REQUIREMENTS.iter().find(|r| r.id == id)
}

/// Rows that apply to an entity, in registry order.
pub fn applicable(entity: &EntityProfile) -> impl Iterator<Item = &'static Requirement> + '_ {
    REQUIREMENTS.iter().filter(move |r| r.applies(entity))
}

/// Every discipline counts.
const ANY_KIND: &[&str] = &[];
/// Disaster-recovery evidence only.
const DR: &[&str] = &["dr"];
/// Continuity evidence: recovery drills and fault injection.
const CONTINUITY: &[&str] = &["dr", "chaos"];
/// Security testing evidence.
const SECURITY: &[&str] = &["security"];
/// Manual exercise types that rehearse crisis communication.
const TABLETOP: &[&str] = &["tabletop", "gameday"];
/// Manual exercise types that rehearse a plan end to end.
const REHEARSAL: &[&str] = &["drill", "failover", "gameday", "tabletop"];
/// Threat-led penetration tests.
const PENTEST: &[&str] = &["pentest"];

/// Row template: a normal-weight, all-function, any-profile row. Every
/// registry row spells out only what differs from it.
const fn row(
    id: &'static str,
    article: &'static str,
    cite: &'static str,
    title: &'static str,
    weight: u8,
    evidence: EvidenceClass,
    selector: Selector,
) -> Requirement {
    Requirement {
        id,
        article,
        cite,
        title,
        weight,
        evidence,
        window_days: YEAR,
        scope: FunctionScope::All,
        strength: EvidenceStrength::Direct,
        profiles: ProfileSet::Any,
        micro: MicroRule::Any,
        flag: FlagRule::None,
        selector,
    }
}

impl Requirement {
    const fn full(mut self) -> Self {
        self.profiles = ProfileSet::Full;
        self
    }

    const fn simplified(mut self) -> Self {
        self.profiles = ProfileSet::Simplified;
        self
    }

    const fn micro(mut self, rule: MicroRule) -> Self {
        self.micro = rule;
        self
    }

    const fn flagged(mut self, flag: FlagRule) -> Self {
        self.flag = flag;
        self
    }

    const fn critical_important(mut self) -> Self {
        self.scope = FunctionScope::CriticalImportant;
        self
    }

    const fn window(mut self, days: u32) -> Self {
        self.window_days = days;
        self
    }
}

use EvidenceClass::{Incident, Manual, Telemetry as TelemetryClass, Test};

/// The requirement registry, in display order.
pub const REQUIREMENTS: &[Requirement] = &[
    // ── Chapter II, full framework (Arts. 5–15) ─────────────────────────
    row(
        "dora.art5.2",
        "5",
        "Art. 5(2)",
        "Management body approves and oversees the ICT risk arrangements",
        2,
        Manual,
        Selector::Attestation,
    )
    .full(),
    row(
        "dora.art6.5",
        "6",
        "Art. 6(5)",
        "ICT risk management framework reviewed and the review recorded",
        2,
        Manual,
        Selector::Attestation,
    )
    .full(),
    row(
        "dora.art9.1",
        "9",
        "Art. 9(1)",
        "ICT systems supporting the function are continuously monitored",
        2,
        TelemetryClass,
        Selector::Telemetry,
    )
    .full()
    .window(TELEMETRY),
    row(
        "dora.art10.1.spof",
        "10",
        "Art. 10(1)",
        "Single points of failure identified from the observed topology",
        2,
        TelemetryClass,
        Selector::Topology,
    )
    .full()
    .window(TELEMETRY),
    row(
        "dora.art10.2",
        "10",
        "Art. 10(2)",
        "Alert thresholds and criteria defined for the function",
        2,
        TelemetryClass,
        Selector::Feed(Feed::Slo),
    )
    .full()
    .window(SLO),
    row(
        "dora.art11.4",
        "11",
        "Art. 11(4)",
        "Business continuity and response plans tested periodically",
        3,
        Test,
        Selector::Run { kinds: CONTINUITY },
    )
    .full(),
    row(
        "dora.art11.6a",
        "11",
        "Art. 11(6)(a)",
        "Continuity and recovery plans tested at least yearly",
        3,
        Test,
        Selector::Run { kinds: CONTINUITY },
    )
    .full(),
    row(
        "dora.art11.6.switchover",
        "11",
        "Art. 11(6)",
        "Switchover between primary and redundant capacity tested yearly",
        5,
        Test,
        Selector::Run { kinds: DR },
    )
    .full()
    .micro(MicroRule::Exempt)
    .critical_important(),
    row(
        "dora.art11.6b",
        "11",
        "Art. 11(6)(b)",
        "Crisis communication plans tested at least yearly",
        2,
        Manual,
        Selector::Exercise {
            types: TABLETOP,
            cited: false,
        },
    )
    .full(),
    row(
        "dora.art11.7",
        "11",
        "Art. 11(7)",
        "Crisis management function established",
        2,
        Manual,
        Selector::Attestation,
    )
    .full()
    .micro(MicroRule::Exempt),
    row(
        "dora.art11.8",
        "11",
        "Art. 11(8)",
        "Records of activities kept during disruption events",
        1,
        Incident,
        Selector::Feed(Feed::Incident),
    )
    .full(),
    row(
        "dora.art11.9",
        "11",
        "Art. 11(9)",
        "Central securities depository: continuity test records provided to the authority",
        2,
        Manual,
        Selector::Attestation,
    )
    .full()
    .flagged(FlagRule::Csd),
    row(
        "dora.art12.1",
        "12",
        "Art. 12(1)",
        "Backup policies and restoration procedures documented",
        2,
        Manual,
        Selector::Attestation,
    )
    .full(),
    row(
        "dora.art12.2",
        "12",
        "Art. 12(2)",
        "Backup and restoration procedures tested periodically",
        3,
        Test,
        Selector::Run { kinds: DR },
    )
    .full(),
    row(
        "dora.art12.3",
        "12",
        "Art. 12(3)",
        "Restoration uses segregated systems",
        2,
        Manual,
        Selector::Attestation,
    )
    .full(),
    row(
        "dora.art12.3.ccp",
        "12",
        "Art. 12(3)",
        "Central counterparty: recovery plans enable recovery of all transactions",
        2,
        Manual,
        Selector::Attestation,
    )
    .full()
    .flagged(FlagRule::Ccp),
    row(
        "dora.art12.4",
        "12",
        "Art. 12(4)",
        "Redundant ICT capacity exercised",
        2,
        Test,
        Selector::Run { kinds: CONTINUITY },
    )
    .full()
    .micro(MicroRule::Exempt),
    row(
        "dora.art12.5",
        "12",
        "Art. 12(5)",
        "Central securities depository: secondary processing site maintained",
        2,
        Manual,
        Selector::Attestation,
    )
    .full()
    .flagged(FlagRule::Csd),
    row(
        "dora.art12.6",
        "12",
        "Art. 12(6)",
        "Recovery time and recovery point objectives declared and met",
        5,
        Test,
        Selector::RecoveryMet,
    )
    .full()
    .critical_important(),
    row(
        "dora.art12.7",
        "12",
        "Art. 12(7)",
        "Recovered data checked against its recovery point",
        3,
        Test,
        Selector::RecoveryPoint,
    )
    .full(),
    row(
        "dora.art13.3",
        "13",
        "Art. 13(3)",
        "Lessons from incidents and threat-led tests fed into the risk assessment",
        2,
        Incident,
        Selector::Feed(Feed::Incident),
    )
    .full(),
    row(
        "dora.art13.5",
        "13",
        "Art. 13(5)",
        "Yearly report to the management body",
        2,
        Manual,
        Selector::Attestation,
    )
    .full(),
    row(
        "rts1774.art25",
        "rts1774",
        "RTS 2024/1774 Art. 25",
        "Continuity plan tests cover the required scenarios and content",
        2,
        Manual,
        Selector::Exercise {
            types: REHEARSAL,
            cited: true,
        },
    )
    .full(),
    // ── Art. 16(1) simplified framework and RTS 2024/1774 Title III ─────
    row(
        "dora.art16.1.a",
        "16",
        "Art. 16(1)(a)",
        "Sound and documented ICT risk management framework in place",
        2,
        Manual,
        Selector::Attestation,
    )
    .simplified(),
    row(
        "dora.art16.1.f",
        "16",
        "Art. 16(1)(f)",
        "Continuity ensured through backup and restoration measures",
        3,
        Test,
        Selector::Run { kinds: DR },
    )
    .simplified(),
    row(
        "dora.art16.1.g",
        "16",
        "Art. 16(1)(g)",
        "Plans and controls tested regularly",
        3,
        Test,
        Selector::Run { kinds: ANY_KIND },
    )
    .simplified(),
    row(
        "dora.art16.1.h",
        "16",
        "Art. 16(1)(h)",
        "Lessons from tests and incidents fed into training",
        1,
        Manual,
        Selector::Attestation,
    )
    .simplified(),
    row(
        "dora.art16.2",
        "16",
        "Art. 16(2)",
        "Simplified framework reviewed and the review recorded",
        2,
        Manual,
        Selector::Attestation,
    )
    .simplified(),
    row(
        "rts1774.art40",
        "rts1774",
        "RTS 2024/1774 Art. 40",
        "Backup and restore procedures tested at least once every year",
        5,
        Test,
        Selector::Run { kinds: DR },
    )
    .simplified(),
    // ── Chapter III: incidents (every profile) ──────────────────────────
    row(
        "dora.art17.3.a",
        "17",
        "Art. 17(3)(a)",
        "Early warning indicators in place",
        2,
        TelemetryClass,
        Selector::Feed(Feed::Slo),
    )
    .window(SLO),
    row(
        "dora.art18.1",
        "18",
        "Art. 18(1)",
        "ICT-related incidents classified",
        2,
        Incident,
        Selector::Feed(Feed::Incident),
    ),
    row(
        "dora.art19.4.a",
        "19",
        "Art. 19(4)(a)",
        "Initial notification of a major incident submitted",
        2,
        Incident,
        Selector::Feed(Feed::Incident),
    ),
    row(
        "dora.art19.4.b",
        "19",
        "Art. 19(4)(b)",
        "Intermediate report of a major incident submitted",
        2,
        Incident,
        Selector::Feed(Feed::Incident),
    ),
    row(
        "dora.art19.4.c",
        "19",
        "Art. 19(4)(c)",
        "Final report of a major incident submitted",
        2,
        Incident,
        Selector::Feed(Feed::Incident),
    ),
    // ── Chapter IV: testing ─────────────────────────────────────────────
    row(
        "dora.art24.1",
        "24",
        "Art. 24(1)",
        "Testing programme declared for the function",
        2,
        Test,
        Selector::Programme,
    )
    .micro(MicroRule::Exempt)
    .critical_important(),
    row(
        "dora.art24.3",
        "24",
        "Art. 24(3)",
        "Criticality assessed, so testing follows a risk-based approach",
        2,
        Test,
        Selector::FunctionAssessed,
    )
    .micro(MicroRule::Exempt)
    .critical_important(),
    row(
        "dora.art24.4",
        "24",
        "Art. 24(4)",
        "Tests undertaken by independent parties",
        2,
        Manual,
        Selector::Attestation,
    )
    .micro(MicroRule::Exempt)
    .critical_important(),
    row(
        "dora.art24.5",
        "24",
        "Art. 24(5)",
        "Issues revealed by tests remediated and re-tested",
        2,
        Test,
        Selector::Remediation,
    )
    .micro(MicroRule::Exempt)
    .critical_important(),
    row(
        "dora.art24.6",
        "24",
        "Art. 24(6)",
        "Systems supporting critical or important functions tested at least yearly",
        5,
        Test,
        Selector::Run { kinds: ANY_KIND },
    )
    .micro(MicroRule::Exempt)
    .critical_important(),
    row(
        "dora.art25.1",
        "25",
        "Art. 25(1)",
        "Testing programme spans more than one kind of test",
        2,
        Test,
        Selector::Disciplines { min: 2 },
    )
    .micro(MicroRule::Exempt),
    row(
        "dora.art25.2",
        "25",
        "Art. 25(2)",
        "Central securities depository or counterparty: vulnerability assessment before deployment",
        3,
        Test,
        Selector::Run { kinds: SECURITY },
    )
    .flagged(FlagRule::CsdOrCcp),
    row(
        "dora.art25.3",
        "25",
        "Art. 25(3)",
        "Microenterprise: planned tests executed within their planned interval",
        5,
        Test,
        Selector::PlanExecuted,
    )
    .micro(MicroRule::Only),
    row(
        "dora.art26.1",
        "26",
        "Art. 26(1)",
        "Threat-led penetration test carried out at least every three years",
        5,
        Manual,
        Selector::Exercise {
            types: PENTEST,
            cited: false,
        },
    )
    .full()
    .flagged(FlagRule::TlptDesignated)
    .window(THREE_YEARS)
    .critical_important(),
    // ── Chapter V: ICT third-party risk ─────────────────────────────────
    row(
        "dora.art28.2",
        "28",
        "Art. 28(2)",
        "Strategy on ICT third-party risk adopted and reviewed",
        2,
        Manual,
        Selector::Attestation,
    )
    .full()
    .micro(MicroRule::Exempt),
    row(
        "dora.art28.3",
        "28",
        "Art. 28(3)",
        "Register of information exported and reconciled",
        2,
        Manual,
        Selector::Feed(Feed::Register),
    ),
    row(
        "dora.art28.8",
        "28",
        "Art. 28(8)",
        "Exit plan tested",
        2,
        Manual,
        Selector::Exercise {
            types: REHEARSAL,
            cited: true,
        },
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, flags: &[&str]) -> EntityProfile {
        let flags: Vec<String> = flags.iter().map(|f| (*f).to_string()).collect();
        EntityProfile::parse(name, &flags).expect("valid profile")
    }

    fn ids(p: &EntityProfile) -> Vec<&'static str> {
        applicable(p).map(|r| r.id).collect()
    }

    /// Numeric DORA article of a row id (`dora.art24.6` → 24); `None` for
    /// rows of other instruments.
    fn dora_article(id: &str) -> Option<u32> {
        let rest = id.strip_prefix("dora.art")?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    #[test]
    fn registry_rows_are_well_formed_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for r in REQUIREMENTS {
            assert!(seen.insert(r.id), "duplicate requirement id {}", r.id);
            assert!(
                r.id.starts_with("dora.art") || r.id.starts_with("rts1774.art"),
                "unexpected id namespace: {}",
                r.id
            );
            assert!(!r.title.is_empty(), "title empty for {}", r.id);
            assert!(!r.cite.is_empty(), "cite empty for {}", r.id);
            assert!(
                matches!(r.weight, 1 | 2 | 3 | 5),
                "weight of {} must be 1, 2, 3 or 5",
                r.id
            );
            assert!(r.window_days > 0, "window missing for {}", r.id);
            assert!(
                article_title(r.article).is_some(),
                "article '{}' of {} has no title",
                r.article,
                r.id
            );
        }
    }

    #[test]
    fn golden_simplified_drops_arts_5_to_15_and_26_but_keeps_24_6() {
        let p = profile("simplified", &[]);
        let rows = ids(&p);
        for id in &rows {
            if let Some(n) = dora_article(id) {
                assert!(!(5..=15).contains(&n), "{id} must be n/a under simplified");
                assert_ne!(n, 26, "{id} must be n/a under simplified");
            }
        }
        for expected in [
            "dora.art24.6",
            "dora.art16.1.g",
            "rts1774.art40",
            "dora.art19.4.a",
            "dora.art28.8",
        ] {
            assert!(rows.contains(&expected), "{expected} must apply: {rows:?}");
        }
        assert!(
            !rows.contains(&"dora.art28.2"),
            "28(2) is n/a under simplified"
        );
    }

    #[test]
    fn golden_full_keeps_arts_5_to_15_and_drops_art_16_and_title_iii() {
        let p = profile("full", &[]);
        let rows = ids(&p);
        for expected in [
            "dora.art5.2",
            "dora.art11.6a",
            "dora.art11.6.switchover",
            "dora.art12.6",
            "dora.art13.5",
            "dora.art24.6",
            "dora.art28.2",
            "rts1774.art25",
            "dora.art10.1.spof",
            "dora.art28.3",
        ] {
            assert!(rows.contains(&expected), "{expected} must apply: {rows:?}");
        }
        for id in &rows {
            assert_ne!(dora_article(id), Some(16), "{id} is simplified-only");
            assert_ne!(dora_article(id), Some(26), "{id} needs designation");
        }
        assert!(!rows.contains(&"rts1774.art40"));
        assert!(
            !rows.contains(&"dora.art25.3"),
            "25(3) is microenterprise-only"
        );
    }

    #[test]
    fn golden_full_microenterprise_carve_outs() {
        let rows = ids(&profile("full", &["microenterprise"]));
        for carved in [
            "dora.art11.6.switchover",
            "dora.art11.7",
            "dora.art12.4",
            "dora.art24.1",
            "dora.art24.3",
            "dora.art24.4",
            "dora.art24.5",
            "dora.art24.6",
            "dora.art28.2",
        ] {
            assert!(!rows.contains(&carved), "{carved} is carved out: {rows:?}");
        }
        assert!(
            rows.contains(&"dora.art25.3"),
            "plan-based testing replaces 24(6)"
        );
        assert!(
            rows.contains(&"dora.art11.6a"),
            "11(6)(a) binds every function"
        );
    }

    #[test]
    fn golden_simplified_microenterprise_also_drops_art_24_rows() {
        let rows = ids(&profile("simplified", &["microenterprise"]));
        assert!(
            rows.iter().all(|id| !id.starts_with("dora.art24.")),
            "{rows:?}"
        );
        assert!(rows.contains(&"dora.art25.3"));
        assert!(rows.contains(&"rts1774.art40"));
    }

    #[test]
    fn tlpt_rows_apply_only_when_designated_with_a_three_year_window() {
        assert!(ids(&profile("full", &[]))
            .iter()
            .all(|id| !id.starts_with("dora.art26.")));
        let designated = profile("full", &["tlpt_designated"]);
        let tlpt: Vec<&Requirement> = applicable(&designated)
            .filter(|r| r.id.starts_with("dora.art26."))
            .collect();
        assert!(!tlpt.is_empty(), "designated entities carry Art. 26 rows");
        for r in tlpt {
            assert_eq!(r.evidence, EvidenceClass::Manual, "{}", r.id);
            assert_eq!(r.window_days, 1095, "{}", r.id);
        }
        // Art. 26 never binds an Art. 16(1) entity, designated or not.
        let simplified = profile("simplified", &["tlpt_designated"]);
        assert!(ids(&simplified)
            .iter()
            .all(|id| !id.starts_with("dora.art26.")));
    }

    #[test]
    fn sector_rows_follow_their_flags() {
        let plain = ids(&profile("full", &[]));
        for id in [
            "dora.art11.9",
            "dora.art12.5",
            "dora.art12.3.ccp",
            "dora.art25.2",
        ] {
            assert!(!plain.contains(&id), "{id} needs a sector flag");
        }
        let csd = ids(&profile("full", &["csd"]));
        assert!(csd.contains(&"dora.art11.9"));
        assert!(csd.contains(&"dora.art12.5"));
        assert!(csd.contains(&"dora.art25.2"));
        assert!(!csd.contains(&"dora.art12.3.ccp"));
        let ccp = ids(&profile("full", &["ccp"]));
        assert!(ccp.contains(&"dora.art12.3.ccp"));
        assert!(ccp.contains(&"dora.art25.2"));
        assert!(!ccp.contains(&"dora.art11.9"));
    }

    #[test]
    fn revisit_rows_carry_the_decided_class_scope_and_weight() {
        let row = |id: &str| requirement(id).unwrap_or_else(|| panic!("missing row {id}"));
        assert_eq!(row("dora.art24.6").weight, 5);
        assert_eq!(row("dora.art24.6").scope, FunctionScope::CriticalImportant);
        assert_eq!(row("dora.art24.6").window_days, 365);
        assert_eq!(row("dora.art11.6a").scope, FunctionScope::All);
        assert_eq!(
            row("dora.art11.6.switchover").scope,
            FunctionScope::CriticalImportant
        );
        assert_eq!(row("dora.art11.6.switchover").weight, 5);
        assert_eq!(row("dora.art12.6").weight, 5);
        assert_eq!(row("rts1774.art40").weight, 5);
        assert_eq!(row("rts1774.art40").window_days, 365);
        assert_eq!(row("dora.art16.1.g").window_days, 365);
        assert_eq!(row("dora.art13.3").evidence, EvidenceClass::Incident);
        assert_eq!(row("dora.art13.5").evidence, EvidenceClass::Manual);
        assert_eq!(row("dora.art28.8").evidence, EvidenceClass::Manual);
        assert_eq!(row("dora.art10.1.spof").selector, Selector::Topology);
        for id in ["dora.art19.4.a", "dora.art19.4.b", "dora.art19.4.c"] {
            assert_eq!(row(id).evidence, EvidenceClass::Incident, "{id}");
        }
    }

    #[test]
    fn expected_status_defaults_follow_weight_and_profile() {
        let full = profile("full", &[]);
        let micro = profile("full", &["microenterprise"]);
        let simplified = profile("simplified", &[]);
        let gating = requirement("dora.art12.6").unwrap();
        let core = requirement("dora.art11.4").unwrap();
        let normal = requirement("dora.art28.8").unwrap();
        assert_eq!(gating.expected_status(&full), EXPECTED_STATUS_OPERATING);
        assert_eq!(core.expected_status(&full), EXPECTED_STATUS_OPERATING);
        assert_eq!(normal.expected_status(&full), EXPECTED_STATUS_EXISTING);
        assert_eq!(core.expected_status(&micro), EXPECTED_STATUS_EXISTING);
        let restore = requirement("rts1774.art40").unwrap();
        assert_eq!(
            restore.expected_status(&simplified),
            EXPECTED_STATUS_EXISTING
        );
    }

    #[test]
    fn profile_parsing_rejects_unknown_names_and_flags() {
        assert!(EntityProfile::parse("partial", &[]).is_err());
        assert!(EntityProfile::parse("full", &["bank".to_string()]).is_err());
        let p = profile("simplified", &["microenterprise", "nis2"]);
        assert_eq!(p.profile, Profile::Simplified);
        assert!(p.flags.microenterprise && p.flags.nis2 && !p.flags.csd);
        assert_eq!(p.label(), "simplified, microenterprise");
        assert_eq!(profile("full", &[]).label(), "full");
        assert_eq!(p.flag_names(), vec!["microenterprise", "nis2"]);
    }

    #[test]
    fn every_row_belongs_to_a_listed_article() {
        for r in REQUIREMENTS {
            assert!(
                ARTICLES.iter().any(|(key, _)| *key == r.article),
                "{}",
                r.id
            );
        }
    }
}
