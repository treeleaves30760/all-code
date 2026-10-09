//! Offline, exact token-price estimates, not invoices or subscription charges.
//!
//! `pricing.toml` is a separate, strict sidecar. Its rates are decimal strings in
//! USD per million tokens (at most six fractional digits). Each `[[models]]`
//! entry requires `provider` and `model`; optional exact selectors are `profile`,
//! `endpoint`, `tier`, `aliases`, `context_min_tokens`, and `context_max_tokens`.
//! Context bounds are inclusive and apply to the entire request, not marginal
//! tokens. Rates are `input`, `output`, `cache_read`, and either `cache_write` or
//! `cache_write_5m`/`cache_write_1h`. A selected override scope replaces the bundled
//! scope, so omitted rates, tiers, or context bands never inherit guessed prices.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::runtime::{bytes_digest, hex_digest};

use super::records::{
    Billing, Granularity, InputBasis, Source, TokenCounts, UsageRecord, is_codex_endpoint,
    official_endpoint_provider,
};

const BUNDLED: &str = include_str!("prices/bundled.toml");
const SOURCE: &str = include_str!("prices/source.toml");
const MAX_OVERRIDE_BYTES: u64 = 1024 * 1024;
const PICO_USD_PER_USD: u128 = 1_000_000_000_000;
const RATE_SCALE: u128 = 1_000_000;

/// Exact pico-USD, with no floating-point conversion or implicit rounding.
///
/// A rate of $0.000001 per million tokens is one pico-USD per token.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Money(u128);

impl Money {
    pub(crate) const ZERO: Self = Self(0);

    pub(crate) const fn from_pico_usd(value: u128) -> Self {
        Self(value)
    }

    pub(crate) const fn pico_usd(self) -> u128 {
        self.0
    }

    pub(crate) fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    pub(crate) fn to_usd_string(self) -> String {
        let whole = self.pico_usd() / PICO_USD_PER_USD;
        let fraction = self.pico_usd() % PICO_USD_PER_USD;
        if fraction == 0 {
            return whole.to_string();
        }
        let fraction = format!("{fraction:012}");
        format!("{whole}.{}", fraction.trim_end_matches('0'))
    }
}

impl fmt::Display for Money {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_usd_string())
    }
}

impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_usd_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CostStatus {
    Complete,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PriceSource {
    pub kind: String,
    pub id: String,
    pub date: Option<String>,
    pub urls: Vec<String>,
    pub sha256: String,
    pub upstream_commit: Option<String>,
    pub upstream_sha256: Option<String>,
    pub license: Option<String>,
    pub notes: Vec<String>,
}

/// A component can have a known subtotal without a complete total. Both are
/// nullable when reconciliation makes even a pooled subtotal unsafe.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct CostComponent {
    pub known_subtotal_usd: Option<Money>,
    pub total_usd: Option<Money>,
}

impl Default for CostComponent {
    fn default() -> Self {
        Self {
            known_subtotal_usd: Some(Money::ZERO),
            total_usd: None,
        }
    }
}

impl CostComponent {
    fn zero() -> Self {
        Self {
            known_subtotal_usd: Some(Money::ZERO),
            total_usd: Some(Money::ZERO),
        }
    }

    fn checked_add(&mut self, other: Self) -> Result<()> {
        let sum = |a: Option<Money>, b: Option<Money>| -> Result<Option<Money>> {
            match (a, b) {
                (Some(a), Some(b)) => Ok(Some(
                    a.checked_add(b)
                        .context("estimated component cost overflow")?,
                )),
                _ => Ok(None),
            }
        };
        self.known_subtotal_usd = sum(self.known_subtotal_usd, other.known_subtotal_usd)?;
        self.total_usd = sum(self.total_usd, other.total_usd)?;
        Ok(())
    }
}

/// Mutually exclusive fees: TTL write buckets are included only in cache_write.
/// Money serializes as the same exact USD strings used by schema-1 totals.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct CostComponents {
    pub uncached_input: CostComponent,
    pub cache_read: CostComponent,
    pub cache_write: CostComponent,
    pub output: CostComponent,
}

impl CostComponents {
    pub(crate) fn zero() -> Self {
        Self {
            uncached_input: CostComponent::zero(),
            cache_read: CostComponent::zero(),
            cache_write: CostComponent::zero(),
            output: CostComponent::zero(),
        }
    }

    pub(crate) fn checked_add(&mut self, other: Self) -> Result<()> {
        self.uncached_input.checked_add(other.uncached_input)?;
        self.cache_read.checked_add(other.cache_read)?;
        self.cache_write.checked_add(other.cache_write)?;
        self.output.checked_add(other.output)
    }

