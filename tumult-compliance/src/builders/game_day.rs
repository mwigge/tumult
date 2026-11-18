//! The R3 game-day report for one experiment run.

use std::collections::HashMap;

use serde_json::Value;
use tumult_lake::{scope, Reader};

use super::{base_meta, cell, counted, q, severity_cell};
use crate::html::fmt_datetime;
use crate::model::{Block, Cell, ReportDoc, TemplateKind};

/// The name of the root span of a run.
const RUN_SPAN: &str = "resilience.experiment";
/// The span a game day opens. Its experiments run under it, in its trace.
const GAME_DAY_SPAN: &str = "resilience.gameday";
/// The span of a probe. The runner samples the probes of a hypothesis in
/// turns, so only these spans fold into a line across the spans between
/// them.
const PROBE_SPAN: &str = "resilience.probe";
/// The name of the span of a step that acts on the target.
const ACTION_SPAN: &str = "resilience.action";
/// The span attribute that names the step a span ran.
const STEP_NAME: &str = "resilience.action.name";
/// The most spans of a run a report reads. A run samples each probe 300
/// times at most unless its experiment asks for more, and a script step
/// opens one more span each time it runs, so this is room for 33 such
/// probes.
pub(super) const MOST_SPANS: usize = 20_000;

/// The root span of the run `id`, when it is in `envs` (empty = unscoped).
/// `outcome` says how the run ended, by the rule of
/// [`tumult_lake::run_outcome_sql`], and is `incomplete` when the run
/// recorded no status.
fn root_of(reader: &Reader, id: &str, envs: &[String]) -> Result<Option<Value>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT s.ts_ns, s.experiment_id, s.experiment_name, s.target_system, \
             s.target_environment, s.fault_type, s.fault_subtype, s.fault_severity, \
             s.blast_radius, s.hypothesis_met, s.recovery_time_s, s.service_name, \
             s.duration_ns, s.span_attrs, coalesce({}, 'incomplete') AS outcome \
             FROM spans AS s LEFT JOIN run_completions AS l ON s.experiment_id = l.experiment_id \
             WHERE s.span_name = 'resilience.experiment' AND s.experiment_id = {id}{} \
             ORDER BY s.ts_ns DESC LIMIT 1",
            tumult_lake::run_outcome_sql("s", "l"),
            scope::and(scope::in_list("s.target_environment", envs)),
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().next())
}

/// The start of a query over the run `id`, for `OF_THE_RUN` to follow.
///
/// A game day runs all its experiments in one trace, each under the span
/// of the game day. There the run owns its tree (`of_run`): the span that
/// carries its id and every span under it. Any other run is alone in its
/// trace and owns the whole trace (`alone`), so a span that lost its
/// parent on the way to the store still counts. Its span has no parent, or
/// it is under a span that is no game day, as the span of the tool call
/// that started it.
///
/// A span is stored when it is over, so the span above the run is the
/// last of its trace to arrive. Until then the span of the run names a
/// parent that is not on record, and the run owns its tree. A run of a
/// game day reads the same before and after. A run that a tool call
/// started owns its tree until the span of the call is stored, and the
/// whole trace from then on.
fn run_tree(id: &str) -> String {
    format!(
        "WITH RECURSIVE in_trace AS ( \
             SELECT s.trace_id, s.span_id, s.parent_span_id, s.span_name, s.experiment_id \
             FROM spans AS s \
             WHERE s.trace_id IN ( \
                 SELECT named.trace_id FROM spans AS named WHERE named.experiment_id = {id} \
             ) \
         ), \
         alone AS ( \
             SELECT run_span.trace_id FROM in_trace AS run_span \
             LEFT JOIN in_trace AS above \
                ON run_span.trace_id = above.trace_id \
               AND run_span.parent_span_id = above.span_id \
             WHERE run_span.experiment_id = {id} \
               AND run_span.span_name = '{RUN_SPAN}' \
               AND ( \
                   run_span.parent_span_id IS NULL \
                   OR above.span_name <> '{GAME_DAY_SPAN}' \
               ) \
         ), \
         of_run (trace_id, span_id) AS ( \
             SELECT named.trace_id, named.span_id FROM in_trace AS named \
             WHERE named.experiment_id = {id} \
             UNION \
             SELECT below.trace_id, below.span_id FROM in_trace AS below \
             INNER JOIN of_run \
                ON below.trace_id = of_run.trace_id \
               AND below.parent_span_id = of_run.span_id \
         )"
    )
}

