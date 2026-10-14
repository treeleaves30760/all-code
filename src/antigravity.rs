//! The models the Antigravity CLI can run.
//!
//! agy answers `agy models` with the list its own Google login can reach, so
//! that answer is the one alc shows - cached for a day, because asking means
//! a network round trip through agy. Everything that can go wrong on the way
//! (no agy, a signed-out agy, a timeout, an answer alc cannot read) falls back
//! to the list alc ships, so the picker is never empty.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::Agent;

const CACHE_FILE: &str = "antigravity-models.json";
const SCHEMA_VERSION: u32 = 1;
/// How long an answer from agy is trusted before asking again.
const TTL_SECS: u64 = 24 * 60 * 60;
/// `agy models` refreshes its login before answering, which is a network
/// call; past this, alc stops waiting and shows the bundled list.
const RUN_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgyModel {
    pub id: String,
    pub label: String,
}

/// The list to show, and where it came from: `agy models`, the cache of an
/// earlier answer, or the bundled list (with the reason agy was not used).
#[derive(Debug, Clone, Serialize)]
pub struct AgyCatalog {
    pub models: Vec<AgyModel>,
    pub source: String,
    /// Unix seconds of the agy answer behind the list; 0 for the bundled one.
    pub refreshed_at: u64,
}

#[derive(Deserialize)]
struct Bundle {
    schema_version: u32,
    models: Vec<AgyModel>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    schema_version: u32,
    source: String,
    refreshed_at: u64,
    models: Vec<AgyModel>,
}

/// The list alc ships, in the order alc offers it.
pub fn bundled() -> Vec<AgyModel> {
    let bundle: Bundle = serde_json::from_str(include_str!("../models/antigravity.json"))
        .expect("the bundled Antigravity model list must be valid");
    debug_assert_eq!(bundle.schema_version, SCHEMA_VERSION);
    bundle.models
}

/// Reads `agy models` stdout: one `<id>\t<label>` line per model. Anything
/// else - the `Fetching available models...` banner, an error, a blank line -
/// is skipped, so an answer with no model lines parses to nothing.
pub fn parse_models_output(text: &str) -> Vec<AgyModel> {
    text.lines()
        .filter_map(|line| {
            let (id, label) = line.trim().split_once('\t')?;
            let (id, label) = (id.trim(), label.trim());
            let plausible =
                !id.is_empty() && !label.is_empty() && !id.chars().any(char::is_whitespace);
            plausible.then(|| AgyModel {
                id: id.to_owned(),
                label: label.to_owned(),
            })
        })
        .collect()
}

/// The Antigravity models, from a cache younger than a day unless `refresh`,
/// otherwise from `agy models`, otherwise the bundled list.
pub fn models(config_dir: &Path, refresh: bool) -> AgyCatalog {
    models_with(config_dir, refresh, run_agy_models)
}

/// [`models`] with the `agy models` call injected; `run` gets the resolved
/// agy binary and returns its stdout.
pub fn models_with(
    config_dir: &Path,
    refresh: bool,
    run: impl Fn(&Path) -> Result<String>,
) -> AgyCatalog {
    models_from(
        config_dir,
        refresh,
        agy_binary(),
        run,
        crate::usage::ledger::now_unix(),
    )
}

fn models_from(
    config_dir: &Path,
    refresh: bool,
    binary: Option<PathBuf>,
    run: impl Fn(&Path) -> Result<String>,
    now: u64,
) -> AgyCatalog {
    if !refresh
        && let Some(cache) = read_cache(config_dir)
        && now >= cache.refreshed_at
        && now - cache.refreshed_at < TTL_SECS
    {
        return AgyCatalog {
            models: cache.models,
            source: format!("cache ({})", cache.source),
            refreshed_at: cache.refreshed_at,
        };
    }
    let reason = match binary {
        None => "agy not found".to_owned(),
        Some(binary) => match run(&binary) {
            Ok(text) => {
                let models = parse_models_output(&text);
                if models.is_empty() {
                    "agy models listed no models".to_owned()
                } else {
                    let catalog = AgyCatalog {
                        models,
                        source: "agy models".to_owned(),
                        refreshed_at: now,
                    };
                    // The cache only saves the next call a round trip, so a
                    // read-only config directory costs nothing but that.
                    let _ = write_cache(config_dir, &catalog);
                    return catalog;
                }
            }
            Err(error) => format!("agy models failed: {error:#}"),
        },
    };
    AgyCatalog {
        models: bundled(),
        source: format!("bundled ({reason})"),
        refreshed_at: 0,
    }
}

/// The agy alc would launch: `ALC_AGY_BIN`, else `agy` on PATH.
fn agy_binary() -> Option<PathBuf> {
    crate::doctor::resolve(&crate::doctor::binary_for(Agent::Agy))
}

/// Runs `agy models` with no stdin, giving up after [`RUN_TIMEOUT`]. Only
/// stdout is read; agy writes its banner and any errors to stderr.
fn run_agy_models(binary: &Path) -> Result<String> {
    let mut child = Command::new(binary)
        .arg("models")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {}", binary.display()))?;
    let mut stdout = child.stdout.take().context("agy stdout was not captured")?;
    // Read on a thread so a long answer cannot fill the pipe and stall agy
    // while this thread waits for it to exit.
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + RUN_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "agy models did not answer within {}s",
                RUN_TIMEOUT.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        bail!("agy models exited with {status}");
    }
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("reading agy models output panicked"))??;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_cache(config_dir: &Path) -> Option<CacheFile> {
    let text = fs::read_to_string(config_dir.join(CACHE_FILE)).ok()?;
    let cache: CacheFile = serde_json::from_str(&text).ok()?;
    (cache.schema_version == SCHEMA_VERSION && !cache.models.is_empty()).then_some(cache)
}

