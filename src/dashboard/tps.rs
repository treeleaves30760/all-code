//! `alc tps` on a terminal: the measured requests as a dashboard.
//!
//! The same rows and summary the plain report prints ([`tps_report`]), laid
//! out as headline figures, a throughput trend, a per-model comparison and a
//! sortable, filterable request table with a detail view.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::TimeZone as _;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, TableState,
};

use super::{hint, spinner, theme};
use crate::config::Config;
use crate::usage::query::{TpsOptions, TpsReport, TpsRow, tps_report};
use crate::usage::records::{Outcome, OutputBasis};

/// The dashboard reads more history than the plain report's twenty rows,
/// unless a limit was asked for.
const DEFAULT_LIMIT: u32 = 200;
const LIMITS: [u32; 6] = [20, 50, 100, 200, 500, 1000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Time,
    Model,
    Agent,
    Ttft,
    Tps,
    E2e,
    Output,
}

impl SortKey {
    const ALL: [Self; 7] = [
        Self::Time,
        Self::Model,
        Self::Agent,
        Self::Ttft,
        Self::Tps,
        Self::E2e,
        Self::Output,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Model => "model",
            Self::Agent => "agent",
            Self::Ttft => "TTFT",
            Self::Tps => "TPS",
            Self::E2e => "E2E",
            Self::Output => "output",
        }
    }
}

/// One model's figures over the rows shown.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelStats {
    pub model: String,
    pub requests: usize,
    pub median_tps: Option<f64>,
    pub median_ttft: Option<f64>,
}

pub(crate) fn per_model(rows: &[&TpsRow]) -> Vec<ModelStats> {
    let mut grouped: BTreeMap<String, (usize, Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for row in rows {
        let model = row
            .record
            .model
            .clone()
            .unwrap_or_else(|| "unknown".to_owned());
        let entry = grouped.entry(model).or_default();
        entry.0 += 1;
        if let Some(tps) = row.metrics.stream_tps.filter(|tps| tps.is_finite()) {
            entry.1.push(tps);
        }
        if let Some(ttft) = row.metrics.ttft_ms {
            entry.2.push(ttft);
        }
    }
    let mut stats: Vec<ModelStats> = grouped
        .into_iter()
        .map(|(model, (requests, mut tps, mut ttft))| ModelStats {
            model,
            requests,
            median_tps: median(&mut tps),
            median_ttft: median(&mut ttft),
        })
        .collect();
    stats.sort_by(|a, b| b.requests.cmp(&a.requests).then(a.model.cmp(&b.model)));
    stats
}

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return 0.0;
    }
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)]
}

/// The median, which one runaway estimate (a burst of cached tokens read
/// as 200,000 tokens a second) cannot drag the way it drags a mean.
fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

struct Dashboard {
    config_dir: PathBuf,
    config: Config,
    options: TpsOptions,
    report: Option<TpsReport>,
    error: Option<String>,
    loading: Option<Receiver<Result<TpsReport, String>>>,
    sort: SortKey,
    descending: bool,
    filter: String,
    editing: bool,
    table: TableState,
    detail: bool,
    help: bool,
    tick: usize,
    refreshed: Option<chrono::DateTime<chrono::Local>>,
    exit: bool,
}

pub(crate) fn run(config_dir: &Path, config: &Config, options: &TpsOptions) -> Result<u8> {
    let mut options = options.clone();
    if options.limit == 20 {
        options.limit = DEFAULT_LIMIT;
    }
    let mut dashboard = Dashboard::new(config_dir.to_owned(), config.clone(), options);
    let mut screen = super::Screen::open()?;
    dashboard.reload();
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
    Ok(0)
}

impl Dashboard {
    fn new(config_dir: PathBuf, config: Config, options: TpsOptions) -> Self {
        Self {
            config_dir,
            config,
            options,
            report: None,
            error: None,
            loading: None,
            sort: SortKey::Time,
            descending: true,
            filter: String::new(),
            editing: false,
            table: TableState::default().with_selected(Some(0)),
            detail: false,
            help: false,
            tick: 0,
            refreshed: None,
            exit: false,
        }
    }