/// Whether a row with a `trace_id` and a `span_id` is of the run that
/// `run_tree` names.
const OF_THE_RUN: &str = "(trace_id IN (SELECT alone.trace_id FROM alone) \
     OR (trace_id, span_id) IN (SELECT of_run.trace_id, of_run.span_id FROM of_run))";

/// What a report read of the spans of a run.
struct Read {
    spans: Vec<Span>,
    /// Whether the run has more spans than the report read.
    cut: bool,
    /// The spans of the trace that are not of the run and name a parent
    /// that is not on record. They may be of this run: nothing says whose
    /// they are.
    without_parent: usize,
}

/// The spans of the run `id` in the order they started, `MOST_SPANS` of
/// them at most.
fn spans_of(reader: &Reader, id: &str) -> Result<Read, String> {
    let rows = reader
        .query_json_rows(&format!(
            "{} SELECT ts_ns, span_id, parent_span_id, span_name, duration_ns, status_code, \
             status_message, span_attrs \
             FROM spans WHERE {OF_THE_RUN} ORDER BY ts_ns LIMIT {}",
            run_tree(id),
            MOST_SPANS + 1,
        ))
        .map_err(|e| e.to_string())?;
    Ok(Read {
        spans: rows.iter().take(MOST_SPANS).map(Span::of).collect(),
        cut: rows.len() > MOST_SPANS,
        without_parent: spans_without_parent(reader, id)?,
    })
}

/// How many spans of the trace of the run `id` are not of the run and name
/// a parent that is not on record. A run alone in its trace has none: it
/// owns them all.
fn spans_without_parent(reader: &Reader, id: &str) -> Result<usize, String> {
    let rows = reader
        .query_json_rows(&format!(
            "{} SELECT count(*) AS spans FROM in_trace \
             WHERE parent_span_id IS NOT NULL \
               AND NOT EXISTS ( \
                   SELECT 1 FROM in_trace AS above \
                   WHERE above.trace_id = in_trace.trace_id \
                     AND above.span_id = in_trace.parent_span_id \
               ) \
               AND NOT {OF_THE_RUN}",
            run_tree(id),
        ))
        .map_err(|e| e.to_string())?;
    let spans = rows.first().and_then(|row| row.get("spans"));
    let spans = spans
        .and_then(Value::as_u64)
        .ok_or("the count of the spans that name a parent not on record is missing")?;
    usize::try_from(spans).map_err(|e| e.to_string())
}

/// The warnings and errors the run `id` logged, oldest first: the lines of
/// its spans, and the lines that carry its id.
fn logged_findings(reader: &Reader, id: &str) -> Result<Vec<Value>, String> {
    reader
        .query_json_rows(&format!(
            "{} SELECT ts_ns, severity_text, body FROM logs \
             WHERE (log_attrs['experiment_id'] = {id} OR {OF_THE_RUN}) \
               AND severity_text IN ('WARN', 'ERROR') \
             ORDER BY ts_ns LIMIT 100",
            run_tree(id),
        ))
        .map_err(|e| e.to_string())
}

/// The attributes of the started log of the run `id`.
fn started_attrs(reader: &Reader, id: &str) -> Result<Option<Value>, String> {
    let rows = reader
        .query_json_rows(&format!(
            "SELECT log_attrs FROM logs \
             WHERE log_attrs['experiment_id'] = {id} AND body = 'experiment.started' \
             ORDER BY ts_ns LIMIT 1"
        ))
        .map_err(|e| e.to_string())?;
    Ok(rows.first().and_then(|row| row.get("log_attrs").cloned()))
}

/// One span of the run, as the report reads it.
struct Span {
    at_ns: Option<i64>,
    id: Option<String>,
    parent: Option<String>,
    name: String,
    /// The step the span ran, when it is the span of a step.
    step: Option<String>,
    duration_ns: Option<f64>,
    /// `ok` or `error`. A span that set no status has none.
    status: Option<&'static str>,
    message: String,
}

