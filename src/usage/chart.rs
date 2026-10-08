//! A static, offline view of the same reconciled daily data as the CLI report.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail, ensure};
use chrono::{Datelike, NaiveDate};
use plotters::coord::Shift;
use plotters::prelude::*;

use super::pricing::Money;
use super::query::{DailyRollup, Statistics};

const WIDTH: u32 = 1560;
const HEIGHT: u32 = 1280;
const SURFACE: RGBColor = RGBColor(252, 252, 251);
const INK: RGBColor = RGBColor(11, 11, 11);
const MUTED: RGBColor = RGBColor(82, 81, 78);
const RULE: RGBColor = RGBColor(225, 225, 220);
// Fixed identities, validated for all pairs with the dataviz palette checker.
// The output fill's contrast relief is its direct label and exact table value.
const SERIES: [RGBColor; 3] = [
    RGBColor(42, 120, 214),
    RGBColor(235, 104, 52),
    RGBColor(27, 175, 122),
];
const LABELS: [&str; 3] = ["Uncached input", "Cache read + write", "Output"];
const FONT: &str = "alc-chart";
type Area<'a> = DrawingArea<BitMapBackend<'a>, Shift>;

pub(super) fn destination(requested: &Path) -> Result<PathBuf> {
    if !requested.as_os_str().is_empty() {
        return Ok(requested.to_owned());
    }
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = env::var_os(variable)
        .filter(|home| !home.is_empty())
        .with_context(|| format!("{variable} is unavailable; use --chart=/absolute/path.png"))?;
    let home = PathBuf::from(home);
    ensure!(
        home.is_absolute(),
        "{variable} must be an absolute home directory"
    );
    Ok(home.join("ai-usage.png"))
}

pub(super) fn export(report: &Statistics, path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.file_type().is_file(),
            "chart destination must be a regular file, not a directory or symlink: {}",
            path.display()
        );
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure!(
        parent.is_dir(),
        "chart destination directory does not exist: {}",
        parent.display()
    );
    let staged = tempfile::Builder::new()
        .prefix(".alc-usage-")
        .suffix(".png")
        .tempfile_in(parent)
        .with_context(|| format!("cannot write chart in {}", parent.display()))?;
    render(report, staged.path())
        .with_context(|| format!("failed to render {}", path.display()))?;
    staged
        .as_file()
        .sync_all()
        .context("failed to flush usage chart")?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("cannot replace chart {}", path.display()))?;
    Ok(())
}

fn register_font() -> Result<()> {
    static FONT_RESULT: OnceLock<bool> = OnceLock::new();
    let valid = *FONT_RESULT.get_or_init(|| {
        plotters::style::register_font(
            FONT,
            FontStyle::Normal,
            include_bytes!("../../assets/fonts/NotoSans.ttf"),
        )
        .is_ok()
    });
    ensure!(valid, "the embedded chart font could not be loaded");
    Ok(())
}

#[derive(Clone)]
struct Bucket {
    label: String,
    tokens: [Option<u64>; 3],
    subtotal: Option<Money>,
    total: Option<Money>,
    records: u64,
}

impl Bucket {
    fn from_day(day: &DailyRollup, label: String) -> Self {
        Self {
            label,
            tokens: [
                day.uncached_input_tokens,
                day.cache_read_tokens
                    .and_then(|read| read.checked_add(day.cache_write_tokens?)),
                day.output_tokens,
            ],
            subtotal: day.subtotal,
            total: day.total,
            records: day.records,
        }
    }

    fn add(&mut self, day: &DailyRollup) -> Result<()> {
        let other = Self::from_day(day, String::new());
        for (value, next) in self.tokens.iter_mut().zip(other.tokens) {
            *value = value.and_then(|value| value.checked_add(next?));
        }
        self.subtotal = add_money(self.subtotal, other.subtotal)?;
        self.total = add_money(self.total, other.total)?;
        self.records = self
            .records
            .checked_add(other.records)
            .context("chart record count overflow")?;
        Ok(())
    }

    fn token_total(&self) -> Option<u64> {
        self.tokens
            .iter()
            .try_fold(0u64, |sum, value| sum.checked_add((*value)?))
    }

