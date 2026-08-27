use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::config::ReasoningEffort;
use crate::model_catalog::{ModelCatalog, ModelInfo};

#[derive(Debug, Clone)]
pub struct PickerRequest {
    pub model: String,
    pub effort: ReasoningEffort,
    pub choose_model: bool,
    pub choose_effort: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSelection {
    pub model: String,
    pub effort: ReasoningEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Model,
    Effort,
}

pub struct PickerApp {
    catalog: ModelCatalog,
    stage: Stage,
    model_selected: usize,
    initial_model_selected: Option<usize>,
    effort_selected: usize,
    choose_model: bool,
    choose_effort: bool,
    fixed_model: String,
    result: Option<Option<RuntimeSelection>>,
}

impl PickerApp {
    pub fn new(catalog: ModelCatalog, request: PickerRequest) -> Self {
        let initial_model_selected = catalog
            .models
            .iter()
            .position(|model| model.id == request.model);
        let model_selected = initial_model_selected.unwrap_or_else(|| {
            catalog
                .models
                .iter()
                .position(|model| model.id == "gpt-5.6-terra")
                .unwrap_or(0)
        });
        let effort_selected = ReasoningEffort::ALL
            .iter()
            .position(|effort| *effort == request.effort)
            .unwrap_or(1);
        let stage = if request.choose_model {
            Stage::Model
        } else {
            Stage::Effort
        };
        Self {
            catalog,
            stage,
            model_selected,
            initial_model_selected,
            effort_selected,
            choose_model: request.choose_model,
            choose_effort: request.choose_effort,
            fixed_model: request.model,
            result: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.result = Some(None);
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_selected(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selected(1),
            KeyCode::Char(character @ '1'..='9') => {
                let index = character.to_digit(10).unwrap_or(1) as usize - 1;
                self.select_index(index);
            }
            KeyCode::Enter => self.advance(),
            KeyCode::Esc => {
                if self.stage == Stage::Effort && self.choose_model {
                    self.stage = Stage::Model;
                } else {
                    self.result = Some(None);
                }
            }
            _ => {}
        }
    }

    fn move_selected(&mut self, delta: isize) {
        let (selected, length) = match self.stage {
            Stage::Model => (&mut self.model_selected, self.catalog.models.len()),
            Stage::Effort => (&mut self.effort_selected, ReasoningEffort::ALL.len()),
        };
        if length > 0 {
            *selected = (*selected as isize + delta).rem_euclid(length as isize) as usize;
        }
    }

    fn select_index(&mut self, index: usize) {
        match self.stage {
            Stage::Model if index < self.catalog.models.len() => self.model_selected = index,
            Stage::Effort if index < ReasoningEffort::ALL.len() => self.effort_selected = index,
            _ => {}
        }
    }

    fn advance(&mut self) {
        if self.stage == Stage::Model && self.choose_effort {
            if self.initial_model_selected != Some(self.model_selected)
                && let Some(model) = self.selected_model()
                && let Some(index) = ReasoningEffort::ALL
                    .iter()
                    .position(|effort| *effort == model.default_effort)
            {
                self.effort_selected = index;
            }
            self.stage = Stage::Effort;
            return;
        }
        let model = if self.choose_model {
            self.catalog.models[self.model_selected].id.clone()
        } else {
            self.fixed_model.clone()
        };
        self.result = Some(Some(RuntimeSelection {
            model,
            effort: ReasoningEffort::ALL[self.effort_selected],
        }));
    }

    /// The finished selection, or `None` when the user cancelled. Returns
    /// `None` overall while the picker is still open.
    pub fn take_result(&mut self) -> Option<Option<RuntimeSelection>> {
        self.result.take()
    }

    fn selected_model(&self) -> Option<&ModelInfo> {
        self.catalog.models.get(self.model_selected)
    }
}

pub fn draw(frame: &mut ratatui::Frame, app: &PickerApp) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let step = match (app.choose_model, app.choose_effort, app.stage) {
        (true, true, Stage::Model) => "Step 1/2 - choose a GPT model",
        (true, true, Stage::Effort) => "Step 2/2 - choose reasoning effort",
        (true, false, _) => "Choose a GPT model",
        (false, true, _) => "Choose reasoning effort",
        (false, false, _) => "Review selection",
    };
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                " alc config / Codex -> Claude ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {step}"), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(" Arrow keys or j/k to move; number keys select directly."),
        Line::from(format!(" Catalog: {}", app.catalog.source)),
    ])
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, areas[0]);

    match app.stage {
        Stage::Model => draw_models(frame, app, areas[1]),
        Stage::Effort => draw_efforts(frame, app, areas[1]),
    }

    let model = if app.choose_model {
        app.selected_model()
            .map(|model| model.id.as_str())
            .unwrap_or("<unknown>")
    } else {
        &app.fixed_model
    };
    let effort = ReasoningEffort::ALL[app.effort_selected];
    let footer = Paragraph::new(vec![
        Line::from(format!(" Default: {model} / {effort}")),
        Line::from(" Enter: confirm    Esc: back/cancel"),
    ])
    .style(Style::default().fg(Color::Green))
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, areas[2]);
}

