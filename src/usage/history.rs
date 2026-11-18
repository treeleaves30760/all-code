//! The remote page's usage history: the same statistics `alc usage` prints,
//! reshaped for charts.
//!
//! The page draws a day-by-day bar chart, an hour-of-day strip and a few
//! ranked lists, so this answers with exactly those series and nothing that
//! names a person or a path. A profile name is the closest thing to identity
//! in the statistics, and it is left out: providers, agents and models are
//! what the charts group by.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{Duration, Utc};
use chrono_tz::Tz;
use serde::Serialize;

use crate::config::Store;
use crate::usage::pricing::Money;
use crate::usage::query::{self, QueryArgs, UsageOptions};

/// The longest range the page may ask for.
pub(crate) const MAX_DAYS: u32 = 365;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct History {
    pub days: u32,
    pub timezone: String,
    pub generated_at: u64,
    /// One entry per day that had usage, oldest first. The page fills the
    /// gaps, because a day with no records is zero only on the chart.
    pub daily: Vec<Day>,
    /// Known tokens by hour of day in `timezone`.
    pub hours: Vec<u64>,
    pub providers: Vec<Share>,
    pub agents: Vec<Share>,
    pub models: Vec<ModelShare>,
    pub totals: Totals,
    pub pricing_snapshot: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Day {
    pub date: String,
    pub uncached: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub requests: u64,
    /// The priced part, in dollars. A float on purpose: it is drawn, never
    /// added up into a bill.
    pub usd: f64,
    /// Every record that day had a price and nothing overlapped.
    pub usd_exact: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Share {
    pub name: String,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelShare {
    pub model: String,
    pub agents: Vec<String>,
    pub tokens: u64,
    pub output: u64,
    pub usd: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Totals {
    pub tokens: u64,
    pub uncached: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub usd: f64,
    pub usd_exact: bool,
    pub records: u64,
    pub sessions: u64,
    pub active_days: u64,
}

fn dollars(money: Money) -> f64 {
    // u128 pico-USD to f64: exact well past any realistic bill.
    money.pico_usd() as f64 / 1e12
}

/// A timezone the page may name: UTC or an IANA zone. Anything else is UTC,
/// because the hub's own local zone is not the viewer's.
pub(crate) fn timezone(value: Option<&str>) -> String {
    match value {
        Some(name) if name == "UTC" || name.parse::<Tz>().is_ok() => name.to_owned(),
        _ => "UTC".to_owned(),
    }
}

pub(crate) fn build(store: &Store, days: u32, timezone_name: &str) -> Result<History> {
    build_from(store, days, timezone_name, QueryArgs::default())
}

/// `base` carries the native history roots; the range and zone are set here.
fn build_from(store: &Store, days: u32, timezone_name: &str, base: QueryArgs) -> Result<History> {
    let days = days.clamp(1, MAX_DAYS);
    let timezone_name = timezone(Some(timezone_name));
    let today = match timezone_name.parse::<Tz>() {
        Ok(zone) => Utc::now().with_timezone(&zone).date_naive(),
        Err(_) => Utc::now().date_naive(),
    };
    let since = today - Duration::days(i64::from(days) - 1);
    let options = UsageOptions {
        query: QueryArgs {
            since: Some(since.format("%Y-%m-%d").to_string()),
            timezone: timezone_name.clone(),
            ..base
        },
        // No vendor calls and no credential reads from a page poll; prices
        // come from the bundled snapshot and whatever LiteLLM map is cached.
        offline: true,
        ..UsageOptions::default()
    };
    let statistics = query::statistics(&store.dir, &store.config, &options)?;

    let daily: Vec<Day> = statistics
        .daily
        .iter()
        .map(|day| Day {
            date: day.period.clone(),
            uncached: day.known_tokens.uncached_input,
            cache_read: day.known_tokens.cache_read,
            cache_write: day.known_tokens.cache_write,
            output: day.known_tokens.output,
            requests: day.known_requests,
            usd: dollars(day.known_cost),
            usd_exact: day.total.is_some(),
        })
        .collect();

    let mut agents = BTreeMap::<String, u64>::new();
    let mut models = BTreeMap::<String, ModelShare>::new();
    for row in &statistics.rows {
        let tokens = row.known_tokens.total();
        let agent = row.agent.as_str().to_owned();
        *agents.entry(agent.clone()).or_default() += tokens;
        let model = row.model.clone().unwrap_or_else(|| "unknown".to_owned());
        let entry = models.entry(model.clone()).or_insert_with(|| ModelShare {
            model,
            agents: Vec::new(),
            tokens: 0,
            output: 0,
            usd: 0.0,
        });
        entry.tokens = entry.tokens.saturating_add(tokens);
        entry.output = entry.output.saturating_add(row.known_tokens.output);
        entry.usd += dollars(row.known_cost);
        if !entry.agents.contains(&agent) {
            entry.agents.push(agent);
        }
    }
    let ranked = |map: BTreeMap<String, u64>| {
        let mut list: Vec<Share> = map
            .into_iter()
            .filter(|(_, tokens)| *tokens > 0)
            .map(|(name, tokens)| Share { name, tokens })
            .collect();
        list.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.name.cmp(&b.name)));
        list
    };
    let mut models: Vec<ModelShare> = models.into_values().filter(|m| m.tokens > 0).collect();
    models.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.model.cmp(&b.model)));
    models.truncate(12);

    let known = statistics.known_tokens;
    let totals = Totals {
        tokens: known.total(),
        uncached: known.uncached_input,
        cache_read: known.cache_read,
        cache_write: known.cache_write,
        output: known.output,
        usd: dollars(statistics.known_cost),
        usd_exact: statistics.total.is_some(),
        records: statistics.records,
        sessions: statistics.activity.sessions,
        active_days: daily.len() as u64,
    };

    Ok(History {
        days,
        timezone: timezone_name,
        generated_at: crate::usage::ledger::now_unix(),
        daily,
        hours: statistics.activity.hours.to_vec(),
        providers: ranked(statistics.activity.providers.clone()),
        agents: ranked(agents),
        models,
        totals,
        pricing_snapshot: statistics.pricing_snapshot.clone(),
        warnings: statistics.warnings.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_utc_and_iana_zones_are_accepted() {
        assert_eq!(timezone(Some("Asia/Taipei")), "Asia/Taipei");
        assert_eq!(timezone(Some("UTC")), "UTC");
        assert_eq!(timezone(Some("local")), "UTC");
        assert_eq!(timezone(Some("../../etc")), "UTC");
        assert_eq!(timezone(None), "UTC");
    }

    #[test]
    fn an_empty_history_builds() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::load(Some(temp.path().to_path_buf())).unwrap();
        // Empty native roots, so the test never reads this machine's real
        // Claude or Codex history.
        let base = QueryArgs {
            claude_dir: vec![temp.path().join("claude")],
            codex_dir: vec![temp.path().join("codex")],
            ..QueryArgs::default()
        };
        let history = build_from(&store, 7, "Asia/Taipei", base).unwrap();
        assert_eq!(history.days, 7);
        assert_eq!(history.timezone, "Asia/Taipei");
        assert_eq!(history.hours.len(), 24);
        assert_eq!(history.totals.tokens, 0);
        assert!(history.daily.is_empty());
    }
}