    pub(crate) fn suppress_overlap(&mut self) {
        for component in [
            &mut self.uncached_input,
            &mut self.cache_read,
            &mut self.cache_write,
            &mut self.output,
        ] {
            component.known_subtotal_usd = None;
            component.total_usd = None;
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CostEstimate {
    pub status: CostStatus,
    /// False means no applicable price/reference (N/A), never a free service.
    pub applicable: bool,
    pub currency: &'static str,
    pub known_subtotal_usd: String,
    pub total_usd: Option<String>,
    pub cost_components: CostComponents,
    /// The identical provenance-aware view used for these fees and token sums.
    #[serde(skip)]
    pub effective_tokens: TokenCounts,
    /// Reference identity only; this never changes UsageRecord attribution.
    pub reference_provider: Option<String>,
    pub reference_model: Option<String>,
    pub price_source: Option<PriceSource>,
    pub assumptions: Vec<String>,
    pub reasons: Vec<String>,
    /// Aggregate these with checked_add before formatting; don't parse the JSON strings.
    #[serde(skip)]
    pub known_subtotal: Money,
    #[serde(skip)]
    pub total: Option<Money>,
}

impl CostEstimate {
    fn unknown() -> Self {
        Self {
            status: CostStatus::Unknown,
            applicable: false,
            currency: "USD",
            known_subtotal_usd: "0".to_owned(),
            total_usd: None,
            cost_components: CostComponents::default(),
            effective_tokens: TokenCounts::default(),
            reference_provider: None,
            reference_model: None,
            price_source: None,
            assumptions: vec![
                "Token-only reference estimate; excludes tax, tools, non-token charges, subscriptions and negotiated discounts; not an invoice.".to_owned(),
            ],
            reasons: Vec::new(),
            known_subtotal: Money::ZERO,
            total: None,
        }
    }

    fn finish(&mut self, accumulator: Accumulator) {
        self.known_subtotal = accumulator.subtotal;
        self.known_subtotal_usd = accumulator.subtotal.to_usd_string();
        self.reasons.extend(accumulator.reasons);
        if accumulator.complete {
            self.status = CostStatus::Complete;
            self.total = Some(accumulator.subtotal);
            self.total_usd = Some(self.known_subtotal_usd.clone());
        } else {
            self.status = if accumulator.known_components > 0 {
                CostStatus::Partial
            } else {
                CostStatus::Unknown
            };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rate(u128);

impl Rate {
    fn parse(value: &str) -> Result<Self> {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            || fraction.len() > 6
            || (value.contains('.') && fraction.is_empty())
        {
            bail!("rates must be nonnegative decimal strings with at most six fractional digits");
        }
        let fraction_digits = fraction.len();
        let whole: u128 = whole.parse().context("rate exceeds fixed-point capacity")?;
        let fraction: u128 = if fraction.is_empty() {
            0
        } else {
            fraction.parse().context("invalid fractional rate")?
        };
        let fraction_scale = 10_u128.pow((6 - fraction_digits) as u32);
        whole
            .checked_mul(RATE_SCALE)
            .and_then(|whole| {
                fraction
                    .checked_mul(fraction_scale)
                    .and_then(|fraction| whole.checked_add(fraction))
            })
            .map(Self)
            .context("rate exceeds fixed-point capacity")
    }

    fn cost(self, tokens: u64) -> Option<Money> {
        self.0
            .checked_mul(u128::from(tokens))
            .map(Money::from_pico_usd)
    }
}

#[derive(Debug, Clone, Default)]
struct Rates {
    input: Option<Rate>,
    output: Option<Rate>,
    cache_read: Option<Rate>,
    cache_write: Option<Rate>,
    cache_write_5m: Option<Rate>,
    cache_write_1h: Option<Rate>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PriceFile {
    version: u32,
    currency: String,
    units: String,
    #[serde(default)]
    models: Vec<ModelFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelFile {
    provider: String,
    model: String,
    #[serde(default)]
    aliases: Vec<String>,
    profile: Option<String>,
    endpoint: Option<String>,
    tier: Option<String>,
    context_min_tokens: Option<u64>,
    context_max_tokens: Option<u64>,
    input: Option<String>,
    output: Option<String>,
    cache_read: Option<String>,
    cache_write: Option<String>,
    cache_write_5m: Option<String>,
    cache_write_1h: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFile {
    version: u32,
    id: String,
    date: String,
    upstream_commit: String,
    upstream_date: String,
    upstream_raw_url: String,
    upstream_sha256: String,
    license: String,
    license_url: String,
    provider_urls: Vec<String>,
    notes: Vec<String>,
}

#[derive(Debug, Clone)]
struct Entry {
    provider: String,
    model: String,
    names: BTreeSet<String>,
    profile: Option<String>,
    endpoint: Option<String>,
    tier: String,
    context_min: Option<u64>,
    context_max: Option<u64>,
    rates: Rates,
    source: PriceSource,
    overridden: bool,
}

impl Entry {
    fn matches_identity(&self, provider: &str, model: &str, record: &UsageRecord) -> bool {
        self.provider == provider
            && self.names.contains(model)
            && self
                .profile
                .as_ref()
                .is_none_or(|profile| record.profile.as_ref() == Some(profile))
            && self
                .endpoint
                .as_ref()
                .is_none_or(|endpoint| record.endpoint.as_ref() == Some(endpoint))
    }

    fn same_family(&self, other: &Self) -> bool {
        self.provider == other.provider
            && self.model == other.model
            && self.names == other.names
            && self.profile == other.profile
            && self.endpoint == other.endpoint
    }

    fn specificity(&self) -> usize {
        usize::from(self.profile.is_some()) + usize::from(self.endpoint.is_some())
    }

    fn has_context_band(&self) -> bool {
        self.context_min.is_some() || self.context_max.is_some()
    }

    fn contains_context(&self, gross: u64) -> bool {
        self.context_min.is_none_or(|min| gross >= min)
            && self.context_max.is_none_or(|max| gross <= max)
    }
}

#[derive(Debug)]
pub(crate) struct PriceBook {
    entries: Vec<Entry>,
    overrides: Vec<Entry>,
    snapshot_id: String,
}

impl PriceBook {
    pub(crate) fn load(config_dir: &Path, override_file: Option<&Path>) -> Result<Self> {
        let metadata: SourceFile =
            toml::from_str(SOURCE).context("invalid bundled pricing metadata")?;
        if metadata.version != 1 {
            bail!("unsupported bundled pricing metadata version");
        }
        let mut urls = vec![metadata.upstream_raw_url.clone(), metadata.license_url];
        urls.extend(metadata.provider_urls);
        let mut notes = metadata.notes;
        notes.push(format!(
            "Upstream revision date: {}",
            metadata.upstream_date
        ));
        let source = PriceSource {
            kind: "bundled".to_owned(),
            id: metadata.id.clone(),
            date: Some(metadata.date),
            urls,
            sha256: bytes_digest(BUNDLED.as_bytes()),
            upstream_commit: Some(metadata.upstream_commit),
            upstream_sha256: Some(metadata.upstream_sha256),
            license: Some(metadata.license),
            notes,
        };
        let entries = parse_prices(BUNDLED, source, false).context("invalid bundled price book")?;
        let default_path = config_dir.join("pricing.toml");
        let path = override_file.unwrap_or(&default_path);
        let override_text = match read_override(path) {
            Ok(text) => Some(text),
            Err(error)
                if override_file.is_none()
                    && error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                None
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot read pricing override {}", path.display()));
            }
        };
        let overrides = if let Some(text) = &override_text {
            let hash = bytes_digest(text.as_bytes());
            let source = PriceSource {
                kind: "override".to_owned(),
                id: format!("pricing.toml:sha256:{hash}"),
                date: None,
                urls: Vec::new(),
                sha256: hash,
                upstream_commit: None,
                upstream_sha256: None,
                license: None,
                notes: vec![
                    "User-supplied exact reference rates; no claim of first-party verification."
                        .to_owned(),
                ],
            };
            parse_prices(text, source, true)
                .with_context(|| format!("invalid pricing override {}", path.display()))?
        } else {
            Vec::new()
        };
        let mut hasher = Sha256::new();
        hasher.update(SOURCE.as_bytes());
        hasher.update([0]);
        hasher.update(BUNDLED.as_bytes());
        hasher.update([0]);
        if let Some(text) = override_text {
            hasher.update(text.as_bytes());
        }
        let snapshot_id = format!("{}:sha256:{}", metadata.id, hex_digest(&hasher.finalize()));
        Ok(Self {
            entries,
            overrides,
            snapshot_id,
        })
    }

    pub(crate) fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }

    /// Whether some record names a model this book cannot price, so a lookup
    /// in LiteLLM's map could help.
    pub(crate) fn has_gaps<'a>(&self, mut records: impl Iterator<Item = &'a UsageRecord>) -> bool {
        records.any(|record| record.model.is_some() && !self.estimate(record).applicable)
    }

    /// Adds LiteLLM's runtime price map, the source `npx ccusage` reads, below
    /// the curated snapshot: it only names models the snapshot and any
    /// override leave out, and never replaces a curated rate. A map that does
    /// not validate is ignored rather than failing the report.
    pub(crate) fn with_live(mut self, config_dir: &Path, network: bool) -> Self {
        let Some(snapshot) = super::litellm::load(config_dir, network) else {
            return self;
        };
        let date = chrono::DateTime::<chrono::Utc>::from(snapshot.fetched)
            .format("%Y-%m-%d")
            .to_string();
        let hash = bytes_digest(snapshot.text.as_bytes());
        let source = PriceSource {
            kind: "litellm".to_owned(),
            id: format!("litellm-live-{date}"),
            date: Some(date.clone()),
            urls: vec![super::litellm::URL.to_owned()],
            sha256: hash.clone(),
            upstream_commit: None,
            upstream_sha256: None,
            license: Some("MIT".to_owned()),
            notes: vec!["Fetched at run time from LiteLLM's public price map, as npx ccusage does; fills only models the curated snapshot lacks and is not first-party verified.".to_owned()],
        };
        let known: BTreeSet<(&str, &str)> = self
            .entries
            .iter()
            .chain(&self.overrides)
            .flat_map(|entry| {
                entry
                    .names
                    .iter()
                    .map(|name| (entry.provider.as_str(), name.as_str()))
            })
            .collect();
        let rate =
            |value: &Option<String>| value.as_deref().and_then(|value| Rate::parse(value).ok());
        let live: Vec<Entry> = super::litellm::prices(&snapshot.text)
            .into_iter()
            .filter(|price| !known.contains(&(price.provider.as_str(), price.model.as_str())))
            .map(|price| Entry {
                rates: Rates {
                    input: rate(&price.input),
                    output: rate(&price.output),
                    cache_read: rate(&price.cache_read),
                    cache_write: rate(&price.cache_write),
                    cache_write_5m: rate(&price.cache_write_5m),
                    cache_write_1h: rate(&price.cache_write_1h),
                },
                names: BTreeSet::from([price.model.clone()]),
                provider: price.provider,
                model: price.model,
                profile: None,
                endpoint: None,
                tier: price.tier,
                context_min: price.context_min,
                context_max: price.context_max,
                source: source.clone(),
                overridden: false,
            })
            .collect();
        if live.is_empty() {
            return self;
        }
        let mut entries = self.entries.clone();
        entries.extend(live);
        if validate_entries(&entries).is_err() {
            return self;
        }
        self.entries = entries;
        self.snapshot_id = format!("{}+litellm-live-{date}:sha256:{hash}", self.snapshot_id);
        self
    }

    pub(crate) fn estimate(&self, record: &UsageRecord) -> CostEstimate {
        let view = record.effective_tokens();
        let invalid_cache = view.invalid_cache;
        let tokens = view.counts;
        let mut estimate = CostEstimate::unknown();
        estimate.effective_tokens = tokens.clone();
        estimate.assumptions.extend(view.assumptions);
        let Some(model) = record.model.as_deref().filter(|model| !model.is_empty()) else {
            estimate
                .reasons
                .push("model is unknown; no exact price match".to_owned());
            return estimate;
        };
        // Explicit overrides can price custom/local endpoints, but cannot invent
        // an observed provider when neither attribution nor an official endpoint exists.
        let endpoint_provider = record
            .endpoint
            .as_deref()
            .and_then(official_endpoint_provider);
        let override_provider = record
            .provider
            .as_deref()
            .or(endpoint_provider)
            .or_else(|| {
                (record.source == Source::Codex && record.endpoint.is_none()).then_some("openai")
            });
        let override_family = override_provider
            .and_then(|provider| select_family(&self.overrides, provider, model, record));
        let family = if let Some(family) = override_family {
            estimate.assumptions.push("Exact user override scope replaces bundled rates; unspecified prices remain unknown.".to_owned());
            family
        } else {
            let Some(provider) = self.reference_provider(record, model, &mut estimate) else {
                return estimate;
            };
            let family = select_family(&self.overrides, provider, model, record)
                .or_else(|| select_family(&self.entries, provider, model, record));
            let Some(family) = family else {
                estimate.reasons.push("no verified price for this exact provider/model; aliases are not fuzzy matched".to_owned());
                return estimate;
            };
            if family[0].overridden {
                estimate.assumptions.push("Exact reference-provider override scope replaces bundled rates; unspecified prices remain unknown.".to_owned());
            }
            family
        };
        let reference = family[0];
        estimate.applicable = true;
        estimate.reference_provider = Some(reference.provider.clone());
        estimate.reference_model = Some(reference.model.clone());
        estimate.price_source = Some(reference.source.clone());
        if record.source != Source::Alc
            || record.billing == Billing::ApiEquivalent
            || record.provider.as_deref() == Some("codex")
        {
            estimate.assumptions.push("Native/subscription usage uses API-equivalent reference pricing, not actual subscription billing.".to_owned());
        }
        if record.granularity == Granularity::Checkpoint {
            estimate.status = CostStatus::Partial;
            estimate.reasons.push(
                "checkpoint is not a billable request or deduplicated usage delta".to_owned(),
            );
            return estimate;
        }
        let tier = match record.service_tier.as_deref() {
            None => {
                estimate
                    .assumptions
                    .push("service tier absent; standard tier assumed".to_owned());
                "standard"
            }
            Some("default") if reference.provider == "openai" || reference.provider == "codex" => {
                estimate
                    .assumptions
                    .push("OpenAI actual service_tier=default maps to standard pricing".to_owned());
                "standard"
            }
            Some(tier) => tier,
        };
        let tier_entries: Vec<_> = family
            .into_iter()
            .filter(|entry| entry.tier == tier)
            .collect();
        if tier_entries.is_empty() {
            estimate.status = CostStatus::Partial;
            estimate.reasons.push(
                "actual service tier has no verified rate; standard rates were not substituted"
                    .to_owned(),
            );
            return estimate;
        }
        let has_bands = tier_entries.iter().any(|entry| entry.has_context_band());
        let mut band_from_lower_bound = false;
        let mut band_uncertain = false;
        let entry = if has_bands {
            if record.granularity != Granularity::Request {
                estimate.status = CostStatus::Partial;
                estimate.reasons.push("context-dependent rates require gross input of each request; cumulative deltas cannot select a band".to_owned());
                return estimate;
            }
            // An unknown cache counter leaves only a lower bound on gross
            // input. Rates rise with the band, so the band holding that bound
            // prices the request at no more than it cost; the estimate stays
            // partial.
            let gross = match tokens.gross_input() {
                Some(gross) => Some(gross),
                None => known_gross_input(&tokens).inspect(|_| band_from_lower_bound = true),
            };
            let Some(gross) = gross else {
                estimate.status = CostStatus::Partial;
                estimate.reasons.push(
                    "gross request input is unknown or overflowed; context band cannot be selected"
                        .to_owned(),
                );
                return estimate;
            };
            let Some(entry) = tier_entries
                .iter()
                .copied()
                .find(|entry| entry.contains_context(gross))
            else {
                estimate.status = CostStatus::Partial;
                estimate.reasons.push("gross request input has no verified context band; flat rates were not substituted".to_owned());
                return estimate;
            };
            // A lower bound below the top band could still belong higher up;
            // that is only a lower-bound price if no higher band is cheaper.
            if band_from_lower_bound
                && let Some(max) = entry.context_max
                && tier_entries
                    .iter()
                    .filter(|other| other.context_min.is_some_and(|min| min > max))
                    .any(|other| !entry.rates.never_above(&other.rates))
            {
                estimate.status = CostStatus::Partial;
                estimate.reasons.push("gross request input is only a lower bound and a higher context band is cheaper; no band was selected".to_owned());
                return estimate;
            }
            band_uncertain = band_from_lower_bound && entry.context_max.is_some();
            estimate.assumptions.push("Context band selected from gross input of this request; its rates apply to all request tokens.".to_owned());
            entry
        } else {
            if record.granularity == Granularity::CumulativeDelta {
                estimate.assumptions.push("Flat token rates applied to a cumulative delta; no per-request context tier inferred.".to_owned());
            }
            tier_entries[0]
        };
        let mut input = Accumulator::new();
        input.price("uncached input", tokens.uncached_input(), entry.rates.input);
        let mut output = Accumulator::new();
        output.price("output", tokens.output_tokens, entry.rates.output);
        let mut cache_read = Accumulator::new();
        cache_read.price(
            "cache read",
            tokens.cache_read_tokens,
            entry.rates.cache_read,
        );
        let mut cache_write = Accumulator::new();
        price_cache_write(
            &tokens,
            &entry.rates,
            &mut cache_write,
            &mut estimate.assumptions,
        );
        if band_uncertain {
            // Every component used the lower band's rates, so none is exact.
            for component in [&mut input, &mut output, &mut cache_read, &mut cache_write] {
                component.missing("context band chosen from a lower bound of gross input");
            }
        }
        estimate.cost_components = CostComponents {
            uncached_input: input.component(),
            output: output.component(),
            cache_read: cache_read.component(),
            cache_write: cache_write.component(),
        };
        let mut accumulator = Accumulator::new();
        for component in [input, output, cache_read, cache_write] {
            accumulator.append(component);
        }
        if invalid_cache {
            accumulator
                .missing("invalid cache counters/subsets were left unknown rather than priced");
        }
        if band_uncertain {
            accumulator.missing("an unknown cache counter left gross input as a lower bound; the context band it selects gives a lower-bound cost");
        }
        if tokens.reasoning_tokens.is_some() {
            estimate.assumptions.push(
                "Reasoning tokens are a subset of output and are not billed again.".to_owned(),
            );
        }
        estimate.finish(accumulator);
        estimate
    }

    fn reference_provider<'a>(
        &'a self,
        record: &UsageRecord,
        model: &str,
        estimate: &mut CostEstimate,
    ) -> Option<&'a str> {
        let endpoint = record.endpoint.as_deref();
        let official = endpoint.and_then(official_endpoint_provider);
        match record.provider.as_deref() {
            Some("codex") => {
                if endpoint.is_some()
                    && official != Some("openai")
                    && !endpoint.is_some_and(is_codex_endpoint)
                {
                    estimate.reasons.push(
                        "custom Codex endpoint has no exact override; cost is N/A, not free"
                            .to_owned(),
                    );
                    return None;
                }
                estimate.assumptions.push("Codex has an explicit OpenAI API-equivalent reference; actual provider attribution is unchanged.".to_owned());
                Some("openai")
            }
            Some(provider @ ("openai" | "anthropic")) => {
                if endpoint.is_some() && official != Some(provider) {
                    estimate.reasons.push("endpoint does not match the first-party provider and has no exact override; cost is N/A, not free".to_owned());
                    return None;
                }
                if endpoint.is_none() {
                    estimate.assumptions.push("Endpoint absent; reference uses the recorded provider's first-party API rates.".to_owned());
                }
                Some(if provider == "openai" {
                    "openai"
                } else {
                    "anthropic"
                })
            }
            Some(_) => {
                if let Some(provider) = official {
                    estimate.assumptions.push("Exact official endpoint supplies a reference only; observed custom/local provider attribution is unchanged.".to_owned());
                    Some(provider)
                } else {
                    estimate.reasons.push("custom/local or unsupported provider has no matching official endpoint or explicit override; cost is N/A, not free".to_owned());
                    None
                }
            }
            None => {
                if let Some(provider) = official {
                    estimate.assumptions.push("Exact official endpoint supplies reference pricing only; actual provider is still unknown.".to_owned());
                    return Some(provider);
                }
                if record.source == Source::Codex
                    && (endpoint.is_none() || endpoint.is_some_and(is_codex_endpoint))
                {
                    estimate.assumptions.push("Native Codex uses an explicit OpenAI reference; actual provider attribution is still unknown.".to_owned());
                    return Some("openai");
                }
                if endpoint.is_some() {
                    estimate.reasons.push(
                        "unknown/custom endpoint has no exact override; cost is N/A, not free"
                            .to_owned(),
                    );
                    return None;
                }
                if record.source == Source::Claude {
                    let providers: BTreeSet<_> = self
                        .entries
                        .iter()
                        .filter(|entry| entry.names.contains(model))
                        .map(|entry| entry.provider.as_str())
                        .collect();
                    if providers.len() == 1 {
                        estimate.assumptions.push("Exact official model alias supplies native API-equivalent reference pricing only; actual provider attribution is still unknown.".to_owned());
                        return providers.into_iter().next();
                    }
                }
                estimate.reasons.push(
                    "provider attribution is unknown and no unique exact native reference exists"
                        .to_owned(),
                );
                None
            }
        }
    }
}

/// Gross input from the counters that are known, when the uncached part is.
fn known_gross_input(tokens: &TokenCounts) -> Option<u64> {
    let input = tokens.input_tokens?;
    match tokens.input_basis {
        InputBasis::Inclusive => Some(input),
        InputBasis::Separate => input
            .checked_add(tokens.cache_read_tokens.unwrap_or(0))?
            .checked_add(tokens.cache_write_tokens.unwrap_or(0)),
    }
}

fn read_override(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_OVERRIDE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OVERRIDE_BYTES {
        bail!("pricing override exceeds 1 MiB");
    }
    String::from_utf8(bytes).context("pricing override must be UTF-8")
}

fn parse_prices(text: &str, source: PriceSource, overridden: bool) -> Result<Vec<Entry>> {
    let file: PriceFile = toml::from_str(text).context("invalid pricing TOML")?;
    if file.version != 1 || file.currency != "USD" || file.units != "USD-per-million-tokens" {
        bail!("pricing requires version=1, currency=USD, units=USD-per-million-tokens");
    }
    let mut entries = Vec::with_capacity(file.models.len());
    for model in file.models {
        validate_selector(&model.provider, "provider")?;
        validate_selector(&model.model, "model")?;
        for (name, value) in [
            ("profile", &model.profile),
            ("endpoint", &model.endpoint),
            ("tier", &model.tier),
        ] {
            if let Some(value) = value {
                validate_selector(value, name)?;
            }
        }
        if let Some(endpoint) = &model.endpoint {
            let url = reqwest::Url::parse(endpoint)
                .context("override endpoint must be an absolute URL")?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                bail!(
                    "override endpoint must be an HTTP(S) URL without credentials, query or fragment"
                );
            }
        }
        if let (Some(min), Some(max)) = (model.context_min_tokens, model.context_max_tokens)
            && min > max
        {
            bail!("context_min_tokens exceeds context_max_tokens");
        }
        if model.cache_write.is_some()
            && (model.cache_write_5m.is_some() || model.cache_write_1h.is_some())
        {
            bail!("flat cache_write and differentiated TTL write rates are mutually exclusive");
        }
        let rate = |name: &str, value: Option<String>| -> Result<Option<Rate>> {
            value
                .map(|value| Rate::parse(&value).with_context(|| format!("invalid {name} rate")))
                .transpose()
        };
        let rates = Rates {
            input: rate("input", model.input)?,
            output: rate("output", model.output)?,
            cache_read: rate("cache_read", model.cache_read)?,
            cache_write: rate("cache_write", model.cache_write)?,
            cache_write_5m: rate("cache_write_5m", model.cache_write_5m)?,
            cache_write_1h: rate("cache_write_1h", model.cache_write_1h)?,
        };
        if [
            rates.input,
            rates.output,
            rates.cache_read,
            rates.cache_write,
            rates.cache_write_5m,
            rates.cache_write_1h,
        ]
        .iter()
        .all(Option::is_none)
        {
            bail!("model entry requires at least one explicit rate");
        }
        let mut names = BTreeSet::from([model.model.clone()]);
        for alias in model.aliases {
            validate_selector(&alias, "alias")?;
            if !names.insert(alias) {
                bail!("duplicate model alias");
            }
        }
        let tier = model.tier.unwrap_or_else(|| "standard".to_owned());
        let tier = if matches!(model.provider.as_str(), "openai" | "codex") && tier == "default" {
            "standard".to_owned()
        } else {
            tier
        };
        entries.push(Entry {
            provider: model.provider,
            model: model.model,
            names,
            profile: model.profile,
            endpoint: model.endpoint,
            tier,
            context_min: model.context_min_tokens,
            context_max: model.context_max_tokens,
            rates,
            source: source.clone(),
            overridden,
        });
    }
    validate_entries(&entries)?;
    Ok(entries)
}

fn validate_selector(value: &str, name: &str) -> Result<()> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        bail!(
            "{name} selector must be nonempty without surrounding whitespace or control characters"
        );
    }
    Ok(())
}

fn selectors_overlap(left: &Option<String>, right: &Option<String>) -> bool {
    left.is_none() || right.is_none() || left == right
}

fn scope_contains(broader: &Entry, narrower: &Entry) -> bool {
    (broader.profile.is_none() || broader.profile == narrower.profile)
        && (broader.endpoint.is_none() || broader.endpoint == narrower.endpoint)
}

fn validate_entries(entries: &[Entry]) -> Result<()> {
    for (index, left) in entries.iter().enumerate() {
        for right in &entries[index + 1..] {
            if left.provider != right.provider
                || left.names.is_disjoint(&right.names)
                || !selectors_overlap(&left.profile, &right.profile)
                || !selectors_overlap(&left.endpoint, &right.endpoint)
            {
                continue;
            }
            if !scope_contains(left, right) && !scope_contains(right, left) {
                bail!(
                    "ambiguous pricing selectors: overlapping profile-only and endpoint-only scopes"
                );
            }
            if left.profile != right.profile || left.endpoint != right.endpoint {
                continue;
            }
            if left.model != right.model || left.names != right.names {
                bail!(
                    "ambiguous pricing aliases: one exact name belongs to inconsistent model families"
                );
            }
            if left.tier != right.tier {
                continue;
            }
            let min = left
                .context_min
                .unwrap_or(0)
                .max(right.context_min.unwrap_or(0));
            let max = left
                .context_max
                .unwrap_or(u64::MAX)
                .min(right.context_max.unwrap_or(u64::MAX));
            if min <= max {
                bail!(
                    "ambiguous pricing selectors: overlapping context bands or duplicate service tier"
                );
            }
        }
    }
    Ok(())
}

fn select_family<'a>(
    entries: &'a [Entry],
    provider: &str,
    model: &str,
    record: &UsageRecord,
) -> Option<Vec<&'a Entry>> {
    let reference = entries
        .iter()
        .filter(|entry| entry.matches_identity(provider, model, record))
        .max_by_key(|entry| entry.specificity())?;
    Some(
        entries
            .iter()
            .filter(|entry| entry.same_family(reference))
            .collect(),
    )
}

