//! Rendering: the tab strip, header/status bar, per-tab bodies, overlays, and
//! the keybind footer. This layer is a projection over [`crate::app::App`] and
//! the verified helpers in [`crate::logic`]; it holds no state of its own.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Sparkline, Table, Tabs,
    Wrap,
};
use ratatui::Frame;

use crate::app::{App, Tab, View};
use crate::logic::{self, StatusKind};
use crate::theme;

/// Draw the whole frame.
pub fn render(app: &mut App, f: &mut Frame) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab strip
            Constraint::Length(4), // header
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    render_tabs(app, f, chunks[0]);
    render_header(app, f, chunks[1]);
    render_body(app, f, chunks[2]);
    render_footer(f, chunks[3]);

    if app.show_help {
        render_help(f, area);
    }
}

fn render_tabs(app: &App, f: &mut Frame, area: Rect) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| Line::from(format!(" {}:{} ", i + 1, t.title())))
        .collect();
    let selected = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::default().fg(theme::MUTED))
        .highlight_style(
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .divider("");
    f.render_widget(tabs, area);
}

fn render_header(app: &App, f: &mut Frame, area: Rect) {
    let live_label = if app.live { "● LIVE" } else { "‖ PAUSED" };
    let line1 = Line::from(vec![
        Span::styled("Store ", Style::default().fg(theme::MUTED)),
        Span::raw(app.store_path.display().to_string()),
        Span::styled("  schema v", Style::default().fg(theme::MUTED)),
        Span::raw(app.schema_version.to_string()),
    ]);
    let line2 = Line::from(vec![
        Span::styled(
            format!("{} experiments  ", app.experiment_count),
            Style::default().fg(theme::ACCENT),
        ),
        Span::raw(format!("{} activities  ", app.activity_count)),
        Span::styled(
            format!(
                "success {:.0}%  ",
                logic::success_rate(&app.experiments) * 100.0
            ),
            Style::default().fg(theme::status_color(StatusKind::Pass)),
        ),
    ]);
    let filter_desc = format!(
        "status:{}  title:{}",
        if app.status_filter.is_empty() {
            "*"
        } else {
            &app.status_filter
        },
        if app.title_filter.is_empty() {
            "*"
        } else {
            &app.title_filter
        },
    );
    let line3 = Line::from(vec![
        Span::styled(live_label, theme::live_style(app.live)),
        Span::raw(format!("  refreshed {}s ago  ", app.refresh_age_secs())),
        Span::styled(
            format!(
                "sort:{} {}  ",
                app.sort_key.label(),
                if app.sort_asc { "▲" } else { "▼" }
            ),
            Style::default().fg(theme::MUTED),
        ),
        Span::styled(filter_desc, Style::default().fg(theme::MUTED)),
    ]);
    let mut content = vec![line1, line2, line3];
    if let Some(err) = &app.error {
        content.push(Line::from(Span::styled(
            format!("! {err}"),
            Style::default().fg(theme::status_color(StatusKind::Failed)),
        )));
    }
    let para = Paragraph::new(content).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(para, area);
}

fn render_body(app: &mut App, f: &mut Frame, area: Rect) {
    match app.tab {
        Tab::Experiments => match app.view {
            View::List => render_experiments(app, f, area),
            View::Detail => render_detail(app, f, area),
            View::Compare => render_compare(app, f, area),
        },
        Tab::Analytics => render_analytics(app, f, area),
        Tab::ChaosGraph | Tab::Compliance => render_graph(app, f, area),
    }
}

fn render_experiments(app: &mut App, f: &mut Frame, area: Rect) {
    let header = Row::new([
        Cell::from("Time"),
        Cell::from("Title"),
        Cell::from("Status"),
        Cell::from("Duration"),
        Cell::from("Resil"),
        Cell::from("Steps"),
        Cell::from("Dev"),
        Cell::from(""),
    ])
    .style(
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .filtered
        .iter()
        .map(|i| {
            let e = &app.experiments[*i];
            let kind = StatusKind::classify(&e.status);
            let mark = if app.marks.contains(&e.id) {
                "◉"
            } else {
                " "
            };
            let mut row = Row::new(vec![
                Cell::from(logic::format_time(e.started_at_ns)),
                Cell::from(e.title.clone()),
                Cell::from(Span::styled(kind.label(), theme::status_style(kind))),
                Cell::from(logic::format_duration(e.duration_ms)),
                Cell::from(logic::format_resilience(e.resilience)),
                Cell::from(e.steps.to_string()),
                Cell::from(e.deviations.to_string()),
                Cell::from(Span::styled(mark, Style::default().fg(theme::ACCENT))),
            ]);
            if app.new_ids.contains(&e.id) {
                row = row.style(Style::default().bg(theme::NEW_ROW_BG));
            }
            row
        })
        .collect();

    let widths = [
        Constraint::Length(14),
        Constraint::Min(20),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Length(4),
        Constraint::Length(2),
    ];
    let title = format!(
        " History — {} of {} shown {}",
        app.filtered.len(),
        app.experiments.len(),
        if app.marks.is_empty() {
            String::new()
        } else {
            format!("• {} marked (c to compare) ", app.marks.len())
        }
    );
    if app.filtered.is_empty() {
        let msg = if app.experiments.is_empty() {
            "No experiments in the store yet. Run `tumult run <experiment>` to populate it."
        } else {
            "No experiments match the current filter (press Esc to clear)."
        };
        let para = Paragraph::new(msg)
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(theme::MUTED))
            .wrap(Wrap { trim: true });
        f.render_widget(para, area);
    } else {
        let table = Table::new(rows, widths)
            .header(header)
            .block(Block::default().borders(Borders::ALL).title(title))
            .row_highlight_style(theme::selection_style())
            .highlight_symbol("▶ ");
        f.render_stateful_widget(table, area, &mut app.table_state);
    }
}