    fn reload(&mut self) {
        let (tx, rx) = mpsc::channel();
        let (dir, config, options) = (
            self.config_dir.clone(),
            self.config.clone(),
            self.options.clone(),
        );
        thread::spawn(move || {
            let _ = tx.send(tps_report(&dir, &config, &options).map_err(|e| format!("{e:#}")));
        });
        self.loading = Some(rx);
    }

    fn pump(&mut self) {
        let Some(rx) = &self.loading else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                match result {
                    Ok(report) => {
                        self.report = Some(report);
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
                self.loading = None;
                self.refreshed = Some(chrono::Local::now());
                self.clamp_selection();
            }
            Err(TryRecvError::Disconnected) => self.loading = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    /// The rows shown: filtered, then sorted.
    fn rows(&self) -> Vec<&TpsRow> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let filter = self.filter.to_lowercase();
        let mut rows: Vec<&TpsRow> = report
            .rows
            .iter()
            .filter(|row| filter.is_empty() || haystack(row).contains(&filter))
            .collect();
        let key = |row: &TpsRow| -> f64 {
            match self.sort {
                SortKey::Time => row.record.timestamp_ms as f64,
                SortKey::Ttft => row.metrics.ttft_ms.unwrap_or(f64::NAN),
                SortKey::Tps => row.metrics.stream_tps.unwrap_or(f64::NAN),
                SortKey::E2e => row.metrics.e2e_tps.unwrap_or(f64::NAN),
                SortKey::Output => row
                    .record
                    .tokens
                    .output_tokens
                    .map_or(f64::NAN, |tokens| tokens as f64),
                SortKey::Model | SortKey::Agent => 0.0,
            }
        };
        rows.sort_by(|a, b| {
            let ordering = match self.sort {
                SortKey::Model => a.record.model.cmp(&b.record.model),
                SortKey::Agent => a.record.agent.as_str().cmp(b.record.agent.as_str()),
                _ => {
                    // Missing figures sort last whichever way the column runs.
                    let (x, y) = (key(a), key(b));
                    match (x.is_nan(), y.is_nan()) {
                        (true, true) => std::cmp::Ordering::Equal,
                        (true, false) => return std::cmp::Ordering::Greater,
                        (false, true) => return std::cmp::Ordering::Less,
                        _ => x.total_cmp(&y),
                    }
                }
            };
            if self.descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
        rows
    }

    fn clamp_selection(&mut self) {
        let count = self.rows().len();
        let selected = self.table.selected().unwrap_or(0);
        self.table
            .select(Some(selected.min(count.saturating_sub(1))));
    }

    fn move_by(&mut self, delta: isize) {
        let count = self.rows().len();
        if count == 0 {
            return;
        }
        let selected = self.table.selected().unwrap_or(0) as isize;
        let next = (selected + delta).clamp(0, count as isize - 1);
        self.table.select(Some(next as usize));
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
            self.clamp_selection();
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
                self.clamp_selection();
            }
            KeyCode::Esc => self.exit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.move_by(10),
            KeyCode::PageUp | KeyCode::Char('b') => self.move_by(-10),
            KeyCode::Home | KeyCode::Char('g') => self.table.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX / 2),
            KeyCode::Enter => self.detail = self.report.is_some() && !self.rows().is_empty(),
            KeyCode::Char('s') | KeyCode::Tab => {
                let index = SortKey::ALL
                    .iter()
                    .position(|k| *k == self.sort)
                    .unwrap_or(0);
                self.sort = SortKey::ALL[(index + 1) % SortKey::ALL.len()];
                self.descending =
                    !matches!(self.sort, SortKey::Model | SortKey::Agent | SortKey::Ttft);
            }
            KeyCode::Char('S') => self.descending = !self.descending,
            KeyCode::Char('/') | KeyCode::Char('f') => {
                self.editing = true;
                self.filter.clear();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => self.step_limit(1),
            KeyCode::Char('-') => self.step_limit(-1),
            KeyCode::Char('u') => {
                self.options.include_unmeasured = !self.options.include_unmeasured;
                self.reload();
            }
            KeyCode::Char('r') => self.reload(),
            _ => {}
        }
    }

    fn step_limit(&mut self, direction: isize) {
        let index = LIMITS
            .iter()
            .position(|limit| *limit >= self.options.limit)
            .unwrap_or(LIMITS.len() - 1) as isize;
        let next = (index + direction).clamp(0, LIMITS.len() as isize - 1) as usize;
        if LIMITS[next] != self.options.limit {
            self.options.limit = LIMITS[next];
            self.reload();
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header, cards, charts, table, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(9),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_header(frame, header);
        self.draw_cards(frame, cards);
        self.draw_charts(frame, charts);
        self.draw_table(frame, table);
        self.draw_footer(frame, footer);
        if self.detail {
            self.draw_detail(frame);
        }
        if self.help {
            draw_help(frame);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let mut subtitle = format!("last {} requests", self.options.limit);
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
        if self.options.include_unmeasured {
            subtitle.push_str(" · with unmeasured");
        }
        let left = Line::from(vec![
            Span::styled(" alc tps ", theme::title()),
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

    fn draw_cards(&self, frame: &mut Frame, area: Rect) {
        let summary = self.report.as_ref().map(|report| &report.summary);
        let shown = self.rows().len();
        let figure = |value: Option<f64>, unit: &str| {
            value.map_or_else(|| "—".to_owned(), |value| format!("{value:.0} {unit}"))
        };
        let cards = [
            (
                "Requests",
                summary.map_or("—".to_owned(), |s| s.records.to_string()),
                if self.filter.is_empty() {
                    self.report
                        .as_ref()
                        .map(|r| format!("{} measured", r.coverage.measured_requests))
                        .unwrap_or_default()
                } else {
                    format!("{shown} match “{}”", self.filter)
                },
                theme::ACCENT,
            ),
            (
                "TTFT p50",
                figure(summary.and_then(|s| s.ttft_p50_ms), "ms"),
                format!("p95 {}", figure(summary.and_then(|s| s.ttft_p95_ms), "ms")),
                Color::Magenta,
            ),
            (
                "TTFT mean",
                figure(summary.and_then(|s| s.ttft_mean_ms), "ms"),
                summary.map_or(String::new(), |s| format!("{} samples", s.ttft_samples)),
                Color::Magenta,
            ),
            (
                "Stream TPS",
                figure(summary.and_then(|s| s.weighted_stream_tps), "tok/s"),
                summary.map_or(String::new(), |s| {
                    format!("weighted · {} samples", s.stream_tps_samples)
                }),
                theme::GOOD,
            ),
            (
                "E2E TPS",
                figure(summary.and_then(|s| s.weighted_e2e_tps), "tok/s"),
                "incl. queue + network".to_owned(),
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

    fn draw_charts(&self, frame: &mut Frame, area: Rect) {
        let [trend, models] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(area);
        let mut chronological: Vec<&TpsRow> = self
            .report
            .as_ref()
            .map(|report| report.rows.iter().collect())
            .unwrap_or_default();
        chronological.sort_by_key(|row| row.record.timestamp_ms);
        let width = trend.width.saturating_sub(2) as usize;
        let tail = chronological.len().saturating_sub(width);
        let raw: Vec<f64> = chronological[tail..]
            .iter()
            .map(|row| row.metrics.stream_tps.unwrap_or(0.0).max(0.0))
            .collect();
        // One cache-burst estimate (200,000 tok/s) would flatten every other
        // bar, so the scale tops out at the 95th percentile.
        let ceiling = percentile(&raw, 0.95).max(1.0);
        let tps: Vec<u64> = raw
            .iter()
            .map(|tps| tps.min(ceiling).round() as u64)
            .collect();
        let peak = ceiling.round() as u64;
        let sparkline = Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(theme::muted())
                    .title(Span::styled(
                        " Stream TPS, oldest → newest ",
                        theme::title(),
                    ))
                    .title_bottom(Span::styled(
                        format!(" scale 0–{peak} tok/s (p95) "),
                        theme::muted(),
                    )),
            )
            .data(&tps)
            .style(Style::default().fg(theme::GOOD));
        frame.render_widget(sparkline, trend);

        let stats = per_model(&self.rows());
        let top = stats
            .iter()
            .filter_map(|stat| stat.median_tps)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        let name_width = stats
            .iter()
            .map(|stat| stat.model.chars().count())
            .max()
            .unwrap_or(5)
            .min(24);
        let bar_width = (models.width as usize)
            .saturating_sub(name_width + 26)
            .max(4);
        let lines: Vec<Line> = stats
            .iter()
            .take(models.height.saturating_sub(2) as usize)
            .map(|stat| {
                let filled = stat
                    .median_tps
                    .map_or(0, |tps| ((tps / top) * bar_width as f64).round() as usize);
                let name: String = stat.model.chars().take(name_width).collect();
                Line::from(vec![
                    Span::raw(format!("{name:<name_width$} ")),
                    Span::styled("█".repeat(filled), Style::default().fg(theme::GOOD)),
                    Span::styled("░".repeat(bar_width.saturating_sub(filled)), theme::muted()),
                    Span::raw(format!(
                        " {:>6} ",
                        stat.median_tps
                            .map_or("—".to_owned(), |tps| format!("{tps:.0}"))
                    )),
                    Span::styled(
                        format!(
                            "{:>7} ×{}",
                            stat.median_ttft
                                .map_or("—".to_owned(), |ttft| format!("{:.1}s", ttft / 1000.0)),
                            stat.requests
                        ),
                        theme::muted(),
                    ),
                ])
            })
            .collect();
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(theme::muted())
                    .title(Span::styled(
                        " Models · median TPS, TTFT, count ",
                        theme::title(),
                    )),
            ),
            models,
        );
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.rows();
        let arrow = if self.descending { "▼" } else { "▲" };
        let heading = |label: &str, key: SortKey| {
            if self.sort == key {
                format!("{label} {arrow}")
            } else {
                label.to_owned()
            }
        };
        let header = Row::new([
            Cell::from(heading("Time", SortKey::Time)),
            Cell::from("Profile"),
            Cell::from(heading("Agent", SortKey::Agent)),
            Cell::from(heading("Model", SortKey::Model)),
            Cell::from("Outcome"),
            right(heading("TTFT ms", SortKey::Ttft)),
            right(heading("TPS", SortKey::Tps)),
            right(heading("E2E", SortKey::E2e)),
            right(heading("Output", SortKey::Output)),
        ])
        .style(
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        );
        let body: Vec<Row> = rows
            .iter()
            .map(|row| {
                let record = &row.record;
                let (outcome, color) = outcome_label(record.outcome);
                Row::new(vec![
                    Cell::from(local_time(record.timestamp_ms)).style(theme::muted()),
                    Cell::from(record.profile.clone().unwrap_or_else(|| "—".to_owned())),
                    Cell::from(record.agent.to_string()),
                    Cell::from(record.model.clone().unwrap_or_else(|| "unknown".to_owned())),
                    Cell::from(outcome).style(Style::default().fg(color)),
                    right(number(row.metrics.ttft_ms, 0)),
                    right(number(row.metrics.stream_tps, 1))
                        .style(Style::default().fg(theme::GOOD)),
                    right(number(row.metrics.e2e_tps, 1)),
                    right(
                        record
                            .tokens
                            .output_tokens
                            .map_or("—".to_owned(), |tokens| tokens.to_string()),
                    ),
                ])
            })
            .collect();
        let empty = body.is_empty();
        let title = if let Some(error) = &self.error {
            format!(" Requests · {error} ")
        } else {
            format!(
                " Requests · {} shown · sorted by {} ",
                rows.len(),
                self.sort.label()
            )
        };
        let table = Table::new(
            body,
            [
                Constraint::Length(14),
                Constraint::Length(14),
                Constraint::Length(8),
                Constraint::Max(32),
                Constraint::Length(10),
                Constraint::Length(9),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(8),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ACCENT))
                .title(Span::styled(title, theme::title())),
        )
        .row_highlight_style(theme::selected())
        .highlight_symbol("▌");
        drop(rows);
        frame.render_stateful_widget(table, area, &mut self.table);
        if empty && self.loading.is_none() {
            let message = if self.filter.is_empty() {
                "no measured requests yet — a bridged agent or `alc --metrics <agent>` records them"
            } else {
                "nothing matches the filter — esc clears it"
            };
            let inner = Rect {
                x: area.x + 2,
                y: area.y + 2,
                width: area.width.saturating_sub(4),
                height: 1,
            };
            frame.render_widget(Paragraph::new(Span::styled(message, theme::muted())), inner);
        }
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        if self.editing {
            let line = Line::from(vec![
                Span::styled(" filter ", theme::key()),
                Span::raw(format!(" {}", self.filter)),
                Span::styled("█", Style::default().fg(theme::ACCENT)),
                Span::styled(
                    "   profile, agent, model or outcome · enter keep · esc clear",
                    theme::muted(),
                ),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        let mut spans = Vec::new();
        for (key, label) in [
            ("↑↓", "select"),
            ("enter", "details"),
            ("s", "sort"),
            ("S", "reverse"),
            ("/", "filter"),
            ("+-", "rows"),
            ("u", "unmeasured"),
            ("r", "reload"),
            ("?", "help"),
            ("q", "quit"),
        ] {
            spans.extend(hint(key, label));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_detail(&self, frame: &mut Frame) {
        let rows = self.rows();
        let Some(row) = self.table.selected().and_then(|index| rows.get(index)) else {
            return;
        };
        let record = &row.record;
        let tokens = &record.tokens;
        let count = |value: Option<u64>| value.map_or("—".to_owned(), |v| v.to_string());
        let micros = |value: Option<u64>| {
            value.map_or("—".to_owned(), |us| {
                format!("{:.0} ms", us as f64 / 1000.0)
            })
        };
        let timing = record.timing.as_ref();
        let mut fields = vec![
            ("time", local_time_full(record.timestamp_ms)),
            ("source", record.source.as_str().to_owned()),
            (
                "profile",
                record.profile.clone().unwrap_or_else(|| "—".to_owned()),
            ),
            (
                "provider",
                record.provider.clone().unwrap_or_else(|| "—".to_owned()),
            ),
            ("agent", record.agent.to_string()),
            (
                "model",
                record.model.clone().unwrap_or_else(|| "unknown".to_owned()),
            ),
            ("outcome", outcome_label(record.outcome).0.to_owned()),
            ("input tokens", count(tokens.input_tokens)),
            ("cache read", count(tokens.cache_read_tokens)),
            ("output tokens", count(tokens.output_tokens)),
            ("reasoning", count(tokens.reasoning_tokens)),
            ("TTFT", number(row.metrics.ttft_ms, 1) + " ms"),
            ("stream TPS", number(row.metrics.stream_tps, 2)),
            (
                "basis",
                match row.metrics.stream_output_basis {
                    OutputBasis::Gross => "gross",
                    OutputBasis::NonReasoning => "non-reasoning",
                    OutputBasis::Unknown => "unknown",
                }
                .to_owned(),
            ),
            ("E2E TPS", number(row.metrics.e2e_tps, 2)),
            (
                "first content",
                micros(timing.and_then(|t| t.first_content_us)),
            ),
            (
                "last content",
                micros(timing.and_then(|t| t.last_content_us)),
            ),
            ("finished", micros(timing.and_then(|t| t.terminal_us))),
            (
                "streaming",
                timing.map_or("—".to_owned(), |t| t.client_streaming.to_string()),
            ),
        ];
        for id in &record.ids {
            fields.push(("id", format!("{} {}", id.protocol, id.id)));
        }
        for warning in &record.warnings {
            fields.push(("warning", warning.clone()));
        }
        let lines: Vec<Line> = fields
            .into_iter()
            .map(|(label, value)| {
                Line::from(vec![
                    Span::styled(format!(" {label:<14}"), Style::default().fg(theme::ACCENT)),
                    Span::raw(value),
                ])
            })
            .collect();
        let area = frame.area();
        let width = 76.min(area.width);
        let height = (lines.len() as u16 + 2).min(area.height);
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme::ACCENT))
                    .title(Span::styled(" Request ", theme::title()))
                    .title_bottom(Span::styled(" esc close ", theme::muted())),
            ),
            popup,
        );
    }
}

fn draw_help(frame: &mut Frame) {
    let rows = [
        ("↑ ↓  j k", "select a request"),
        ("PgUp PgDn  g G", "page, top, bottom"),
        ("enter", "the request in full"),
        ("s  tab", "sort by the next column"),
        ("S", "reverse the order"),
        ("/  f", "filter by profile, agent, model or outcome"),
        ("+  -", "read more or fewer requests"),
        ("u", "include unmeasured history"),
        ("r", "read the history again"),
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
        .collect();
    let area = frame.area();
    let width = 62.min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ACCENT))
                .title(Span::styled(" Keys ", theme::title())),
        ),
        popup,
    );
}

fn haystack(row: &TpsRow) -> String {
    format!(
        "{} {} {} {}",
        row.record.profile.as_deref().unwrap_or_default(),
        row.record.agent,
        row.record.model.as_deref().unwrap_or_default(),
        outcome_label(row.record.outcome).0
    )
    .to_lowercase()
}

fn outcome_label(outcome: Outcome) -> (&'static str, Color) {
    match outcome {
        Outcome::Completed => ("completed", theme::GOOD),
        Outcome::Incomplete => ("incomplete", theme::WARN),
        Outcome::Failed => ("failed", theme::BAD),
        Outcome::Cancelled => ("cancelled", theme::WARN),
        Outcome::TimedOut => ("timed out", theme::BAD),
        Outcome::Truncated => ("truncated", theme::WARN),
        Outcome::Unknown => ("unknown", theme::MUTED),
    }
}

fn right(text: String) -> Cell<'static> {
    Cell::from(Line::from(text).alignment(ratatui::layout::Alignment::Right))
}

fn number(value: Option<f64>, decimals: usize) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value:.decimals$}"))
}

