//! CLI-only analytics. The remote quota report deliberately never reads histories.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Days, Local, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use clap::builder::TypedValueParser;
use clap::{Args, ValueEnum};
use serde::Serialize;

use super::native;
use super::pricing::{CostComponents, CostEstimate, CostStatus, Money, PriceBook, PriceSource};
use super::records::{
    Granularity, InputBasis, Metrics, OutputBasis, ReadResult, Source, SourceDiagnostics,
    TokenCounts, UsageRecord,
};
use crate::config::{Agent, Config};
use crate::doctor::{Cell, INDENT, Table, Theme, Tone, heading_text};

#[derive(Debug, Clone, Args)]
pub(crate) struct QueryArgs {
    /// alc, claude, codex, or all; accepts comma-separated sources.
    #[arg(long, value_delimiter = ',')]
    pub source: Vec<String>,
    /// Inclusive start: YYYY-MM-DD in --timezone, or RFC3339.
    #[arg(long, value_name = "DATE")]
    pub since: Option<String>,
    /// Exclusive end: YYYY-MM-DD in --timezone, or RFC3339.
    #[arg(long, value_name = "DATE")]
    pub until: Option<String>,
    /// Timezone for date-only bounds and calendar periods: UTC, local, or an IANA name.
    #[arg(long, default_value = "UTC", value_name = "ZONE")]
    pub timezone: String,
    /// Filter recorded alc profiles; native histories may have no profile.
    #[arg(long, value_name = "PROFILE")]
    pub filter_profile: Option<String>,
    /// Filter the recorded coding agent.
    #[arg(long, value_name = "AGENT")]
    pub filter_agent: Option<Agent>,
    /// Filter the exact reported model ID.
    #[arg(long, value_name = "MODEL")]
    pub filter_model: Option<String>,
    /// Claude config directory (may be repeated); reads projects/ only.
    #[arg(long, value_name = "PATH")]
    pub claude_dir: Vec<PathBuf>,
    /// Codex home (may be repeated); reads sessions/ and archived_sessions/.
    #[arg(long, value_name = "PATH")]
    pub codex_dir: Vec<PathBuf>,
}

impl Default for QueryArgs {
    fn default() -> Self {
        Self {
            source: Vec::new(),
            since: None,
            until: None,
            timezone: "UTC".to_owned(),
            filter_profile: None,
            filter_agent: None,
            filter_model: None,
            claude_dir: Vec::new(),
            codex_dir: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CalendarWindow {
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Default, Args)]
pub(crate) struct UsageOptions {
    #[command(flatten)]
    pub query: QueryArgs,
    /// Select the current calendar week (Monday start), month, or year; retain daily details.
    #[arg(value_enum, value_name = "WINDOW", conflicts_with_all = ["since", "until", "daily", "monthly"])]
    pub window: Option<CalendarWindow>,
    /// Group all selected history by day in --timezone (no implicit date window).
    #[arg(long, conflicts_with = "monthly")]
    pub daily: bool,
    /// Group all selected history by month in --timezone (no implicit date window).
    #[arg(long)]
    pub monthly: bool,
    /// Read only local statistics; do not read credentials or query quota APIs.
    #[arg(long)]
    pub offline: bool,
    /// Override pricing.toml; rates are USD per million tokens.
    #[arg(long, value_name = "PATH")]
    pub pricing_file: Option<PathBuf>,
    /// Write an offline PNG chart; without a path, use ~/ai-usage.png.
    #[arg(long, num_args = 0..=1, default_missing_value = "", require_equals = true, value_name = "PATH", value_parser = clap::builder::OsStringValueParser::new().map(PathBuf::from))]
    pub chart: Option<PathBuf>,
    /// Write a shareable PNG summary of every agent and provider; without a path, use ~/alc-wrapped.png.
    #[arg(long, num_args = 0..=1, default_missing_value = "", require_equals = true, value_name = "PATH", value_parser = clap::builder::OsStringValueParser::new().map(PathBuf::from))]
    pub wrapped: Option<PathBuf>,
    /// Print the report as JSON.
    #[arg(long)]
    pub json: bool,
    /// Also print per-source rows, assumptions and coverage diagnostics.
    #[arg(long)]
    pub details: bool,
}

#[derive(Debug, Clone, Args)]
pub(crate) struct TpsOptions {
    #[command(flatten)]
    pub query: QueryArgs,
    /// Number of most recent matching requests to show.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=10000))]
    pub limit: u32,
    /// Also show historical, non-request, and untimed records (metrics stay unavailable).
    #[arg(long)]
    pub include_unmeasured: bool,
    /// Print timing, token counters and coverage as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone)]
enum QueryTimezone {
    Utc,
    Local,
    Named(Tz),
}

impl QueryTimezone {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "UTC" => Ok(Self::Utc),
            "local" => Ok(Self::Local),
            _ => value.parse::<Tz>().map(Self::Named).with_context(|| {
                format!("unknown timezone '{value}'; expected UTC, local, or an IANA timezone name")
            }),
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Utc => "UTC".to_owned(),
            Self::Local => "local".to_owned(),
            Self::Named(zone) => zone.name().to_owned(),
        }
    }

    fn date(&self, timestamp: DateTime<Utc>) -> NaiveDate {
        match self {
            Self::Utc => timestamp.date_naive(),
            Self::Local => timestamp.with_timezone(&Local).date_naive(),
            Self::Named(zone) => timestamp.with_timezone(zone).date_naive(),
        }
    }

    fn hour(&self, timestamp: DateTime<Utc>) -> u32 {
        match self {
            Self::Utc => timestamp.hour(),
            Self::Local => timestamp.with_timezone(&Local).hour(),
            Self::Named(zone) => timestamp.with_timezone(zone).hour(),
        }
    }

    fn midnight(&self, date: NaiveDate) -> Result<DateTime<Utc>> {
        let naive = date
            .and_hms_opt(0, 0, 0)
            .context("invalid calendar boundary")?;
        // A repeated midnight starts at its first occurrence. A nonexistent
        // midnight is rejected explicitly rather than silently shifted to UTC.
        let timestamp = match self {
            Self::Utc => Some(naive.and_utc()),
            Self::Local => Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|dt| dt.with_timezone(&Utc)),
            Self::Named(zone) => zone
                .from_local_datetime(&naive)
                .earliest()
                .map(|dt| dt.with_timezone(&Utc)),
        };
        timestamp.with_context(|| {
            format!(
                "midnight on {date} does not exist in timezone {}",
                self.name()
            )
        })
    }

    fn label(&self, timestamp_ms: u64) -> Option<String> {
        let timestamp = timestamp(timestamp_ms)?;
        Some(match self {
            Self::Utc => timestamp.to_rfc3339(),
            Self::Local => timestamp.with_timezone(&Local).to_rfc3339(),
            Self::Named(zone) => timestamp.with_timezone(zone).to_rfc3339(),
        })
    }

    fn period(&self, timestamp_ms: u64, monthly: bool) -> String {
        timestamp(timestamp_ms)
            .map(|timestamp| {
                self.date(timestamp)
                    .format(if monthly { "%Y-%m" } else { "%Y-%m-%d" })
                    .to_string()
            })
            .unwrap_or_else(|| "unknown-date".to_owned())
    }
}

fn timestamp(timestamp_ms: u64) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp_millis(i64::try_from(timestamp_ms).ok()?)
}

#[derive(Debug, Serialize)]
pub(crate) struct QueryRange {
    /// Inclusive resolved boundary, with the selected timezone's offset.
    pub start: Option<String>,
    /// Exclusive resolved boundary, with the selected timezone's offset.
    pub end: Option<String>,
}

#[derive(Debug)]
struct Query {
    sources: BTreeSet<Source>,
    since: Option<u64>,
    until: Option<u64>,
    timezone: QueryTimezone,
}

impl Query {
    fn new(
        args: &QueryArgs,
        default_all: bool,
        window: Option<CalendarWindow>,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let mut sources = BTreeSet::new();
        let selected = if args.source.is_empty() {
            vec![if default_all { "all" } else { "alc" }.to_owned()]
        } else {
            args.source.clone()
        };
        for source in selected {
            match source.as_str() {
                "all" => sources.extend([Source::Alc, Source::Claude, Source::Codex]),
                "alc" => {
                    sources.insert(Source::Alc);
                }
                "claude" => {
                    sources.insert(Source::Claude);
                }
                "codex" => {
                    sources.insert(Source::Codex);
                }
                _ => bail!("unknown usage source '{source}'; expected all, alc, claude, or codex"),
            }
        }
        if args
            .claude_dir
            .iter()
            .chain(&args.codex_dir)
            .any(|path| !path.is_absolute())
        {
            bail!("--claude-dir and --codex-dir must be absolute configuration directories");
        }
        let timezone = QueryTimezone::parse(&args.timezone)?;
        if window.is_some() && (args.since.is_some() || args.until.is_some()) {
            bail!("calendar window cannot be used with --since or --until");
        }
        let (since, until) = if let Some(window) = window {
            let (start, end) = calendar_dates(timezone.date(now), window)?;
            (
                Some(epoch_ms(timezone.midnight(start)?)?),
                Some(epoch_ms(timezone.midnight(end)?)?),
            )
        } else {
            (
                args.since
                    .as_deref()
                    .map(|text| parse_bound(text, &timezone))
                    .transpose()?,
                args.until
                    .as_deref()
                    .map(|text| parse_bound(text, &timezone))
                    .transpose()?,
            )
        };
        if let (Some(since), Some(until)) = (since, until)
            && since >= until
        {
            bail!("--since must precede the exclusive --until boundary");
        }
        Ok(Self {
            sources,
            since,
            until,
            timezone,
        })
    }