impl Span {
    fn of(row: &Value) -> Self {
        let text = |key: &str| cell(row, key).map(str::to_owned);
        Self {
            at_ns: row.get("ts_ns").and_then(Value::as_i64),
            id: text("span_id"),
            parent: text("parent_span_id"),
            name: text("span_name").unwrap_or_else(|| "—".into()),
            step: row
                .get("span_attrs")
                .and_then(|attrs| attrs.get(STEP_NAME))
                .and_then(Value::as_str)
                .map(str::to_owned),
            duration_ns: row.get("duration_ns").and_then(Value::as_f64),
            status: match cell(row, "status_code") {
                Some("Ok") => Some("ok"),
                Some("Error") => Some("error"),
                _ => None,
            },
            message: text("status_message").unwrap_or_default(),
        }
    }

    fn failed(&self) -> bool {
        self.status == Some("error")
    }

    fn is_rollback(&self) -> bool {
        self.name.to_ascii_lowercase().contains("rollback")
    }

    /// Whether the span is a child of one of `parents`.
    fn is_under(&self, parents: &[&Span]) -> bool {
        self.parent
            .as_ref()
            .is_some_and(|parent| parents.iter().any(|span| span.id.as_ref() == Some(parent)))
    }

    /// Whether the span is a step that ended the way `earlier` did. `lines`
    /// asks it of two spans that ran the same step in the same place.
    fn ended_like(&self, earlier: &Span) -> bool {
        self.step.is_some() && self.status == earlier.status
    }

    /// Whether the span was opened inside a step: a span above it names
    /// one. `by_id` holds the spans of the run, and no chain of parents is
    /// longer than their number.
    fn is_inside_a_step(&self, by_id: &HashMap<&str, &Span>) -> bool {
        let above = |span: &&Span| -> Option<&Span> {
            let parent = span.parent.as_deref()?;
            by_id.get(parent).copied()
        };
        std::iter::successors(above(&self), above)
            .take(by_id.len())
            .any(|span| span.step.is_some())
    }

    /// The finding a failed span is: what failed, and what it said.
    fn failure(&self) -> String {
        let what = match &self.step {
            Some(step) => format!("{step} ({})", self.name),
            None => self.name.clone(),
        };
        if self.message.is_empty() {
            return format!("[ERROR] {what}");
        }
        format!("[ERROR] {what}: {}", self.message)
    }
}

/// The rollback of a run: the spans of the phase and the steps under them.
struct Rollback<'a> {
    phases: Vec<&'a Span>,
    steps: Vec<&'a Span>,
    /// Whether the report read the run to its end. A rollback is the last
    /// thing a run does, so a report that stopped earlier has not seen it.
    read_to_the_end: bool,
}

impl<'a> Rollback<'a> {
    fn of(read: &'a Read) -> Self {
        let phases: Vec<&Span> = read.spans.iter().filter(|s| s.is_rollback()).collect();
        let steps = read
            .spans
            .iter()
            .filter(|span| span.is_under(&phases))
            .collect();
        Self {
            phases,
            steps,
            read_to_the_end: !read.cut,
        }
    }

    fn ran(&self) -> bool {
        !self.phases.is_empty()
    }

    /// Whether the run exercised a rollback, in a word.
    fn exercised(&self) -> &'static str {
        if self.ran() {
            return "yes";
        }
        if self.read_to_the_end {
            return "no";
        }
        "not read"
    }

    /// What the rollback ran, and whether all of it ended clean.
    fn paragraph(&self) -> String {
        if !self.ran() && !self.read_to_the_end {
            return "This report did not read the run to its end, so it cannot say whether a \
                    rollback ran."
                .into();
        }
        if !self.ran() {
            return "No rollback action was exercised in this run.".into();
        }
        let names: Vec<&str> = self
            .steps
            .iter()
            .filter_map(|span| span.step.as_deref())
            .collect();
        let ran = if names.is_empty() {
            "The rollback ran".to_string()
        } else {
            format!(
                "The rollback ran {} ({})",
                counted(names.len(), "step", "steps"),
                names.join(", ")
            )
        };
        let failed = self.phases.iter().chain(&self.steps).any(|s| s.failed());
        let status = match (failed, self.read_to_the_end) {
            (true, _) => "errors present — investigate",
            (false, false) => "not read to its end",
            (false, true) => "all clean",
        };
        format!("{ran}; status: {status}.")
    }
}

