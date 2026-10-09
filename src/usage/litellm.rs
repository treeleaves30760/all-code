//! Runtime token prices from LiteLLM's public model map, the same file
//! `npx ccusage` reads, so a model newer than the curated snapshot still gets
//! an estimate without anyone transcribing its rates by hand.
//!
//! The map is fetched at most once a day into the alc configuration directory
//! and read from there; `--offline` uses only that cached copy. Only first-party
//! `anthropic` and `openai` rows are taken, because those are the only
//! reference providers the price book resolves usage to.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

pub(crate) const URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const CACHE_FILE: &str = "litellm-prices.json";
/// Touched after a failed refresh, so an offline or blocked machine waits a
/// day before trying again instead of paying the timeout on every report.
const FAILED_FILE: &str = ".litellm-prices.failed";
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// The map is about 3 MiB today; the bound only stops a runaway response.
const LIMIT: u64 = 32 * 1024 * 1024;
const PROVIDERS: [&str; 2] = ["anthropic", "openai"];
/// LiteLLM's suffix for each OpenAI service tier alc can price.
const TIERS: [(&str, &str); 3] = [
    ("standard", ""),
    ("priority", "_priority"),
    ("flex", "_flex"),
];

/// The cached map's text and when it was fetched.
pub(crate) struct Snapshot {
    pub text: String,
    pub fetched: SystemTime,
}

/// The cached map, refreshed first when it is missing or a day old and the
/// network is allowed. A failed refresh falls back to the stale copy.
pub(crate) fn load(config_dir: &Path, network: bool) -> Option<Snapshot> {
    let path = config_dir.join(CACHE_FILE);
    let cached = || -> Option<Snapshot> {
        let fetched = fs::symlink_metadata(&path)
            .ok()
            .filter(fs::Metadata::is_file)?
            .modified()
            .ok()?;
        let mut text = String::new();
        fs::File::open(&path)
            .ok()?
            .take(LIMIT)
            .read_to_string(&mut text)
            .ok()?;
        Some(Snapshot { text, fetched })
    };
    let fresh = cached().filter(|snapshot| {
        SystemTime::now()
            .duration_since(snapshot.fetched)
            .is_ok_and(|age| age < MAX_AGE)
    });
    let failed = config_dir.join(FAILED_FILE);
    let recently_failed = fs::metadata(&failed)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|at| SystemTime::now().duration_since(at).ok())
        .is_some_and(|age| age < MAX_AGE);
    if fresh.is_some() || !network || recently_failed {
        return fresh.or_else(cached);
    }
    let Some(url) = url() else {
        return cached();
    };
    match download(&url).and_then(|text| store(config_dir, &text).map(|()| text)) {
        Ok(text) => {
            let _ = fs::remove_file(&failed);
            Some(Snapshot {
                text,
                fetched: SystemTime::now(),
            })
        }
        Err(_) => {
            let _ = fs::write(&failed, b"");
            cached()
        }
    }
}

/// `None` turns fetching off; debug builds let tests point it elsewhere.
fn url() -> Option<String> {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("ALC_LITELLM_URL") {
        return (url != "off").then_some(url);
    }
    Some(URL.to_owned())
}

fn download(url: &str) -> Result<String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build();
    let mut response = ureq::Agent::new_with_config(config)
        .get(url)
        .header("User-Agent", concat!("alc/", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("GET {url} failed"))?;
    let text = response
        .body_mut()
        .with_config()
        .limit(LIMIT)
        .read_to_string()
        .with_context(|| format!("could not read {url}"))?;
    // Never replace a good cache with something that is not the map.
    ensure!(
        !prices(&text).is_empty(),
        "LiteLLM price map has no usable rows"
    );
    Ok(text)
}

fn store(config_dir: &Path, text: &str) -> Result<()> {
    fs::create_dir_all(config_dir)?;
    let mut staged = tempfile::Builder::new()
        .prefix(".litellm-prices-")
        .tempfile_in(config_dir)?;
    std::io::Write::write_all(&mut staged, text.as_bytes())?;
    staged.persist(config_dir.join(CACHE_FILE))?;
    Ok(())
}

/// One priced scope, with rates in decimal USD per million tokens, the units
/// of `pricing.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Price {
    pub provider: String,
    pub model: String,
    pub tier: String,
    pub context_min: Option<u64>,
    pub context_max: Option<u64>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub cache_read: Option<String>,
    pub cache_write: Option<String>,
    pub cache_write_5m: Option<String>,
    pub cache_write_1h: Option<String>,
}

