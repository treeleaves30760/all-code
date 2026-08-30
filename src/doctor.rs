use std::env;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::Result;

use crate::config::{Agent, AuthStyle, Provider, ProviderKind, ReasoningEffort, Store};
use crate::model_catalog::ModelCatalog;

const INDENT: &str = "  ";
const GUTTER: &str = "  ";
/// Profile names are elided past this so one outlier cannot stretch every column.
const NAME_LIMIT: usize = 24;

pub fn run(store: &Store) -> Result<bool> {
    let theme = Theme::detect();
    let mut issues = Vec::new();
    let codex_for_claude = store
        .config
        .providers
        .values()
        .any(|provider| provider.enabled && provider.kind == ProviderKind::Codex);

    println!("{}", theme.paint(Tone::Head, "alc doctor"));
    environment(store, &theme, &mut issues);
    binaries(&theme, codex_for_claude, &mut issues);
    profiles(store, &theme, &mut issues);
    defaults(store, &theme);
    if codex_for_claude {
        codex_to_claude(store, &theme, &mut issues);
    }
    summary(&theme, &issues);

    Ok(issues.is_empty())
}

fn environment(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    heading(theme, "Environment");
    let validation = match store.config.validate() {
        Ok(()) => format!("{} ok", theme.mark(Status::Good)),
        Err(error) => {
            let error = error.to_string();
            let line = format!(
                "{} {}",
                theme.mark(Status::Bad),
                theme.paint(Tone::Bad, &error)
            );
            issues.push(Issue::new("config", error, None));
            line
        }
    };
    pairs(&[
        ("Config", store.config_path().display().to_string()),
        (
            "Credentials",
            store.credentials_path().display().to_string(),
        ),
        ("Validation", validation),
    ]);
}

fn binaries(theme: &Theme, codex_for_claude: bool, issues: &mut Vec<Issue>) {
    heading(theme, "Agent binaries");
    let mut rows = Vec::new();

    for agent in Agent::ALL {
        let binary = binary_for(agent);
        match resolve(&binary) {
            Some(path) => rows.push(Row::new(Status::Good, agent, path.display().to_string())),
            None => {
                let name = binary.to_string_lossy().into_owned();
                rows.push(Row::new(
                    Status::Bad,
                    agent,
                    theme.paint(Tone::Bad, &format!("not found ({name})")),
                ));
                issues.push(Issue::new(
                    agent.to_string(),
                    format!("`{name}` is not on PATH"),
                    Some(format!("install {agent}, then reopen your shell")),
                ));
            }
        }
    }

    match helper_path() {
        Some(path) => rows.push(Row::new(
            Status::Good,
            "adapter",
            format!(
                "{}{}",
                path.display(),
                theme.paint(
                    Tone::Dim,
                    &format!(
                        "{GUTTER}(claude-codex {})",
                        crate::launch::CLAUDE_CODEX_HELPER_VERSION
                    )
                )
            ),
        )),
        None if codex_for_claude => {
            rows.push(Row::new(
                Status::Bad,
                "adapter",
                theme.paint(Tone::Bad, "claude-codex not installed"),
            ));
            issues.push(Issue::new(
                "adapter",
                format!(
                    "claude-codex is required by Codex {} Claude profiles",
                    theme.arrow()
                ),
                Some("reinstall alc, or put `claude-codex` on PATH".to_owned()),
            ));
        }
        None => rows.push(Row::new(
            Status::Off,
            "adapter",
            theme.paint(Tone::Dim, "optional helper not installed"),
        )),
    }

    marked(theme, &rows);
}

fn profiles(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    heading(theme, "Provider profiles");
    let mut headers = vec!["PROFILE".to_owned(), "KIND".to_owned(), "KEY".to_owned()];
    headers.extend(Agent::ALL.map(|agent| agent.as_str().to_uppercase()));
    let mut table = Table::new(headers);

    for (name, provider) in &store.config.providers {
        let (key, tone) = key_status(store, name, provider);
        if tone == Tone::Bad {
            issues.push(Issue::new(
                name.clone(),
                "API key missing".to_owned(),
                Some(format!("alc config key {name}")),
            ));
        }

        let mut row = vec![
            Cell::left(
                truncate(name, NAME_LIMIT),
                if provider.enabled {
                    Tone::Plain
                } else {
                    Tone::Dim
                },
            ),
            Cell::left(provider.kind.to_string(), Tone::Dim),
            Cell::left(key, tone),
        ];
        row.extend(Agent::ALL.map(|agent| {
            let status = if provider.supports(agent) {
                Status::Good
            } else {
                Status::Off
            };
            Cell::center(theme.glyph(status), status.tone())
        }));
        table.push(row);
    }

    for line in table.render(theme) {
        println!("{INDENT}{line}");
    }
}