/// What the run did to its target: the fault the root span names, or else
/// the steps of the method that acted on the target, each named once. A
/// step of the rollback undoes a fault and is not one.
fn scenario(root: &Value, spans: &[Span], rollback: &Rollback) -> String {
    match (cell(root, "fault_type"), cell(root, "fault_subtype")) {
        (Some(kind), Some(subtype)) => return format!("{kind} / {subtype}"),
        (Some(kind), None) => return kind.to_string(),
        _ => {}
    }
    let mut faults: Vec<&str> = Vec::new();
    let acted = spans
        .iter()
        .filter(|span| span.name == ACTION_SPAN && !span.is_under(&rollback.phases));
    for step in acted.filter_map(|span| span.step.as_deref()) {
        if !faults.contains(&step) {
            faults.push(step);
        }
    }
    if faults.is_empty() {
        return "—".into();
    }
    faults.join(", ")
}

/// The verdict on the hypothesis, as the root span has it.
fn hypothesis(root: &Value) -> &'static str {
    match root.get("hypothesis_met").and_then(Value::as_bool) {
        Some(true) => "Met",
        Some(false) => "Not met",
        None => "Not recorded",
    }
}

fn recovery_time_s(root: &Value) -> Option<f64> {
    root.get("recovery_time_s").and_then(Value::as_f64)
}

/// The key facts of the run.
fn summary(root: &Value, run_id: &str, scenario: String, outcome: &str) -> Block {
    let at = root.get("ts_ns").and_then(Value::as_i64).unwrap_or(0);
    let text = |key: &str| Cell::text(cell(root, key).unwrap_or("—"));
    let hypothesis = hypothesis(root);
    Block::KeyValues(vec![
        (
            "Experiment".into(),
            Cell::text(cell(root, "experiment_name").unwrap_or("unknown experiment")),
        ),
        ("Run id".into(), Cell::text(run_id)),
        ("Date".into(), Cell::text(fmt_datetime(at))),
        ("Target".into(), text("target_system")),
        ("Environment".into(), text("target_environment")),
        ("Scenario".into(), Cell::text(scenario)),
        (
            "Severity".into(),
            severity_cell(cell(root, "fault_severity")),
        ),
        (
            "Hypothesis".into(),
            if hypothesis == "Not recorded" {
                Cell::text(hypothesis)
            } else {
                Cell::status(hypothesis)
            },
        ),
        ("Outcome".into(), Cell::status(outcome)),
    ])
}

/// How far the fault reached, and how the run made the target safe again.
fn safety(root: &Value, rollback: &Rollback) -> Block {
    Block::KeyValues(vec![
        (
            "Blast radius".into(),
            Cell::text(cell(root, "blast_radius").unwrap_or("not recorded")),
        ),
        (
            "Recovery time".into(),
            Cell::text(recovery_time_s(root).map_or("not recorded".into(), |r| format!("{r:.1}s"))),
        ),
        (
            "Rollback exercised".into(),
            Cell::text(rollback.exercised()),
        ),
    ])
}

/// The spans the timeline shows: the run, its phases and its steps. What
/// the plugin of a step opened inside it is left out, so a step is one span
/// however its plugin ran it.
fn phases_and_steps(spans: &[Span]) -> Vec<&Span> {
    let by_id: HashMap<&str, &Span> = spans
        .iter()
        .filter_map(|span| Some((span.id.as_deref()?, span)))
        .collect();
    spans
        .iter()
        .filter(|span| !span.is_inside_a_step(&by_id))
        .collect()
}

/// The lines under one parent that a span can still join.
struct Turns<'a> {
    /// The name of the newest span under the parent.
    kind: &'a str,
    /// The newest line of each step, since a span of another name came.
    newest: HashMap<Option<&'a str>, usize>,
}

/// The spans in lines of the timeline: a step that ran several times in a
/// row with the same ending is one line. A probe is one line also when it
/// took turns with other probes under the same parent: two probes that
/// were sampled thirty times in turns are two lines and not sixty. A span
/// of another name under that parent ends the turns, so a probe before an
/// action and the same probe after it are two lines. A step that ended
/// another way than the time before starts a new line. An action that ran
/// again after another action is a new line, so the actions read in the
/// order they ran.
fn lines<'a>(spans: &[&'a Span]) -> Vec<Vec<&'a Span>> {
    let mut lines: Vec<Vec<&Span>> = Vec::new();
    let mut open: HashMap<Option<&str>, Turns> = HashMap::new();
    for &span in spans {
        let turns = open.entry(span.parent.as_deref()).or_insert(Turns {
            kind: &span.name,
            newest: HashMap::new(),
        });
        if turns.kind != span.name {
            turns.kind = &span.name;
            turns.newest.clear();
        }
        let known = turns.newest.get(&span.step.as_deref()).copied();
        let in_reach = |line: &usize| span.name == PROBE_SPAN || line + 1 == lines.len();
        if let Some(line) = known
            .filter(in_reach)
            .filter(|&line| span.ended_like(lines[line][0]))
        {
            lines[line].push(span);
            continue;
        }
        turns.newest.insert(span.step.as_deref(), lines.len());
        lines.push(vec![span]);
    }
    lines
}

