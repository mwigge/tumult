//! The function register as declared in `org.yaml`.
//!
//! Three optional top-level sections sit next to the org hierarchy:
//!
//! ```yaml
//! legal_entities:
//!   - lei: 529900T8BM49AURSDO55
//!     name: Demo Firm
//!     profile: simplified          # full | simplified
//!     flags: []                    # microenterprise, tlpt_designated, …
//!     feeds: [incident]            # evidence feeds that are connected
//! functions:
//!   - id: cif-0042                 # the ict.entity.id
//!     name: Checkout
//!     tier: critical               # critical | important | other
//!     licensed_activities: [payment-services]
//!     org_unit: platform/edge
//!     rto_s: 3600
//!     services: ["checkout*"]      # service.name globs
//! score: {}                        # house-parameter overrides
//! ```
//!
//! The declaration is validated as a whole and refused on the first
//! fault: a malformed or duplicate function id, a LEI that fails its
//! check digits, an unknown profile, flag, feed, tier or org unit. A
//! refused declaration changes nothing in the lake.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tumult_core::compliance::{check_entity_id, lei_is_valid, EntityProfile};
use tumult_lake::{IctFunction, IctFunctionService, IctLegalEntity, IctRegister, PlannedTest};

use super::model::parse_day;
use super::params::HouseParams;
use crate::org::OrgTree;

/// Licensed activity recorded for a function that names none.
pub const UNSPECIFIED_ACTIVITY: &str = "unspecified";
/// Mapping kind: an `ict.entity.id` value.
pub const MAP_ENTITY: &str = "entity";
/// Mapping kind: a `service.name` glob.
pub const MAP_SERVICE: &str = "service";
/// Feeds a legal entity can declare as connected. The first three decide
/// whether registry rows are observable; the last two are facts the
/// upper level gates read.
pub const FEED_NAMES: [&str; 5] = ["incident", "slo", "register", "alert_rules", "forecast"];
/// Tier names a function can carry.
pub const TIER_NAMES: [&str; 3] = ["critical", "important", "other"];

/// Why a declaration was refused.
#[derive(Debug)]
pub enum RegisterError {
    Io(std::io::Error),
    Yaml(String),
    Invalid(String),
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read the function register: {e}"),
            Self::Yaml(e) => write!(f, "cannot parse the function register: {e}"),
            Self::Invalid(e) => write!(f, "invalid function register: {e}"),
        }
    }
}

impl std::error::Error for RegisterError {}

