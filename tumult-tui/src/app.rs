//! Application state machine: tabs, history table selection, sort/filter,
//! drill-in, run comparison, the live/paused refresh, and keyboard handling.
//!
//! The store is never held open across ticks — [`App`] keeps only the store
//! *path* and re-takes a read-only snapshot on refresh, so the UI stays a live
//! reader alongside a running MCP server or ingest.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::data;
use crate::logic::{self, SortKey};
use crate::model::{ActivityRow, ExperimentRow, GraphNodeRow};

/// Top-level tabs, switched with the number keys or Tab/BackTab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Experiments,
    Analytics,
    ChaosGraph,
    Compliance,
}

impl Tab {
    pub const ALL: [Tab; 4] = [
        Tab::Experiments,
        Tab::Analytics,
        Tab::ChaosGraph,
        Tab::Compliance,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Experiments => "Experiments",
            Tab::Analytics => "Analytics",
            Tab::ChaosGraph => "ChaosGraph",
            Tab::Compliance => "Compliance",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// What the Experiments tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Detail,
    Compare,
}

/// Text-input focus for the `/` title filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    None,
    Title,
}

/// The full UI state.
pub struct App {
    pub store_path: PathBuf,
    /// Experiments in current sort order (a superset of the filtered view).
    pub experiments: Vec<ExperimentRow>,
    /// Indices into `experiments` that pass the active filter.
    pub filtered: Vec<usize>,
    /// Experiment ids that appeared since the previous refresh (row highlight).
    pub new_ids: HashSet<String>,
    pub experiment_count: usize,
    pub activity_count: usize,
    pub schema_version: i64,

    pub tab: Tab,
    pub view: View,
    pub table_state: TableState,
    pub selected: usize,

    pub sort_key: SortKey,
    pub sort_asc: bool,
    pub status_filter: String,
    pub title_filter: String,
    pub input: Input,

    pub detail_activities: Vec<ActivityRow>,
    /// Experiment ids marked for side-by-side comparison.
    pub marks: HashSet<String>,

    pub graph_kind_idx: usize,
    pub graph_nodes: Vec<GraphNodeRow>,
    pub graph_state: TableState,
    pub graph_neighbors: Vec<String>,

    pub live: bool,
    pub refresh_interval: Duration,
    pub last_refresh: Instant,
    pub show_help: bool,
    pub error: Option<String>,
}

impl App {
    /// Build the app and load the first snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if the initial read-only snapshot cannot be loaded.
    pub fn new(store_path: PathBuf, refresh_secs: u64) -> anyhow::Result<Self> {
        let snap = data::load_snapshot(&store_path)?;
        let mut app = Self {
            store_path,
            experiments: snap.experiments,
            filtered: Vec::new(),
            new_ids: HashSet::new(),
            experiment_count: snap.experiment_count,
            activity_count: snap.activity_count,
            schema_version: snap.schema_version,
            tab: Tab::Experiments,
            view: View::List,
            table_state: TableState::default(),
            selected: 0,
            sort_key: SortKey::Time,
            sort_asc: false,
            status_filter: String::new(),
            title_filter: String::new(),
            input: Input::None,
            detail_activities: Vec::new(),
            marks: HashSet::new(),
            graph_kind_idx: 0,
            graph_nodes: Vec::new(),
            graph_state: TableState::default(),
            graph_neighbors: Vec::new(),
            live: true,
            refresh_interval: Duration::from_secs(refresh_secs.max(1)),
            last_refresh: Instant::now(),
            show_help: false,
            error: None,
        };
        app.apply_sort_filter();
        app.load_graph();
        Ok(app)
    }

    /// Currently highlighted experiment, if any.
    pub fn selected_experiment(&self) -> Option<&ExperimentRow> {
        self.filtered
            .get(self.selected)
            .and_then(|i| self.experiments.get(*i))
    }

    /// Whether a live refresh is due, then perform it.
    pub fn tick(&mut self) {
        if self.live && self.last_refresh.elapsed() >= self.refresh_interval {
            self.refresh();
        }
    }

