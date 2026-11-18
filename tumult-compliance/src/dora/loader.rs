//! Reads the lake into the score inputs of one legal entity.
//!
//! What is read, and how it is bound to a function:
//!
//! - **Runs**: every `resilience.experiment` root span up to the as-of
//!   time, with its completion status and measured recovery. A run
//!   belongs to the function its `ict.entity.id` names, else to the
//!   function whose service globs match its target.
//! - **Manual records**: verified, conclusive rows of
//!   `manual_experiments`. A record with no target speaks for the whole
//!   legal entity; its `framework_refs` are the requirement ids it cites.
//! - **Services**: every `service.name` that emitted spans, with the
//!   conformance level its telemetry reached inside the conformance
//!   window. A service glob of the register that matches nothing becomes a
//!   silent service: declared, never seen, and scored at the floor.
//! - **Feeds**: a feed counts only through records of it stored in the
//!   lake. The lake has no ingest path for incidents, SLOs, the register
//!   of information, alert rules or forecasts, so every feed is not
//!   connected, every row that needs one is unknown, and no gate fact
//!   that depends on one holds. Listing a feed in the register is a
//!   declaration: it is reported ([`FeedState`]) and never scored.
//! - **Snapshots**: rows of `requirement_status_daily` stored by the
//!   daily job on the day they describe. Only these count toward the
//!   Operating streak: a day without a stored row, or stored as unknown,
//!   ends it, and nothing is reconstructed from the time of the
//!   evidence. A caller scoped to some environments gets no stored rows
//!   at all (they were computed over every environment), so nothing it
//!   sees is Operating.
//!
//! Without a registered function the loader stands in one unregistered
//! function per `ict.entity.id` seen in telemetry, so an unregistered
//! deployment is scored (at the bottom) rather than hidden.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;
use tumult_core::compliance::{entity_id_is_valid, truncated, EntityProfile};
use tumult_core::types::canonical_kind;
use tumult_lake::{
    scope, IctFunction, IctFunctionService, IctLegalEntity, IctRegister, Reader, RunVerdict,
};

use super::model::{
    day_of, parse_day, Evidence, Facts, Function, IncidentFeed, Inputs, Outcome, PlannedTest,
    Service, SloFeed, Snapshots, Source, Tier, Workload, DAY_NS,
};
use super::params::HouseParams;
use super::register::{FEED_NAMES, MAP_ENTITY, MAP_SERVICE};
use crate::org::glob_match;

/// Source of a status row computed on its own day.
pub const SNAPSHOT_SOURCE: &str = "snapshot";
/// Source of a day the proof tool stored for a past date. Such a day was
/// not scored on its own day, so it counts for nothing unless the daemon
/// was started with the proof opt-in, and then every response says so.
pub const PROOF_SOURCE: &str = "proof";
/// Shown with every document while proof days are in use.
pub const PROOF_DAYS_NOTE: &str =
    "proof days are in use: days the proof tool stored for past dates (source 'proof') count \
     as stored days. This is a test setting; the Operating status and the history shown here \
     are not evidence of operation.";

/// Whether a stored day of `source` counts: one the daily job stored on
/// its own day always, one of the proof tool only with the opt-in.
#[must_use]
pub fn day_counts(source: &str, proof_days: bool) -> bool {
    source == SNAPSHOT_SOURCE || (proof_days && source == PROOF_SOURCE)
}

/// What a load did with the days of the proof tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProofDays {
    /// The opt-in is set: proof days count as stored days.
    pub enabled: bool,
    /// Proof days of the Operating window that were counted.
    pub counted: usize,
    /// Proof days of the window left out because the opt-in is not set.
    pub ignored: usize,
}
/// Days of stored snapshots that count as "two quarters" of posture
/// history for the level-5 gate.
pub const TWO_QUARTERS_DAYS: i64 = 182;
/// Most status rows one load reads. A deployment of 200 functions and
/// 150 requirements stores 30 000 rows a day, so the 183-day window is
/// 5.5 million rows there; the limit keeps one request from reading
/// more than about a sixth of that. Over the limit the oldest days are
/// left out, which shortens a streak and never lengthens one.
pub const STATUS_ROW_LIMIT: usize = 1_000_000;
/// Most bytes of a value from a file or a sender that one log event
/// carries.
pub const LOG_VALUE_MAX_LEN: usize = 128;
/// Discipline of an automated run that names none.
pub const DEFAULT_RUN_KIND: &str = "chaos";
/// Name shown for the stand-in legal entity of a deployment that has
/// registered none.
pub const UNREGISTERED_NAME: &str = "(no legal entity registered)";

/// Why a feed is not connected: nothing of it is stored in the lake.
pub const FEED_NO_INGEST: &str =
    "the lake has no ingest path for this feed, so no record of it is stored";

/// Why the inputs could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// A lake query failed.
    Store(String),
    /// The requested LEI is not registered.
    UnknownLei(String),
    /// The requested profile is not a profile.
    Profile(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "lake: {e}"),
            Self::UnknownLei(lei) => write!(f, "legal entity '{lei}' is not registered"),
            Self::Profile(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadError {}

fn store(e: impl fmt::Display) -> LoadError {
    LoadError::Store(e.to_string())
}

/// What to load.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// Environments the caller may see; empty means all.
    pub envs: &'a [String],
    /// The legal entity; `None` selects the first one registered.
    pub lei: Option<&'a str>,
    /// Score under this profile instead of the registered one.
    pub profile: Option<&'a str>,
    pub as_of_ns: i64,
    /// Workloads from the deployment manifests.
    pub workloads: &'a [Workload],
    /// Count the days of the proof tool ([`PROOF_SOURCE`]) as stored
    /// days. Off unless the daemon was started with the opt-in.
    pub proof_days: bool,
}

/// The inputs of one legal entity with the register rows behind them.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    /// The legal entity scored (a stand-in when none is registered).
    pub entity: IctLegalEntity,
    /// Every registered legal entity, for the selector.
    pub entities: Vec<IctLegalEntity>,
    /// Active register rows of the entity, one per licensed activity.
    pub register: Vec<IctFunction>,
    /// Every feed the score knows, with what the lake holds of it.
    pub feeds: Vec<FeedState>,
    /// The request scored under another profile than the registered one.
    pub profile_overridden: bool,
    /// The caller is limited to some environments: only the functions
    /// its telemetry binds to are loaded, and no register data beyond
    /// the entity scored.
    pub scoped: bool,
    /// What became of the days the proof tool stored.
    pub proof: ProofDays,
    pub inputs: Inputs,
}

/// The stored days one load read.
#[derive(Default)]
struct StoredDays {
    values: Snapshots,
    /// The first counted day inside the two-quarter look-back.
    since: Option<i64>,
    proof: ProofDays,
}

/// One evidence feed as the score reads it. `declared` is what the
/// register says; `connected` is what the lake holds, and only that is
/// scored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedState {
    pub name: &'static str,
    pub declared: bool,
    pub connected: bool,
    /// Why the feed is not connected.
    pub reason: Option<&'static str>,
}

/// The state of every feed for `entity`. No feed has an ingest path, so
/// none is connected whatever the register declares.
#[must_use]
pub fn feed_states(entity: &IctLegalEntity) -> Vec<FeedState> {
    FEED_NAMES
        .iter()
        .map(|name| FeedState {
            name,
            declared: entity.feeds.iter().any(|feed| feed == name),
            connected: false,
            reason: Some(FEED_NO_INGEST),
        })
        .collect()
}

fn text(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn int(row: &Value, key: &str) -> Option<i64> {
    row.get(key).and_then(Value::as_i64)
}

fn num(row: &Value, key: &str) -> Option<f64> {
    row.get(key).and_then(Value::as_f64)
}

/// A statement with the values bound to its `?`, in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Sql {
    text: String,
    binds: Vec<String>,
}

fn stand_in_entity() -> IctLegalEntity {
    IctLegalEntity {
        name: UNREGISTERED_NAME.to_string(),
        profile: "full".to_string(),
        ..IctLegalEntity::default()
    }
}