fn invalid<T>(message: String) -> Result<T, RegisterError> {
    Err(RegisterError::Invalid(message))
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntitySpec {
    lei: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entity_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    competent_authority: Option<String>,
    profile: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    feeds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    registry_signed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    registry_signed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanSpec {
    kind: String,
    interval_days: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionSpec {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// May be left out when exactly one legal entity is declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lei: Option<String>,
    tier: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    licensed_activities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasons: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_assessed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rto_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rpo_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    impact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    org_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    declared: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    test_plan: Vec<PlanSpec>,
    /// `service.name` globs whose telemetry and tests belong to the
    /// function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    services: Vec<String>,
    /// Further `ict.entity.id` values that belong to the function; its
    /// own id always does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    entity_ids: Vec<String>,
    /// A function kept for its register code after it left service.
    #[serde(default, skip_serializing_if = "is_false")]
    retired: bool,
}

#[derive(Debug, Default, Deserialize)]
struct RegisterFile {
    #[serde(default)]
    legal_entities: Vec<EntitySpec>,
    #[serde(default)]
    functions: Vec<FunctionSpec>,
    #[serde(default)]
    score: Option<HouseParams>,
}

#[derive(Debug, Serialize)]
struct RegisterOut {
    legal_entities: Vec<EntitySpec>,
    functions: Vec<FunctionSpec>,
}

/// A validated declaration: the register rows and the house parameters.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Register {
    pub ict: IctRegister,
    pub params: HouseParams,
}

fn check_date(what: &str, date: Option<&str>) -> Result<(), RegisterError> {
    let Some(date) = date else {
        return Ok(());
    };
    if parse_day(date).is_none() {
        return invalid(format!("{what} '{date}' is not a YYYY-MM-DD date"));
    }
    Ok(())
}

fn check_entity(spec: &EntitySpec, seen: &mut BTreeSet<String>) -> Result<(), RegisterError> {
    if !lei_is_valid(&spec.lei) {
        return invalid(format!(
            "legal entity '{}': '{}' is not a valid LEI (20 upper-case alphanumerics with \
             MOD 97-10 check digits)",
            spec.name, spec.lei
        ));
    }
    if !seen.insert(spec.lei.clone()) {
        return invalid(format!("duplicate legal entity '{}'", spec.lei));
    }
    if let Err(e) = EntityProfile::parse(&spec.profile, &spec.flags) {
        return invalid(format!("legal entity '{}': {e}", spec.lei));
    }
    if let Some(feed) = spec
        .feeds
        .iter()
        .find(|f| !FEED_NAMES.contains(&f.as_str()))
    {
        return invalid(format!(
            "legal entity '{}': unknown feed '{feed}' ({})",
            spec.lei,
            FEED_NAMES.join("|")
        ));
    }
    check_date("registry_signed_at", spec.registry_signed_at.as_deref())
}

fn entity_row(spec: EntitySpec) -> IctLegalEntity {
    IctLegalEntity {
        lei: spec.lei,
        name: spec.name,
        country: spec.country,
        entity_type: spec.entity_type,
        competent_authority: spec.competent_authority,
        profile: spec.profile,
        flags: spec.flags,
        feeds: spec.feeds,
        registry_signed_by: spec.registry_signed_by,
        registry_signed_at: spec.registry_signed_at,
    }
}

/// The LEI a function belongs to: the one it names, or the only one
/// declared.
fn function_lei(spec: &FunctionSpec, leis: &BTreeSet<String>) -> Result<String, RegisterError> {
    if let Some(lei) = &spec.lei {
        if !leis.contains(lei) {
            return invalid(format!(
                "function '{}': legal entity '{lei}' is not declared",
                spec.id
            ));
        }
        return Ok(lei.clone());
    }
    let mut all = leis.iter();
    match (all.next(), all.next()) {
        (Some(only), None) => Ok(only.clone()),
        (None, _) => invalid(format!(
            "function '{}': no legal entity is declared",
            spec.id
        )),
        _ => invalid(format!(
            "function '{}': name its legal entity (lei), more than one is declared",
            spec.id
        )),
    }
}

fn check_function(spec: &FunctionSpec, org: &OrgTree) -> Result<(), RegisterError> {
    if let Err(e) = check_entity_id(&spec.id) {
        return invalid(format!("function id: {e}"));
    }
    if !TIER_NAMES.contains(&spec.tier.as_str()) {
        return invalid(format!(
            "function '{}': unknown tier '{}' ({})",
            spec.id,
            spec.tier,
            TIER_NAMES.join("|")
        ));
    }
    if let Some(unit) = spec
        .org_unit
        .as_deref()
        .filter(|u| org.resolve(u).is_none())
    {
        return invalid(format!(
            "function '{}': org unit '{unit}' is not in the hierarchy",
            spec.id
        ));
    }
    for (what, value) in [("rto_s", spec.rto_s), ("rpo_s", spec.rpo_s)] {
        if value.is_some_and(|v| !v.is_finite() || v < 0.0) {
            return invalid(format!(
                "function '{}': {what} must not be negative",
                spec.id
            ));
        }
    }
    if let Some(plan) = spec.test_plan.iter().find(|p| p.interval_days == 0) {
        return invalid(format!(
            "function '{}': test plan '{}' needs an interval of at least one day",
            spec.id, plan.kind
        ));
    }
    if let Some(e) = spec
        .entity_ids
        .iter()
        .find_map(|id| check_entity_id(id).err())
    {
        return invalid(format!("function '{}': {e}", spec.id));
    }
    let mut activities = BTreeSet::new();
    if let Some(twice) = spec
        .licensed_activities
        .iter()
        .find(|a| !activities.insert(*a))
    {
        return invalid(format!(
            "function '{}': licensed activity '{twice}' is listed twice",
            spec.id
        ));
    }
    check_date("last_assessed", spec.last_assessed.as_deref())
}

/// One register row per licensed activity of a function.
fn function_rows(spec: &FunctionSpec, lei: &str) -> Vec<IctFunction> {
    let unspecified = [UNSPECIFIED_ACTIVITY.to_string()];
    let activities: &[String] = if spec.licensed_activities.is_empty() {
        &unspecified
    } else {
        &spec.licensed_activities
    };
    activities
        .iter()
        .map(|activity| IctFunction {
            function_id: spec.id.clone(),
            register_code: String::new(),
            lei: lei.to_string(),
            name: spec.name.clone().unwrap_or_else(|| spec.id.clone()),
            licensed_activity: activity.clone(),
            tier: spec.tier.clone(),
            reasons: spec.reasons.clone(),
            last_assessed: spec.last_assessed.clone(),
            rto_s: spec.rto_s,
            rpo_s: spec.rpo_s,
            impact: spec.impact.clone(),
            org_unit: spec
                .org_unit
                .as_deref()
                .map(|unit| unit.trim_matches('/').to_string()),
            declared: spec.declared.clone(),
            test_plan: spec
                .test_plan
                .iter()
                .map(|plan| PlannedTest {
                    kind: plan.kind.clone(),
                    interval_days: plan.interval_days,
                })
                .collect(),
            active: !spec.retired,
        })
        .collect()
}

fn mapping_rows(spec: &FunctionSpec) -> Vec<IctFunctionService> {
    let row = |kind: &str, pattern: &String| IctFunctionService {
        function_id: spec.id.clone(),
        kind: kind.to_string(),
        pattern: pattern.clone(),
    };
    let entities = spec.entity_ids.iter().map(|id| row(MAP_ENTITY, id));
    let services = spec.services.iter().map(|glob| row(MAP_SERVICE, glob));
    entities.chain(services).collect()
}

fn build(file: RegisterFile, org: &OrgTree) -> Result<Register, RegisterError> {
    let params = file.score.unwrap_or_default();
    params.validate().map_err(RegisterError::Invalid)?;
    let mut leis = BTreeSet::new();
    for spec in &file.legal_entities {
        check_entity(spec, &mut leis)?;
    }
    let mut ict = IctRegister::default();
    let mut ids = BTreeSet::new();
    for spec in &file.functions {
        check_function(spec, org)?;
        if !ids.insert(spec.id.as_str()) {
            return invalid(format!("duplicate function id '{}'", spec.id));
        }
        let lei = function_lei(spec, &leis)?;
        ict.functions.extend(function_rows(spec, &lei));
        ict.services.extend(mapping_rows(spec));
    }
    ict.legal_entities = file.legal_entities.into_iter().map(entity_row).collect();
    Ok(Register { ict, params })
}

/// Parse and validate the register sections of an org file.
///
/// # Errors
/// Returns `Yaml` when the text does not parse and `Invalid` on the first
/// fault of the declaration.
pub fn parse(text: &str) -> Result<Register, RegisterError> {
    let org = OrgTree::from_yaml(text).map_err(|e| RegisterError::Invalid(e.to_string()))?;
    let file: Option<RegisterFile> =
        serde_yaml::from_str(text).map_err(|e| RegisterError::Yaml(e.to_string()))?;
    build(file.unwrap_or_default(), &org)
}

/// Load the register from an org file. A file that does not exist
/// declares nothing.
///
/// # Errors
/// Returns `Io` when the file exists and cannot be read, otherwise as
/// [`parse`].
pub fn load(path: &Path) -> Result<Register, RegisterError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Register::default()),
        Err(e) => Err(RegisterError::Io(e)),
    }
}