    /// Re-take the read-only snapshot, preserving selection by experiment id and
    /// flagging newly-arrived rows.
    pub fn refresh(&mut self) {
        let selected_id = self.selected_experiment().map(|e| e.id.clone());
        let prev_ids: HashSet<String> = self.experiments.iter().map(|e| e.id.clone()).collect();
        match data::load_snapshot(&self.store_path) {
            Ok(snap) => {
                self.new_ids = snap
                    .experiments
                    .iter()
                    .filter(|e| !prev_ids.contains(&e.id))
                    .map(|e| e.id.clone())
                    .collect();
                self.experiments = snap.experiments;
                self.experiment_count = snap.experiment_count;
                self.activity_count = snap.activity_count;
                self.schema_version = snap.schema_version;
                self.error = None;
                self.apply_sort_filter();
                if let Some(id) = selected_id {
                    self.reselect_by_id(&id);
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.last_refresh = Instant::now();
    }

    fn reselect_by_id(&mut self, id: &str) {
        if let Some(pos) = self
            .filtered
            .iter()
            .position(|i| self.experiments[*i].id == id)
        {
            self.selected = pos;
            self.table_state.select(Some(pos));
        }
    }

    /// Re-sort the full list and recompute the filtered index view.
    pub fn apply_sort_filter(&mut self) {
        logic::sort_experiments(&mut self.experiments, self.sort_key, self.sort_asc);
        self.filtered =
            logic::filter_indices(&self.experiments, &self.status_filter, &self.title_filter);
        if self.filtered.is_empty() {
            self.selected = 0;
            self.table_state.select(None);
        } else {
            self.selected = self.selected.min(self.filtered.len() - 1);
            self.table_state.select(Some(self.selected));
        }
    }

    fn move_selection(&mut self, delta: i64) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as i64;
        let next = (self.selected as i64 + delta).rem_euclid(len);
        self.selected = next as usize;
        self.table_state.select(Some(self.selected));
    }

    fn move_graph_selection(&mut self, delta: i64) {
        if self.graph_nodes.is_empty() {
            self.graph_state.select(None);
            return;
        }
        let len = self.graph_nodes.len() as i64;
        let cur = self.graph_state.selected().unwrap_or(0) as i64;
        let next = (cur + delta).rem_euclid(len) as usize;
        self.graph_state.select(Some(next));
        self.load_graph_neighbors(next);
    }

    /// Load graph nodes for the current kind and refresh the neighbour pane.
    pub fn load_graph(&mut self) {
        let kind = self.graph_kind();
        match data::load_graph_nodes(&self.store_path, kind, None) {
            Ok(nodes) => {
                self.graph_nodes = nodes;
                if self.graph_nodes.is_empty() {
                    self.graph_state.select(None);
                    self.graph_neighbors.clear();
                } else {
                    self.graph_state.select(Some(0));
                    self.load_graph_neighbors(0);
                }
            }
            Err(e) => {
                self.graph_nodes.clear();
                self.error = Some(e.to_string());
            }
        }
    }

    fn load_graph_neighbors(&mut self, idx: usize) {
        if let Some(node) = self.graph_nodes.get(idx) {
            self.graph_neighbors =
                data::load_graph_neighbors(&self.store_path, &node.id).unwrap_or_default();
        }
    }

    /// The `ChaosGraph`/Compliance node kind currently selected.
    pub fn graph_kind(&self) -> &'static str {
        // Compliance tab pins its own kinds; ChaosGraph cycles the full set.
        if self.tab == Tab::Compliance {
            const COMPLIANCE_KINDS: [&str; 2] = ["compliance_article", "coverage_gap"];
            COMPLIANCE_KINDS[self.graph_kind_idx % COMPLIANCE_KINDS.len()]
        } else {
            data::GRAPH_KINDS[self.graph_kind_idx % data::GRAPH_KINDS.len()]
        }
    }

    fn enter_detail(&mut self) {
        if let Some(exp) = self.selected_experiment() {
            let id = exp.id.clone();
            self.detail_activities =
                data::load_activities(&self.store_path, &id).unwrap_or_default();
            self.view = View::Detail;
        }
    }

    fn toggle_mark(&mut self) {
        if let Some(exp) = self.selected_experiment() {
            let id = exp.id.clone();
            if !self.marks.remove(&id) {
                self.marks.insert(id);
            }
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.view = View::List;
        if matches!(tab, Tab::ChaosGraph | Tab::Compliance) {
            self.graph_kind_idx = 0;
            self.load_graph();
        }
    }

    /// Handle one key event. Returns `true` when the app should quit.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        // Text entry for the `/` title filter intercepts everything else.
        if self.input == Input::Title {
            self.handle_title_input(key);
            return false;
        }
        if self.show_help {
            self.show_help = false;
            return false;
        }
        match key.code {
            KeyCode::Char('q') if self.view == View::List => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Esc if self.view == View::List => {
                // Esc on the history list clears an active filter first.
                if self.tab == Tab::Experiments
                    && (!self.title_filter.is_empty() || !self.status_filter.is_empty())
                {
                    self.title_filter.clear();
                    self.status_filter.clear();
                    self.apply_sort_filter();
                }
            }
            KeyCode::Esc => {
                self.view = View::List;
            }
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('1') => self.switch_tab(Tab::Experiments),
            KeyCode::Char('2') => self.switch_tab(Tab::Analytics),
            KeyCode::Char('3') => self.switch_tab(Tab::ChaosGraph),
            KeyCode::Char('4') => self.switch_tab(Tab::Compliance),
            KeyCode::Tab => {
                let next = (self.tab.index() + 1) % Tab::ALL.len();
                self.switch_tab(Tab::ALL[next]);
            }
            KeyCode::BackTab => {
                let n = Tab::ALL.len();
                let prev = (self.tab.index() + n - 1) % n;
                self.switch_tab(Tab::ALL[prev]);
            }
            KeyCode::Char('l') => self.live = !self.live,
            KeyCode::Char('r') => self.refresh(),
            _ => self.handle_tab_key(key),
        }
        false
    }

    fn handle_tab_key(&mut self, key: KeyEvent) {
        match self.tab {
            Tab::Experiments => self.handle_experiments_key(key),
            Tab::ChaosGraph | Tab::Compliance => self.handle_graph_key(key),
            Tab::Analytics => {}
        }
    }

    fn handle_experiments_key(&mut self, key: KeyEvent) {
        match self.view {
            View::Detail => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Backspace) {
                    self.view = View::List;
                }
            }
            View::Compare => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Backspace) {
                    self.view = View::List;
                }
            }
            View::List => match key.code {
                KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
                KeyCode::Enter => self.enter_detail(),
                KeyCode::Char('s') => {
                    self.sort_key = self.sort_key.next();
                    self.apply_sort_filter();
                }
                KeyCode::Char('S') => {
                    self.sort_asc = !self.sort_asc;
                    self.apply_sort_filter();
                }
                KeyCode::Char('/') => {
                    self.input = Input::Title;
                }
                KeyCode::Char('f') => self.cycle_status_filter(),
                KeyCode::Char(' ') => self.toggle_mark(),
                KeyCode::Char('c') if !self.marks.is_empty() => self.view = View::Compare,
                _ => {}
            },
        }
    }

    fn handle_graph_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_graph_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_graph_selection(-1),
            KeyCode::Char('s') | KeyCode::Right => {
                self.graph_kind_idx += 1;
                self.load_graph();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.graph_kind_idx == 0 {
                    let n = if self.tab == Tab::Compliance {
                        2
                    } else {
                        data::GRAPH_KINDS.len()
                    };
                    self.graph_kind_idx = n - 1;
                } else {
                    self.graph_kind_idx -= 1;
                }
                self.load_graph();
            }
            _ => {}
        }
    }

    fn cycle_status_filter(&mut self) {
        const CYCLE: [&str; 5] = ["", "pass", "deviated", "fail", "aborted"];
        let cur = CYCLE
            .iter()
            .position(|s| *s == self.status_filter)
            .unwrap_or(0);
        self.status_filter = CYCLE[(cur + 1) % CYCLE.len()].to_string();
        self.apply_sort_filter();
    }

    fn handle_title_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.title_filter.clear();
                self.input = Input::None;
                self.apply_sort_filter();
            }
            KeyCode::Enter => self.input = Input::None,
            KeyCode::Backspace => {
                self.title_filter.pop();
                self.apply_sort_filter();
            }
            KeyCode::Char(c) => {
                self.title_filter.push(c);
                self.apply_sort_filter();
            }
            _ => {}
        }
    }

    /// Experiments currently marked for comparison, in display order.
    pub fn marked_experiments(&self) -> Vec<&ExperimentRow> {
        self.experiments
            .iter()
            .filter(|e| self.marks.contains(&e.id))
            .collect()
    }

    /// Seconds since the last successful refresh, for the status bar.
    pub fn refresh_age_secs(&self) -> u64 {
        self.last_refresh.elapsed().as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExperimentRow;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_with(rows: Vec<ExperimentRow>) -> App {
        // Build an App without touching the store by constructing the fields
        // directly, then wiring the sort/filter view.
        let mut app = App {
            store_path: PathBuf::from("/nonexistent/store.duckdb"),
            experiments: rows,
            filtered: Vec::new(),
            new_ids: HashSet::new(),
            experiment_count: 0,
            activity_count: 0,
            schema_version: 3,
            tab: Tab::Experiments,
            view: View::List,
            table_state: TableState::default(),
            selected: 0,
            sort_key: SortKey::Time,
            sort_asc: false,
            status_filter: String::new(),
            title_filter: String::new(),
            input: Input::None,
            detail_activities: Vec::new(),
            marks: HashSet::new(),
            graph_kind_idx: 0,
            graph_nodes: Vec::new(),
            graph_state: TableState::default(),
            graph_neighbors: Vec::new(),
            live: false,
            refresh_interval: Duration::from_secs(1),
            last_refresh: Instant::now(),
            show_help: false,
            error: None,
        };
        app.apply_sort_filter();
        app
    }

    fn exp(id: &str, status: &str, ns: i64) -> ExperimentRow {
        ExperimentRow {
            id: id.into(),
            title: format!("Experiment {id}"),
            status: status.into(),
            started_at_ns: ns,
            duration_ms: 10,
            resilience: None,
            steps: 1,
            deviations: 0,
        }
    }

    #[test]
    fn navigation_wraps_around() {
        let mut app = app_with(vec![
            exp("a", "completed", 3),
            exp("b", "deviated", 2),
            exp("c", "completed", 1),
        ]);
        assert_eq!(app.selected, 0);
        app.on_key(key(KeyCode::Up)); // wraps to last
        assert_eq!(app.selected, 2);
        app.on_key(key(KeyCode::Down)); // wraps to first
        assert_eq!(app.selected, 0);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn slash_filters_by_title_and_esc_clears() {
        let mut app = app_with(vec![
            exp("alpha", "completed", 2),
            exp("beta", "completed", 1),
        ]);
        app.on_key(key(KeyCode::Char('/')));
        assert_eq!(app.input, Input::Title);
        for c in "alpha".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(app.filtered.len(), 1);
        app.on_key(key(KeyCode::Esc)); // clears filter, exits input
        assert_eq!(app.input, Input::None);
        assert_eq!(app.filtered.len(), 2);
    }

    #[test]
    fn status_filter_cycles() {
        let mut app = app_with(vec![exp("a", "completed", 2), exp("b", "deviated", 1)]);
        app.on_key(key(KeyCode::Char('f'))); // "" -> pass
        assert_eq!(app.status_filter, "pass");
        assert_eq!(app.filtered.len(), 1);
        assert_eq!(app.selected_experiment().unwrap().id, "a");
    }

    #[test]
    fn sort_cycle_changes_key() {
        let mut app = app_with(vec![exp("a", "completed", 1)]);
        assert_eq!(app.sort_key, SortKey::Time);
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!(app.sort_key, SortKey::Duration);
    }

    #[test]
    fn enter_and_esc_toggle_detail_view() {
        let mut app = app_with(vec![exp("a", "completed", 1)]);
        app.on_key(key(KeyCode::Enter)); // store open fails silently, view unchanged
                                         // With an unreachable store, detail activities stay empty but the view
                                         // still switches so the user sees an (empty) timeline rather than nothing.
        assert_eq!(app.view, View::Detail);
        app.on_key(key(KeyCode::Char('q')));
        assert_eq!(app.view, View::List);
    }

    #[test]
    fn marking_and_compare_view() {
        let mut app = app_with(vec![exp("a", "completed", 2), exp("b", "deviated", 1)]);
        app.on_key(key(KeyCode::Char(' '))); // mark a
        app.on_key(key(KeyCode::Char('c'))); // open compare
        assert_eq!(app.view, View::Compare);
        assert_eq!(app.marked_experiments().len(), 1);
    }

    #[test]
    fn tab_switch_by_number() {
        let mut app = app_with(vec![exp("a", "completed", 1)]);
        app.on_key(key(KeyCode::Char('2')));
        assert_eq!(app.tab, Tab::Analytics);
    }

    #[test]
    fn live_toggle_flips() {
        let mut app = app_with(vec![exp("a", "completed", 1)]);
        assert!(!app.live);
        app.on_key(key(KeyCode::Char('l')));
        assert!(app.live);
    }

    #[test]
    fn q_quits_from_list() {
        let mut app = app_with(vec![exp("a", "completed", 1)]);
        assert!(app.on_key(key(KeyCode::Char('q'))));
    }
}