struct Accumulator {
    subtotal: Money,
    known_components: usize,
    complete: bool,
    reasons: Vec<String>,
}

impl Accumulator {
    fn new() -> Self {
        Self {
            subtotal: Money::ZERO,
            known_components: 0,
            complete: true,
            reasons: Vec::new(),
        }
    }

    fn component(&self) -> CostComponent {
        CostComponent {
            known_subtotal_usd: Some(self.subtotal),
            total_usd: self.complete.then_some(self.subtotal),
        }
    }

    fn append(&mut self, component: Self) {
        self.complete &= component.complete;
        self.reasons.extend(component.reasons);
        if let Some(subtotal) = self.subtotal.checked_add(component.subtotal) {
            self.subtotal = subtotal;
            self.known_components += component.known_components;
        } else {
            self.missing("component subtotal exceeds fixed-point capacity");
        }
    }

    fn missing(&mut self, reason: &str) {
        self.complete = false;
        self.reasons.push(reason.to_owned());
    }

    fn price(&mut self, label: &str, tokens: Option<u64>, rate: Option<Rate>) {
        let Some(tokens) = tokens else {
            self.missing(&format!("{label} token count is unknown"));
            return;
        };
        if tokens == 0 {
            // Explicit zero usage needs no rate; it doesn't make absent usage zero.
            self.known_components += 1;
            return;
        }
        let Some(rate) = rate else {
            self.missing(&format!("{label} rate is unknown for positive usage"));
            return;
        };
        let Some(cost) = rate.cost(tokens) else {
            self.missing(&format!("{label} cost exceeds fixed-point capacity"));
            return;
        };
        let Some(subtotal) = self.subtotal.checked_add(cost) else {
            self.missing(&format!("{label} subtotal exceeds fixed-point capacity"));
            return;
        };
        self.subtotal = subtotal;
        self.known_components += 1;
    }
}