fn select_entity(register: &IctRegister, lei: Option<&str>) -> Result<IctLegalEntity, LoadError> {
    let Some(lei) = lei else {
        return Ok(register
            .legal_entities
            .first()
            .cloned()
            .unwrap_or_else(stand_in_entity));
    };
    register
        .legal_entities
        .iter()
        .find(|e| e.lei == lei)
        .cloned()
        .ok_or_else(|| LoadError::UnknownLei(lei.to_string()))
}

fn profile_of(entity: &IctLegalEntity, profile: Option<&str>) -> Result<EntityProfile, LoadError> {
    EntityProfile::parse(profile.unwrap_or(&entity.profile), &entity.flags)
        .map_err(LoadError::Profile)
}

fn plan(row: &IctFunction) -> Vec<PlannedTest> {
    row.test_plan
        .iter()
        .map(|p| PlannedTest {
            kind: canonical_kind(&p.kind).to_string(),
            interval_days: p.interval_days,
        })
        .collect()
}

/// One function per function id, in register order, carrying the
/// register code of each licensed activity.
fn registered_functions(rows: &[IctFunction]) -> Vec<Function> {
    let mut functions: Vec<Function> = Vec::new();
    for row in rows {
        if let Some(known) = functions.iter_mut().find(|f| f.id == row.function_id) {
            known.register_codes.push(row.register_code.clone());
            continue;
        }
        functions.push(Function {
            name: row.name.clone(),
            rto_s: row.rto_s,
            rpo_s: row.rpo_s,
            last_assessed_ns: row
                .last_assessed
                .as_deref()
                .and_then(parse_day)
                .map(|day| day * DAY_NS),
            org_unit: row.org_unit.clone(),
            declared: row.declared.clone(),
            test_plan: plan(row),
            register_codes: vec![row.register_code.clone()],
            ..Function::new(&row.function_id, Tier::parse(&row.tier))
        });
    }
    functions
}

/// Stand-in functions for a deployment with no register: one per
/// `ict.entity.id` seen, at the tier its telemetry claims.
fn unregistered_functions(rows: &[&Value]) -> Vec<Function> {
    let mut tiers: BTreeMap<String, Tier> = BTreeMap::new();
    for row in rows {
        let Some(id) = text(row, "entity").filter(|id| entity_id_is_valid(id)) else {
            continue;
        };
        let tier = text(row, "tier").map_or(Tier::Other, |t| Tier::parse(&t));
        let held = tiers.entry(id).or_insert(tier);
        *held = (*held).min(tier);
    }
    tiers
        .into_iter()
        .map(|(id, tier)| Function {
            registered: false,
            ..Function::new(&id, tier)
        })
        .collect()
}

/// Binds telemetry identity to functions.
#[derive(Debug, Default)]
struct Mapper {
    /// `ict.entity.id` value → function.
    entity: Vec<(String, String)>,
    /// `service.name` glob → function.
    service: Vec<(String, String)>,
}

impl Mapper {
    fn new<'f>(ids: impl Iterator<Item = &'f str>, mappings: &[IctFunctionService]) -> Self {
        let ids: BTreeSet<&str> = ids.collect();
        let mut mapper = Self::default();
        for id in &ids {
            mapper.entity.push(((*id).to_string(), (*id).to_string()));
        }
        for row in mappings
            .iter()
            .filter(|m| ids.contains(m.function_id.as_str()))
        {
            let pair = (row.pattern.clone(), row.function_id.clone());
            match row.kind.as_str() {
                MAP_ENTITY => mapper.entity.push(pair),
                MAP_SERVICE => mapper.service.push(pair),
                other => tracing::warn!(
                    "resilience.function.id" = ?truncated(&row.function_id, LOG_VALUE_MAX_LEN),
                    "resilience.mapping.kind" = ?truncated(other, LOG_VALUE_MAX_LEN),
                    "unknown function mapping kind skipped"
                ),
            }
        }
        mapper
    }

    fn by_entity(&self, id: &str) -> BTreeSet<String> {
        self.entity
            .iter()
            .filter(|(value, _)| value == id)
            .map(|(_, function)| function.clone())
            .collect()
    }

    fn by_service(&self, name: &str) -> BTreeSet<String> {
        self.service
            .iter()
            .filter(|(glob, _)| glob_match(glob, name))
            .map(|(_, function)| function.clone())
            .collect()
    }

    /// Functions behind a piece of telemetry: the one its entity id
    /// names, else those whose globs match one of its names.
    fn functions_of(&self, entity: Option<&str>, names: &[Option<&str>]) -> Vec<String> {
        let named = entity.map(|id| self.by_entity(id)).unwrap_or_default();
        if !named.is_empty() {
            return named.into_iter().collect();
        }
        let mut matched = BTreeSet::new();
        for name in names.iter().flatten() {
            matched.extend(self.by_entity(name));
            matched.extend(self.by_service(name));
        }
        matched.into_iter().collect()
    }
}

/// Outcome of a completion status; `None` for a run that proves nothing
/// either way. A run takes the outcome of its verdict
/// ([`tumult_lake::run_verdict`]), a manual record that of its outcome. A
/// run with no completion record counts as failed.
fn outcome_of(status: Option<&str>) -> Option<Outcome> {
    let status = status.map(str::to_ascii_lowercase);
    match status.as_deref() {
        Some("passed") => Some(Outcome::Passed),
        Some("partial") => Some(Outcome::Partial),
        Some("inconclusive") => None,
        run => match run.and_then(tumult_lake::run_verdict) {
            Some(RunVerdict::Pass) => Some(Outcome::Passed),
            Some(RunVerdict::NoEvidence) => None,
            Some(RunVerdict::Fail) | None => Some(Outcome::Failed),
        },
    }
}

/// The runs up to `as_of_ns`, with or without a trace: the rows of
/// `run_roots`, as the scorecard reads them.
fn runs_sql(as_of_ns: i64, envs: &[String]) -> String {
    let scope = scope::and(scope::in_list("s.target_environment", envs));
    let status = tumult_lake::run_outcome_sql("s", "l");
    format!(
        "SELECT COALESCE(s.experiment_id, s.span_id) AS id, s.experiment_name AS name, \
         s.ts_ns AS ts, s.target_system AS system, s.target_environment AS env, \
         s.span_attrs['resilience.target.name'] AS target_name, \
         s.span_attrs['resilience.target.component'] AS component, \
         COALESCE(s.span_attrs['ict.entity.id'], s.resource_attrs['ict.entity.id']) AS entity, \
         COALESCE(s.span_attrs['ict.entity.tier'], s.resource_attrs['ict.entity.tier']) AS tier, \
         COALESCE(e.test_kind, s.span_attrs['resilience.test.kind']) AS kind, \
         {status} AS status, \
         e.rto_target_s AS rto_target_s, e.rpo_target_s AS rpo_target_s, \
         COALESCE(e.rto_measured_s, s.recovery_time_s) AS rto_measured_s, \
         e.rpo_measured_s AS rpo_measured_s \
         FROM run_roots s \
         LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id \
         LEFT JOIN experiments e ON e.experiment_id = s.experiment_id \
         WHERE s.experiment_name IS NOT NULL \
         AND s.ts_ns <= {as_of_ns}{scope} \
         ORDER BY s.ts_ns, id"
    )
}

fn manual_sql(as_of_ns: i64, envs: &[String]) -> String {
    let scope = scope::and(scope::in_list("m.target_environment", envs));
    format!(
        "SELECT m.id AS id, m.experiment_name AS name, m.exercise_type AS exercise_type, \
         m.executed_at_ns AS ts, m.outcome_status AS status, m.target_system AS system, \
         m.target_environment AS env, m.recovery_time_s AS rto_measured_s, \
         m.framework_refs AS refs \
         FROM manual_experiments m \
         WHERE m.status = 'verified' AND m.outcome_status <> 'inconclusive' \
         AND m.executed_at_ns <= {as_of_ns}{scope} \
         ORDER BY m.executed_at_ns, m.id"
    )
}

/// SQL predicate: the span carries the contract key. The key comes from
/// the org file, so it is bound: pushed onto `binds` once per `?`.
fn carries(key: &str, binds: &mut Vec<String>) -> String {
    let promoted = match key {
        "resilience.target.system" => Some("target_system"),
        "resilience.target.technology" => Some("target_technology"),
        "resilience.target.environment" => Some("target_environment"),
        "service.version" => Some("service_version"),
        _ => None,
    };
    if let Some(column) = promoted {
        return format!("{column} IS NOT NULL");
    }
    binds.extend([key.to_string(), key.to_string()]);
    "(list_contains(map_keys(resource_attrs), ?) OR list_contains(map_keys(span_attrs), ?))"
        .to_string()
}

