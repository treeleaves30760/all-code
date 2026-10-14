//! The models the Antigravity CLI can run.
//!
//! agy answers `agy models` with the list its own Google login can reach, so
//! that answer is the one alc shows - cached for a day, because asking means
//! a network round trip through agy. Everything that can go wrong on the way
//! (no agy, a signed-out agy, a timeout, an answer alc cannot read) falls back
//! to the list alc ships, so the picker is never empty. A failed ask is
//! recorded like an answer, so it costs at most one attempt a day.

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
    /// Unix seconds of the last time agy was asked, answered or not; 0 when
    /// there was no agy to ask.
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
    let cached = read_cache(config_dir);
    if !refresh
        && let Some(cache) = &cached
        && now >= cache.refreshed_at
        && now - cache.refreshed_at < TTL_SECS
    {
        return AgyCatalog {
            models: cache.models.clone(),
            source: format!("cache ({})", cache.source),
            refreshed_at: cache.refreshed_at,
        };
    }
    // No agy means nothing was spawned and nothing was waited for, so there
    // is no attempt worth recording: installing agy takes effect at once.
    let Some(binary) = binary else {
        return AgyCatalog {
            models: bundled(),
            source: "bundled (agy not found)".to_owned(),
            refreshed_at: 0,
        };
    };
    let reason = match run(&binary) {
        Ok(text) => {
            let models = in_bundled_order(parse_models_output(&text));
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
    };
    // The attempt is recorded against the clock, as the Codex catalog does:
    // otherwise a signed-out or offline agy is spawned - and, offline, waited
    // on for up to RUN_TIMEOUT - by every later `alc config` and `alc models
    // --agy`, forever, on exactly the machines where it cannot work. What an
    // earlier answer listed is kept rather than traded for the shipped list.
    let catalog = match cached {
        Some(cache) if !cache.source.starts_with("bundled") => AgyCatalog {
            models: cache.models,
            source: format!("kept from an earlier agy models ({reason})"),
            refreshed_at: now,
        },
        _ => AgyCatalog {
            models: bundled(),
            source: format!("bundled ({reason})"),
            refreshed_at: now,
        },
    };
    let _ = write_cache(config_dir, &catalog);
    catalog
}