    fn matches(&self, record: &UsageRecord, args: &QueryArgs) -> bool {
        self.since.is_none_or(|since| record.timestamp_ms >= since)
            && self.until.is_none_or(|until| record.timestamp_ms < until)
            && args
                .filter_profile
                .as_ref()
                .is_none_or(|profile| record.profile.as_ref() == Some(profile))
            && args
                .filter_model
                .as_ref()
                .is_none_or(|model| record.model.as_ref() == Some(model))
            && args.filter_agent.is_none_or(|agent| record.agent == agent)
    }
}

fn epoch_ms(timestamp: DateTime<Utc>) -> Result<u64> {
    u64::try_from(timestamp.timestamp_millis())
        .context("usage dates must be at or after 1970-01-01")
}

fn parse_bound(text: &str, timezone: &QueryTimezone) -> Result<u64> {
    let timestamp = if text.len() == 10 {
        let date = NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .context("expected a valid YYYY-MM-DD date")?;
        timezone.midnight(date)?
    } else {
        DateTime::parse_from_rfc3339(text)
            .context("expected RFC3339 or a valid YYYY-MM-DD date in --timezone")?
            .with_timezone(&Utc)
    };
    epoch_ms(timestamp)
}

fn calendar_dates(today: NaiveDate, window: CalendarWindow) -> Result<(NaiveDate, NaiveDate)> {
    let (start, end) = match window {
        CalendarWindow::Weekly => {
            let start = today
                .checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday())))
                .context("calendar week start is out of range")?;
            let end = start
                .checked_add_days(Days::new(7))
                .context("calendar week end is out of range")?;
            (start, end)
        }
        CalendarWindow::Monthly => {
            let start = today.with_day(1).context("invalid calendar month")?;
            let (year, month) = if today.month() == 12 {
                (
                    today
                        .year()
                        .checked_add(1)
                        .context("calendar year is out of range")?,
                    1,
                )
            } else {
                (today.year(), today.month() + 1)
            };
            let end = NaiveDate::from_ymd_opt(year, month, 1)
                .context("calendar month end is out of range")?;
            (start, end)
        }
        CalendarWindow::Yearly => {
            let start =
                NaiveDate::from_ymd_opt(today.year(), 1, 1).context("invalid calendar year")?;
            let year = today
                .year()
                .checked_add(1)
                .context("calendar year is out of range")?;
            let end =
                NaiveDate::from_ymd_opt(year, 1, 1).context("calendar year end is out of range")?;
            (start, end)
        }
    };
    Ok((start, end))
}

fn roots(config: &Config, args: &QueryArgs, claude: bool) -> Vec<PathBuf> {
    let explicit = if claude {
        &args.claude_dir
    } else {
        &args.codex_dir
    };
    let mut dirs = explicit.clone();
    if dirs.is_empty() {
        for provider in config.providers.values() {
            let pinned = if claude {
                provider.pinned_claude_config_dir()
            } else {
                provider.pinned_codex_home()
            };
            if let Some(dir) = pinned {
                dirs.push(PathBuf::from(dir));
            }
        }
        let variable = if claude {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        if let Some(dir) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            dirs.push(PathBuf::from(dir));
        } else if let Some(home) = crate::launch::home_dir() {
            dirs.push(home.join(if claude { ".claude" } else { ".codex" }));
        }
    }
    let mut seen = BTreeSet::new();
    dirs.retain(|path| {
        path.is_absolute()
            && seen.insert(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
    });
    if claude {
        dirs.into_iter().map(|root| root.join("projects")).collect()
    } else {
        dirs.into_iter()
            .flat_map(|root| [root.join("sessions"), root.join("archived_sessions")])
            .collect()
    }
}

#[derive(Debug, Serialize)]
struct Reconciled {
    #[serde(flatten)]
    record: UsageRecord,
    provenance: BTreeSet<Source>,
    /// Lacking a verifiable join is not evidence that sources are disjoint.
    possible_overlap: bool,
}

fn reconcile(mut records: Vec<UsageRecord>) -> Vec<Reconciled> {
    // Union exact identities before selecting a winner: a later record can
    // provide the bridge's missing link between two protocol namespaces.
    records.sort_by_key(|record| (record.source, record.granularity));
    let mut parents: Vec<usize> = (0..records.len()).collect();
    fn root(parents: &mut [usize], mut index: usize) -> usize {
        while parents[index] != index {
            parents[index] = parents[parents[index]];
            index = parents[index];
        }
        index
    }
    let mut ids = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        for id in &record.ids {
            if id.id.is_empty() || id.id.len() > 512 || id.id.chars().any(char::is_control) {
                continue;
            }
            if let Some(previous) = ids.insert((record.agent, id.clone()), index) {
                let a = root(&mut parents, previous);
                let b = root(&mut parents, index);
                parents[b.max(a)] = b.min(a);
            }
        }
    }
    let mut groups = BTreeMap::<usize, Reconciled>::new();
    for (index, record) in records.into_iter().enumerate() {
        let index = root(&mut parents, index);
        if let Some(row) = groups.get_mut(&index) {
            row.provenance.insert(record.source);
            for id in record.ids {
                if !row.record.ids.contains(&id) {
                    row.record.ids.push(id);
                }
            }
        } else {
            groups.insert(
                index,
                Reconciled {
                    provenance: BTreeSet::from([record.source]),
                    record,
                    possible_overlap: false,
                },
            );
        }
    }
    let mut result: Vec<_> = groups.into_values().collect();
    recompute_overlap(&mut result);
    result
}

fn overlap_sources<'a>(
    rows: impl Iterator<Item = &'a Reconciled>,
) -> BTreeMap<Agent, BTreeSet<Source>> {
    rows.fold(
        BTreeMap::<Agent, BTreeSet<Source>>::new(),
        |mut map, row| {
            map.entry(row.record.agent)
                .or_default()
                .extend(&row.provenance);
            map
        },
    )
}

fn has_overlap(row: &Reconciled, sources: &BTreeMap<Agent, BTreeSet<Source>>) -> bool {
    sources
        .get(&row.record.agent)
        .is_some_and(|sources| !sources.is_subset(&row.provenance))
}

fn recompute_overlap(rows: &mut [Reconciled]) {
    let sources = overlap_sources(rows.iter());
    for row in rows {
        // Recompute after selection: a source seen only outside the requested
        // range/profile/model is not evidence of overlap within that selection.
        row.possible_overlap = has_overlap(row, &sources);
    }
}

fn read(config_dir: &Path, config: &Config, args: &QueryArgs, query: &Query) -> ReadResult {
    let mut result = ReadResult::default();
    let mut append = |mut source: ReadResult| {
        result.records.append(&mut source.records);
        result.diagnostics.append(&mut source.diagnostics);
    };
    if query.sources.contains(&Source::Alc) {
        append(super::ledger::read_records(config_dir));
    }
    if query.sources.contains(&Source::Claude) {
        append(native::read_claude(&roots(config, args, true)));
    }
    if query.sources.contains(&Source::Codex) {
        append(native::read_codex(&roots(config, args, false)));
    }
    result
}

#[derive(Debug, Serialize)]
pub(crate) struct Statistics {
    pub schema_version: u32,
    pub timezone: String,
    pub range: QueryRange,
    pub window: Option<CalendarWindow>,
    pub pricing_snapshot: String,
    pub rows: Vec<UsageRow>,
    #[serde(rename = "daily_rollups")]
    pub daily: Vec<DailyRollup>,
    pub input_tokens: Option<u64>,
    pub uncached_input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub known_tokens: KnownTokens,
    pub cost_components: CostComponents,
    pub known_subtotal_usd: Option<String>,
    /// None if pricing or reconciliation is incomplete.
    pub total_usd: Option<String>,
    pub currency: &'static str,
    pub records: u64,
    pub priced_records: u64,
    pub unpriced_records: u64,
    pub deduplicated_records: u64,
    pub possible_overlap: bool,
    pub coverage_incomplete: bool,
    pub sources: Vec<SourceDiagnostics>,
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub(crate) subtotal: Option<Money>,
    #[serde(skip)]
    pub(crate) total: Option<Money>,
    #[serde(skip)]
    pub(crate) known_cost: Money,
    /// Calendar months, built like `daily`, for `--monthly` and charts.
    #[serde(skip)]
    pub(crate) monthly: Vec<DailyRollup>,
    #[serde(skip)]
    pub(crate) activity: Activity,
    /// Today in the selected timezone, the reference for "days ago".
    #[serde(skip)]
    pub(crate) today: NaiveDate,
}

