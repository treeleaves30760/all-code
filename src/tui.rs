use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
};

use crate::config::{
    Agent, AuthStyle, Config, Protocol, Provider, ProviderKind, ReasoningEffort, Store,
    validate_profile_name,
};
use crate::model_catalog::ModelCatalog;
use crate::model_picker::{PickerApp, PickerRequest};

type Backend = CrosstermBackend<Stdout>;

pub fn run(store: &mut Store) -> Result<()> {
    // Refresh before taking over the screen so the model chooser is current.
    let catalog = ModelCatalog::load_and_refresh_if_due(&store.dir);
    let mut terminal = setup_terminal()?;
    let _cleanup = TerminalCleanup;
    let mut app = App::new(store.clone(), catalog);

    loop {
        terminal.draw(|frame| draw(frame, &mut app))?;
        if app.exit {
            break;
        }
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == event::KeyEventKind::Press
        {
            app.handle_key(key);
        }
    }

    if !app.cancelled {
        *store = app.store;
    }
    Ok(())
}

fn setup_terminal() -> Result<Terminal<Backend>> {
    enable_raw_mode()?;
    let result = (|| {
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        terminal.clear()?;
        Ok(terminal)
    })();
    if result.is_err() {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
    result
}

struct TerminalCleanup;

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Providers,
    Defaults,
    Edit,
    Model,
    ConfirmDelete,
}

struct App {
    store: Store,
    catalog: ModelCatalog,
    picker: Option<PickerApp>,
    screen: Screen,
    provider_selected: usize,
    default_selected: usize,
    edit: Option<EditForm>,
    status: String,
    status_error: bool,
    dirty: bool,
    exit: bool,
    cancelled: bool,
}

impl App {
    fn new(store: Store, catalog: ModelCatalog) -> Self {
        Self {
            store,
            catalog,
            picker: None,
            screen: Screen::Providers,
            provider_selected: 0,
            default_selected: 0,
            edit: None,
            status: "Ready. Changes are saved with s or q.".to_owned(),
            status_error: false,
            dirty: false,
            exit: false,
            cancelled: false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.cancelled = true;
            self.exit = true;
            return;
        }
        match self.screen {
            Screen::Providers => self.handle_providers(key),
            Screen::Defaults => self.handle_defaults(key),
            Screen::Edit => self.handle_edit(key),
            Screen::Model => self.handle_model_picker(key),
            Screen::ConfirmDelete => self.handle_delete_confirmation(key),
        }
    }

    fn handle_providers(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_provider(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_provider(1),
            KeyCode::Char('a') => {
                self.edit = Some(EditForm::new(self.next_profile_name()));
                self.screen = Screen::Edit;
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                if let Some(name) = self.selected_provider_name() {
                    let provider = self.store.config.providers[&name].clone();
                    let has_key = self.store.credentials.api_keys.contains_key(&name);
                    self.edit = Some(EditForm::existing(name, provider, has_key));
                    self.screen = Screen::Edit;
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if self.selected_provider_name().is_some() {
                    self.screen = Screen::ConfirmDelete;
                }
            }
            KeyCode::Tab => self.screen = Screen::Defaults,
            KeyCode::Char('s') => self.save(false),
            KeyCode::Char('q') => self.save(true),
            _ => {}
        }
    }

    fn handle_defaults(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.default_selected = self.default_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.default_selected = (self.default_selected + 1).min(Agent::ALL.len() - 1)
            }
            KeyCode::Left | KeyCode::Char('h') => self.cycle_default(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => self.cycle_default(1),
            KeyCode::Tab | KeyCode::Esc => self.screen = Screen::Providers,
            KeyCode::Char('s') => self.save(false),
            KeyCode::Char('q') => self.save(true),
            _ => {}
        }
    }

