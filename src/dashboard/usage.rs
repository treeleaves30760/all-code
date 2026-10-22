//! `alc usage` on a terminal: token statistics and account quotas as a
//! dashboard.
//!
//! The same [`Statistics`] the plain report prints, laid out in four tabs:
//! an overview of headline figures and charts, the periods as a table, the
//! models as a sortable, filterable table, and the accounts' quota report.
//! The period and the history source change in place; only the statistics
//! reload, while the accounts (network lookups) are fetched once.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Bar, BarChart, BarGroup, Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Sparkline,
    Table, TableState, Tabs, Wrap,
};

use super::{ansi, hint, spinner, theme};
use crate::config::{Agent, Store};
use crate::doctor::Theme;
use crate::usage::ResolvedBy;
use crate::usage::accounts::{ClaudeStores, Env};
use crate::usage::pricing::Money;
use crate::usage::query::{self, CalendarWindow, KnownTokens, Statistics, UsageOptions};
use crate::usage::view::{compact, short_model, usd};

/// The periods `p` steps through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Period {
    AsAsked,
    Week,
    Month,
    Year,
    AllByDay,
    AllByMonth,
}

impl Period {
    const CYCLE: [Self; 5] = [
        Self::Week,
        Self::Month,
        Self::Year,
        Self::AllByDay,
        Self::AllByMonth,
    ];

    fn label(self, options: &UsageOptions) -> String {
        match self {
            Self::AsAsked => match (options.window, options.monthly) {
                (Some(window), _) => format!("this {}", window_word(window)),
                (None, true) => "all history by month".to_owned(),
                (None, false) => "all history by day".to_owned(),
            },
            Self::Week => "this week".to_owned(),
            Self::Month => "this month".to_owned(),
            Self::Year => "this year".to_owned(),
            Self::AllByDay => "all history by day".to_owned(),
            Self::AllByMonth => "all history by month".to_owned(),
        }
    }

    fn apply(self, options: &mut UsageOptions) {
        let (window, daily, monthly) = match self {
            Self::AsAsked => return,
            Self::Week => (Some(CalendarWindow::Weekly), false, false),
            Self::Month => (Some(CalendarWindow::Monthly), false, false),
            Self::Year => (Some(CalendarWindow::Yearly), false, false),
            Self::AllByDay => (None, true, false),
            Self::AllByMonth => (None, false, true),
        };
        options.window = window;
        options.daily = daily;
        options.monthly = monthly;
        // A calendar window conflicts with explicit bounds.
        options.query.since = None;
        options.query.until = None;
    }
}

fn window_word(window: CalendarWindow) -> &'static str {
    match window {
        CalendarWindow::Weekly => "week",
        CalendarWindow::Monthly => "month",
        CalendarWindow::Yearly => "year",
    }
}

const SOURCES: [&str; 4] = ["all", "alc", "claude", "codex"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Periods,
    Models,
    Accounts,
}

impl Tab {
    const ALL: [Self; 4] = [Self::Overview, Self::Periods, Self::Models, Self::Accounts];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Periods => "Periods",
            Self::Models => "Models",
            Self::Accounts => "Accounts",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0)
    }
}

/// What the charts and the share bars measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    Tokens,
    Cost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Tokens,
    Cost,
    Requests,
    Name,
}

impl SortKey {
    const ALL: [Self; 4] = [Self::Tokens, Self::Cost, Self::Requests, Self::Name];

    fn label(self) -> &'static str {
        match self {
            Self::Tokens => "tokens",
            Self::Cost => "cost",
            Self::Requests => "requests",
            Self::Name => "name",
        }
    }
}

/// One row of a breakdown: a period, a model, an agent or a provider.
#[derive(Debug, Clone, Default)]
pub(crate) struct Group {
    pub label: String,
    /// The model, for a model row; the agents, for a period.
    pub detail: String,
    pub requests: u64,
    pub known: KnownTokens,
    pub cost: Money,
    pub priced: bool,
    pub unpriced: bool,
    pub overlap: bool,
    pub agents: BTreeSet<Agent>,
    pub models: BTreeSet<String>,
}

impl Group {
    fn measure(&self, metric: Metric) -> u64 {
        match metric {
            Metric::Tokens => self.known.total(),
            // Whole tenths of a cent: fine enough for a bar, and fits a u64.
            Metric::Cost => u64::try_from(self.cost.pico_usd() / 1_000_000_000).unwrap_or(u64::MAX),
        }
    }

    /// `+` when a part is unknown, `~` when two histories may overlap.
    fn mark(&self, incomplete: bool) -> (&'static str, &'static str) {
        if self.overlap {
            ("~", "")
        } else if incomplete {
            ("", "+")
        } else {
            ("", "")
        }
    }

    fn tokens_text(&self) -> String {
        let (before, after) = self.mark(self.known.incomplete);
        format!("{before}{}{after}", compact(self.known.total()))
    }

    fn cost_text(&self) -> String {
        if !self.priced {
            return "—".to_owned();
        }
        let (before, after) = self.mark(self.unpriced);
        format!("{before}{}{after}", usd(self.cost))
    }
}

