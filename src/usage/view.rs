//! The terminal face of `alc usage`: boxed tables sized to the terminal,
//! known sums marked rather than erased, and one short legend.
//!
//! The exact, strict figures stay in `--json` and `--details`; this view is
//! for reading. A cell carries a marker instead of becoming `N/A` when part of
//! what it sums is unknown (`+`, a lower bound) or may be counted twice by two
//! uncorrelated histories (`~`).

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::io::{self, IsTerminal};

use super::pricing::Money;
use super::query::{DailyRollup, KnownTokens, Statistics, format_count};
use crate::config::Agent;
use crate::doctor::{INDENT, Theme, Tone, heading_text, width};

/// What the caller asked for, beyond the statistics themselves.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct View {
    pub monthly: bool,
    /// Columns available, or `None` when the output is not a terminal (then
    /// nothing is compacted, so piped text keeps every digit).
    pub width: Option<usize>,
}

pub(crate) fn terminal_width() -> Option<usize> {
    if let Some(columns) = env::var("COLUMNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|columns| *columns >= 40)
    {
        return Some(columns);
    }
    if !io::stdout().is_terminal() {
        return None;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns >= 40)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Right,
}

/// One table column. `wrap` columns hold item lists and may wrap onto more
/// lines down to `min`; `drop` ranks optional columns, highest dropped first.
pub(crate) struct Column {
    header: &'static str,
    align: Align,
    wrap: Option<usize>,
    drop: u8,
}

impl Column {
    pub(crate) fn left(header: &'static str) -> Self {
        Self {
            header,
            align: Align::Left,
            wrap: None,
            drop: 0,
        }
    }

    pub(crate) fn right(header: &'static str) -> Self {
        Self {
            align: Align::Right,
            ..Self::left(header)
        }
    }

    pub(crate) fn wrap(mut self, min: usize) -> Self {
        self.wrap = Some(min);
        self
    }

    pub(crate) fn optional(mut self, rank: u8) -> Self {
        self.drop = rank;
        self
    }
}

#[derive(Clone)]
pub(crate) enum Cell {
    /// Toned fragments on one line, e.g. a number and its marker.
    Spans(Vec<(String, Tone)>),
    /// A list that may wrap between items.
    Items(Vec<String>, Tone),
}

impl Cell {
    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self::Spans(vec![(text.into(), Tone::Plain)])
    }

    pub(crate) fn toned(text: impl Into<String>, tone: Tone) -> Self {
        Self::Spans(vec![(text.into(), tone)])
    }

    fn natural(&self) -> usize {
        match self {
            Self::Spans(spans) => spans.iter().map(|(text, _)| width(text)).sum(),
            Self::Items(items, _) => {
                items.iter().map(|item| width(item)).sum::<usize>()
                    + items.len().saturating_sub(1) * 2
            }
        }
    }

    /// Plain-text lines that fit `limit`, each with its tone fragments.
    fn lines(&self, limit: usize, unicode: bool) -> Vec<Vec<(String, Tone)>> {
        match self {
            Self::Spans(spans) => {
                if spans.iter().map(|(text, _)| width(text)).sum::<usize>() <= limit {
                    vec![spans.clone()]
                } else {
                    let joined: String = spans.iter().map(|(text, _)| text.as_str()).collect();
                    vec![vec![(truncate(&joined, limit, unicode), spans[0].1)]]
                }
            }
            Self::Items(items, tone) => {
                let mut lines: Vec<String> = Vec::new();
                let mut current = String::new();
                for (index, item) in items.iter().enumerate() {
                    let last = index + 1 == items.len();
                    let piece = if last {
                        item.clone()
                    } else {
                        format!("{item},")
                    };
                    let piece = truncate(&piece, limit, unicode);
                    if current.is_empty() {
                        current = piece;
                    } else if width(&current) + 1 + width(&piece) <= limit {
                        current.push(' ');
                        current.push_str(&piece);
                    } else {
                        lines.push(std::mem::take(&mut current));
                        current = piece;
                    }
                }
                if !current.is_empty() || lines.is_empty() {
                    lines.push(current);
                }
                lines.into_iter().map(|line| vec![(line, *tone)]).collect()
            }
        }
    }
}

fn truncate(text: &str, limit: usize, unicode: bool) -> String {
    if width(text) <= limit {
        return text.to_owned();
    }
    let ellipsis = if unicode { "…" } else { "~" };
    let mut out = String::new();
    for character in text.chars() {
        let next = format!("{out}{character}");
        if width(&next) + width(ellipsis) > limit {
            break;
        }
        out = next;
    }
    out.push_str(ellipsis);
    out
}