    fn handle_edit(&mut self, key: KeyEvent) {
        let Some(form) = self.edit.as_mut() else {
            self.screen = Screen::Providers;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.edit = None;
                self.screen = Screen::Providers;
                self.set_status("Edit cancelled.", false);
            }
            KeyCode::Enter => self.apply_edit(),
            KeyCode::Tab | KeyCode::Down => form.move_field(1),
            KeyCode::BackTab | KeyCode::Up => form.move_field(-1),
            KeyCode::Left | KeyCode::Right if form.browses_models() => self.open_model_picker(),
            KeyCode::Left => form.cycle(-1),
            KeyCode::Right => form.cycle(1),
            KeyCode::Backspace => form.backspace(),
            KeyCode::Delete => form.clear_current(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                form.clear_current()
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                form.push(character)
            }
            _ => {}
        }
    }

    fn open_model_picker(&mut self) {
        let Some(form) = self.edit.as_ref() else {
            return;
        };
        let effort = form
            .provider
            .reasoning_effort
            .unwrap_or(ReasoningEffort::Medium);
        self.picker = Some(PickerApp::new(
            self.catalog.clone(),
            PickerRequest {
                model: form.provider.model.clone(),
                effort,
                choose_model: true,
                choose_effort: true,
            },
        ));
        self.screen = Screen::Model;
    }

    fn handle_model_picker(&mut self, key: KeyEvent) {
        let Some(picker) = self.picker.as_mut() else {
            self.screen = Screen::Edit;
            return;
        };
        picker.handle_key(key);
        let Some(result) = picker.take_result() else {
            return;
        };
        if let Some(selection) = result
            && let Some(form) = self.edit.as_mut()
        {
            form.provider.model = selection.model.clone();
            form.provider.reasoning_effort = Some(selection.effort);
            self.set_status(
                format!("Default model: {} / {}", selection.model, selection.effort),
                false,
            );
        }
        self.picker = None;
        self.screen = Screen::Edit;
    }

    fn handle_delete_confirmation(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => self.delete_selected(),
            KeyCode::Char('n') | KeyCode::Esc => {
                self.screen = Screen::Providers;
                self.set_status("Delete cancelled.", false);
            }
            _ => {}
        }
    }

    fn move_provider(&mut self, delta: isize) {
        let len = self.store.config.providers.len();
        if len == 0 {
            self.provider_selected = 0;
            return;
        }
        self.provider_selected = self
            .provider_selected
            .saturating_add_signed(delta)
            .min(len - 1);
    }

    fn selected_provider_name(&self) -> Option<String> {
        self.store
            .config
            .providers
            .keys()
            .nth(self.provider_selected)
            .cloned()
    }

    fn next_profile_name(&self) -> String {
        for index in 1.. {
            let name = format!("provider-{index}");
            if !self.store.config.providers.contains_key(&name) {
                return name;
            }
        }
        unreachable!()
    }

    fn cycle_default(&mut self, delta: isize) {
        let agent = Agent::ALL[self.default_selected];
        let compatible: Vec<_> = self
            .store
            .config
            .providers
            .iter()
            .filter(|(_, provider)| provider.supports(agent))
            .map(|(name, _)| name.clone())
            .collect();
        if compatible.is_empty() {
            self.set_status(format!("No compatible provider exists for {agent}."), true);
            return;
        }
        let current = self.store.config.defaults.get(agent);
        let index = compatible
            .iter()
            .position(|name| name == current)
            .unwrap_or(0);
        let next = (index as isize + delta).rem_euclid(compatible.len() as isize) as usize;
        self.store.config.defaults.set(agent, &compatible[next]);
        self.dirty = true;
        self.set_status(
            format!("Default {agent} provider: {}", compatible[next]),
            false,
        );
    }

    fn apply_edit(&mut self) {
        let Some(form) = self.edit.clone() else {
            return;
        };
        match form.validate(&self.store.config) {
            Ok(()) => {
                let original = form.original_name.clone();
                let name = form.name.clone();
                let mut candidate = self.store.clone();
                if let Some(old) = &original
                    && old != &name
                {
                    candidate.config.providers.remove(old);
                    candidate.move_key(old, &name);
                    for agent in Agent::ALL {
                        if candidate.config.defaults.get(agent) == old {
                            candidate.config.defaults.set(agent, &name);
                        }
                    }
                }
                candidate
                    .config
                    .providers
                    .insert(name.clone(), form.provider.clone());
                if form.secret_touched {
                    candidate.set_key(&name, form.secret.clone());
                }
                if let Err(error) = candidate.config.validate() {
                    self.set_status(error.to_string(), true);
                    self.edit = Some(form);
                    return;
                }
                self.store = candidate;
                self.provider_selected = self
                    .store
                    .config
                    .providers
                    .keys()
                    .position(|candidate| candidate == &name)
                    .unwrap_or(0);
                self.edit = None;
                self.screen = Screen::Providers;
                self.dirty = true;
                self.set_status(format!("Updated provider '{name}'."), false);
            }
            Err(error) => self.set_status(error.to_string(), true),
        }
    }

    fn delete_selected(&mut self) {
        let Some(name) = self.selected_provider_name() else {
            self.screen = Screen::Providers;
            return;
        };
        let defaults: Vec<_> = Agent::ALL
            .into_iter()
            .filter(|agent| {
                self.store.config.defaults.is_explicit(*agent)
                    && self.store.config.defaults.get(*agent) == name
            })
            .collect();
        if !defaults.is_empty() {
            let list = defaults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            self.screen = Screen::Providers;
            self.set_status(
                format!("'{name}' is the default for {list}; change defaults first."),
                true,
            );
            return;
        }
        self.store.config.providers.remove(&name);
        self.store.credentials.api_keys.remove(&name);
        self.provider_selected = self
            .provider_selected
            .min(self.store.config.providers.len().saturating_sub(1));
        self.screen = Screen::Providers;
        self.dirty = true;
        self.set_status(format!("Deleted provider '{name}'."), false);
    }

    fn save(&mut self, exit_after: bool) {
        match self.store.save() {
            Ok(()) => {
                self.dirty = false;
                self.set_status(
                    format!("Saved {}.", self.store.config_path().display()),
                    false,
                );
                if exit_after {
                    self.exit = true;
                }
            }
            Err(error) => self.set_status(error.to_string(), true),
        }
    }

    fn set_status(&mut self, message: impl Into<String>, error: bool) {
        self.status = message.into();
        self.status_error = error;
    }
}