/// The periods, oldest first, as the report grouped them.
pub(crate) fn periods(statistics: &Statistics, monthly: bool) -> Vec<Group> {
    let rollups = if monthly {
        &statistics.monthly
    } else {
        &statistics.daily
    };
    rollups
        .iter()
        .map(|rollup| Group {
            label: rollup.period.clone(),
            detail: rollup
                .agents
                .iter()
                .map(|agent| agent.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            requests: rollup.known_requests,
            known: rollup.known_tokens,
            cost: rollup.known_cost,
            priced: rollup.priced_records + rollup.partial_records > 0,
            unpriced: rollup.unpriced_records > 0,
            overlap: rollup.possible_overlap,
            agents: rollup.agents.clone(),
            models: rollup.models.clone(),
        })
        .collect()
}

/// The report's rows merged by `key`, largest first; rows with no known
/// tokens are left out.
pub(crate) fn grouped(
    statistics: &Statistics,
    key: impl Fn(&query::UsageRow) -> (String, String),
) -> Vec<Group> {
    let mut groups = BTreeMap::<(String, String), Group>::new();
    for row in &statistics.rows {
        let (label, detail) = key(row);
        let group = groups
            .entry((label.clone(), detail.clone()))
            .or_insert_with(|| Group {
                label,
                detail,
                ..Group::default()
            });
        group.requests += row.known_requests;
        group.known.add(&row.known_tokens);
        group.cost = group.cost.checked_add(row.known_cost).unwrap_or(group.cost);
        group.priced |= row.priced_records + row.partial_records > 0;
        group.unpriced |= row.unpriced_records > 0;
        group.overlap |= row.possible_overlap;
        group.agents.insert(row.agent);
        if let Some(model) = &row.model {
            group.models.insert(model.clone());
        }
    }
    let mut groups: Vec<Group> = groups
        .into_values()
        .filter(|group| group.known.total() > 0)
        .collect();
    groups.sort_by(|a, b| {
        b.known
            .total()
            .cmp(&a.known.total())
            .then_with(|| a.label.cmp(&b.label))
    });
    groups
}

fn by_model(statistics: &Statistics) -> Vec<Group> {
    grouped(statistics, |row| {
        (
            row.agent.as_str().to_owned(),
            row.model.clone().unwrap_or_else(|| "unknown".to_owned()),
        )
    })
}

fn by_agent(statistics: &Statistics) -> Vec<Group> {
    grouped(statistics, |row| {
        (row.agent.as_str().to_owned(), String::new())
    })
}

fn by_provider(statistics: &Statistics) -> Vec<Group> {
    grouped(statistics, |row| {
        (
            query::provider_label(row.provider.as_deref(), row.model.as_deref()),
            String::new(),
        )
    })
}

/// One load's statistics and the breakdowns drawn from them, grouped once
/// on the loader thread rather than on every frame.
struct Loaded {
    statistics: Statistics,
    monthly: bool,
    /// Oldest first.
    periods: Vec<Group>,
    active_days: usize,
    models: Vec<Group>,
    agents: Vec<Group>,
    providers: Vec<Group>,
}

impl Loaded {
    fn new(statistics: Statistics, monthly: bool) -> Self {
        Self {
            monthly,
            periods: periods(&statistics, monthly),
            active_days: periods(&statistics, false)
                .iter()
                .filter(|day| day.known.total() > 0)
                .count(),
            models: by_model(&statistics),
            agents: by_agent(&statistics),
            providers: by_provider(&statistics),
            statistics,
        }
    }
}

/// The accounts' quota report, as the plain report prints it.
struct Accounts {
    text: String,
    attention: bool,
}

/// One running background load.
struct Loading<T> {
    rx: Receiver<Result<T, String>>,
}

impl<T: Send + 'static> Loading<T> {
    fn start(work: impl FnOnce() -> Result<T> + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(work().map_err(|error| format!("{error:#}")));
        });
        Self { rx }
    }

    /// The result, once it has arrived.
    fn poll(&self) -> Option<Result<T, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Disconnected) => Some(Err("the loader stopped".to_owned())),
            Err(TryRecvError::Empty) => None,
        }
    }
}

struct Dashboard {
    store: Arc<Store>,
    requested: Option<String>,
    options: UsageOptions,
    period: Period,
    loaded: Option<Loaded>,
    error: Option<String>,
    loading: Option<Loading<Loaded>>,
    accounts: Option<Result<Accounts, String>>,
    accounts_loading: Option<Loading<Accounts>>,
    tab: Tab,
    metric: Metric,
    sort: SortKey,
    descending: bool,
    filter: String,
    editing: bool,
    periods_table: TableState,
    models_table: TableState,
    accounts_scroll: u16,
    detail: bool,
    help: bool,
    tick: usize,
    refreshed: Option<chrono::DateTime<chrono::Local>>,
    exit: bool,
}

pub(crate) fn run(store: &Store, requested: Option<&str>, options: &UsageOptions) -> Result<u8> {
    let mut dashboard = Dashboard::new(
        Arc::new(store.clone()),
        requested.map(str::to_owned),
        options.clone(),
    );
    let mut screen = super::Screen::open()?;
    dashboard.reload();
    dashboard.reload_accounts();
    let mut last_tick = Instant::now();
    loop {
        dashboard.pump();
        screen.terminal.draw(|frame| dashboard.draw(frame))?;
        if dashboard.exit {
            break;
        }
        let timeout = Duration::from_millis(100).saturating_sub(last_tick.elapsed());
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            dashboard.key(key);
        }
        if last_tick.elapsed() >= Duration::from_millis(100) {
            dashboard.tick = dashboard.tick.wrapping_add(1);
            last_tick = Instant::now();
        }
    }
    Ok(u8::from(matches!(
        &dashboard.accounts,
        Some(Ok(accounts)) if accounts.attention
    )))
}

impl Dashboard {
    fn new(store: Arc<Store>, requested: Option<String>, options: UsageOptions) -> Self {
        Self {
            store,
            requested,
            options,
            period: Period::AsAsked,
            loaded: None,
            error: None,
            loading: None,
            accounts: None,
            accounts_loading: None,
            tab: Tab::Overview,
            metric: Metric::Tokens,
            sort: SortKey::Tokens,
            descending: true,
            filter: String::new(),
            editing: false,
            periods_table: TableState::default(),
            models_table: TableState::default().with_selected(Some(0)),
            accounts_scroll: 0,
            detail: false,
            help: false,
            tick: 0,
            refreshed: None,
            exit: false,
        }
    }

    /// The options the statistics are read with: as asked, then the period.
    fn effective(&self) -> UsageOptions {
        let mut options = self.options.clone();
        self.period.apply(&mut options);
        options
    }