fn write_cache(config_dir: &Path, catalog: &AgyCatalog) -> Result<()> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    let cache = CacheFile {
        schema_version: SCHEMA_VERSION,
        source: catalog.source.clone(),
        refreshed_at: catalog.refreshed_at,
        models: catalog.models.clone(),
    };
    let path = config_dir.join(CACHE_FILE);
    let temporary = config_dir.join(format!(".{CACHE_FILE}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(&cache)?)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("failed to replace {}", path.display()))?;
    }
    fs::rename(&temporary, &path).with_context(|| {
        format!(
            "failed to move {} to {}",
            temporary.display(),
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const REAL_SHAPE: &str = "Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\nclaude-opus-5-5-low\tClaude Opus 5.5 (Low)\n";

    #[test]
    fn agy_models_output_is_parsed_and_the_banner_skipped() {
        let text = "Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\nclaude-opus-5-5-low\tClaude Opus 5.5 (Low)\n";
        let models = parse_models_output(text);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gemini-3.8-flash-high");
        assert_eq!(models[1].label, "Claude Opus 5.5 (Low)");
    }

    #[test]
    fn a_bad_agy_models_answer_keeps_the_bundled_list() {
        assert!(parse_models_output("Fetching available models...\n").is_empty());
        assert!(parse_models_output("error: not logged in").is_empty());
        let temp = tempfile::tempdir().unwrap();
        // ALC_AGY_BIN pointing at a program that prints nothing useful
        let catalog = models_with(temp.path(), true, |_| Ok(String::from("nope")));
        assert_eq!(catalog.models.len(), bundled().len());
        assert!(catalog.source.contains("bundled"));
    }

    #[test]
    fn the_bundled_list_is_the_ten_models_in_the_shipped_order() {
        let ids: Vec<_> = bundled().into_iter().map(|model| model.id).collect();
        assert_eq!(
            ids,
            [
                "claude-opus-5-5-high",
                "claude-opus-5-5-medium",
                "claude-opus-5-5-low",
                "claude-sonnet-5-5-high",
                "claude-sonnet-5-5-medium",
                "claude-sonnet-5-5-low",
                "gemini-3.8-flash-high",
                "gemini-3.8-flash-medium",
                "gemini-3.8-flash-low",
                "gpt-oss-120b-medium",
            ]
        );
        assert_eq!(bundled()[0].label, "Claude Opus 5.5 (High)");
        assert_eq!(bundled()[9].label, "GPT-OSS 120B (Medium)");
    }

    #[test]
    fn windows_line_endings_and_blank_lines_are_tolerated() {
        let models =
            parse_models_output("\r\nclaude-opus-5-5-high\tClaude Opus 5.5 (High)\r\n\r\n");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-opus-5-5-high");
        assert_eq!(models[0].label, "Claude Opus 5.5 (High)");
    }

    #[test]
    fn a_good_answer_is_used_and_cached_for_a_day() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let run = |_: &Path| {
            calls.set(calls.get() + 1);
            Ok(REAL_SHAPE.to_owned())
        };
        let agy = || Some(PathBuf::from("agy"));
        let fresh = models_from(temp.path(), false, agy(), run, 1_000);
        assert_eq!(fresh.models.len(), 2);
        assert_eq!(fresh.source, "agy models");
        assert_eq!(fresh.refreshed_at, 1_000);
        assert!(temp.path().join(CACHE_FILE).is_file());

        let cached = models_from(temp.path(), false, agy(), run, 1_000 + TTL_SECS - 1);
        assert_eq!(calls.get(), 1, "a fresh cache must not run agy again");
        assert!(cached.source.contains("cache"));
        assert_eq!(cached.models[0].id, "gemini-3.8-flash-high");
        assert_eq!(cached.refreshed_at, 1_000);
    }

    #[test]
    fn a_stale_cache_or_refresh_asks_agy_again() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let run = |_: &Path| {
            calls.set(calls.get() + 1);
            Ok(REAL_SHAPE.to_owned())
        };
        let agy = || Some(PathBuf::from("agy"));
        models_from(temp.path(), false, agy(), run, 1_000);
        models_from(temp.path(), false, agy(), run, 1_000 + TTL_SECS + 1);
        assert_eq!(calls.get(), 2);
        models_from(temp.path(), true, agy(), run, 1_000 + TTL_SECS + 2);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn a_failing_or_missing_agy_keeps_the_bundled_list_and_writes_no_cache() {
        let temp = tempfile::tempdir().unwrap();
        let failed = models_from(
            temp.path(),
            true,
            Some(PathBuf::from("agy")),
            |_| anyhow::bail!("agy models exited with status 1"),
            1_000,
        );
        assert_eq!(failed.models, bundled());
        assert!(failed.source.contains("bundled"));
        assert!(!temp.path().join(CACHE_FILE).exists());

        let missing = models_from(
            temp.path(),
            true,
            None,
            |_| panic!("there is no agy to run"),
            1_000,
        );
        assert_eq!(missing.models, bundled());
        assert!(missing.source.contains("bundled"));
    }

    #[test]
    fn an_unreadable_or_empty_cache_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(CACHE_FILE), "{not json").unwrap();
        let catalog = models_from(temp.path(), false, None, |_| unreachable!(), 1_000);
        assert_eq!(catalog.models, bundled());
        std::fs::write(
            temp.path().join(CACHE_FILE),
            r#"{"schema_version":1,"source":"agy models","refreshed_at":1000,"models":[]}"#,
        )
        .unwrap();
        let catalog = models_from(temp.path(), false, None, |_| unreachable!(), 1_000);
        assert_eq!(catalog.models, bundled());
    }
}