/// Every usable first-party row. Keys with a `/` are provider- or
/// size-qualified variants (image sizes, regional routes), not model IDs.
pub(crate) fn prices(text: &str) -> Vec<Price> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (model, row) in &map {
        let Some(row) = row.as_object() else { continue };
        let Some(provider) = row
            .get("litellm_provider")
            .and_then(Value::as_str)
            .filter(|provider| PROVIDERS.contains(provider))
        else {
            continue;
        };
        if model.contains('/')
            || model.trim() != model
            || model.is_empty()
            || model == "sample_spec"
        {
            continue;
        }
        let rate = |field: &str| row.get(field).and_then(Value::as_f64).and_then(per_million);
        // A row that charges long requests differently says so on its
        // standard tier; another tier without its own band is then only known
        // up to that threshold, never flat across it.
        let row_threshold = threshold(row.keys(), "");
        // Anthropic prices the 5-minute and 1-hour cache separately.
        let ttl_split = |suffix: &str| {
            row.contains_key(&format!(
                "cache_creation_input_token_cost_above_1hr{suffix}"
            ))
        };
        for (tier, suffix) in TIERS {
            let scope = |above: Option<u64>| {
                let field = |name: &str| match above {
                    Some(threshold) => rate(&format!("{name}_above_{threshold}k_tokens{suffix}")),
                    None => rate(&format!("{name}{suffix}")),
                };
                let mut price = Price {
                    provider: provider.to_owned(),
                    model: model.clone(),
                    tier: tier.to_owned(),
                    input: field("input_cost_per_token"),
                    output: field("output_cost_per_token"),
                    cache_read: field("cache_read_input_token_cost"),
                    ..Price::default()
                };
                let write = field("cache_creation_input_token_cost");
                if ttl_split(suffix) {
                    // A missing 1h rate stays unknown rather than borrowing
                    // the 5m one, so 1h writes price as partial.
                    price.cache_write_5m = write;
                    price.cache_write_1h = field("cache_creation_input_token_cost_above_1hr");
                } else {
                    price.cache_write = write;
                }
                price
            };
            let base = scope(None);
            if base.input.is_none() && base.output.is_none() {
                continue;
            }
            match threshold(row.keys(), suffix).or(row_threshold) {
                Some(threshold) => {
                    let limit = threshold * 1000;
                    out.push(Price {
                        context_max: Some(limit),
                        ..base
                    });
                    let above = scope(Some(threshold));
                    if above.input.is_some() || above.output.is_some() {
                        out.push(Price {
                            context_min: Some(limit + 1),
                            ..above
                        });
                    }
                }
                None => out.push(base),
            }
        }
    }
    out
}

/// The `N` of `input_cost_per_token_above_{N}k_tokens{suffix}`, when the row
/// charges long requests differently.
fn threshold<'a>(keys: impl Iterator<Item = &'a String>, suffix: &str) -> Option<u64> {
    let mut found = BTreeMap::new();
    for key in keys {
        let Some(rest) = key.strip_prefix("input_cost_per_token_above_") else {
            continue;
        };
        let Some(number) = rest
            .strip_suffix(suffix)
            .and_then(|rest| rest.strip_suffix("k_tokens"))
        else {
            continue;
        };
        if let Ok(value) = number.parse::<u64>() {
            found.insert(value, ());
        }
    }
    // More than one band is not something the price book can express.
    (found.len() == 1).then(|| *found.keys().next().unwrap_or(&0))
}