/// Unusable defaults are already reported by `Config::validate`, so this section
/// only points at which agent is affected instead of counting the problem twice.
fn defaults(store: &Store, theme: &Theme) {
    heading(theme, "Defaults");
    let rows = Agent::ALL.map(|agent| {
        let name = store.config.defaults.get(agent);
        match store.config.providers.get(name) {
            Some(provider) if provider.supports(agent) => {
                Row::new(Status::Good, agent, name.to_owned())
            }
            Some(_) => Row::new(
                Status::Bad,
                agent,
                format!(
                    "{name}{GUTTER}{}",
                    theme.paint(Tone::Bad, "(cannot run this agent)")
                ),
            ),
            None => Row::new(
                Status::Bad,
                agent,
                format!(
                    "{name}{GUTTER}{}",
                    theme.paint(Tone::Bad, "(no such profile)")
                ),
            ),
        }
    });
    marked(theme, &rows);
}

fn codex_to_claude(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    heading(theme, &format!("Codex {} Claude", theme.arrow()));
    let catalog = ModelCatalog::load(&store.dir);
    let mut rows = Vec::new();

    for (name, provider) in &store.config.providers {
        if !provider.enabled || provider.kind != ProviderKind::Codex {
            continue;
        }
        let (status, model) = match crate::launch::resolve_codex_model(provider) {
            Ok(model) => (Status::Good, model),
            Err(_) => (Status::Warn, "<unresolved>".to_owned()),
        };
        let effort = crate::launch::resolve_codex_effort(provider)
            .ok()
            .flatten()
            .or_else(|| catalog.find(&model).map(|entry| entry.default_effort))
            .unwrap_or(ReasoningEffort::Medium);
        rows.push(Row::new(
            status,
            name.clone(),
            format!("{model}{}", theme.paint(Tone::Dim, &format!(" / {effort}"))),
        ));
    }

    match codex_login_status() {
        Some(true) => rows.push(Row::new(Status::Good, "login", "signed in".to_owned())),
        Some(false) => {
            rows.push(Row::new(
                Status::Bad,
                "login",
                theme.paint(Tone::Bad, "signed out or expired"),
            ));
            issues.push(Issue::new(
                "codex login",
                "signed out or expired".to_owned(),
                Some("codex login".to_owned()),
            ));
        }
        None => rows.push(Row::new(
            Status::Warn,
            "login",
            theme.paint(Tone::Warn, "cannot check (Codex binary missing)"),
        )),
    }
    rows.push(Row::blank(
        "catalog",
        theme.paint(Tone::Dim, &catalog.source),
    ));

    marked(theme, &rows);
}

fn summary(theme: &Theme, issues: &[Issue]) {
    println!();
    if issues.is_empty() {
        println!(
            "{} {}",
            theme.mark(Status::Good),
            theme.paint(Tone::Good, "ready")
        );
        return;
    }

    let count = issues.len();
    let noun = if count == 1 { "issue" } else { "issues" };
    println!(
        "{} {}",
        theme.mark(Status::Bad),
        theme.paint(
            Tone::Bad,
            &format!("needs attention {} {count} {noun}", theme.dash())
        )
    );

    let subject = issues
        .iter()
        .map(|issue| width(&issue.subject))
        .max()
        .unwrap_or_default();
    // Only the issues that carry a fix line up their arrows; a long fix-less problem
    // would otherwise push that column off to the right.
    let problem = issues
        .iter()
        .filter(|issue| issue.fix.is_some())
        .map(|issue| width(&issue.problem))
        .max()
        .unwrap_or_default();
    for issue in issues {
        let fix = match &issue.fix {
            Some(fix) => format!(
                "{GUTTER}{}{GUTTER}{}",
                theme.arrow(),
                theme.paint(Tone::Dim, fix)
            ),
            None => String::new(),
        };
        let tail = pad(
            &issue.problem,
            if fix.is_empty() { 0 } else { problem },
            Align::Left,
        );
        println!(
            "{INDENT}{} {}{GUTTER}{tail}{fix}",
            theme.bullet(),
            pad(&issue.subject, subject, Align::Left)
        );
    }
}