    fn cost(&self) -> Option<Money> {
        // An unknown bill with a known zero subtotal is not a measured free day.
        self.total
            .or_else(|| self.subtotal.filter(|value| *value > Money::ZERO))
    }
}

fn add_money(left: Option<Money>, right: Option<Money>) -> Result<Option<Money>> {
    match (left, right) {
        (Some(left), Some(right)) => Ok(Some(
            left.checked_add(right).context("chart cost overflow")?,
        )),
        _ => Ok(None),
    }
}

fn bucket_label(date: NaiveDate, scale: &str) -> String {
    match scale {
        "weekly" => format!("{}-W{:02}", date.iso_week().year(), date.iso_week().week()),
        "monthly" => date.format("%Y-%m").to_string(),
        "yearly" => date.year().to_string(),
        _ => date.format("%Y-%m-%d").to_string(),
    }
}

fn buckets(days: &[DailyRollup], possible_overlap: bool) -> Result<(Vec<Bucket>, &'static str)> {
    let dates = days
        .iter()
        .filter_map(|day| NaiveDate::parse_from_str(&day.period, "%Y-%m-%d").ok())
        .collect::<Vec<_>>();
    let span = dates
        .first()
        .zip(dates.last())
        .map(|(first, last)| (*last - *first).num_days())
        .unwrap_or(0);
    let scale = if span <= 35 {
        "daily"
    } else if span <= 240 {
        "weekly"
    } else if span <= 1080 {
        "monthly"
    } else {
        "yearly"
    };
    let mut grouped = BTreeMap::<String, Bucket>::new();
    for day in days {
        let date = NaiveDate::parse_from_str(&day.period, "%Y-%m-%d").ok();
        let label = date
            .map(|date| bucket_label(date, scale))
            .unwrap_or_else(|| day.period.clone());
        match grouped.entry(label.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Bucket::from_day(day, label));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => entry.get_mut().add(day)?,
        }
    }
    // Preserve gaps at every display scale; absence is not measured zero.
    if let Some((first, last)) = dates.first().zip(dates.last()) {
        let empty = |label: String| Bucket {
            label,
            tokens: [None; 3],
            subtotal: None,
            total: None,
            records: 0,
        };
        if scale == "yearly" {
            ensure!(
                last.year() - first.year() < 120,
                "chart range has more than 120 yearly buckets; select a shorter --since/--until range"
            );
            for year in first.year()..=last.year() {
                let label = year.to_string();
                grouped.entry(label.clone()).or_insert_with(|| empty(label));
            }
        } else {
            let mut date = *first;
            while date <= *last {
                let label = bucket_label(date, scale);
                grouped.entry(label.clone()).or_insert_with(|| empty(label));
                let Some(next) = date.succ_opt() else { break };
                date = next;
            }
        }
    }
    let mut values = grouped.into_values().collect::<Vec<_>>();
    if scale != "daily" && possible_overlap {
        // Individually safe days do not prove sources are disjoint within a
        // coarser period. Keep counts/dates, but never reintroduce pooled sums.
        for bucket in &mut values {
            bucket.tokens = [None; 3];
            bucket.subtotal = None;
            bucket.total = None;
        }
    }
    ensure!(
        values.len() <= 120,
        "chart range has more than 120 yearly buckets; select a shorter --since/--until range"
    );
    Ok((values, scale))
}