fn render_detail(app: &App, f: &mut Frame, area: Rect) {
    let Some(exp) = app.selected_experiment() else {
        return;
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(0)])
        .split(area);

    let kind = StatusKind::classify(&exp.status);
    let meta = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Title  ", Style::default().fg(theme::MUTED)),
            Span::styled(
                exp.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Status ", Style::default().fg(theme::MUTED)),
            Span::styled(kind.label(), theme::status_style(kind)),
            Span::raw(format!(
                "   duration {}   resilience {}   steps {}   deviations {}",
                logic::format_duration(exp.duration_ms),
                logic::format_resilience(exp.resilience),
                exp.steps,
                exp.deviations,
            )),
        ]),
        Line::from(vec![
            Span::styled("Started ", Style::default().fg(theme::MUTED)),
            Span::raw(logic::format_time(exp.started_at_ns)),
            Span::styled("   id ", Style::default().fg(theme::MUTED)),
            Span::raw(exp.id.clone()),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Experiment "));
    f.render_widget(meta, chunks[0]);

    // Activity waterfall: indented duration micro-bars against the slowest step.
    let max_dur = app
        .detail_activities
        .iter()
        .map(|a| a.duration_ms)
        .max()
        .unwrap_or(1)
        .max(1);
    let bar_width = 16usize;
    let items: Vec<ListItem> = if app.detail_activities.is_empty() {
        vec![ListItem::new(Span::styled(
            "No activity timeline recorded for this experiment.",
            Style::default().fg(theme::MUTED),
        ))]
    } else {
        app.detail_activities
            .iter()
            .map(|a| {
                let kind = StatusKind::classify(&a.status);
                let bar = logic::microbar(a.duration_ms as f64, max_dur as f64, bar_width);
                let phase = if a.phase.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", a.phase)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{bar} "),
                        Style::default().fg(theme::status_color(kind)),
                    ),
                    Span::raw(format!("{:>8}  ", logic::format_duration(a.duration_ms))),
                    Span::styled(
                        format!("{} ", a.name),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("({}){}", a.activity_type, phase),
                        Style::default().fg(theme::MUTED),
                    ),
                ]))
            })
            .collect()
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Timeline (duration waterfall) — q/⌫ back "),
    );
    f.render_widget(list, chunks[1]);
}

fn render_compare(app: &App, f: &mut Frame, area: Rect) {
    let marked = app.marked_experiments();
    let header = Row::new([
        Cell::from("Time"),
        Cell::from("Title"),
        Cell::from("Status"),
        Cell::from("Duration"),
        Cell::from("Resil"),
    ])
    .style(
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    );
    let rows: Vec<Row> = marked
        .iter()
        .map(|e| {
            let kind = StatusKind::classify(&e.status);
            Row::new(vec![
                Cell::from(logic::format_time(e.started_at_ns)),
                Cell::from(e.title.clone()),
                Cell::from(Span::styled(kind.label(), theme::status_style(kind))),
                Cell::from(logic::format_duration(e.duration_ms)),
                Cell::from(logic::format_resilience(e.resilience)),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(14),
        Constraint::Min(20),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(6),
    ];
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(4)])
        .split(area);
    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Compare {} runs — q/⌫ back ", marked.len())),
    );
    f.render_widget(table, chunks[0]);

    // Duration trend across the compared runs (chronological).
    let mut chrono = marked.clone();
    chrono.sort_by_key(|e| e.started_at_ns);
    let durations: Vec<u64> = chrono.iter().map(|e| e.duration_ms).collect();
    let spark = Sparkline::default()
        .data(&durations)
        .style(Style::default().fg(theme::ACCENT))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Duration trend "),
        );
    f.render_widget(spark, chunks[1]);
}

