//! The scores as the JSON the API serves.
//!
//! Three documents are built from one scored [`Matrix`]:
//!
//! - [`dora`]: coverage per article and per function (or org unit), the
//!   expectation table and the reporting-readiness facts;
//! - [`article`]: one article with every evidence row behind each cell;
//! - [`maturity`]: the internal level of the legal entity with its
//!   pillars and gates, and the per-node roll-up with the cap chain.
//!
//! No number leaves this module without the unknown share it must be
//! read with.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::{json, Map, Value};
use tumult_core::compliance::{
    article_title, EvidenceClass, ARTICLES, EVIDENCE_DISCLAIMER, REQUIREMENTS_VERSION,
};
use tumult_lake::{Reader, RequirementStatusRow};

use super::coverage::{CellState, Matrix, MatrixCell, MatrixRow, RowState, Scored};
use super::loader::{self, LoadError, Loaded, Request, SNAPSHOT_SOURCE};
use super::maturity::{self, Maturity, Pillar, Scope, CONFORMANCE_LABELS};
use super::model::{day_of, days_from_civil, end_of_day, format_day, Function, Source, Workload};
use super::params::{HouseParams, PillarWeights};
use super::rollup::{self, Cap, CapRule, Kind, NodeScore, ROOT_ID};
use super::status::{self, RowStatus, Standing, Status, StatusReport};

/// Where the 1–5 scale comes from, printed with every level.
pub const SCALE_SOURCE: &str = "Internal house scale. DORA defines no maturity levels; \
     this is not a regulatory rating.";
/// House names of the five levels.
pub const SCALE_LEVELS: [&str; 5] = ["Initial", "Repeatable", "Defined", "Measured", "Improving"];
/// Quarter ends the maturity history looks back over.
pub const HISTORY_QUARTERS: usize = 4;
/// Decimal places of a coverage value or share.
const SHARE_PLACES: i32 = 4;
/// Decimal places of an index or a pillar value.
const INDEX_PLACES: i32 = 3;

/// What the rows of the coverage heatmap are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoraBy {
    /// One row per function (ICT entity).
    Entity,
    /// One row per org unit, and one for the legal entity.
    Org,
}

impl DoraBy {
    /// Parse the `by` query parameter; absent means per function.
    ///
    /// # Errors
    /// Names the accepted values.
    pub fn parse(text: Option<&str>) -> Result<Self, String> {
        match text {
            None | Some("entity") => Ok(Self::Entity),
            Some("org") => Ok(Self::Org),
            Some(other) => Err(format!("by must be entity or org, got '{other}'")),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Org => "org",
        }
    }
}

/// Which node rows the maturity document lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaturityBy {
    /// Every node of the spine.
    Node,
    /// The functions (`ent:` rows).
    Entity,
    /// The org units and the organisation.
    Org,
}

impl MaturityBy {
    /// Parse the `by` query parameter; absent lists no node rows.
    ///
    /// # Errors
    /// Names the accepted values.
    pub fn parse(text: Option<&str>) -> Result<Option<Self>, String> {
        match text {
            None => Ok(None),
            Some("node") => Ok(Some(Self::Node)),
            Some("entity") => Ok(Some(Self::Entity)),
            Some("org") => Ok(Some(Self::Org)),
            Some(other) => Err(format!("by must be node, entity or org, got '{other}'")),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Entity => "entity",
            Self::Org => "org",
        }
    }

    fn lists(self, kind: Kind) -> bool {
        match self {
            Self::Node => true,
            Self::Entity => kind == Kind::Entity,
            Self::Org => kind.is_org(),
        }
    }
}

/// A request that names something the entity does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    UnknownArticle(String),
    UnknownEntity(String),
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownArticle(a) => write!(f, "article '{a}' is not in the registry"),
            Self::UnknownEntity(e) => write!(f, "function '{e}' is not registered"),
        }
    }
}

impl std::error::Error for ReportError {}

fn rounded(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

fn share(value: f64) -> f64 {
    rounded(value, SHARE_PLACES)
}

fn opt_share(value: Option<f64>) -> Value {
    json!(value.map(share))
}

/// `good`, `fair` or `weak` by the house bands; `unknown` without a value.
#[must_use]
pub fn band(coverage: Option<f64>, params: &HouseParams) -> &'static str {
    let Some(coverage) = coverage else {
        return "unknown";
    };
    if coverage >= params.band_good {
        return "good";
    }
    if coverage >= params.band_fair {
        return "fair";
    }
    "weak"
}

/// One row of the heatmap: a function, an org unit, or the legal entity.
struct Group {
    id: String,
    label: String,
    tier: Option<&'static str>,
    functions: Vec<usize>,
}

fn function_groups(functions: &[Function]) -> Vec<Group> {
    functions
        .iter()
        .enumerate()
        .map(|(index, f)| Group {
            id: f.id.clone(),
            label: if f.name.is_empty() {
                f.id.clone()
            } else {
                f.name.clone()
            },
            tier: Some(f.tier.as_str()),
            functions: vec![index],
        })
        .collect()
}

/// Every prefix of an org path: `a/b` gives `a` and `a/b`.
fn org_prefixes(path: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    let mut prefix = String::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(segment);
        prefixes.push(prefix.clone());
    }
    prefixes
}

fn org_groups(loaded: &Loaded) -> Vec<Group> {
    let functions = &loaded.inputs.functions;
    let mut groups = vec![Group {
        id: ROOT_ID.to_string(),
        label: loaded.entity.name.clone(),
        tier: None,
        functions: (0..functions.len()).collect(),
    }];
    let paths: BTreeSet<String> = functions
        .iter()
        .filter_map(|f| f.org_unit.as_deref())
        .flat_map(org_prefixes)
        .collect();
    for path in paths {
        let inside = |f: &Function| {
            f.org_unit
                .as_deref()
                .is_some_and(|unit| org_prefixes(unit).contains(&path))
        };
        groups.push(Group {
            id: rollup::org_id(&path),
            label: path.rsplit('/').next().unwrap_or(&path).to_string(),
            tier: None,
            functions: (0..functions.len())
                .filter(|i| inside(&functions[*i]))
                .collect(),
        });
    }
    groups
}

fn groups(loaded: &Loaded, by: DoraBy) -> Vec<Group> {
    match by {
        DoraBy::Entity => function_groups(&loaded.inputs.functions),
        DoraBy::Org => org_groups(loaded),
    }
}

/// Evidence status of one article over some functions: the weakest
/// observable cell, and how many rows fall short of their expectation.
fn article_cell_status(matrix: &Matrix<'_>, article: &str, functions: &[usize]) -> (Status, usize) {
    let params = matrix.ctx.params;
    let mut weakest: Option<u8> = None;
    let mut unknown = false;
    let mut short = 0;
    for row in matrix.rows.iter().filter(|r| r.req.article == article) {
        let expected = status::expected_rank(row.req, matrix);
        let mut row_short = false;
        for cell in row.among(functions) {
            let cell_status = status::cell_status(cell, params);
            unknown |= cell_status == Status::Unknown;
            let Some(rank) = cell_status.rank() else {
                continue;
            };
            weakest = Some(weakest.map_or(rank, |w| w.min(rank)));
            row_short |= rank < expected;
        }
        short += usize::from(row_short);
    }
    let status = match weakest {
        Some(rank) => status_of_rank(rank),
        None if unknown => Status::Unknown,
        None => Status::NoEvents,
    };
    (status, short)
}