impl Rates {
    /// No rate here exceeds the matching rate in `higher`, where both exist.
    fn never_above(&self, higher: &Self) -> bool {
        [
            (self.input, higher.input),
            (self.output, higher.output),
            (self.cache_read, higher.cache_read),
            (self.cache_write, higher.cache_write),
            (self.cache_write_5m, higher.cache_write_5m),
            (self.cache_write_1h, higher.cache_write_1h),
        ]
        .into_iter()
        .all(|pair| match pair {
            (Some(low), Some(high)) => low.0 <= high.0,
            _ => true,
        })
    }
}

fn price_cache_write(
    tokens: &TokenCounts,
    rates: &Rates,
    accumulator: &mut Accumulator,
    assumptions: &mut Vec<String>,
) {
    if rates.cache_write.is_some()
        || (rates.cache_write_5m.is_none() && rates.cache_write_1h.is_none())
    {
        accumulator.price("cache write", tokens.cache_write_tokens, rates.cache_write);
        return;
    }
    let short = tokens.cache_write_5m_tokens;
    let long = tokens.cache_write_1h_tokens;
    let Some(total) = tokens.cache_write_tokens else {
        accumulator.price("5m cache write", short, rates.cache_write_5m);
        accumulator.price("1h cache write", long, rates.cache_write_1h);
        return;
    };
    if short.is_some_and(|short| short > total)
        || long.is_some_and(|long| long > total)
        || matches!((short, long), (Some(short), Some(long)) if short.checked_add(long) != Some(total))
    {
        accumulator
            .missing("cache-write TTL buckets do not match the aggregate; writes left unpriced");
        return;
    }
    if total == 0 {
        accumulator.price("cache write", Some(0), None);
        return;
    }
    let (short, long) = match (short, long) {
        (Some(short), Some(long)) => (short, long),
        (Some(short), None) => {
            assumptions.push(
                "1h cache-write count derived from aggregate minus reported 5m count.".to_owned(),
            );
            (short, total - short)
        }
        (None, Some(long)) => {
            assumptions.push(
                "5m cache-write count derived from aggregate minus reported 1h count.".to_owned(),
            );
            (total - long, long)
        }
        (None, None)
            if rates.cache_write_5m.is_some() && rates.cache_write_5m == rates.cache_write_1h =>
        {
            assumptions.push("Equal verified 5m/1h write rates permit pricing the aggregate without guessing TTL.".to_owned());
            accumulator.price("cache write", Some(total), rates.cache_write_5m);
            return;
        }
        (None, None) => {
            accumulator.missing(
                "cache-write TTL breakdown is unknown; differentiated 5m/1h rates were not guessed",
            );
            return;
        }
    };
    // The TTL buckets replace the aggregate; it is never charged again.
    accumulator.price("5m cache write", Some(short), rates.cache_write_5m);
    accumulator.price("1h cache write", Some(long), rates.cache_write_1h);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Agent;
    use crate::usage::records::InputBasis;

    fn record(provider: Option<&str>, model: &str, basis: InputBasis) -> UsageRecord {
        let mut record = UsageRecord::new(Source::Alc, Agent::Claude, 0);
        record.provider = provider.map(str::to_owned);
        record.model = Some(model.to_owned());
        record.billing = Billing::Api;
        record.tokens = TokenCounts {
            input_tokens: Some(0),
            input_basis: basis,
            output_tokens: Some(0),
            cache_read_tokens: Some(0),
            cache_write_tokens: Some(0),
            ..TokenCounts::default()
        };
        record
    }

    fn overridden(text: &str) -> (tempfile::TempDir, PriceBook) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pricing.toml"), text).unwrap();
        let book = PriceBook::load(dir.path(), None).unwrap();
        (dir, book)
    }

    fn sidecar(models: &str) -> String {
        format!("version=1\ncurrency=\"USD\"\nunits=\"USD-per-million-tokens\"\n{models}")
    }

    #[test]
    fn one_token_and_fractional_sums_are_exact_decimal_strings() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="custom"
