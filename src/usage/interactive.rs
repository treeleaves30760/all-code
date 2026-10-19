//! `alc usage` in the interactive viewer.
//!
//! Two slots: the accounts (quota lookups over the network, so they fill in
//! last) and the token statistics read from local history. The period and
//! the history source can be changed in place; only the statistics reload.

use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use super::query::{self, CalendarWindow, UsageOptions};
use super::{ClaudeStores, Env, ResolvedBy, view};
use crate::config::Store;
use crate::dashboard::emit;
use crate::dashboard::report::{Controls, Loader, Viewer};
use crate::doctor::Theme;

/// The periods `p` steps through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
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

/// Columns the viewer's sidebar and borders take from a section.
const SIDEBAR: usize = 38;

const SOURCES: [&str; 4] = ["all", "alc", "claude", "codex"];

struct UsageControls {
    store: Arc<Store>,
    requested: Option<String>,
    options: UsageOptions,
    period: Period,
}

impl UsageControls {
    fn source(&self) -> &str {
        match self.options.query.source.as_slice() {
            [] => "all",
            [one] => one.as_str(),
            _ => "mixed",
        }
    }
}

impl Controls for UsageControls {
    fn loaders(&self) -> Vec<(String, Loader)> {
        let store = Arc::clone(&self.store);
        let requested = self.requested.clone();
        let offline = self.options.offline;
        let accounts: Loader = Arc::new(move || {
            if offline {
                emit("Accounts\n  not fetched (--offline)\n");
                return Ok(0);
            }
            let report = super::build_report(
                &store,
                requested.as_deref(),
                ResolvedBy::Cli,
                &Env::from_process(),
                ClaudeStores::All,
            );
            emit(&super::render(&report, &Theme::detect()));
            Ok(u8::from(report.needs_attention()))
        });

        let store = Arc::clone(&self.store);
        let mut options = self.options.clone();
        self.period.apply(&mut options);
        let statistics: Loader = Arc::new(move || {
            let statistics = query::statistics(&store.dir, &store.config, &options)?;
            let theme = Theme::detect();
            let view = view::View {
                monthly: options.monthly,
                // The section pane, not the terminal: the sidebar and the
                // pane's borders take their share.
                width: view::terminal_width().map(|columns| columns.saturating_sub(SIDEBAR)),
            };
            emit(&view::render_statistics(&statistics, &theme, view));
            if options.details {
                emit(&query::render_details(&statistics, &theme));
            }
            Ok(0)
        });
        vec![
            ("Accounts".to_owned(), accounts),
            ("Token usage".to_owned(), statistics),
        ]
    }

    fn subtitle(&self) -> String {
        let mut subtitle = format!(
            "{} · source {}",
            self.period.label(&self.options),
            self.source()
        );
        if let Some(profile) = &self.options.query.filter_profile {
            subtitle.push_str(&format!(" · profile {profile}"));
        }
        if self.options.details {
            subtitle.push_str(" · details");
        }
        subtitle
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("p", "period"), ("s", "source"), ("d", "details")]
    }

    fn key(&mut self, key: KeyEvent) -> Option<Vec<usize>> {
        match key.code {
            KeyCode::Char('p') => {
                let next = Period::CYCLE
                    .iter()
                    .position(|period| *period == self.period)
                    .map_or(0, |index| (index + 1) % Period::CYCLE.len());
                self.period = Period::CYCLE[next];
                Some(vec![1])
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
                Some(vec![1])
            }
            KeyCode::Char('d') => {
                self.options.details = !self.options.details;
                Some(vec![1])
            }
            _ => None,
        }
    }
}

pub(crate) fn run(store: &Store, requested: Option<&str>, options: &UsageOptions) -> Result<u8> {
    let controls = UsageControls {
        store: Arc::new(store.clone()),
        requested: requested.map(str::to_owned),
        options: options.clone(),
        period: Period::AsAsked,
    };
    Viewer::new("alc usage", controls).run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controls() -> UsageControls {
        UsageControls {
            store: Arc::new(Store {
                dir: std::path::PathBuf::from("/nonexistent"),
                config: crate::config::Config::default(),
                credentials: Default::default(),
            }),
            requested: None,
            options: UsageOptions::default(),
            period: Period::AsAsked,
        }
    }

    #[test]
    fn the_period_key_steps_through_windows_and_reloads_only_the_statistics() {
        let mut controls = controls();
        assert_eq!(controls.subtitle(), "all history by day · source all");
        assert_eq!(
            controls.key(KeyEvent::from(KeyCode::Char('p'))),
            Some(vec![1])
        );
        assert_eq!(controls.period, Period::Week);
        assert!(controls.subtitle().starts_with("this week"));
        for _ in 0..4 {
            controls.key(KeyEvent::from(KeyCode::Char('p')));
        }
        assert_eq!(controls.period, Period::AllByMonth);
        controls.key(KeyEvent::from(KeyCode::Char('p')));
        assert_eq!(controls.period, Period::Week);

        let mut options = UsageOptions::default();
        options.query.since = Some("2026-01-01".to_owned());
        Period::Month.apply(&mut options);
        assert_eq!(options.window, Some(CalendarWindow::Monthly));
        assert!(options.query.since.is_none());
    }

    #[test]
    fn the_source_key_cycles_every_history() {
        let mut controls = controls();
        let mut seen = Vec::new();
        for _ in 0..4 {
            controls.key(KeyEvent::from(KeyCode::Char('s')));
            seen.push(controls.source().to_owned());
        }
        assert_eq!(seen, ["alc", "claude", "codex", "all"]);
        assert_eq!(controls.key(KeyEvent::from(KeyCode::Char('x'))), None);
    }
}