#[derive(Clone)]
struct EditForm {
    original_name: Option<String>,
    name: String,
    provider: Provider,
    secret: String,
    secret_touched: bool,
    had_secret: bool,
    selected: usize,
}

impl EditForm {
    const FIELD_COUNT: usize = 13;

    fn new(name: String) -> Self {
        Self {
            original_name: None,
            name,
            provider: Provider::for_kind(ProviderKind::Custom),
            secret: String::new(),
            secret_touched: false,
            had_secret: false,
            selected: 0,
        }
    }

    fn existing(name: String, provider: Provider, had_secret: bool) -> Self {
        Self {
            original_name: Some(name.clone()),
            name,
            provider,
            secret: String::new(),
            secret_touched: false,
            had_secret,
            selected: 0,
        }
    }

    fn move_field(&mut self, delta: isize) {
        self.selected =
            (self.selected as isize + delta).rem_euclid(Self::FIELD_COUNT as isize) as usize;
    }

    /// Codex profiles pick their model from the synchronized catalog instead
    /// of typing an ID by hand.
    fn browses_models(&self) -> bool {
        self.selected == 2 && self.provider.kind == ProviderKind::Codex
    }

    fn cycle(&mut self, delta: isize) {
        match self.selected {
            1 => {
                let index = ProviderKind::ALL
                    .iter()
                    .position(|kind| *kind == self.provider.kind)
                    .unwrap_or(0);
                let next = (index as isize + delta).rem_euclid(ProviderKind::ALL.len() as isize);
                self.provider = Provider::for_kind(ProviderKind::ALL[next as usize]);
            }
            3 => {
                let choices = [
                    None,
                    Some(ReasoningEffort::Low),
                    Some(ReasoningEffort::Medium),
                    Some(ReasoningEffort::High),
                    Some(ReasoningEffort::Xhigh),
                    Some(ReasoningEffort::Max),
                ];
                let index = choices
                    .iter()
                    .position(|effort| *effort == self.provider.reasoning_effort)
                    .unwrap_or(0);
                let next = (index as isize + delta).rem_euclid(choices.len() as isize);
                self.provider.reasoning_effort = choices[next as usize];
            }
            7 => {
                let index = Protocol::ALL
                    .iter()
                    .position(|protocol| *protocol == self.provider.protocol)
                    .unwrap_or(0);
                let next = (index as isize + delta).rem_euclid(Protocol::ALL.len() as isize);
                self.provider.protocol = Protocol::ALL[next as usize];
            }
            8 => {
                let index = AuthStyle::ALL
                    .iter()
                    .position(|auth| *auth == self.provider.auth)
                    .unwrap_or(0);
                let next = (index as isize + delta).rem_euclid(AuthStyle::ALL.len() as isize);
                self.provider.auth = AuthStyle::ALL[next as usize];
            }
            12 => self.provider.enabled = !self.provider.enabled,
            _ => {}
        }
    }