model="tiny"
input="0.000001"
output="0.000002"
cache_read="0"
cache_write="0"
"#,
        ));
        let mut record = record(Some("custom"), "tiny", InputBasis::Inclusive);
        record.tokens.input_tokens = Some(1);
        record.tokens.output_tokens = Some(1);
        let estimate = book.estimate(&record);
        assert_eq!(estimate.status, CostStatus::Complete);
        assert_eq!(estimate.total_usd.as_deref(), Some("0.000000000003"));
        assert_eq!(estimate.known_subtotal.pico_usd(), 3);
        let sum = estimate
            .known_subtotal
            .checked_add(estimate.known_subtotal)
            .unwrap();
        assert_eq!(sum.to_usd_string(), "0.000000000006");
        assert_eq!(serde_json::to_string(&sum).unwrap(), "\"0.000000000006\"");
        let json = serde_json::to_value(&estimate).unwrap();
        assert_eq!(json["status"], "complete");
        assert_eq!(json["currency"], "USD");
        assert_eq!(json["known_subtotal_usd"], "0.000000000003");
        assert!(json.get("known_subtotal").is_none());
    }

    #[test]
    fn price_components_use_the_same_effective_view_and_exact_ttl_subtotals() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="custom"
model="ttl"
input="2"
output="8"
cache_read="0.5"
cache_write_5m="3"
cache_write_1h="6"
"#,
        ));
        let mut record = record(Some("custom"), "ttl", InputBasis::Separate);
        record.tokens.input_tokens = Some(30);
        record.tokens.cache_read_tokens = Some(60);
        record.tokens.cache_write_tokens = None;
        record.tokens.cache_write_5m_tokens = Some(4);
        record.tokens.cache_write_1h_tokens = Some(6);
        record.tokens.output_tokens = Some(5);
        let estimate = book.estimate(&record);
        assert_eq!(estimate.effective_tokens.cache_write_tokens, Some(10));
        assert_eq!(estimate.effective_tokens.gross_input(), Some(100));
        let components = estimate.cost_components;
        assert_eq!(
            components.uncached_input.total_usd.unwrap().to_usd_string(),
            "0.00006"
        );
        assert_eq!(
            components.cache_read.total_usd.unwrap().to_usd_string(),
            "0.00003"
        );
        assert_eq!(
            components.cache_write.total_usd.unwrap().to_usd_string(),
            "0.000048"
        );
        assert_eq!(
            components.output.total_usd.unwrap().to_usd_string(),
            "0.00004"
        );
        assert_eq!(estimate.total_usd.as_deref(), Some("0.000178"));
        record.tokens.cache_write_tokens = Some(10);
        record.tokens.cache_write_5m_tokens = None;
        record.tokens.cache_write_1h_tokens = None;
        let partial = book.estimate(&record);
        assert_eq!(
            partial.cost_components.cache_write.known_subtotal_usd,
            Some(Money::ZERO)
        );
        assert_eq!(partial.cost_components.cache_write.total_usd, None);
        assert_eq!(partial.known_subtotal_usd, "0.00013");
        assert_eq!(partial.total, None);
    }

    #[test]
    fn price_override_does_not_change_a_verified_effective_token_schema() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="openai"