/// SQL expression: the attribute `key`, from the resource or the span.
/// The key is written into the statement, so it is a compile-time name
/// of letters, digits, dots and underscores and nothing else; a value
/// that comes from a file or a request is bound instead ([`Sql`]).
fn attr(key: &'static str) -> String {
    let plain = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_');
    assert!(plain, "attribute key is not a plain name");
    format!("COALESCE(resource_attrs['{key}'], span_attrs['{key}'])")
}

/// The first nanosecond of a window of `days` ending at `as_of_ns`.
/// A window too long to count in nanoseconds opens at the start of time.
fn window_start(as_of_ns: i64, days: u32) -> i64 {
    i64::from(days)
        .checked_mul(DAY_NS)
        .and_then(|span| as_of_ns.checked_sub(span))
        .unwrap_or(i64::MIN)
}

fn services_sql(as_of_ns: i64, envs: &[String], params: &HouseParams) -> Sql {
    let mut binds = Vec::new();
    let from_ns = window_start(as_of_ns, params.conformance.window_days);
    let scope = scope::and(scope::in_list("target_environment", envs));
    let levels: Vec<String> = params
        .conformance
        .levels()
        .iter()
        .enumerate()
        .map(|(i, keys)| {
            let all: Vec<String> = keys.iter().map(|key| carries(key, &mut binds)).collect();
            let all = if all.is_empty() {
                "TRUE".to_string()
            } else {
                all.join(" AND ")
            };
            format!(
                "COUNT(*) FILTER (WHERE ts_ns >= {from_ns} AND {all}) AS l{}",
                i + 1
            )
        })
        .collect();
    // The level filters are the only placeholders, in the order pushed.
    let text = format!(
        "SELECT service_name AS name, MIN(ts_ns) AS first_seen, MAX(ts_ns) AS last_seen, \
         COUNT(*) FILTER (WHERE ts_ns >= {from_ns}) AS spans, {levels}, \
         arg_max(target_system, ts_ns) AS system, \
         arg_max(target_environment, ts_ns) AS env, \
         arg_max({component}, ts_ns) AS component, \
         arg_max({entity}, ts_ns) AS entity, \
         arg_max({tier}, ts_ns) AS tier \
         FROM spans \
         WHERE service_name <> '' AND experiment_id IS NULL \
         AND span_name NOT LIKE 'resilience.%' AND ts_ns <= {as_of_ns}{scope} \
         GROUP BY 1 ORDER BY 1",
        levels = levels.join(", "),
        component = attr("resilience.target.component"),
        entity = attr("ict.entity.id"),
        tier = attr("ict.entity.tier"),
    );
    Sql { text, binds }
}

fn topology_sql(as_of_ns: i64, envs: &[String], params: &HouseParams) -> String {
    let from_ns = window_start(as_of_ns, params.conformance.window_days);
    let scope = scope::and(scope::in_list("c.target_environment", envs));
    format!(
        "SELECT COUNT(*) AS edges FROM ( \
         SELECT 1 AS edge FROM spans c \
         INNER JOIN spans p ON p.trace_id = c.trace_id AND p.span_id = c.parent_span_id \
         WHERE c.service_name <> p.service_name \
         AND c.ts_ns >= {from_ns} AND c.ts_ns <= {as_of_ns}{scope} \
         LIMIT 1)"
    )
}

fn schedules_sql(envs: &[String]) -> String {
    let scope = scope::and(scope::in_list("env", envs));
    format!(
        "SELECT target AS target FROM run_schedules \
         WHERE enabled AND target IS NOT NULL{scope} ORDER BY 1"
    )
}

/// Conformance level 0–4: the highest level whose keys, and those of
/// every level below it, were on every span of the window.
fn conformance_level(row: &Value, levels: usize) -> u8 {
    let spans = int(row, "spans").unwrap_or(0);
    if spans == 0 {
        return 0;
    }
    let mut level = 0;
    for i in 1..=levels {
        if int(row, &format!("l{i}")) != Some(spans) {
            break;
        }
        level += 1;
    }
    level
}

fn service_of(row: &Value, functions: Vec<String>, params: &HouseParams) -> Option<Service> {
    let name = text(row, "name")?;
    let level = conformance_level(row, params.conformance.levels().len());
    Some(Service {
        name,
        level,
        functions,
        observed: int(row, "spans").unwrap_or(0) > 0,
        first_seen_ns: int(row, "first_seen"),
        last_seen_ns: int(row, "last_seen"),
        env: text(row, "env"),
        system: text(row, "system"),
        component: text(row, "component"),
    })
}

fn run_of(row: &Value, functions: Vec<String>) -> Option<Evidence> {
    let id = text(row, "id")?;
    let outcome = outcome_of(text(row, "status").as_deref())?;
    let system = text(row, "system");
    let target = text(row, "target_name");
    let mut services: Vec<String> = system.iter().chain(target.iter()).cloned().collect();
    services.dedup();
    Some(Evidence {
        source: Source::Run,
        functions,
        services,
        system,
        component: text(row, "component"),
        env: text(row, "env"),
        experiment: text(row, "name").unwrap_or_else(|| id.clone()),
        kind: Some(text(row, "kind").map_or_else(
            || DEFAULT_RUN_KIND.to_string(),
            |kind| canonical_kind(&kind).to_string(),
        )),
        outcome,
        rto_target_s: num(row, "rto_target_s"),
        rto_measured_s: num(row, "rto_measured_s"),
        rpo_target_s: num(row, "rpo_target_s"),
        rpo_measured_s: num(row, "rpo_measured_s"),
        ..Evidence::run(&id, "", "", int(row, "ts")?)
    })
}

fn cited(row: &Value) -> Vec<String> {
    row.get("refs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn manual_of(row: &Value, functions: Vec<String>, params: &HouseParams) -> Option<Evidence> {
    let id = text(row, "id")?;
    let outcome = outcome_of(text(row, "status").as_deref())?;
    let exercise = text(row, "exercise_type").unwrap_or_default();
    let system = text(row, "system");
    Some(Evidence {
        functions,
        services: system.iter().cloned().collect(),
        system,
        env: text(row, "env"),
        experiment: text(row, "name").unwrap_or_else(|| id.clone()),
        kind: params.manual_kinds.get(&exercise).cloned(),
        cited: cited(row),
        outcome,
        rto_measured_s: num(row, "rto_measured_s"),
        ..Evidence::manual(&id, &[], &exercise, int(row, "ts")?)
    })
}

/// Reads one legal entity's inputs through a lake reader.
struct Loader<'a> {
    reader: &'a Reader,
    request: Request<'a>,
    params: &'a HouseParams,
    /// Binds telemetry to the functions of the entity being scored.
    own: Mapper,
    /// Binds telemetry to the functions of the other legal entities.
    foreign: Mapper,
}

