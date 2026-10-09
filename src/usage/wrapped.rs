//! `alc usage --wrapped`: one shareable image of the selected usage across
//! every agent and provider alc can read, in the spirit of a year-in-review.
//!
//! Figures are the known sums the terminal view shows (see
//! [`super::query::KnownTokens`]); the footer says when they are lower bounds.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};

use anyhow::{Context, Result};
use chrono::{Datelike, Duration, NaiveDate};
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::HPos;

use super::chart::write_png;
use super::pricing::Money;
use super::query::{KnownTokens, Statistics};
use super::view::{compact, short_model, usd};
use crate::config::Agent;

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 1440;
const MARGIN: i32 = 64;
const GAP: i32 = 24;

// The dataviz reference instance: light surfaces, ink, and the blue ramp.
const PAGE: RGBColor = RGBColor(249, 249, 247);
const CARD: RGBColor = RGBColor(252, 252, 251);
const BORDER: RGBColor = RGBColor(225, 224, 217);
const INK: RGBColor = RGBColor(11, 11, 11);
const SECONDARY: RGBColor = RGBColor(82, 81, 78);
const MUTED: RGBColor = RGBColor(137, 135, 129);
const EMPTY: RGBColor = RGBColor(238, 237, 232);
const BLUE: [RGBColor; 6] = [
    RGBColor(183, 211, 246),
    RGBColor(134, 182, 239),
    RGBColor(85, 152, 231),
    RGBColor(42, 120, 214),
    RGBColor(28, 92, 171),
    RGBColor(16, 66, 129),
];
const ACCENT: RGBColor = BLUE[3];
const ACCENT_STRONG: RGBColor = BLUE[5];
/// Fixed categorical slots, by `Agent` order: color follows the agent, never
/// its rank, so the same agent keeps its color across images.
const AGENT_COLORS: [RGBColor; 8] = [
    RGBColor(42, 120, 214),
    RGBColor(235, 104, 52),
    RGBColor(27, 175, 122),
    RGBColor(237, 161, 0),
    RGBColor(232, 123, 164),
    RGBColor(0, 131, 0),
    RGBColor(74, 58, 167),
    RGBColor(227, 73, 72),
];

type Area<'a> = DrawingArea<BitMapBackend<'a>, Shift>;

pub(super) fn export(report: &Statistics, path: &Path, today: NaiveDate) -> Result<()> {
    let summary = Summary::new(report);
    write_png(path, |staged| render(&summary, staged, today))
}

/// Everything the image shows, computed once so drawing stays layout only.
struct Summary {
    days: Vec<(NaiveDate, u64)>,
    known: KnownTokens,
    overlap: bool,
    agents: Vec<(Agent, u64)>,
    models: Vec<(String, u64)>,
    providers: Vec<(String, u64)>,
    hours: [u64; 24],
    requests: u64,
    sessions: u64,
    cost: Money,
    priced: bool,
    unpriced: bool,
    records: u64,
    timezone: String,
}

impl Summary {
    fn new(report: &Statistics) -> Self {
        let days = report
            .daily
            .iter()
            .filter_map(|day| {
                let date = NaiveDate::parse_from_str(&day.period, "%Y-%m-%d").ok()?;
                Some((date, day.known_tokens.total()))
            })
            .collect();
        let mut agents = BTreeMap::<Agent, u64>::new();
        let mut models = BTreeMap::<String, u64>::new();
        for row in &report.rows {
            let total = row.known_tokens.total();
            *agents.entry(row.agent).or_default() += total;
            if let Some(model) = row.model.as_deref().filter(|model| !model.starts_with('<')) {
                *models.entry(short_model(model)).or_default() += total;
            }
        }
        let ranked = |map: BTreeMap<String, u64>| {
            let mut list: Vec<_> = map.into_iter().filter(|(_, total)| *total > 0).collect();
            list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            list
        };
        let mut agents: Vec<_> = agents.into_iter().filter(|(_, total)| *total > 0).collect();
        agents.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Self {
            days,
            known: report.known_tokens,
            overlap: report.possible_overlap,
            agents,
            models: ranked(models),
            providers: ranked(report.activity.providers.clone()),
            hours: report.activity.hours,
            requests: report.rows.iter().map(|row| row.known_requests).sum(),
            sessions: report.activity.sessions,
            cost: report.known_cost,
            priced: report
                .rows
                .iter()
                .any(|row| row.priced_records + row.partial_records > 0),
            unpriced: report.unpriced_records > 0,
            records: report.records,
            timezone: report.timezone.clone(),
        }
    }