pub(crate) struct BoxTable {
    columns: Vec<Column>,
    rows: Vec<(Vec<Cell>, bool)>,
}

impl BoxTable {
    pub(crate) fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, cells: Vec<Cell>) {
        self.rows.push((cells, false));
    }

    /// A summary row, set off by a rule and drawn bold.
    pub(crate) fn push_total(&mut self, cells: Vec<Cell>) {
        self.rows.push((cells, true));
    }

    fn natural_widths(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                self.rows
                    .iter()
                    .filter_map(|(cells, _)| cells.get(index))
                    .map(Cell::natural)
                    .chain([width(column.header)])
                    .max()
                    .unwrap_or_default()
            })
            .collect()
    }

    /// Lays the table out within `max`. Wrapping columns shrink first; with
    /// `drop`, optional columns then go. `None` when it still does not fit.
    pub(crate) fn render(
        &self,
        theme: &Theme,
        max: Option<usize>,
        drop: bool,
    ) -> Option<Vec<String>> {
        let mut widths = self.natural_widths();
        let mut visible: Vec<bool> = vec![true; self.columns.len()];
        let total = |widths: &[usize], visible: &[bool]| {
            1 + widths
                .iter()
                .zip(visible)
                .filter(|(_, shown)| **shown)
                .map(|(width, _)| width + 3)
                .sum::<usize>()
        };
        if let Some(max) = max {
            loop {
                let mut excess = total(&widths, &visible).saturating_sub(max);
                if excess == 0 {
                    break;
                }
                // Take a column at a time from whichever list has the most
                // slack, so the widest one wraps first.
                while excess > 0 {
                    let widest = self
                        .columns
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| visible[*index])
                        .filter_map(|(index, column)| {
                            let floor = column.wrap?.max(width(column.header));
                            let slack = widths[index].saturating_sub(floor);
                            (slack > 0).then_some((slack, index))
                        })
                        .max();
                    let Some((_, index)) = widest else { break };
                    widths[index] -= 1;
                    excess -= 1;
                }
                if excess == 0 {
                    break;
                }
                if !drop {
                    return None;
                }
                let candidate = self
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(index, column)| visible[*index] && column.drop > 0)
                    .max_by_key(|(index, column)| (column.drop, *index))
                    .map(|(index, _)| index);
                match candidate {
                    Some(index) => visible[index] = false,
                    None => break,
                }
            }
        }
        Some(self.draw(theme, &widths, &visible))
    }

    fn draw(&self, theme: &Theme, widths: &[usize], visible: &[bool]) -> Vec<String> {
        let unicode = theme.unicode();
        let shown: Vec<usize> = (0..self.columns.len()).filter(|i| visible[*i]).collect();
        let rule = |left: &str, middle: &str, right: &str| {
            let horizontal = if unicode { "─" } else { "-" };
            let body = shown
                .iter()
                .map(|index| horizontal.repeat(widths[*index] + 2))
                .collect::<Vec<_>>()
                .join(middle);
            theme.paint(Tone::Dim, &format!("{left}{body}{right}"))
        };
        let (top, mid, bottom) = if unicode {
            (("╭", "┬", "╮"), ("├", "┼", "┤"), ("╰", "┴", "╯"))
        } else {
            (("+", "+", "+"), ("+", "+", "+"), ("+", "+", "+"))
        };
        let bar = theme.paint(Tone::Dim, if unicode { "│" } else { "|" });
        let line = |fragments: Vec<Vec<(String, Tone)>>, bold: bool| {
            let mut out = bar.clone();
            for (slot, index) in shown.iter().enumerate() {
                let spans = &fragments[slot];
                let used: usize = spans.iter().map(|(text, _)| width(text)).sum();
                let padding = " ".repeat(widths[*index].saturating_sub(used));
                let text: String = spans
                    .iter()
                    .map(|(text, tone)| {
                        let tone = if bold && *tone == Tone::Plain {
                            Tone::Head
                        } else {
                            *tone
                        };
                        theme.paint(tone, text)
                    })
                    .collect();
                let cell = match self.columns[*index].align {
                    Align::Left => format!("{text}{padding}"),
                    Align::Right => format!("{padding}{text}"),
                };
                out.push_str(&format!(" {cell} {bar}"));
            }
            out
        };

        let mut lines = vec![rule(top.0, top.1, top.2)];
        lines.push(line(
            shown
                .iter()
                .map(|index| vec![(self.columns[*index].header.to_owned(), Tone::Head)])
                .collect(),
            false,
        ));
        lines.push(rule(mid.0, mid.1, mid.2));
        for (cells, is_total) in &self.rows {
            if *is_total {
                lines.push(rule(mid.0, mid.1, mid.2));
            }
            let wrapped: Vec<Vec<Vec<(String, Tone)>>> = shown
                .iter()
                .map(|index| {
                    cells
                        .get(*index)
                        .map(|cell| cell.lines(widths[*index], unicode))
                        .unwrap_or_else(|| vec![Vec::new()])
                })
                .collect();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
            for row in 0..height {
                lines.push(line(
                    wrapped
                        .iter()
                        .map(|cell| cell.get(row).cloned().unwrap_or_default())
                        .collect(),
                    *is_total,
                ));
            }
        }
        lines.push(rule(bottom.0, bottom.1, bottom.2));
        lines
    }
}