model="gpt-4.1"
input="2"
output="8"
cache_read="0.5"
"#,
        ));
        let mut record = record(Some("openai"), "gpt-4.1", InputBasis::Inclusive);
        record.tokens.input_tokens = Some(100);
        record.tokens.cache_read_tokens = Some(20);
        record.tokens.cache_write_tokens = None;
        let estimate = book.estimate(&record);
        assert_eq!(estimate.effective_tokens.cache_write_tokens, Some(0));
        assert_eq!(estimate.total_usd.as_deref(), Some("0.00017"));
        assert_eq!(
            estimate.cost_components.cache_write.total_usd,
            Some(Money::ZERO)
        );
    }

    #[test]
    fn explicit_zero_is_not_missing_and_missing_usage_is_not_zero() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="custom"
model="free"
input="0"
output="0"
cache_read="0"
cache_write="0"
"#,
        ));
        let mut record = record(Some("custom"), "free", InputBasis::Inclusive);
        record.tokens.input_tokens = Some(100);
        record.tokens.output_tokens = Some(10);
        let zero = book.estimate(&record);
        assert_eq!(zero.status, CostStatus::Complete);
        assert_eq!(zero.total, Some(Money::ZERO));
        record.tokens.cache_read_tokens = None;
        let unknown = book.estimate(&record);
        assert_eq!(unknown.status, CostStatus::Partial);
        assert_eq!(unknown.total_usd, None);
        assert!(
            unknown
                .reasons
                .iter()
                .any(|reason| reason.contains("cache read token count is unknown"))
        );
    }

    #[test]
    fn invalid_rate_strings_and_numeric_rates_are_rejected() {
        for rate in [
            "-1",
            "-0",
            "nan",
            "NaN",
            "inf",
            "+1",
            "1e3",
            "",
            "1.",
            ".5",
            "1.0000001",
            " 1",
            "340282366920938463463374607431768211456",
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("pricing.toml"),
                sidecar(&format!(
                    "[[models]]\nprovider=\"custom\"\nmodel=\"x\"\ninput=\"{rate}\"\n"
                )),
            )
            .unwrap();
            assert!(
                PriceBook::load(dir.path(), None).is_err(),
                "accepted rate {rate:?}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pricing.toml"),
            sidecar("[[models]]\nprovider=\"custom\"\nmodel=\"x\"\ninput=1.5\n"),
        )
        .unwrap();
        assert!(PriceBook::load(dir.path(), None).is_err());
    }

    #[test]
    fn overflow_is_checked_without_wrapping_or_saturating_totals() {
        assert!(
            Money::from_pico_usd(u128::MAX)
                .checked_add(Money::from_pico_usd(1))
                .is_none()
        );
        assert_eq!(
            Rate::parse("340282366920938463463374607431768.211455")
                .unwrap()
                .0,
            u128::MAX
        );
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="custom"
model="overflow"
input="340282366920938463463374607431768.211455"
output="0.000001"
"#,
        ));
        let mut record = record(Some("custom"), "overflow", InputBasis::Separate);
        record.tokens.input_tokens = Some(2);
        record.tokens.output_tokens = Some(1);
        let estimate = book.estimate(&record);
        assert_eq!(estimate.status, CostStatus::Partial);
        assert_eq!(estimate.known_subtotal_usd, "0.000000000001");
        assert!(
            estimate
                .reasons
                .iter()
                .any(|reason| reason.contains("capacity"))
        );
        record.tokens.input_tokens = Some(1);
        let estimate = book.estimate(&record);
        assert_eq!(estimate.known_subtotal.pico_usd(), u128::MAX);
        assert_eq!(estimate.total, None);
    }

    #[test]
    fn conflicting_selectors_aliases_bands_and_write_modes_are_errors() {
        let cases = [
            r#"
[[models]]
provider="custom"
model="x"
input="1"
[[models]]
provider="custom"
model="x"
input="2"
"#,
            r#"
[[models]]
provider="custom"
model="x"
profile="work"
input="1"
[[models]]
provider="custom"
model="x"
endpoint="https://example.invalid/v1"
input="2"
"#,
            r#"
[[models]]
provider="custom"
model="x"
aliases=["shared"]
input="1"
[[models]]
provider="custom"
model="y"
aliases=["shared"]
input="2"
"#,
            r#"
[[models]]
provider="custom"
model="x"
context_max_tokens=100
input="1"
[[models]]
provider="custom"
model="x"
context_min_tokens=100
input="2"
"#,
            r#"
[[models]]
provider="custom"
model="x"
input="1"
cache_write="1"
cache_write_5m="1.25"
"#,
            r#"
[[models]]
provider="custom"
model="x"
input="1"
unknown_rate="2"
"#,
        ];
        for text in cases {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("pricing.toml"), sidecar(text)).unwrap();
            assert!(
                PriceBook::load(dir.path(), None).is_err(),
                "accepted conflicting sidecar: {text}"
            );
        }
    }

    #[test]
    fn specific_overrides_replace_scopes_without_implicit_rate_fallbacks() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="openai"
model="gpt-4.1"
input="9"
output="8"
cache_read="0.5"
cache_write="9"
[[models]]
provider="openai"
model="gpt-4.1"
profile="work"
input="1"
"#,
        ));
        let mut record = record(Some("openai"), "gpt-4.1", InputBasis::Inclusive);
        record.tokens.input_tokens = Some(1_000_000);
        assert_eq!(book.estimate(&record).total_usd.as_deref(), Some("9"));
        record.profile = Some("work".to_owned());
        record.tokens.output_tokens = Some(1);
        let estimate = book.estimate(&record);
        assert_eq!(estimate.status, CostStatus::Partial);
        assert_eq!(estimate.known_subtotal_usd, "1");
        record.service_tier = Some("priority".to_owned());
        let estimate = book.estimate(&record);
        assert_eq!(estimate.status, CostStatus::Partial);
        assert_eq!(estimate.known_subtotal, Money::ZERO);
        assert!(
            estimate
                .reasons
                .iter()
                .any(|reason| reason.contains("service tier"))
        );
    }

    #[test]
    fn request_context_tiers_use_gross_input_not_aggregate_or_uncached_input() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="custom"
model="long"
context_max_tokens=100
input="1"
output="2"
cache_read="0.1"
cache_write="1"
[[models]]
provider="custom"
model="long"
context_min_tokens=101
input="3"
output="4"
cache_read="0.3"
cache_write="3"
"#,
        ));
        let mut request = record(Some("custom"), "long", InputBasis::Inclusive);
        request.tokens.input_tokens = Some(80);
        let short = book.estimate(&request);
        assert_eq!(short.total_usd.as_deref(), Some("0.00008"));
        let sum = short
            .total
            .unwrap()
            .checked_add(short.total.unwrap())
            .unwrap();
        assert_eq!(sum.to_usd_string(), "0.00016");
        request.tokens.input_tokens = Some(160);
        let aggregate_as_request = book.estimate(&request);
        assert_eq!(aggregate_as_request.total_usd.as_deref(), Some("0.00048"));
        request.granularity = Granularity::CumulativeDelta;
        let cumulative = book.estimate(&request);
        assert_eq!(cumulative.status, CostStatus::Partial);
        assert_eq!(cumulative.total, None);
        assert_eq!(cumulative.known_subtotal, Money::ZERO);
        request.granularity = Granularity::Request;
        request.tokens.input_basis = InputBasis::Separate;
        request.tokens.input_tokens = Some(50);
        request.tokens.cache_read_tokens = Some(60);
        let separate = book.estimate(&request);
        assert_eq!(separate.total_usd.as_deref(), Some("0.000168"));
        request.tokens.cache_read_tokens = None;
        // 50 known gross tokens select the low band: a lower bound, kept partial.
        let lower = book.estimate(&request);
        assert_eq!(lower.total_usd, None);
        assert_eq!(lower.status, CostStatus::Partial);
        assert_eq!(lower.known_subtotal.to_usd_string(), "0.00005");
        // No component is exact either: a higher band could still apply.
        assert_eq!(lower.cost_components.uncached_input.total_usd, None);
        assert_eq!(lower.cost_components.output.total_usd, None);
        request.tokens.input_tokens = None;
        assert_eq!(book.estimate(&request).known_subtotal, Money::ZERO);
        request.tokens.input_tokens = Some(50);
        request.tokens.cache_read_tokens = Some(0);
        request.tokens.input_tokens = Some(100);
        assert_eq!(book.estimate(&request).total_usd.as_deref(), Some("0.0001"));
        request.tokens.input_tokens = Some(101);
        assert_eq!(
            book.estimate(&request).total_usd.as_deref(),
            Some("0.000303")
        );
    }

    #[test]
    fn a_lower_bound_never_selects_a_band_when_a_higher_one_is_cheaper() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="custom"