/// When and where the selected usage happened, for the wrapped image. Token
/// figures are known sums (see [`KnownTokens`]).
#[derive(Debug, Default)]
pub(crate) struct Activity {
    pub sessions: u64,
    /// Known tokens by hour of day in the selected timezone.
    pub hours: [u64; 24],
    /// Known tokens by recorded provider, or by the model's maker for native
    /// histories that do not record one.
    pub providers: BTreeMap<String, u64>,
}

/// A native history knows the model, not the route; its maker is the best
/// provider label available.
pub(crate) fn provider_label(provider: Option<&str>, model: Option<&str>) -> String {
    if let Some(provider) = provider.filter(|provider| !provider.is_empty()) {
        return provider.to_owned();
    }
    let Some(model) = model.map(str::to_ascii_lowercase) else {
        return "unknown".to_owned();
    };
    let maker = [
        (
            &["claude", "opus", "sonnet", "haiku", "fable"][..],
            "anthropic",
        ),
        (&["gpt", "o1", "o3", "o4", "codex", "chatgpt"], "openai"),
        (&["gemini", "gemma"], "google"),
        (&["qwen"], "qwen"),
        (&["deepseek"], "deepseek"),
        (&["kimi", "moonshot"], "moonshot"),
        (&["glm"], "zai"),
        (&["grok"], "xai"),
        (&["mistral", "codestral", "devstral"], "mistral"),
        (&["minimax"], "minimax"),
        (&["llama"], "meta"),
    ]
    .into_iter()
    .find(|(prefixes, _)| prefixes.iter().any(|prefix| model.starts_with(prefix)))
    .map(|(_, maker)| maker);
    maker.unwrap_or("other").to_owned()
}

#[derive(Debug, Serialize)]
pub(crate) struct DailyRollup {
    pub period: String,
    pub records: u64,
    pub requests: Option<u64>,
    pub known_requests: u64,
    /// The schema-1 gross input view; don't stack this with cached input.
    pub input_tokens: Option<u64>,
    pub uncached_input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_components: CostComponents,
    pub known_subtotal_usd: Option<String>,
    pub total_usd: Option<String>,
    pub priced_records: u64,
    pub partial_records: u64,
    pub unpriced_records: u64,
    pub possible_overlap: bool,
    pub coverage_incomplete: bool,
    /// Display sums; see [`KnownTokens`]. The exact fields above stay strict.
    pub known_tokens: KnownTokens,
    pub agents: BTreeSet<Agent>,
    pub models: BTreeSet<String>,
    #[serde(skip)]
    pub(crate) subtotal: Option<Money>,
    #[serde(skip)]
    pub(crate) total: Option<Money>,
    /// The priced part even under possible overlap, for an approximate display.
    #[serde(skip)]
    pub(crate) known_cost: Money,
}

#[derive(Debug, Serialize)]
pub(crate) struct UsageRow {
    pub period: String,
    pub source: Source,
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub agent: Agent,
    pub model: Option<String>,
    pub granularity: Granularity,
    pub records: u64,
    /// Cumulative deltas/checkpoints do not identify an API request count.
    pub requests: Option<u64>,
    pub known_requests: u64,
    pub input_tokens: Option<u64>,
    pub uncached_input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub known_tokens: KnownTokens,
    pub cost_components: CostComponents,
    pub known_subtotal_usd: String,
    pub total_usd: Option<String>,
    pub priced_records: u64,
    pub partial_records: u64,
    pub unpriced_records: u64,
    pub cost_status: CostStatus,
    pub reference_providers: BTreeSet<String>,
    pub reference_models: BTreeSet<String>,
    pub price_sources: Vec<PriceSource>,
    pub provenance: BTreeSet<Source>,
    pub possible_overlap: bool,
    pub billing_meaning: String,
    pub assumptions: BTreeSet<String>,
    pub reasons: BTreeSet<String>,
    #[serde(skip)]
    pub(crate) known_cost: Money,
}

fn add(total: Option<u64>, value: Option<u64>) -> Option<u64> {
    total?.checked_add(value?)
}

#[derive(Debug)]
struct TokenTotals {
    input: Option<u64>,
    uncached_input: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    output: Option<u64>,
}

impl Default for TokenTotals {
    fn default() -> Self {
        Self {
            input: Some(0),
            uncached_input: Some(0),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(0),
        }
    }
}

impl TokenTotals {
    fn push(&mut self, tokens: &TokenCounts) {
        self.input = add(self.input, tokens.gross_input());
        self.uncached_input = add(self.uncached_input, tokens.uncached_input());
        self.cache_read = add(self.cache_read, tokens.cache_read_tokens);
        self.cache_write = add(self.cache_write, tokens.cache_write_tokens);
        self.output = add(self.output, tokens.output_tokens);
    }

    fn suppress(&mut self) {
        self.input = None;
        self.uncached_input = None;
        self.cache_read = None;
        self.cache_write = None;
        self.output = None;
    }
}

/// What is known, for display: unlike the exact totals beside it, one unknown
/// counter does not erase the rest. With `incomplete` the sums are a lower
/// bound; under possible source overlap they may also count a request twice.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct KnownTokens {
    pub uncached_input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// At least one counter in the group was unknown.
    pub incomplete: bool,
}

impl KnownTokens {
    fn push(&mut self, tokens: &TokenCounts) {
        // An inclusive count with an unknown cache part still bounds the
        // uncached input: whatever cache is unknown is counted as uncached, so
        // gross input stays exact and only the split is uncertain.
        let uncached = tokens.uncached_input().or_else(|| {
            let input = tokens.input_tokens?;
            if tokens.input_basis != InputBasis::Inclusive {
                return None;
            }
            self.incomplete = true;
            Some(
                input
                    .saturating_sub(tokens.cache_read_tokens.unwrap_or(0))
                    .saturating_sub(tokens.cache_write_tokens.unwrap_or(0)),
            )
        });
        for (sum, value) in [
            (&mut self.uncached_input, uncached),
            (&mut self.cache_read, tokens.cache_read_tokens),
            (&mut self.cache_write, tokens.cache_write_tokens),
            (&mut self.output, tokens.output_tokens),
        ] {
            match value {
                Some(value) => *sum = sum.saturating_add(value),
                None => self.incomplete = true,
            }
        }
    }

    pub(crate) fn add(&mut self, other: &Self) {
        self.uncached_input = self.uncached_input.saturating_add(other.uncached_input);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.output = self.output.saturating_add(other.output);
        self.incomplete |= other.incomplete;
    }

    pub(crate) fn input(&self) -> u64 {
        self.uncached_input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    pub(crate) fn total(&self) -> u64 {
        self.input().saturating_add(self.output)
    }
}

#[derive(Debug)]
struct Tally {
    records: u64,
    requests: Option<u64>,
    known_requests: u64,
    tokens: TokenTotals,
    known: KnownTokens,
    cost_components: CostComponents,
    subtotal: Money,
    total: Option<Money>,
    priced_records: u64,
    partial_records: u64,
    unpriced_records: u64,
    possible_overlap: bool,
    agents: BTreeSet<Agent>,
    models: BTreeSet<String>,
}

impl Default for Tally {
    fn default() -> Self {
        Self {
            records: 0,
            requests: Some(0),
            known_requests: 0,
            tokens: TokenTotals::default(),
            known: KnownTokens::default(),
            cost_components: CostComponents::zero(),
            subtotal: Money::ZERO,
            total: Some(Money::ZERO),
            priced_records: 0,
            partial_records: 0,
            unpriced_records: 0,
            possible_overlap: false,
            agents: BTreeSet::new(),
            models: BTreeSet::new(),
        }
    }
}

impl Tally {
    fn push(
        &mut self,
        record: &UsageRecord,
        estimate: &CostEstimate,
        possible_overlap: bool,
    ) -> Result<()> {
        self.records += 1;
        if record.granularity == Granularity::Request {
            self.known_requests += 1;
            self.requests = add(self.requests, Some(1));
        } else {
            self.requests = None;
        }
        // estimate.effective_tokens is also unknown for checkpoints, so no
        // checkpoint counters can leak into either token sums or chart stacks.
        self.tokens.push(&estimate.effective_tokens);
        self.known.push(&estimate.effective_tokens);
        self.possible_overlap |= possible_overlap;
        self.agents.insert(record.agent);
        if let Some(model) = &record.model {
            self.models.insert(model.clone());
        }
        self.subtotal = self
            .subtotal
            .checked_add(estimate.known_subtotal)
            .context("estimated cost overflow")?;
        self.total = match (self.total, estimate.total) {
            (Some(total), Some(cost)) => {
                Some(total.checked_add(cost).context("estimated cost overflow")?)
            }
            _ => None,
        };
        self.cost_components.checked_add(estimate.cost_components)?;
        match estimate.status {
            CostStatus::Complete => self.priced_records += 1,
            CostStatus::Partial => {
                self.partial_records += 1;
                self.unpriced_records += 1;
            }
            CostStatus::Unknown => self.unpriced_records += 1,
        }
        Ok(())
    }