/// USD per token to a decimal USD-per-million string with at most six
/// fractional digits, the precision `pricing.toml` accepts.
fn per_million(per_token: f64) -> Option<String> {
    let value = per_token * 1_000_000.0;
    if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
        return None;
    }
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    Some(if text.is_empty() {
        "0".to_owned()
    } else {
        text.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = r#"{
        "sample_spec": {"litellm_provider": "openai", "input_cost_per_token": 0},
        "claude-opus-9": {
            "litellm_provider": "anthropic",
            "input_cost_per_token": 5e-06, "output_cost_per_token": 2.5e-05,
            "cache_read_input_token_cost": 5e-07,
            "cache_creation_input_token_cost": 6.25e-06,
            "cache_creation_input_token_cost_above_1hr": 1e-05,
            "input_cost_per_token_batches": 2.5e-06
        },
        "gpt-9": {
            "litellm_provider": "openai",
            "input_cost_per_token": 2e-06, "output_cost_per_token": 1e-05,
            "cache_read_input_token_cost": 1e-07,
            "input_cost_per_token_above_272k_tokens": 4e-06,
            "output_cost_per_token_above_272k_tokens": 1.5e-05,
            "input_cost_per_token_priority": 4e-06, "output_cost_per_token_priority": 2e-05
        },
        "claude-long-9": {
            "litellm_provider": "anthropic",
            "input_cost_per_token": 1e-06, "output_cost_per_token": 5e-06,
            "cache_creation_input_token_cost": 1.25e-06,
            "cache_creation_input_token_cost_above_1hr": 2e-06,
            "input_cost_per_token_above_100k_tokens": 2e-06,
            "output_cost_per_token_above_100k_tokens": 1e-05,
            "cache_creation_input_token_cost_above_100k_tokens": 2.5e-06,
            "cache_creation_input_token_cost_above_1hr_above_100k_tokens": 4e-06
        },
        "claude-partial-9": {
            "litellm_provider": "anthropic",
            "input_cost_per_token": 1e-06, "output_cost_per_token": 5e-06,
            "cache_creation_input_token_cost": 1.25e-06,
            "cache_creation_input_token_cost_above_1hr": 2e-06,
            "input_cost_per_token_above_100k_tokens": 2e-06,
            "cache_creation_input_token_cost_above_100k_tokens": 2.5e-06
        },
        "openrouter/gpt-9": {"litellm_provider": "openrouter", "input_cost_per_token": 1},
        "low/1024-x-1024/gpt-image": {"litellm_provider": "openai", "input_cost_per_token": 1},
        "gemini-9": {"litellm_provider": "gemini", "input_cost_per_token": 1e-06}
    }"#;

    #[test]
    fn rows_become_per_million_rates_with_ttl_writes_tiers_and_bands() {
        let prices = prices(MAP);
        let opus: Vec<_> = prices
            .iter()
            .filter(|p| p.model == "claude-opus-9")
            .collect();
        assert_eq!(opus.len(), 1);
        assert_eq!(opus[0].input.as_deref(), Some("5"));
        assert_eq!(opus[0].output.as_deref(), Some("25"));
        assert_eq!(opus[0].cache_read.as_deref(), Some("0.5"));
        assert_eq!(opus[0].cache_write_5m.as_deref(), Some("6.25"));
        assert_eq!(opus[0].cache_write_1h.as_deref(), Some("10"));
        assert_eq!(opus[0].cache_write, None);

        let gpt: Vec<_> = prices.iter().filter(|p| p.model == "gpt-9").collect();
        let standard: Vec<_> = gpt.iter().filter(|p| p.tier == "standard").collect();
        assert_eq!(standard.len(), 2);
        assert_eq!(standard[0].context_max, Some(272_000));
        assert_eq!(standard[0].input.as_deref(), Some("2"));
        assert_eq!(standard[1].context_min, Some(272_001));
        assert_eq!(standard[1].input.as_deref(), Some("4"));
        assert_eq!(standard[1].cache_read, None);
        let priority: Vec<_> = gpt.iter().filter(|p| p.tier == "priority").collect();
        assert_eq!(priority.len(), 1);
        assert_eq!(priority[0].output.as_deref(), Some("20"));
        // The row is banded, so a tier without its own band stops at it.
        assert_eq!(priority[0].context_max, Some(272_000));
        assert!(gpt.iter().all(|p| p.tier != "flex"));

        let long: Vec<_> = prices
            .iter()
            .filter(|p| p.model == "claude-long-9")
            .collect();
        assert_eq!(long.len(), 2);
        assert_eq!(long[1].context_min, Some(100_001));
        assert_eq!(long[1].cache_write_5m.as_deref(), Some("2.5"));
        assert_eq!(long[1].cache_write_1h.as_deref(), Some("4"));
        assert_eq!(long[1].cache_write, None);
        let partial: Vec<_> = prices
            .iter()
            .filter(|p| p.model == "claude-partial-9")
            .collect();
        assert_eq!(partial[1].cache_write_5m.as_deref(), Some("2.5"));
        assert_eq!(partial[1].cache_write_1h, None);
        assert_eq!(partial[1].cache_write, None);

        assert!(prices.iter().all(|p| !p.model.contains('/')));
        assert!(prices.iter().all(|p| p.model != "gemini-9"));
    }

    #[test]
    fn rates_keep_six_fractional_digits_and_reject_nonsense() {
        assert_eq!(per_million(1.25e-07).as_deref(), Some("0.125"));
        assert_eq!(per_million(0.0).as_deref(), Some("0"));
        assert_eq!(per_million(-1.0), None);
        assert_eq!(per_million(f64::NAN), None);
        assert!(prices("not json").is_empty());
    }

    #[test]
    fn a_failed_refresh_backs_off_for_a_day() {
        let dir = tempfile::tempdir().unwrap();
        // What load() records after a failed download.
        fs::write(dir.path().join(FAILED_FILE), b"").unwrap();
        fs::write(dir.path().join(CACHE_FILE), MAP).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(dir.path().join(CACHE_FILE))
            .unwrap()
            .set_modified(old)
            .unwrap();
        // Network allowed, cache stale, but a recent failure: no new fetch.
        let snapshot = load(dir.path(), true).unwrap();
        assert_eq!(snapshot.fetched, old);
    }

    #[test]
    fn offline_reads_a_stale_cache_and_never_fetches() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path(), false).is_none());
        fs::write(dir.path().join(CACHE_FILE), MAP).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(dir.path().join(CACHE_FILE))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let snapshot = load(dir.path(), false).unwrap();
        assert_eq!(snapshot.text, MAP);
        assert_eq!(snapshot.fetched, old);
    }
}
