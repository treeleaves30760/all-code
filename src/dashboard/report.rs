//! A full-screen reader for alc's sectioned reports.
//!
//! A report is loaded by one or more *slots*, each a function that prints
//! (through [`super::emit`]) on a thread of its own, so a slow check - a
//! quota lookup, a server probe - fills in while the rest is already
//! readable. The text is split into sections at its headings and listed on
//! the left, each marked by the worst status it reports.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};

use super::{ansi, hint, spinner, theme};

/// Prints one part of a report and answers its exit code.
pub(crate) type Loader = Arc<dyn Fn() -> Result<u8> + Send + Sync>;

/// What a command adds to the viewer: its own keys, and the loaders they
/// change.
pub(crate) trait Controls {
    /// The loaders, one per slot, as the controls now stand.
    fn loaders(&self) -> Vec<(String, Loader)>;
    /// Shown beside the title: the window, the filters.
    fn subtitle(&self) -> String {
        String::new()
    }
    /// Extra footer hints.
    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }
    /// A key the viewer does not use. Answers the slots to reload, if any.
    fn key(&mut self, _key: KeyEvent) -> Option<Vec<usize>> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Status {
    Info,
    Good,
    Warn,
    Bad,
}

impl Status {
    fn glyph(self) -> (&'static str, Style) {
        match self {
            Self::Info => ("•", theme::muted()),
            Self::Good => ("✓", Style::default().fg(theme::GOOD)),
            Self::Warn => ("!", Style::default().fg(theme::WARN)),
            Self::Bad => ("✗", Style::default().fg(theme::BAD)),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Section {
    pub title: String,
    pub raw: Vec<String>,
    pub plain: Vec<String>,
    pub status: Status,
    /// The slot is still loading this section.
    pub loading: bool,
}

/// Splits report text at its headings: an unindented line, bold where the
/// terminal has colour. An unindented line that opens with a status mark is
/// the closing summary, and starts a section of its own.
pub(crate) fn sections(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for raw in text.lines() {
        let plain = ansi::strip(raw);
        let unindented = !plain.is_empty() && !plain.starts_with(' ');
        if unindented {
            let summary = ["✓", "✗", "!", "+ ", "x "]
                .iter()
                .any(|mark| plain.starts_with(mark));
            let title = if summary {
                "Summary".to_owned()
            } else {
                plain.trim().to_owned()
            };
            out.push(Section {
                title,
                raw: Vec::new(),
                plain: Vec::new(),
                status: Status::Info,
                loading: false,
            });
            if !summary {
                continue;
            }
        }
        if out.is_empty() {
            out.push(Section {
                title: "Report".to_owned(),
                raw: Vec::new(),
                plain: Vec::new(),
                status: Status::Info,
                loading: false,
            });
        }
        let section = out.last_mut().expect("a section was just pushed");
        section.raw.push(raw.to_owned());
        section.plain.push(plain);
    }
    for section in &mut out {
        while section
            .plain
            .first()
            .is_some_and(|line| line.trim().is_empty())
        {
            section.plain.remove(0);
            section.raw.remove(0);
        }
        while section
            .plain
            .last()
            .is_some_and(|line| line.trim().is_empty())
        {
            section.plain.pop();
            section.raw.pop();
        }
        section.status = status_of(&section.plain);
    }
    out.retain(|section| !section.plain.is_empty());
    out
}

/// The worst mark a section's lines carry.
pub(crate) fn status_of(lines: &[String]) -> Status {
    lines.iter().fold(Status::Info, |worst, line| {
        let trimmed = line.trim_start();
        let status = if line.contains('✗') {
            Status::Bad
        } else if trimmed.starts_with("! ") || trimmed.starts_with("!  ") {
            Status::Warn
        } else if line.contains('✓') {
            Status::Good
        } else {
            Status::Info
        };
        worst.max(status)
    })
}

struct Slot {
    name: String,
    text: String,
    code: Option<u8>,
    error: Option<String>,
    loading: bool,
    generation: u64,
    text_rx: Option<Receiver<String>>,
    done_rx: Option<Receiver<Result<u8, String>>>,
}

impl Slot {
    fn start(&mut self, loader: Loader) {
        self.generation += 1;
        self.loading = true;
        self.text.clear();
        self.error = None;
        let (text_tx, text_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        self.text_rx = Some(text_rx);
        self.done_rx = Some(done_rx);
        thread::spawn(move || {
            let _capture = super::capture(text_tx);
            let result = loader().map_err(|error| format!("{error:#}"));
            let _ = done_tx.send(result);
        });
    }

    /// Takes what arrived. True when anything did.
    fn pump(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.text_rx {
            while let Ok(chunk) = rx.try_recv() {
                self.text.push_str(&chunk);
                changed = true;
            }
        }
        if let Some(rx) = &self.done_rx {
            match rx.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(code) => self.code = Some(code),
                        Err(error) => {
                            self.code = Some(1);
                            self.error = Some(error);
                        }
                    }
                    self.loading = false;
                    self.done_rx = None;
                    changed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.loading = false;
                    self.done_rx = None;
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        changed
    }
}

#[derive(Default)]
struct Search {
    editing: bool,
    query: String,
}

pub(crate) struct Viewer<C: Controls> {
    title: String,
    controls: C,
    slots: Vec<Slot>,
    sections: Vec<Section>,
    selected: usize,
    scroll: u16,
    pan: u16,
    all: bool,
    wrap: bool,
    search: Search,
    help: bool,
    tick: usize,
    refreshed: Option<chrono::DateTime<chrono::Local>>,
    exit: bool,
    content_height: u16,
    /// The user has moved between sections; the view stops choosing for them.
    navigated: bool,
}

impl<C: Controls> Viewer<C> {
    pub(crate) fn new(title: &str, controls: C) -> Self {
        let slots = controls
            .loaders()
            .into_iter()
            .map(|(name, _)| Slot {
                name,
                text: String::new(),
                code: None,
                error: None,
                loading: false,
                generation: 0,
                text_rx: None,
                done_rx: None,
            })
            .collect();
        Self {
            title: title.to_owned(),
            controls,
            slots,
            sections: Vec::new(),
            selected: 0,
            scroll: 0,
            pan: 0,
            all: false,
            wrap: false,
            search: Search::default(),
            help: false,
            tick: 0,
            refreshed: None,
            exit: false,
            content_height: 0,
            navigated: false,
        }
    }

    /// Runs until the user leaves; answers the report's exit code.
    pub(crate) fn run(mut self) -> Result<u8> {
        let mut screen = super::Screen::open()?;
        self.reload(None);
        let mut last_tick = Instant::now();
        loop {
            if self.pump() {
                self.rebuild();
            }
            screen.terminal.draw(|frame| self.draw(frame))?;
            if self.exit {
                break;
            }
            let timeout = Duration::from_millis(100).saturating_sub(last_tick.elapsed());
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.key(key),
                    _ => {}
                }
            }
            if last_tick.elapsed() >= Duration::from_millis(100) {
                self.tick = self.tick.wrapping_add(1);
                last_tick = Instant::now();
            }
        }
        drop(screen);
        Ok(self.code())
    }

    fn code(&self) -> u8 {
        self.slots
            .iter()
            .filter_map(|slot| slot.code)
            .max()
            .unwrap_or(0)
    }

    fn reload(&mut self, which: Option<Vec<usize>>) {
        let loaders = self.controls.loaders();
        for (index, (name, loader)) in loaders.into_iter().enumerate() {
            if which.as_ref().is_some_and(|which| !which.contains(&index)) {
                continue;
            }
            if let Some(slot) = self.slots.get_mut(index) {
                slot.name = name;
                slot.start(loader);
            }
        }
        self.refreshed = Some(chrono::Local::now());
        self.rebuild();
    }

    fn pump(&mut self) -> bool {
        let mut changed = false;
        for slot in &mut self.slots {
            changed |= slot.pump();
        }
        changed
    }

    fn loading(&self) -> bool {
        self.slots.iter().any(|slot| slot.loading)
    }

    fn rebuild(&mut self) {
        let keep = self.sections.get(self.selected).map(|s| s.title.clone());
        let mut sections = Vec::new();
        for slot in &self.slots {
            let mut parsed = sections_of_slot(slot);
            if slot.loading {
                match parsed.last_mut() {
                    Some(last) => last.loading = true,
                    None => parsed.push(Section {
                        title: slot.name.clone(),
                        raw: vec![format!("  {}", "loading…")],
                        plain: vec!["  loading…".to_owned()],
                        status: Status::Info,
                        loading: true,
                    }),
                }
            }
            sections.extend(parsed);
        }
        // The verdict, and what to run about it, comes first.
        if let Some(at) = sections.iter().position(|s| s.title == "Summary") {
            let summary = sections.remove(at);
            sections.insert(0, summary);
        }
        self.sections = sections;
        if !self.navigated
            && !self.loading()
            && self.sections.first().is_some_and(|s| s.title == "Summary")
        {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        if let Some(title) = keep
            && let Some(index) = self.sections.iter().position(|s| s.title == title)
        {
            self.selected = index;
        }
        self.selected = self.selected.min(self.sections.len().saturating_sub(1));
    }

    fn select(&mut self, index: usize) {
        self.navigated = true;
        if index != self.selected {
            self.selected = index;
            self.scroll = 0;
            self.pan = 0;
        }
    }

    /// The lines the content pane shows, with their section index.
    fn visible(&self) -> Vec<(usize, &str, &str)> {
        let mut out = Vec::new();
        for (index, section) in self.sections.iter().enumerate() {
            if !self.all && index != self.selected {
                continue;
            }
            if self.all {
                if !out.is_empty() {
                    out.push((index, "", ""));
                }
                out.push((index, "\u{1}", section.title.as_str()));
            }
            for (raw, plain) in section.raw.iter().zip(&section.plain) {
                out.push((index, raw.as_str(), plain.as_str()));
            }
        }
        out
    }

    fn matches(&self) -> Vec<(usize, usize)> {
        let query = self.search.query.to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        self.sections
            .iter()
            .enumerate()
            .flat_map(|(section, s)| {
                s.plain
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| line.to_lowercase().contains(&query))
                    .map(move |(line, _)| (section, line))
            })
            .collect()
    }

    /// Moves to the next (or previous) search hit after the current view.
    fn jump(&mut self, forward: bool) {
        let hits = self.matches();
        if hits.is_empty() {
            return;
        }
        let here = (self.selected, self.scroll as usize);
        let next = if forward {
            hits.iter()
                .find(|hit| **hit > here)
                .or_else(|| hits.first())
        } else {
            hits.iter()
                .rev()
                .find(|hit| **hit < here)
                .or_else(|| hits.last())
        };
        if let Some(&(section, line)) = next {
            if self.all {
                let offset = self
                    .visible()
                    .iter()
                    .position(|(index, raw, _)| *index == section && *raw != "\u{1}")
                    .unwrap_or(0);
                self.scroll = (offset + line) as u16;
            } else {
                self.select(section);
                self.scroll = line as u16;
            }
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.exit = true;
            return;
        }
        if self.search.editing {
            match key.code {
                KeyCode::Enter => {
                    self.search.editing = false;
                    self.jump(true);
                }
                KeyCode::Esc => {
                    self.search.editing = false;
                    self.search.query.clear();
                }
                KeyCode::Backspace => {
                    self.search.query.pop();
                }
                KeyCode::Char(ch) => self.search.query.push(ch),
                _ => {}
            }
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        let page = self.content_height.saturating_sub(2).max(1);
        match key.code {
            KeyCode::Char('q') => self.exit = true,
            KeyCode::Esc if !self.search.query.is_empty() => self.search.query.clear(),
            KeyCode::Esc => self.exit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.scroll = self.scroll.saturating_add(page)
            }
            KeyCode::PageUp | KeyCode::Char('b') => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll = u16::MAX,
            KeyCode::Right | KeyCode::Char('l') => self.pan = self.pan.saturating_add(8),
            KeyCode::Left | KeyCode::Char('h') => self.pan = self.pan.saturating_sub(8),
            KeyCode::Tab | KeyCode::Char(']') | KeyCode::Char('J') => {
                if !self.sections.is_empty() {
                    self.select((self.selected + 1) % self.sections.len());
                }
            }
            KeyCode::BackTab | KeyCode::Char('[') | KeyCode::Char('K') => {
                if !self.sections.is_empty() {
                    let count = self.sections.len();
                    self.select((self.selected + count - 1) % count);
                }
            }
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index < self.sections.len() {
                    self.all = false;
                    self.select(index);
                }
            }
            KeyCode::Char('a') => {
                self.all = !self.all;
                self.scroll = 0;
            }
            KeyCode::Char('w') => self.wrap = !self.wrap,
            KeyCode::Char('/') => {
                self.search.editing = true;
                self.search.query.clear();
            }
            KeyCode::Char('n') => self.jump(true),
            KeyCode::Char('N') => self.jump(false),
            KeyCode::Char('r') => self.reload(None),
            _ => {
                if let Some(slots) = self.controls.key(key) {
                    self.scroll = 0;
                    self.reload(Some(slots));
                }
            }
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_header(frame, header);

        let sidebar_width = self
            .sections
            .iter()
            .map(|section| section.title.chars().count() as u16 + 7)
            .max()
            .unwrap_or(20)
            .clamp(18, 34);
        let narrow = body.width < 70;
        let [sidebar, content] = if narrow {
            [Rect::default(), body]
        } else {
            Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(20)]).areas(body)
        };
        if !narrow {
            self.draw_sidebar(frame, sidebar);
        }
        self.draw_content(frame, content);
        self.draw_footer(frame, footer);
        if self.help {
            self.draw_help(frame);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let mut left = vec![Span::styled(format!(" {} ", self.title), theme::title())];
        let subtitle = self.controls.subtitle();
        if !subtitle.is_empty() {
            left.push(Span::styled(subtitle, theme::muted()));
        }
        let counts = |status| {
            self.sections
                .iter()
                .filter(|section| section.status == status)
                .count()
        };
        let mut right = Vec::new();
        if self.loading() {
            right.push(Span::styled(
                format!("{} checking  ", spinner(self.tick)),
                Style::default().fg(theme::ACCENT),
            ));
        }
        for status in [Status::Good, Status::Warn, Status::Bad] {
            let count = counts(status);
            if count > 0 {
                let (glyph, style) = status.glyph();
                right.push(Span::styled(format!("{glyph} {count}  "), style));
            }
        }
        if let Some(at) = self.refreshed {
            right.push(Span::styled(
                at.format("%H:%M:%S ").to_string(),
                theme::muted(),
            ));
        }
        let right_width: usize = right.iter().map(|span| span.content.chars().count()).sum();
        let [left_area, right_area] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(right_width as u16)])
                .areas(area);
        frame.render_widget(Paragraph::new(Line::from(left)), left_area);
        frame.render_widget(Paragraph::new(Line::from(right)), right_area);
    }

    fn draw_sidebar(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .sections
            .iter()
            .enumerate()
            .map(|(index, section)| {
                let (glyph, style) = if section.loading {
                    (spinner(self.tick), Style::default().fg(theme::ACCENT))
                } else {
                    section.status.glyph()
                };
                let number = if index < 9 {
                    format!("{} ", index + 1)
                } else {
                    "  ".to_owned()
                };
                ListItem::new(Line::from(vec![
                    Span::styled(number, theme::muted()),
                    Span::styled(glyph, style),
                    Span::raw(" "),
                    Span::raw(section.title.clone()),
                ]))
            })
            .collect();
        let mut state = ListState::default();
        if !self.all {
            state.select(Some(self.selected));
        }
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(theme::muted())
                    .title(Span::styled(" Sections ", theme::title())),
            )
            .highlight_style(theme::selected())
            .highlight_symbol("▌");
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_content(&mut self, frame: &mut Frame, area: Rect) {
        let query = self.search.query.to_lowercase();
        let lines: Vec<Line> = self
            .visible()
            .into_iter()
            .map(|(_, raw, plain)| {
                if raw == "\u{1}" {
                    return Line::from(Span::styled(
                        plain.to_owned(),
                        theme::title().add_modifier(Modifier::UNDERLINED),
                    ));
                }
                let line = ansi::line(raw);
                if !query.is_empty() && plain.to_lowercase().contains(&query) {
                    highlight(line, &query)
                } else {
                    line
                }
            })
            .collect();
        let total = lines.len() as u16;
        let inner_height = area.height.saturating_sub(2);
        self.content_height = inner_height;
        let max_scroll = total.saturating_sub(inner_height);
        if !self.wrap {
            self.scroll = self.scroll.min(max_scroll);
        }
        let title = if self.all {
            " All sections ".to_owned()
        } else {
            self.sections
                .get(self.selected)
                .map(|section| format!(" {} ", section.title))
                .unwrap_or_else(|| " … ".to_owned())
        };
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::ACCENT))
            .title(Span::styled(title, theme::title()));
        if total > inner_height && !self.wrap {
            block = block.title_bottom(Span::styled(
                format!(" {}/{} ", (self.scroll + inner_height).min(total), total),
                theme::muted(),
            ));
        }
        if self.pan > 0 {
            block = block.title_bottom(Span::styled(format!(" ← {} ", self.pan), theme::muted()));
        }
        let mut paragraph = Paragraph::new(lines).block(block);
        if self.wrap {
            paragraph = paragraph
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0));
        } else {
            paragraph = paragraph.scroll((self.scroll, self.pan));
        }
        frame.render_widget(paragraph, area);
        if total > inner_height && !self.wrap {
            let mut state = ScrollbarState::new(max_scroll as usize).position(self.scroll as usize);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight),
                area.inner(Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut state,
            );
        }
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        if self.search.editing {
            let line = Line::from(vec![
                Span::styled(" / ", theme::key()),
                Span::raw(format!(" {}", self.search.query)),
                Span::styled("█", Style::default().fg(theme::ACCENT)),
                Span::styled("   enter find · esc cancel", theme::muted()),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        let mut spans = Vec::new();
        // First, so a narrow terminal cuts the hints rather than the hits.
        if !self.search.query.is_empty() {
            let hits = self.matches().len();
            spans.push(Span::styled(
                format!(" “{}” {hits} hits · n/N  ", self.search.query),
                Style::default().fg(theme::WARN),
            ));
        }
        for (key, label) in [
            ("↑↓", "scroll"),
            ("tab", "section"),
            ("←→", "pan"),
            ("/", "search"),
            ("a", "all"),
            ("r", "refresh"),
        ] {
            spans.extend(hint(key, label));
        }
        for (key, label) in self.controls.hints() {
            spans.extend(hint(key, label));
        }
        spans.extend(hint("?", "help"));
        spans.extend(hint("q", "quit"));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_help(&self, frame: &mut Frame) {
        let mut rows = vec![
            ("↑ ↓  j k", "scroll the section"),
            ("PgUp PgDn  b space", "scroll a page"),
            ("g G  Home End", "top, bottom"),
            ("← →  h l", "pan wide tables"),
            ("tab  shift-tab  ] [", "next, previous section"),
            ("1-9", "jump to a section"),
            ("a", "every section in one page"),
            ("w", "wrap long lines"),
            ("/  n N", "search, next and previous hit"),
            ("r", "run the report again"),
        ];
        rows.extend(self.controls.hints());
        rows.push(("q  esc", "leave"));
        let lines: Vec<Line> = rows
            .iter()
            .map(|(key, label)| {
                Line::from(vec![
                    Span::styled(format!(" {key:<22}"), Style::default().fg(theme::ACCENT)),
                    Span::raw(*label),
                ])
            })
            .collect();
        let height = lines.len() as u16 + 2;
        let area = frame.area();
        let width = 60.min(area.width);
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height: height.min(area.height),
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
}

fn sections_of_slot(slot: &Slot) -> Vec<Section> {
    let mut parsed = sections(&slot.text);
    if let Some(error) = &slot.error {
        let line = format!("  ✗ {error}");
        parsed.push(Section {
            title: format!("{} failed", slot.name),
            raw: vec![line.clone()],
            plain: vec![line],
            status: Status::Bad,
            loading: false,
        });
    }
    parsed
}

/// Marks every occurrence of `query` (already lowercase) in `line`.
fn highlight(line: Line<'static>, query: &str) -> Line<'static> {
    let mut out = Vec::new();
    for span in line.spans {
        let text = span.content.to_string();
        let lower = text.to_lowercase();
        // Lowercasing can change byte lengths; fall back to the whole span.
        if lower.len() != text.len() {
            if lower.contains(query) {
                out.push(Span::styled(text, theme::highlight()));
            } else {
                out.push(Span::styled(text, span.style));
            }
            continue;
        }
        let mut start = 0;
        while let Some(found) = lower[start..].find(query) {
            let at = start + found;
            if at > start {
                out.push(Span::styled(text[start..at].to_owned(), span.style));
            }
            out.push(Span::styled(
                text[at..at + query.len()].to_owned(),
                theme::highlight(),
            ));
            start = at + query.len();
        }
        if start < text.len() {
            out.push(Span::styled(text[start..].to_owned(), span.style));
        }
    }
    Line::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const DOCTOR: &str = "\x1b[1malc doctor\x1b[0m\n\n\x1b[1mEnvironment\x1b[0m\n  Config  /x/config.toml\n  Validation  \x1b[32m✓\x1b[0m ok\n\n\x1b[1mAgent binaries\x1b[0m\n  \x1b[32m✓\x1b[0m  claude  /bin/claude\n  \x1b[33m!\x1b[0m  pi  not found (pi)\n\n\x1b[31m✗\x1b[0m \x1b[31mneeds attention — 1 issue\x1b[0m\n  • anthropic  expired  →  claude\n";

    #[test]
    fn a_report_splits_at_its_headings_and_keeps_the_summary_apart() {
        let parsed = sections(DOCTOR);
        let titles: Vec<_> = parsed.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Environment", "Agent binaries", "Summary"]);
        assert_eq!(parsed[0].status, Status::Good);
        assert_eq!(parsed[1].status, Status::Warn);
        assert_eq!(parsed[2].status, Status::Bad);
        assert_eq!(parsed[2].plain[1], "  • anthropic  expired  →  claude");
    }

    #[test]
    fn plain_headings_without_colour_split_too() {
        let parsed = sections("Accounts\n  ✓  codex  fine\n\nToken usage\n  none\n");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].title, "Token usage");
        assert_eq!(parsed[1].status, Status::Info);
    }

    #[test]
    fn a_search_hit_is_marked_inside_its_span() {
        let marked = highlight(ansi::line("  \x1b[32mclaude\x1b[0m runs"), "aud");
        let texts: Vec<_> = marked.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(texts, ["  ", "cl", "aud", "e", " runs"]);
        assert_eq!(marked.spans[2].style, theme::highlight());
    }

    struct Fixed(Vec<(String, Loader)>);

    impl Controls for Fixed {
        fn loaders(&self) -> Vec<(String, Loader)> {
            self.0.clone()
        }
    }

    fn rendered(viewer: &mut Viewer<Fixed>, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| viewer.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn loaded(text: &'static str) -> Viewer<Fixed> {
        let loader: Loader = Arc::new(move || {
            super::super::emit(text);
            Ok(0)
        });
        let mut viewer = Viewer::new("alc doctor", Fixed(vec![("Doctor".to_owned(), loader)]));
        viewer.reload(None);
        let deadline = Instant::now() + Duration::from_secs(5);
        while viewer.loading() && Instant::now() < deadline {
            viewer.pump();
            thread::sleep(Duration::from_millis(5));
        }
        viewer.pump();
        viewer.rebuild();
        viewer
    }

    #[test]
    fn the_view_lists_sections_and_shows_the_selected_one() {
        let mut viewer = loaded(DOCTOR);
        let screen = rendered(&mut viewer, 100, 20);
        assert!(screen.contains("alc doctor"), "{screen}");
        assert!(screen.contains("1 ✗ Summary"), "{screen}");
        assert!(screen.contains("2 ✓ Environment"), "{screen}");
        assert!(screen.contains("3 ! Agent binaries"), "{screen}");
        assert!(screen.contains("needs attention"), "{screen}");
        assert!(!screen.contains("Validation"), "{screen}");

        viewer.key(KeyEvent::from(KeyCode::Tab));
        let screen = rendered(&mut viewer, 100, 20);
        assert!(screen.contains("Validation"), "{screen}");
        viewer.key(KeyEvent::from(KeyCode::Tab));
        let screen = rendered(&mut viewer, 100, 20);
        assert!(screen.contains("not found (pi)"), "{screen}");

        viewer.key(KeyEvent::from(KeyCode::Char('a')));
        let screen = rendered(&mut viewer, 100, 20);
        assert!(screen.contains("Validation") && screen.contains("needs attention"));
    }

    #[test]
    fn searching_jumps_to_the_section_that_matches() {
        let mut viewer = loaded(DOCTOR);
        viewer.key(KeyEvent::from(KeyCode::Char('/')));
        for ch in "expired".chars() {
            viewer.key(KeyEvent::from(KeyCode::Char(ch)));
        }
        viewer.key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(viewer.sections[viewer.selected].title, "Summary");
        let screen = rendered(&mut viewer, 100, 20);
        assert!(screen.contains("1 hits"), "{screen}");
        viewer.key(KeyEvent::from(KeyCode::Char('q')));
        assert!(viewer.exit);
    }

    #[test]
    fn a_failed_loader_reads_as_a_failed_section_and_a_failing_code() {
        let loader: Loader = Arc::new(|| anyhow::bail!("no network"));
        let mut viewer = Viewer::new("alc usage", Fixed(vec![("Accounts".to_owned(), loader)]));
        viewer.reload(None);
        let deadline = Instant::now() + Duration::from_secs(5);
        while viewer.loading() && Instant::now() < deadline {
            viewer.pump();
            thread::sleep(Duration::from_millis(5));
        }
        viewer.rebuild();
        assert_eq!(viewer.sections[0].title, "Accounts failed");
        assert_eq!(viewer.code(), 1);
    }
}