fn heading(theme: &Theme, title: &str) {
    println!();
    println!("{}", theme.paint(Tone::Head, title));
}

/// Prints `label  value` rows with the labels padded to a common width.
fn pairs(rows: &[(&str, String)]) {
    let label = rows
        .iter()
        .map(|(label, _)| width(label))
        .max()
        .unwrap_or_default();
    for (name, value) in rows {
        println!("{INDENT}{}{GUTTER}{value}", pad(name, label, Align::Left));
    }
}

/// Prints `glyph  name  detail` rows with the names padded to a common width.
fn marked(theme: &Theme, rows: &[Row]) {
    let name = rows
        .iter()
        .map(|row| width(&row.name))
        .max()
        .unwrap_or_default();
    for row in rows {
        let mark = match row.status {
            Some(status) => theme.mark(status),
            None => " ".to_owned(),
        };
        println!(
            "{INDENT}{mark}{GUTTER}{}{GUTTER}{}",
            pad(&row.name, name, Align::Left),
            row.detail
        );
    }
}

fn key_status(store: &Store, name: &str, provider: &Provider) -> (String, Tone) {
    if provider
        .api_key_env
        .as_deref()
        .and_then(|variable| env::var(variable).ok())
        .is_some_and(|value| !value.is_empty())
    {
        ("env".to_owned(), Tone::Good)
    } else if store.credentials.api_keys.contains_key(name) {
        ("saved".to_owned(), Tone::Good)
    } else if matches!(provider.auth, AuthStyle::Native | AuthStyle::None) {
        ("n/a".to_owned(), Tone::Dim)
    } else if provider.kind == ProviderKind::Anthropic {
        ("native/key".to_owned(), Tone::Plain)
    } else {
        ("missing".to_owned(), Tone::Bad)
    }
}

/// A `glyph  name  detail` line; `status` is `None` for rows that only carry information.
struct Row {
    status: Option<Status>,
    name: String,
    detail: String,
}

impl Row {
    fn new(status: Status, name: impl ToString, detail: String) -> Self {
        Self {
            status: Some(status),
            name: name.to_string(),
            detail,
        }
    }

    fn blank(name: impl ToString, detail: String) -> Self {
        Self {
            status: None,
            name: name.to_string(),
            detail,
        }
    }
}

/// One reason `alc doctor` reports failure, with the command that clears it.
struct Issue {
    subject: String,
    problem: String,
    fix: Option<String>,
}

impl Issue {
    fn new(subject: impl Into<String>, problem: String, fix: Option<String>) -> Self {
        Self {
            subject: subject.into(),
            problem,
            fix,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Good,
    Warn,
    Bad,
    Off,
}

impl Status {
    fn tone(self) -> Tone {
        match self {
            Self::Good => Tone::Good,
            Self::Warn => Tone::Warn,
            Self::Bad => Tone::Bad,
            Self::Off => Tone::Dim,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    Head,
    Dim,
    Good,
    Warn,
    Bad,
}

impl Tone {
    fn code(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Head => Some("1"),
            Self::Dim => Some("90"),
            Self::Good => Some("32"),
            Self::Warn => Some("33"),
            Self::Bad => Some("31"),
        }
    }
}

/// What the stream the report is written to can render.
struct Theme {
    color: bool,
    unicode: bool,
}

impl Theme {
    fn detect() -> Self {
        Self {
            color: color_supported(),
            unicode: unicode_supported(),
        }
    }

    fn paint(&self, tone: Tone, text: &str) -> String {
        match tone.code() {
            Some(code) if self.color => format!("\x1b[{code}m{text}\x1b[0m"),
            _ => text.to_owned(),
        }
    }

    fn glyph(&self, status: Status) -> &'static str {
        match (status, self.unicode) {
            (Status::Good, true) => "✓",
            (Status::Good, false) => "+",
            (Status::Warn, _) => "!",
            (Status::Bad, true) => "✗",
            (Status::Bad, false) => "x",
            (Status::Off, true) => "·",
            (Status::Off, false) => "-",
        }
    }

    fn mark(&self, status: Status) -> String {
        self.paint(status.tone(), self.glyph(status))
    }

    fn arrow(&self) -> &'static str {
        if self.unicode { "→" } else { "->" }
    }

    fn bullet(&self) -> &'static str {
        if self.unicode { "•" } else { "*" }
    }