fn entity_spec(row: &IctLegalEntity) -> EntitySpec {
    EntitySpec {
        lei: row.lei.clone(),
        name: row.name.clone(),
        country: row.country.clone(),
        entity_type: row.entity_type.clone(),
        competent_authority: row.competent_authority.clone(),
        profile: row.profile.clone(),
        flags: row.flags.clone(),
        feeds: row.feeds.clone(),
        registry_signed_by: row.registry_signed_by.clone(),
        registry_signed_at: row.registry_signed_at.clone(),
    }
}

fn function_spec(row: &IctFunction, register: &IctRegister) -> FunctionSpec {
    let patterns = |kind: &str| {
        register
            .services
            .iter()
            .filter(|s| s.function_id == row.function_id && s.kind == kind)
            .map(|s| s.pattern.clone())
            .collect()
    };
    let activities: Vec<String> = register
        .functions
        .iter()
        .filter(|f| f.function_id == row.function_id)
        .map(|f| f.licensed_activity.clone())
        .filter(|activity| activity != UNSPECIFIED_ACTIVITY)
        .collect();
    FunctionSpec {
        id: row.function_id.clone(),
        name: Some(row.name.clone()),
        lei: Some(row.lei.clone()),
        tier: row.tier.clone(),
        licensed_activities: activities,
        reasons: row.reasons.clone(),
        last_assessed: row.last_assessed.clone(),
        rto_s: row.rto_s,
        rpo_s: row.rpo_s,
        impact: row.impact.clone(),
        org_unit: row.org_unit.clone(),
        declared: row.declared.clone(),
        test_plan: row
            .test_plan
            .iter()
            .map(|plan| PlanSpec {
                kind: plan.kind.clone(),
                interval_days: plan.interval_days,
            })
            .collect(),
        services: patterns(MAP_SERVICE),
        entity_ids: patterns(MAP_ENTITY),
        retired: !row.active,
    }
}