/// Renders the first build that fits: full numbers, then compact numbers,
/// then compact numbers with optional columns dropped.
pub(crate) fn fit(theme: &Theme, max: Option<usize>, build: impl Fn(bool) -> BoxTable) -> String {
    let available = max.map(|max| max.saturating_sub(width(INDENT)));
    let lines = build(false)
        .render(theme, available, false)
        .or_else(|| build(true).render(theme, available, false))
        .or_else(|| build(true).render(theme, available, true))
        .unwrap_or_default();
    lines
        .into_iter()
        .map(|line| format!("{INDENT}{line}\n"))
        .collect()
}

/// `1.23M`-style, three significant digits.
pub(crate) fn compact(value: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1_000_000_000_000, "T"),
        (1_000_000_000, "B"),
        (1_000_000, "M"),
        (1_000, "K"),
    ];
    for (scale, suffix) in UNITS {
        if value >= scale {
            let scaled = value as f64 / scale as f64;
            let text = if scaled >= 100.0 {
                format!("{scaled:.0}")
            } else if scaled >= 10.0 {
                format!("{scaled:.1}")
            } else {
                format!("{scaled:.2}")
            };
            return format!("{text}{suffix}");
        }
    }
    value.to_string()
}

pub(crate) fn count(value: u64, short: bool) -> String {
    if short {
        compact(value)
    } else {
        format_count(value)
    }
}

/// Whole cents with separators; a nonzero amount under a cent stays visible.
pub(crate) fn usd(money: Money) -> String {
    const PICO_PER_CENT: u128 = 10_000_000_000;
    let pico = money.pico_usd();
    let cents = (pico + PICO_PER_CENT / 2) / PICO_PER_CENT;
    if pico > 0 && cents == 0 {
        return "<$0.01".to_owned();
    }
    let dollars = u64::try_from(cents / 100).unwrap_or(u64::MAX);
    format!("${}.{:02}", format_count(dollars), cents % 100)
}