    fn active(&self) -> impl Iterator<Item = &(NaiveDate, u64)> {
        self.days.iter().filter(|(_, total)| *total > 0)
    }

    fn longest_streak(&self) -> u32 {
        let mut best = 0;
        let mut run = 0;
        let mut previous: Option<NaiveDate> = None;
        for (date, _) in self.active() {
            run = match previous {
                Some(before) if *date - before == Duration::days(1) => run + 1,
                _ => 1,
            };
            best = best.max(run);
            previous = Some(*date);
        }
        best
    }

    fn weekdays(&self) -> [u64; 7] {
        let mut totals = [0u64; 7];
        for (date, total) in &self.days {
            let slot = &mut totals[date.weekday().num_days_from_monday() as usize];
            *slot = slot.saturating_add(*total);
        }
        totals
    }
}

fn render(summary: &Summary, path: &Path, today: NaiveDate) -> Result<()> {
    let root = BitMapBackend::new(path, (WIDTH, HEIGHT)).into_drawing_area();
    root.fill(&PAGE)?;
    // A soft disc behind the title, the only decoration.
    root.draw(&Circle::new(
        (WIDTH as i32 - 120, 40),
        260,
        RGBColor(232, 240, 251).filled(),
    ))?;

    header(&root, summary)?;
    let inner = WIDTH as i32 - 2 * MARGIN;
    let quarter = (inner - 3 * GAP) / 4;
    let column = |index: i32| MARGIN + index * (quarter + GAP);

    let top = 168;
    let height = 214;
    hero_card(&root, (column(0), top, quarter, height), summary)?;
    started_card(&root, (column(1), top, quarter, height), summary, today)?;
    busiest_card(&root, (column(2), top, quarter, height), summary)?;
    weekly_card(&root, (column(3), top, quarter, height), summary)?;

    heatmap_card(&root, (MARGIN, top + height + GAP, inner, 300), summary)?;

    let third = (inner - 2 * GAP) / 3;
    let lists = top + height + GAP + 300 + GAP;
    let list_height = 330;
    agents_card(&root, (MARGIN, lists, third, list_height), summary)?;
    ranked_card(
        &root,
        (MARGIN + third + GAP, lists, third, list_height),
        "TOP MODELS",
        &summary.models,
    )?;
    ranked_card(
        &root,
        (MARGIN + 2 * (third + GAP), lists, third, list_height),
        "PROVIDERS",
        &summary.providers,
    )?;

    let tiles = lists + list_height + GAP;
    let tile_height = 128;
    let cache = cache_hit(&summary.known);
    let peak = summary
        .hours
        .iter()
        .enumerate()
        .max_by_key(|(hour, total)| (**total, std::cmp::Reverse(*hour)))
        .filter(|(_, total)| **total > 0)
        .map(|(hour, _)| format!("{hour:02}:00"));
    // The terminal view's markers: `~` may double count, `+` is at least.
    let cost = if !summary.priced {
        "—".to_owned()
    } else if summary.overlap {
        format!("~{}", usd(summary.cost))
    } else if summary.unpriced {
        format!("{}+", usd(summary.cost))
    } else {
        usd(summary.cost)
    };
    let stats: [(&str, String); 8] = [
        ("REQUESTS", grouped(summary.requests)),
        ("SESSIONS", grouped(summary.sessions)),
        ("ACTIVE DAYS", grouped(summary.active().count() as u64)),
        ("LONGEST STREAK", format!("{}d", summary.longest_streak())),
        ("CACHE HIT", cache.unwrap_or_else(|| "—".to_owned())),
        ("OUTPUT TOKENS", compact(summary.known.output)),
        ("PEAK HOUR", peak.unwrap_or_else(|| "—".to_owned())),
        ("EST. COST", cost),
    ];
    for (index, (label, value)) in stats.iter().enumerate() {
        let row = index as i32 / 4;
        let x = column(index as i32 % 4);
        let y = tiles + row * (tile_height + GAP / 2 + 4);
        tile(&root, (x, y, quarter, tile_height), label, value)?;
    }

    footer(&root, summary)?;
    root.present().context("failed to encode PNG")?;
    Ok(())
}

