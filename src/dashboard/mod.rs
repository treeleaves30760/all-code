//! The interactive views of `alc doctor`, `alc usage` and `alc tps`.
//!
//! Each command still renders its plain report, and that report is still
//! what a pipe, a file, `--json` or `--plain` gets. On a terminal the same
//! report opens in a full-screen view instead: doctor streams its sections
//! into [`report::Viewer`] as each check finishes, and tps and usage
//! get dashboards of their own ([`tps`], [`usage`]) over the structured rows.

pub(crate) mod ansi;
pub(crate) mod report;
pub(crate) mod tps;
pub(crate) mod usage;

use std::cell::RefCell;
use std::io::{self, IsTerminal, Stdout};
use std::sync::mpsc::Sender;

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};

pub(crate) type Backend = CrosstermBackend<Stdout>;

/// Whether a report should open interactively: only on a terminal both
/// ways, never when its output was asked for as data or as plain text.
pub(crate) fn wanted(plain: bool) -> bool {
    !plain
        && std::env::var_os("ALC_NO_TUI").is_none()
        && std::env::var("TERM").map_or(true, |term| term != "dumb")
        && io::stdout().is_terminal()
        && io::stdin().is_terminal()
}

/// The alternate screen in raw mode, put back however the view ends.
pub(crate) struct Screen {
    pub terminal: Terminal<Backend>,
}

impl Screen {
    pub(crate) fn open() -> Result<Self> {
        enable_raw_mode()?;
        let result = (|| {
            let mut stdout = io::stdout();
            execute!(stdout, EnterAlternateScreen)?;
            let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
            terminal.clear()?;
            Ok(Self { terminal })
        })();
        if result.is_err() {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
        result
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

thread_local! {
    /// Where this thread's report text goes while a view is loading it.
    static SINK: RefCell<Option<Sender<String>>> = const { RefCell::new(None) };
}

/// Writes report text: to the view loading it on this thread, else stdout.
pub(crate) fn emit(text: &str) {
    let captured = SINK.with(|sink| {
        sink.borrow()
            .as_ref()
            .map(|sender| sender.send(text.to_owned()))
    });
    if captured.is_none() {
        print!("{text}");
    }
}

/// Sends this thread's report text to `sender` until the guard drops.
pub(crate) fn capture(sender: Sender<String>) -> CaptureGuard {
    SINK.with(|sink| *sink.borrow_mut() = Some(sender));
    CaptureGuard
}

pub(crate) struct CaptureGuard;

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        SINK.with(|sink| *sink.borrow_mut() = None);
    }
}

/// `println!`, through [`emit`].
macro_rules! outln {
    () => { $crate::dashboard::emit("\n") };
    ($($arg:tt)*) => { $crate::dashboard::emit(&format!("{}\n", format_args!($($arg)*))) };
}

/// `print!`, through [`emit`].
macro_rules! out {
    ($($arg:tt)*) => { $crate::dashboard::emit(&format!($($arg)*)) };
}

pub(crate) use {out, outln};

/// The views' shared palette.
pub(crate) mod theme {
    use super::*;

    pub(crate) const ACCENT: Color = Color::Cyan;
    pub(crate) const GOOD: Color = Color::Green;
    pub(crate) const WARN: Color = Color::Yellow;
    pub(crate) const BAD: Color = Color::Red;
    pub(crate) const MUTED: Color = Color::DarkGray;

    pub(crate) fn title() -> Style {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    }

    pub(crate) fn key() -> Style {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn muted() -> Style {
        Style::default().fg(MUTED)
    }

    pub(crate) fn selected() -> Style {
        Style::default()
            .bg(Color::Indexed(236))
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn highlight() -> Style {
        Style::default().fg(Color::Black).bg(WARN)
    }
}

/// A footer hint: the key, reversed, then what it does.
pub(crate) fn hint<'a>(key: &'a str, label: &'a str) -> [ratatui::text::Span<'a>; 3] {
    [
        ratatui::text::Span::styled(format!(" {key} "), theme::key()),
        ratatui::text::Span::raw(format!(" {label}")),
        ratatui::text::Span::raw("  "),
    ]
}

/// A spinner frame for work still running.
pub(crate) fn spinner(tick: usize) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[tick % FRAMES.len()]
}