/// `claude-opus-4-5-20251101` reads as `opus-4-5`; other IDs are kept.
pub(crate) fn short_model(model: &str) -> String {
    let model = model.strip_prefix("claude-").unwrap_or(model);
    match model.rsplit_once('-') {
        Some((head, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => {
            head.to_owned()
        }
        _ => model.to_owned(),
    }
}

/// How far to trust one cell's sum.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Exact,
    /// Part of what it sums is unknown.
    AtLeast,
    /// Two uncorrelated histories may both hold the same requests.
    Approximate,
}

impl Mark {
    fn of(overlap: bool, incomplete: bool) -> Self {
        if overlap {
            Self::Approximate
        } else if incomplete {
            Self::AtLeast
        } else {
            Self::Exact
        }
    }

    fn cell(self, text: String) -> Cell {
        match self {
            Self::Exact => Cell::plain(text),
            Self::AtLeast => Cell::Spans(vec![(text, Tone::Plain), ("+".to_owned(), Tone::Warn)]),
            Self::Approximate => {
                Cell::Spans(vec![("~".to_owned(), Tone::Warn), (text, Tone::Plain)])
            }
        }
    }
}

struct Line {
    label: String,
    agents: BTreeSet<Agent>,
    models: BTreeSet<String>,
    requests: u64,
    known: KnownTokens,
    cost: Money,
    priced: bool,
    unpriced: bool,
    overlap: bool,
}

impl Line {
    fn from_rollup(row: &DailyRollup) -> Self {
        Self {
            label: row.period.clone(),
            agents: row.agents.clone(),
            models: row.models.clone(),
            requests: row.known_requests,
            known: row.known_tokens,
            cost: row.known_cost,
            priced: row.priced_records + row.partial_records > 0,
            unpriced: row.unpriced_records > 0,
            overlap: row.possible_overlap,
        }
    }

    fn token_cells(&self, short: bool) -> Vec<Cell> {
        let mark = Mark::of(self.overlap, self.known.incomplete);
        vec![
            Cell::plain(count(self.known.uncached_input, short)),
            Cell::plain(count(self.known.output, short)),
            Cell::plain(count(self.known.cache_write, short)),
            Cell::plain(count(self.known.cache_read, short)),
            mark.cell(count(self.known.total(), short)),
        ]
    }

    fn cost_cell(&self, theme: &Theme) -> Cell {
        if !self.priced {
            return Cell::toned(theme.dash(), Tone::Dim);
        }
        Mark::of(self.overlap, self.unpriced).cell(usd(self.cost))
    }
}

fn models(models: &BTreeSet<String>) -> Vec<String> {
    let short: BTreeSet<String> = models
        .iter()
        .filter(|model| !model.starts_with('<'))
        .map(|model| short_model(model))
        .collect();
    short.into_iter().collect()
}

fn agents(agents: &BTreeSet<Agent>) -> Vec<String> {
    agents
        .iter()
        .map(|agent| agent.as_str().to_owned())
        .collect()
}

fn snapshot_name(snapshot: &str) -> &str {
    snapshot.split(":sha256:").next().unwrap_or(snapshot)
}

fn subtitle(report: &Statistics, theme: &Theme) -> String {
    let date = |label: &Option<String>| {
        label
            .as_deref()
            .map(|text| text.chars().take(10).collect::<String>())
    };
    let range = match (date(&report.range.start), date(&report.range.end)) {
        (None, None) => "all history".to_owned(),
        (Some(start), None) => format!("since {start}"),
        (None, Some(end)) => format!("before {end}"),
        (Some(start), Some(end)) => format!("{start} to {end} (exclusive)"),
    };
    theme.paint(
        Tone::Dim,
        &format!(
            "{range} · {} · {} records · prices {}",
            report.timezone,
            format_count(report.records),
            snapshot_name(&report.pricing_snapshot)
        ),
    )
}

pub(crate) fn render_statistics(report: &Statistics, theme: &Theme, view: View) -> String {
    let mut out = heading_text(theme, "Token usage");
    out.push_str(&format!("{INDENT}{}\n", subtitle(report, theme)));
    if report.records == 0 {
        out.push_str(&format!(
            "{INDENT}{}\n",
            theme.paint(Tone::Dim, "no matching usage records")
        ));
        return out;
    }

    let periods = if view.monthly {
        &report.monthly
    } else {
        &report.daily
    };
    let lines: Vec<Line> = periods.iter().map(Line::from_rollup).collect();
    let overall = Line {
        label: "Total".to_owned(),
        agents: BTreeSet::new(),
        models: BTreeSet::new(),
        requests: report.rows.iter().map(|row| row.known_requests).sum(),
        known: report.known_tokens,
        cost: report.known_cost,
        priced: report.priced_records > 0 || report.rows.iter().any(|row| row.partial_records > 0),
        unpriced: report.unpriced_records > 0,
        overlap: report.possible_overlap,
    };
    out.push_str(&fit(theme, view.width, |short| {
        let mut table = BoxTable::new(vec![
            Column::left(if view.monthly { "Month" } else { "Date" }),
            Column::left("Agents").wrap(6).optional(2),
            Column::left("Models").wrap(12).optional(3),
            Column::right("Input"),
            Column::right("Output"),
            Column::right("Cache write").optional(1),
            Column::right("Cache read").optional(1),
            Column::right("Total tokens"),
            Column::right("Cost"),
        ]);
        let row = |line: &Line| {
            let mut cells = vec![
                Cell::plain(line.label.clone()),
                Cell::Items(agents(&line.agents), Tone::Plain),
                Cell::Items(models(&line.models), Tone::Dim),
            ];
            cells.extend(line.token_cells(short));
            cells.push(line.cost_cell(theme));
            cells
        };
        for line in &lines {
            table.push(row(line));
        }
        table.push_total(row(&overall));
        table
    }));

    out.push_str(&heading_text(theme, "By model"));
    let by_model = by_model(report);
    let grand = report.known_tokens.total().max(1);
    out.push_str(&fit(theme, view.width, |short| {
        let mut table = BoxTable::new(vec![
            Column::left("Agent"),
            Column::left("Model").wrap(12),
            Column::right("Requests").optional(2),
            Column::right("Input"),
            Column::right("Output"),
            Column::right("Cache write").optional(1),
            Column::right("Cache read").optional(1),
            Column::right("Total tokens"),
            Column::left("Share").optional(3),
            Column::right("Cost"),
        ]);
        for line in &by_model {
            let mut cells = vec![
                Cell::plain(line.label.clone()),
                Cell::Items(models(&line.models), Tone::Plain),
                Cell::plain(if line.requests == 0 {
                    theme.dash().to_owned()
                } else {
                    count(line.requests, short)
                }),
            ];
            let mut tokens = line.token_cells(short);
            let share = tokens.pop().map(|total| (total, line.known.total()));
            cells.append(&mut tokens);
            if let Some((total, value)) = share {
                cells.push(total);
                cells.push(share_cell(value, grand, theme));
            }
            cells.push(line.cost_cell(theme));
            table.push(cells);
        }
        table
    }));

    out.push_str(&legend(report, &lines, &by_model, theme));
    out
}

fn share_cell(value: u64, total: u64, theme: &Theme) -> Cell {
    let percent = (u128::from(value) * 100 / u128::from(total)) as u64;
    let text = if percent == 0 && value > 0 {
        "<1%".to_owned()
    } else {
        format!("{percent}%")
    };
    if !theme.unicode() {
        return Cell::toned(format!("{text:>4}"), Tone::Dim);
    }
    // Eighth blocks give an 8-cell bar 64 steps of resolution.
    let eighths = (u128::from(value) * 64 / u128::from(total)) as usize;
    let mut bar = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        bar.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][eighths % 8 - 1]);
    }
    let pad = 8 - width(&bar);
    Cell::Spans(vec![
        (bar, Tone::Good),
        (" ".repeat(pad), Tone::Plain),
        (format!(" {text:>4}"), Tone::Dim),
    ])
}