fn header(root: &Area<'_>, summary: &Summary) -> Result<()> {
    text(root, "alc", (MARGIN, 64), 76, ACCENT, HPos::Left)?;
    let word = measure(face()?, "alc ", PxScale::from(76.0)).round() as i32;
    text(root, "wrapped", (MARGIN + word, 64), 76, INK, HPos::Left)?;
    let range = match (summary.days.first(), summary.days.last()) {
        (Some((first, _)), Some((last, _))) => format!(
            "{} – {}",
            first.format("%b %-d, %Y"),
            last.format("%b %-d, %Y")
        ),
        _ => "no usage recorded".to_owned(),
    };
    let right = WIDTH as i32 - MARGIN;
    text(root, &range, (right, 62), 30, INK, HPos::Right)?;
    let agents = summary.agents.len();
    let providers = summary.providers.len();
    text(
        root,
        &format!(
            "{agents} agent{} · {providers} provider{} · {} model{} · {}",
            plural(agents),
            plural(providers),
            summary.models.len(),
            plural(summary.models.len()),
            summary.timezone
        ),
        (right, 104),
        19,
        SECONDARY,
        HPos::Right,
    )
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn hero_card(root: &Area<'_>, (x, y, w, h): (i32, i32, i32, i32), summary: &Summary) -> Result<()> {
    card(root, x, y, w, h, "TOTAL TOKENS")?;
    let mark = if summary.overlap {
        "~"
    } else if summary.known.incomplete {
        "+"
    } else {
        ""
    };
    text(
        root,
        &if mark == "+" {
            format!("{}+", compact(summary.known.total()))
        } else {
            format!("{mark}{}", compact(summary.known.total()))
        },
        (x + 28, y + 104),
        64,
        INK,
        HPos::Left,
    )?;
    text(
        root,
        &format!(
            "in {} · out {}",
            compact(summary.known.input()),
            compact(summary.known.output)
        ),
        (x + 28, y + 166),
        19,
        SECONDARY,
        HPos::Left,
    )
}

fn started_card(
    root: &Area<'_>,
    (x, y, w, h): (i32, i32, i32, i32),
    summary: &Summary,
    today: NaiveDate,
) -> Result<()> {
    card(root, x, y, w, h, "STARTED")?;
    let Some((first, _)) = summary.active().next() else {
        return text(root, "—", (x + 28, y + 80), 48, INK, HPos::Left);
    };
    text(
        root,
        &first.format("%B %-d, %Y").to_string(),
        (x + 28, y + 74),
        24,
        SECONDARY,
        HPos::Left,
    )?;
    let ago = (today - *first).num_days().max(0);
    text(
        root,
        &format!("{ago} day{} ago", if ago == 1 { "" } else { "s" }),
        (x + 28, y + 118),
        44,
        INK,
        HPos::Left,
    )
}

fn busiest_card(
    root: &Area<'_>,
    (x, y, w, h): (i32, i32, i32, i32),
    summary: &Summary,
) -> Result<()> {
    card(root, x, y, w, h, "MOST ACTIVE DAY")?;
    let Some((date, total)) = summary
        .active()
        .max_by_key(|(date, total)| (*total, std::cmp::Reverse(*date)))
    else {
        return text(root, "—", (x + 28, y + 80), 48, INK, HPos::Left);
    };
    text(
        root,
        &date.format("%A").to_string(),
        (x + 28, y + 74),
        24,
        SECONDARY,
        HPos::Left,
    )?;
    text(
        root,
        &date.format("%b %-d").to_string(),
        (x + 28, y + 108),
        48,
        INK,
        HPos::Left,
    )?;
    text(
        root,
        &format!("{} tokens", compact(*total)),
        (x + 28, y + 172),
        18,
        MUTED,
        HPos::Left,
    )
}

fn weekly_card(
    root: &Area<'_>,
    (x, y, w, h): (i32, i32, i32, i32),
    summary: &Summary,
) -> Result<()> {
    card(root, x, y, w, h, "WEEKLY")?;
    let totals = summary.weekdays();
    let max = totals.iter().copied().max().unwrap_or(0).max(1);
    let peak = totals
        .iter()
        .enumerate()
        .max_by_key(|(index, total)| (**total, std::cmp::Reverse(*index)))
        .map(|(index, _)| index);
    let left = x + 28;
    let slot = (w - 56) / 7;
    let base = y + h - 46;
    let tallest = h - 120;
    for (index, (total, day)) in totals
        .iter()
        .zip(["M", "T", "W", "T", "F", "S", "S"])
        .enumerate()
    {
        let bar = ((*total as f64 / max as f64) * tallest as f64).round() as i32;
        let bar = if *total > 0 { bar.max(4) } else { 0 };
        let color = if Some(index) == peak && *total > 0 {
            ACCENT_STRONG
        } else {
            BLUE[2]
        };
        let center = left + slot * index as i32 + slot / 2;
        rounded(
            root,
            center - slot / 2 + 5,
            base - bar,
            center + slot / 2 - 5,
            base,
            4,
            color,
        )?;
        text(
            root,
            day,
            (center, base + 12),
            15,
            if Some(index) == peak && *total > 0 {
                INK
            } else {
                MUTED
            },
            HPos::Center,
        )?;
    }
    Ok(())
}

fn heatmap_card(
    root: &Area<'_>,
    (x, y, w, h): (i32, i32, i32, i32),
    summary: &Summary,
) -> Result<()> {
    card(root, x, y, w, h, "ACTIVITY")?;
    let (Some((first, _)), Some((last, _))) = (summary.days.first(), summary.days.last()) else {
        return text(
            root,
            "no usage recorded",
            (x + 28, y + 140),
            20,
            MUTED,
            HPos::Left,
        );
    };
    let available = w - 56 - 36;
    let top = y + 96;
    // Seven rows and the legend must fit the card; long histories keep the
    // most recent weeks that fit rather than spilling past the edge.
    let tallest = ((h - 96 - 44) / 7).min(26);
    let narrowest = 8;
    let monday =
        |date: NaiveDate| date - Duration::days(i64::from(date.weekday().num_days_from_monday()));
    let mut start = monday(*first);
    let mut weeks = ((*last - start).num_days() / 7 + 1).max(1) as i32;
    let fits = (available / narrowest).max(1);
    if weeks > fits {
        start = monday(*last) - Duration::days(i64::from(fits - 1) * 7);
        weeks = fits;
    }
    let shown_from = (*first).max(start);
    let pitch = (available / weeks).clamp(narrowest, tallest);
    let cell = (pitch - pitch.clamp(1, 4)).max(3);
    let left = x + 28 + 36;

    let totals: BTreeMap<NaiveDate, u64> = summary.days.iter().copied().collect();
    // Levels from the quantiles of active days, so one huge day does not
    // flatten the rest of the year into the lightest step.
    let mut active: Vec<u64> = totals.values().copied().filter(|t| *t > 0).collect();
    active.sort_unstable();
    let level = |total: u64| heat_level(&active, total);

    for (row, label) in [(0, "Mon"), (2, "Wed"), (4, "Fri")] {
        text(
            root,
            label,
            (left - 10, top + row * pitch + cell / 2),
            13,
            MUTED,
            HPos::Right,
        )?;
    }
    let mut month = None;
    let mut label_end = i32::MIN;
    for week in 0..weeks {
        let monday = start + Duration::days(i64::from(week) * 7);
        let wx = left + week * pitch;
        if month != Some(monday.month()) && wx >= label_end && wx + 30 < x + w {
            month = Some(monday.month());
            label_end = wx + 36;
            text(
                root,
                &monday.format("%b").to_string(),
                (wx, top - 26),
                14,
                MUTED,
                HPos::Left,
            )?;
        }
        for row in 0..7 {
            let date = monday + Duration::days(i64::from(row));
            if date < shown_from || date > *last {
                continue;
            }
            let color = level(totals.get(&date).copied().unwrap_or(0))
                .map(|level| BLUE[level])
                .unwrap_or(EMPTY);
            let cy = top + row * pitch;
            rounded(root, wx, cy, wx + cell, cy + cell, 3.min(cell / 3), color)?;
        }
    }
    let legend_y = y + h - 26;
    text(root, "Less", (left, legend_y), 13, MUTED, HPos::Left)?;
    let mut lx = left + 40;
    for color in std::iter::once(EMPTY).chain(BLUE) {
        rounded(root, lx, legend_y - 6, lx + 12, legend_y + 6, 3, color)?;
        lx += 16;
    }
    text(root, "More", (lx + 6, legend_y), 13, MUTED, HPos::Left)
}

/// The ramp step for a day: its rank among sorted active days, so the
/// busiest day is the darkest and equal days share a step. `None` is idle.
fn heat_level(active: &[u64], total: u64) -> Option<usize> {
    if total == 0 || active.is_empty() {
        return None;
    }
    let rank = active.partition_point(|value| *value <= total).max(1);
    Some(
        (rank * BLUE.len())
            .div_ceil(active.len())
            .clamp(1, BLUE.len())
            - 1,
    )
}

fn agents_card(
    root: &Area<'_>,
    (x, y, w, h): (i32, i32, i32, i32),
    summary: &Summary,
) -> Result<()> {
    card(root, x, y, w, h, "AGENTS")?;
    let total: u64 = summary
        .agents
        .iter()
        .map(|(_, total)| total)
        .sum::<u64>()
        .max(1);
    if summary.agents.is_empty() {
        return text(root, "—", (x + 28, y + 90), 30, MUTED, HPos::Left);
    }
    // A stacked share bar, then one labelled row per agent.
    let bar_left = x + 28;
    let bar_right = x + w - 28;
    let mut cursor = bar_left;
    for (index, (agent, value)) in summary.agents.iter().enumerate() {
        let end = if index + 1 == summary.agents.len() {
            bar_right
        } else {
            cursor + ((*value as f64 / total as f64) * (bar_right - bar_left) as f64) as i32
        };
        if end - cursor > 2 {
            root.draw(&Rectangle::new(
                [(cursor, y + 70), (end - 2, y + 88)],
                agent_color(*agent).filled(),
            ))?;
        }
        cursor = end;
    }
    for (index, (agent, value)) in summary.agents.iter().take(6).enumerate() {
        let row = y + 118 + index as i32 * 34;
        rounded(
            root,
            x + 28,
            row - 7,
            x + 42,
            row + 7,
            3,
            agent_color(*agent),
        )?;
        text_within(
            root,
            agent.as_str(),
            (x + 54, row),
            21,
            INK,
            HPos::Left,
            Some(w - 54 - 28 - 170),
        )?;
        text(
            root,
            &format!("{} · {}", compact(*value), percent(*value, total)),
            (x + w - 28, row),
            19,
            SECONDARY,
            HPos::Right,
        )?;
    }
    Ok(())
}

fn ranked_card(
    root: &Area<'_>,
    (x, y, w, h): (i32, i32, i32, i32),
    title: &str,
    items: &[(String, u64)],
) -> Result<()> {
    card(root, x, y, w, h, title)?;
    if items.is_empty() {
        return text(root, "—", (x + 28, y + 90), 30, MUTED, HPos::Left);
    }
    let total: u64 = items.iter().map(|(_, total)| total).sum::<u64>().max(1);
    let max = items[0].1.max(1);
    for (index, (name, value)) in items.iter().take(6).enumerate() {
        let row = y + 80 + index as i32 * 40;
        text(
            root,
            &(index + 1).to_string(),
            (x + 28, row),
            22,
            if index == 0 { ACCENT } else { MUTED },
            HPos::Left,
        )?;
        text_within(
            root,
            name,
            (x + 56, row),
            21,
            INK,
            HPos::Left,
            Some(w - 56 - 28 - 76),
        )?;
        text(
            root,
            &percent(*value, total),
            (x + w - 28, row),
            19,
            SECONDARY,
            HPos::Right,
        )?;
        let length = ((*value as f64 / max as f64) * (w - 84 - 28) as f64) as i32;
        rounded(
            root,
            x + 56,
            row + 14,
            x + 56 + length.max(3),
            row + 18,
            2,
            BLUE[1],
        )?;
    }
    Ok(())
}

fn tile(
    root: &Area<'_>,
    (x, y, w, h): (i32, i32, i32, i32),
    label: &str,
    value: &str,
) -> Result<()> {
    panel(root, x, y, w, h)?;
    text(
        root,
        label,
        (x + w / 2, y + 36),
        17,
        SECONDARY,
        HPos::Center,
    )?;
    text_within(
        root,
        value,
        (x + w / 2, y + 84),
        44,
        INK,
        HPos::Center,
        Some(w - 32),
    )
}

fn footer(root: &Area<'_>, summary: &Summary) -> Result<()> {
    let mut notes = Vec::new();
    if summary.known.incomplete || summary.unpriced {
        notes.push("+ lower bound: some records carry no token counts or no price");
    }
    if summary.overlap {
        notes.push("~ alc's ledger and native histories may overlap");
    }
    notes.push("costs are API-equivalent estimates, not bills");
    let y = HEIGHT as i32 - 46;
    text(
        root,
        &format!(
            "{} records · {}",
            grouped(summary.records),
            notes.join(" · ")
        ),
        (MARGIN, y),
        15,
        MUTED,
        HPos::Left,
    )?;
    text(
        root,
        "alc usage --wrapped · github.com/treeleaves30760/all-code",
        (WIDTH as i32 - MARGIN, y),
        15,
        SECONDARY,
        HPos::Right,
    )
}

fn agent_color(agent: Agent) -> RGBColor {
    let index = Agent::ALL
        .iter()
        .position(|candidate| *candidate == agent)
        .unwrap_or(0);
    AGENT_COLORS[index % AGENT_COLORS.len()]
}

fn cache_hit(known: &KnownTokens) -> Option<String> {
    let input = known.input();
    (input > 0).then(|| {
        let percent = known.cache_read as f64 * 100.0 / input as f64;
        format!("{percent:.1}%")
    })
}

fn percent(value: u64, total: u64) -> String {
    let share = value as f64 * 100.0 / total.max(1) as f64;
    if share > 0.0 && share < 1.0 {
        "<1%".to_owned()
    } else {
        format!("{share:.0}%")
    }
}

fn grouped(value: u64) -> String {
    super::query::format_count(value)
}

fn card(root: &Area<'_>, x: i32, y: i32, w: i32, h: i32, title: &str) -> Result<()> {
    panel(root, x, y, w, h)?;
    text(root, title, (x + 28, y + 36), 17, SECONDARY, HPos::Left)
}

fn panel(root: &Area<'_>, x: i32, y: i32, w: i32, h: i32) -> Result<()> {
    rounded(root, x, y, x + w, y + h, 14, BORDER)?;
    rounded(root, x + 1, y + 1, x + w - 1, y + h - 1, 13, CARD)
}

/// A filled rectangle with all four corners rounded.
fn rounded(
    root: &Area<'_>,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    radius: i32,
    color: RGBColor,
) -> Result<()> {
    if right <= left || bottom <= top {
        return Ok(());
    }
    let radius = radius
        .min((right - left) / 2)
        .min((bottom - top) / 2)
        .max(0);
    if radius == 0 {
        root.draw(&Rectangle::new(
            [(left, top), (right, bottom)],
            color.filled(),
        ))?;
        return Ok(());
    }
    let corners = [
        (right - radius, top + radius, -90.0),
        (right - radius, bottom - radius, 0.0),
        (left + radius, bottom - radius, 90.0),
        (left + radius, top + radius, 180.0),
    ];
    let mut points = Vec::with_capacity(4 * 9);
    for (cx, cy, start) in corners {
        for step in 0..=8 {
            let angle = (start + f64::from(step) * 90.0 / 8.0_f64).to_radians();
            points.push((
                cx + (f64::from(radius) * angle.cos()).round() as i32,
                cy + (f64::from(radius) * angle.sin()).round() as i32,
            ));
        }
    }
    root.draw(&Polygon::new(points, color.filled()))?;
    Ok(())
}

fn face() -> Result<&'static FontRef<'static>> {
    static FACE: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    FACE.get_or_init(|| {
        FontRef::try_from_slice(include_bytes!("../../assets/fonts/NotoSans.ttf")).ok()
    })
    .as_ref()
    .context("the embedded font could not be loaded")
}

fn measure(face: &FontRef<'_>, value: &str, scale: PxScale) -> f32 {
    let scaled = face.as_scaled(scale);
    let mut previous = None;
    let mut width = 0.0;
    for character in value.chars() {
        let id = scaled.glyph_id(character);
        if let Some(previous) = previous {
            width += scaled.kern(previous, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

/// Text centred vertically on `at.1`, shrunk until it fits the image.
///
/// Laid out here with `ab_glyph` rather than through plotters, whose text
/// path ignores each glyph's left side bearing and so crowds letters together.
fn text(
    root: &Area<'_>,
    value: &str,
    at: (i32, i32),
    size: u32,
    color: RGBColor,
    anchor: HPos,
) -> Result<()> {
    text_within(root, value, at, size, color, anchor, None)
}

/// [`text`] kept within `max` pixels: it shrinks to a floor, then ends in an
/// ellipsis. Characters the bundled Latin font lacks (CJK, emoji) become `?`
/// rather than empty boxes.
fn text_within(
    root: &Area<'_>,
    value: &str,
    at: (i32, i32),
    size: u32,
    color: RGBColor,
    anchor: HPos,
    max: Option<i32>,
) -> Result<()> {
    let face = face()?;
    let mut value: String = value
        .chars()
        .map(|character| {
            if character.is_whitespace() || face.glyph_id(character).0 != 0 {
                character
            } else {
                '?'
            }
        })
        .collect();
    let edge = match anchor {
        HPos::Left => WIDTH as i32 - at.0 - 8,
        HPos::Right => at.0 - 8,
        HPos::Center => WIDTH as i32,
    };
    let limit = max.map_or(edge, |max| max.min(edge)).max(24) as f32;
    let floor = (size as f32 * 0.7).max(10.0);
    let mut scale = PxScale::from(size as f32);
    while measure(face, &value, scale) > limit && scale.y > floor {
        scale = PxScale::from(scale.y - 1.0);
    }
    while measure(face, &value, scale) > limit && value.chars().count() > 1 {
        let kept: String = value.trim_end_matches('…').chars().collect();
        let mut kept: Vec<char> = kept.chars().collect();
        kept.pop();
        value = kept.into_iter().collect::<String>() + "…";
    }
    let value = value.as_str();
    let measure = |scale: PxScale| measure(face, value, scale);
    let scaled = face.as_scaled(scale);
    let width = measure(scale);
    let mut caret = match anchor {
        HPos::Left => at.0 as f32,
        HPos::Center => at.0 as f32 - width / 2.0,
        HPos::Right => at.0 as f32 - width,
    };
    // Centre on the cap/descender span: ascent is positive, descent negative.
    let baseline = at.1 as f32 + (scaled.ascent() + scaled.descent()) / 2.0;
    let mut previous = None;
    let mut failed = false;
    for character in value.chars() {
        let id = scaled.glyph_id(character);
        if let Some(previous) = previous {
            caret += scaled.kern(previous, id);
        }
        let glyph = id.with_scale_and_position(scale, point(caret, baseline));
        if let Some(outline) = face.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|x, y, coverage| {
                let pixel = (
                    bounds.min.x as i32 + x as i32,
                    bounds.min.y as i32 + y as i32,
                );
                failed |= root
                    .draw_pixel(pixel, &color.mix(f64::from(coverage.min(1.0))))
                    .is_err();
            });
        }
        caret += scaled.h_advance(id);
        previous = Some(id);
    }
    anyhow::ensure!(!failed, "failed to draw text");
    Ok(())
}

/// Shows the image in terminals with an inline image protocol (iTerm2 and
/// WezTerm; kitty and Ghostty). Elsewhere, and inside tmux or when stdout is
/// not a terminal, the saved path is all there is.
pub(super) fn show_inline(path: &Path) {
    use base64::Engine;
    use std::io::{IsTerminal, Write};

    let stdout = std::io::stdout();
    if !stdout.is_terminal() || std::env::var_os("TMUX").is_some() {
        return;
    }
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    let kitty = std::env::var_os("KITTY_WINDOW_ID").is_some()
        || term.contains("kitty")
        || program.eq_ignore_ascii_case("ghostty");
    let iterm = matches!(program.as_str(), "iTerm.app" | "WezTerm");
    if !kitty && !iterm {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let columns = super::view::terminal_width().unwrap_or(100).min(110);
    let mut out = stdout.lock();
    let result = if iterm {
        writeln!(
            out,
            "\x1b]1337;File=inline=1;size={};width={columns};preserveAspectRatio=1:{encoded}\x07",
            bytes.len()
        )
    } else {
        let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(4096).collect();
        let mut result = Ok(());
        for (index, chunk) in chunks.iter().enumerate() {
            let more = u8::from(index + 1 < chunks.len());
            let control = if index == 0 {
                format!("a=T,f=100,c={columns},m={more}")
            } else {
                format!("m={more}")
            };
            result = write!(
                out,
                "\x1b_G{control};{}\x1b\\",
                String::from_utf8_lossy(chunk)
            );
            if result.is_err() {
                break;
            }
        }
        result.and_then(|()| writeln!(out))
    };
    let _ = result.and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(days: &[(&str, u64)]) -> Summary {
        Summary {
            days: days
                .iter()
                .map(|(date, total)| (date.parse().unwrap(), *total))
                .collect(),
            known: KnownTokens::default(),
            overlap: false,
            agents: Vec::new(),
            models: Vec::new(),
            providers: Vec::new(),
            hours: [0; 24],
            requests: 0,
            sessions: 0,
            cost: Money::ZERO,
            priced: false,
            unpriced: false,
            records: 0,
            timezone: "UTC".to_owned(),
        }
    }

    #[test]
    fn streak_counts_consecutive_active_days_only() {
        let days = summary(&[
            ("2026-01-01", 5),
            ("2026-01-02", 5),
            ("2026-01-03", 0),
            ("2026-01-04", 1),
            ("2026-01-05", 1),
            ("2026-01-06", 1),
            ("2026-01-08", 1),
        ]);
        assert_eq!(days.longest_streak(), 3);
        assert_eq!(summary(&[]).longest_streak(), 0);
    }

    #[test]
    fn weekdays_start_on_monday() {
        // 2026-01-05 is a Monday, 2026-01-11 a Sunday.
        let week = summary(&[("2026-01-05", 3), ("2026-01-11", 7)]).weekdays();
        assert_eq!(week[0], 3);
        assert_eq!(week[6], 7);
    }

    #[test]
    fn the_busiest_day_is_the_darkest_and_idle_days_are_empty() {
        assert_eq!(heat_level(&[5], 5), Some(BLUE.len() - 1));
        assert_eq!(heat_level(&[5, 5, 5], 5), Some(BLUE.len() - 1));
        let days: Vec<u64> = (1..=12).collect();
        assert_eq!(heat_level(&days, 1), Some(0));
        assert_eq!(heat_level(&days, 12), Some(BLUE.len() - 1));
        assert_eq!(heat_level(&days, 0), None);
    }

    #[test]
    fn cache_hit_is_read_share_of_gross_input() {
        let known = KnownTokens {
            uncached_input: 10,
            cache_read: 80,
            cache_write: 10,
            output: 5,
            incomplete: false,
        };
        assert_eq!(cache_hit(&known).as_deref(), Some("80.0%"));
        assert_eq!(cache_hit(&KnownTokens::default()), None);
    }
}
