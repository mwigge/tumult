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
use tumult_lake::{MaturityDailyRow, Reader, RequirementStatusRow, StoreError, Writer};

use super::coverage::{CellState, Matrix, MatrixCell, MatrixRow, RowState, Scored};
use super::loader::{
    self, LoadError, Loaded, Request, PROOF_DAYS_NOTE, PROOF_SOURCE, SNAPSHOT_SOURCE,
};
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
/// Days of daily snapshots the lake keeps; older days are deleted each
/// time a snapshot is stored. The Operating streak and the two-quarter
/// gate fact read the last 183 days, and the history reads the current
/// quarter and the four before it (at most 458 days), so 800 days keeps
/// both whole with more than a year to spare.
pub const SNAPSHOT_RETENTION_DAYS: i64 = 800;
/// Said in every document served to a caller limited to some
/// environments.
pub const SCOPE_NOTE: &str = "Partial view: only the functions with telemetry or evidence in \
     your environments are listed, and the level is of that part alone. Register data, stored \
     history and the Operating status cover every environment and are not shown.";
/// Why a view other than the stored one has no past quarters.
pub const HISTORY_NOT_STORED: &str = "Stored days are of every environment under the \
     registered profile; this view has none.";
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
        "window_days": req.window_for(&ctx.inputs.entity),
        "scope": req.scope.as_str(),
        "scope_basis": req.scope_basis.as_str(),
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
        "scope_basis": req.scope_basis.as_str(),
        "window_days": req.window_for(&inputs.entity),
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
        "profile_overridden".into(),
        json!(loaded.profile_overridden),
    );
    map.insert("registered_profile".into(), json!(loaded.entity.profile));
    map.insert("scoped".into(), json!(loaded.scoped));
    map.insert(
        "scope_note".into(),
        json!(loaded.scoped.then_some(SCOPE_NOTE)),
    );
    map.insert(
        "legal_entities".into(),
        Value::Array(legal_entities(loaded)),
    );
    map.insert("feeds".into(), Value::Array(feeds(loaded)));
    map.insert(
        "proof_days".into(),
        json!({
            "enabled": loaded.proof.enabled,
            "counted": loaded.proof.counted,
            "ignored": loaded.proof.ignored,
            "note": loaded.proof.enabled.then_some(PROOF_DAYS_NOTE),
        }),
    );
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

    /// The unknown share the shown index is read with: the roll-up
    /// root's, computed over the same tree as [`Self::index`].
    #[must_use]
    pub fn unknown_share(&self) -> f64 {
        self.root()
            .map_or(self.entity.unknown_share, |root| root.unknown_share)
    }

    /// `level n of target m (profile), p % unknown`.
    #[must_use]
    pub fn headline(&self) -> String {
        format!(
            "level {} of target {} ({}), {} % unknown",
            self.level(),
            self.entity.target,
            self.entity.label,
            (self.unknown_share() * 100.0).round()
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

/// The roll-up root in a few words, for the heading over the node table:
/// what caps it, or its level against the target when nothing does.
#[must_use]
pub fn rollup_headline(node: &NodeScore) -> String {
    if let Some(capped) = cap_sentence(node) {
        return capped;
    }
    format!("level {} of target {}", node.level, node.target)
}

/// One point of the maturity history: the quarter `day` lies in, with
/// the level, index and unknown share shown on that day. `state` is
/// `stored` for a stored day and `today` for the live one; `source` is
/// who stored the day (the daily job, or the proof tool).
#[must_use]
pub fn history_point(
    state: &str,
    source: Option<&str>,
    day: i64,
    level: u8,
    shown_index: f64,
    unknown_share: f64,
) -> Value {
    json!({
        "quarter": quarter_label(day),
        "state": state,
        "source": source,
        "as_of": format_day(day),
        "as_of_ns": end_of_day(day),
        "level": level,
        "index": index(shown_index),
        "unknown_share": share(unknown_share),
        "reason": null,
    })
}

/// A quarter with no stored day.
fn no_data_point(quarter: &str, reason: Option<&str>) -> Value {
    json!({
        "quarter": quarter,
        "state": "no_data",
        "source": null,
        "as_of": null,
        "as_of_ns": null,
        "level": null,
        "index": null,
        "unknown_share": null,
        "reason": reason,
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

/// The maturity of the legal entity in each of the last
/// [`HISTORY_QUARTERS`] finished quarters, then today's.
///
/// A past quarter shows what the daily job stored on its last stored
/// day: the level, index and unknown share as they were computed then,
/// against the register and the evidence of that day. Nothing is
/// recomputed, so a later change of the register does not rewrite the
/// past. A quarter with no stored day is `no_data`. Stored days are of
/// every environment under the registered profile, so a scoped caller
/// and a profile override get `no_data` for every past quarter. A day
/// of the proof tool is shown only with the opt-in, marked by its source.
///
/// # Errors
/// `Store` when the stored days cannot be read.
pub fn history(
    reader: &Reader,
    loaded: &Loaded,
    today: &Assessment,
) -> Result<Vec<Value>, LoadError> {
    let as_of_ns = loaded.inputs.as_of_ns;
    let ends: Vec<i64> = quarter_ends(as_of_ns, HISTORY_QUARTERS)
        .into_iter()
        .map(day_of)
        .collect();
    let other_view = loaded.scoped || loaded.profile_overridden;
    let stored = match (ends.first(), ends.last()) {
        (Some(first), Some(last)) if !other_view => reader
            .maturity_daily_between(&loaded.entity.lei, first - QUARTER_MAX_DAYS, *last)
            .map_err(|e| LoadError::Store(e.to_string()))?,
        _ => Vec::new(),
    };
    let reason = other_view.then_some(HISTORY_NOT_STORED);
    let mut points: Vec<Value> = ends
        .iter()
        .map(|end| {
            let quarter = quarter_label(*end);
            let last = stored
                .iter()
                .rev()
                .filter(|row| loader::day_counts(&row.source, loaded.proof.enabled))
                .find(|row| quarter_label(row.as_of_day) == quarter);
            match last {
                Some(row) => history_point(
                    "stored",
                    Some(&row.source),
                    row.as_of_day,
                    row.level,
                    row.index,
                    row.unknown_share,
                ),
                None => no_data_point(&quarter, reason),
            }
        })
        .collect();
    points.push(history_point(
        "today",
        None,
        day_of(as_of_ns),
        today.level(),
        today.index(),
        today.unknown_share(),
    ));
    Ok(points)
}

/// No quarter is longer than this many days.
const QUARTER_MAX_DAYS: i64 = 92;

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
    map.insert("flat_unknown_share".into(), json!(share(m.unknown_share)));
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
    map.insert(
        "unknown_share".into(),
        json!(share(assessment.unknown_share())),
    );
    map.insert("history".into(), Value::Array(history));
    map.insert("rollup".into(), json!(root.map(node_json)));
    map.insert("rollup_headline".into(), json!(root.map(rollup_headline)));
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

/// What the daily job stores for one day: the status of every cell and
/// the level shown for every legal entity.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub status: Vec<RequirementStatusRow>,
    pub maturity: Vec<MaturityDailyRow>,
}

impl Snapshot {
    fn add(&mut self, loaded: &Loaded, params: &HouseParams) {
        let matrix = Matrix::build(&loaded.inputs, params);
        let assessment = assess(&matrix);
        self.status.extend(snapshot_rows(&matrix));
        self.maturity.push(MaturityDailyRow {
            as_of_day: day_of(loaded.inputs.as_of_ns),
            lei: loaded.entity.lei.clone(),
            level: assessment.level(),
            index: assessment.index(),
            unknown_share: assessment.unknown_share(),
            source: SNAPSHOT_SOURCE.to_string(),
        });
    }

    /// The same day marked as stored by the proof tool, for a past date:
    /// read back only by a daemon started with the proof opt-in.
    #[must_use]
    pub fn as_proof(mut self) -> Self {
        for row in &mut self.status {
            row.source = PROOF_SOURCE.to_string();
        }
        for row in &mut self.maturity {
            row.source = PROOF_SOURCE.to_string();
        }
        self
    }

    /// The day the snapshot describes.
    fn day(&self) -> Option<i64> {
        self.maturity.iter().map(|row| row.as_of_day).max()
    }

    /// Store the day, replacing what was stored for it, and delete the
    /// days older than [`SNAPSHOT_RETENTION_DAYS`] before it. Returns
    /// the number of rows deleted.
    ///
    /// # Errors
    /// Returns an error if a statement fails.
    pub fn store(&self, writer: &Writer, computed_at_ns: i64) -> Result<usize, StoreError> {
        writer.upsert_requirement_status(&self.status, computed_at_ns)?;
        writer.upsert_maturity_daily(&self.maturity, computed_at_ns)?;
        let Some(day) = self.day() else {
            return Ok(0);
        };
        writer.prune_score_snapshots(day - SNAPSHOT_RETENTION_DAYS)
    }
}

/// Today's snapshot of every registered legal entity, over all
/// environments under the registered profile: what the daily job stores.
/// `proof_days` lets the days of the proof tool count towards it.
///
/// # Errors
/// As [`loader::load`].
pub fn snapshot_all(
    reader: &Reader,
    params: &HouseParams,
    workloads: &[Workload],
    as_of_ns: i64,
    proof_days: bool,
) -> Result<Snapshot, LoadError> {
    let request = Request {
        envs: &[],
        lei: None,
        profile: None,
        as_of_ns,
        workloads,
        proof_days,
    };
    let first = loader::load(reader, request, params)?;
    let mut snapshot = Snapshot::default();
    snapshot.add(&first, params);
    let others = first.entities.iter().filter(|e| e.lei != first.entity.lei);
    for entity in others {
        let one = Request {
            lei: Some(&entity.lei),
            ..request
        };
        snapshot.add(&loader::load(reader, one, params)?, params);
    }
    Ok(snapshot)
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
            profile_overridden: false,
            scoped: false,
            proof: loader::ProofDays::default(),
            inputs,
        }
    }

    /// The index shown is the roll-up root's, so the unknown share beside
    /// it is the root's too, and the headline says it.
    #[test]
    fn the_unknown_share_shown_is_the_one_of_the_rolled_up_index() {
        let mut inputs = firm_b();
        inputs
            .services
            .push(Service::silent("payments-ledger-archive", &["cif-0001"]));
        let doc = maturity_of(inputs, None);
        assert_eq!(doc["index"], doc["rollup"]["index"]);
        assert_eq!(doc["unknown_share"], doc["rollup"]["unknown_share"]);
        let rolled = doc["unknown_share"].as_f64().unwrap();
        let flat = doc["flat_unknown_share"].as_f64().unwrap();
        assert!((rolled - flat).abs() > 0.01, "rolled {rolled}, flat {flat}");
        let percent = (rolled * 100.0).round();
        assert_eq!(
            doc["headline"],
            format!("level 1 of target 3 (simplified), {percent} % unknown")
        );
    }

    /// The reader sees which scope restriction is the house's and that a
    /// microenterprise has no window on the framework review.
    #[test]
    fn a_row_says_when_its_scope_is_house_and_when_it_has_no_window() {
        let mut inputs = bank_a();
        let by_id = |doc: &Value, id: &str| {
            list(&doc["requirements"])
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("no row {id}"))
                .clone()
        };
        let art24 = article_of(inputs.clone(), "24");
        assert_eq!(by_id(&art24, "dora.art24.1")["scope_basis"], "house");
        assert_eq!(by_id(&art24, "dora.art24.6")["scope_basis"], "regulation");
        let art6 = article_of(inputs.clone(), "6");
        assert_eq!(by_id(&art6, "dora.art6.5")["window_days"], 365);

        inputs.entity =
            tumult_core::compliance::EntityProfile::parse("full", &["microenterprise".to_string()])
                .unwrap();
        let art6 = article_of(inputs, "6");
        assert_eq!(by_id(&art6, "dora.art6.5")["window_days"], Value::Null);
    }

    /// The expectation table carries the marks of the article rows, so
    /// the page can label a house scope without a second request.
    #[test]
    fn an_expectation_says_when_its_scope_is_house_and_what_its_window_is() {
        let doc = dora_of(bank_a(), DoraBy::Entity);
        let by_id = |id: &str| {
            list(&doc["expectations"])
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("no row {id}"))
                .clone()
        };
        assert_eq!(by_id("dora.art24.1")["scope_basis"], "house");
        assert_eq!(by_id("dora.art24.6")["scope_basis"], "regulation");
        assert_eq!(by_id("dora.art6.5")["window_days"], 365);
    }

    fn article_of(inputs: Inputs, key: &str) -> Value {
        let loaded = loaded(inputs);
        let params = HouseParams::default();
        let matrix = Matrix::build(&loaded.inputs, &params);
        article(&loaded, &matrix, key, None).unwrap()
    }

    #[test]
    fn a_profile_override_and_a_scoped_view_are_named_in_every_document() {
        let params = HouseParams::default();
        let plain = dora_of(firm_b(), DoraBy::Entity);
        assert_eq!(plain["profile_overridden"], false);
        assert_eq!(plain["registered_profile"], "simplified");
        assert_eq!(plain["scoped"], false);
        assert_eq!(plain["scope_note"], Value::Null);

        let mut loaded = loaded(firm_b());
        loaded.profile_overridden = true;
        loaded.entity.profile = "full".into();
        loaded.scoped = true;
        let matrix = Matrix::build(&loaded.inputs, &params);
        for doc in [
            dora(&loaded, &matrix, DoraBy::Entity),
            article(&loaded, &matrix, "24", None).unwrap(),
            maturity(&loaded, &assess(&matrix), None, Vec::new()),
        ] {
            assert_eq!(doc["profile_overridden"], true);
            assert_eq!(doc["registered_profile"], "full");
            assert_eq!(doc["profile"], "simplified");
            assert_eq!(doc["scoped"], true);
            assert_eq!(doc["scope_note"], SCOPE_NOTE);
        }
    }

    fn stored_level(day: &str, level: u8, index: f64) -> tumult_lake::MaturityDailyRow {
        tumult_lake::MaturityDailyRow {
            as_of_day: super::super::model::parse_day(day).unwrap(),
            lei: "529900T8BM49AURSDO55".into(),
            level,
            index,
            unknown_share: 0.4,
            source: SNAPSHOT_SOURCE.into(),
        }
    }

    /// History is what was stored on the day, never today's register
    /// applied to old evidence; a quarter with no stored day says so.
    #[test]
    fn history_is_read_from_stored_days_and_a_quarter_without_one_is_no_data() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let mut other = stored_level("2026-06-25", 5, 4.9);
        other.lei = "506700GE1G29325QX363".into();
        let mut restated = stored_level("2026-06-28", 5, 4.9);
        restated.source = "backfill".into();
        writer
            .upsert_maturity_daily(
                &[
                    stored_level("2026-05-10", 1, 1.2),
                    stored_level("2026-06-20", 2, 2.4567),
                    other,
                    restated,
                    stored_level("2026-12-31", 3, 3.1),
                ],
                1,
            )
            .unwrap();
        drop(writer);
        let reader = store.read_only().unwrap();
        let mut inputs = firm_b();
        inputs.as_of_ns = ns("2027-01-15");
        let mut loaded = loaded(inputs);
        let params = HouseParams::default();
        let today = assess(&Matrix::build(&loaded.inputs, &params));

        let points = history(&reader, &loaded, &today).unwrap();
        let seen: Vec<(&str, &str)> = points
            .iter()
            .map(|p| (p["quarter"].as_str().unwrap(), p["state"].as_str().unwrap()))
            .collect();
        assert_eq!(
            seen,
            [
                ("2026-Q1", "no_data"),
                ("2026-Q2", "stored"),
                ("2026-Q3", "no_data"),
                ("2026-Q4", "stored"),
                ("2027-Q1", "today"),
            ]
        );
        assert_eq!(points[0]["level"], Value::Null);
        assert_eq!(points[0]["index"], Value::Null);
        assert_eq!(points[0]["unknown_share"], Value::Null);
        // The last stored day of the quarter, as stored.
        assert_eq!(points[1]["as_of"], "2026-06-20");
        assert_eq!(points[1]["level"], 2);
        assert_eq!(points[1]["index"], 2.457);
        assert_eq!(points[1]["unknown_share"], 0.4);
        assert_eq!(points[3]["as_of"], "2026-12-31");
        assert_eq!(points[4]["level"], today.level());
        assert_eq!(points[4]["index"], index(today.index()));
        assert_eq!(points[4]["unknown_share"], share(today.unknown_share()));

        // Stored days are of every environment under the registered
        // profile: another view has no history but today's.
        for (scoped, overridden) in [(true, false), (false, true)] {
            loaded.scoped = scoped;
            loaded.profile_overridden = overridden;
            let points = history(&reader, &loaded, &today).unwrap();
            assert_eq!(points.len(), 5);
            assert!(points[..4].iter().all(|p| p["state"] == "no_data"));
            assert_eq!(points[4]["state"], "today");
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
    fn worked_example_a_reads_level_4_index_4_44() {
        let doc = maturity_of(bank_a(), None);
        assert_eq!(doc["flat_level"], 4);
        assert_eq!(rounded(doc["flat_index"].as_f64().unwrap(), 2), 4.44);
        assert_eq!(doc["target"], 3);
        // The level and index shown are the roll-up root's.
        assert_eq!(doc["level"], doc["rollup"]["level"]);
        assert_eq!(doc["index"], doc["rollup"]["index"]);
        let shown = doc["level"].as_u64().unwrap();
        let unknown = (doc["unknown_share"].as_f64().unwrap() * 100.0).round();
        assert_eq!(
            doc["headline"],
            format!("level {shown} of target 3 (full), {unknown} % unknown")
        );
        assert_eq!(doc["scale"]["internal"], true);
        assert_eq!(list(&doc["scale"]["levels"]).len(), 5);
        assert_eq!(doc["weights"]["slo"], 0.25);
        assert!(list(&doc["nodes"]).is_empty());
        assert_eq!(doc["rollup"]["id"], ROOT_ID);
    }

    /// The roll-up root of bank A, with its cap as given.
    fn bank_a_root(cap: Cap) -> NodeScore {
        let loaded = loaded(bank_a());
        let params = HouseParams::default();
        let matrix = Matrix::build(&loaded.inputs, &params);
        let assessment = assess(&matrix);
        let root = assessment.root().expect("bank A has a root").clone();
        NodeScore { cap, ..root }
    }

    fn cap(rule: CapRule, origin: &[&str], reason: Option<&str>) -> Cap {
        Cap {
            rule,
            level: None,
            node_ids: Vec::new(),
            origin: origin.iter().map(ToString::to_string).collect(),
            origin_state: None,
            reason: reason.map(ToString::to_string),
        }
    }

    #[test]
    fn an_uncapped_roll_up_still_has_a_headline() {
        let root = bank_a_root(cap(CapRule::None, &[], None));
        let expected = format!("level {} of target 3", root.level);
        assert_eq!(rollup_headline(&root), expected);
    }

    #[test]
    fn a_capped_roll_up_names_what_caps_it() {
        let by_child = bank_a_root(cap(CapRule::WeakestCriticalChild, &["svc:a"], None));
        let level = by_child.level;
        assert_eq!(
            rollup_headline(&by_child),
            format!("level {level}, capped by svc:a")
        );
        let by_gate = bank_a_root(cap(CapRule::Gate, &[], Some("no evidence")));
        assert_eq!(
            rollup_headline(&by_gate),
            format!("level {level}, held by: no evidence")
        );
    }

    #[test]
    fn worked_example_b_reads_level_2_of_target_3_with_its_cap() {
        let doc = maturity_of(firm_b(), None);
        assert_eq!(doc["flat_level"], 2);
        assert_eq!(rounded(doc["flat_index"].as_f64().unwrap(), 2), 3.60);
        assert_eq!(doc["level"], 2);
        assert_eq!(doc["index"], doc["rollup"]["index"]);
        assert!(
            doc["headline"]
                .as_str()
                .unwrap()
                .starts_with("level 2 of target 3 (simplified), "),
            "{}",
            doc["headline"]
        );
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
        assert!(doc["headline"]
            .as_str()
            .unwrap()
            .starts_with("level 1 of target 3 (simplified), "));
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
    fn a_history_point_carries_the_shown_level_index_and_unknown_share() {
        let point = history_point("today", None, day_of(worked_ns()), 2, 3.4567, 0.123_45);
        assert_eq!(point["level"], 2);
        assert_eq!(point["index"], 3.457);
        assert_eq!(point["unknown_share"], 0.1235);
        assert_eq!(point["quarter"], "2026-Q4");
        assert_eq!(point["as_of"], "2026-11-13");
        assert_eq!(point["state"], "today");
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

        let snapshot = snapshot_all(&store.read_only().unwrap(), &params, &[], now, false).unwrap();
        let levels: Vec<(&str, u8)> = snapshot
            .maturity
            .iter()
            .map(|m| (m.lei.as_str(), m.level))
            .collect();
        assert_eq!(
            levels,
            [("506700GE1G29325QX363", 1), ("529900T8BM49AURSDO55", 1)]
        );
        assert!(snapshot
            .maturity
            .iter()
            .all(|m| m.as_of_day == day_of(now) && m.source == SNAPSHOT_SOURCE));
        assert!(snapshot.maturity.iter().all(|m| m.unknown_share > 0.0));
        let rows = &snapshot.status;
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

        // Storing keeps the retention: a day older than it goes.
        let today = day_of(now);
        let mut old = stored_level("2026-01-01", 2, 2.0);
        old.as_of_day = today - SNAPSHOT_RETENTION_DAYS - 1;
        let mut kept = old.clone();
        kept.as_of_day = today - SNAPSHOT_RETENTION_DAYS;
        writer.upsert_maturity_daily(&[old, kept], now).unwrap();
        assert_eq!(snapshot.store(&writer, now).unwrap(), 1, "one day pruned");
        let stored = store
            .read_only()
            .unwrap()
            .maturity_daily_between("529900T8BM49AURSDO55", 0, today)
            .unwrap();
        let days: Vec<i64> = stored.iter().map(|m| m.as_of_day).collect();
        assert_eq!(days, [today - SNAPSHOT_RETENTION_DAYS, today]);
        let request = Request {
            envs: &[],
            lei: None,
            profile: None,
            as_of_ns: now,
            workloads: &[],
            proof_days: false,
        };
        let loaded = loader::load(&store.read_only().unwrap(), request, &params).unwrap();
        assert!(!loaded.inputs.snapshots.is_empty());

        // The proof tool stores the same day under its own source, and
        // only a load with the opt-in reads it back.
        let proof = snapshot.clone().as_proof();
        assert!(proof.status.iter().all(|r| r.source == PROOF_SOURCE));
        assert!(proof.maturity.iter().all(|m| m.source == PROOF_SOURCE));
        assert_eq!(proof.status.len(), snapshot.status.len());
        proof.store(&writer, now).unwrap();
        let loaded = loader::load(&store.read_only().unwrap(), request, &params).unwrap();
        assert!(
            loaded.inputs.snapshots.is_empty(),
            "proof days are not read"
        );
        assert_eq!(loaded.proof.ignored, 1);
    }

    /// A proof day is no history without the opt-in; with it the point
    /// names its source, and every document says proof days are in use.
    #[test]
    fn a_proof_day_is_history_only_with_the_opt_in_and_is_named() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
        let mut day = stored_level("2026-06-20", 2, 2.4);
        day.source = PROOF_SOURCE.into();
        let writer = store.writer().unwrap();
        writer.upsert_maturity_daily(&[day], 1).unwrap();
        drop(writer);
        let reader = store.read_only().unwrap();
        let mut inputs = firm_b();
        inputs.as_of_ns = ns("2027-01-15");
        let mut loaded = loaded(inputs);
        let params = HouseParams::default();
        let today = assess(&Matrix::build(&loaded.inputs, &params));

        let points = history(&reader, &loaded, &today).unwrap();
        assert_eq!(points[1]["quarter"], "2026-Q2");
        assert_eq!(points[1]["state"], "no_data");
        assert_eq!(points[1]["source"], Value::Null);
        let doc = maturity(&loaded, &today, None, points);
        assert_eq!(
            doc["proof_days"],
            json!({"enabled": false, "counted": 0, "ignored": 0, "note": null})
        );

        loaded.proof = loader::ProofDays {
            enabled: true,
            counted: 3,
            ignored: 0,
        };
        let points = history(&reader, &loaded, &today).unwrap();
        assert_eq!(points[1]["state"], "stored");
        assert_eq!(points[1]["source"], "proof");
        assert_eq!(points[1]["level"], 2);
        assert_eq!(points[4]["source"], Value::Null, "today is live");
        let doc = maturity(&loaded, &today, None, points);
        assert_eq!(doc["proof_days"]["enabled"], true);
        assert_eq!(doc["proof_days"]["counted"], 3);
        assert_eq!(doc["proof_days"]["note"], PROOF_DAYS_NOTE);
        let dora = dora(
            &loaded,
            &Matrix::build(&loaded.inputs, &params),
            DoraBy::Entity,
        );
        assert_eq!(dora["proof_days"]["note"], PROOF_DAYS_NOTE);
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