    fn dash(&self) -> &'static str {
        if self.unicode { "—" } else { "-" }
    }
}

fn color_supported() -> bool {
    if env::var_os("NO_COLOR").is_some() || !io::stdout().is_terminal() {
        return false;
    }
    if env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    #[cfg(windows)]
    {
        crossterm::ansi_support::supports_ansi()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

fn unicode_supported() -> bool {
    if env::var_os("ALC_ASCII").is_some() {
        return false;
    }
    // Rust routes console writes on Windows through the wide API, so the code page
    // does not decide this; elsewhere the locale does.
    if cfg!(windows) {
        return true;
    }
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()))
        .is_none_or(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
}

struct Cell {
    text: String,
    tone: Tone,
    align: Align,
}

impl Cell {
    fn left(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            align: Align::Left,
        }
    }

    fn center(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            align: Align::Center,
        }
    }
}

/// A whitespace-aligned table whose columns are sized from the widest value in each,
/// so the layout cannot drift as profile names and provider kinds change.
struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    fn new<S: Into<String>>(headers: Vec<S>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    fn push(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    fn widths(&self) -> Vec<usize> {
        self.headers
            .iter()
            .enumerate()
            .map(|(column, header)| {
                self.rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| width(&cell.text))
                    .chain(std::iter::once(width(header)))
                    .max()
                    .unwrap_or_default()
            })
            .collect()
    }

    fn render(&self, theme: &Theme) -> Vec<String> {
        let widths = self.widths();
        let last = self.headers.len().saturating_sub(1);
        let mut lines = Vec::with_capacity(self.rows.len() + 1);

        let header: Vec<String> = self
            .headers
            .iter()
            .enumerate()
            .map(|(column, header)| {
                trim_last(pad(header, widths[column], Align::Left), column, last)
            })
            .collect();
        lines.push(theme.paint(Tone::Dim, &header.join(GUTTER)));

        for row in &self.rows {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    let text = trim_last(pad(&cell.text, widths[column], cell.align), column, last);
                    theme.paint(cell.tone, &text)
                })
                .collect();
            lines.push(cells.join(GUTTER));
        }
        lines
    }
}

/// Drops the padding of the final column so rows carry no trailing whitespace.
fn trim_last(text: String, column: usize, last: usize) -> String {
    if column == last {
        text.trim_end().to_owned()
    } else {
        text
    }
}

fn width(value: &str) -> usize {
    value.chars().count()
}

fn pad(value: &str, to: usize, align: Align) -> String {
    let current = width(value);
    if current >= to {
        return value.to_owned();
    }
    let missing = to - current;
    match align {
        Align::Left => format!("{value}{}", " ".repeat(missing)),
        Align::Center => {
            let left = missing / 2;
            format!("{}{value}{}", " ".repeat(left), " ".repeat(missing - left))
        }
    }
}

fn binary_for(agent: Agent) -> std::ffi::OsString {
    let override_name = match agent {
        Agent::Claude => "ALC_CLAUDE_BIN",
        Agent::Codex => "ALC_CODEX_BIN",
        Agent::Opencode => "ALC_OPENCODE_BIN",
        Agent::Pi => "ALC_PI_BIN",
        Agent::Copilot => "ALC_COPILOT_BIN",
        Agent::Goose => "ALC_GOOSE_BIN",
        Agent::Qwen => "ALC_QWEN_BIN",
        Agent::Kimi => "ALC_KIMI_BIN",
    };
    env::var_os(override_name).unwrap_or_else(|| agent.as_str().into())
}

fn resolve(binary: &std::ffi::OsStr) -> Option<PathBuf> {
    let path = PathBuf::from(binary);
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file().then_some(path);
    }
    which::which(binary).ok()
}

fn helper_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ALC_CLAUDE_CODEX_BIN") {
        return resolve(&path);
    }
    let file_name = if cfg!(windows) {
        "claude-codex.exe"
    } else {
        "claude-codex"
    };
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let sibling = parent.join(file_name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    which::which("claude-codex").ok()
}