/// One value to a decimal, or the lowest and the highest of several.
fn range(values: impl Iterator<Item = f64>) -> String {
    let mut shown: Vec<f64> = values.collect();
    shown.sort_by(f64::total_cmp);
    let (Some(low), Some(high)) = (shown.first(), shown.last()) else {
        return "—".into();
    };
    let (low, high) = (format!("{low:.1}"), format!("{high:.1}"));
    if low == high {
        return low;
    }
    format!("{low} – {high}")
}

/// The line of the timeline for `line`, the spans of one step in a row.
/// `start_ns` is the time the first span of the run started.
fn timeline_row(line: &[&Span], start_ns: i64) -> Vec<Cell> {
    let span = line[0];
    let step = match (&span.step, line.len()) {
        (None, _) => "—".to_string(),
        (Some(step), 1) => step.clone(),
        (Some(step), times) => format!("{step} × {times}"),
    };
    let started = line
        .iter()
        .filter_map(|span| span.at_ns)
        .map(|at| (at - start_ns) as f64 / 1e9);
    let took = line
        .iter()
        .filter_map(|span| span.duration_ns)
        .map(|ns| ns / 1e6);
    vec![
        Cell::text(range(started)),
        Cell::text(span.name.clone()),
        Cell::text(step),
        Cell::text(range(took)),
        // OTel codes → readable status: Unset is "no status".
        span.status.map_or(Cell::text("—"), Cell::status),
    ]
}

/// What the run did, in the order it did it.
fn timeline(spans: &[Span], root_at_ns: i64) -> Block {
    let start_ns = spans
        .first()
        .and_then(|span| span.at_ns)
        .unwrap_or(root_at_ns);
    Block::Table {
        headers: vec![
            "t+ (s)".into(),
            "Span".into(),
            "Step".into(),
            "Duration (ms)".into(),
            "Status".into(),
        ],
        rows: lines(&phases_and_steps(spans))
            .iter()
            .map(|line| timeline_row(line, start_ns))
            .collect(),
        numeric_cols: vec![0, 3],
        widths: Some(vec![1.0, 2.0, 1.7, 1.1, 0.8]),
    }
}

fn verdict(root: &Value, outcome: &str) -> String {
    format!(
        "Outcome: {outcome}. Steady-state hypothesis: {}.{}",
        hypothesis(root).to_lowercase(),
        recovery_time_s(root).map_or(String::new(), |r| format!(" Service recovered in {r:.1}s.")),
    )
}

/// What a report says of the spans that name a parent that is not on
/// record, when it left them out.
fn left_out(spans: usize) -> Option<String> {
    let said = match spans {
        0 => return None,
        1 => "1 span of this trace names a parent that is not on record. It and the spans \
              under it are"
            .to_string(),
        _ => format!(
            "{spans} spans of this trace name a parent that is not on record. They and the \
             spans under them are"
        ),
    };
    Some(format!("[WARN] {said} not in this report."))
}

/// What a report says first when it did not read the whole run.
fn cut_short() -> String {
    format!(
        "[WARN] The run has more than {MOST_SPANS} spans. This report reads the first \
         {MOST_SPANS}, so it may miss a step that failed later."
    )
}

/// What went wrong in the run: the steps that failed, each said once with
/// the number of times it failed, then the warnings and errors the run
/// logged. The root span is left out: how the run ended is the verdict.
fn findings(read: &Read, logs: &[Value]) -> Vec<String> {
    let mut failures: Vec<(String, usize)> = Vec::new();
    let failed = |span: &&Span| span.failed() && span.name != RUN_SPAN;
    for span in read.spans.iter().filter(failed) {
        let failure = span.failure();
        match failures.iter_mut().find(|(known, _)| *known == failure) {
            Some((_, times)) => *times += 1,
            None => failures.push((failure, 1)),
        }
    }
    let failed = failures.into_iter().map(|(failure, times)| {
        if times == 1 {
            return failure;
        }
        format!("{failure} (× {times})")
    });
    let logged = logs.iter().filter_map(|log| {
        Some(format!(
            "[{}] {}",
            cell(log, "severity_text")?,
            cell(log, "body")?
        ))
    });
    let cut = read.cut.then(cut_short);
    let said_first = cut.into_iter().chain(left_out(read.without_parent));
    said_first.chain(failed).chain(logged).collect()
}

