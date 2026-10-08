//! CLI-only analytics. The remote quota report deliberately never reads histories.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use clap::Args;
use serde::Serialize;

use super::native;
use super::pricing::{CostStatus, Money, PriceBook, PriceSource};
use super::records::{
    Granularity, Metrics, OutputBasis, ReadResult, Source, SourceDiagnostics, UsageRecord,
};
use crate::config::{Agent, Config};
use crate::doctor::{Cell, INDENT, Table, Theme, Tone, heading_text};

#[derive(Debug, Clone, Default, Args)]
pub(crate) struct QueryArgs {
    /// alc, claude, codex, or all; accepts comma-separated sources.
    #[arg(long, value_delimiter = ',')]
    pub source: Vec<String>,
    /// Inclusive start: YYYY-MM-DD (UTC) or RFC3339.
    #[arg(long, value_name = "DATE")]
    pub since: Option<String>,
    /// Exclusive end: YYYY-MM-DD (UTC) or RFC3339.
    #[arg(long, value_name = "DATE")]
    pub until: Option<String>,
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

#[derive(Debug, Clone, Default, Args)]
pub(crate) struct UsageOptions {
    #[command(flatten)]
    pub query: QueryArgs,
    /// Group estimated usage by UTC day.
    #[arg(long, conflicts_with = "monthly")]
    pub daily: bool,
    /// Group estimated usage by UTC month.
    #[arg(long)]
    pub monthly: bool,
    /// Read only local statistics; do not read credentials or query quota APIs.
    #[arg(long)]
    pub offline: bool,
    /// Override pricing.toml; rates are USD per million tokens.
    #[arg(long, value_name = "PATH")]
    pub pricing_file: Option<PathBuf>,
    /// Print the report as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub(crate) struct TpsOptions {
    #[command(flatten)]
    pub query: QueryArgs,
    /// Number of most recent matching requests to show.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=10000))]
    pub limit: u32,
    /// Print timing, token counters and coverage as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug)]
struct Query {
    sources: BTreeSet<Source>,
    since: Option<u64>,
    until: Option<u64>,
}

impl Query {
    fn new(args: &QueryArgs, default_all: bool) -> Result<Self> {
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
        let since = args.since.as_deref().map(parse_bound).transpose()?;
        let until = args.until.as_deref().map(parse_bound).transpose()?;
        if let (Some(since), Some(until)) = (since, until)
            && since >= until
        {
            bail!("--since must precede the exclusive --until boundary");
        }
        Ok(Self {
            sources,
            since,
            until,
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

fn parse_bound(text: &str) -> Result<u64> {
    let timestamp = if text.len() == 10 {
        let date = NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .context("expected a valid YYYY-MM-DD date")?;
        date.and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    } else {
        DateTime::parse_from_rfc3339(text)
            .context("expected RFC3339 or YYYY-MM-DD (UTC)")?
            .timestamp_millis()
    };
    u64::try_from(timestamp).context("usage dates must be at or after 1970-01-01")
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
    let agents = result.iter().fold(
        BTreeMap::<Agent, BTreeSet<Source>>::new(),
        |mut map, row| {
            map.entry(row.record.agent)
                .or_default()
                .extend(&row.provenance);
            map
        },
    );
    for row in &mut result {
        if let Some(sources) = agents.get(&row.record.agent) {
            // A group missing a source cannot be proved distinct from that
            // source's other records, even if a different group did join.
            row.possible_overlap = !sources.is_subset(&row.provenance);
        }
    }
    result
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
    pub timezone: &'static str,
    pub pricing_snapshot: String,
    pub rows: Vec<UsageRow>,
    pub known_subtotal_usd: Option<String>,
    /// None if pricing or reconciliation is incomplete.
    pub total_usd: Option<String>,
    pub currency: &'static str,
    pub records: u64,
    pub priced_records: u64,
    pub unpriced_records: u64,
    pub deduplicated_records: u64,
    pub possible_overlap: bool,
    pub sources: Vec<SourceDiagnostics>,
    pub warnings: Vec<String>,
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
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
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
    subtotal: Money,
    #[serde(skip)]
    total: Option<Money>,
}

fn add(total: Option<u64>, value: Option<u64>) -> Option<u64> {
    total?.checked_add(value?)
}

pub(crate) fn statistics(
    config_dir: &Path,
    config: &Config,
    options: &UsageOptions,
) -> Result<Statistics> {
    let query = Query::new(&options.query, true)?;
    let read = read(config_dir, config, &options.query, &query);
    let before = read.records.len();
    let records = reconcile(read.records);
    let deduplicated_records = before.saturating_sub(records.len()) as u64;
    let book = PriceBook::load(config_dir, options.pricing_file.as_deref())?;
    let mut rows = BTreeMap::new();
    let mut count = 0;
    let mut possible_overlap = false;
    for row in records
        .into_iter()
        .filter(|row| query.matches(&row.record, &options.query))
    {
        count += 1;
        possible_overlap |= row.possible_overlap;
        let estimate = book.estimate(&row.record);
        let period = period(row.record.timestamp_ms, options);
        let key = (
            period.clone(),
            row.record.source,
            row.record.profile.clone(),
            row.record.provider.clone(),
            row.record.agent,
            row.record.model.clone(),
            row.record.granularity,
        );
        let aggregate = rows.entry(key).or_insert_with(|| UsageRow {
            period,
            source: row.record.source,
            profile: row.record.profile.clone(),
            provider: row.record.provider.clone(),
            agent: row.record.agent,
            model: row.record.model.clone(),
            granularity: row.record.granularity,
            records: 0,
            requests: Some(0),
            known_requests: 0,
            input_tokens: Some(0),
            cache_read_tokens: Some(0),
            cache_write_tokens: Some(0),
            output_tokens: Some(0),
            known_subtotal_usd: "0".to_owned(),
            total_usd: Some("0".to_owned()),
            priced_records: 0,
            partial_records: 0,
            unpriced_records: 0,
            cost_status: CostStatus::Unknown,
            reference_providers: BTreeSet::new(),
            reference_models: BTreeSet::new(),
            price_sources: Vec::new(),
            provenance: BTreeSet::new(),
            possible_overlap: false,
            billing_meaning: match row.record.billing {
                super::records::Billing::Api => "API token-rate estimate",
                _ => "API-equivalent estimate, not a bill",
            }
            .to_owned(),
            assumptions: BTreeSet::new(),
            reasons: BTreeSet::new(),
            subtotal: Money::ZERO,
            total: Some(Money::ZERO),
        });
        aggregate.records += 1;
        if row.record.granularity == Granularity::Request {
            aggregate.known_requests += 1;
            aggregate.requests = aggregate
                .requests
                .and_then(|requests| requests.checked_add(1));
        } else {
            aggregate.requests = None;
        }
        if row.record.granularity == Granularity::Checkpoint {
            aggregate.input_tokens = None;
            aggregate.cache_read_tokens = None;
            aggregate.cache_write_tokens = None;
            aggregate.output_tokens = None;
        } else {
            aggregate.input_tokens = add(aggregate.input_tokens, row.record.tokens.gross_input());
            aggregate.cache_read_tokens = add(
                aggregate.cache_read_tokens,
                row.record.tokens.cache_read_tokens,
            );
            aggregate.cache_write_tokens = add(
                aggregate.cache_write_tokens,
                row.record.tokens.cache_write_tokens,
            );
            aggregate.output_tokens = add(aggregate.output_tokens, row.record.tokens.output_tokens);
        }
        aggregate.possible_overlap |= row.possible_overlap;
        aggregate.subtotal = aggregate
            .subtotal
            .checked_add(estimate.known_subtotal)
            .context("estimated cost overflow")?;
        aggregate.total = match (aggregate.total, estimate.total) {
            (Some(total), Some(cost)) => {
                Some(total.checked_add(cost).context("estimated cost overflow")?)
            }
            _ => None,
        };
        match estimate.status {
            CostStatus::Complete => aggregate.priced_records += 1,
            CostStatus::Partial => {
                aggregate.partial_records += 1;
                aggregate.unpriced_records += 1;
            }
            CostStatus::Unknown => aggregate.unpriced_records += 1,
        }
        if let Some(provider) = estimate.reference_provider {
            aggregate.reference_providers.insert(provider);
        }
        if let Some(model) = estimate.reference_model {
            aggregate.reference_models.insert(model);
        }
        if let Some(source) = estimate.price_source
            && !aggregate
                .price_sources
                .iter()
                .any(|existing| existing.id == source.id && existing.sha256 == source.sha256)
        {
            aggregate.price_sources.push(source);
        }
        aggregate.provenance.extend(row.provenance);
        aggregate.assumptions.extend(estimate.assumptions);
        aggregate.reasons.extend(estimate.reasons);
    }
    let mut subtotal = Money::ZERO;
    let mut total = Some(Money::ZERO);
    let mut priced = 0;
    let mut unpriced = 0;
    let mut rows: Vec<_> = rows.into_values().collect();
    for row in &mut rows {
        row.known_subtotal_usd = row.subtotal.to_usd_string();
        row.total_usd = row.total.map(Money::to_usd_string);
        row.cost_status = if row.total.is_some() {
            CostStatus::Complete
        } else if row.priced_records > 0 || row.partial_records > 0 {
            CostStatus::Partial
        } else {
            CostStatus::Unknown
        };
        subtotal = subtotal
            .checked_add(row.subtotal)
            .context("estimated cost overflow")?;
        total = match (total, row.total) {
            (Some(total), Some(cost)) => {
                Some(total.checked_add(cost).context("estimated cost overflow")?)
            }
            _ => None,
        };
        priced += row.priced_records;
        unpriced += row.unpriced_records;
    }
    let mut warnings = vec!["Token-price estimates exclude taxes, discounts and unrecorded tool charges; historical usage is repriced with the named snapshot.".to_owned()];
    if count == 0 {
        total = None;
        warnings.push(
            "No matching token records were observed; this is not evidence of zero API spend."
                .to_owned(),
        );
    }
    if possible_overlap {
        total = None;
        warnings.push("Uncorrelated history sources may overlap. Source subtotals are not an additive grand total.".to_owned());
    }
    if read.diagnostics.iter().any(|source| {
        source.skipped_lines > 0
            || source.unsupported_records > 0
            || source.ambiguous_records > 0
            || !source.warnings.is_empty()
    }) {
        total = None;
        warnings.push(
            "Source coverage is incomplete; skipped or ambiguous records are not zero usage."
                .to_owned(),
        );
    }
    Ok(Statistics {
        schema_version: 1,
        timezone: "UTC",
        pricing_snapshot: book.snapshot_id().to_owned(),
        rows,
        known_subtotal_usd: (!possible_overlap).then(|| subtotal.to_usd_string()),
        total_usd: total.map(Money::to_usd_string),
        currency: "USD",
        records: count,
        priced_records: priced,
        unpriced_records: unpriced,
        deduplicated_records,
        possible_overlap,
        sources: read.diagnostics,
        warnings,
    })
}

fn period(timestamp_ms: u64, options: &UsageOptions) -> String {
    if !options.daily && !options.monthly {
        return "all-time".to_owned();
    }
    let date = i64::try_from(timestamp_ms)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis);
    date.map(|date| {
        date.format(if options.daily { "%Y-%m-%d" } else { "%Y-%m" })
            .to_string()
    })
    .unwrap_or_else(|| "unknown-date".to_owned())
}

pub(crate) fn render_statistics(report: &Statistics, theme: &Theme) -> String {
    let mut out = heading_text(theme, "Token usage and estimated cost (USD)");
    let mut table = Table::new(vec![
        "PERIOD",
        "SOURCE",
        "PROFILE",
        "AGENT",
        "MODEL",
        "INPUT",
        "CACHE READ",
        "CACHE WRITE",
        "OUTPUT",
        "EST. USD",
    ]);
    let value = |tokens: Option<u64>| {
        tokens
            .map(super::compact_count)
            .unwrap_or_else(|| "N/A".to_owned())
    };
    for row in &report.rows {
        let cost = match &row.total_usd {
            Some(total) => format!("${total}"),
            None if row.priced_records > 0 || row.known_subtotal_usd != "0" => {
                format!("${} + ?", row.known_subtotal_usd)
            }
            None => "N/A".to_owned(),
        };
        table.push(vec![
            Cell::left(row.period.clone(), Tone::Plain),
            Cell::left(row.source.as_str(), Tone::Plain),
            Cell::left(row.profile.as_deref().unwrap_or("unknown"), Tone::Plain),
            Cell::left(row.agent.to_string(), Tone::Plain),
            Cell::left(row.model.as_deref().unwrap_or("unknown"), Tone::Plain),
            Cell::left(value(row.input_tokens), Tone::Plain),
            Cell::left(value(row.cache_read_tokens), Tone::Plain),
            Cell::left(value(row.cache_write_tokens), Tone::Plain),
            Cell::left(value(row.output_tokens), Tone::Plain),
            Cell::left(cost, Tone::Plain),
        ]);
    }
    for line in table.render(theme) {
        out.push_str(&format!("{INDENT}{line}\n"));
    }
    if report.rows.is_empty() {
        out.push_str(&format!("{INDENT}no matching usage records\n"));
    }
    out.push_str(&format!(
        "{INDENT}priced: {}/{} records; snapshot: {}; timezone: UTC\n",
        report.priced_records, report.records, report.pricing_snapshot
    ));
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
                source.skipped_lines,
                source.unsupported_records,
                source.ambiguous_records,
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
    rows: Vec<TpsRow>,
    summary: TpsSummary,
    sources: Vec<SourceDiagnostics>,
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

pub(crate) fn run_tps(config_dir: &Path, config: &Config, options: &TpsOptions) -> Result<u8> {
    let query = Query::new(&options.query, false)?;
    let read = read(config_dir, config, &options.query, &query);
    let mut rows = reconcile(read.records)
        .into_iter()
        .filter(|row| query.matches(&row.record, &options.query))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| std::cmp::Reverse(row.record.timestamp_ms));
    rows.truncate(options.limit as usize);
    let rows: Vec<_> = rows
        .into_iter()
        .map(|row| {
            let metrics = row.record.metrics();
            TpsRow {
                record: row.record,
                provenance: row.provenance,
                metrics,
            }
        })
        .collect();
    let report = TpsReport {
        schema_version: 1,
        measurement: "client-observed; stream estimate excludes first token; E2E includes queue/network/reasoning, not server decode speed",
        summary: tps_summary(&rows),
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
            Cell::left(value(row.metrics.ttft_ms), Tone::Plain),
            Cell::left(value(row.metrics.stream_tps), Tone::Plain),
            Cell::left(
                match row.metrics.stream_output_basis {
                    OutputBasis::Gross => "gross",
                    OutputBasis::NonReasoning => "non-reasoning",
                    OutputBasis::Unknown => "unknown",
                },
                Tone::Plain,
            ),
            Cell::left(value(row.metrics.e2e_tps), Tone::Plain),
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
    fn dates_are_strict_and_offsets_normalized() {
        assert_eq!(
            parse_bound("2026-10-01").unwrap(),
            parse_bound("2026-10-01T08:00:00+08:00").unwrap()
        );
        assert!(parse_bound("2026-02-30").is_err());
        assert!(parse_bound("2026-13-01").is_err());
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