/// agy's answer in the order alc ships, so browsing does not reshuffle the
/// day agy is reachable; models agy lists that alc does not ship follow, in
/// agy's order. Shipped models agy did not list stay out: its answer is what
/// the login can reach.
fn in_bundled_order(mut live: Vec<AgyModel>) -> Vec<AgyModel> {
    let mut ordered = Vec::with_capacity(live.len());
    for shipped in bundled() {
        if let Some(index) = live.iter().position(|model| model.id == shipped.id) {
            ordered.push(live.remove(index));
        }
    }
    ordered.extend(live);
    ordered
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
        assert_eq!(cached.models[0].id, "claude-opus-5-5-low");
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
    fn a_missing_agy_keeps_the_bundled_list_and_writes_no_cache() {
        let temp = tempfile::tempdir().unwrap();
        let missing = models_from(
            temp.path(),
            true,
            None,
            |_| panic!("there is no agy to run"),
            1_000,
        );
        assert_eq!(missing.models, bundled());
        assert!(missing.source.contains("bundled"));
        assert!(missing.source.contains("agy not found"));
        assert!(!temp.path().join(CACHE_FILE).exists());
    }

    /// The Antigravity twin of the Codex catalog's
    /// `a_sync_that_fails_is_not_retried_by_every_later_command`: without the
    /// attempt on record, every `alc config` and `alc models --agy` on a
    /// machine with a signed-out or offline agy pays for the same failure.
    #[test]
    fn an_agy_that_fails_is_not_asked_again_until_tomorrow_or_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let run = |_: &Path| {
            calls.set(calls.get() + 1);
            anyhow::bail!("agy models exited with status 1")
        };
        let agy = || Some(PathBuf::from("agy"));
        let failed = models_from(temp.path(), false, agy(), run, 1_000);
        assert_eq!(calls.get(), 1);
        assert_eq!(failed.models, bundled());
        assert!(failed.source.contains("bundled"), "{}", failed.source);
        assert!(
            failed.source.contains("agy models failed"),
            "{}",
            failed.source
        );
        assert!(
            temp.path().join(CACHE_FILE).is_file(),
            "the attempt is recorded"
        );

        let later = models_from(temp.path(), false, agy(), run, 1_000 + TTL_SECS - 1);
        assert_eq!(
            calls.get(),
            1,
            "a recorded failure is not retried within a day"
        );
        assert_eq!(later.models, bundled());
        assert!(
            later.source.contains("agy models failed"),
            "{}",
            later.source
        );

        models_from(temp.path(), true, agy(), run, 1_000 + 10);
        assert_eq!(calls.get(), 2, "--refresh still asks");
        models_from(temp.path(), false, agy(), run, 1_000 + 10 + TTL_SECS);
        assert_eq!(calls.get(), 3, "and so does the next day");
    }

    #[test]
    fn an_answer_with_no_models_is_recorded_like_a_failure() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let run = |_: &Path| {
            calls.set(calls.get() + 1);
            Ok("error: not logged in".to_owned())
        };
        let agy = || Some(PathBuf::from("agy"));
        let first = models_from(temp.path(), false, agy(), run, 1_000);
        assert_eq!(first.models, bundled());
        assert!(
            first.source.contains("listed no models"),
            "{}",
            first.source
        );
        models_from(temp.path(), false, agy(), run, 1_001);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_failure_keeps_the_models_an_earlier_answer_listed() {
        let temp = tempfile::tempdir().unwrap();
        let agy = || Some(PathBuf::from("agy"));
        let good = models_from(
            temp.path(),
            false,
            agy(),
            |_| Ok(REAL_SHAPE.to_owned()),
            1_000,
        );
        let stale = 1_000 + TTL_SECS + 1;
        let kept = models_from(
            temp.path(),
            false,
            agy(),
            |_| anyhow::bail!("timed out"),
            stale,
        );
        assert_eq!(kept.models, good.models);
        assert!(!kept.source.contains("bundled"), "{}", kept.source);
        assert!(kept.source.contains("agy models failed"), "{}", kept.source);
        assert_eq!(kept.refreshed_at, stale);

        let again = models_from(temp.path(), false, agy(), |_| unreachable!(), stale + 1);
        assert_eq!(again.models, good.models);
    }

    #[test]
    fn a_cache_from_the_future_is_not_trusted() {
        let temp = tempfile::tempdir().unwrap();
        let agy = || Some(PathBuf::from("agy"));
        let now = 1_000_000;
        models_from(
            temp.path(),
            false,
            agy(),
            |_| Ok(REAL_SHAPE.to_owned()),
            now + 10,
        );
        let calls = Cell::new(0);
        models_from(
            temp.path(),
            false,
            agy(),
            |_| {
                calls.set(calls.get() + 1);
                Ok(REAL_SHAPE.to_owned())
            },
            now,
        );
        assert_eq!(calls.get(), 1, "a cache dated after now is asked again");
    }

    #[test]
    fn a_live_answer_is_shown_in_the_bundled_order_with_additions_last() {
        let temp = tempfile::tempdir().unwrap();
        // agy 1.3.4's own order, plus two models alc does not ship, minus one
        // it does.
        let live = "gemini-3.8-flash-high\tGemini 3.8 Flash (High)\n\
gemini-9-pro\tGemini 9 Pro\n\
gemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)\n\
claude-opus-5-5-low\tClaude Opus 5.5 (Low)\n\
claude-opus-5-5-medium\tClaude Opus 5.5 (Medium)\n\
claude-opus-5-5-high\tClaude Opus 5.5 (High)\n\
claude-sonnet-5-5-low\tClaude Sonnet 5.5 (Low)\n\
claude-sonnet-5-5-medium\tClaude Sonnet 5.5 (Medium)\n\
claude-sonnet-5-5-high\tClaude Sonnet 5.5 (High)\n\
gpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n\
aardvark-1\tAardvark 1\n";
        let catalog = models_from(
            temp.path(),
            true,
            Some(PathBuf::from("agy")),
            |_| Ok(live.to_owned()),
            1_000,
        );
        let ids: Vec<_> = catalog
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
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
                "gpt-oss-120b-medium",
                "gemini-9-pro",
                "aardvark-1",
            ]
        );
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