fn render(report: &Statistics, path: &Path) -> Result<()> {
    register_font()?;
    let root = BitMapBackend::new(path, (WIDTH, HEIGHT)).into_drawing_area();
    root.fill(&SURFACE)?;
    text(&root, "AI usage", (52, 28), 36, INK)?;
    let first = report.range.start.as_deref().unwrap_or_else(|| {
        report
            .daily
            .first()
            .map_or("no records", |day| day.period.as_str())
    });
    let end = report.range.end.as_deref().unwrap_or_else(|| {
        report
            .daily
            .last()
            .map_or("no records", |day| day.period.as_str())
    });
    let end_label = if report.range.end.is_some() {
        "end exclusive"
    } else {
        "last observed date"
    };
    let sources = report
        .sources
        .iter()
        .map(|source| source.source.as_str())
        .collect::<Vec<_>>()
        .join(" + ");
    text(
        &root,
        &format!(
            "{first} to {end}  |  timezone {}  |  {end_label}  |  sources: {sources}",
            report.timezone
        ),
        (54, 83),
        16,
        MUTED,
    )?;
    let cost = report
        .total
        .map(|value| format!("${value}"))
        .or_else(|| {
            report
                .subtotal
                .map(|value| format!("${value} known subtotal; incomplete"))
        })
        .unwrap_or_else(|| "N/A; sources may overlap".to_owned());
    text(
        &root,
        &format!("API-equivalent token cost: {cost} USD"),
        (54, 113),
        23,
        INK,
    )?;
    text(
        &root,
        &format!(
            "{} records  |  {} priced  |  {} unpriced  |  prices: {}",
            report.records, report.priced_records, report.unpriced_records, report.pricing_snapshot
        ),
        (54, 153),
        16,
        MUTED,
    )?;
    let (values, scale) = buckets(&report.daily, report.possible_overlap)?;
    let token_area = root.clone().shrink((50, 205), (1460, 365));
    draw_tokens(&token_area, &values, scale)?;
    let cost_area = root.clone().shrink((50, 595), (1460, 300));
    draw_costs(&cost_area, &values, scale)?;
    let composition = root.clone().shrink((50, 930), (640, 270));
    draw_composition(&composition, report)?;
    let table = root.clone().shrink((735, 930), (780, 270));
    draw_table(&table, report)?;
    let warning = if report.possible_overlap {
        "Overlapping sources: unsafe pooled totals are N/A. Filter --source to obtain an additive view."
    } else if report.coverage_incomplete {
        "Source coverage is incomplete. Missing days/counters are gaps, not zero usage or free spend."
    } else {
        "Token-only API-equivalent estimates, not subscription bills. Missing days/counters are gaps, not zero."
    };
    text(&root, warning, (54, 1220), 15, MUTED)?;
    text(
        &root,
        "Exact daily values and cost components: alc usage --offline --json. Historical deltas use their recorded endpoint date.",
        (54, 1248),
        14,
        MUTED,
    )?;
    root.present().context("failed to encode PNG")?;
    Ok(())
}

fn text(area: &Area<'_>, value: &str, at: (i32, i32), size: u32, color: RGBColor) -> Result<()> {
    let mut style = (FONT, size).into_font().color(&color);
    let available = area.dim_in_pixel().0.saturating_sub(at.0.max(0) as u32 + 8);
    while area.estimate_text_size(value, &style)?.0 > available && style.font.get_size() > 10.0 {
        style.font = style.font.resize(style.font.get_size() - 1.0);
    }
    area.draw(&Text::new(value, at, style))?;
    Ok(())
}

fn axes(area: &Area<'_>, max: f64, height: i32, currency: bool) -> Result<()> {
    let width = area.dim_in_pixel().0 as i32;
    for tick in 0..=4 {
        let y = 56 + (height - 56) * (4 - tick) / 4;
        area.draw(&PathElement::new([(110, y), (width - 20, y)], RULE))?;
        let value = max * tick as f64 / 4.0;
        let label = if currency {
            format!("${}", compact(value))
        } else {
            compact(value)
        };
        let style =
            (FONT, 14)
                .into_font()
                .color(&MUTED)
                .pos(plotters::style::text_anchor::Pos::new(
                    plotters::style::text_anchor::HPos::Right,
                    plotters::style::text_anchor::VPos::Center,
                ));
        area.draw(&Text::new(label, (96, y), style))?;
    }
    Ok(())
}

fn geometry(area: &Area<'_>, count: usize) -> (f64, i32, i32) {
    let width = area.dim_in_pixel().0 as f64;
    let step = (width - 140.0) / count.max(1) as f64;
    let thickness = step.mul_add(0.55, 0.0).clamp(2.0, 24.0) as i32;
    (step, thickness, 110)
}