fn local_time(timestamp_ms: u64) -> String {
    chrono::Local
        .timestamp_millis_opt(timestamp_ms as i64)
        .single()
        .map(|at| at.format("%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "—".to_owned())
}

fn local_time_full(timestamp_ms: u64) -> String {
    chrono::Local
        .timestamp_millis_opt(timestamp_ms as i64)
        .single()
        .map(|at| at.format("%Y-%m-%d %H:%M:%S%.3f %Z").to_string())
        .unwrap_or_else(|| "—".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Agent;
    use crate::usage::query::{TpsCoverage, TpsSummary};
    use crate::usage::records::{Metrics, Source, UsageRecord};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn row(ms: u64, model: &str, tps: Option<f64>, ttft: Option<f64>) -> TpsRow {
        let mut record = UsageRecord::new(Source::Alc, Agent::Claude, ms);
        record.model = Some(model.to_owned());
        record.profile = Some("codex".to_owned());
        record.outcome = Outcome::Completed;
        record.tokens.output_tokens = Some(100);
        TpsRow {
            record,
            provenance: Default::default(),
            metrics: Metrics {
                ttft_ms: ttft,
                stream_tps: tps,
                e2e_tps: tps.map(|tps| tps / 2.0),
                stream_output_basis: OutputBasis::NonReasoning,
            },
        }
    }

    fn dashboard() -> Dashboard {
        let mut dashboard = Dashboard::new(
            PathBuf::from("/nonexistent"),
            Config::default(),
            TpsOptions {
                query: Default::default(),
                limit: 200,
                include_unmeasured: false,
                json: false,
                plain: false,
            },
        );
        let rows = vec![
            row(1_000, "gpt-6-astra", Some(50.0), Some(2_000.0)),
            row(3_000, "gemini-3.8-flash", Some(130.0), Some(9_000.0)),
            row(2_000, "gpt-6-astra", Some(70.0), None),
        ];
        dashboard.report = Some(TpsReport {
            schema_version: 1,
            measurement: "test",
            timezone: "UTC".to_owned(),
            range: Default::default(),
            summary: TpsSummary::default(),
            coverage: TpsCoverage::default(),
            rows,
            sources: Vec::new(),
        });
        dashboard
    }

    #[test]
    fn the_median_ignores_one_runaway_estimate() {
        assert_eq!(median(&mut [50.0, 60.0, 207_000.0]), Some(60.0));
        assert_eq!(median(&mut [1.0, 3.0]), Some(2.0));
        assert_eq!(median(&mut []), None);
    }

    #[test]
    fn rows_sort_newest_first_and_by_any_column() {
        let mut dashboard = dashboard();
        let times: Vec<_> = dashboard
            .rows()
            .iter()
            .map(|r| r.record.timestamp_ms)
            .collect();
        assert_eq!(times, [3_000, 2_000, 1_000]);

        dashboard.key(KeyEvent::from(KeyCode::Char('s'))); // model, ascending
        let models: Vec<_> = dashboard
            .rows()
            .iter()
            .map(|r| r.record.model.clone().unwrap())
            .collect();
        assert_eq!(models, ["gemini-3.8-flash", "gpt-6-astra", "gpt-6-astra"]);

        dashboard.sort = SortKey::Ttft;
        dashboard.descending = false;
        let ttft: Vec<_> = dashboard.rows().iter().map(|r| r.metrics.ttft_ms).collect();
        assert_eq!(
            ttft,
            [Some(2_000.0), Some(9_000.0), None],
            "missing sorts last"
        );
    }

    #[test]
    fn the_filter_narrows_rows_and_models_group_them() {
        let mut dashboard = dashboard();
        dashboard.key(KeyEvent::from(KeyCode::Char('/')));
        for ch in "astra".chars() {
            dashboard.key(KeyEvent::from(KeyCode::Char(ch)));
        }
        dashboard.key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(dashboard.rows().len(), 2);
        let stats = per_model(&dashboard.rows());
        assert_eq!(
            stats,
            [ModelStats {
                model: "gpt-6-astra".to_owned(),
                requests: 2,
                median_tps: Some(60.0),
                median_ttft: Some(2_000.0),
            }]
        );
        dashboard.key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(dashboard.rows().len(), 3);
    }

    #[test]
    fn the_dashboard_draws_cards_models_and_rows_and_opens_a_request() {
        let mut dashboard = dashboard();
        let mut terminal = Terminal::new(TestBackend::new(140, 36)).unwrap();
        terminal.draw(|frame| dashboard.draw(frame)).unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        for expected in [
            "alc tps",
            "Stream TPS",
            "Models",
            "gemini-3.8-flash",
            "Requests",
        ] {
            assert!(screen.contains(expected), "{expected} missing:\n{screen}");
        }
        dashboard.key(KeyEvent::from(KeyCode::Enter));
        assert!(dashboard.detail);
        terminal.draw(|frame| dashboard.draw(frame)).unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("output tokens"), "{screen}");
        dashboard.key(KeyEvent::from(KeyCode::Esc));
        assert!(!dashboard.detail);
    }
}