    fn cost_status(&self) -> CostStatus {
        if self.total.is_some() {
            CostStatus::Complete
        } else if self.priced_records > 0 || self.partial_records > 0 {
            CostStatus::Partial
        } else {
            CostStatus::Unknown
        }
    }

    fn daily(mut self, period: String, coverage_incomplete: bool) -> DailyRollup {
        let subtotal = (!self.possible_overlap).then_some(self.subtotal);
        let total = (!self.possible_overlap && !coverage_incomplete)
            .then_some(self.total)
            .flatten();
        if self.possible_overlap {
            self.tokens.suppress();
            self.requests = None;
            self.cost_components.suppress_overlap();
        } else if coverage_incomplete {
            suppress_component_totals(&mut self.cost_components);
        }
        DailyRollup {
            period,
            records: self.records,
            requests: self.requests,
            known_requests: self.known_requests,
            input_tokens: self.tokens.input,
            uncached_input_tokens: self.tokens.uncached_input,
            cache_read_tokens: self.tokens.cache_read,
            cache_write_tokens: self.tokens.cache_write,
            output_tokens: self.tokens.output,
            cost_components: self.cost_components,
            known_subtotal_usd: subtotal.map(Money::to_usd_string),
            total_usd: total.map(Money::to_usd_string),
            priced_records: self.priced_records,
            partial_records: self.partial_records,
            unpriced_records: self.unpriced_records,
            possible_overlap: self.possible_overlap,
            coverage_incomplete,
            known_tokens: self.known,
            agents: self.agents,
            models: self.models,
            subtotal,
            total,
            known_cost: self.subtotal,
        }
    }
}

fn suppress_component_totals(components: &mut CostComponents) {
    components.uncached_input.total_usd = None;
    components.cache_read.total_usd = None;
    components.cache_write.total_usd = None;
    components.output.total_usd = None;
}

#[derive(Default)]
struct UsageGroup {
    tally: Tally,
    reference_providers: BTreeSet<String>,
    reference_models: BTreeSet<String>,
    price_sources: Vec<PriceSource>,
    provenance: BTreeSet<Source>,
    api_equivalent: bool,
    assumptions: BTreeSet<String>,
    reasons: BTreeSet<String>,
}

type GroupKey = (
    String,
    Source,
    Option<String>,
    Option<String>,
    Agent,
    Option<String>,
    Granularity,
);

impl UsageGroup {
    fn push(&mut self, row: &Reconciled, estimate: &CostEstimate) -> Result<()> {
        self.tally
            .push(&row.record, estimate, row.possible_overlap)?;
        self.api_equivalent |= row.record.billing != super::records::Billing::Api;
        if let Some(provider) = &estimate.reference_provider {
            self.reference_providers.insert(provider.clone());
        }
        if let Some(model) = &estimate.reference_model {
            self.reference_models.insert(model.clone());
        }
        if let Some(source) = &estimate.price_source
            && !self
                .price_sources
                .iter()
                .any(|existing| existing.id == source.id && existing.sha256 == source.sha256)
        {
            self.price_sources.push(source.clone());
        }
        self.provenance.extend(&row.provenance);
        self.assumptions
            .extend(estimate.assumptions.iter().cloned());
        self.reasons.extend(estimate.reasons.iter().cloned());
        self.reasons.extend(row.record.warnings.iter().cloned());
        Ok(())
    }

    fn finish(self, key: GroupKey) -> UsageRow {
        let (period, source, profile, provider, agent, model, granularity) = key;
        let cost_status = self.tally.cost_status();
        UsageRow {
            period,
            source,
            profile,
            provider,
            agent,
            model,
            granularity,
            records: self.tally.records,
            requests: self.tally.requests,
            known_requests: self.tally.known_requests,
            input_tokens: self.tally.tokens.input,
            uncached_input_tokens: self.tally.tokens.uncached_input,
            cache_read_tokens: self.tally.tokens.cache_read,
            cache_write_tokens: self.tally.tokens.cache_write,
            output_tokens: self.tally.tokens.output,
            known_tokens: self.tally.known,
            cost_components: self.tally.cost_components,
            known_subtotal_usd: self.tally.subtotal.to_usd_string(),
            total_usd: self.tally.total.map(Money::to_usd_string),
            priced_records: self.tally.priced_records,
            partial_records: self.tally.partial_records,
            unpriced_records: self.tally.unpriced_records,
            cost_status,
            reference_providers: self.reference_providers,
            reference_models: self.reference_models,
            price_sources: self.price_sources,
            provenance: self.provenance,
            possible_overlap: self.tally.possible_overlap,
            billing_meaning: if self.api_equivalent {
                "API-equivalent estimate, not a bill"
            } else {
                "API token-rate estimate"
            }
            .to_owned(),
            assumptions: self.assumptions,
            reasons: self.reasons,
            known_cost: self.tally.subtotal,
        }
    }
}

pub(crate) fn statistics(
    config_dir: &Path,
    config: &Config,
    options: &UsageOptions,
) -> Result<Statistics> {
    statistics_at(config_dir, config, options, Utc::now(), !options.offline)
}

/// `network` allows refreshing LiteLLM's price map; a cached copy is used either way.
fn statistics_at(
    config_dir: &Path,
    config: &Config,
    options: &UsageOptions,
    now: DateTime<Utc>,
    network: bool,
) -> Result<Statistics> {
    if options.window.is_some() && (options.daily || options.monthly) {
        bail!("calendar window cannot be used with --daily or --monthly grouping");
    }
    let query = Query::new(&options.query, true, options.window, now)?;
    let read = read(config_dir, config, &options.query, &query);
    let before = read.records.len();
    // Fold native cumulative baselines and join exact identities across all
    // history, before date/profile/model selection changes the visible scope.
    let records = reconcile(read.records);
    let deduplicated_records = before.saturating_sub(records.len()) as u64;
    let mut records: Vec<_> = records
        .into_iter()
        .filter(|row| query.matches(&row.record, &options.query))
        .collect();
    recompute_overlap(&mut records);
    let mut day_sources = BTreeMap::<String, BTreeMap<Agent, BTreeSet<Source>>>::new();
    let mut month_sources = BTreeMap::<String, BTreeMap<Agent, BTreeSet<Source>>>::new();
    for row in &records {
        for (map, monthly) in [(&mut day_sources, false), (&mut month_sources, true)] {
            map.entry(query.timezone.period(row.record.timestamp_ms, monthly))
                .or_default()
                .entry(row.record.agent)
                .or_default()
                .extend(&row.provenance);
        }
    }
    let mut book = PriceBook::load(config_dir, options.pricing_file.as_deref())?;
    if book.has_gaps(records.iter().map(|row| &row.record)) {
        book = book.with_live(config_dir, network);
    }
    let mut groups = BTreeMap::<GroupKey, UsageGroup>::new();
    let mut days = BTreeMap::<String, Tally>::new();
    let mut months = BTreeMap::<String, Tally>::new();
    let mut overall = Tally::default();
    let mut activity = Activity::default();
    let mut sessions = BTreeSet::new();
    for row in &records {
        let estimate = book.estimate(&row.record);
        let day = query.timezone.period(row.record.timestamp_ms, false);
        let daily_overlap = has_overlap(row, &day_sources[&day]);
        days.entry(day)
            .or_default()
            .push(&row.record, &estimate, daily_overlap)?;
        let month = query.timezone.period(row.record.timestamp_ms, true);
        let monthly_overlap = has_overlap(row, &month_sources[&month]);
        months
            .entry(month)
            .or_default()
            .push(&row.record, &estimate, monthly_overlap)?;
        overall.push(&row.record, &estimate, row.possible_overlap)?;
        let mut known = KnownTokens::default();
        known.push(&estimate.effective_tokens);
        if let Some(at) = timestamp(row.record.timestamp_ms) {
            let hour = &mut activity.hours[query.timezone.hour(at) as usize];
            *hour = hour.saturating_add(known.total());
        }
        let provider = activity
            .providers
            .entry(provider_label(
                row.record.provider.as_deref(),
                row.record.model.as_deref(),
            ))
            .or_default();
        *provider = provider.saturating_add(known.total());
        if let Some(session) = &row.record.session_id {
            sessions.insert((row.record.agent, session.as_str()));
        }
        let period = if options.daily || options.window.is_some() {
            query.timezone.period(row.record.timestamp_ms, false)
        } else if options.monthly {
            query.timezone.period(row.record.timestamp_ms, true)
        } else {
            "all-time".to_owned()
        };
        let key = (
            period,
            row.record.source,
            row.record.profile.clone(),
            row.record.provider.clone(),
            row.record.agent,
            row.record.model.clone(),
            row.record.granularity,
        );
        groups.entry(key).or_default().push(row, &estimate)?;
    }
    let mut warnings = vec!["Token-price estimates exclude taxes, discounts and unrecorded tool charges; historical usage is repriced with the named snapshot.".to_owned()];
    let coverage_incomplete = read.diagnostics.iter().any(|source| {
        source.skipped_lines > 0
            || source.unsupported_records > 0
            || source.ambiguous_records > 0
            || !source.warnings.is_empty()
    });
    if overall.records == 0 {
        overall.total = None;
        overall.tokens.suppress();
        suppress_component_totals(&mut overall.cost_components);
        warnings.push(
            "No matching token records were observed; this is not evidence of zero API spend."
                .to_owned(),
        );
    }
    let subtotal = (!overall.possible_overlap).then_some(overall.subtotal);
    if overall.possible_overlap {
        overall.total = None;
        overall.tokens.suppress();
        overall.cost_components.suppress_overlap();
        warnings.push("Uncorrelated history sources may overlap. Source subtotals are not an additive grand total.".to_owned());
    }
    if coverage_incomplete {
        overall.total = None;
        suppress_component_totals(&mut overall.cost_components);
        warnings.push(
            "Source coverage is incomplete; skipped or ambiguous records are not zero usage."
                .to_owned(),
        );
    }
    if records
        .iter()
        .any(|row| row.record.granularity == Granularity::CumulativeDelta)
    {
        warnings.push("Cumulative deltas are assigned to the later checkpoint's date; activity between checkpoints cannot be reconstructed into original daily usage.".to_owned());
    }
    Ok(Statistics {
        schema_version: 1,
        timezone: query.timezone.name(),
        range: QueryRange {
            start: query.since.and_then(|since| query.timezone.label(since)),
            end: query.until.and_then(|until| query.timezone.label(until)),
        },
        window: options.window,
        pricing_snapshot: book.snapshot_id().to_owned(),
        rows: groups
            .into_iter()
            .map(|(key, group)| group.finish(key))
            .collect(),
        daily: days
            .into_iter()
            .map(|(day, tally)| tally.daily(day, coverage_incomplete))
            .collect(),
        input_tokens: overall.tokens.input,
        uncached_input_tokens: overall.tokens.uncached_input,
        cache_read_tokens: overall.tokens.cache_read,
        cache_write_tokens: overall.tokens.cache_write,
        output_tokens: overall.tokens.output,
        known_tokens: overall.known,
        cost_components: overall.cost_components,
        known_subtotal_usd: subtotal.map(Money::to_usd_string),
        total_usd: overall.total.map(Money::to_usd_string),
        currency: "USD",
        records: overall.records,
        priced_records: overall.priced_records,
        unpriced_records: overall.unpriced_records,
        deduplicated_records,
        possible_overlap: overall.possible_overlap,
        coverage_incomplete,
        sources: read.diagnostics,
        warnings,
        subtotal,
        total: overall.total,
        known_cost: overall.subtotal,
        monthly: months
            .into_iter()
            .map(|(month, tally)| tally.daily(month, coverage_incomplete))
            .collect(),
        activity: Activity {
            sessions: sessions.len() as u64,
            ..activity
        },
        today: query.timezone.date(now),
    })
}

/// Complete integer counts with separators; no floating point or abbreviated suffixes.
pub(crate) fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(digit));
    }
    out
}