/// One line per agent and model, merging sources, profiles and granularity;
/// largest first.
fn by_model(report: &Statistics) -> Vec<Line> {
    let mut groups = BTreeMap::<(Agent, String), Line>::new();
    for row in &report.rows {
        let model = row.model.clone().unwrap_or_else(|| "unknown".to_owned());
        let line = groups
            .entry((row.agent, model.clone()))
            .or_insert_with(|| Line {
                label: row.agent.as_str().to_owned(),
                agents: BTreeSet::from([row.agent]),
                models: BTreeSet::from([model]),
                requests: 0,
                known: KnownTokens::default(),
                cost: Money::ZERO,
                priced: false,
                unpriced: false,
                overlap: false,
            });
        line.requests += row.known_requests;
        line.known.add(&row.known_tokens);
        line.cost = line.cost.checked_add(row.known_cost).unwrap_or(line.cost);
        line.priced |= row.priced_records + row.partial_records > 0;
        line.unpriced |= row.unpriced_records > 0;
        line.overlap |= row.possible_overlap;
    }
    let mut lines: Vec<Line> = groups
        .into_values()
        .filter(|line| line.known.total() > 0)
        .collect();
    lines.sort_by(|a, b| {
        b.known
            .total()
            .cmp(&a.known.total())
            .then_with(|| a.label.cmp(&b.label))
    });
    lines
}