model="odd"
context_max_tokens=100
input="3"
output="3"
[[models]]
provider="custom"
model="odd"
context_min_tokens=101
input="1"
output="3"
"#,
        ));
        let mut request = record(Some("custom"), "odd", InputBasis::Separate);
        request.tokens.input_tokens = Some(50);
        request.tokens.cache_read_tokens = None;
        let estimate = book.estimate(&request);
        assert_eq!(estimate.status, CostStatus::Partial);
        assert_eq!(estimate.known_subtotal, Money::ZERO);
        assert!(
            estimate
                .reasons
                .iter()
                .any(|reason| reason.contains("cheaper"))
        );
        // In the top band the lower bound is exact about the band itself.
        request.tokens.input_tokens = Some(150);
        let top = book.estimate(&request);
        assert_eq!(top.known_subtotal.to_usd_string(), "0.00015");
        assert_eq!(
            top.cost_components
                .uncached_input
                .total_usd
                .unwrap()
                .to_usd_string(),
            "0.00015"
        );
    }

    #[test]
    fn live_litellm_prices_fill_gaps_without_replacing_curated_rates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("litellm-prices.json"),
            r#"{
                "claude-sonnet-4-6": {"litellm_provider": "anthropic",
                    "input_cost_per_token": 0.001, "output_cost_per_token": 0.001},
                "claude-opus-9": {"litellm_provider": "anthropic",
                    "input_cost_per_token": 5e-06, "output_cost_per_token": 2.5e-05,
                    "cache_read_input_token_cost": 5e-07,
                    "cache_creation_input_token_cost": 6.25e-06,
                    "cache_creation_input_token_cost_above_1hr": 1e-05}
            }"#,
        )
        .unwrap();
        let curated = PriceBook::load(dir.path(), None).unwrap();
        let mut native = record(None, "claude-opus-9", InputBasis::Separate);
        native.source = Source::Claude;
        native.billing = Billing::ApiEquivalent;
        native.tokens.input_tokens = Some(1_000_000);
        native.tokens.output_tokens = Some(1_000_000);
        assert!(!curated.estimate(&native).applicable);
        assert!(curated.has_gaps(std::iter::once(&native)));

        // Offline: the cached map is read, nothing is fetched.
        let book = PriceBook::load(dir.path(), None)
            .unwrap()
            .with_live(dir.path(), false);
        assert!(book.snapshot_id().contains("+litellm-live-"));
        let estimate = book.estimate(&native);
        assert_eq!(estimate.total_usd.as_deref(), Some("30"));
        assert_eq!(estimate.price_source.unwrap().kind, "litellm");
        assert!(!book.has_gaps(std::iter::once(&native)));

        let mut sonnet = record(Some("anthropic"), "claude-sonnet-4-6", InputBasis::Separate);
        sonnet.tokens.input_tokens = Some(1_000_000);
        let curated_rate = curated.estimate(&sonnet);
        let with_live = book.estimate(&sonnet);
        assert_eq!(with_live.total_usd, curated_rate.total_usd);
        assert_eq!(with_live.total_usd.as_deref(), Some("3"));
        assert_eq!(with_live.price_source.unwrap().kind, "bundled");

        // No cache, no network: the book is unchanged.
        let empty = tempfile::tempdir().unwrap();
        let unchanged = PriceBook::load(empty.path(), None)
            .unwrap()
            .with_live(empty.path(), false);
        assert_eq!(unchanged.snapshot_id(), curated.snapshot_id());
    }

    #[test]
    fn absent_standard_tier_is_explicit_and_actual_tiers_are_not_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let book = PriceBook::load(dir.path(), None).unwrap();
        let mut record = record(Some("openai"), "gpt-5", InputBasis::Inclusive);
        record.tokens.input_tokens = Some(1_000_000);
        let standard = book.estimate(&record);
        assert_eq!(standard.total_usd.as_deref(), Some("1.25"));
        assert!(
            standard
                .assumptions
                .iter()
                .any(|assumption| assumption.contains("standard tier assumed"))
        );
        record.service_tier = Some("default".to_owned());
        assert_eq!(book.estimate(&record).total_usd.as_deref(), Some("1.25"));
        record.service_tier = Some("priority".to_owned());
        assert_eq!(book.estimate(&record).total_usd.as_deref(), Some("2.5"));
        record.service_tier = Some("flex".to_owned());
        assert_eq!(book.estimate(&record).total_usd.as_deref(), Some("0.625"));
        record.service_tier = Some("auto".to_owned());
        let auto = book.estimate(&record);
        assert_eq!(auto.status, CostStatus::Partial);
        assert_eq!(auto.total_usd, None);
    }

    #[test]
    fn inclusive_cache_read_and_write_are_replacement_subsets() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="custom"