fn token_text(value: Option<u64>) -> String {
    value.map(format_count).unwrap_or_else(|| "N/A".to_owned())
}

fn usd_text(subtotal: Option<&str>, total: Option<&str>) -> String {
    match (subtotal, total) {
        (_, Some(total)) => format!("${total}"),
        (Some(subtotal), None) => format!("${subtotal} + ?"),
        (None, None) => "N/A".to_owned(),
    }
}

fn component_text(component: super::pricing::CostComponent) -> String {
    usd_text(
        component
            .known_subtotal_usd
            .map(Money::to_usd_string)
            .as_deref(),
        component.total_usd.map(Money::to_usd_string).as_deref(),
    )
}

fn daily_cells(
    period: &str,
    records: u64,
    tokens: [Option<u64>; 4],
    components: CostComponents,
    subtotal: Option<&str>,
    total: Option<&str>,
) -> Vec<Cell> {
    let mut cells = vec![
        Cell::left(period, Tone::Plain),
        Cell::right(format_count(records), Tone::Plain),
    ];
    cells.extend(tokens.map(|tokens| Cell::right(token_text(tokens), Tone::Plain)));
    cells.extend(
        [
            components.uncached_input,
            components.cache_read,
            components.cache_write,
            components.output,
        ]
        .map(|component| Cell::right(component_text(component), Tone::Plain)),
    );
    cells.push(Cell::right(usd_text(subtotal, total), Tone::Plain));
    cells
}

fn append_table(out: &mut String, table: &Table, theme: &Theme) {
    for line in table.render(theme) {
        out.push_str(&format!("{INDENT}{line}\n"));
    }
}

/// The strict, every-row account behind the summary view: exact sums or N/A,
/// fee components, and each row's assumptions and coverage gaps.
pub(crate) fn render_details(report: &Statistics, theme: &Theme) -> String {
    let mut out = heading_text(theme, "Details: exact token usage and estimated cost (USD)");
    out.push_str(&format!(
        "{INDENT}Daily totals (mutually exclusive token and fee categories)\n"
    ));
    let mut daily = Table::new(vec![
        "DATE",
        "RECORDS",
        "UNCACHED INPUT",
        "CACHE READ",
        "CACHE WRITE",
        "OUTPUT",
        "UNCACHED USD",
        "CACHE READ USD",
        "CACHE WRITE USD",
        "OUTPUT USD",
        "EST. USD",
    ]);
    for row in &report.daily {
        daily.push(daily_cells(
            &row.period,
            row.records,
            [
                row.uncached_input_tokens,
                row.cache_read_tokens,
                row.cache_write_tokens,
                row.output_tokens,
            ],
            row.cost_components,
            row.known_subtotal_usd.as_deref(),
            row.total_usd.as_deref(),
        ));
    }
    if !report.daily.is_empty() {
        daily.push(daily_cells(
            "TOTAL",
            report.records,
            [
                report.uncached_input_tokens,
                report.cache_read_tokens,
                report.cache_write_tokens,
                report.output_tokens,
            ],
            report.cost_components,
            report.known_subtotal_usd.as_deref(),
            report.total_usd.as_deref(),
        ));
    }
    append_table(&mut out, &daily, theme);
    out.push_str(&heading_text(
        theme,
        "Model / source details (gross input includes cache)",
    ));
    let mut table = Table::new(vec![
        "PERIOD",
        "SOURCE",
        "PROFILE",
        "PROVIDER",
        "AGENT",
        "MODEL",
        "GRANULARITY",
        "GROSS INPUT",
        "UNCACHED INPUT",
        "CACHE READ",
        "CACHE WRITE",
        "OUTPUT",
        "EST. USD",
    ]);
    for row in &report.rows {
        table.push(vec![
            Cell::left(&row.period, Tone::Plain),
            Cell::left(row.source.as_str(), Tone::Plain),
            Cell::left(row.profile.as_deref().unwrap_or("unknown"), Tone::Plain),
            Cell::left(row.provider.as_deref().unwrap_or("unknown"), Tone::Plain),
            Cell::left(row.agent.to_string(), Tone::Plain),
            Cell::left(row.model.as_deref().unwrap_or("unknown"), Tone::Plain),
            Cell::left(
                match row.granularity {
                    Granularity::Request => "request",
                    Granularity::CumulativeDelta => "cumulative-delta",
                    Granularity::Checkpoint => "checkpoint",
                },
                Tone::Plain,
            ),
            Cell::right(token_text(row.input_tokens), Tone::Plain),
            Cell::right(token_text(row.uncached_input_tokens), Tone::Plain),
            Cell::right(token_text(row.cache_read_tokens), Tone::Plain),
            Cell::right(token_text(row.cache_write_tokens), Tone::Plain),
            Cell::right(token_text(row.output_tokens), Tone::Plain),
            Cell::right(
                usd_text(Some(&row.known_subtotal_usd), row.total_usd.as_deref()),
                Tone::Plain,
            ),
        ]);
    }
    append_table(&mut out, &table, theme);
    if report.rows.is_empty() {
        out.push_str(&format!("{INDENT}no matching usage records\n"));
    }
    out.push_str(&format!(
        "{INDENT}priced: {}/{} records; snapshot: {}; timezone: {}\n",
        format_count(report.priced_records),
        format_count(report.records),
        report.pricing_snapshot,
        report.timezone,
    ));
    if report.range.start.is_some() || report.range.end.is_some() {
        out.push_str(&format!(
            "{INDENT}range: {} <= timestamp < {}\n",
            report.range.start.as_deref().unwrap_or("unbounded"),
            report.range.end.as_deref().unwrap_or("unbounded")
        ));
    } else {
        out.push_str(&format!(
            "{INDENT}range: all history (no implicit date window)\n"
        ));
    }
    if let Some(total) = &report.total_usd {
        out.push_str(&format!("{INDENT}estimated total: ${total}\n"));
    } else if !report.possible_overlap {
        out.push_str(&format!(
            "{INDENT}known subtotal: ${}; total: N/A\n",
            report.known_subtotal_usd.as_deref().unwrap_or("N/A")
        ));
    }
    for warning in &report.warnings {
        out.push_str(&format!("{INDENT}{warning}\n"));
    }
    for row in &report.rows {
        if row.requests.is_none() {
            out.push_str(&format!("{INDENT}{} / {}: {:?} records have no exact API request count; checkpoints are not summed as usage.\n", row.source.as_str(), row.model.as_deref().unwrap_or("unknown"), row.granularity));
        }
        if row.unpriced_records > 0 {
            out.push_str(&format!(
                "{INDENT}{} / {}: {}\n",
                row.source.as_str(),
                row.model.as_deref().unwrap_or("unknown"),
                row.reasons.iter().cloned().collect::<Vec<_>>().join("; ")
            ));
        }
    }
    let assumptions: BTreeSet<_> = report
        .rows
        .iter()
        .flat_map(|row| &row.assumptions)
        .collect();
    for assumption in assumptions {
        out.push_str(&format!("{INDENT}assumption: {assumption}\n"));
    }
    for source in &report.sources {
        if source.skipped_lines > 0
            || source.unsupported_records > 0
            || source.ambiguous_records > 0
            || !source.warnings.is_empty()
        {
            out.push_str(&format!(
                "{INDENT}{} coverage: {} skipped, {} unsupported, {} ambiguous; {}\n",
                source.source.as_str(),
                format_count(source.skipped_lines),
                format_count(source.unsupported_records),
                format_count(source.ambiguous_records),
                source.warnings.join("; ")
            ));
        }
    }
    out.push_str(&format!(
        "{INDENT}Subscription/native history is an API-equivalent estimate, not actual spend.\n"
    ));
    out
}