fn draw_models(frame: &mut ratatui::Frame, app: &PickerApp, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(area);
    let items = app.catalog.models.iter().enumerate().map(|(index, model)| {
        let badge = match model.id.as_str() {
            "gpt-5.6-luna" => "budget",
            "gpt-5.6-terra" => "recommended",
            "gpt-5.6-sol" => "frontier",
            _ => "custom",
        };
        ListItem::new(vec![
            Line::from(format!("{}. {}  [{badge}]", index + 1, model.name)),
            Line::from(Span::styled(
                format!("   {}", model.id),
                Style::default().fg(Color::DarkGray),
            )),
        ])
    });
    let list = List::new(items)
        .block(Block::default().title(" Models ").borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(app.model_selected));
    frame.render_stateful_widget(list, columns[0], &mut state);

    let selected = app.selected_model();
    let details = selected.map_or_else(
        || vec![Line::from("No model available.")],
        |model| {
            vec![
                Line::from(Span::styled(
                    &model.name,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(model.description.as_str()),
                Line::from(""),
                Line::from(beginner_model_hint(&model.id)),
                Line::from(""),
                Line::from(format!("Codex default effort: {}", model.default_effort)),
                Line::from(format!(
                    "Codex context window: {}K tokens",
                    model.context_window / 1_000
                )),
            ]
        },
    );
    frame.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: true }).block(
            Block::default()
                .title(" Beginner guide ")
                .borders(Borders::ALL),
        ),
        columns[1],
    );
}

fn draw_efforts(frame: &mut ratatui::Frame, app: &PickerApp, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);
    let items = ReasoningEffort::ALL
        .iter()
        .enumerate()
        .map(|(index, effort)| {
            let recommended = (*effort == ReasoningEffort::Medium).then_some("  [recommended]");
            ListItem::new(format!(
                "{}. {}{}",
                index + 1,
                effort,
                recommended.unwrap_or_default()
            ))
        });
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Reasoning effort ")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(app.effort_selected));
    frame.render_stateful_widget(list, columns[0], &mut state);

    let effort = ReasoningEffort::ALL[app.effort_selected];
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                effort.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(effort_description(effort)),
            Line::from(""),
            Line::from("Higher effort can improve difficult work, but uses more time and quota."),
        ])
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .title(" What this means ")
                .borders(Borders::ALL),
        ),
        columns[1],
    );
}

fn beginner_model_hint(model: &str) -> &'static str {
    match model {
        "gpt-5.6-luna" => "Best for quick fixes, repetitive work, and keeping usage low.",
        "gpt-5.6-terra" => "Best starting point for most coding sessions.",
        "gpt-5.6-sol" => "Best for architecture, hard debugging, and large refactors.",
        _ => "Choose based on the provider documentation.",
    }
}

fn effort_description(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Low => "Fastest. Good for simple edits, questions, and routine tasks.",
        ReasoningEffort::Medium => "Balanced default. Recommended for everyday development.",
        ReasoningEffort::High => "More analysis for debugging and multi-file implementation.",
        ReasoningEffort::Xhigh => "Deep analysis for difficult, ambiguous, or high-risk work.",
        ReasoningEffort::Max => "Maximum depth for the hardest tasks; slowest and most expensive.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn rendered(app: &PickerApp) -> String {
        let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn the_model_screen_shows_the_whole_catalog() {
        let app = PickerApp::new(
            ModelCatalog::built_in(),
            PickerRequest {
                model: "gpt-5.6-terra".into(),
                effort: ReasoningEffort::Medium,
                choose_model: true,
                choose_effort: true,
            },
        );
        let screen = rendered(&app);
        for id in ["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"] {
            assert!(screen.contains(id), "{id} is missing from the picker");
        }
    }

    #[test]
    fn the_effort_screen_shows_every_level() {
        let mut app = PickerApp::new(
            ModelCatalog::built_in(),
            PickerRequest {
                model: "gpt-5.6-terra".into(),
                effort: ReasoningEffort::Medium,
                choose_model: true,
                choose_effort: true,
            },
        );
        app.advance();
        let screen = rendered(&app);
        for effort in ReasoningEffort::ALL {
            assert!(screen.contains(effort.as_str()), "{effort} is missing");
        }
    }

    #[test]
    fn picker_defaults_to_terra_when_model_is_unknown() {
        let catalog = ModelCatalog::built_in();
        let app = PickerApp::new(
            catalog,
            PickerRequest {
                model: "unknown".into(),
                effort: ReasoningEffort::Medium,
                choose_model: true,
                choose_effort: true,
            },
        );
        assert_eq!(app.selected_model().unwrap().id, "gpt-5.6-terra");
    }

    #[test]
    fn max_effort_is_available() {
        assert_eq!(ReasoningEffort::ALL.last(), Some(&ReasoningEffort::Max));
        assert!(effort_description(ReasoningEffort::Max).contains("Maximum"));
    }

    #[test]
    fn saved_effort_is_preserved_for_the_current_model() {
        let catalog = ModelCatalog::built_in();
        let mut app = PickerApp::new(
            catalog,
            PickerRequest {
                model: "gpt-5.6-sol".into(),
                effort: ReasoningEffort::Max,
                choose_model: true,
                choose_effort: true,
            },
        );
        app.advance();
        assert_eq!(
            ReasoningEffort::ALL[app.effort_selected],
            ReasoningEffort::Max
        );
    }

    #[test]
    fn changing_model_selects_its_codex_default_effort() {
        let catalog = ModelCatalog::built_in();
        let mut app = PickerApp::new(
            catalog,
            PickerRequest {
                model: "gpt-5.6-terra".into(),
                effort: ReasoningEffort::Medium,
                choose_model: true,
                choose_effort: true,
            },
        );
        app.move_selected(-1);
        app.advance();
        assert_eq!(
            app.selected_model().unwrap().id,
            "gpt-5.6-sol",
            "the catalog lists the most capable model first"
        );
        assert_eq!(
            ReasoningEffort::ALL[app.effort_selected],
            ReasoningEffort::Low
        );
    }
}