model="inclusive"
input="2"
output="8"
cache_read="0.5"
cache_write="3"
"#,
        ));
        let mut record = record(Some("custom"), "inclusive", InputBasis::Inclusive);
        record.tokens.input_tokens = Some(100);
        record.tokens.cache_read_tokens = Some(60);
        record.tokens.cache_write_tokens = Some(10);
        record.tokens.output_tokens = Some(5);
        record.tokens.reasoning_tokens = Some(4);
        let estimate = book.estimate(&record);
        assert_eq!(estimate.total_usd.as_deref(), Some("0.00016"));
        // 30 uncached*2 + 60 read*.5 + 10 write*3 + 5 output*8.
        record.tokens.cache_write_tokens = None;
        let unknown = book.estimate(&record);
        assert_eq!(unknown.status, CostStatus::Partial);
        assert_eq!(unknown.known_subtotal_usd, "0.00007");
        assert_eq!(unknown.total, None);
        record.tokens.cache_write_tokens = Some(50);
        assert_eq!(book.estimate(&record).total, None);
    }

    #[test]
    fn anthropic_ttl_buckets_replace_aggregate_and_unknown_ttl_is_partial() {
        let dir = tempfile::tempdir().unwrap();
        let book = PriceBook::load(dir.path(), None).unwrap();
        let mut record = record(Some("anthropic"), "claude-sonnet-4-6", InputBasis::Separate);
        record.tokens.input_tokens = Some(100);
        record.tokens.output_tokens = Some(10);
        record.tokens.cache_read_tokens = Some(200);
        record.tokens.cache_write_tokens = Some(30);
        record.tokens.cache_write_5m_tokens = Some(10);
        record.tokens.cache_write_1h_tokens = Some(20);
        let estimate = book.estimate(&record);
        assert_eq!(estimate.total_usd.as_deref(), Some("0.0006675"));
        // 100*3 + 10*15 + 200*.3 + 10*3.75 + 20*6, in millionths.
        record.tokens.cache_write_5m_tokens = None;
        record.tokens.cache_write_1h_tokens = None;
        let unknown = book.estimate(&record);
        assert_eq!(unknown.status, CostStatus::Partial);
        assert_eq!(unknown.known_subtotal_usd, "0.00051");
        assert!(unknown.reasons.iter().any(|reason| reason.contains("TTL")));
        record.tokens.cache_write_1h_tokens = Some(20);
        assert_eq!(
            book.estimate(&record).total_usd.as_deref(),
            Some("0.0006675")
        );
        record.tokens.cache_write_tokens = None;
        record.tokens.cache_write_5m_tokens = Some(10);
        assert_eq!(
            book.estimate(&record).total_usd.as_deref(),
            Some("0.0006675")
        );
        record.tokens.cache_write_tokens = Some(0);
        record.tokens.cache_write_5m_tokens = None;
        record.tokens.cache_write_1h_tokens = None;
        assert_eq!(book.estimate(&record).total_usd.as_deref(), Some("0.00051"));
    }

    #[test]
    fn missing_positive_rates_remain_partial_but_explicit_zero_counts_need_no_rate() {
        let (_dir, book) = overridden(&sidecar(
            r#"
[[models]]
provider="custom"
model="missing"
input="1"
output="2"
"#,
        ));
        let mut record = record(Some("custom"), "missing", InputBasis::Separate);
        record.tokens.input_tokens = Some(10);
        assert_eq!(book.estimate(&record).total_usd.as_deref(), Some("0.00001"));
        record.tokens.cache_read_tokens = Some(1);
        let missing_rate = book.estimate(&record);
        assert_eq!(missing_rate.status, CostStatus::Partial);
        assert_eq!(missing_rate.known_subtotal_usd, "0.00001");
        record.tokens.cache_read_tokens = None;
        assert_eq!(book.estimate(&record).total, None);
        record.tokens = TokenCounts::default();
        assert_eq!(book.estimate(&record).status, CostStatus::Unknown);
    }

    #[test]
    fn native_references_do_not_reassign_actual_attribution_and_custom_is_not_free() {
        let dir = tempfile::tempdir().unwrap();
        let book = PriceBook::load(dir.path(), None).unwrap();
        let mut native = record(None, "gpt-4.1-2025-04-14", InputBasis::Inclusive);
        native.source = Source::Claude;
        native.billing = Billing::ApiEquivalent;
        let estimate = book.estimate(&native);
        assert_eq!(estimate.reference_provider.as_deref(), Some("openai"));
        assert_eq!(estimate.reference_model.as_deref(), Some("gpt-4.1"));
        assert_eq!(native.provider, None);
        assert!(
            estimate
                .assumptions
                .iter()
                .any(|assumption| assumption
                    .contains("actual provider attribution is still unknown"))
        );
        native.provider = Some("custom".to_owned());
        let custom = book.estimate(&native);
        assert!(!custom.applicable);
        assert_eq!(custom.total, None);
        native.endpoint = Some("https://api.openai.com/v1".to_owned());
        assert!(book.estimate(&native).applicable);
        assert_eq!(native.provider.as_deref(), Some("custom"));
        native.endpoint = Some("https://api.openai.com.evil.invalid/v1".to_owned());
        assert!(!book.estimate(&native).applicable);
        native.provider = Some("openai".to_owned());
        assert!(!book.estimate(&native).applicable);
        native.endpoint = None;
        native.model = Some("gpt-6-astra".to_owned());
        assert!(!book.estimate(&native).applicable);
        native.model = Some("gpt-4.1-2025-04-14-extra".to_owned());
        assert!(!book.estimate(&native).applicable);
    }

    #[test]
    fn codex_reference_and_verified_write_schema_do_not_erase_unknown_reads() {
        let dir = tempfile::tempdir().unwrap();
        let book = PriceBook::load(dir.path(), None).unwrap();
        let mut native = record(None, "gpt-5.2-codex", InputBasis::Inclusive);
        native.source = Source::Codex;
        native.agent = Agent::Codex;
        native.billing = Billing::ApiEquivalent;
        native.tokens.input_tokens = Some(100);
        native.tokens.cache_write_tokens = None;
        let complete = book.estimate(&native);
        assert_eq!(complete.reference_provider.as_deref(), Some("openai"));
        assert_eq!(complete.total_usd.as_deref(), Some("0.000175"));
        assert!(
            complete
                .assumptions
                .iter()
                .any(|assumption| assumption.contains("absent write counter treated as zero"))
        );
        native.tokens.cache_read_tokens = None;
        assert_eq!(book.estimate(&native).total_usd, None);
        native.source = Source::Alc;
        native.provider = Some("openai".to_owned());
        native.billing = Billing::Unknown;
        native.tokens.cache_read_tokens = Some(0);
        assert_eq!(book.estimate(&native).total_usd, None);
        native.billing = Billing::Api;
        assert_eq!(
            book.estimate(&native).total_usd.as_deref(),
            Some("0.000175")
        );
        native.provider = Some("custom".to_owned());
        assert!(!book.estimate(&native).applicable);
        native.provider = Some("codex".to_owned());
        native.billing = Billing::ApiEquivalent;
        assert_eq!(
            book.estimate(&native).total_usd.as_deref(),
            Some("0.000175")
        );
        assert_eq!(native.provider.as_deref(), Some("codex"));
    }

    #[test]
    fn checkpoints_are_not_billed_and_flat_deltas_can_be_priced() {
        let dir = tempfile::tempdir().unwrap();
        let book = PriceBook::load(dir.path(), None).unwrap();
        let mut record = record(Some("openai"), "gpt-4.1", InputBasis::Inclusive);
        record.tokens.input_tokens = Some(10);
        record.granularity = Granularity::CumulativeDelta;
        assert_eq!(book.estimate(&record).total_usd.as_deref(), Some("0.00002"));
        record.granularity = Granularity::Checkpoint;
        let checkpoint = book.estimate(&record);
        assert!(checkpoint.applicable);
        assert_eq!(checkpoint.status, CostStatus::Partial);
        assert_eq!(checkpoint.total, None);
        assert_eq!(checkpoint.known_subtotal, Money::ZERO);
    }

    #[test]
    fn explicit_override_path_snapshot_identity_and_strict_header() {
        let dir = tempfile::tempdir().unwrap();
        let default = PriceBook::load(dir.path(), None).unwrap();
        let path = dir.path().join("chosen.toml");
        assert!(PriceBook::load(dir.path(), Some(&path)).is_err());
        std::fs::write(
            &path,
            sidecar(
                r#"
[[models]]
provider="custom"
model="x"
input="1"
"#,
            ),
        )
        .unwrap();
        let chosen = PriceBook::load(dir.path(), Some(&path)).unwrap();
        assert_ne!(default.snapshot_id(), chosen.snapshot_id());
        assert_eq!(
            chosen.snapshot_id(),
            PriceBook::load(dir.path(), Some(&path))
                .unwrap()
                .snapshot_id()
        );
        for text in [
            "version=2\ncurrency=\"USD\"\nunits=\"USD-per-million-tokens\"\n",
            "version=1\ncurrency=\"EUR\"\nunits=\"USD-per-million-tokens\"\n",
            "version=1\ncurrency=\"USD\"\nunits=\"USD-per-token\"\n",
            "version=1\ncurrency=\"USD\"\nunits=\"USD-per-million-tokens\"\nextra=true\n",
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(PriceBook::load(dir.path(), Some(&path)).is_err());
        }
    }

    #[test]
    fn bundled_metadata_pin_and_license_are_present() {
        let metadata: SourceFile = toml::from_str(SOURCE).unwrap();
        assert_eq!(
            metadata.upstream_commit,
            "33d908e0ae2c0a257eeb5d546df08527d348a670"
        );
        assert_eq!(
            metadata.upstream_sha256,
            "4bda1a19da601af95df65ba20c40bcb1e914ee00b890ea95f8d14de9886fac46"
        );
        assert_eq!(metadata.license, "MIT");
        assert!(
            metadata
                .upstream_raw_url
                .contains(&metadata.upstream_commit)
        );
        let license = include_str!("prices/LICENSE");
        assert!(license.contains("Copyright (c) 2023 Berri AI"));
        assert_eq!(
            license,
            include_str!("../../THIRD_PARTY_LICENSES/LiteLLM-LICENSE")
        );
    }
}