#[derive(Debug, Serialize)]
struct TpsReport {
    schema_version: u32,
    measurement: &'static str,
    timezone: String,
    range: QueryRange,
    rows: Vec<TpsRow>,
    summary: TpsSummary,
    coverage: TpsCoverage,
    sources: Vec<SourceDiagnostics>,
}

#[derive(Debug, Default, Serialize)]
struct TpsCoverage {
    matching_records: usize,
    measured_requests: usize,
    /// Disjoint excluded categories, evaluated before sort/limit.
    excluded_legacy_records: usize,
    excluded_unmeasured_records: usize,
    excluded_nonrequest_records: usize,
    eligible_records: usize,
    returned_records: usize,
    limited_records: usize,
    include_unmeasured: bool,
}

#[derive(Debug, Serialize)]
struct TpsRow {
    #[serde(flatten)]
    record: UsageRecord,
    provenance: BTreeSet<Source>,
    metrics: Metrics,
}

#[derive(Debug, Default, Serialize)]
struct TpsSummary {
    requests: Option<usize>,
    records: usize,
    known_requests: usize,
    ttft_samples: usize,
    ttft_mean_ms: Option<f64>,
    ttft_p50_ms: Option<f64>,
    ttft_p95_ms: Option<f64>,
    stream_tps_samples: usize,
    weighted_stream_tps: Option<f64>,
    e2e_tps_samples: usize,
    weighted_e2e_tps: Option<f64>,
}

fn select_tps(
    records: Vec<Reconciled>,
    include_unmeasured: bool,
    limit: u32,
) -> (Vec<TpsRow>, TpsCoverage) {
    let mut coverage = TpsCoverage {
        matching_records: records.len(),
        include_unmeasured,
        ..TpsCoverage::default()
    };
    let mut rows = Vec::new();
    for row in records {
        let category = if row.record.granularity != Granularity::Request {
            1
        } else if row.record.timing.is_none() {
            if row
                .record
                .warnings
                .iter()
                .any(|warning| warning == "legacy request has no timing or correlation IDs")
            {
                2
            } else {
                3
            }
        } else {
            coverage.measured_requests += 1;
            0
        };
        if !include_unmeasured && category != 0 {
            match category {
                1 => coverage.excluded_nonrequest_records += 1,
                2 => coverage.excluded_legacy_records += 1,
                _ => coverage.excluded_unmeasured_records += 1,
            }
            continue;
        }
        // Timing presence, not success or usage presence, defines an observed
        // request. Failures/cancellations and missing usage stay visible.
        let metrics = row.record.metrics();
        rows.push(TpsRow {
            record: row.record,
            provenance: row.provenance,
            metrics,
        });
    }
    coverage.eligible_records = rows.len();
    rows.sort_by_key(|row| std::cmp::Reverse(row.record.timestamp_ms));
    rows.truncate(limit as usize);
    coverage.returned_records = rows.len();
    coverage.limited_records = coverage.eligible_records - coverage.returned_records;
    (rows, coverage)
}

pub(crate) fn run_tps(config_dir: &Path, config: &Config, options: &TpsOptions) -> Result<u8> {
    let query = Query::new(&options.query, false, None, Utc::now())?;
    let read = read(config_dir, config, &options.query, &query);
    let rows = reconcile(read.records)
        .into_iter()
        .filter(|row| query.matches(&row.record, &options.query))
        .collect::<Vec<_>>();
    let (rows, coverage) = select_tps(rows, options.include_unmeasured, options.limit);
    let report = TpsReport {
        schema_version: 1,
        measurement: "client-observed; stream estimate excludes first token; E2E includes queue/network/reasoning, not server decode speed",
        timezone: query.timezone.name(),
        range: QueryRange {
            start: query.since.and_then(|since| query.timezone.label(since)),
            end: query.until.and_then(|until| query.timezone.label(until)),
        },
        summary: tps_summary(&rows),
        coverage,
        rows,
        sources: read.diagnostics,
    };
    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_tps(&report, &Theme::detect()));
    }
    Ok(0)
}

fn tps_summary(rows: &[TpsRow]) -> TpsSummary {
    let mut ttft: Vec<_> = rows.iter().filter_map(|row| row.metrics.ttft_ms).collect();
    ttft.sort_by(f64::total_cmp);
    let percentile = |p: f64| -> Option<f64> {
        (!ttft.is_empty())
            .then(|| ttft[((ttft.len() as f64 * p).ceil() as usize).saturating_sub(1)])
    };
    let known_requests = rows
        .iter()
        .filter(|row| row.record.granularity == Granularity::Request)
        .count();
    let mut summary = TpsSummary {
        requests: (known_requests == rows.len()).then_some(known_requests),
        records: rows.len(),
        known_requests,
        ttft_samples: ttft.len(),
        ttft_mean_ms: (!ttft.is_empty()).then(|| ttft.iter().sum::<f64>() / ttft.len() as f64),
        ttft_p50_ms: percentile(0.5),
        ttft_p95_ms: percentile(0.95),
        ..TpsSummary::default()
    };
    let mut e2e_tokens = 0.0;
    let mut e2e_us = 0.0;
    let mut stream_tokens = 0.0;
    let mut stream_us = 0.0;
    for row in rows {
        let Some(timing) = &row.record.timing else {
            continue;
        };
        if row.metrics.e2e_tps.is_some() {
            summary.e2e_tps_samples += 1;
            e2e_tokens += row.record.tokens.output_tokens.unwrap_or(0) as f64;
            e2e_us += timing.terminal_us.unwrap_or(0) as f64;
        }
        if let Some(tps) = row.metrics.stream_tps {
            summary.stream_tps_samples += 1;
            let first = match timing.output_basis {
                super::records::OutputBasis::NonReasoning => timing.first_visible_us,
                _ => timing.first_content_us,
            };
            let us = timing
                .terminal_us
                .unwrap_or(0)
                .saturating_sub(first.unwrap_or(0)) as f64;
            stream_us += us;
            stream_tokens += tps * us / 1_000_000.0;
        }
    }
    summary.weighted_e2e_tps = (e2e_us > 0.0).then(|| e2e_tokens * 1_000_000.0 / e2e_us);
    summary.weighted_stream_tps =
        (stream_us > 0.0).then(|| stream_tokens * 1_000_000.0 / stream_us);
    summary
}