impl Loader<'_> {
    fn query(&self, sql: &str) -> Result<Vec<Value>, LoadError> {
        self.reader.query_json_rows(sql).map_err(store)
    }

    /// Functions of the entity behind a row; `None` when the row belongs
    /// to another legal entity.
    fn bind(&self, entity: Option<&str>, names: &[Option<&str>]) -> Option<Vec<String>> {
        let functions = self.own.functions_of(entity, names);
        if functions.is_empty() && !self.foreign.functions_of(entity, names).is_empty() {
            return None;
        }
        Some(functions)
    }

    fn runs(&self, rows: &[Value]) -> Vec<Evidence> {
        let mut seen = BTreeSet::new();
        let mut runs = Vec::new();
        for row in rows {
            let names = [text(row, "system"), text(row, "target_name")];
            let names = [names[0].as_deref(), names[1].as_deref()];
            let Some(functions) = self.bind(text(row, "entity").as_deref(), &names) else {
                continue;
            };
            let Some(run) = run_of(row, functions) else {
                continue;
            };
            if seen.insert(run.id.clone()) {
                runs.push(run);
            }
        }
        runs
    }

    fn manual(&self) -> Result<Vec<Evidence>, LoadError> {
        let rows = self.query(&manual_sql(self.request.as_of_ns, self.request.envs))?;
        let mut records = Vec::new();
        for row in &rows {
            let target = text(row, "system");
            let functions = match target.as_deref() {
                None => Some(Vec::new()),
                Some(name) => self
                    .bind(Some(name), &[Some(name)])
                    .filter(|functions| !functions.is_empty()),
            };
            // A record that names a target no function of the entity
            // owns says nothing about the entity.
            let Some(functions) = functions else {
                continue;
            };
            records.extend(manual_of(row, functions, self.params));
        }
        Ok(records)
    }

    fn services(&self, rows: &[Value]) -> Vec<Service> {
        let mut services = Vec::new();
        for row in rows {
            let name = text(row, "name");
            let Some(functions) = self.bind(text(row, "entity").as_deref(), &[name.as_deref()])
            else {
                continue;
            };
            services.extend(service_of(row, functions, self.params));
        }
        if !self.request.envs.is_empty() {
            // A declared name with no telemetry is register data the
            // scoped caller has no view of.
            return services;
        }
        for (glob, function) in &self.own.service {
            if services.iter().any(|s| glob_match(glob, &s.name)) {
                continue;
            }
            match services.iter_mut().find(|s| s.name == *glob) {
                Some(silent) => silent.functions.push(function.clone()),
                None => services.push(Service::silent(glob, &[function.as_str()])),
            }
        }
        services
    }

    fn scheduled(&self, functions: &mut [Function]) -> Result<(), LoadError> {
        for row in self.query(&schedules_sql(self.request.envs))? {
            let Some(target) = text(&row, "target") else {
                continue;
            };
            let bound = self.own.functions_of(Some(&target), &[Some(&target)]);
            for function in functions.iter_mut().filter(|f| bound.contains(&f.id)) {
                function.scheduled = true;
            }
        }
        Ok(())
    }

    /// Facts the gates read, each from what the lake holds. Alert
    /// rules, forecasts and the register of information have no ingest
    /// path, so their facts never hold.
    fn facts(&self, stored_since: Option<i64>) -> Result<Facts, LoadError> {
        let request = &self.request;
        let edges = self.query(&topology_sql(request.as_of_ns, request.envs, self.params))?;
        let today = day_of(request.as_of_ns);
        Ok(Facts {
            alert_rules_exported: false,
            forecasts_with_provenance: false,
            posture_delta_two_quarters: stored_since
                .is_some_and(|first| today - first >= TWO_QUARTERS_DAYS),
            register_connected: false,
            register_reconciled_ns: None,
            topology_observed: edges
                .first()
                .and_then(|row| int(row, "edges"))
                .is_some_and(|n| n > 0),
        })
    }

    /// Stored snapshot values of the Operating window, and the first day
    /// a snapshot was stored inside the two-quarter look-back. A day of
    /// the proof tool is read only with the opt-in, and counted either
    /// way so the response can say what was done with it.
    fn snapshots(&self) -> Result<StoredDays, LoadError> {
        let today = day_of(self.request.as_of_ns);
        let back = i64::from(self.params.operating_days).max(TWO_QUARTERS_DAYS);
        let rows = self
            .reader
            .requirement_status_between(today - back - 1, today, STATUS_ROW_LIMIT)
            .map_err(store)?;
        let enabled = self.request.proof_days;
        let mut stored = StoredDays::default();
        let mut proof_days = BTreeSet::new();
        for row in rows {
            if row.as_of_day > today {
                continue;
            }
            if row.source == PROOF_SOURCE {
                proof_days.insert(row.as_of_day);
            }
            if !day_counts(&row.source, enabled) {
                continue;
            }
            let first = stored
                .since
                .map_or(row.as_of_day, |day| day.min(row.as_of_day));
            stored.since = Some(first);
            stored
                .values
                .entry((row.function_id, row.requirement_id))
                .or_default()
                .insert(row.as_of_day, row.value);
        }
        stored.proof = ProofDays {
            enabled,
            counted: if enabled { proof_days.len() } else { 0 },
            ignored: if enabled { 0 } else { proof_days.len() },
        };
        Ok(stored)
    }
}

/// Read the inputs of one legal entity.
///
/// A caller limited to some environments that names a legal entity it
/// has no function of gets `UnknownLei`, the answer an unregistered LEI
/// gets, whatever profile it asks for: the register is not its to probe.
///
/// # Errors
/// `UnknownLei` and `Profile` for a request that names something that
/// does not exist, `Store` when a lake query fails.
pub fn load(
    reader: &Reader,
    request: Request<'_>,
    params: &HouseParams,
) -> Result<Loaded, LoadError> {
    let loaded = load_entity(reader, request, params);
    let (false, Some(lei)) = (request.envs.is_empty(), request.lei) else {
        return loaded;
    };
    let hidden = || LoadError::UnknownLei(lei.to_string());
    let in_scope = |loaded: &Loaded| !loaded.inputs.functions.is_empty();
    match loaded {
        Ok(loaded) if in_scope(&loaded) => Ok(loaded),
        Ok(_) => Err(hidden()),
        Err(LoadError::Profile(refused)) => {
            // A refused profile is said only for an entity in scope,
            // which is judged under the registered profile.
            let registered = Request {
                profile: None,
                ..request
            };
            match load_entity(reader, registered, params) {
                Ok(loaded) if in_scope(&loaded) => Err(LoadError::Profile(refused)),
                Err(LoadError::Store(e)) => Err(LoadError::Store(e)),
                _ => Err(hidden()),
            }
        }
        Err(e) => Err(e),
    }
}

fn load_entity(
    reader: &Reader,
    request: Request<'_>,
    params: &HouseParams,
) -> Result<Loaded, LoadError> {
    let register = reader.ict_register().map_err(store)?;
    let entity = select_entity(&register, request.lei)?;
    let profile = profile_of(&entity, request.profile)?;
    let rows: Vec<IctFunction> = register
        .functions
        .iter()
        .filter(|f| f.active && f.lei == entity.lei)
        .cloned()
        .collect();
    let query = |sql: &str| reader.query_json_rows(sql).map_err(store);
    let run_rows = query(&runs_sql(request.as_of_ns, request.envs))?;
    let services = services_sql(request.as_of_ns, request.envs, params);
    let binds: Vec<&str> = services.binds.iter().map(String::as_str).collect();
    let service_rows = reader
        .query_json_rows_bound(&services.text, &binds)
        .map_err(store)?;
    let mut functions = registered_functions(&rows);
    if register.functions.is_empty() {
        let seen: Vec<&Value> = run_rows.iter().chain(service_rows.iter()).collect();
        functions = unregistered_functions(&seen);
    }
    let own_ids = functions.iter().map(|f| f.id.as_str());
    let foreign_ids = register
        .functions
        .iter()
        .filter(|f| f.active && f.lei != entity.lei)
        .map(|f| f.function_id.as_str());
    let loader = Loader {
        reader,
        request,
        params,
        own: Mapper::new(own_ids, &register.services),
        foreign: Mapper::new(foreign_ids, &register.services),
    };
    loader.scheduled(&mut functions)?;
    let stored = if request.envs.is_empty() {
        loader.snapshots()?
    } else {
        StoredDays {
            proof: ProofDays {
                enabled: request.proof_days,
                ..ProofDays::default()
            },
            ..StoredDays::default()
        }
    };
    let mut evidence = loader.runs(&run_rows);
    evidence.extend(loader.manual()?);
    let inputs = Inputs {
        functions,
        evidence,
        services: loader.services(&service_rows),
        workloads: request.workloads.to_vec(),
        slos: SloFeed::NotConnected,
        incidents: IncidentFeed::NotConnected,
        facts: loader.facts(stored.since)?,
        snapshots: stored.values,
        ..Inputs::empty(request.as_of_ns, profile)
    };
    let profile_overridden = request.profile.is_some()
        && profile_of(&entity, None).map_or(true, |own| own.label() != profile.label());
    let loaded = Loaded {
        feeds: feed_states(&entity),
        entity,
        entities: register.legal_entities,
        register: rows,
        profile_overridden,
        scoped: false,
        proof: stored.proof,
        inputs,
    };
    if request.envs.is_empty() {
        return Ok(loaded);
    }
    Ok(scoped(loaded))
}