fn date_labels(
    area: &Area<'_>,
    values: &[Bucket],
    step: f64,
    bottom: i32,
    scale: &str,
) -> Result<()> {
    let interval = values.len().div_ceil(10).max(1);
    for (index, bucket) in values.iter().enumerate() {
        if index % interval != 0 && index + 1 != values.len() {
            continue;
        }
        let label = if scale == "daily" {
            bucket.label.get(5..).unwrap_or(&bucket.label)
        } else {
            &bucket.label
        };
        let x = (110.0 + step * (index as f64 + 0.5)) as i32;
        let style =
            (FONT, 13)
                .into_font()
                .color(&MUTED)
                .pos(plotters::style::text_anchor::Pos::new(
                    plotters::style::text_anchor::HPos::Center,
                    plotters::style::text_anchor::VPos::Top,
                ));
        area.draw(&Text::new(label, (x, bottom + 13), style))?;
    }
    Ok(())
}

fn rounded_bar(
    area: &Area<'_>,
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
    color: RGBColor,
    rounded: bool,
) -> Result<()> {
    if right <= left || bottom <= top {
        return Ok(());
    }
    let radius = if rounded {
        4.min((right - left) / 2).min(bottom - top)
    } else {
        0
    };
    if radius == 0 {
        area.draw(&Rectangle::new(
            [(left, top), (right, bottom)],
            color.filled(),
        ))?;
        return Ok(());
    }
    let mut points = vec![(left, bottom), (left, top + radius)];
    for step in 0..=8 {
        let angle = std::f64::consts::PI + step as f64 * std::f64::consts::FRAC_PI_2 / 8.0;
        points.push((
            left + radius + (radius as f64 * angle.cos()).round() as i32,
            top + radius + (radius as f64 * angle.sin()).round() as i32,
        ));
    }
    for step in 0..=8 {
        let angle = -std::f64::consts::FRAC_PI_2 + step as f64 * std::f64::consts::FRAC_PI_2 / 8.0;
        points.push((
            right - radius + (radius as f64 * angle.cos()).round() as i32,
            top + radius + (radius as f64 * angle.sin()).round() as i32,
        ));
    }
    points.push((right, bottom));
    area.draw(&Polygon::new(points, color.filled()))?;
    Ok(())
}

fn draw_tokens(area: &Area<'_>, values: &[Bucket], scale: &str) -> Result<()> {
    text(area, &format!("Tokens by {scale} period"), (0, 0), 23, INK)?;
    for index in 0..3 {
        let x = 570 + index as i32 * 278;
        area.draw(&Rectangle::new(
            [(x, 6), (x + 10, 16)],
            SERIES[index].filled(),
        ))?;
        text(area, LABELS[index], (x + 18, 0), 15, INK)?;
    }
    let max = values
        .iter()
        .filter_map(Bucket::token_total)
        .max()
        .unwrap_or(0);
    if max == 0 {
        return text(
            area,
            "No complete, additive token values for this range",
            (110, 155),
            20,
            MUTED,
        );
    }
    let scale_max = max as f64 * 1.12;
    let bottom = 304;
    axes(area, scale_max, bottom, false)?;
    let (step, width, _) = geometry(area, values.len());
    for (index, bucket) in values.iter().enumerate() {
        let x = (110.0 + step * (index as f64 + 0.5)) as i32;
        if bucket.token_total().is_none() {
            if bucket.records > 0 {
                text(area, "?", (x - 4, bottom - 20), 16, MUTED)?;
            }
            continue;
        }
        let last = bucket
            .tokens
            .iter()
            .rposition(|value| value.is_some_and(|value| value > 0));
        let mut y = bottom;
        for (category, tokens) in bucket.tokens.iter().enumerate() {
            let tokens = tokens.unwrap_or(0);
            if tokens == 0 {
                continue;
            }
            let pixels = ((bottom - 56) as f64 * tokens as f64 / scale_max)
                .round()
                .max(1.0) as i32;
            let top = y - pixels;
            rounded_bar(
                area,
                x - width / 2,
                x + width / 2,
                top,
                y - 2.min(pixels - 1),
                SERIES[category],
                last == Some(category),
            )?;
            y = top;
        }
        if bucket.token_total() == Some(max) {
            text(area, &integer(max), (x - 18, y - 25), 14, INK)?;
        }
    }
    date_labels(area, values, step, bottom, scale)?;
    Ok(())
}