fn status_of_rank(rank: u8) -> Status {
    match rank {
        0 => Status::None,
        1 => Status::Designed,
        2 => Status::Existing,
        _ => Status::Operating,
    }
}

fn cells(matrix: &Matrix<'_>, groups: &[Group]) -> Vec<Value> {
    let mut out = Vec::new();
    for (article, _) in ARTICLES {
        for group in groups {
            let (ratio, state) = matrix.article_over(article, &group.functions);
            if state == RowState::Na {
                continue;
            }
            let (status, short) = article_cell_status(matrix, article, &group.functions);
            out.push(json!({
                "article": article,
                "entity_id": group.id,
                "coverage": opt_share(ratio.coverage),
                "unknown": share(ratio.unknown),
                "state": state.as_str(),
                "status": status.as_str(),
                "status_rank": status.rank(),
                "gaps": short,
            }));
        }
    }
    out
}

fn evidence_json(scored: &Scored<'_>) -> Value {
    let e = scored.evidence;
    json!({
        "evidence_id": e.id,
        "source": e.source.as_str(),
        "kind": e.kind,
        "exercise_type": e.exercise_type,
        "experiment": e.experiment,
        "at_ns": e.at_ns,
        "age_days": scored.age_days,
        "outcome": e.outcome.as_str(),
        "phi": share(scored.phi),
        "o": share(scored.o),
        "s": share(scored.s),
        "v": share(scored.v),
    })
}

fn cell_state(state: CellState) -> &'static str {
    match state {
        CellState::Unknown => "unknown",
        CellState::NoEvents => "no_events",
        CellState::Scored => "known",
    }
}

/// One function's cell of a requirement row.
fn entity_json(matrix: &Matrix<'_>, cell: &MatrixCell<'_>, evidence: Option<Vec<Value>>) -> Value {
    let params = matrix.ctx.params;
    let function = &matrix.ctx.inputs.functions[cell.function];
    let status = status::cell_status(cell, params);
    let best = cell.cell.best.as_ref();
    let scored = cell.cell.state == CellState::Scored;
    let mut out = json!({
        "entity_id": function.id,
        "tier": function.tier.as_str(),
        "state": cell_state(cell.cell.state),
        "v": scored.then(|| share(cell.cell.v)),
        "age_days": best.map(|b| b.age_days),
        "outcome": best.map(|b| b.evidence.outcome.as_str()),
        "evidence_id": best.map(|b| b.evidence.id.as_str()),
        "source": best.map(|b| b.evidence.source.as_str()),
        "status": status.as_str(),
        "streak_days": cell.streak_days,
        "days_until_operating": (status == Status::Existing)
            .then(|| status::days_until_operating(cell.streak_days, params)),
        "reason": cell.cell.reason,
    });
    if let (Some(rows), Some(map)) = (evidence, out.as_object_mut()) {
        map.insert("evidence".into(), Value::Array(rows));
    }
    out
}

/// Criticality-weighted mean of a row's scored cells.
fn row_coverage(matrix: &Matrix<'_>, row: &MatrixRow<'_>) -> Option<f64> {
    let mut num = 0.0;
    let mut den = 0.0;
    for cell in row
        .cells
        .iter()
        .filter(|c| c.cell.state == CellState::Scored)
    {
        let weight = matrix.weight(cell.function);
        num += weight * cell.cell.v;
        den += weight;
    }
    (den > 0.0).then(|| num / den)
}

/// One requirement row. `detail` adds every evidence row to each cell and
/// narrows the cells to one function when `only` names it.
fn requirement_json(
    matrix: &Matrix<'_>,
    row: &MatrixRow<'_>,
    row_status: &RowStatus,
    detail: bool,
    only: Option<usize>,
) -> Value {
    let ctx = &matrix.ctx;
    let entities: Vec<Value> = row
        .scoped(only)
        .map(|cell| {
            let function = &ctx.inputs.functions[cell.function];
            let evidence = detail.then(|| {
                ctx.evidence_rows(row.req, function, ctx.inputs.as_of_ns)
                    .iter()
                    .map(evidence_json)
                    .collect()
            });
            entity_json(matrix, cell, evidence)
        })
        .collect();
    let req = row.req;
    json!({
        "id": req.id,
        "cite": req.cite,
        "title": req.title,
        "weight": req.weight,
        "evidence": req.evidence.as_str(),
        "window_days": req.window_days,
        "scope": req.scope.as_str(),
        "strength": req.strength.as_str(),
        "coverage": opt_share(row_coverage(matrix, row)),
        "state": row.state(only).as_str(),
        "status": row_status.status.as_str(),
        "expected": status::rank_name(row_status.expected),
        "gap": row_status.gap,
        "standing": row_status.standing.as_str(),
        "entities": entities,
        "reason": row.reason(only),
    })
}

/// Where the evidence of an article comes from, as shares of its cells
/// weighted by row weight and function criticality.
fn sources(matrix: &Matrix<'_>, article: &str) -> Value {
    let mut split = [0.0_f64; 5];
    for row in matrix.rows.iter().filter(|r| r.req.article == article) {
        for cell in &row.cells {
            let Some(slot) = source_slot(row, cell) else {
                continue;
            };
            split[slot] += f64::from(row.req.weight) * matrix.weight(cell.function);
        }
    }
    let total: f64 = split.iter().sum();
    let part = |value: f64| {
        if total > 0.0 {
            return share(value / total);
        }
        0.0
    };
    json!({
        "otel_only": part(split[0]),
        "tested": part(split[1]),
        "manual": part(split[2]),
        "none": part(split[3]),
        "unknown": part(split[4]),
    })
}

/// Which source bucket a cell counts in: 0 telemetry and declarations,
/// 1 automated runs, 2 manual records, 3 nothing, 4 unknown. A cell
/// without events counts nowhere.
fn source_slot(row: &MatrixRow<'_>, cell: &MatrixCell<'_>) -> Option<usize> {
    match cell.cell.state {
        CellState::NoEvents => return None,
        CellState::Unknown => return Some(4),
        CellState::Scored => {}
    }
    if cell.cell.v <= 0.0 {
        return Some(3);
    }
    match cell.cell.best.as_ref().map(|b| b.evidence.source) {
        Some(Source::Run) => Some(1),
        Some(Source::Manual) => Some(2),
        None if row.req.evidence == EvidenceClass::Manual => Some(2),
        None => Some(0),
    }
}

fn articles(matrix: &Matrix<'_>, report: &StatusReport) -> Vec<Value> {
    let mut out = Vec::new();
    for (article, title) in ARTICLES {
        let rows: Vec<&MatrixRow<'_>> = matrix
            .rows
            .iter()
            .filter(|r| r.req.article == *article)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let na_rows = matrix.na.iter().filter(|r| r.article == *article).count();
        let ratio = matrix.article(article, None);
        let requirements: Vec<Value> = rows
            .iter()
            .filter_map(|row| {
                let row_status = report.row(row.req.id)?;
                Some(requirement_json(matrix, row, row_status, false, None))
            })
            .collect();
        let counts = report.articles.iter().find(|a| a.article == *article);
        out.push(json!({
            "article": article,
            "title": title,
            "coverage": opt_share(ratio.coverage),
            "unknown": share(ratio.unknown),
            "band": band(ratio.coverage, matrix.ctx.params),
            "na": share(na_rows as f64 / (na_rows + rows.len()) as f64),
            "na_rows": na_rows,
            "status": counts.map(|c| json!({
                "meets": c.meets,
                "gap": c.gap,
                "unknown": c.unknown,
                "no_events": c.no_events,
                "na": c.na,
            })),
            "requirements": requirements,
            "sources": sources(matrix, article),
        }));
    }
    out
}