/// The `legal_entities` and `functions` sections that declare `register`,
/// as YAML. Register codes are not part of a declaration: the store
/// assigns them.
///
/// # Errors
/// Returns `Yaml` when the register cannot be serialised.
pub fn to_yaml(register: &IctRegister) -> Result<String, RegisterError> {
    let mut seen = BTreeSet::new();
    let out = RegisterOut {
        legal_entities: register.legal_entities.iter().map(entity_spec).collect(),
        functions: register
            .functions
            .iter()
            .filter(|row| seen.insert(row.function_id.as_str()))
            .map(|row| function_spec(row, register))
            .collect(),
    };
    serde_yaml::to_string(&out).map_err(|e| RegisterError::Yaml(e.to_string()))
}

/// `register` with every store-assigned code blanked, for comparing a
/// stored register with a declaration.
#[must_use]
pub fn without_codes(register: &IctRegister) -> IctRegister {
    let mut copy = register.clone();
    for function in &mut copy.functions {
        function.register_code.clear();
    }
    copy
}

/// Register codes per function id, in the order stored.
#[must_use]
pub fn codes_by_function(register: &IctRegister) -> BTreeMap<String, Vec<String>> {
    let mut codes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in &register.functions {
        codes
            .entry(row.function_id.clone())
            .or_default()
            .push(row.register_code.clone());
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEI: &str = "529900T8BM49AURSDO55";
    const OTHER_LEI: &str = "506700GE1G29325QX363";

    const ORG: &str = "\
nodes:
  - {name: platform, kind: domain}
  - {name: edge, kind: unit, parent: platform}
legal_entities:
  - lei: 529900T8BM49AURSDO55
    name: Demo Firm
    country: SE
    entity_type: investment firm
    competent_authority: Finansinspektionen
    profile: simplified
    flags: [nis2]
    feeds: [incident]
    registry_signed_by: compliance owner
    registry_signed_at: 2026-11-13
functions:
  - id: cif-0042
    name: Checkout
    tier: critical
    licensed_activities: [payment-services, e-money]
    reasons: customer payments
    last_assessed: 2026-09-01
    rto_s: 14400
    impact: high
    org_unit: platform/edge
    declared: [dora.art28.8]
    test_plan:
      - {kind: dr, interval_days: 365}
    services: ['checkout*', payments-gw]
    entity_ids: [cif-0042-eu]
  - id: ops-0003
    tier: other
score:
  operating_days: 90
";

    fn fault(text: &str) -> String {
        match parse(text) {
            Err(RegisterError::Invalid(message)) => message,
            other => panic!("expected an invalid register, got {other:?}"),
        }
    }

    #[test]
    fn a_declaration_becomes_register_rows() {
        let register = parse(ORG).unwrap();
        assert_eq!(register.params.operating_days, 90);
        assert_eq!(register.params.existing_threshold, 0.75);
        assert_eq!(register.ict.legal_entities.len(), 1);
        assert_eq!(register.ict.legal_entities[0].flags, ["nis2"]);

        let rows = &register.ict.functions;
        let keys: Vec<(&str, &str)> = rows
            .iter()
            .map(|f| (f.function_id.as_str(), f.licensed_activity.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                ("cif-0042", "payment-services"),
                ("cif-0042", "e-money"),
                ("ops-0003", UNSPECIFIED_ACTIVITY),
            ]
        );
        assert!(rows.iter().all(|f| f.lei == LEI && f.active));
        assert_eq!(rows[0].rto_s, Some(14_400.0));
        assert_eq!(rows[0].org_unit.as_deref(), Some("platform/edge"));
        assert_eq!(rows[0].test_plan[0].interval_days, 365);
        assert_eq!(rows[2].name, "ops-0003");

        let mapping: Vec<(&str, &str)> = register
            .ict
            .services
            .iter()
            .map(|s| (s.kind.as_str(), s.pattern.as_str()))
            .collect();
        assert_eq!(
            mapping,
            [
                (MAP_ENTITY, "cif-0042-eu"),
                (MAP_SERVICE, "checkout*"),
                (MAP_SERVICE, "payments-gw"),
            ]
        );
    }

    #[test]
    fn a_register_survives_the_trip_to_yaml_and_back() {
        let declared = parse(ORG).unwrap().ict;
        let yaml = to_yaml(&declared).unwrap();
        let nodes = "nodes:\n  - {name: platform, kind: domain}\n  - {name: edge, kind: unit, parent: platform}\n";
        let again = parse(&format!("{nodes}{yaml}")).unwrap().ict;
        assert_eq!(again, declared);
    }

    #[test]
    fn an_org_file_without_register_sections_declares_nothing() {
        let register = parse("nodes:\n  - {name: platform, kind: domain}\n").unwrap();
        assert_eq!(register, Register::default());
        assert_eq!(parse("").unwrap(), Register::default());
    }

    #[test]
    fn a_missing_file_declares_nothing_and_a_present_one_is_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("org.yaml");
        assert_eq!(load(&path).unwrap(), Register::default());
        std::fs::write(&path, ORG).unwrap();
        assert_eq!(load(&path).unwrap().ict.functions.len(), 3);
    }

    #[test]
    fn duplicate_function_ids_are_rejected() {
        let text = ORG.replace("id: ops-0003", "id: cif-0042");
        assert_eq!(fault(&text), "duplicate function id 'cif-0042'");
    }

    #[test]
    fn a_malformed_function_id_is_rejected_not_repaired() {
        let message = fault(&ORG.replace("id: cif-0042\n", "id: CF-0042\n"));
        assert!(message.contains("'CF-0042' is malformed"), "{message}");
        let message = fault(&ORG.replace("[cif-0042-eu]", "[Cif_0042]"));
        assert!(message.contains("'Cif_0042' is malformed"), "{message}");
    }

    #[test]
    fn a_lei_must_pass_its_check_digits_and_be_unique() {
        let message = fault(&ORG.replace(LEI, "529900T8BM49AURSDO56"));
        assert!(message.contains("not a valid LEI"), "{message}");
        let twice = ORG.replace(
            "functions:",
            &format!("  - {{lei: {LEI}, name: Twin, profile: full}}\nfunctions:"),
        );
        assert_eq!(fault(&twice), format!("duplicate legal entity '{LEI}'"));
    }

    #[test]
    fn a_function_names_its_legal_entity_when_there_are_several() {
        let two = ORG.replace(
            "functions:",
            &format!("  - {{lei: {OTHER_LEI}, name: Other, profile: full}}\nfunctions:"),
        );
        assert!(fault(&two).contains("name its legal entity"));
        let named = two
            .replace("tier: critical", &format!("tier: critical\n    lei: {LEI}"))
            .replace("tier: other", &format!("tier: other\n    lei: {OTHER_LEI}"));
        let register = parse(&named).unwrap();
        assert_eq!(register.ict.functions[2].lei, OTHER_LEI);
        let unknown = named.replace(
            &format!("lei: {OTHER_LEI}\nscore"),
            "lei: 5493001KJTIIGC8Y1R12\nscore",
        );
        assert!(fault(&unknown).contains("is not declared"));
    }

    #[test]
    fn functions_without_a_legal_entity_are_rejected() {
        let message = fault("functions:\n  - {id: cif-0001, tier: critical}\n");
        assert!(message.contains("no legal entity is declared"), "{message}");
    }

    #[test]
    fn unknown_names_are_rejected() {
        let cases = [
            (
                "profile: simplified",
                "profile: light",
                "unknown profile 'light'",
            ),
            (
                "flags: [nis2]",
                "flags: [bank]",
                "unknown entity flag 'bank'",
            ),
            (
                "feeds: [incident]",
                "feeds: [pager]",
                "unknown feed 'pager'",
            ),
            ("tier: other", "tier: high", "unknown tier 'high'"),
            (
                "org_unit: platform/edge",
                "org_unit: platform/core",
                "org unit 'platform/core'",
            ),
            (
                "last_assessed: 2026-09-01",
                "last_assessed: last week",
                "not a YYYY-MM-DD date",
            ),
            ("rto_s: 14400", "rto_s: -1", "rto_s must not be negative"),
            ("interval_days: 365", "interval_days: 0", "at least one day"),
            (
                "[payment-services, e-money]",
                "[e-money, e-money]",
                "listed twice",
            ),
            ("operating_days: 90", "operating_days: 0", "operating_days"),
        ];
        for (from, to, expected) in cases {
            let message = fault(&ORG.replace(from, to));
            assert!(message.contains(expected), "{to}: {message}");
        }
    }

    #[test]
    fn unknown_fields_are_a_parse_error() {
        let text = ORG.replace("tier: other", "tier: other\n    rto: 5");
        assert!(matches!(parse(&text), Err(RegisterError::Yaml(_))));
    }

    #[test]
    fn a_retired_function_is_kept_inactive() {
        let text = ORG.replace("tier: other", "tier: other\n    retired: true");
        let register = parse(&text).unwrap();
        assert!(!register.ict.functions[2].active);
        let yaml = to_yaml(&register.ict).unwrap();
        assert!(yaml.contains("retired: true"));
    }

    #[test]
    fn codes_are_listed_per_function_and_blanked_for_comparison() {
        let mut stored = parse(ORG).unwrap().ict;
        for (n, row) in stored.functions.iter_mut().enumerate() {
            row.register_code = format!("F{}", n + 1);
        }
        let codes = codes_by_function(&stored);
        assert_eq!(codes["cif-0042"], ["F1", "F2"]);
        assert_eq!(codes["ops-0003"], ["F3"]);
        assert_eq!(without_codes(&stored), parse(ORG).unwrap().ict);
    }

    fn sorted(mut register: IctRegister) -> IctRegister {
        register.legal_entities.sort_by(|a, b| a.lei.cmp(&b.lei));
        register.functions.sort_by(|a, b| {
            (&a.function_id, &a.licensed_activity).cmp(&(&b.function_id, &b.licensed_activity))
        });
        register.services.sort_by(|a, b| {
            (&a.function_id, &a.kind, &a.pattern).cmp(&(&b.function_id, &b.kind, &b.pattern))
        });
        register
    }

    #[test]
    fn the_org_file_round_trips_through_the_lake() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let declared = parse(ORG).unwrap().ict;
        store
            .writer()
            .unwrap()
            .sync_ict_register(&declared, 1)
            .unwrap();

        let stored = store.read_only().unwrap().ict_register().unwrap();
        assert_eq!(sorted(without_codes(&stored)), sorted(declared.clone()));

        // The stored register declares the same structure again, next to
        // the hierarchy its org units live in.
        let hierarchy = ORG.split("legal_entities:").next().unwrap();
        let exported = format!("{hierarchy}{}", to_yaml(&stored).unwrap());
        let again = parse(&exported).unwrap().ict;
        assert_eq!(sorted(again), sorted(declared));
    }

    #[test]
    fn register_codes_survive_a_second_sync_and_new_rows_number_on() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        writer
            .sync_ict_register(&parse(ORG).unwrap().ict, 1)
            .unwrap();
        let first = codes_by_function(&writer.ict_register().unwrap());
        assert_eq!(first["cif-0042"], ["F1", "F2"]);
        assert_eq!(first["ops-0003"], ["F3"]);

        // A function declared ahead of the others must not renumber them.
        let grown = ORG.replace(
            "functions:\n",
            "functions:\n  - id: cif-0007\n    tier: important\n",
        );
        writer
            .sync_ict_register(&parse(&grown).unwrap().ict, 2)
            .unwrap();
        let second = codes_by_function(&writer.ict_register().unwrap());
        assert_eq!(second["cif-0042"], first["cif-0042"]);
        assert_eq!(second["ops-0003"], first["ops-0003"]);
        assert_eq!(second["cif-0007"], ["F4"]);
    }
}