fn draw_costs(area: &Area<'_>, values: &[Bucket], scale: &str) -> Result<()> {
    text(
        area,
        &format!("API-equivalent USD by {scale} period"),
        (0, 0),
        23,
        INK,
    )?;
    text(
        area,
        "* known priced subtotal only; ? unavailable",
        (950, 3),
        15,
        MUTED,
    )?;
    let max = values
        .iter()
        .filter_map(Bucket::cost)
        .max()
        .unwrap_or(Money::ZERO);
    if max == Money::ZERO {
        let message = if values.iter().any(|value| value.total == Some(Money::ZERO)) {
            "Known priced cost is zero; unknown costs remain N/A"
        } else {
            "No known additive priced costs for this range"
        };
        return text(area, message, (110, 130), 20, MUTED);
    }
    let scale_max = max.pico_usd() as f64 / 1e12 * 1.15;
    let bottom = 244;
    axes(area, scale_max, bottom, true)?;
    let (step, width, _) = geometry(area, values.len());
    for (index, bucket) in values.iter().enumerate() {
        let x = (110.0 + step * (index as f64 + 0.5)) as i32;
        let Some(cost) = bucket.cost() else {
            if bucket.records > 0 {
                text(area, "?", (x - 4, bottom - 20), 16, MUTED)?;
            }
            continue;
        };
        let top = bottom
            - (((bottom - 56) as f64 * (cost.pico_usd() as f64 / 1e12) / scale_max).round() as i32);
        let top = if cost > Money::ZERO {
            top.min(bottom - 1)
        } else {
            top
        };
        rounded_bar(
            area,
            x - width / 2,
            x + width / 2,
            top,
            bottom,
            SERIES[0],
            true,
        )?;
        if bucket.total.is_none() {
            text(area, "*", (x - 4, top - 19), 15, INK)?;
        }
        if cost == max {
            text(area, &format!("${cost}"), (x - 20, top - 36), 14, INK)?;
        }
    }
    date_labels(area, values, step, bottom, scale)?;
    Ok(())
}

fn draw_composition(area: &Area<'_>, report: &Statistics) -> Result<()> {
    text(area, "Token composition", (0, 0), 23, INK)?;
    let values = [
        report.uncached_input_tokens,
        report
            .cache_read_tokens
            .and_then(|read| read.checked_add(report.cache_write_tokens?)),
        report.output_tokens,
    ];
    let Some(values) = values.into_iter().collect::<Option<Vec<_>>>() else {
        return text(
            area,
            "N/A: incomplete counters or overlapping sources",
            (0, 80),
            17,
            MUTED,
        );
    };
    let Some(total) = values
        .iter()
        .try_fold(0u64, |sum, value| sum.checked_add(*value))
    else {
        bail!("chart token sum overflow")
    };
    if total == 0 {
        return text(area, "No nonzero complete token values", (0, 80), 18, MUTED);
    }
    let nonzero = values.iter().filter(|value| **value > 0).count();
    if nonzero == 3 {
        draw_pie(area, &values, total)?;
    } else {
        // Angles add no information for one/two nonzero categories.
        let mut left = 0;
        for (index, value) in values.iter().enumerate() {
            if *value == 0 {
                continue;
            }
            let width = (240.0 * *value as f64 / total as f64).round() as i32;
            area.draw(&Rectangle::new(
                [(left, 115), (left + width - 2, 138)],
                SERIES[index].filled(),
            ))?;
            left += width;
        }
        text(area, "Nonzero categories only", (0, 157), 14, MUTED)?;
    }
    for (index, value) in values.iter().enumerate() {
        let y = 62 + index as i32 * 65;
        area.draw(&Rectangle::new(
            [(270, y + 5), (280, y + 15)],
            SERIES[index].filled(),
        ))?;
        text(area, LABELS[index], (291, y), 16, INK)?;
        text(
            area,
            &format!(
                "{}  ({:.1}%)",
                integer(*value),
                *value as f64 / total as f64 * 100.0
            ),
            (291, y + 25),
            16,
            MUTED,
        )?;
    }
    Ok(())
}