/// A value of the appendix as a reader takes it: a time is a date and a
/// time of day, not a count of nanoseconds.
fn shown(key: &str, value: &str) -> String {
    if !key.ends_with("_at_ns") {
        return value.to_string();
    }
    value
        .parse::<i64>()
        .map_or_else(|_| value.to_string(), fmt_datetime)
}

/// The attributes of the root span and, as `start.<key>`, of the started
/// log: the first forty by name.
fn appendix(root: &Value, started: Option<&Value>) -> Block {
    let attrs_of = |attrs: Option<&Value>, prefix: &str| -> Vec<(String, Cell)> {
        let Some(Value::Object(attrs)) = attrs else {
            return Vec::new();
        };
        attrs
            .iter()
            .map(|(key, value)| {
                let value = shown(key, value.as_str().unwrap_or(""));
                (format!("{prefix}{key}"), Cell::text(value))
            })
            .collect()
    };
    let mut config = attrs_of(root.get("span_attrs"), "");
    config.extend(attrs_of(started, "start."));
    if config.is_empty() {
        return Block::Paragraph("No configuration attributes recorded.".into());
    }
    config.sort_by(|a, b| a.0.cmp(&b.0));
    config.truncate(40);
    Block::KeyValues(config)
}

/// Build the R3 game-day report for one experiment run (tumult
/// `experiment_id`). Returns `Ok(None)` when the id is unknown — or when
/// `envs` is scoped and the run's root span sits outside those environments
/// (no existence leak across scopes). The report reads the spans and logs
/// of the run that owns the root span, so confining the root span confines
/// the report.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn build_game_day(
    reader: &Reader,
    experiment_id: &str,
    generated_at_ns: i64,
    envs: &[String],
) -> Result<Option<ReportDoc>, String> {
    let id = q(experiment_id);
    let Some(root) = root_of(reader, &id, envs)? else {
        return Ok(None);
    };
    let outcome = cell(&root, "outcome").unwrap_or_default();
    let read = spans_of(reader, &id)?;
    let spans = &read.spans;
    let rollback = Rollback::of(&read);
    let findings = findings(&read, &logged_findings(reader, &id)?);
    let started = started_attrs(reader, &id)?;

    let name = cell(&root, "experiment_name").unwrap_or("unknown experiment");
    let root_at_ns = root.get("ts_ns").and_then(Value::as_i64).unwrap_or(0);
    let mut meta = base_meta(
        TemplateKind::GameDay,
        format!("Game-day report — {name}"),
        generated_at_ns,
        root_at_ns,
        &format!("R3|{experiment_id}|{generated_at_ns}"),
    );
    meta.experiment_id = Some(experiment_id.to_string());

    let blocks = vec![
        Block::H1("Run summary".into()),
        summary(
            &root,
            experiment_id,
            scenario(&root, spans, &rollback),
            outcome,
        ),
        Block::H2("Blast radius and safety".into()),
        safety(&root, &rollback),
        Block::H2("Timeline".into()),
        timeline(spans, root_at_ns),
        Block::H2("Verdict".into()),
        Block::Paragraph(verdict(&root, outcome)),
        Block::H2("Findings".into()),
        if findings.is_empty() {
            Block::Paragraph(
                "No step failed and no warnings or errors were logged during this run.".into(),
            )
        } else {
            Block::Bullets(findings)
        },
        Block::H2("Rollback".into()),
        Block::Paragraph(rollback.paragraph()),
        Block::PageBreak,
        Block::H2("Configuration appendix".into()),
        appendix(&root, started.as_ref()),
        Block::Footnote(format!(
            "Generated by Tumult v{} from the run's trace and logs.",
            env!("CARGO_PKG_VERSION")
        )),
        Block::Signoff(vec![
            ("Run operator".into(), String::new()),
            ("Reviewed by".into(), String::new()),
        ]),
    ];

    Ok(Some(ReportDoc { meta, blocks }))
}