fn render_analytics(app: &App, f: &mut Frame, area: Rect) {
    // Chronological (oldest→newest) copy for trend series.
    let mut chrono = app.experiments.clone();
    chrono.sort_by_key(|e| e.started_at_ns);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // success gauge
            Constraint::Length(8), // breakdown + trends
            Constraint::Min(0),    // duration sparkline
        ])
        .split(area);

    let rate = logic::success_rate(&app.experiments);
    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Overall success rate "),
        )
        .gauge_style(Style::default().fg(theme::status_color(StatusKind::Pass)))
        .ratio(rate)
        .label(format!("{:.1}%", rate * 100.0));
    f.render_widget(gauge, chunks[0]);

    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    // Status breakdown with micro-bars.
    let breakdown = logic::status_breakdown(&app.experiments);
    let max_count = breakdown.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    let bd_items: Vec<ListItem> = breakdown
        .iter()
        .map(|(kind, n)| {
            let bar = logic::microbar(*n as f64, max_count as f64, 12);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<9}", kind.label()), theme::status_style(*kind)),
                Span::styled(
                    format!("{bar} "),
                    Style::default().fg(theme::status_color(*kind)),
                ),
                Span::raw(n.to_string()),
            ]))
        })
        .collect();
    let bd_list = List::new(bd_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Status breakdown "),
    );
    f.render_widget(bd_list, mid[0]);

    // Success + resilience block sparklines (chronological).
    let succ = logic::sparkline(&logic::success_series(&chrono), 40);
    let resil = logic::sparkline(&logic::resilience_series(&chrono), 40);
    let trend = Paragraph::new(vec![
        Line::from(Span::styled(
            "Success (oldest → newest)",
            Style::default().fg(theme::MUTED),
        )),
        Line::from(Span::styled(
            succ,
            Style::default().fg(theme::status_color(StatusKind::Pass)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Resilience score",
            Style::default().fg(theme::MUTED),
        )),
        Line::from(Span::styled(resil, Style::default().fg(theme::ACCENT))),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Trends "));
    f.render_widget(trend, mid[1]);

    // Full-width duration sparkline via the ratatui widget.
    let durations: Vec<u64> = logic::duration_series(&chrono)
        .iter()
        .map(|d| *d as u64)
        .collect();
    let spark = Sparkline::default()
        .data(&durations)
        .style(Style::default().fg(theme::ACCENT))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Duration over the run sequence (ms) "),
        );
    f.render_widget(spark, chunks[2]);
}

fn render_graph(app: &mut App, f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let kind = app.graph_kind();
    let rows: Vec<Row> = app
        .graph_nodes
        .iter()
        .map(|n| Row::new(vec![Cell::from(n.label.clone()), Cell::from(n.id.clone())]))
        .collect();
    let widths = [Constraint::Min(20), Constraint::Min(20)];
    let title = format!(
        " {} nodes: {}  ({}) — s/→ next kind, h/← prev ",
        if app.tab == Tab::Compliance {
            "Compliance"
        } else {
            "ChaosGraph"
        },
        kind,
        app.graph_nodes.len(),
    );
    let table = Table::new(rows, widths)
        .header(
            Row::new([Cell::from("Label"), Cell::from("Id")]).style(
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(theme::selection_style())
        .highlight_symbol("▶ ");
    f.render_stateful_widget(table, chunks[0], &mut app.graph_state);

    let neigh_items: Vec<ListItem> = if app.graph_neighbors.is_empty() {
        vec![ListItem::new(Span::styled(
            "No edges for this node (or empty for this kind).",
            Style::default().fg(theme::MUTED),
        ))]
    } else {
        app.graph_neighbors
            .iter()
            .map(|e| ListItem::new(Span::raw(e.clone())))
            .collect()
    };
    let neigh = List::new(neigh_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Neighbours (edges, depth 1) "),
    );
    f.render_widget(neigh, chunks[1]);
}

fn render_footer(f: &mut Frame, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            "q",
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " quit  ↑↓/jk move  ⏎ detail  s sort  / filter  f status  space mark  c compare  l live  r refresh  Tab/1-4 tabs  ? help",
            Style::default().fg(theme::MUTED),
        ),
    ]));
    f.render_widget(footer, area);
}

fn render_help(f: &mut Frame, area: Rect) {
    let popup = centered_rect(64, 60, area);
    f.render_widget(Clear, popup);
    let lines = vec![
        Line::from(Span::styled(
            "Tumult TUI — keybindings",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  1-4 / Tab / Shift-Tab   switch tabs"),
        Line::from("  ↑ ↓ / j k               move selection"),
        Line::from("  Enter                   drill into experiment detail"),
        Line::from("  q / Backspace           back out of detail/compare"),
        Line::from("  s                        cycle sort key (Experiments)"),
        Line::from("  S                        toggle sort ascending/descending"),
        Line::from("  /                        filter by title substring"),
        Line::from("  f                        cycle status filter"),
        Line::from("  Space                   mark run for comparison"),
        Line::from("  c                        compare marked runs"),
        Line::from("  s / → , h / ←            cycle node kind (ChaosGraph)"),
        Line::from("  l                        toggle live / paused"),
        Line::from("  r                        refresh now"),
        Line::from("  ?                        toggle this help"),
        Line::from("  q                        quit"),
        Line::from(""),
        Line::from(Span::styled(
            "Reads the store read-only — safe alongside a running MCP server.",
            Style::default().fg(theme::MUTED),
        )),
    ];
    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Help "))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });
    f.render_widget(para, popup);
}

/// A centered rectangle `percent_x`×`percent_y` of `area`.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