/// The cell of a row that is furthest below the expectation, heaviest
/// function first.
fn weakest_cell<'m, 'a>(
    matrix: &Matrix<'_>,
    row: &'m MatrixRow<'a>,
    expected: u8,
) -> Option<&'m MatrixCell<'a>> {
    let params = matrix.ctx.params;
    row.cells.iter().min_by(|a, b| {
        let key = |c: &MatrixCell<'_>| {
            let rank = status::cell_status(c, params).rank().unwrap_or(expected);
            (rank, -matrix.weight(c.function))
        };
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// One line of the expectation table.
fn expectation(matrix: &Matrix<'_>, row: &MatrixRow<'_>, row_status: &RowStatus) -> Value {
    let params = matrix.ctx.params;
    let inputs = matrix.ctx.inputs;
    let short: Vec<&MatrixCell<'_>> = row
        .cells
        .iter()
        .filter(|c| {
            status::cell_status(c, params)
                .rank()
                .is_some_and(|rank| rank < row_status.expected)
        })
        .collect();
    let criticality = short
        .iter()
        .map(|c| matrix.weight(c.function))
        .fold(0.0, f64::max);
    let oldest = row
        .cells
        .iter()
        .filter_map(|c| c.cell.best.as_ref())
        .map(|b| b.evidence.at_ns)
        .min();
    let waiting = row
        .cells
        .iter()
        .filter(|c| status::cell_status(c, params) == Status::Existing)
        .map(|c| status::days_until_operating(c.streak_days, params))
        .max();
    let link = weakest_cell(matrix, row, row_status.expected)
        .and_then(|c| c.cell.best.as_ref())
        .map(|b| json!({ "id": b.evidence.id, "source": b.evidence.source.as_str() }));
    let req = row.req;
    json!({
        "id": req.id,
        "article": req.article,
        "cite": req.cite,
        "title": req.title,
        "weight": req.weight,
        "expected": status::rank_name(row_status.expected),
        "actual": row_status.status.as_str(),
        "gap": row_status.gap,
        "standing": row_status.standing.as_str(),
        "criticality": criticality,
        "priority": f64::from(row_status.gap) * f64::from(req.weight) * criticality,
        "short": short
            .iter()
            .map(|c| inputs.functions[c.function].id.as_str())
            .collect::<Vec<_>>(),
        "oldest_evidence_ns": oldest,
        "oldest_evidence_date": oldest.map(|ns| format_day(day_of(ns))),
        "days_until_operating": waiting,
        "evidence": link,
        "reason": row.reason(None),
    })
}

/// The expectation table, worst first: gap × weight × criticality.
fn expectations(matrix: &Matrix<'_>, report: &StatusReport) -> Vec<Value> {
    let mut rows: Vec<Value> = matrix
        .rows
        .iter()
        .filter_map(|row| Some(expectation(matrix, row, report.row(row.req.id)?)))
        .collect();
    let priority = |v: &Value| v["priority"].as_f64().unwrap_or(0.0);
    rows.sort_by(|a, b| {
        priority(b)
            .partial_cmp(&priority(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

fn function_json(matrix: &Matrix<'_>, index: usize, function: &Function) -> Value {
    let ratio = matrix.portfolio(Some(index));
    json!({
        "entity_id": function.id,
        "name": function.name,
        "tier": function.tier.as_str(),
        "registered": function.registered,
        "register_codes": function.register_codes,
        "org_unit": function.org_unit,
        "rto_s": function.rto_s,
        "rpo_s": function.rpo_s,
        "last_assessed_ns": function.last_assessed_ns,
        "last_assessed": function.last_assessed_ns.map(|ns| format_day(day_of(ns))),
        "coverage": opt_share(ratio.coverage),
        "unknown": share(ratio.unknown),
        "band": band(ratio.coverage, matrix.ctx.params),
    })
}

/// Reporting readiness: how complete the function register is, and
/// which services map to no function.
fn readiness(loaded: &Loaded) -> Value {
    let inputs = &loaded.inputs;
    let functions = &inputs.functions;
    let count = |has: fn(&Function) -> bool| functions.iter().filter(|f| has(f)).count();
    let complete = |f: &Function| {
        f.registered && f.rto_s.is_some() && f.rpo_s.is_some() && f.last_assessed_ns.is_some()
    };
    let unmapped: Vec<&str> = inputs
        .services
        .iter()
        .filter(|s| s.functions.is_empty())
        .map(|s| s.name.as_str())
        .collect();
    json!({
        "functions": functions.len(),
        "registered": count(|f| f.registered),
        "with_rto": count(|f| f.rto_s.is_some()),
        "with_rpo": count(|f| f.rpo_s.is_some()),
        "with_assessment": count(|f| f.last_assessed_ns.is_some()),
        "complete": functions.iter().filter(|f| complete(f)).count(),
        "register": {
            "connected": inputs.facts.register_connected,
            "reconciled_ns": inputs.facts.register_reconciled_ns,
            "signed_by": loaded.entity.registry_signed_by,
            "signed_at": loaded.entity.registry_signed_at,
            "rows": loaded.register.len(),
        },
        "unmapped_services": unmapped,
    })
}

fn legal_entities(loaded: &Loaded) -> Vec<Value> {
    loaded
        .entities
        .iter()
        .map(|e| json!({ "lei": e.lei, "name": e.name, "profile": e.profile }))
        .collect()
}

/// Fields every document opens with.
fn header(loaded: &Loaded) -> Map<String, Value> {
    let inputs = &loaded.inputs;
    let mut map = Map::new();
    map.insert("as_of_ns".into(), json!(inputs.as_of_ns));
    map.insert("registry_version".into(), json!(REQUIREMENTS_VERSION));
    map.insert("lei".into(), json!(loaded.entity.lei));
    map.insert("entity_name".into(), json!(loaded.entity.name));
    map.insert("profile".into(), json!(inputs.entity.profile.as_str()));
    map.insert("profile_label".into(), json!(inputs.entity.label()));
    map.insert("profile_flags".into(), json!(inputs.entity.flag_names()));
    map.insert(
        "legal_entities".into(),
        Value::Array(legal_entities(loaded)),
    );
    map.insert("feeds".into(), Value::Array(feeds(loaded)));
    map
}

/// Every feed with what the register declares and what the lake holds.
fn feeds(loaded: &Loaded) -> Vec<Value> {
    loaded
        .feeds
        .iter()
        .map(|feed| {
            json!({
                "name": feed.name,
                "declared": feed.declared,
                "connected": feed.connected,
                "reason": feed.reason,
            })
        })
        .collect()
}

/// The coverage document of one legal entity.
#[must_use]
pub fn dora(loaded: &Loaded, matrix: &Matrix<'_>, by: DoraBy) -> Value {
    let params = matrix.ctx.params;
    let report = status::report(matrix, None);
    let portfolio = matrix.portfolio(None);
    let groups = groups(loaded, by);
    let mut map = header(loaded);
    map.insert("disclaimer".into(), json!(EVIDENCE_DISCLAIMER));
    map.insert("by".into(), json!(by.as_str()));
    map.insert(
        "portfolio".into(),
        json!({
            "coverage": opt_share(portfolio.coverage),
            "unknown": share(portfolio.unknown),
            "band": band(portfolio.coverage, params),
        }),
    );
    map.insert(
        "status".into(),
        json!({
            "meets": report.count(Standing::Meets),
            "gap": report.count(Standing::Gap),
            "unknown": report.count(Standing::Unknown),
            "no_events": report.count(Standing::NoEvents),
            "na": matrix.na.len(),
        }),
    );
    map.insert(
        "rows".into(),
        Value::Array(
            groups
                .iter()
                .map(|g| json!({ "entity_id": g.id, "label": g.label, "tier": g.tier }))
                .collect(),
        ),
    );
    map.insert(
        "functions".into(),
        Value::Array(
            loaded
                .inputs
                .functions
                .iter()
                .enumerate()
                .map(|(index, f)| function_json(matrix, index, f))
                .collect(),
        ),
    );
    map.insert("articles".into(), Value::Array(articles(matrix, &report)));
    map.insert("cells".into(), Value::Array(cells(matrix, &groups)));
    map.insert(
        "expectations".into(),
        Value::Array(expectations(matrix, &report)),
    );
    map.insert(
        "na".into(),
        json!({
            "count": matrix.na.len(),
            "rows": matrix
                .na
                .iter()
                .map(|r| json!({ "id": r.id, "article": r.article, "cite": r.cite }))
                .collect::<Vec<_>>(),
        }),
    );
    map.insert("readiness".into(), readiness(loaded));
    Value::Object(map)
}

/// One article with every evidence row behind each cell, optionally for
/// one function.
///
/// # Errors
/// `UnknownArticle` when the registry has no such article, `UnknownEntity`
/// when `entity` names no function of the legal entity.
pub fn article(
    loaded: &Loaded,
    matrix: &Matrix<'_>,
    key: &str,
    entity: Option<&str>,
) -> Result<Value, ReportError> {
    let title = article_title(key).ok_or_else(|| ReportError::UnknownArticle(key.to_string()))?;
    let only = match entity {
        None => None,
        Some(id) => Some(
            loaded
                .inputs
                .functions
                .iter()
                .position(|f| f.id == id)
                .ok_or_else(|| ReportError::UnknownEntity(id.to_string()))?,
        ),
    };
    let ratio = matrix.article(key, only);
    let requirements: Vec<Value> = matrix
        .rows
        .iter()
        .filter(|row| row.req.article == key)
        .map(|row| {
            let row_status = status::row_report(matrix, row, only);
            requirement_json(matrix, row, &row_status, true, only)
        })
        .collect();
    let mut map = header(loaded);
    map.insert("disclaimer".into(), json!(EVIDENCE_DISCLAIMER));
    map.insert("article".into(), json!(key));
    map.insert("title".into(), json!(title));
    map.insert("entity".into(), json!(entity));
    map.insert("coverage".into(), opt_share(ratio.coverage));
    map.insert("unknown".into(), json!(share(ratio.unknown)));
    map.insert(
        "na_rows".into(),
        json!(matrix.na.iter().filter(|r| r.article == key).count()),
    );
    map.insert("requirements".into(), Value::Array(requirements));
    Ok(Value::Object(map))
}

/// The maturity of a legal entity: the flat §3.6 assessment and the
/// per-node roll-up.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    /// Pillars, gates and level of the legal entity as a whole.
    pub entity: Maturity,
    /// Every spine node, rolled up; the root is [`ROOT_ID`].
    pub nodes: Vec<NodeScore>,
}

impl Assessment {
    /// The organisation row of the roll-up.
    #[must_use]
    pub fn root(&self) -> Option<&NodeScore> {
        self.nodes.iter().find(|n| n.id == ROOT_ID)
    }

    /// The level shown for the organisation: the roll-up root's, which
    /// already takes the gates and the weakest critical child.
    #[must_use]
    pub fn level(&self) -> u8 {
        self.root().map_or(self.entity.level, |root| root.level)
    }

    /// The index shown for the organisation: the roll-up root's.
    #[must_use]
    pub fn index(&self) -> f64 {
        self.root().map_or(self.entity.index, |root| root.index)
    }

    /// The level sits below what an index alone would give: the rolled-up
    /// one, or the one of the pillars before the roll-up.
    #[must_use]
    pub fn capped(&self) -> bool {
        let by_index = maturity::floor_level(self.index());
        let by_pillars = maturity::floor_level(self.entity.index);
        self.level() < by_index.max(by_pillars)
    }

    /// `level n of target m (profile)`.
    #[must_use]
    pub fn headline(&self) -> String {
        format!(
            "level {} of target {} ({})",
            self.level(),
            self.entity.target,
            self.entity.label
        )
    }
}

/// Assess the legal entity behind `matrix` and roll its nodes up.
#[must_use]
pub fn assess(matrix: &Matrix<'_>) -> Assessment {
    let entity = maturity::assess(matrix, Scope::Entity);
    let nodes = rollup::assess(matrix, &entity);
    Assessment { entity, nodes }
}

fn index(value: f64) -> f64 {
    rounded(value, INDEX_PLACES)
}

fn pillar_json(pillar: &Pillar) -> Value {
    let reason = match pillar {
        Pillar::Unknown(reason) => Some(reason.as_str()),
        Pillar::Value(_) | Pillar::Excluded(_) => None,
    };
    json!({
        "value": pillar.value().map(index),
        "state": pillar.state(),
        "reason": reason,
    })
}

fn node_json(node: &NodeScore) -> Value {
    let pillars: Map<String, Value> = node
        .pillars
        .iter()
        .map(|(letter, pillar)| ((*letter).to_string(), pillar_json(pillar)))
        .collect();
    json!({
        "id": node.id,
        "kind": node.kind.as_str(),
        "label": node.label,
        "parent": node.parent,
        "index": index(node.index),
        "level": node.level,
        "target": node.target,
        "own": node.own.map(index),
        "pillars": pillars,
        "rollup": {
            "mean": node.mean.map(index),
            "weight": node.weight,
            "children": node.children,
        },
        "unknown_share": share(node.unknown_share),
        "unknown": node.unknown,
        "critical": node.critical,
        "cap": {
            "rule": node.cap.rule.as_str(),
            "level": node.cap.level,
            "node_ids": node.cap.node_ids,
            "origin": node.cap.origin,
            "origin_state": node.cap.origin_state,
            "reason": node.cap.reason,
        },
        "drag": node.drag,
        "note": node.note,
    })
}

/// The pillar a failing gate condition is about, when it names one.
fn gate_component(reason: &str) -> Option<&'static str> {
    const MARKS: [(&str, &str); 4] = [
        ("(T)", "coverage"),
        ("(S)", "slo"),
        ("(R)", "recovery"),
        ("(L)", "loop"),
    ];
    if let Some((_, name)) = MARKS.iter().find(|(mark, _)| reason.contains(mark)) {
        return Some(name);
    }
    let conformance = CONFORMANCE_LABELS
        .iter()
        .any(|label| reason.starts_with(label))
        || reason.contains("service in scope");
    if conformance {
        return Some("conformance");
    }
    // Evidence that is too young or broken is no pillar: the level
    // waits for the days to pass.
    reason.contains("evidence").then_some("evidence")
}

/// The gate that holds, or next blocks, the legal entity's level.
fn gate_cap(m: &Maturity) -> Value {
    let Some((level, reason)) = m.blocked.as_ref() else {
        return json!({ "rule": "none", "level": null, "component": null, "reason": null });
    };
    json!({
        "rule": if m.capped { "gate" } else { "none" },
        "level": level.saturating_sub(1),
        "blocked_level": level,
        "component": gate_component(reason),
        "reason": m.reason(),
    })
}

/// Why a critical child holds its parent down, in words.
fn child_cap_reason(cap: &Cap) -> String {
    let what = match cap.origin_state {
        Some("unknown") => "critical, no telemetry and no test",
        Some("known") => "critical, at a lower level",
        _ => "critical",
    };
    format!("capped by {} ({what})", cap.origin.join(", "))
}

/// What holds the shown level: the weakest critical child when the
/// roll-up names one, else the blocked gate.
fn shown_cap(assessment: &Assessment) -> Value {
    let by_child = assessment
        .root()
        .map(|root| &root.cap)
        .filter(|cap| cap.rule == CapRule::WeakestCriticalChild);
    let Some(cap) = by_child else {
        return gate_cap(&assessment.entity);
    };
    json!({
        "rule": cap.rule.as_str(),
        "level": cap.level,
        "component": null,
        "node_ids": cap.node_ids,
        "origin": cap.origin,
        "reason": child_cap_reason(cap),
    })
}

fn weights_json(w: &PillarWeights) -> Value {
    json!({
        "conformance": w.conformance,
        "coverage": w.coverage,
        "slo": w.slo,
        "recovery": w.recovery,
        "loop": w.loop_,
    })
}

fn components(m: &Maturity) -> (Value, Value) {
    let parts = [
        ("conformance", &m.pillars.conformance),
        ("coverage", &m.pillars.coverage),
        ("slo", &m.pillars.slo),
        ("recovery", &m.pillars.recovery),
        ("loop", &m.pillars.loop_),
    ];
    let mut values = Map::new();
    let mut states = Map::new();
    for (name, pillar) in parts {
        values.insert(name.into(), json!(pillar.value().map(index)));
        states.insert(name.into(), pillar_json(pillar));
    }
    (Value::Object(values), Value::Object(states))
}

/// The cap sentence of a rolled-up row: `level 1, capped by svc:a`.
#[must_use]
pub fn cap_sentence(node: &NodeScore) -> Option<String> {
    if node.cap.origin.is_empty() {
        return node
            .cap
            .reason
            .as_ref()
            .map(|reason| format!("level {}, held by: {reason}", node.level));
    }
    Some(format!(
        "level {}, capped by {}",
        node.level,
        node.cap.origin.join(", ")
    ))
}

/// One point of the maturity history: the quarter that ends on the
/// as-of day, with the level and index shown on that day.
#[must_use]
pub fn history_point(as_of_ns: i64, level: u8, shown_index: f64) -> Value {
    json!({
        "quarter": quarter_label(day_of(as_of_ns)),
        "as_of_ns": as_of_ns,
        "level": level,
        "index": index(shown_index),
    })
}

/// `YYYY-Qn` of a day number.
#[must_use]
pub fn quarter_label(day: i64) -> String {
    let date = format_day(day);
    let month: u32 = date.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
    format!("{}-Q{}", &date[..4], (month - 1) / 3 + 1)
}

/// The last nanosecond of each of the `count` quarters that ended before
/// `as_of_ns`, oldest first.
#[must_use]
pub fn quarter_ends(as_of_ns: i64, count: usize) -> Vec<i64> {
    let date = format_day(day_of(as_of_ns));
    let mut year: i64 = date.get(..4).and_then(|y| y.parse().ok()).unwrap_or(1970);
    let month: i64 = date.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
    let mut quarter = (month - 1) / 3;
    let mut ends = Vec::with_capacity(count);
    for _ in 0..count {
        // The first day of the quarter, less one day, ends the one before.
        let first = days_from_civil(year, quarter * 3 + 1, 1);
        ends.push(end_of_day(first - 1));
        if quarter == 0 {
            year -= 1;
            quarter = 4;
        }
        quarter -= 1;
    }
    ends.reverse();
    ends
}

/// The maturity of the legal entity at the end of each of the last
/// [`HISTORY_QUARTERS`] quarters the lake has anything for, then today's.
///
/// # Errors
/// As [`loader::load`].
pub fn history(
    reader: &Reader,
    request: &Request<'_>,
    params: &HouseParams,
    today: &Assessment,
) -> Result<Vec<Value>, LoadError> {
    let mut points = Vec::new();
    for as_of_ns in quarter_ends(request.as_of_ns, HISTORY_QUARTERS) {
        let past = Request {
            as_of_ns,
            ..*request
        };
        let loaded = loader::load(reader, past, params)?;
        if loaded.inputs.evidence.is_empty() && loaded.inputs.services.is_empty() {
            continue;
        }
        let past = assess(&Matrix::build(&loaded.inputs, params));
        points.push(history_point(as_of_ns, past.level(), past.index()));
    }
    points.push(history_point(
        request.as_of_ns,
        today.level(),
        today.index(),
    ));
    Ok(points)
}

/// The maturity document. `by` selects the node rows listed under
/// `nodes`; the root row is always under `rollup`.
#[must_use]
pub fn maturity(
    loaded: &Loaded,
    assessment: &Assessment,
    by: Option<MaturityBy>,
    history: Vec<Value>,
) -> Value {
    let m = &assessment.entity;
    let root = assessment.root();
    let (values, states) = components(m);
    let mut map = header(loaded);
    map.insert(
        "scale".into(),
        json!({ "internal": true, "source": SCALE_SOURCE, "levels": SCALE_LEVELS }),
    );
    map.insert("level".into(), json!(assessment.level()));
    map.insert("index".into(), json!(index(assessment.index())));
    map.insert("flat_level".into(), json!(m.level));
    map.insert("flat_index".into(), json!(index(m.index)));
    map.insert("target".into(), json!(m.target));
    map.insert("headline".into(), json!(assessment.headline()));
    map.insert("gate_level".into(), json!(m.gate_level));
    map.insert("capped".into(), json!(assessment.capped()));
    map.insert("cap".into(), shown_cap(assessment));
    map.insert("components".into(), values);
    map.insert("component_states".into(), states);
    map.insert("weights".into(), weights_json(&m.weights));
    map.insert(
        "conformance_shares".into(),
        json!(m.pillars.shares.map(|shares| shares.map(index))),
    );
    map.insert(
        "gates".into(),
        Value::Array(
            m.gates
                .iter()
                .map(|g| json!({ "level": g.level, "passed": g.passed, "failing": g.failing }))
                .collect(),
        ),
    );
    map.insert("unknown_share".into(), json!(share(m.unknown_share)));
    map.insert("history".into(), Value::Array(history));
    map.insert("rollup".into(), json!(root.map(node_json)));
    map.insert("rollup_headline".into(), json!(root.and_then(cap_sentence)));
    map.insert("by".into(), json!(by.map(MaturityBy::as_str)));
    let rows: Vec<Value> = by.map_or_else(Vec::new, |by| {
        assessment
            .nodes
            .iter()
            .filter(|n| by.lists(n.kind))
            .map(node_json)
            .collect()
    });
    map.insert("nodes".into(), Value::Array(rows));
    Value::Object(map)
}

/// Today's status of every cell as rows of `requirement_status_daily`.
#[must_use]
pub fn snapshot_rows(matrix: &Matrix<'_>) -> Vec<RequirementStatusRow> {
    let today = day_of(matrix.ctx.inputs.as_of_ns);
    status::daily(matrix)
        .into_iter()
        .map(|row| RequirementStatusRow {
            as_of_day: today,
            function_id: row.function_id,
            requirement_id: row.requirement_id.to_string(),
            status: row.status.as_str().to_string(),
            value: row.value,
            source: SNAPSHOT_SOURCE.to_string(),
        })
        .collect()
}

/// Today's status rows of every registered legal entity, over all
/// environments: what the daily job stores.
///
/// # Errors
/// As [`loader::load`].
pub fn snapshot_all(
    reader: &Reader,
    params: &HouseParams,
    workloads: &[Workload],
    as_of_ns: i64,
) -> Result<Vec<RequirementStatusRow>, LoadError> {
    let request = Request {
        envs: &[],
        lei: None,
        profile: None,
        as_of_ns,
        workloads,
    };
    let first = loader::load(reader, request, params)?;
    let mut rows = snapshot_rows(&Matrix::build(&first.inputs, params));
    let others = first.entities.iter().filter(|e| e.lei != first.entity.lei);
    for entity in others {
        let one = Request {
            lei: Some(&entity.lei),
            ..request
        };
        let loaded = loader::load(reader, one, params)?;
        rows.extend(snapshot_rows(&Matrix::build(&loaded.inputs, params)));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use tumult_lake::IctLegalEntity;

    use super::super::model::{IncidentFeed, Inputs, Service, Tier};
    use super::super::worked::{bank_a, firm_b, ns};
    use super::*;

    fn loaded(inputs: Inputs) -> Loaded {
        let entity = IctLegalEntity {
            lei: "529900T8BM49AURSDO55".into(),
            name: "Worked Example".into(),
            profile: inputs.entity.profile.as_str().into(),
            ..IctLegalEntity::default()
        };
        Loaded {
            entities: vec![entity.clone()],
            feeds: super::super::loader::feed_states(&entity),
            entity,
            register: Vec::new(),
            inputs,
        }
    }

    #[test]
    fn every_document_lists_the_feeds_with_what_the_lake_holds() {
        let doc = dora_of(firm_b(), DoraBy::Entity);
        let feeds = doc["feeds"].as_array().expect("feeds");
        assert_eq!(feeds.len(), 5);
        assert_eq!(feeds[0]["name"], "incident");
        assert_eq!(feeds[0]["declared"], false);
        assert_eq!(feeds[0]["connected"], false);
        assert!(feeds[0]["reason"]
            .as_str()
            .unwrap()
            .contains("no ingest path"));
    }

    fn worked_ns() -> i64 {
        ns("2026-11-13")
    }

    fn dora_of(inputs: Inputs, by: DoraBy) -> Value {
        let loaded = loaded(inputs);
        let params = HouseParams::default();
        let matrix = Matrix::build(&loaded.inputs, &params);
        dora(&loaded, &matrix, by)
    }

    fn maturity_of(inputs: Inputs, by: Option<MaturityBy>) -> Value {
        let loaded = loaded(inputs);
        let params = HouseParams::default();
        let matrix = Matrix::build(&loaded.inputs, &params);
        maturity(&loaded, &assess(&matrix), by, Vec::new())
    }

    fn list(value: &Value) -> &Vec<Value> {
        value
            .as_array()
            .unwrap_or_else(|| panic!("{value} is not a list"))
    }

    fn cell<'v>(doc: &'v Value, article: &str, entity: &str) -> Option<&'v Value> {
        list(&doc["cells"])
            .iter()
            .find(|c| c["article"] == article && c["entity_id"] == entity)
    }

    #[test]
    fn the_simplified_profile_lists_no_article_5_to_15() {
        let doc = dora_of(firm_b(), DoraBy::Entity);
        assert_eq!(doc["profile"], "simplified");
        let listed: Vec<u32> = list(&doc["articles"])
            .iter()
            .filter_map(|a| a["article"].as_str()?.parse().ok())
            .collect();
        assert!(!listed.is_empty());
        assert!(
            listed.iter().all(|n| !(5..=15).contains(n)),
            "listed {listed:?}"
        );
        for cell in list(&doc["cells"]) {
            let n: u32 = cell["article"].as_str().unwrap().parse().unwrap_or(0);
            assert!(!(5..=15).contains(&n), "cell {cell}");
        }
        // The rows that do not apply are counted, not hidden.
        assert!(doc["na"]["count"].as_u64().unwrap() > 0);

        let full = dora_of(bank_a(), DoraBy::Entity);
        assert!(list(&full["articles"]).iter().any(|a| a["article"] == "11"));
    }

    #[test]
    fn every_number_carries_its_unknown_share() {
        for doc in [
            dora_of(bank_a(), DoraBy::Entity),
            dora_of(firm_b(), DoraBy::Org),
        ] {
            assert!(doc["portfolio"]["unknown"].is_number());
            for article in list(&doc["articles"]) {
                assert!(article["unknown"].is_number(), "{article}");
            }
            for cell in list(&doc["cells"]) {
                assert!(cell["unknown"].is_number(), "{cell}");
            }
            for function in list(&doc["functions"]) {
                assert!(function["unknown"].is_number(), "{function}");
            }
        }
    }

    #[test]
    fn article_19_is_unknown_without_the_feed_and_has_no_events_with_it() {
        let connected = dora_of(firm_b(), DoraBy::Entity);
        let dash = cell(&connected, "19", "cif-0001").unwrap();
        assert_eq!(dash["state"], "no_events");
        assert_eq!(dash["coverage"], Value::Null);
        assert_eq!(dash["unknown"], 0.0);

        let mut inputs = firm_b();
        inputs.incidents = IncidentFeed::NotConnected;
        let blind = dora_of(inputs, DoraBy::Entity);
        let hatched = cell(&blind, "19", "cif-0001").unwrap();
        assert_eq!(hatched["state"], "unknown");
        assert_eq!(hatched["unknown"], 1.0);
        assert_eq!(hatched["status"], "unknown");
    }

    #[test]
    fn worked_example_a_portfolio_and_requirement_rows() {
        let doc = dora_of(bank_a(), DoraBy::Entity);
        assert_eq!(doc["registry_version"], REQUIREMENTS_VERSION);
        assert_eq!(doc["disclaimer"], EVIDENCE_DISCLAIMER);
        assert_eq!(doc["portfolio"]["band"], "good");
        let coverage = doc["portfolio"]["coverage"].as_f64().unwrap();
        assert_eq!(rounded(coverage, 2), 0.96);

        let art24 = list(&doc["articles"])
            .iter()
            .find(|a| a["article"] == "24")
            .unwrap();
        let row = list(&art24["requirements"])
            .iter()
            .find(|r| r["id"] == "dora.art24.6")
            .unwrap();
        assert_eq!(row["weight"], 5);
        assert_eq!(row["evidence"], "test");
        assert_eq!(row["state"], "known");
        let entity = &list(&row["entities"])[0];
        assert_eq!(entity["entity_id"], "cif-0001");
        assert_eq!(entity["tier"], "critical");
        assert!(entity["evidence_id"].is_string());
        assert!(entity["v"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn source_shares_sum_to_one_per_article() {
        let doc = dora_of(bank_a(), DoraBy::Entity);
        for article in list(&doc["articles"]) {
            let s = &article["sources"];
            let total: f64 = ["otel_only", "tested", "manual", "none", "unknown"]
                .iter()
                .map(|k| s[*k].as_f64().unwrap())
                .sum();
            assert!(
                total == 0.0 || (total - 1.0).abs() < 1e-3,
                "article {} sums to {total}",
                article["article"]
            );
        }
        let art24 = list(&doc["articles"])
            .iter()
            .find(|a| a["article"] == "24")
            .unwrap();
        assert!(art24["sources"]["tested"].as_f64().unwrap() > 0.5);
    }

    #[test]
    fn the_expectation_table_is_sorted_worst_first() {
        let doc = dora_of(firm_b(), DoraBy::Entity);
        let rows = list(&doc["expectations"]);
        assert!(!rows.is_empty());
        let priorities: Vec<f64> = rows
            .iter()
            .map(|r| r["priority"].as_f64().unwrap())
            .collect();
        assert!(
            priorities.windows(2).all(|w| w[0] >= w[1]),
            "{priorities:?}"
        );
        for row in rows {
            assert!(row["expected"].is_string(), "{row}");
            assert!(row["actual"].is_string(), "{row}");
            let gap = row["gap"].as_u64().unwrap();
            let weight = row["weight"].as_f64().unwrap();
            let criticality = row["criticality"].as_f64().unwrap();
            assert_eq!(
                row["priority"].as_f64().unwrap(),
                gap as f64 * weight * criticality
            );
        }
        let gap = rows.iter().find(|r| r["gap"].as_u64() > Some(0)).unwrap();
        assert!(!list(&gap["short"]).is_empty());
    }

    #[test]
    fn a_function_waiting_for_operating_shows_the_days_left() {
        let doc = dora_of(firm_b(), DoraBy::Entity);
        let waiting: Vec<&Value> = list(&doc["expectations"])
            .iter()
            .filter(|r| r["days_until_operating"].is_number())
            .collect();
        assert!(!waiting.is_empty());
        for row in waiting {
            let days = row["days_until_operating"].as_u64().unwrap();
            assert!((1..=180).contains(&days), "{row}");
        }
    }

    #[test]
    fn org_rows_cover_the_units_and_the_legal_entity() {
        let mut inputs = bank_a();
        inputs.functions[0].org_unit = Some("bank/payments".into());
        inputs.functions[1].org_unit = Some("bank/payments".into());
        inputs.functions[2].org_unit = Some("bank/platform".into());
        let doc = dora_of(inputs.clone(), DoraBy::Org);
        let ids: Vec<&str> = list(&doc["rows"])
            .iter()
            .filter_map(|r| r["entity_id"].as_str())
            .collect();
        assert_eq!(
            ids,
            ["org:", "org:bank", "org:bank/payments", "org:bank/platform"]
        );
        // The root row is the legal entity's own article coverage.
        let root = cell(&doc, "24", "org:").unwrap();
        let article = list(&doc["articles"])
            .iter()
            .find(|a| a["article"] == "24")
            .unwrap();
        assert_eq!(root["coverage"], article["coverage"]);
        // A unit with one function reads as that function.
        let by_entity = dora_of(inputs, DoraBy::Entity);
        assert_eq!(
            cell(&doc, "24", "org:bank/platform").unwrap()["coverage"],
            cell(&by_entity, "24", "cif-0003").unwrap()["coverage"]
        );
    }

    #[test]
    fn the_article_document_lists_every_evidence_row() {
        let loaded = loaded(bank_a());
        let params = HouseParams::default();
        let matrix = Matrix::build(&loaded.inputs, &params);
        let doc = article(&loaded, &matrix, "24", Some("cif-0001")).unwrap();
        assert_eq!(doc["article"], "24");
        assert_eq!(doc["entity"], "cif-0001");
        let row = list(&doc["requirements"])
            .iter()
            .find(|r| r["id"] == "dora.art24.6")
            .unwrap();
        let entities = list(&row["entities"]);
        assert_eq!(entities.len(), 1);
        let evidence = list(&entities[0]["evidence"]);
        assert!(evidence.len() > 1, "more than the best row");
        for e in evidence {
            for key in ["phi", "o", "s", "v"] {
                let value = e[key].as_f64().unwrap();
                assert!((0.0..=1.0).contains(&value), "{key} of {e}");
            }
            assert!(e["evidence_id"].is_string());
        }
        // Best first.
        assert_eq!(evidence[0]["v"], entities[0]["v"]);

        let all = article(&loaded, &matrix, "24", None).unwrap();
        let row = list(&all["requirements"])
            .iter()
            .find(|r| r["id"] == "dora.art24.6")
            .unwrap();
        assert!(list(&row["entities"]).len() > 1);

        assert_eq!(
            article(&loaded, &matrix, "99", None),
            Err(ReportError::UnknownArticle("99".into()))
        );
        assert_eq!(
            article(&loaded, &matrix, "24", Some("cif-9999")),
            Err(ReportError::UnknownEntity("cif-9999".into()))
        );
    }

    #[test]
    fn worked_example_a_reads_level_4_index_4_42() {
        let doc = maturity_of(bank_a(), None);
        assert_eq!(doc["flat_level"], 4);
        assert_eq!(rounded(doc["flat_index"].as_f64().unwrap(), 2), 4.42);
        assert_eq!(doc["target"], 3);
        // The level and index shown are the roll-up root's.
        assert_eq!(doc["level"], doc["rollup"]["level"]);
        assert_eq!(doc["index"], doc["rollup"]["index"]);
        let shown = doc["level"].as_u64().unwrap();
        assert_eq!(doc["headline"], format!("level {shown} of target 3 (full)"));
        assert_eq!(doc["scale"]["internal"], true);
        assert_eq!(list(&doc["scale"]["levels"]).len(), 5);
        assert_eq!(doc["weights"]["slo"], 0.25);
        assert!(list(&doc["nodes"]).is_empty());
        assert_eq!(doc["rollup"]["id"], ROOT_ID);
    }

    #[test]
    fn worked_example_b_reads_level_2_of_target_3_with_its_cap() {
        let doc = maturity_of(firm_b(), None);
        assert_eq!(doc["flat_level"], 2);
        assert_eq!(rounded(doc["flat_index"].as_f64().unwrap(), 2), 3.60);
        assert_eq!(doc["level"], 2);
        assert_eq!(doc["index"], doc["rollup"]["index"]);
        assert_eq!(doc["headline"], "level 2 of target 3 (simplified)");
        assert_eq!(doc["capped"], true);
        assert_eq!(doc["cap"]["rule"], "gate");
        assert_eq!(doc["cap"]["level"], 2);
        assert_eq!(doc["cap"]["component"], "evidence");
        let reason = doc["cap"]["reason"].as_str().unwrap();
        assert!(reason.starts_with("level 3 blocked by: "), "{reason}");
        // S is not declared under the simplified framework: excluded.
        assert_eq!(doc["components"]["slo"], Value::Null);
        assert_eq!(doc["component_states"]["slo"]["state"], "not_declared");
        let gate3 = list(&doc["gates"])
            .iter()
            .find(|g| g["level"] == 3)
            .unwrap();
        assert_eq!(gate3["passed"], false);
        assert!(!list(&gate3["failing"]).is_empty());
    }

    #[test]
    fn by_selects_the_node_rows() {
        let kinds = |by: MaturityBy| -> BTreeSet<String> {
            list(&maturity_of(bank_a(), Some(by))["nodes"])
                .iter()
                .map(|n| n["kind"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(kinds(MaturityBy::Entity), BTreeSet::from(["entity".into()]));
        assert_eq!(
            kinds(MaturityBy::Org),
            BTreeSet::from(["organisation".into()])
        );
        let all = kinds(MaturityBy::Node);
        assert!(all.contains("service") && all.contains("entity"));

        let doc = maturity_of(bank_a(), Some(MaturityBy::Node));
        for node in list(&doc["nodes"]) {
            assert!(node["cap"]["rule"].is_string(), "{node}");
            assert!(node["unknown_share"].is_number(), "{node}");
            assert!(node["rollup"]["weight"].is_number(), "{node}");
        }
    }

    #[test]
    fn an_uninstrumented_critical_service_caps_the_organisation_by_name() {
        let mut inputs = firm_b();
        inputs
            .services
            .push(Service::silent("payments-ledger-archive", &["cif-0001"]));
        assert_eq!(inputs.functions[0].tier, Tier::Critical);
        let doc = maturity_of(inputs, Some(MaturityBy::Org));
        let root = &doc["rollup"];
        assert_eq!(root["level"], 1);
        assert_eq!(root["cap"]["rule"], "weakest_critical_child");
        assert_eq!(
            root["cap"]["origin"],
            json!(["svc:payments-ledger-archive"])
        );
        assert_eq!(root["cap"]["origin_state"], "unknown");
        assert_eq!(
            doc["rollup_headline"],
            "level 1, capped by svc:payments-ledger-archive"
        );
        // The page shows the roll-up, and the flat pillar result next to it.
        assert_eq!(doc["level"], 1);
        assert_eq!(doc["flat_level"], 2);
        assert_eq!(doc["headline"], "level 1 of target 3 (simplified)");
        assert_eq!(doc["capped"], true);
        assert_eq!(doc["cap"]["rule"], "weakest_critical_child");
        assert_eq!(doc["cap"]["level"], 1);
        assert_eq!(doc["cap"]["origin"], json!(["svc:payments-ledger-archive"]));
        assert_eq!(
            doc["cap"]["reason"],
            "capped by svc:payments-ledger-archive (critical, no telemetry and no test)"
        );
    }

    #[test]
    fn a_history_point_carries_the_shown_level_and_index() {
        let point = history_point(worked_ns(), 2, 3.4567);
        assert_eq!(point["level"], 2);
        assert_eq!(point["index"], 3.457);
        assert_eq!(point["quarter"], "2026-Q4");
    }

    #[test]
    fn the_by_parameters_are_parsed_strictly() {
        assert_eq!(DoraBy::parse(None), Ok(DoraBy::Entity));
        assert_eq!(DoraBy::parse(Some("org")), Ok(DoraBy::Org));
        assert!(DoraBy::parse(Some("node")).is_err());
        assert_eq!(MaturityBy::parse(None), Ok(None));
        assert_eq!(MaturityBy::parse(Some("node")), Ok(Some(MaturityBy::Node)));
        assert!(MaturityBy::parse(Some("env")).is_err());
    }

    #[test]
    fn snapshot_rows_store_today_for_every_cell() {
        let inputs = firm_b();
        let params = HouseParams::default();
        let matrix = Matrix::build(&inputs, &params);
        let rows = snapshot_rows(&matrix);
        let cells: usize = matrix.rows.iter().map(|r| r.cells.len()).sum();
        assert_eq!(rows.len(), cells);
        let today = day_of(inputs.as_of_ns);
        assert!(rows
            .iter()
            .all(|r| r.as_of_day == today && r.source == SNAPSHOT_SOURCE));
        let unknown = rows.iter().find(|r| r.status == "no_events").unwrap();
        assert_eq!(unknown.value, None);
        let scored = rows.iter().find(|r| r.status == "existing").unwrap();
        assert!(scored.value.unwrap() >= params.existing_threshold);
    }

    #[test]
    fn the_daily_snapshot_covers_every_legal_entity_and_is_read_back() {
        const ORG: &str = "
legal_entities:
  - {lei: 529900T8BM49AURSDO55, name: Firm One, profile: simplified}
  - {lei: 506700GE1G29325QX363, name: Firm Two, profile: full}
functions:
  - {id: cif-0001, lei: 529900T8BM49AURSDO55, name: Payments, tier: critical}
  - {id: cif-0002, lei: 506700GE1G29325QX363, name: Custody, tier: important}
";
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
        let declared = super::super::register::parse(ORG).unwrap();
        let now = 1_800_000_000 * 1_000_000_000_i64;
        let writer = store.writer().unwrap();
        writer.sync_ict_register(&declared.ict, now).unwrap();
        let params = HouseParams::default();

        let rows = snapshot_all(&store.read_only().unwrap(), &params, &[], now).unwrap();
        let functions: BTreeSet<&str> = rows.iter().map(|r| r.function_id.as_str()).collect();
        assert_eq!(functions, BTreeSet::from(["cif-0001", "cif-0002"]));
        assert!(rows
            .iter()
            .all(|r| r.as_of_day == day_of(now) && r.source == SNAPSHOT_SOURCE));
        // The simplified firm has no Art. 9 row, the full one does.
        let has = |function: &str, prefix: &str| {
            rows.iter()
                .any(|r| r.function_id == function && r.requirement_id.starts_with(prefix))
        };
        assert!(!has("cif-0001", "dora.art9."));
        assert!(has("cif-0002", "dora.art9."));

        writer.upsert_requirement_status(&rows, now).unwrap();
        let request = Request {
            envs: &[],
            lei: None,
            profile: None,
            as_of_ns: now,
            workloads: &[],
        };
        let loaded = loader::load(&store.read_only().unwrap(), request, &params).unwrap();
        assert!(!loaded.inputs.snapshots.is_empty());
    }

    #[test]
    fn quarter_ends_walk_back_over_the_year_boundary() {
        use super::super::model::parse_day;
        let as_of = end_of_day(parse_day("2026-05-12").unwrap()) - 5;
        let days: Vec<String> = quarter_ends(as_of, 3)
            .into_iter()
            .map(|ns| format_day(day_of(ns)))
            .collect();
        assert_eq!(days, ["2025-09-30", "2025-12-31", "2026-03-31"]);
        // The last day of a quarter is not yet a finished quarter.
        let last = end_of_day(parse_day("2026-03-31").unwrap());
        assert_eq!(format_day(day_of(quarter_ends(last, 1)[0])), "2025-12-31");
    }

    #[test]
    fn quarters_are_labelled_by_calendar_quarter() {
        use super::super::model::parse_day;
        assert_eq!(quarter_label(parse_day("2026-09-30").unwrap()), "2026-Q3");
        assert_eq!(quarter_label(parse_day("2026-10-01").unwrap()), "2026-Q4");
        assert_eq!(quarter_label(parse_day("2026-01-15").unwrap()), "2026-Q1");
    }

    #[test]
    fn bands_follow_the_house_thresholds() {
        let params = HouseParams::default();
        assert_eq!(band(None, &params), "unknown");
        assert_eq!(band(Some(0.8), &params), "good");
        assert_eq!(band(Some(0.5), &params), "fair");
        assert_eq!(band(Some(0.49), &params), "weak");
    }
}