/// What a caller limited to some environments may have of `loaded`: the
/// functions its own telemetry and evidence bind to, and of the register
/// only the entity scored, without its sign-off and register codes.
fn scoped(mut loaded: Loaded) -> Loaded {
    let inputs = &mut loaded.inputs;
    let mut visible: BTreeSet<String> = BTreeSet::new();
    for evidence in &inputs.evidence {
        visible.extend(evidence.functions.iter().cloned());
    }
    for service in &inputs.services {
        visible.extend(service.functions.iter().cloned());
    }
    inputs.functions.retain(|f| visible.contains(&f.id));
    for function in &mut inputs.functions {
        function.register_codes.clear();
    }
    inputs.workloads.clear();
    loaded.entity.registry_signed_by = None;
    loaded.entity.registry_signed_at = None;
    loaded.entities = vec![loaded.entity.clone()];
    loaded.register.clear();
    loaded.scoped = true;
    loaded
}

#[cfg(test)]
mod tests {
    use tumult_lake::{LogRow, SpanRow, Store};

    use super::super::register;
    use super::*;

    const LEI: &str = "529900T8BM49AURSDO55";
    const NOW: i64 = 1_800_000_000 * 1_000_000_000;

    const ORG: &str = "\
nodes:
  - {name: platform, kind: domain}
legal_entities:
  - lei: 529900T8BM49AURSDO55
    name: Demo Firm
    profile: simplified
    feeds: [incident, register]
functions:
  - id: cif-0001
    name: Payments
    tier: critical
    org_unit: platform
    rto_s: 3600
    last_assessed: 2026-10-01
    licensed_activities: [payment-services, e-money]
    test_plan:
      - kind: dr
        interval_days: 365
    services: [\"payments-*\", \"ledger\"]
  - id: cif-0002
    name: Reporting
    tier: important
    entity_ids: [cif-0902]
";

    fn store_with(org: &str) -> (tempfile::TempDir, Store) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let declared = register::parse(org).unwrap();
        store
            .writer()
            .unwrap()
            .sync_ict_register(&declared.ict, NOW - DAY_NS)
            .unwrap();
        (dir, store)
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// An application span of `service` with the given resource attributes.
    fn app_span(service: &str, ts: i64, resource: &[(&str, &str)]) -> SpanRow {
        SpanRow {
            ts_ns: ts,
            trace_id: format!("t-{service}-{ts}"),
            span_id: format!("s-{service}-{ts}"),
            span_name: "GET /".into(),
            span_kind: "Server".into(),
            service_name: service.into(),
            resource_attrs: pairs(resource),
            events: "[]".into(),
            ..SpanRow::default()
        }
    }

    /// The root span of a run and its completion record.
    fn run(id: &str, target: &str, kind: &str, status: &str, ts: i64) -> (SpanRow, LogRow) {
        let span = SpanRow {
            ts_ns: ts,
            trace_id: format!("t-{id}"),
            span_id: format!("s-{id}"),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            service_name: "tumult".into(),
            experiment_id: Some(id.into()),
            experiment_name: Some(format!("{target}-{kind}")),
            target_system: Some(target.into()),
            target_environment: Some("prod".into()),
            span_attrs: pairs(&[("resilience.test.kind", kind)]),
            events: "[]".into(),
            ..SpanRow::default()
        };
        let log = LogRow {
            ts_ns: ts,
            severity_text: "INFO".into(),
            body: "experiment.completed".into(),
            service_name: "tumult".into(),
            log_attrs: pairs(&[("experiment_id", id), ("status", status)]),
            ..LogRow::default()
        };
        (span, log)
    }

    fn seed(store: &Store, spans: &[SpanRow], logs: &[LogRow]) {
        let writer = store.writer().unwrap();
        writer.insert_spans(spans).unwrap();
        writer.insert_logs(logs).unwrap();
    }

    fn request<'a>(envs: &'a [String]) -> Request<'a> {
        Request {
            envs,
            lei: None,
            profile: None,
            as_of_ns: NOW,
            workloads: &[],
            proof_days: false,
        }
    }

    fn loaded(store: &Store, request: Request<'_>) -> Loaded {
        let reader = store.read_only().unwrap();
        load(&reader, request, &HouseParams::default()).unwrap()
    }

    const SPINE: [(&str, &str); 4] = [
        ("ict.schema.version", "1"),
        ("ict.entity.id", "cif-0001"),
        ("ict.entity.tier", "critical"),
        ("ict.entity.owner", "payments"),
    ];

    fn spine_span(service: &str, ts: i64) -> SpanRow {
        SpanRow {
            target_system: Some("payments".into()),
            target_technology: Some("rust".into()),
            target_environment: Some("prod".into()),
            ..app_span(service, ts, &SPINE)
        }
    }

    /// (id, functions, kind, cited rows) of one loaded evidence row.
    type Seen<'a> = (&'a str, Vec<String>, Option<&'a str>, Vec<String>);

    #[test]
    fn the_register_becomes_functions_with_their_codes() {
        let (_dir, store) = store_with(ORG);
        let loaded = loaded(&store, request(&[]));
        assert_eq!(loaded.entity.lei, LEI);
        assert_eq!(loaded.inputs.entity.label(), "simplified");
        let ids: Vec<&str> = loaded
            .inputs
            .functions
            .iter()
            .map(|f| f.id.as_str())
            .collect();
        assert_eq!(ids, ["cif-0001", "cif-0002"]);
        let payments = &loaded.inputs.functions[0];
        assert_eq!(payments.tier, Tier::Critical);
        assert_eq!(payments.register_codes, ["F1", "F2"]);
        assert_eq!(payments.org_unit.as_deref(), Some("platform"));
        assert_eq!(payments.rto_s, Some(3600.0));
        assert_eq!(
            payments.last_assessed_ns,
            parse_day("2026-10-01").map(|d| d * DAY_NS)
        );
        assert_eq!(payments.test_plan.len(), 1);
        assert!(payments.registered);
        assert_eq!(loaded.register.len(), 3, "one row per licensed activity");
    }

    #[test]
    fn a_declared_glob_that_matches_nothing_is_a_silent_service() {
        let (_dir, store) = store_with(ORG);
        seed(&store, &[spine_span("payments-api", NOW - DAY_NS)], &[]);
        let loaded = loaded(&store, request(&[]));
        let names: Vec<(&str, bool)> = loaded
            .inputs
            .services
            .iter()
            .map(|s| (s.name.as_str(), s.observed))
            .collect();
        assert_eq!(names, [("payments-api", true), ("ledger", false)]);
        assert_eq!(loaded.inputs.services[1].functions, ["cif-0001"]);
        assert_eq!(loaded.inputs.services[1].last_seen_ns, None);
    }

    #[test]
    fn conformance_needs_every_key_on_every_span_of_the_window() {
        let (_dir, store) = store_with(ORG);
        let mut versioned = spine_span("payments-api", NOW - DAY_NS);
        versioned.service_version = Some("1.4.0".into());
        versioned
            .resource_attrs
            .push(("resilience.target.component".into(), "api".into()));
        let bare = app_span("payments-web", NOW - DAY_NS, &[]);
        let mut half = spine_span("payments-batch", NOW - DAY_NS);
        half.span_id = "s-half".into();
        let old = spine_span("payments-old", NOW - 200 * DAY_NS);
        seed(
            &store,
            &[
                versioned,
                bare,
                spine_span("payments-batch", NOW - 2 * DAY_NS),
                SpanRow {
                    target_technology: None,
                    ..half
                },
                old,
            ],
            &[],
        );
        let loaded = loaded(&store, request(&[]));
        let level = |name: &str| {
            let service = loaded
                .inputs
                .services
                .iter()
                .find(|s| s.name == name)
                .unwrap();
            (service.level, service.observed)
        };
        assert_eq!(level("payments-api"), (2, true));
        assert_eq!(level("payments-web"), (0, true));
        assert_eq!(level("payments-batch"), (0, true), "one span lacks a key");
        assert_eq!(level("payments-old"), (0, false), "outside the window");
        let api = &loaded.inputs.services[0];
        assert_eq!(
            (
                api.system.as_deref(),
                api.component.as_deref(),
                api.env.as_deref()
            ),
            (Some("payments"), Some("api"), Some("prod"))
        );
    }

    #[test]
    fn the_status_of_a_run_is_read_in_either_spelling_and_from_its_root_span() {
        let (_dir, store) = store_with(ORG);
        let (before, before_log) = run("r1", "payments-db", "dr", "Completed", NOW - 3 * DAY_NS);
        let (mut since, _) = run("r2", "payments-db", "dr", "", NOW - 2 * DAY_NS);
        since.outcome_status = Some("completed".into());
        let (mut halted, _) = run("r3", "payments-db", "dr", "", NOW - DAY_NS);
        halted.outcome_status = Some("halted".into());
        seed(&store, &[before, since, halted], &[before_log]);

        let loaded = loaded(&store, request(&[]));

        let seen: Vec<(&str, Outcome)> = loaded
            .inputs
            .evidence
            .iter()
            .map(|e| (e.id.as_str(), e.outcome))
            .collect();
        assert_eq!(
            seen,
            [
                ("r1", Outcome::Passed),
                ("r2", Outcome::Passed),
                ("r3", Outcome::Failed),
            ]
        );
    }

    #[test]
    fn a_run_that_sent_no_trace_is_evidence_like_a_run_that_did() {
        use tumult_core::types::{Experiment, ExperimentStatus, ExperimentTarget, Journal};
        let (_dir, store) = store_with(ORG);
        let (traced, traced_log) = run("r1", "payments-db", "dr", "Completed", NOW - 2 * DAY_NS);
        seed(&store, &[traced], &[traced_log]);
        // A plain `tumult run` against the same system: the lake holds its
        // journal and no span.
        let journal = Journal {
            started_at_ns: NOW - DAY_NS,
            ended_at_ns: NOW - DAY_NS + 300_000_000_000,
            ..tumult_lake::duckdb_store::sample_journal("r2", ExperimentStatus::Deviated)
        };
        let definition = Experiment {
            title: journal.experiment_title.clone(),
            target: Some(ExperimentTarget {
                system: "payments-db".into(),
                technology: None,
                environment: Some("prod".into()),
                criticality: None,
                namespace: None,
                version: None,
            }),
            ..Experiment::default()
        };
        assert!(store
            .writer()
            .unwrap()
            .ingest_journal(&journal, Some(&definition))
            .unwrap());

        let loaded = loaded(&store, request(&[]));

        let seen: Vec<(&str, Outcome, Vec<String>)> = loaded
            .inputs
            .evidence
            .iter()
            .map(|e| (e.id.as_str(), e.outcome, e.functions.clone()))
            .collect();
        let payments = vec!["cif-0001".to_string()];
        assert_eq!(
            seen,
            [
                ("r1", Outcome::Passed, payments.clone()),
                ("r2", Outcome::Failed, payments),
            ]
        );
    }

    #[test]
    fn a_run_with_two_completion_logs_is_one_piece_of_evidence() {
        let (_dir, store) = store_with(ORG);
        let (span, first) = run("r1", "payments-db", "dr", "Deviated", NOW - DAY_NS);
        let again = LogRow {
            ts_ns: first.ts_ns + 1,
            log_attrs: pairs(&[("experiment_id", "r1"), ("status", "Completed")]),
            ..first.clone()
        };
        // The later log is stored first.
        seed(&store, &[span], &[again, first]);

        let loaded = loaded(&store, request(&[]));

        let seen: Vec<(&str, Outcome)> = loaded
            .inputs
            .evidence
            .iter()
            .map(|e| (e.id.as_str(), e.outcome))
            .collect();
        assert_eq!(
            seen,
            [("r1", Outcome::Passed)],
            "the later log says how it ended"
        );
    }

    #[test]
    fn runs_bind_by_entity_id_then_by_target_glob() {
        let (_dir, store) = store_with(ORG);
        let (by_glob, glob_log) = run("r1", "payments-db", "dr", "completed", NOW - 3 * DAY_NS);
        let (mut by_id, id_log) = run("r2", "warehouse", "load", "failed", NOW - 2 * DAY_NS);
        by_id
            .span_attrs
            .push(("ict.entity.id".into(), "cif-0902".into()));
        let (nobody, nobody_log) = run("r3", "elsewhere", "chaos", "completed", NOW - DAY_NS);
        let (dropped, dropped_log) = run("r4", "ledger", "chaos", "aborted", NOW - DAY_NS);
        let (missing, _) = run("r5", "ledger", "chaos", "completed", NOW - DAY_NS);
        seed(
            &store,
            &[by_glob, by_id, nobody, dropped, missing],
            &[glob_log, id_log, nobody_log, dropped_log],
        );
        let loaded = loaded(&store, request(&[]));
        let seen: Vec<(&str, Vec<String>, Outcome, Option<&str>)> = loaded
            .inputs
            .evidence
            .iter()
            .map(|e| {
                (
                    e.id.as_str(),
                    e.functions.clone(),
                    e.outcome,
                    e.kind.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (
                    "r1",
                    vec!["cif-0001".to_string()],
                    Outcome::Passed,
                    Some("dr")
                ),
                (
                    "r2",
                    vec!["cif-0002".to_string()],
                    Outcome::Failed,
                    Some("performance")
                ),
                ("r3", Vec::new(), Outcome::Passed, Some("chaos")),
                (
                    "r5",
                    vec!["cif-0001".to_string()],
                    Outcome::Failed,
                    Some("chaos")
                ),
            ],
            "an aborted run is no evidence; a run with no completion record failed"
        );
        assert_eq!(loaded.inputs.evidence[0].services, ["payments-db"]);
        assert_eq!(loaded.inputs.evidence[0].env.as_deref(), Some("prod"));
    }

    #[test]
    fn scoped_callers_see_only_their_environments() {
        let (_dir, store) = store_with(ORG);
        let (prod, prod_log) = run("r1", "payments-db", "dr", "completed", NOW - DAY_NS);
        let (mut staging, staging_log) = run("r2", "payments-db", "dr", "completed", NOW - DAY_NS);
        staging.target_environment = Some("staging".into());
        seed(&store, &[prod, staging], &[prod_log, staging_log]);
        let envs = vec!["staging".to_string()];
        let loaded = loaded(&store, request(&envs));
        let ids: Vec<&str> = loaded
            .inputs
            .evidence
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(ids, ["r2"]);
    }

    #[test]
    fn a_declared_feed_with_no_ingested_record_stays_not_connected() {
        let org = ORG.replace(
            "feeds: [incident, register]",
            "feeds: [incident, slo, register, alert_rules, forecast]",
        );
        let (_dir, store) = store_with(&org);
        let loaded = loaded(&store, request(&[]));
        assert_eq!(loaded.inputs.incidents, IncidentFeed::NotConnected);
        assert_eq!(loaded.inputs.slos, SloFeed::NotConnected);
        let facts = &loaded.inputs.facts;
        assert!(!facts.register_connected);
        assert_eq!(facts.register_reconciled_ns, None);
        assert!(!facts.alert_rules_exported && !facts.forecasts_with_provenance);
        assert!(!facts.topology_observed && !facts.posture_delta_two_quarters);
        let declared: Vec<(&str, bool, bool)> = loaded
            .feeds
            .iter()
            .map(|feed| (feed.name, feed.declared, feed.connected))
            .collect();
        assert_eq!(
            declared,
            [
                ("incident", true, false),
                ("slo", true, false),
                ("register", true, false),
                ("alert_rules", true, false),
                ("forecast", true, false),
            ]
        );
        assert!(loaded
            .feeds
            .iter()
            .all(|f| f.reason == Some(FEED_NO_INGEST)));
    }

    /// A window no validated file can ask for still gives a statement
    /// and not an overflow: the window opens at the start of time.
    #[test]
    fn a_window_longer_than_time_opens_at_the_start_of_time() {
        let mut params = HouseParams::default();
        params.conformance.window_days = u32::MAX;
        let sql = services_sql(NOW, &[], &params).text;
        assert!(sql.contains(&format!("ts_ns >= {}", i64::MIN)), "{sql}");
        let sql = topology_sql(NOW, &[], &params);
        assert!(sql.contains(&format!("c.ts_ns >= {}", i64::MIN)), "{sql}");
        params.conformance.window_days = 90;
        let sql = services_sql(NOW, &[], &params).text;
        assert!(
            sql.contains(&format!("ts_ns >= {}", NOW - 90 * DAY_NS)),
            "{sql}"
        );
    }

    /// A conformance key comes from the org file. It is bound to the
    /// statement, never written into it, and is matched as it stands.
    #[test]
    fn a_conformance_key_from_the_org_file_is_bound_not_written() {
        const KEY: &str = "it's.a') OR TRUE --";
        let mut params = HouseParams::default();
        params.conformance.level1 = vec![KEY.to_string(), "service.version".to_string()];
        let sql = services_sql(NOW, &[], &params);
        assert!(!sql.text.contains("it's"), "{}", sql.text);
        assert!(!sql.text.contains("it''s"), "{}", sql.text);
        assert_eq!(sql.binds.iter().filter(|b| *b == KEY).count(), 2, "{sql:?}");
        assert_eq!(
            sql.text.matches('?').count(),
            sql.binds.len(),
            "one value per placeholder"
        );

        let (_dir, store) = store_with(ORG);
        let mut with = app_span("payments-api", NOW - DAY_NS, &[(KEY, "1")]);
        with.service_version = Some("1.0".into());
        let mut without = app_span("ledger", NOW - DAY_NS, &[]);
        without.service_version = Some("1.0".into());
        seed(&store, &[with, without], &[]);
        let reader = store.read_only().unwrap();
        let loaded = load(&reader, request(&[]), &params).unwrap();
        let level = |name: &str| {
            let service = loaded.inputs.services.iter().find(|s| s.name == name);
            service.unwrap_or_else(|| panic!("{name} is loaded")).level
        };
        assert_eq!(level("payments-api"), 1);
        assert_eq!(level("ledger"), 0);
    }

    /// A caller limited to some environments learns nothing of a legal
    /// entity it has no telemetry of: naming its LEI gives the answer an
    /// unregistered one gives, whatever else the request asks for.
    #[test]
    fn a_scoped_request_for_an_entity_out_of_scope_is_an_unknown_lei() {
        let (_dir, store) = store_with(TWO_FIRMS);
        let mut staging = spine_span("payments-api", NOW - DAY_NS);
        staging.target_environment = Some("staging".into());
        seed(&store, &[staging], &[]);
        let reader = store.read_only().unwrap();
        let params = HouseParams::default();
        let envs = vec!["staging".to_string()];
        const OTHER: &str = "506700GE1G29325QX363";
        const NOWHERE: &str = "5493001KJTIIGC8Y1R12";
        let ask = |lei: &'static str, profile: Option<&'static str>| {
            let request = Request {
                lei: Some(lei),
                profile,
                ..request(&envs)
            };
            load(&reader, request, &params).map(|loaded| loaded.entity.lei)
        };
        for profile in [None, Some("full"), Some("no-such-profile")] {
            let unknown = ask(NOWHERE, profile).unwrap_err();
            assert_eq!(unknown, LoadError::UnknownLei(NOWHERE.into()));
            let hidden = ask(OTHER, profile).unwrap_err();
            assert_eq!(hidden, LoadError::UnknownLei(OTHER.into()), "{profile:?}");
            assert_eq!(
                hidden.to_string().replace(OTHER, "x"),
                unknown.to_string().replace(NOWHERE, "x")
            );
        }
        // Its own entity answers, and a bad profile is still said there.
        assert_eq!(ask(LEI, None), Ok(LEI.to_string()));
        assert!(matches!(ask(LEI, Some("lite")), Err(LoadError::Profile(_))));
        // An unscoped caller may ask for any registered entity.
        let open = Request {
            lei: Some(OTHER),
            ..request(&[])
        };
        assert_eq!(load(&reader, open, &params).unwrap().entity.lei, OTHER);
    }

    #[test]
    fn a_cross_service_call_is_an_observed_topology() {
        let (_dir, store) = store_with(ORG);
        let parent = app_span("payments-api", NOW - DAY_NS, &[]);
        let mut child = app_span("ledger", NOW - DAY_NS, &[]);
        child.trace_id.clone_from(&parent.trace_id);
        child.parent_span_id = Some(parent.span_id.clone());
        seed(&store, &[parent, child], &[]);
        assert!(loaded(&store, request(&[])).inputs.facts.topology_observed);
    }

    #[test]
    fn only_snapshot_rows_are_read_back() {
        let (_dir, store) = store_with(ORG);
        let today = day_of(NOW);
        let row = |day: i64, source: &str, value: f64| tumult_lake::RequirementStatusRow {
            as_of_day: day,
            function_id: "cif-0001".into(),
            requirement_id: "dora.art12.2".into(),
            status: "existing".into(),
            value: Some(value),
            source: source.into(),
        };
        store
            .writer()
            .unwrap()
            .upsert_requirement_status(
                &[
                    row(today - 1, SNAPSHOT_SOURCE, 0.9),
                    row(today - 2, "imported", 0.1),
                    row(today - TWO_QUARTERS_DAYS, SNAPSHOT_SOURCE, 0.8),
                ],
                NOW,
            )
            .unwrap();
        let loaded = loaded(&store, request(&[]));
        let key = ("cif-0001".to_string(), "dora.art12.2".to_string());
        let days: Vec<i64> = loaded.inputs.snapshots[&key].keys().copied().collect();
        assert_eq!(days, [today - TWO_QUARTERS_DAYS, today - 1]);
        assert!(loaded.inputs.facts.posture_delta_two_quarters);

        // Stored rows cover every environment: a scoped caller gets none.
        let scope = ["prod".to_string()];
        let scoped = super::load(
            &store.read_only().unwrap(),
            request(&scope),
            &HouseParams::default(),
        )
        .unwrap();
        assert!(scoped.inputs.snapshots.is_empty());
        assert!(!scoped.inputs.facts.posture_delta_two_quarters);
    }

    /// A day the proof tool stored for a past date was not scored on
    /// that date. It is no stored day, for the Operating window or the
    /// two-quarter fact, unless the opt-in is set; either way the load
    /// says what it did with it.
    #[test]
    fn proof_days_never_count_without_the_opt_in() {
        let (_dir, store) = store_with(ORG);
        let today = day_of(NOW);
        let row = |day: i64, source: &str| tumult_lake::RequirementStatusRow {
            as_of_day: day,
            function_id: "cif-0001".into(),
            requirement_id: "dora.art12.2".into(),
            status: "existing".into(),
            value: Some(0.9),
            source: source.into(),
        };
        let rows = [
            row(today - 1, PROOF_SOURCE),
            row(today - 2, SNAPSHOT_SOURCE),
            row(today - TWO_QUARTERS_DAYS, PROOF_SOURCE),
        ];
        let writer = store.writer().unwrap();
        writer.upsert_requirement_status(&rows, NOW).unwrap();
        drop(writer);
        let key = ("cif-0001".to_string(), "dora.art12.2".to_string());
        let days = |loaded: &Loaded| -> Vec<i64> {
            loaded.inputs.snapshots[&key].keys().copied().collect()
        };

        let plain = loaded(&store, request(&[]));
        assert_eq!(days(&plain), [today - 2]);
        assert!(!plain.inputs.facts.posture_delta_two_quarters);
        let ignored = ProofDays {
            enabled: false,
            counted: 0,
            ignored: 2,
        };
        assert_eq!(plain.proof, ignored);

        let opted = Request {
            proof_days: true,
            ..request(&[])
        };
        let opted = loaded(&store, opted);
        assert_eq!(
            days(&opted),
            [today - TWO_QUARTERS_DAYS, today - 2, today - 1]
        );
        assert!(opted.inputs.facts.posture_delta_two_quarters);
        let counted = ProofDays {
            enabled: true,
            counted: 2,
            ignored: 0,
        };
        assert_eq!(opted.proof, counted);

        // No other source is let in by the opt-in.
        assert!(!day_counts("imported", true));
        assert!(!day_counts("", true));
        assert!(day_counts(SNAPSHOT_SOURCE, false));
    }

    #[test]
    fn an_unregistered_deployment_gets_stand_in_functions() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        seed(&store, &[spine_span("checkout", NOW - DAY_NS)], &[]);
        let loaded = loaded(&store, request(&[]));
        assert_eq!(loaded.entity.name, UNREGISTERED_NAME);
        assert_eq!(loaded.inputs.entity.label(), "full");
        let function = &loaded.inputs.functions[0];
        assert_eq!(
            (function.id.as_str(), function.tier, function.registered),
            ("cif-0001", Tier::Critical, false)
        );
        assert_eq!(loaded.inputs.services[0].functions, ["cif-0001"]);
    }

    /// An id that is not in the canonical form is a sender's mistake or
    /// an attack, never a function: it would otherwise be shown as a row
    /// label on the page.
    /// `attr` writes its key into SQL, so it takes only a plain
    /// compile-time name and refuses anything else outright.
    #[test]
    #[should_panic(expected = "attribute key is not a plain name")]
    fn attr_refuses_a_key_that_is_not_a_plain_name() {
        let _ = attr("ict.entity.id'] OR TRUE --");
    }

    #[test]
    fn attr_reads_a_plain_key_from_both_attribute_maps() {
        assert_eq!(
            attr("ict.entity.id"),
            "COALESCE(resource_attrs['ict.entity.id'], span_attrs['ict.entity.id'])"
        );
    }

    #[test]
    fn a_malformed_entity_id_never_becomes_a_stand_in_function() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let with_id = |service: &str, id: &str| {
            let mut span = spine_span(service, NOW - DAY_NS);
            for (key, value) in &mut span.resource_attrs {
                if key == "ict.entity.id" {
                    *value = id.to_string();
                }
            }
            span
        };
        let long = format!(
            "a-{}",
            "0".repeat(tumult_core::compliance::ENTITY_ID_MAX_LEN)
        );
        seed(
            &store,
            &[
                with_id("checkout", "cif-0001"),
                with_id("evil", "<img src=x onerror=alert(1)>"),
                with_id("legacy", "CF-0042"),
                with_id("wordy", &long),
            ],
            &[],
        );
        let loaded = loaded(&store, request(&[]));
        let ids: Vec<&str> = loaded
            .inputs
            .functions
            .iter()
            .map(|f| f.id.as_str())
            .collect();
        assert_eq!(ids, ["cif-0001"]);
    }

    const TWO_FIRMS: &str = "