    fn push(&mut self, character: char) {
        if let Some(value) = self.current_text_mut() {
            value.push(character);
        }
    }

    fn backspace(&mut self) {
        if let Some(value) = self.current_text_mut() {
            value.pop();
        }
    }

    fn clear_current(&mut self) {
        if let Some(value) = self.current_text_mut() {
            value.clear();
        }
    }

    fn current_text_mut(&mut self) -> Option<&mut String> {
        match self.selected {
            0 => Some(&mut self.name),
            2 => Some(&mut self.provider.model),
            4 => Some(self.provider.small_model.get_or_insert_default()),
            5 => Some(self.provider.base_url.get_or_insert_default()),
            6 => Some(self.provider.anthropic_base_url.get_or_insert_default()),
            9 => Some(self.provider.api_key_env.get_or_insert_default()),
            10 => {
                if !self.secret_touched {
                    self.secret.clear();
                    self.secret_touched = true;
                }
                Some(&mut self.secret)
            }
            11 => Some(self.provider.codex_profile.get_or_insert_default()),
            _ => None,
        }
    }

    fn validate(&self, config: &Config) -> Result<()> {
        validate_profile_name(&self.name)?;
        if self.provider.kind != ProviderKind::Codex && self.provider.model.trim().is_empty() {
            bail!("Model is required for non-Codex profiles.");
        }
        if self.provider.kind == ProviderKind::Custom
            && self.provider.effective_base_url().is_none()
        {
            bail!("Custom providers need a base URL.");
        }
        if config.providers.contains_key(&self.name)
            && self.original_name.as_deref() != Some(self.name.as_str())
        {
            bail!("Provider profile '{}' already exists.", self.name);
        }
        Ok(())
    }

    fn fields(&self) -> Vec<(&'static str, String, &'static str)> {
        vec![
            ("Profile name", self.name.clone(), "text"),
            ("Kind", self.provider.kind.to_string(), "←/→"),
            (
                "Model",
                self.provider.model.clone(),
                if self.provider.kind == ProviderKind::Codex {
                    "←/→ browse"
                } else {
                    "text"
                },
            ),
            (
                "Reasoning effort",
                self.provider
                    .reasoning_effort
                    .map_or_else(|| "auto".to_owned(), |effort| effort.to_string()),
                "←/→",
            ),
            (
                "Small model",
                self.provider.small_model.clone().unwrap_or_default(),
                "optional",
            ),
            (
                "Base URL",
                self.provider.base_url.clone().unwrap_or_default(),
                "text",
            ),
            (
                "Anthropic URL",
                self.provider.anthropic_base_url.clone().unwrap_or_default(),
                "optional",
            ),
            ("Protocol", self.provider.protocol.to_string(), "←/→"),
            ("Auth", self.provider.auth.to_string(), "←/→"),
            (
                "API key env",
                self.provider.api_key_env.clone().unwrap_or_default(),
                "optional",
            ),
            (
                "Saved API key",
                if self.secret_touched {
                    "•".repeat(self.secret.chars().count().max(1))
                } else if self.had_secret {
                    "<saved; type to replace>".to_owned()
                } else {
                    "<not saved; type to add>".to_owned()
                },
                "secret",
            ),
            (
                "Codex profile",
                self.provider.codex_profile.clone().unwrap_or_default(),
                "optional",
            ),
            (
                "Enabled",
                if self.provider.enabled { "yes" } else { "no" }.to_owned(),
                "←/→",
            ),
        ]
    }
}

fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    if app.screen == Screen::Model
        && let Some(picker) = &app.picker
    {
        crate::model_picker::draw(frame, picker);
        return;
    }

    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let dirty = if app.dirty { " • unsaved" } else { "" };
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                " all-code / alc config ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(dirty, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(format!(" {}", app.store.config_path().display())),
    ])
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, areas[0]);

    match app.screen {
        Screen::Providers | Screen::ConfirmDelete => draw_providers(frame, app, areas[1]),
        Screen::Defaults => draw_defaults(frame, app, areas[1]),
        Screen::Edit | Screen::Model => draw_edit(frame, app, areas[1]),
    }

    let status_style = if app.status_error {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", status_style),
            Span::styled(&app.status, status_style),
        ])),
        areas[2],
    );

    let help = match app.screen {
        Screen::Providers | Screen::ConfirmDelete => {
            " ↑↓ select  Enter/e edit  a add  d delete  Tab defaults  s save  q save+quit  Ctrl+C cancel"
        }
        Screen::Defaults => {
            " ↑↓ agent  ←→ provider  Tab/Esc providers  s save  q save+quit  Ctrl+C cancel"
        }
        Screen::Edit => {
            " ↑↓/Tab field  ←→ choice  type/backspace edit  Ctrl+U clear  Enter apply  Esc cancel"
        }
        Screen::Model => " ↑↓ select  Enter confirm  Esc back",
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
        areas[3],
    );

    if app.screen == Screen::ConfirmDelete {
        let name = app.selected_provider_name().unwrap_or_default();
        let area = centered_rect(58, 7, frame.area());
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(format!(
                "\nDelete provider '{name}' and its saved key?\n\n[y/Enter] delete    [n/Esc] cancel"
            ))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(" Confirm delete ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red)),
            ),
            area,
        );
    }
}