    fn reload(&mut self) {
        let store = Arc::clone(&self.store);
        let options = self.effective();
        self.loading = Some(Loading::start(move || {
            let statistics = query::statistics(&store.dir, &store.config, &options)?;
            Ok(Loaded::new(statistics, options.monthly))
        }));
    }

    fn reload_accounts(&mut self) {
        if self.options.offline {
            self.accounts = Some(Ok(Accounts {
                text: "not fetched (--offline)\n".to_owned(),
                attention: false,
            }));
            return;
        }
        let store = Arc::clone(&self.store);
        let requested = self.requested.clone();
        self.accounts_loading = Some(Loading::start(move || {
            let report = crate::usage::build_report(
                &store,
                requested.as_deref(),
                ResolvedBy::Cli,
                &Env::from_process(),
                ClaudeStores::All,
            );
            Ok(Accounts {
                text: crate::usage::render(&report, &Theme::detect()),
                attention: report.needs_attention(),
            })
        }));
    }

    fn pump(&mut self) {
        if let Some(result) = self.loading.as_ref().and_then(Loading::poll) {
            match result {
                Ok(loaded) => {
                    self.loaded = Some(loaded);
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
            self.loading = None;
            self.refreshed = Some(chrono::Local::now());
            self.reset_selection();
        }
        if let Some(result) = self.accounts_loading.as_ref().and_then(Loading::poll) {
            self.accounts = Some(result);
            self.accounts_loading = None;
        }
    }

    fn monthly(&self) -> bool {
        self.loaded
            .as_ref()
            .map_or_else(|| self.effective().monthly, |loaded| loaded.monthly)
    }

    fn source(&self) -> &str {
        match self.options.query.source.as_slice() {
            [] => "all",
            [one] => one.as_str(),
            _ => "mixed",
        }
    }

    /// The periods, newest first for the table.
    fn period_rows(&self) -> Vec<Group> {
        let Some(loaded) = &self.loaded else {
            return Vec::new();
        };
        let mut rows = loaded.periods.clone();
        rows.reverse();
        rows
    }

    /// The model rows shown: filtered, then sorted.
    fn model_rows(&self) -> Vec<Group> {
        let Some(loaded) = &self.loaded else {
            return Vec::new();
        };
        let filter = self.filter.to_lowercase();
        let mut rows: Vec<Group> = loaded
            .models
            .iter()
            .filter(|row| {
                filter.is_empty()
                    || format!("{} {}", row.label, row.detail)
                        .to_lowercase()
                        .contains(&filter)
            })
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            let ordering = match self.sort {
                SortKey::Tokens => a.known.total().cmp(&b.known.total()),
                SortKey::Cost => a.cost.cmp(&b.cost),
                SortKey::Requests => a.requests.cmp(&b.requests),
                SortKey::Name => b.detail.cmp(&a.detail).then(b.label.cmp(&a.label)),
            };
            if self.descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
        rows
    }

    fn reset_selection(&mut self) {
        let periods = self.period_rows().len();
        let models = self.model_rows().len();
        self.periods_table.select((periods > 0).then_some(0));
        let selected = self.models_table.selected().unwrap_or(0);
        self.models_table
            .select((models > 0).then(|| selected.min(models - 1)));
    }

    fn move_by(&mut self, delta: isize) {
        let (count, table) = match self.tab {
            Tab::Periods => (self.period_rows().len(), &mut self.periods_table),
            Tab::Models => (self.model_rows().len(), &mut self.models_table),
            Tab::Accounts => {
                self.accounts_scroll = (i64::from(self.accounts_scroll) + delta as i64)
                    .clamp(0, i64::from(u16::MAX)) as u16;
                return;
            }
            Tab::Overview => return,
        };
        if count == 0 {
            return;
        }
        let selected = table.selected().unwrap_or(0) as isize;
        let next = selected.saturating_add(delta).clamp(0, count as isize - 1);
        table.select(Some(next as usize));
    }

    fn switch(&mut self, tab: Tab) {
        self.tab = tab;
        self.detail = false;
    }

    fn key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.exit = true;
            return;
        }
        if self.editing {
            match key.code {
                KeyCode::Enter => self.editing = false,
                KeyCode::Esc => {
                    self.editing = false;
                    self.filter.clear();
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(ch) => self.filter.push(ch),
                _ => {}
            }
            self.reset_selection();
            return;
        }
        if self.help || self.detail {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?')
            ) {
                self.help = false;
                self.detail = false;
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.exit = true,
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.reset_selection();
            }
            KeyCode::Esc => self.exit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.switch(Tab::ALL[(self.tab.index() + 1) % Tab::ALL.len()]);
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.switch(Tab::ALL[(self.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len()]);
            }
            KeyCode::Char(digit @ '1'..='4') => {
                self.switch(Tab::ALL[digit as usize - '1' as usize]);
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.move_by(10),
            KeyCode::PageUp | KeyCode::Char('b') => self.move_by(-10),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX / 2),
            KeyCode::Enter => {
                self.detail = match self.tab {
                    Tab::Periods => !self.period_rows().is_empty(),
                    Tab::Models => !self.model_rows().is_empty(),
                    _ => false,
                };
            }
            KeyCode::Char('p') => {
                let next = Period::CYCLE
                    .iter()
                    .position(|period| *period == self.period)
                    .map_or(0, |index| (index + 1) % Period::CYCLE.len());
                self.period = Period::CYCLE[next];
                self.reload();
            }
            KeyCode::Char('s') => {
                let current = SOURCES
                    .iter()
                    .position(|source| *source == self.source())
                    .unwrap_or(0);
                let next = SOURCES[(current + 1) % SOURCES.len()];
                self.options.query.source = if next == "all" {
                    Vec::new()
                } else {
                    vec![next.to_owned()]
                };
                self.reload();
            }
            KeyCode::Char('c') => {
                self.metric = match self.metric {
                    Metric::Tokens => Metric::Cost,
                    Metric::Cost => Metric::Tokens,
                };
            }
            KeyCode::Char('o') => {
                let index = SortKey::ALL
                    .iter()
                    .position(|key| *key == self.sort)
                    .unwrap_or(0);
                self.sort = SortKey::ALL[(index + 1) % SortKey::ALL.len()];
                self.descending = self.sort != SortKey::Name;
                if self.tab != Tab::Models {
                    self.switch(Tab::Models);
                }
            }
            KeyCode::Char('O') => self.descending = !self.descending,
            KeyCode::Char('/') | KeyCode::Char('f') => {
                self.switch(Tab::Models);
                self.editing = true;
                self.filter.clear();
            }
            KeyCode::Char('r') => {
                self.reload();
                self.reload_accounts();
            }
            _ => {}
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header, tabs, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_header(frame, header);
        self.draw_tabs(frame, tabs);
        match self.tab {
            Tab::Overview => self.draw_overview(frame, body),
            Tab::Periods => self.draw_periods(frame, body),
            Tab::Models => self.draw_models(frame, body),
            Tab::Accounts => self.draw_accounts(frame, body),
        }
        self.draw_footer(frame, footer);
        if self.detail {
            self.draw_detail(frame);
        }
        if self.help {
            draw_help(frame);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let mut subtitle = format!(
            "{} · source {}",
            self.period.label(&self.options),
            self.source()
        );
        let query = &self.options.query;
        for (label, value) in [
            ("profile", query.filter_profile.clone()),
            ("agent", query.filter_agent.map(|agent| agent.to_string())),
            ("model", query.filter_model.clone()),
            ("since", query.since.clone()),
            ("until", query.until.clone()),
        ] {
            if let Some(value) = value {
                subtitle.push_str(&format!(" · {label} {value}"));
            }
        }
        if let Some(loaded) = &self.loaded {
            subtitle.push_str(&format!(" · {}", loaded.statistics.timezone));
        }
        let left = Line::from(vec![
            Span::styled(" alc usage ", theme::title()),
            Span::styled(subtitle, theme::muted()),
        ]);
        let mut right = Vec::new();
        if self.loading.is_some() {
            right.push(Span::styled(
                format!("{} reading history  ", spinner(self.tick)),
                Style::default().fg(theme::ACCENT),
            ));
        }
        if let Some(at) = self.refreshed {
            right.push(Span::styled(
                at.format("%H:%M:%S ").to_string(),
                theme::muted(),
            ));
        }
        let width: usize = right.iter().map(|span| span.content.chars().count()).sum();
        let [left_area, right_area] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(width as u16)]).areas(area);
        frame.render_widget(Paragraph::new(left), left_area);
        frame.render_widget(Paragraph::new(Line::from(right)), right_area);
    }