fn codex_login_status() -> Option<bool> {
    let binary = resolve(&binary_for(Agent::Codex))?;
    Command::new(binary)
        .args(["login", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .map(|status| status.success())
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        value.to_owned()
    } else {
        let mut result: String = value.chars().take(width.saturating_sub(1)).collect();
        result.push('…');
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Theme {
        Theme {
            color: false,
            unicode: true,
        }
    }

    #[test]
    fn truncation_keeps_short_values() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghijk", 5), "abcd…");
    }

    #[test]
    fn padding_fills_to_the_requested_width() {
        assert_eq!(pad("ab", 5, Align::Left), "ab   ");
        assert_eq!(pad("ab", 5, Align::Center), " ab  ");
        assert_eq!(pad("✓", 5, Align::Center), "  ✓  ");
    }

    #[test]
    fn padding_never_shrinks_a_wide_value() {
        assert_eq!(pad("abcdef", 3, Align::Left), "abcdef");
    }

    #[test]
    fn columns_take_the_width_of_their_widest_value() {
        let mut table = Table::new(vec!["PROFILE", "KIND"]);
        table.push(vec![
            Cell::left("openrouter", Tone::Plain),
            Cell::left("x", Tone::Plain),
        ]);
        table.push(vec![
            Cell::left("a", Tone::Plain),
            Cell::left("anthropic", Tone::Plain),
        ]);
        assert_eq!(table.widths(), vec![10, 9]);
    }

    #[test]
    fn every_rendered_row_lines_up() {
        let mut table = Table::new(vec!["PROFILE", "KIND", "CLAUDE"]);
        table.push(vec![
            Cell::left("openrouter", Tone::Plain),
            Cell::left("openrouter", Tone::Plain),
            Cell::center("✓", Tone::Good),
        ]);
        table.push(vec![
            Cell::left("vllm", Tone::Plain),
            Cell::left("vllm", Tone::Plain),
            Cell::center("·", Tone::Dim),
        ]);

        let lines = table.render(&plain());
        let column = lines[0].find("KIND").expect("header column");
        for line in &lines[1..] {
            assert_eq!(
                line.char_indices()
                    .nth(column)
                    .map(|(_, character)| character),
                line[column..].chars().next(),
                "row does not start the KIND column at {column}: {line}"
            );
        }
        assert!(lines.iter().all(|line| line == line.trim_end()));
    }

    #[test]
    fn rendered_rows_carry_no_escape_codes_without_colour() {
        let mut table = Table::new(vec!["KEY"]);
        table.push(vec![Cell::left("missing", Tone::Bad)]);
        assert_eq!(table.render(&plain())[1], "missing");
    }

    /// Colour codes are invisible but not zero-width to `str::len`, so padding has to
    /// happen before painting or every coloured column drifts.
    #[test]
    fn colour_does_not_disturb_the_layout() {
        let build = || {
            let mut table = Table::new(vec!["PROFILE", "KEY", "CLAUDE"]);
            table.push(vec![
                Cell::left("openrouter", Tone::Plain),
                Cell::left("missing", Tone::Bad),
                Cell::center("✓", Tone::Good),
            ]);
            table.push(vec![
                Cell::left("vllm", Tone::Plain),
                Cell::left("n/a", Tone::Dim),
                Cell::center("·", Tone::Dim),
            ]);
            table
        };

        let coloured = build().render(&Theme {
            color: true,
            unicode: true,
        });
        assert!(
            coloured.iter().any(|line| line.contains('\x1b')),
            "expected colour codes to be emitted"
        );
        let stripped: Vec<String> = coloured.iter().map(|line| strip_ansi(line)).collect();
        assert_eq!(stripped, build().render(&plain()));
    }

    fn strip_ansi(line: &str) -> String {
        let mut out = String::new();
        let mut characters = line.chars();
        while let Some(character) = characters.next() {
            if character == '\x1b' {
                for escaped in characters.by_ref() {
                    if escaped == 'm' {
                        break;
                    }
                }
            } else {
                out.push(character);
            }
        }
        out
    }

    #[test]
    fn ascii_themes_avoid_multi_byte_glyphs() {
        let ascii = Theme {
            color: false,
            unicode: false,
        };
        for status in [Status::Good, Status::Warn, Status::Bad, Status::Off] {
            assert!(ascii.glyph(status).is_ascii());
        }
        assert!(ascii.arrow().is_ascii() && ascii.bullet().is_ascii() && ascii.dash().is_ascii());
    }

    #[test]
    fn every_glyph_is_one_column_wide() {
        let theme = plain();
        for status in [Status::Good, Status::Warn, Status::Bad, Status::Off] {
            assert_eq!(width(theme.glyph(status)), 1);
        }
    }
}