legal_entities:
  - lei: 529900T8BM49AURSDO55
    name: Demo Firm
    profile: simplified
    registry_signed_by: Head of Compliance
    registry_signed_at: 2026-10-01
  - {lei: 506700GE1G29325QX363, name: Other Firm, profile: full}
functions:
  - id: cif-0001
    lei: 529900T8BM49AURSDO55
    name: Payments
    tier: critical
    services: [\"payments-*\", \"ledger\"]
  - {id: cif-0002, lei: 529900T8BM49AURSDO55, name: Reporting, tier: important}
  - {id: cif-0003, lei: 506700GE1G29325QX363, name: Custody, tier: critical}
";

    /// A caller scoped to some environments sees the functions its own
    /// telemetry binds to and nothing of the register beyond them.
    #[test]
    fn a_scoped_caller_gets_only_the_functions_it_can_see_and_no_register_data() {
        let (_dir, store) = store_with(TWO_FIRMS);
        let mut staging = spine_span("payments-api", NOW - DAY_NS);
        staging.target_environment = Some("staging".into());
        seed(&store, &[staging], &[]);
        let workloads = [Workload {
            id: "wl:shop/deployment/payments-api".into(),
            service: Some("payments-api".into()),
        }];

        let all = Request {
            lei: Some(LEI),
            workloads: &workloads,
            ..request(&[])
        };
        let open = loaded(&store, all);
        assert_eq!(open.inputs.functions.len(), 2);
        assert_eq!(open.entities.len(), 2);
        assert_eq!(
            open.entity.registry_signed_by.as_deref(),
            Some("Head of Compliance")
        );
        assert!(!open.scoped);

        let envs = vec!["staging".to_string()];
        let scoped = loaded(
            &store,
            Request {
                lei: Some(LEI),
                workloads: &workloads,
                ..request(&envs)
            },
        );
        assert!(scoped.scoped);
        let ids: Vec<&str> = scoped
            .inputs
            .functions
            .iter()
            .map(|f| f.id.as_str())
            .collect();
        assert_eq!(ids, ["cif-0001"], "Reporting has nothing in staging");
        assert!(scoped.inputs.functions[0].register_codes.is_empty());
        assert!(scoped.register.is_empty());
        assert_eq!(scoped.entities.len(), 1);
        assert_eq!(scoped.entities[0].lei, LEI);
        assert_eq!(scoped.entity.registry_signed_by, None);
        assert_eq!(scoped.entity.registry_signed_at, None);
        assert!(scoped.inputs.workloads.is_empty());
        let services: Vec<&str> = scoped
            .inputs
            .services
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(services, ["payments-api"], "no declared name it cannot see");

        // Nothing in the environment: no function at all, not the register.
        let none = vec!["dev".to_string()];
        let empty = loaded(&store, request(&none));
        assert!(empty.inputs.functions.is_empty());
        assert!(empty.inputs.services.is_empty());
    }

    #[test]
    fn the_request_can_override_the_profile_and_must_name_a_known_entity() {
        let (_dir, store) = store_with(ORG);
        let reader = store.read_only().unwrap();
        let params = HouseParams::default();
        let full = Request {
            profile: Some("full"),
            ..request(&[])
        };
        let overridden = load(&reader, full, &params).unwrap();
        assert!(overridden.profile_overridden);
        let same = Request {
            profile: Some("simplified"),
            ..request(&[])
        };
        assert!(!load(&reader, same, &params).unwrap().profile_overridden);
        assert!(
            !load(&reader, request(&[]), &params)
                .unwrap()
                .profile_overridden
        );
        assert_eq!(
            load(&reader, full, &params).unwrap().inputs.entity.label(),
            "full"
        );
        let bad = Request {
            profile: Some("lite"),
            ..request(&[])
        };
        assert!(matches!(
            load(&reader, bad, &params),
            Err(LoadError::Profile(_))
        ));
        let unknown = Request {
            lei: Some("5493001KJTIIGC8Y1R12"),
            ..request(&[])
        };
        assert_eq!(
            load(&reader, unknown, &params),
            Err(LoadError::UnknownLei("5493001KJTIIGC8Y1R12".into()))
        );
    }

    #[test]
    fn manual_records_cite_rows_and_speak_for_the_entity_without_a_target() {
        let (_dir, store) = store_with(ORG);
        let writer = store.writer().unwrap();
        let record = |id: &str, exercise: &str, target: &str, refs: &str| {
            let target = if target.is_empty() {
                "NULL".to_string()
            } else {
                format!("'{target}'")
            };
            format!(
                "INSERT INTO manual_experiments (id, experiment_name, exercise_type, \
                 executed_at_ns, hypothesis, method, outcome_status, target_system, \
                 target_environment, entered_by, entered_at_ns, attestation, status, \
                 framework_refs, content_hash) VALUES ('{id}', '{id}', '{exercise}', {at}, \
                 'h', 'm', 'passed', {target}, 'prod', 'ada', {at}, 'a', 'verified', \
                 CAST('{refs}' AS VARCHAR[]), 'hash')",
                at = NOW - DAY_NS
            )
        };
        for sql in [
            record("m1", "tabletop", "", "[\"dora.art11.7\"]"),
            record("m2", "drill", "ledger", "[]"),
            record("m3", "drill", "elsewhere", "[]"),
        ] {
            writer.execute(&sql, []).unwrap();
        }
        let loaded = loaded(&store, request(&[]));
        let seen: Vec<Seen<'_>> = loaded
            .inputs
            .evidence
            .iter()
            .map(|e| {
                (
                    e.id.as_str(),
                    e.functions.clone(),
                    e.kind.as_deref(),
                    e.cited.clone(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("m1", Vec::new(), None, vec!["dora.art11.7".to_string()]),
                ("m2", vec!["cif-0001".to_string()], Some("dr"), Vec::new()),
            ]
        );
        assert!(loaded
            .inputs
            .evidence
            .iter()
            .all(|e| e.source == Source::Manual));
    }
}