    fn draw_tabs(&self, frame: &mut Frame, area: Rect) {
        let titles: Vec<Line> = Tab::ALL
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let mut spans = vec![
                    Span::styled(format!("{} ", index + 1), theme::muted()),
                    Span::raw(tab.label()),
                ];
                if *tab == Tab::Accounts && self.accounts_loading.is_some() {
                    spans.push(Span::styled(
                        format!(" {}", spinner(self.tick)),
                        Style::default().fg(theme::ACCENT),
                    ));
                }
                if *tab == Tab::Accounts
                    && matches!(&self.accounts, Some(Ok(accounts)) if accounts.attention)
                {
                    spans.push(Span::styled(" !", Style::default().fg(theme::WARN)));
                }
                Line::from(spans)
            })
            .collect();
        let tabs = Tabs::new(titles)
            .select(self.tab.index())
            .highlight_style(
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )
            .divider(Span::styled("│", theme::muted()));
        frame.render_widget(tabs, area);
    }

    /// Shown in place of a tab's content while it has nothing to show.
    fn placeholder(&self, frame: &mut Frame, area: Rect) -> bool {
        let statistics = self.loaded.as_ref().map(|loaded| &loaded.statistics);
        let message = match (statistics, &self.error) {
            (_, Some(error)) => Some((format!("could not read usage: {error}"), theme::BAD)),
            (None, None) => Some((
                format!("{} reading local history…", spinner(self.tick)),
                theme::ACCENT,
            )),
            (Some(statistics), None) if statistics.records == 0 => Some((
                "no matching usage records — p changes the period, s the source".to_owned(),
                theme::MUTED,
            )),
            _ => None,
        };
        let Some((message, color)) = message else {
            return false;
        };
        let block = panel(" Token usage ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(Span::styled(message, Style::default().fg(color)))
                .wrap(Wrap { trim: true }),
            inner.inner(ratatui::layout::Margin::new(1, 1)),
        );
        true
    }

    fn draw_overview(&self, frame: &mut Frame, area: Rect) {
        if self.placeholder(frame, area) {
            return;
        }
        let Some(loaded) = &self.loaded else {
            return;
        };
        let statistics = &loaded.statistics;
        let [cards, charts, shares] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(9.min(area.height.saturating_sub(12)).max(5)),
        ])
        .areas(area);
        self.draw_cards(frame, cards, loaded);
        let [trend, hours] =
            Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)])
                .areas(charts);
        self.draw_trend(frame, trend, loaded);
        draw_hours(frame, hours, statistics);
        let [models, agents, providers] = Layout::horizontal([
            Constraint::Percentage(44),
            Constraint::Percentage(28),
            Constraint::Percentage(28),
        ])
        .areas(shares);
        let model_rows: Vec<Group> = loaded
            .models
            .iter()
            .map(|group| Group {
                label: short_model(&group.detail),
                ..group.clone()
            })
            .collect();
        self.draw_shares(frame, models, " Top models ", &model_rows, Color::Magenta);
        self.draw_shares(frame, agents, " Agents ", &loaded.agents, theme::GOOD);
        self.draw_shares(
            frame,
            providers,
            " Providers ",
            &loaded.providers,
            Color::Blue,
        );
    }

    fn draw_cards(&self, frame: &mut Frame, area: Rect, loaded: &Loaded) {
        let statistics = &loaded.statistics;
        let known = statistics.known_tokens;
        let requests: u64 = statistics.rows.iter().map(|row| row.known_requests).sum();
        let total = Group {
            known,
            cost: statistics.known_cost,
            priced: statistics.priced_records > 0
                || statistics.rows.iter().any(|row| row.partial_records > 0),
            unpriced: statistics.unpriced_records > 0,
            overlap: statistics.possible_overlap,
            ..Group::default()
        };
        let cache_share = if known.input() == 0 {
            "—".to_owned()
        } else {
            format!(
                "{:.0}%",
                known.cache_read as f64 * 100.0 / known.input() as f64
            )
        };
        let days = loaded.active_days;
        let cards = [
            (
                "Total tokens",
                total.tokens_text(),
                format!("{} records", query::format_count(statistics.records)),
                theme::ACCENT,
            ),
            (
                "Est. cost",
                total.cost_text(),
                if statistics.unpriced_records > 0 {
                    format!("{} unpriced records", statistics.unpriced_records)
                } else {
                    "API-equivalent, not a bill".to_owned()
                },
                theme::GOOD,
            ),
            (
                "Requests",
                if requests == 0 {
                    "—".to_owned()
                } else {
                    query::format_count(requests)
                },
                format!("{} active day{}", days, if days == 1 { "" } else { "s" }),
                Color::Magenta,
            ),
            (
                "Input / output",
                format!("{} / {}", compact(known.input()), compact(known.output)),
                format!("{} uncached input", compact(known.uncached_input)),
                Color::Blue,
            ),
            (
                "Cache reads",
                cache_share,
                format!("of input · {} written", compact(known.cache_write)),
                theme::WARN,
            ),
        ];
        let areas = Layout::horizontal([Constraint::Ratio(1, 5); 5]).split(area);
        for ((label, value, note, color), area) in cards.into_iter().zip(areas.iter()) {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(color))
                .title(Span::styled(
                    format!(" {label} "),
                    Style::default().fg(color),
                ));
            let text = vec![
                Line::from(Span::styled(
                    value,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(note, theme::muted())),
            ];
            frame.render_widget(Paragraph::new(text).block(block), *area);
        }
    }

    fn draw_trend(&self, frame: &mut Frame, area: Rect, loaded: &Loaded) {
        let monthly = loaded.monthly;
        let rows = &loaded.periods;
        let (bar_width, label_of): (u16, fn(&str) -> String) = if monthly {
            // 2026-10 → 26-10
            (5, |period| period.get(2..).unwrap_or(period).to_owned())
        } else {
            // 2026-10-23 → 10-23
            (5, |period| period.get(5..).unwrap_or(period).to_owned())
        };
        let inner_width = area.width.saturating_sub(2);
        let fits = usize::from((inner_width + 1) / (bar_width + 1)).max(1);
        let shown = &rows[rows.len().saturating_sub(fits)..];
        let color = match self.metric {
            Metric::Tokens => theme::ACCENT,
            Metric::Cost => theme::GOOD,
        };
        let bars: Vec<Bar> = shown
            .iter()
            .map(|row| {
                let value = row.measure(self.metric);
                let text = match self.metric {
                    Metric::Tokens => compact(row.known.total()),
                    Metric::Cost if row.priced => short_usd(row.cost),
                    Metric::Cost => String::new(),
                };
                Bar::default()
                    .value(value)
                    .text_value(text)
                    .label(Line::from(label_of(&row.label)))
                    .style(Style::default().fg(color))
                    .value_style(Style::default().fg(Color::Black).bg(color))
            })
            .collect();
        let what = match self.metric {
            Metric::Tokens => "Tokens",
            Metric::Cost => "Est. cost",
        };
        let span = if monthly { "month" } else { "day" };
        let hidden = rows.len() - shown.len();
        let mut block = panel(&format!(" {what} per {span} "));
        if hidden > 0 {
            block = block.title_bottom(Span::styled(
                format!(
                    " last {} of {} · 2 Periods has all ",
                    shown.len(),
                    rows.len()
                ),
                theme::muted(),
            ));
        } else {
            block = block.title_bottom(Span::styled(" c tokens ⇄ cost ", theme::muted()));
        }
        frame.render_widget(
            BarChart::default()
                .block(block)
                .data(BarGroup::default().bars(&bars))
                .bar_width(bar_width)
                .bar_gap(1)
                .label_style(theme::muted()),
            area,
        );
    }

    fn draw_shares(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        rows: &[Group],
        color: Color,
    ) {
        let block = panel(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let grand: u64 = rows.iter().map(|row| row.measure(self.metric)).sum();
        if grand == 0 {
            frame.render_widget(Paragraph::new(Span::styled(" —", theme::muted())), inner);
            return;
        }
        let name_width = rows
            .iter()
            .map(|row| row.label.chars().count())
            .max()
            .unwrap_or(4)
            .clamp(4, 22)
            .min(usize::from(inner.width) / 2);
        // name, space, bar, space, "100%"
        let bar_width = usize::from(inner.width)
            .saturating_sub(name_width + 7)
            .max(2);
        let lines: Vec<Line> = rows
            .iter()
            .take(usize::from(inner.height))
            .map(|row| {
                let value = row.measure(self.metric);
                let share = value as f64 / grand as f64;
                let name: String = row.label.chars().take(name_width).collect();
                Line::from(vec![
                    Span::raw(format!("{name:<name_width$} ")),
                    Span::styled(bar(share, bar_width), Style::default().fg(color)),
                    Span::styled(format!(" {:>4}", percent(share)), theme::muted()),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_periods(&mut self, frame: &mut Frame, area: Rect) {
        if self.placeholder(frame, area) {
            return;
        }
        let rows = self.period_rows();
        let peak = rows
            .iter()
            .map(|row| row.measure(self.metric))
            .max()
            .unwrap_or(0)
            .max(1);
        let color = match self.metric {
            Metric::Tokens => theme::ACCENT,
            Metric::Cost => theme::GOOD,
        };
        let header = Row::new([
            Cell::from(if self.monthly() { "Month" } else { "Date" }),
            right("Requests".to_owned()),
            right("Input".to_owned()),
            right("Output".to_owned()),
            right("Cache read".to_owned()),
            right("Total".to_owned()),
            right("Cost".to_owned()),
            Cell::from(match self.metric {
                Metric::Tokens => "  tokens",
                Metric::Cost => "  cost",
            }),
            Cell::from("Agents"),
        ])
        .style(
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        );
        let body: Vec<Row> = rows
            .iter()
            .map(|row| {
                let share = row.measure(self.metric) as f64 / peak as f64;
                Row::new(vec![
                    Cell::from(row.label.clone()),
                    right(count_or_dash(row.requests)),
                    right(compact(row.known.uncached_input)),
                    right(compact(row.known.output)),
                    right(compact(row.known.cache_read)),
                    right(row.tokens_text()).style(Style::default().add_modifier(Modifier::BOLD)),
                    right(row.cost_text()).style(Style::default().fg(theme::GOOD)),
                    Cell::from(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(bar(share, 16), Style::default().fg(color)),
                    ])),
                    Cell::from(row.detail.clone()).style(theme::muted()),
                ])
            })
            .collect();
        let table = Table::new(
            body,
            [
                Constraint::Length(10),
                Constraint::Length(9),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(9),
                Constraint::Length(11),
                Constraint::Length(18),
                Constraint::Min(8),
            ],
        )
        .header(header)
        .block(
            panel(&format!(" Periods · {} · newest first ", rows.len()))
                .border_style(Style::default().fg(theme::ACCENT)),
        )
        .row_highlight_style(theme::selected())
        .highlight_symbol("▌");
        frame.render_stateful_widget(table, area, &mut self.periods_table);
    }

    fn draw_models(&mut self, frame: &mut Frame, area: Rect) {
        if self.placeholder(frame, area) {
            return;
        }
        let rows = self.model_rows();
        let grand: u64 = self
            .loaded
            .as_ref()
            .map(|loaded| {
                loaded
                    .models
                    .iter()
                    .map(|row| row.measure(self.metric))
                    .sum()
            })
            .unwrap_or(0)
            .max(1);
        // What the fixed columns, the spacing and the borders leave over.
        let share_width = usize::from(area.width).saturating_sub(112 + 7).clamp(6, 40);
        let arrow = if self.descending { "▼" } else { "▲" };
        let heading = |label: &str, key: SortKey| {
            if self.sort == key {
                format!("{label} {arrow}")
            } else {
                label.to_owned()
            }
        };
        let header = Row::new([
            Cell::from("Agent"),
            Cell::from(heading("Model", SortKey::Name)),
            right(heading("Requests", SortKey::Requests)),
            right("Input".to_owned()),
            right("Output".to_owned()),
            right("Cache W".to_owned()),
            right("Cache R".to_owned()),
            right(heading("Total", SortKey::Tokens)),
            Cell::from("  Share"),
            right(heading("Cost", SortKey::Cost)),
        ])
        .style(
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        );
        let body: Vec<Row> = rows
            .iter()
            .map(|row| {
                let share = row.measure(self.metric) as f64 / grand as f64;
                Row::new(vec![
                    Cell::from(row.label.clone()).style(theme::muted()),
                    Cell::from(short_model(&row.detail)),
                    right(count_or_dash(row.requests)),
                    right(compact(row.known.uncached_input)),
                    right(compact(row.known.output)),
                    right(compact(row.known.cache_write)),
                    right(compact(row.known.cache_read)),
                    right(row.tokens_text()).style(Style::default().add_modifier(Modifier::BOLD)),
                    Cell::from(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(bar(share, share_width), Style::default().fg(Color::Magenta)),
                        Span::styled(format!(" {:>4}", percent(share)), theme::muted()),
                    ])),
                    right(row.cost_text()).style(Style::default().fg(theme::GOOD)),
                ])
            })
            .collect();
        let empty = body.is_empty();
        let title = format!(
            " Models · {} shown · sorted by {}{} ",
            rows.len(),
            self.sort.label(),
            if self.filter.is_empty() {
                String::new()
            } else {
                format!(" · matching “{}”", self.filter)
            }
        );
        let table = Table::new(
            body,
            [
                Constraint::Length(8),
                Constraint::Max(30),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(9),
                Constraint::Length(share_width as u16 + 7),
                Constraint::Length(11),
            ],
        )
        .header(header)
        .block(panel(&title).border_style(Style::default().fg(theme::ACCENT)))
        .row_highlight_style(theme::selected())
        .highlight_symbol("▌");
        frame.render_stateful_widget(table, area, &mut self.models_table);
        if empty {
            let inner = Rect {
                x: area.x + 2,
                y: area.y + 2,
                width: area.width.saturating_sub(4),
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "nothing matches the filter — esc clears it",
                    theme::muted(),
                )),
                inner,
            );
        }
    }

    fn draw_accounts(&mut self, frame: &mut Frame, area: Rect) {
        let block = panel(" Accounts · login quota and plan usage ")
            .border_style(Style::default().fg(theme::ACCENT));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let lines: Vec<Line> = match &self.accounts {
            None => vec![Line::from(Span::styled(
                format!(
                    "{} checking accounts — quota lookups go over the network",
                    spinner(self.tick)
                ),
                Style::default().fg(theme::ACCENT),
            ))],
            Some(Err(error)) => vec![Line::from(Span::styled(
                format!("could not check accounts: {error}"),
                Style::default().fg(theme::BAD),
            ))],
            Some(Ok(accounts)) => accounts.text.lines().map(ansi::line).collect(),
        };
        let most = (lines.len() as u16).saturating_sub(inner.height);
        self.accounts_scroll = self.accounts_scroll.min(most);
        frame.render_widget(
            Paragraph::new(lines).scroll((self.accounts_scroll, 0)),
            inner,
        );
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        if self.editing {
            let line = Line::from(vec![
                Span::styled(" filter ", theme::key()),
                Span::raw(format!(" {}", self.filter)),
                Span::styled("█", Style::default().fg(theme::ACCENT)),
                Span::styled("   agent or model · enter keep · esc clear", theme::muted()),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        let mut keys = vec![
            ("tab", "view"),
            ("p", "period"),
            ("s", "source"),
            ("c", "tokens/cost"),
        ];
        match self.tab {
            Tab::Periods => keys.extend([("↑↓", "select"), ("enter", "details")]),
            Tab::Models => keys.extend([
                ("↑↓", "select"),
                ("enter", "details"),
                ("o", "sort"),
                ("/", "filter"),
            ]),
            Tab::Accounts => keys.push(("↑↓", "scroll")),
            Tab::Overview => {}
        }
        keys.extend([("r", "reload"), ("?", "help"), ("q", "quit")]);
        let mut spans = Vec::new();
        for (key, label) in keys {
            spans.extend(hint(key, label));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_detail(&self, frame: &mut Frame) {
        let (title, row) = match self.tab {
            Tab::Periods => {
                let rows = self.period_rows();
                let Some(row) = self
                    .periods_table
                    .selected()
                    .and_then(|index| rows.get(index).cloned())
                else {
                    return;
                };
                (format!(" {} ", row.label), row)
            }
            Tab::Models => {
                let rows = self.model_rows();
                let Some(row) = self
                    .models_table
                    .selected()
                    .and_then(|index| rows.get(index).cloned())
                else {
                    return;
                };
                (format!(" {} · {} ", row.label, row.detail), row)
            }
            _ => return,
        };
        let known = row.known;
        let mut fields = vec![
            ("requests", count_or_dash(row.requests)),
            ("uncached input", query::format_count(known.uncached_input)),
            ("cache write", query::format_count(known.cache_write)),
            ("cache read", query::format_count(known.cache_read)),
            ("output", query::format_count(known.output)),
            ("total tokens", row.tokens_text()),
            ("est. cost", row.cost_text()),
        ];
        if !row.agents.is_empty() {
            fields.push((
                "agents",
                row.agents
                    .iter()
                    .map(|agent| agent.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        let models: Vec<String> = row
            .models
            .iter()
            .filter(|model| !model.starts_with('<'))
            .cloned()
            .collect();
        if self.tab == Tab::Periods && !models.is_empty() {
            fields.push(("models", models.join(", ")));
        }
        if known.incomplete {
            fields.push(("note", "some records carry no token counts (+)".to_owned()));
        }
        if row.unpriced && row.priced {
            fields.push(("note", "some records have no price (+)".to_owned()));
        }
        if row.overlap {
            fields.push((
                "note",
                "two histories may count a request twice (~); s picks one".to_owned(),
            ));
        }
        let area = frame.area();
        let width = 80.min(area.width);
        let label_width = 16;
        let lines: Vec<Line> = fields
            .into_iter()
            .map(|(label, value)| {
                Line::from(vec![
                    Span::styled(
                        format!(" {label:<label_width$}"),
                        Style::default().fg(theme::ACCENT),
                    ),
                    Span::raw(value),
                ])
            })
            .collect();
        let height = (lines.len() as u16 + 4).min(area.height);
        let popup = centered(area, width, height);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                panel(&title)
                    .border_style(Style::default().fg(theme::ACCENT))
                    .title_bottom(Span::styled(" esc close ", theme::muted())),
            ),
            popup,
        );
    }
}

fn draw_hours(frame: &mut Frame, area: Rect, statistics: &Statistics) {
    let hours = &statistics.activity.hours;
    let peak_hour = hours
        .iter()
        .enumerate()
        .max_by_key(|(_, tokens)| **tokens)
        .filter(|(_, tokens)| **tokens > 0)
        .map(|(hour, _)| hour);
    let block = panel(" Hour of day ").title_bottom(Span::styled(
        match peak_hour {
            Some(hour) => format!(" busiest {hour:02}:00 · {} ", statistics.timezone),
            None => " no timed records ".to_owned(),
        },
        theme::muted(),
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 {
        return;
    }
    // Each hour as wide as the panel allows, so 24 columns fill it.
    let per_hour = (usize::from(inner.width) / 24).max(1);
    let data: Vec<u64> = hours
        .iter()
        .flat_map(|tokens| std::iter::repeat_n(*tokens, per_hour))
        .collect();
    let [chart, axis] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Sparkline::default()
            .data(&data)
            .style(Style::default().fg(Color::Magenta)),
        chart,
    );
    let mut ruler = String::new();
    for hour in (0..24).step_by(6) {
        let label = format!("{hour:02}");
        let at = hour * per_hour;
        while ruler.chars().count() < at {
            ruler.push(' ');
        }
        ruler.push_str(&label);
    }
    frame.render_widget(Paragraph::new(Span::styled(ruler, theme::muted())), axis);
}

fn draw_help(frame: &mut Frame) {
    let rows = [
        ("tab ← →  1-4", "switch view"),
        (
            "p",
            "next period: week, month, year, all by day, all by month",
        ),
        ("s", "next source: all, alc, claude, codex"),
        ("c", "chart and share by tokens or by cost"),
        ("↑ ↓  j k", "select a row, or scroll the accounts"),
        ("PgUp PgDn  g G", "page, top, bottom"),
        ("enter", "the period or model in full"),
        ("o  O", "sort models by the next column, reverse"),
        ("/  f", "filter models by agent or model"),
        ("r", "read history and check accounts again"),
        ("q  esc", "leave"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(key, label)| {
            Line::from(vec![
                Span::styled(format!(" {key:<16}"), Style::default().fg(theme::ACCENT)),
                Span::raw(*label),
            ])
        })
        .chain([
            Line::raw(""),
            Line::from(Span::styled(
                " + at least (unknown parts) · ~ approximate (sources may overlap)",
                theme::muted(),
            )),
            Line::from(Span::styled(
                " Costs are API-equivalent estimates. alc usage -nt prints the plain report.",
                theme::muted(),
            )),
        ])
        .collect();
    let area = frame.area();
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = centered(area, 82.min(area.width), height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(" Keys ").border_style(Style::default().fg(theme::ACCENT))),
        popup,
    );
}

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::muted())
        .title(Span::styled(title.to_owned(), theme::title()))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// A bar of eighth blocks, `share` of `width` cells.
fn bar(share: f64, width: usize) -> String {
    let eighths = (share.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
    let mut text = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        text.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][eighths % 8 - 1]);
    }
    let drawn = text.chars().count();
    text.push_str(&" ".repeat(width.saturating_sub(drawn)));
    text
}

fn percent(share: f64) -> String {
    if share > 0.0 && share < 0.01 {
        "<1%".to_owned()
    } else {
        format!("{:.0}%", share * 100.0)
    }
}

/// Whole dollars past $100, so a bar's label fits its five columns.
fn short_usd(money: Money) -> String {
    let dollars = money.pico_usd() as f64 / 1e12;
    if dollars >= 1000.0 {
        format!("${:.1}k", dollars / 1000.0)
    } else if dollars >= 100.0 {
        format!("${dollars:.0}")
    } else {
        format!("${dollars:.1}")
    }
}

fn count_or_dash(value: u64) -> String {
    if value == 0 {
        "—".to_owned()
    } else {
        query::format_count(value)
    }
}

fn right(text: String) -> Cell<'static> {
    Cell::from(Line::from(text).alignment(Alignment::Right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn dashboard() -> Dashboard {
        Dashboard::new(
            Arc::new(Store {
                dir: std::path::PathBuf::from("/nonexistent"),
                config: crate::config::Config::default(),
                credentials: Default::default(),
            }),
            None,
            UsageOptions::default(),
        )
    }

    fn screen(dashboard: &mut Dashboard) -> String {
        let mut terminal = Terminal::new(TestBackend::new(150, 40)).unwrap();
        terminal.draw(|frame| dashboard.draw(frame)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn the_period_key_steps_through_windows() {
        let mut dashboard = dashboard();
        assert!(
            dashboard
                .period
                .label(&dashboard.options)
                .starts_with("all history by day")
        );
        dashboard.key(KeyEvent::from(KeyCode::Char('p')));
        assert_eq!(dashboard.period, Period::Week);
        assert!(dashboard.loading.is_some(), "a new period reloads");
        for _ in 0..4 {
            dashboard.key(KeyEvent::from(KeyCode::Char('p')));
        }
        assert_eq!(dashboard.period, Period::AllByMonth);
        assert!(dashboard.monthly());
        dashboard.key(KeyEvent::from(KeyCode::Char('p')));
        assert_eq!(dashboard.period, Period::Week);

        let mut options = UsageOptions::default();
        options.query.since = Some("2026-01-01".to_owned());
        Period::Month.apply(&mut options);
        assert_eq!(options.window, Some(CalendarWindow::Monthly));
        assert!(options.query.since.is_none());
    }

    #[test]
    fn the_source_key_cycles_every_history() {
        let mut dashboard = dashboard();
        let mut seen = Vec::new();
        for _ in 0..4 {
            dashboard.key(KeyEvent::from(KeyCode::Char('s')));
            seen.push(dashboard.source().to_owned());
        }
        assert_eq!(seen, ["alc", "claude", "codex", "all"]);
    }

    #[test]
    fn tabs_switch_by_number_arrow_and_tab() {
        let mut dashboard = dashboard();
        dashboard.key(KeyEvent::from(KeyCode::Char('3')));
        assert_eq!(dashboard.tab, Tab::Models);
        dashboard.key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(dashboard.tab, Tab::Accounts);
        dashboard.key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(dashboard.tab, Tab::Overview);
        dashboard.key(KeyEvent::from(KeyCode::Left));
        assert_eq!(dashboard.tab, Tab::Accounts);
        dashboard.key(KeyEvent::from(KeyCode::Char('/')));
        assert_eq!(dashboard.tab, Tab::Models, "the filter belongs to models");
        assert!(dashboard.editing);
    }

    #[test]
    fn every_tab_draws_while_loading_and_after_an_error() {
        let mut dashboard = dashboard();
        assert!(screen(&mut dashboard).contains("reading local history"));
        dashboard.error = Some("boom".to_owned());
        for tab in Tab::ALL {
            dashboard.switch(tab);
            let text = screen(&mut dashboard);
            assert!(text.contains("alc usage"), "{text}");
            if tab != Tab::Accounts {
                assert!(text.contains("could not read usage: boom"), "{text}");
            }
        }
        dashboard.accounts = Some(Ok(Accounts {
            text: "Accounts\n  \x1b[32mcodex\x1b[0m 42% left\n".to_owned(),
            attention: true,
        }));
        let text = screen(&mut dashboard);
        assert!(text.contains("42% left"), "{text}");
    }

    #[test]
    fn bars_fill_in_eighths_and_keep_their_width() {
        assert_eq!(bar(0.0, 4), "    ");
        assert_eq!(bar(1.0, 4), "████");
        assert_eq!(bar(0.5, 4), "██  ");
        assert_eq!(bar(1.0 / 32.0, 4), "▏   ");
        assert_eq!(percent(0.004), "<1%");
        assert_eq!(percent(0.5), "50%");
        assert_eq!(short_usd(Money::from_pico_usd(12_340_000_000_000)), "$12.3");
        assert_eq!(
            short_usd(Money::from_pico_usd(1_234_000_000_000_000)),
            "$1.2k"
        );
    }
}