fn draw_providers(frame: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let header = Row::new(["Profile", "Kind", "Model", "Key", "Defaults"])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);
    let rows = app.store.config.providers.iter().map(|(name, provider)| {
        let key = credential_status(app, name, provider.auth);
        let defaults = Agent::ALL
            .iter()
            .filter(|agent| app.store.config.defaults.get(**agent) == name)
            .map(|agent| agent.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let style = if provider.enabled {
            Style::default()
        } else {
            Style::default().fg(Color::DarkGray)
        };
        Row::new([
            Cell::from(name.clone()),
            Cell::from(provider.kind.to_string()),
            Cell::from(format!(
                "{} / {}",
                if provider.model.is_empty() {
                    "<Codex config>".to_owned()
                } else {
                    provider.model.clone()
                },
                provider
                    .reasoning_effort
                    .map_or_else(|| "auto".to_owned(), |effort| effort.to_string())
            )),
            Cell::from(key),
            Cell::from(defaults),
        ])
        .style(style)
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(12),
            Constraint::Min(20),
            Constraint::Length(11),
            Constraint::Length(22),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Provider profiles ")
            .borders(Borders::ALL),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("› ");
    let mut state = TableState::default().with_selected(Some(app.provider_selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_defaults(frame: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let header = Row::new(["Agent", "Default provider", "Compatibility"])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);
    let rows = Agent::ALL.into_iter().map(|agent| {
        let name = app.store.config.defaults.get(agent);
        let compatible = app
            .store
            .config
            .providers
            .get(name)
            .is_some_and(|provider| provider.supports(agent));
        Row::new([
            Cell::from(agent.to_string()),
            Cell::from(name.to_owned()),
            Cell::from(if compatible { "ready" } else { "incompatible" }),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(28),
            Constraint::Min(16),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Agent defaults — Left/Right cycles compatible profiles ")
            .borders(Borders::ALL),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("› ");
    let mut state = TableState::default().with_selected(Some(app.default_selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_edit(frame: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let Some(form) = &app.edit else {
        return;
    };
    let items = form.fields().into_iter().map(|(label, value, hint)| {
        ListItem::new(Line::from(vec![
            Span::styled(format!("{label:<18}"), Style::default().fg(Color::Cyan)),
            Span::raw(if value.is_empty() { "<empty>" } else { &value }.to_owned()),
            Span::styled(format!("  [{hint}]"), Style::default().fg(Color::DarkGray)),
        ]))
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(if form.original_name.is_some() {
                    " Edit provider — Enter applies all fields "
                } else {
                    " Add provider — Enter applies all fields "
                })
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default().with_selected(Some(form.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn credential_status(app: &App, name: &str, auth: AuthStyle) -> String {
    let provider = &app.store.config.providers[name];
    if provider
        .api_key_env
        .as_deref()
        .and_then(|variable| std::env::var(variable).ok())
        .is_some_and(|value| !value.is_empty())
    {
        "environment".to_owned()
    } else if app.store.credentials.api_keys.contains_key(name) {
        "saved".to_owned()
    } else if matches!(auth, AuthStyle::Native | AuthStyle::None) {
        "not needed".to_owned()
    } else if provider.kind == ProviderKind::Anthropic {
        "native/key".to_owned()
    } else {
        "missing".to_owned()
    }
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Credentials};
    use crate::model_catalog::ModelCatalog;
    use std::path::PathBuf;

    fn app_editing_codex(model: &str) -> App {
        let store = Store {
            dir: PathBuf::from("test"),
            config: Config::default(),
            credentials: Credentials::default(),
        };
        let mut provider = Provider::for_kind(ProviderKind::Codex);
        provider.model = model.to_owned();
        let mut app = App::new(store, ModelCatalog::built_in());
        let mut form = EditForm::existing("codex".to_owned(), provider, false);
        form.selected = 2;
        app.edit = Some(form);
        app.screen = Screen::Edit;
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn the_model_field_browses_the_catalog_for_codex_providers() {
        let mut app = app_editing_codex("gpt-5.6-luna");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.screen, Screen::Model);
    }

    #[test]
    fn other_provider_kinds_keep_typing_their_model_name() {
        let mut app = app_editing_codex("gpt-5.6-luna");
        app.edit.as_mut().unwrap().provider.kind = ProviderKind::Openrouter;
        press(&mut app, KeyCode::Right);
        assert_eq!(app.screen, Screen::Edit);
    }

    #[test]
    fn choosing_a_model_fills_in_the_provider_form() {
        let mut app = app_editing_codex("gpt-5.6-sol");
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);

        let form = app.edit.as_ref().expect("edit form");
        assert_eq!(form.provider.model, "gpt-5.6-terra");
        assert_eq!(
            form.provider.reasoning_effort,
            Some(ReasoningEffort::Medium)
        );
        assert_eq!(app.screen, Screen::Edit);
    }

    #[test]
    fn changing_kind_applies_kind_defaults() {
        let mut form = EditForm::new("test".to_owned());
        form.selected = 1;
        while form.provider.kind != ProviderKind::Ollama {
            form.cycle(1);
        }
        assert_eq!(form.provider.protocol, Protocol::Dual);
        assert_eq!(form.provider.model, "qwen3-coder");
    }

    #[test]
    fn secret_is_not_loaded_into_edit_text() {
        let form = EditForm::existing(
            "openai".to_owned(),
            Provider::for_kind(ProviderKind::Openai),
            true,
        );
        assert!(form.secret.is_empty());
        assert!(form.fields()[10].1.contains("saved"));
    }

    #[test]
    fn reasoning_effort_cycles_from_auto_through_max() {
        let mut form = EditForm::new("test".to_owned());
        form.selected = 3;
        assert_eq!(form.provider.reasoning_effort, None);
        form.cycle(1);
        assert_eq!(form.provider.reasoning_effort, Some(ReasoningEffort::Low));
        for _ in 0..4 {
            form.cycle(1);
        }
        assert_eq!(form.provider.reasoning_effort, Some(ReasoningEffort::Max));
        form.cycle(1);
        assert_eq!(form.provider.reasoning_effort, None);
    }
}