fn render_tps(report: &TpsReport, theme: &Theme) -> String {
    let mut out = heading_text(theme, "Request performance");
    let mut table = Table::new(vec![
        "PROFILE", "AGENT", "MODEL", "OUTCOME", "TTFT ms", "TPS est.", "BASIS", "TPS E2E",
    ]);
    let value = |metric: Option<f64>| {
        metric
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "N/A".to_owned())
    };
    for row in &report.rows {
        let outcome = serde_json::to_value(row.record.outcome)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        table.push(vec![
            Cell::left(
                row.record.profile.as_deref().unwrap_or("unknown"),
                Tone::Plain,
            ),
            Cell::left(row.record.agent.to_string(), Tone::Plain),
            Cell::left(
                row.record.model.as_deref().unwrap_or("unknown"),
                Tone::Plain,
            ),
            Cell::left(outcome, Tone::Plain),
            Cell::right(value(row.metrics.ttft_ms), Tone::Plain),
            Cell::right(value(row.metrics.stream_tps), Tone::Plain),
            Cell::left(
                match row.metrics.stream_output_basis {
                    OutputBasis::Gross => "gross",
                    OutputBasis::NonReasoning => "non-reasoning",
                    OutputBasis::Unknown => "unknown",
                },
                Tone::Plain,
            ),
            Cell::right(value(row.metrics.e2e_tps), Tone::Plain),
        ]);
    }
    for line in table.render(theme) {
        out.push_str(&format!("{INDENT}{line}\n"));
    }
    if report.rows.is_empty() {
        out.push_str(&format!(
            "{INDENT}no matching measured requests; use a Codex bridge or `alc --metrics <agent>`\n"
        ));
    }
    out.push_str(&format!(
        "{INDENT}coverage: {} matching records; {} measured requests; excluded {} legacy, {} unmeasured, {} non-request; {} shown, {} limited; timezone: {}\n",
        report.coverage.matching_records, report.coverage.measured_requests,
        report.coverage.excluded_legacy_records, report.coverage.excluded_unmeasured_records,
        report.coverage.excluded_nonrequest_records, report.coverage.returned_records,
        report.coverage.limited_records, report.timezone,
    ));
    if !report.coverage.include_unmeasured
        && report.coverage.excluded_legacy_records
            + report.coverage.excluded_unmeasured_records
            + report.coverage.excluded_nonrequest_records
            > 0
    {
        out.push_str(&format!("{INDENT}Use --include-unmeasured to inspect excluded history; missing timing remains N/A.\n"));
    }
    for source in &report.sources {
        if source.skipped_lines > 0
            || source.unsupported_records > 0
            || source.ambiguous_records > 0
            || !source.warnings.is_empty()
        {
            out.push_str(&format!(
                "{INDENT}{} coverage: {} skipped, {} unsupported, {} ambiguous; {}\n",
                source.source.as_str(),
                source.skipped_lines,
                source.unsupported_records,
                source.ambiguous_records,
                source.warnings.join("; ")
            ));
        }
    }
    out.push_str(&format!(
        "{INDENT}TTFT: {} valid samples; mean {} ms, p50 {} ms, p95 {} ms\n",
        report.summary.ttft_samples,
        value(report.summary.ttft_mean_ms),
        value(report.summary.ttft_p50_ms),
        value(report.summary.ttft_p95_ms)
    ));
    out.push_str(&format!(
        "{INDENT}weighted TPS: {} est. ({} samples), {} E2E ({} samples)\n",
        value(report.summary.weighted_stream_tps),
        report.summary.stream_tps_samples,
        value(report.summary.weighted_e2e_tps),
        report.summary.e2e_tps_samples
    ));
    out.push_str(&format!(
        "{INDENT}{}; unavailable timing/token domains are N/A.\n",
        report.measurement
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::super::records::{CorrelationId, Outcome, OutputBasis, Timing};
    use super::*;

    #[test]
    fn known_inclusive_input_survives_an_unknown_cache_counter() {
        let mut known = KnownTokens::default();
        known.push(&TokenCounts {
            input_tokens: Some(100),
            input_basis: InputBasis::Inclusive,
            output_tokens: Some(5),
            cache_read_tokens: Some(30),
            cache_write_tokens: None,
            ..TokenCounts::default()
        });
        assert_eq!(known.input(), 100);
        assert_eq!(known.uncached_input, 70);
        assert_eq!(known.cache_read, 30);
        assert_eq!(known.output, 5);
        assert!(known.incomplete);
    }

    #[test]
    fn dates_are_strict_and_offsets_normalized() {
        assert_eq!(
            parse_bound("2026-10-01", &QueryTimezone::Utc).unwrap(),
            parse_bound("2026-10-01T08:00:00+08:00", &QueryTimezone::Utc).unwrap()
        );
        assert!(parse_bound("2026-02-30", &QueryTimezone::Utc).is_err());
        assert!(parse_bound("2026-13-01", &QueryTimezone::Utc).is_err());
    }

    #[test]
    fn exact_counts_and_calendar_windows_do_not_use_rolling_durations() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(1_234_567), "1,234,567");
        assert_eq!(format_count(u64::MAX), "18,446,744,073,709,551,615");
        let date = |text| NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap();
        assert_eq!(
            calendar_dates(date("2021-01-01"), CalendarWindow::Weekly).unwrap(),
            (date("2020-12-28"), date("2021-01-04"))
        );
        assert_eq!(
            calendar_dates(date("2024-02-29"), CalendarWindow::Monthly).unwrap(),
            (date("2024-02-01"), date("2024-03-01"))
        );
        assert_eq!(
            calendar_dates(date("2024-12-31"), CalendarWindow::Monthly).unwrap(),
            (date("2024-12-01"), date("2025-01-01"))
        );
        assert_eq!(
            calendar_dates(date("2024-02-29"), CalendarWindow::Yearly).unwrap(),
            (date("2024-01-01"), date("2025-01-01"))
        );
        let args = QueryArgs::default();
        let now = DateTime::parse_from_rfc3339("2021-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let all = Query::new(&args, true, None, now).unwrap();
        assert!(all.since.is_none() && all.until.is_none());
        let week = Query::new(&args, true, Some(CalendarWindow::Weekly), now).unwrap();
        assert_eq!(
            week.since,
            Some(parse_bound("2020-12-28", &QueryTimezone::Utc).unwrap())
        );
        assert_eq!(
            week.until,
            Some(parse_bound("2021-01-04", &QueryTimezone::Utc).unwrap())
        );
    }

    #[test]
    fn timezone_midnight_and_daily_buckets_follow_dst_and_offsets() {
        let zone = QueryTimezone::parse("America/New_York").unwrap();
        let start = parse_bound("2024-03-10", &zone).unwrap();
        let end = parse_bound("2024-03-11", &zone).unwrap();
        assert_eq!(end - start, 23 * 60 * 60 * 1000);
        assert_eq!(zone.period(start - 1, false), "2024-03-09");
        assert_eq!(zone.period(start, false), "2024-03-10");
        assert_eq!(zone.period(end - 1, false), "2024-03-10");
        assert_eq!(
            zone.label(start).as_deref(),
            Some("2024-03-10T00:00:00-05:00")
        );
        assert_eq!(
            zone.label(end).as_deref(),
            Some("2024-03-11T00:00:00-04:00")
        );
        let fall =
            parse_bound("2024-11-04", &zone).unwrap() - parse_bound("2024-11-03", &zone).unwrap();
        assert_eq!(fall, 25 * 60 * 60 * 1000);
        let mut args = QueryArgs {
            timezone: "America/New_York".to_owned(),
            ..QueryArgs::default()
        };
        let now = DateTime::parse_from_rfc3339("2024-03-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let week = Query::new(&args, true, Some(CalendarWindow::Weekly), now).unwrap();
        assert_eq!(
            week.until.unwrap() - week.since.unwrap(),
            167 * 60 * 60 * 1000
        );
        args.timezone = "Asia/Taipei".to_owned();
        let now = DateTime::parse_from_rfc3339("2023-12-31T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let year = Query::new(&args, true, Some(CalendarWindow::Yearly), now).unwrap();
        assert_eq!(
            year.timezone.label(year.since.unwrap()).as_deref(),
            Some("2024-01-01T00:00:00+08:00")
        );
        assert!(QueryTimezone::parse("Not/A_Zone").is_err());
        let skipped = QueryTimezone::parse("Pacific/Apia").unwrap();
        assert!(parse_bound("2011-12-30", &skipped).is_err());
    }

    #[test]
    fn clap_presets_conflict_but_chart_accepts_an_empty_missing_path() {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            options: UsageOptions,
        }
        use clap::Parser;
        let bare = Cli::try_parse_from(["usage", "--chart"]).unwrap();
        assert_eq!(bare.options.chart, Some(PathBuf::new()));
        let explicit = Cli::try_parse_from(["usage", "--chart=fixture.png"]).unwrap();
        assert_eq!(explicit.options.chart, Some(PathBuf::from("fixture.png")));
        assert!(Cli::try_parse_from(["usage", "--chart", "fixture.png"]).is_err());
        for args in [
            vec!["usage", "weekly", "--since", "2024-01-01"],
            vec!["usage", "monthly", "--until", "2024-02-01"],
            vec!["usage", "yearly", "--daily"],
            vec!["usage", "weekly", "--monthly"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        let options = Cli::try_parse_from(["usage", "monthly", "--offline"])
            .unwrap()
            .options;
        assert_eq!(options.window, Some(CalendarWindow::Monthly));
        assert_eq!(options.query.timezone, "UTC");
    }

    #[test]
    fn injected_clock_presets_retain_daily_details_and_all_history_stays_unbounded() {
        let dir = tempfile::tempdir().unwrap();
        let parse = |text| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&Utc)
        };
        let mut lines = String::new();
        for text in [
            "2020-12-27T00:00:00Z",
            "2020-12-28T00:00:00Z",
            "2021-01-01T00:00:00Z",
            "2021-01-04T00:00:00Z",
        ] {
            let mut record =
                UsageRecord::new(Source::Alc, Agent::Codex, epoch_ms(parse(text)).unwrap());
            record.provider = Some("openai".to_owned());
            record.model = Some("gpt-4.1".to_owned());
            record.billing = super::super::records::Billing::Api;
            record.tokens = TokenCounts {
                input_tokens: Some(100),
                cache_read_tokens: Some(20),
                output_tokens: Some(10),
                ..TokenCounts::default()
            };
            let entry = serde_json::json!({ "t": "request", "v": 3, "record": record });
            lines.push_str(&format!("{entry}\n"));
        }
        std::fs::write(dir.path().join("usage.jsonl"), lines).unwrap();
        let mut options = UsageOptions::default();
        options.query.source = vec!["alc".to_owned()];
        options.window = Some(CalendarWindow::Weekly);
        let report = statistics_at(
            dir.path(),
            &Config::default(),
            &options,
            parse("2021-01-01T12:00:00Z"),
            false,
        )
        .unwrap();
        assert_eq!(report.records, 2);
        assert_eq!(
            report.range.start.as_deref(),
            Some("2020-12-28T00:00:00+00:00")
        );
        assert_eq!(
            report.range.end.as_deref(),
            Some("2021-01-04T00:00:00+00:00")
        );
        assert_eq!(
            report
                .rows
                .iter()
                .map(|row| row.period.as_str())
                .collect::<Vec<_>>(),
            ["2020-12-28", "2021-01-01"]
        );
        assert_eq!(
            report
                .daily
                .iter()
                .map(|row| row.period.as_str())
                .collect::<Vec<_>>(),
            ["2020-12-28", "2021-01-01"]
        );
        assert_eq!(report.uncached_input_tokens, Some(160));
        assert_eq!(report.cache_write_tokens, Some(0));
        assert!(report.total.is_some());
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["daily_rollups"].as_array().is_some());
        assert!(json.get("subtotal").is_none() && json.get("total").is_none());
        options.window = None;
        let all = statistics_at(
            dir.path(),
            &Config::default(),
            &options,
            parse("2021-01-01T12:00:00Z"),
            false,
        )
        .unwrap();
        assert_eq!(all.records, 4);
        assert_eq!(all.rows.len(), 1);
        assert_eq!(all.rows[0].period, "all-time");
        assert_eq!(all.daily.len(), 4);
        assert!(all.range.start.is_none() && all.range.end.is_none());
    }

    #[test]
    fn overlap_is_recomputed_within_filtered_scope_and_each_day() {
        let alc = UsageRecord::new(Source::Alc, Agent::Claude, 1);
        let claude = UsageRecord::new(Source::Claude, Agent::Claude, 2);
        let mut rows = reconcile(vec![alc, claude]);
        assert!(rows.iter().all(|row| row.possible_overlap));
        rows.retain(|row| row.record.timestamp_ms == 1);
        recompute_overlap(&mut rows);
        assert!(!rows[0].possible_overlap);
        let alc = UsageRecord::new(Source::Alc, Agent::Claude, 1);
        let claude = UsageRecord::new(Source::Claude, Agent::Claude, 2);
        let rows = reconcile(vec![alc, claude]);
        let first_day_sources =
            overlap_sources(rows.iter().filter(|row| row.record.timestamp_ms == 1));
        assert!(!has_overlap(&rows[0], &first_day_sources));
        let same_day_sources = overlap_sources(rows.iter());
        assert!(has_overlap(&rows[0], &same_day_sources));
    }

    #[test]
    fn timed_requests_survive_newer_unmeasured_history_before_limit() {
        let mut timed = UsageRecord::new(Source::Alc, Agent::Claude, 1);
        timed.outcome = Outcome::Cancelled;
        timed.timing = Some(Timing::default());
        let mut records = vec![timed];
        for time in 2..30 {
            let mut legacy = UsageRecord::new(Source::Alc, Agent::Claude, time);
            legacy
                .warnings
                .push("legacy request has no timing or correlation IDs".to_owned());
            records.push(legacy);
        }
        let mut unmeasured = UsageRecord::new(Source::Alc, Agent::Claude, 30);
        unmeasured.tokens = TokenCounts::default();
        records.push(unmeasured);
        let mut checkpoint = UsageRecord::new(Source::Alc, Agent::Claude, 31);
        checkpoint.granularity = Granularity::Checkpoint;
        records.push(checkpoint);
        let (rows, coverage) = select_tps(reconcile(records.clone()), false, 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].record.timestamp_ms, 1);
        assert_eq!(rows[0].record.outcome, Outcome::Cancelled);
        assert_eq!(coverage.measured_requests, 1);
        assert_eq!(coverage.excluded_legacy_records, 28);
        assert_eq!(coverage.excluded_unmeasured_records, 1);
        assert_eq!(coverage.excluded_nonrequest_records, 1);
        assert_eq!(coverage.limited_records, 0);
        let (rows, coverage) = select_tps(reconcile(records), true, 1);
        assert_eq!(rows[0].record.timestamp_ms, 31);
        assert_eq!(
            coverage.excluded_legacy_records
                + coverage.excluded_unmeasured_records
                + coverage.excluded_nonrequest_records,
            0
        );
        assert_eq!(coverage.limited_records, 30);
    }

    #[test]
    fn only_ids_reconcile_sources_not_matching_tokens_or_time() {
        let mut observed = UsageRecord::new(Source::Alc, Agent::Claude, 100);
        observed.ids.push(CorrelationId {
            protocol: "messages".to_owned(),
            id: "msg_same".to_owned(),
        });
        let mut native = UsageRecord::new(Source::Claude, Agent::Claude, 100);
        native.ids = observed.ids.clone();
        let uncorrelated = UsageRecord::new(Source::Claude, Agent::Claude, 100);
        let rows = reconcile(vec![native, uncorrelated, observed]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].record.source, Source::Alc);
        assert_eq!(rows[0].provenance.len(), 2);
        assert!(rows[1].possible_overlap);
    }

    #[test]
    fn protocol_aliases_join_transitively_without_merging_checkpoint_totals() {
        let mut first = UsageRecord::new(Source::Alc, Agent::Claude, 1);
        first.ids.push(CorrelationId {
            protocol: "messages".to_owned(),
            id: "msg_1".to_owned(),
        });
        let mut second = first.clone();
        second.ids = vec![CorrelationId {
            protocol: "responses".to_owned(),
            id: "resp_1".to_owned(),
        }];
        let mut link = UsageRecord::new(Source::Claude, Agent::Claude, 2);
        link.ids = first.ids.iter().chain(&second.ids).cloned().collect();
        let rows = reconcile(vec![first.clone(), second, link]);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].provenance,
            BTreeSet::from([Source::Alc, Source::Claude])
        );
        assert_eq!(rows[0].record.ids.len(), 2);
        assert!(!rows[0].possible_overlap);
        let mut checkpoint = UsageRecord::new(Source::Codex, Agent::Claude, 2);
        checkpoint.granularity = Granularity::Checkpoint;
        let rows = reconcile(vec![first, checkpoint]);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.possible_overlap));
    }

    #[test]
    fn visible_basis_has_the_same_weighted_span_as_individual_metrics() {
        let mut record = UsageRecord::new(Source::Alc, Agent::Qwen, 0);
        record.tokens.output_tokens = Some(6);
        record.tokens.reasoning_tokens = Some(2);
        record.timing = Some(Timing {
            client_streaming: true,
            first_content_us: Some(100_000),
            first_visible_us: Some(1_000_000),
            terminal_us: Some(3_000_000),
            elapsed_us: 3_000_000,
            output_basis: OutputBasis::NonReasoning,
            ..Timing::default()
        });
        let expected = record.metrics().stream_tps;
        let row = TpsRow {
            metrics: record.metrics(),
            record,
            provenance: BTreeSet::from([Source::Alc]),
        };
        assert_eq!(tps_summary(&[row]).weighted_stream_tps, expected);
    }

    #[test]
    fn text_performance_rows_disclose_the_stream_token_basis() {
        let rows = [
            OutputBasis::Gross,
            OutputBasis::NonReasoning,
            OutputBasis::Unknown,
        ]
        .into_iter()
        .map(|basis| {
            let mut record = UsageRecord::new(Source::Alc, Agent::Claude, 0);
            record.timing = Some(Timing {
                output_basis: basis,
                ..Timing::default()
            });
            TpsRow {
                metrics: record.metrics(),
                record,
                provenance: BTreeSet::from([Source::Alc]),
            }
        })
        .collect::<Vec<_>>();
        let report = TpsReport {
            schema_version: 1,
            measurement: "client-observed",
            timezone: "UTC".to_owned(),
            range: QueryRange {
                start: None,
                end: None,
            },
            coverage: TpsCoverage::default(),
            summary: tps_summary(&rows),
            rows,
            sources: Vec::new(),
        };
        let text = render_tps(&report, &Theme::for_test(false, false));
        for label in ["BASIS", "gross", "non-reasoning", "unknown"] {
            assert!(text.contains(label), "{text}");
        }
    }

    #[test]
    fn weighted_speed_is_not_the_sum_of_concurrent_request_speeds() {
        let row = |duration| {
            let mut record = UsageRecord::new(Source::Alc, Agent::Codex, 0);
            record.tokens.output_tokens = Some(10);
            record.outcome = Outcome::Completed;
            record.timing = Some(Timing {
                terminal_us: Some(duration),
                elapsed_us: duration,
                output_basis: OutputBasis::Gross,
                ..Timing::default()
            });
            TpsRow {
                metrics: record.metrics(),
                record,
                provenance: BTreeSet::from([Source::Alc]),
            }
        };
        let summary = tps_summary(&[row(1_000_000), row(3_000_000)]);
        assert_eq!(summary.weighted_e2e_tps, Some(5.0));
    }
}