fn draw_pie(area: &Area<'_>, values: &[u64], total: u64) -> Result<()> {
    let local_center = (130, 145);
    let (x_range, y_range) = area.get_pixel_range();
    // Plotters' Pie bypasses the drawing area's coordinate translation.
    // Its center must be absolute; ordinary path elements remain local.
    let center = (
        local_center.0 + x_range.start,
        local_center.1 + y_range.start,
    );
    let radius = 88.0;
    let sizes = values.iter().map(|value| *value as f64).collect::<Vec<_>>();
    let empty = ["", "", ""];
    let mut pie = Pie::new(&center, &radius, &sizes, &SERIES, &empty);
    pie.start_angle(-90.0);
    pie.label_style((FONT, 1).into_font().color(&SURFACE));
    area.draw(&pie)?;
    let mut angle = -std::f64::consts::FRAC_PI_2;
    for size in &sizes {
        let end = (
            local_center.0 + (radius * angle.cos()).round() as i32,
            local_center.1 + (radius * angle.sin()).round() as i32,
        );
        area.draw(&PathElement::new(
            [local_center, end],
            SURFACE.stroke_width(2),
        ))?;
        angle += *size / total as f64 * std::f64::consts::TAU;
    }
    Ok(())
}

fn draw_table(area: &Area<'_>, report: &Statistics) -> Result<()> {
    text(area, "Exact selected-range totals", (0, 0), 23, INK)?;
    text(area, "CATEGORY", (0, 41), 14, MUTED)?;
    text(area, "TOKENS", (268, 41), 14, MUTED)?;
    text(area, "USD (known / complete)", (455, 41), 14, MUTED)?;
    let rows = [
        (
            "Uncached input",
            report.uncached_input_tokens,
            &report.cost_components.uncached_input,
        ),
        (
            "Cache read",
            report.cache_read_tokens,
            &report.cost_components.cache_read,
        ),
        (
            "Cache write",
            report.cache_write_tokens,
            &report.cost_components.cache_write,
        ),
        (
            "Output",
            report.output_tokens,
            &report.cost_components.output,
        ),
    ];
    for (index, (label, tokens, cost)) in rows.iter().enumerate() {
        let y = 73 + index as i32 * 39;
        text(area, label, (0, y), 16, INK)?;
        let token = tokens.map(integer).unwrap_or_else(|| "N/A".to_owned());
        let style = (FONT, 16)
            .into_font()
            .color(&INK)
            .pos(plotters::style::text_anchor::Pos::new(
                plotters::style::text_anchor::HPos::Right,
                plotters::style::text_anchor::VPos::Top,
            ));
        area.draw(&Text::new(token, (378, y), style))?;
        let known = cost
            .known_subtotal_usd
            .map(|value| value.to_string())
            .unwrap_or_else(|| "N/A".to_owned());
        let complete = cost
            .total_usd
            .map(|value| value.to_string())
            .unwrap_or_else(|| "N/A".to_owned());
        text(area, &format!("{known} / {complete}"), (455, y), 15, INK)?;
    }
    Ok(())
}

fn integer(value: u64) -> String {
    super::query::format_count(value)
}

