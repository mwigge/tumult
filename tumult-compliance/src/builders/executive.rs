//! The R1 executive resilience digest.

use tumult_lake::Reader;

use super::{base_meta, bluf, decision_for, f1, issue_stats, outlook, runs_in_window, DAY_NS};
use crate::html::fmt_date;
use crate::model::{Block, Cell, ChartSpec, ReportDoc, TemplateKind};
use crate::org::{OrgTree, ScoredLeaf};
use crate::scoring::{self, RunState};

/// Build the R1 executive resilience digest as of `as_of_ns`, comparing
/// against `period_ns` earlier. `org` drives the "By domain" rollup section
/// (an empty tree collapses everything into `(unassigned)`). `envs` confines
/// every aggregate to the given environments (empty = unscoped); a scoped
/// principal gets a digest of its own environments only.
///
/// # Errors
/// Returns the store error string when a query fails.
pub fn build_executive(
    reader: &Reader,
    org: &OrgTree,
    as_of_ns: i64,
    period_ns: i64,
    generated_at_ns: i64,
    envs: &[String],
) -> Result<ReportDoc, String> {
    let card = scoring::compute_scoped(reader, as_of_ns, Some(period_ns), envs)?;
    let from_ns = as_of_ns - period_ns;

    let (discovered, fixed, mttr_s) = issue_stats(reader, from_ns, as_of_ns, envs)?;
    let open: Vec<&scoring::ExperimentScore> = card
        .experiments
        .iter()
        .filter(|e| e.state != RunState::Passed && e.state != RunState::Stale)
        .collect();
    let tested = card
        .experiments
        .iter()
        .filter(|e| e.state != RunState::NeverRun)
        .count();

    // The scope set is part of the document identity: a scoped digest is a
    // different document from the global one for the same instant.
    let seed = format!(
        "R1|{}|{from_ns}|{as_of_ns}|{generated_at_ns}",
        envs.join(",")
    );
    let mut meta = base_meta(
        TemplateKind::ExecutiveDigest,
        "Executive resilience digest".into(),
        generated_at_ns,
        as_of_ns,
        &seed,
    );
    meta.period = Some((from_ns, as_of_ns));

    let mut blocks = vec![
        Block::H1("Bottom line".into()),
        Block::Paragraph(bluf(&card, &open)),
    ];

    blocks.push(Block::Kpis(vec![
        (
            "Portfolio score".into(),
            f1(card.portfolio),
            Some(card.band.clone()),
        ),
        (
            "Δ vs previous period".into(),
            card.delta.map_or("—".into(), |d| format!("{d:+.1}")),
            None,
        ),
        (
            "Experiments tested".into(),
            format!("{tested} / {}", card.experiments.len()),
            None,
        ),
        ("Open weaknesses".into(), open.len().to_string(), None),
        (
            "Issues fixed".into(),
            format!("{fixed} / {discovered}"),
            mttr_s.map(|m| format!("MTTR {m:.1}s")),
        ),
    ]));

    // Score trend across the period (portfolio sampled at 10 instants).
    let trend = scoring::portfolio_series_scoped(reader, as_of_ns, period_ns, 10, envs)?;
    blocks.push(Block::H2("Score trend".into()));
    blocks.push(Block::Chart(ChartSpec::Lines(vec![(
        "portfolio".into(),
        trend,
    )])));

    // Per-experiment bars, weakest first (experiments are sorted by score).
    blocks.push(Block::H2("Experiment scores".into()));
    blocks.push(Block::Chart(ChartSpec::Bars(
        card.experiments
            .iter()
            .map(|e| (e.name.clone(), f64::from(e.score)))
            .collect(),
    )));

    // By domain: top-level org rollup (criticality-weighted mean over all
    // leaves in each subtree; pending manual records count toward coverage).
    let mut leaves: Vec<ScoredLeaf> = card
        .experiments
        .iter()
        .map(|e| ScoredLeaf {
            name: e.name.clone(),
            score: Some(e.score),
        })
        .collect();
    leaves.extend(
        scoring::pending_manual_leaves_scoped(reader, envs)?
            .into_iter()
            .map(|name| ScoredLeaf { name, score: None }),
    );
    blocks.push(Block::H2("By domain".into()));
    if let Some(root) = org.compute_node("", &leaves) {
        if root.children.is_empty() {
            blocks.push(Block::Paragraph("No org structure on record.".into()));
        } else {
            blocks.push(Block::Table {
                headers: vec![
                    "Domain".into(),
                    "Score".into(),
                    "Band".into(),
                    "Coverage".into(),
                    "Weakest member".into(),
                ],
                rows: root
                    .children
                    .iter()
                    .map(|c| {
                        vec![
                            Cell::text(c.name.clone()),
                            Cell::text(f1(c.score)),
                            Cell::status(c.band.clone()),
                            Cell::text(format!("{}/{}", c.scored, c.expected)),
                            Cell::text(c.weakest.clone().unwrap_or("—".into())),
                        ]
                    })
                    .collect(),
                numeric_cols: vec![1],
                widths: Some(vec![2.6, 0.8, 1.0, 1.0, 2.2]),
            });
        }
    }

    // Coverage: experiments with ≥ 1 run inside the window vs not.
    let in_window = runs_in_window(reader, from_ns, as_of_ns, envs)?;
    let tested_in_window = card
        .experiments
        .iter()
        .filter(|e| in_window.contains(&e.name))
        .count() as f64;
    let not_run = card.experiments.len() as f64 - tested_in_window;
    blocks.push(Block::H2("Coverage".into()));
    if card.experiments.is_empty() {
        blocks.push(Block::Paragraph("No experiments on record.".into()));
    } else {
        blocks.push(Block::Chart(ChartSpec::Donut(vec![
            ("Tested in window".into(), tested_in_window),
            ("Not run in window".into(), not_run),
        ])));
    }

    blocks.push(Block::H2("Target scores".into()));
    blocks.push(Block::Table {
        headers: vec![
            "Target".into(),
            "Experiments".into(),
            "Runs".into(),
            "Score".into(),
            "Band".into(),
        ],
        rows: card
            .targets
            .iter()
            .map(|t| {
                vec![
                    Cell::text(t.target.clone()),
                    Cell::text(
                        card.experiments
                            .iter()
                            .filter(|e| e.target.as_deref().unwrap_or("(untargeted)") == t.target)
                            .count()
                            .to_string(),
                    ),
                    Cell::text(t.runs.to_string()),
                    Cell::text(f1(t.score)),
                    Cell::status(t.band.clone()),
                ]
            })
            .collect(),
        numeric_cols: vec![1, 2, 3],
        widths: Some(vec![3.0, 1.1, 0.8, 0.8, 1.0]),
    });

    blocks.push(Block::H2("Issues discovered and fixed".into()));
    blocks.push(Block::Paragraph(format!(
        "In the period {} – {}, {} experiment runs deviated or failed; {} were later \
         fixed (a subsequent completed run of the same experiment — an intentionally \
         simple, auditable heuristic).{}",
        fmt_date(from_ns),
        fmt_date(as_of_ns),
        discovered,
        fixed,
        mttr_s.map_or(String::new(), |m| format!(
            " Mean time to recovery across runs reporting one: {m:.1}s."
        )),
    )));

    blocks.push(Block::H2("Open weaknesses and decisions needed".into()));
    if open.is_empty() {
        blocks.push(Block::Paragraph(
            "No open weaknesses: every known experiment last ran green.".into(),
        ));
    } else {
        blocks.push(Block::Table {
            headers: vec![
                "Severity".into(),
                "Experiment".into(),
                "Last run".into(),
                "Age (d)".into(),
                "Decision needed".into(),
            ],
            rows: open
                .iter()
                .map(|e| {
                    vec![
                        Cell::status(e.severity.clone().unwrap_or_else(|| "medium".into())),
                        Cell::text(e.name.clone()),
                        Cell::text(e.last_run_ns.map_or("never".into(), fmt_date)),
                        Cell::text(
                            e.last_run_ns
                                .map_or("—".into(), |ts| ((as_of_ns - ts) / DAY_NS).to_string()),
                        ),
                        Cell::text(decision_for(e.state)),
                    ]
                })
                .collect(),
            numeric_cols: vec![3],
            widths: Some(vec![1.2, 3.0, 1.0, 0.7, 2.0]),
        });
    }

    blocks.push(Block::H2("Outlook".into()));
    blocks.push(Block::Paragraph(outlook(&card)));

    let n_manual = card
        .experiments
        .iter()
        .filter(|e| e.origin == "manual")
        .count();
    let n_automated = card.experiments.len() - n_manual;
    blocks.push(Block::Footnote(format!(
        "Scores: passed 100, stale pass (>30d) 75, failed 50, never run 0; bands \
         >70 good, 50–70 fair, <50 poor. Evidence mix: {n_automated} automated, \
         {n_manual} verified manual experiment(s) (manual partials score 75; \
         inconclusive manual outcomes are excluded). Generated by Tumult v{}; \
         data as of {}.",
        env!("CARGO_PKG_VERSION"),
        fmt_date(as_of_ns)
    )));
    blocks.push(Block::Signoff(vec![
        ("Prepared by".into(), "Tumult".into()),
        ("Approved by".into(), String::new()),
    ]));

    Ok(ReportDoc { meta, blocks })
}