fn legend(report: &Statistics, periods: &[Line], models: &[Line], theme: &Theme) -> String {
    let mut notes = Vec::new();
    let all = || periods.iter().chain(models);
    if all().any(|line| !line.overlap && (line.known.incomplete || line.unpriced)) {
        notes.push(format!(
            "{}  at least: some records carry no token counts or no price",
            theme.paint(Tone::Warn, "+")
        ));
    }
    if all().any(|line| line.overlap) {
        notes.push(format!(
            "{}  approximate: alc's ledger and a native history both saw this agent and may count a request twice; --source picks one",
            theme.paint(Tone::Warn, "~")
        ));
    }
    let unpriced: BTreeSet<String> = report
        .rows
        .iter()
        .filter(|row| row.priced_records + row.partial_records == 0)
        .filter_map(|row| row.model.as_deref())
        .filter(|model| !model.starts_with('<'))
        .map(short_model)
        .collect();
    if !unpriced.is_empty() {
        let shown: Vec<_> = unpriced.iter().take(6).cloned().collect();
        let more = unpriced.len().saturating_sub(shown.len());
        notes.push(format!(
            "{}  no price in the snapshot for {}{}; add rates with --pricing-file",
            theme.paint(Tone::Dim, theme.dash()),
            shown.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ));
    }
    if report.coverage_incomplete {
        notes
            .push("Some history lines were skipped or ambiguous, so totals can be low.".to_owned());
    }
    notes.push(
        "Costs are API-equivalent estimates, not bills. --details explains every row; --json has the exact figures."
            .to_owned(),
    );
    let mut out = String::from("\n");
    for note in notes {
        out.push_str(&format!("{INDENT}{}\n", theme.paint(Tone::Dim, &note)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_keeps_three_significant_digits() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_234), "1.23K");
        assert_eq!(compact(45_600_000), "45.6M");
        assert_eq!(compact(1_511_132_672), "1.51B");
        assert_eq!(compact(312_000_000_000), "312B");
    }

    #[test]
    fn usd_rounds_to_cents_without_hiding_small_amounts() {
        assert_eq!(usd(Money::ZERO), "$0.00");
        assert_eq!(usd(Money::from_pico_usd(1)), "<$0.01");
        assert_eq!(
            usd(Money::from_pico_usd(1_234_567_000_000_000)),
            "$1,234.57"
        );
    }

    #[test]
    fn model_names_lose_vendor_prefix_and_snapshot_date() {
        assert_eq!(short_model("claude-opus-4-5-20251101"), "opus-4-5");
        assert_eq!(short_model("claude-sonnet-4-6"), "sonnet-4-6");
        assert_eq!(short_model("gpt-5.1-codex"), "gpt-5.1-codex");
    }

    fn table() -> BoxTable {
        let mut table = BoxTable::new(vec![
            Column::left("Date"),
            Column::left("Models").wrap(6).optional(2),
            Column::right("Tokens"),
            Column::right("Extra").optional(1),
        ]);
        table.push(vec![
            Cell::plain("2026-01-02"),
            Cell::Items(vec!["opus-4-5".into(), "gpt-5.1-codex".into()], Tone::Plain),
            Cell::plain("1,234,567"),
            Cell::plain("x"),
        ]);
        table.push_total(vec![
            Cell::plain("Total"),
            Cell::Items(Vec::new(), Tone::Plain),
            Cell::plain("1,234,567"),
            Cell::plain("x"),
        ]);
        table
    }

    #[test]
    fn box_table_draws_rounded_borders_and_a_total_rule() {
        let theme = Theme::for_test(false, true);
        let lines = table().render(&theme, None, false).unwrap();
        assert!(lines[0].starts_with('╭') && lines[0].ends_with('╮'));
        assert!(lines.last().unwrap().starts_with('╰'));
        assert!(lines[1].contains("Date") && lines[1].contains("Tokens"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("opus-4-5, gpt-5.1-codex"))
        );
        let total = lines
            .iter()
            .position(|line| line.contains("Total"))
            .unwrap();
        assert!(lines[total - 1].starts_with('├'));
        let widths: BTreeSet<usize> = lines.iter().map(|line| width(line)).collect();
        assert_eq!(widths.len(), 1, "{lines:#?}");
    }

    #[test]
    fn narrow_tables_wrap_lists_then_drop_optional_columns() {
        let theme = Theme::for_test(false, false);
        let wrapped = table().render(&theme, Some(50), false).unwrap();
        assert!(wrapped.iter().all(|line| width(line) <= 50), "{wrapped:#?}");
        assert!(wrapped.iter().any(|line| line.contains("opus-4-5,")));
        assert!(wrapped.iter().any(|line| line.contains("gpt-5.1-codex")));
        assert!(table().render(&theme, Some(28), false).is_none());
        let dropped = table().render(&theme, Some(28), true).unwrap();
        assert!(dropped.iter().all(|line| width(line) <= 28), "{dropped:#?}");
        assert!(!dropped[1].contains("Extra"));
    }

    #[test]
    fn markers_distinguish_lower_bounds_from_overlap() {
        let text = |cell: Cell| match cell {
            Cell::Spans(spans) => spans.into_iter().map(|(text, _)| text).collect::<String>(),
            Cell::Items(..) => unreachable!(),
        };
        assert_eq!(text(Mark::of(false, false).cell("5".into())), "5");
        assert_eq!(text(Mark::of(false, true).cell("5".into())), "5+");
        assert_eq!(text(Mark::of(true, true).cell("5".into())), "~5");
    }
}