fn compact(value: f64) -> String {
    if value >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else if value > 0.0 && value < 0.001 {
        format!("{value:.1e}")
    } else if value < 10.0 {
        format!("{value:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(period: &str) -> DailyRollup {
        DailyRollup {
            period: period.to_owned(),
            records: 1,
            requests: Some(1),
            known_requests: 1,
            input_tokens: Some(10),
            uncached_input_tokens: Some(7),
            cache_read_tokens: Some(2),
            cache_write_tokens: Some(1),
            output_tokens: Some(1),
            cost_components: super::super::pricing::CostComponents::zero(),
            known_subtotal_usd: Some("0".to_owned()),
            total_usd: Some("0".to_owned()),
            priced_records: 1,
            partial_records: 0,
            unpriced_records: 0,
            possible_overlap: false,
            coverage_incomplete: false,
            subtotal: Some(Money::ZERO),
            total: Some(Money::ZERO),
        }
    }

    #[test]
    fn calendar_gaps_survive_daily_and_aggregate_views() {
        for (first, last, expected_scale, missing) in [
            ("2026-01-01", "2026-01-03", "daily", "2026-01-02"),
            ("2025-12-29", "2026-02-10", "weekly", "2026-W02"),
            ("2025-01-01", "2026-01-01", "monthly", "2025-02"),
            ("2020-01-01", "2026-01-01", "yearly", "2021"),
        ] {
            let (values, scale) = buckets(&[day(first), day(last)], false).unwrap();
            assert_eq!(scale, expected_scale);
            let gap = values.iter().find(|value| value.label == missing).unwrap();
            assert_eq!(gap.records, 0);
            assert_eq!(gap.token_total(), None);
            assert_eq!(gap.cost(), None);
            assert_eq!(
                values.iter().filter_map(Bucket::token_total).sum::<u64>(),
                22,
                "cache must never be counted twice"
            );
        }
    }

    #[test]
    fn overlapping_selection_suppresses_coarser_pooled_buckets_but_keeps_daily_values() {
        let cost = Money::from_pico_usd(450_000_000);
        let priced_day = |period| {
            let mut value = day(period);
            value.subtotal = Some(cost);
            value.total = Some(cost);
            value
        };
        let days = [priced_day("2026-01-01"), priced_day("2026-01-02")];
        assert!(days.iter().all(|day| !day.possible_overlap));
        let (daily, scale) = buckets(&days, true).unwrap();
        assert_eq!(scale, "daily");
        assert!(daily.iter().all(|bucket| bucket.token_total() == Some(11)));
        assert!(daily.iter().all(|bucket| bucket.cost() == Some(cost)));
        for (last, expected_scale) in [
            ("2026-02-20", "weekly"),
            ("2027-01-01", "monthly"),
            ("2030-01-01", "yearly"),
        ] {
            let days = [
                priced_day("2026-01-01"),
                priced_day("2026-01-02"),
                day(last),
            ];
            let (safe, scale) = buckets(&days, false).unwrap();
            assert_eq!(scale, expected_scale);
            assert_eq!(safe[0].token_total(), Some(22));
            assert_eq!(safe[0].cost(), cost.checked_add(cost));
            let (overlapping, scale) = buckets(&days, true).unwrap();
            assert_eq!(scale, expected_scale);
            assert_eq!(overlapping[0].records, 2);
            assert!(overlapping.iter().all(|bucket| {
                bucket.tokens == [None; 3] && bucket.subtotal.is_none() && bucket.total.is_none()
            }));
        }
    }

    #[test]
    fn invalid_pooled_counters_and_unknown_zero_cost_are_not_measured() {
        let mut incomplete = day("2026-01-01");
        incomplete.cache_write_tokens = None;
        incomplete.total = None;
        let bucket = Bucket::from_day(&incomplete, incomplete.period.clone());
        assert_eq!(bucket.token_total(), None);
        assert_eq!(bucket.cost(), None);
        let mut overlapping = day("2026-01-02");
        overlapping.subtotal = None;
        overlapping.total = None;
        overlapping.uncached_input_tokens = None;
        let bucket = Bucket::from_day(&overlapping, overlapping.period.clone());
        assert_eq!(bucket.token_total(), None);
        assert_eq!(bucket.cost(), None);
    }

    #[test]
    fn pie_respects_a_shifted_panels_origin() {
        register_font().unwrap();
        let width = 400;
        let height = 550;
        let mut pixels = vec![0u8; width * height * 3];
        {
            let root = BitMapBackend::with_buffer(&mut pixels, (width as u32, height as u32))
                .into_drawing_area();
            root.fill(&SURFACE).unwrap();
            let panel = root.clone().shrink((50, 250), (300, 260));
            draw_pie(&panel, &[7, 3, 1], 11).unwrap();
            root.present().unwrap();
        }
        let is_series = |pixel: &[u8; 3]| {
            SERIES
                .iter()
                .any(|color| *pixel == [color.0, color.1, color.2])
        };
        assert!(
            !pixels[..width * 250 * 3]
                .as_chunks::<3>()
                .0
                .iter()
                .any(is_series),
            "a shifted pie must not paint over the report header"
        );
        assert!(
            pixels[width * 250 * 3..]
                .as_chunks::<3>()
                .0
                .iter()
                .filter(|pixel| is_series(pixel))
                .count()
                > 10_000,
            "pie must render inside its composition panel"
        );
    }

    #[test]
    fn integers_are_exact_and_small_costs_do_not_look_free() {
        assert_eq!(integer(12_345_678), "12,345,678");
        assert_eq!(compact(0.00000001), "1.0e-8");
        assert_eq!(compact(0.0), "0");
        assert_eq!(compact(1.0851), "1.0851");
    }
}
