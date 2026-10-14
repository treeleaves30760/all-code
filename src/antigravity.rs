//! The models the Antigravity CLI can run.
//!
//! agy answers `agy models` with the list its own Google login can reach, so
//! that answer is the one alc shows - cached for a day, because asking means
//! a network round trip through agy. Everything that can go wrong on the way
//! (no agy, a signed-out agy, a timeout, an answer alc cannot read) falls back
//! to the list alc ships, so the picker is never empty. A failed ask is
//! recorded like an answer, so it costs at most one attempt a day.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::{Agent, ContextChoice, ReasoningEffort};
use crate::model_catalog::ModelInfo;

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
    #[serde(default)]
    families: Vec<AgyFamily>,
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

/// One Antigravity model family: the wire ids are `<id>-<effort>` for each
/// effort in `thinking`. Windows and caps are KohakuTerrarium's table (the
/// Code Assist endpoint publishes none); Argon needs a Google AI Ultra plan.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgyFamily {
    pub id: String,
    pub label: String,
    pub context_window: u64,
    pub max_context_window: u64,
    pub max_output_tokens: u64,
    /// Effort word → the `thinkingConfig` fields that effort sends, without
    /// `includeThoughts` (always true; the bridge adds it).
    pub thinking: BTreeMap<String, serde_json::Value>,
}

/// Where Claude Code's `opus` tier lands on this login.
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub const STRONGEST_FAMILY: &str = "claude-opus-5-5";
/// Where `haiku` and Claude Code's background work land.
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub const CHEAPEST_FAMILY: &str = "gemini-3.8-flash";

/// The families alc ships, most capable first.
pub fn families() -> Vec<AgyFamily> {
    let bundle: Bundle = serde_json::from_str(include_str!("../models/antigravity.json"))
        .expect("the bundled Antigravity model list must be valid");
    bundle.families
}

pub fn family(id: &str) -> Option<AgyFamily> {
    families().into_iter().find(|family| family.id == id)
}

/// `claude-opus-5-5-low` → (the Opus family, `low`). `None` for a family id,
/// an unknown model, or an effort the family does not serve.
pub fn split_wire_id(model: &str) -> Option<(AgyFamily, String)> {
    let (id, effort) = model.trim().rsplit_once('-')?;
    let family = family(id)?;
    family
        .thinking
        .contains_key(effort)
        .then(|| (family, effort.to_owned()))
}

/// The family a family id or a wire id belongs to.
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub fn family_of_model(model: &str) -> Option<AgyFamily> {
    family(model.trim()).or_else(|| split_wire_id(model).map(|(family, _)| family))
}

/// alc's effort ladder on Antigravity's three rungs. Absent is `medium`,
/// agy's own default.
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub fn effort_word(effort: Option<ReasoningEffort>) -> &'static str {
    match effort {
        None | Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::Low) => "low",
        Some(
            ReasoningEffort::High
            | ReasoningEffort::Xhigh
            | ReasoningEffort::Max
            | ReasoningEffort::Ultra,
        ) => "high",
    }
}

/// The wire id for `effort` on `family`, or the family's only rung when it
/// does not serve that one (GPT-OSS serves `medium` alone).
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub fn wire_model(family: &AgyFamily, effort: &str) -> String {
    let effort = if family.thinking.contains_key(effort) {
        effort
    } else if family.thinking.contains_key("medium") {
        "medium"
    } else {
        family
            .thinking
            .keys()
            .next()
            .map_or("medium", String::as_str)
    };
    format!("{}-{effort}", family.id)
}

/// The wire id an agent pinned at launch sends: the profile's model with the
/// pinned effort's suffix. A wire id with no pinned effort stays as it is,
/// and a model alc does not know passes through for upstream to judge.
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub fn wire_model_for(model: &str, effort: Option<ReasoningEffort>) -> String {
    if let Some((family, _)) = split_wire_id(model) {
        return match effort {
            Some(_) => wire_model(&family, effort_word(effort)),
            None => model.trim().to_owned(),
        };
    }
    match family(model.trim()) {
        Some(family) => wire_model(&family, effort_word(effort)),
        None => model.trim().to_owned(),
    }
}

/// The window a session on `family` is handed: standard, or the family's
/// maximum when the profile opted in.
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub fn context_window(family: &AgyFamily, choice: ContextChoice) -> u64 {
    match choice {
        ContextChoice::Standard => family.context_window,
        ContextChoice::Max => family.max_context_window.max(family.context_window),
    }
}

/// The families as pickers show them: one row per family, effort chosen
/// separately, windows per the profile's context choice.
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub fn model_infos(choice: ContextChoice) -> Vec<ModelInfo> {
    families()
        .into_iter()
        .map(|family| {
            let supported_efforts = family
                .thinking
                .keys()
                .filter_map(|effort| effort.parse::<ReasoningEffort>().ok())
                .collect::<Vec<_>>();
            ModelInfo {
                id: family.id.clone(),
                name: family.label.clone(),
                description: format!("{} through your Antigravity login", family.label),
                context_window: context_window(&family, choice),
                max_context_window: family.max_context_window,
                default_effort: ReasoningEffort::Medium,
                supported_efforts: [
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ]
                .into_iter()
                .filter(|effort| supported_efforts.contains(effort))
                .collect(),
                priority: None,
            }
        })
        .collect()
}

/// Where Claude's tiers land: Opus for the strongest, the session's family
/// for the default, Flash for the cheapest.
#[allow(dead_code, reason = "used by the Antigravity bridge (Tasks 6-11)")]
pub fn tiers(default_family: &str) -> crate::bridge::tiers::ModelTiers {
    crate::bridge::tiers::ModelTiers {
        strongest: STRONGEST_FAMILY.to_owned(),
        default: default_family.to_owned(),
        cheapest: CHEAPEST_FAMILY.to_owned(),
    }
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

/// Where the list `alc agy` would show comes from, read off the disk alone:
/// the cache of an earlier ask, else the bundled list. Never runs agy and
/// never writes, so `alc doctor` can report it without a network round trip;
/// a stale cache is still named, since it is what is on disk.
pub fn source_on_disk(config_dir: &Path) -> String {
    match read_cache(config_dir) {
        Some(cache) => format!("cache ({})", cache.source),
        None => "bundled".to_owned(),
    }
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
        // An agy that answers with nothing alc can parse. The binary is
        // injected so the answer is read whether or not agy is installed.
        let catalog = models_from(
            temp.path(),
            true,
            Some(PathBuf::from("agy")),
            |_| Ok(String::from("nope")),
            1_000,
        );
        assert_eq!(catalog.models, bundled());
        assert!(catalog.source.contains("bundled"), "{}", catalog.source);
        assert!(
            catalog.source.contains("listed no models"),
            "{}",
            catalog.source
        );
    }

    #[test]
    fn the_bundled_list_is_the_thirteen_models_in_the_shipped_order() {
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
                "gemini-4-argon-high",
                "gemini-4-argon-medium",
                "gemini-4-argon-low",
                "gemini-3.8-flash-high",
                "gemini-3.8-flash-medium",
                "gemini-3.8-flash-low",
                "gpt-oss-120b-medium",
            ]
        );
        assert_eq!(bundled()[0].label, "Claude Opus 5.5 (High)");
        assert_eq!(bundled()[6].label, "Gemini 4 Argon (High)");
        assert_eq!(bundled()[12].label, "GPT-OSS 120B (Medium)");
    }

    #[test]
    fn the_families_carry_the_windows_caps_and_thinking_of_each_model() {
        let rows: Vec<_> = families()
            .into_iter()
            .map(|f| {
                (
                    f.id,
                    f.context_window,
                    f.max_context_window,
                    f.max_output_tokens,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("claude-opus-5-5".to_owned(), 256_000, 840_000, 128_000),
                ("claude-sonnet-5-5".to_owned(), 256_000, 840_000, 128_000),
                ("gemini-4-argon".to_owned(), 256_000, 900_000, 65_536),
                ("gemini-3.8-flash".to_owned(), 256_000, 1_048_576, 65_536),
                ("gpt-oss-120b".to_owned(), 131_072, 131_072, 32_768),
            ]
        );
        let thinking = |id: &str, effort: &str| family(id).unwrap().thinking[effort].clone();
        assert_eq!(
            thinking("claude-opus-5-5", "low"),
            serde_json::json!({"thinkingLevel": "LOW"})
        );
        assert_eq!(
            thinking("gemini-4-argon", "high"),
            serde_json::json!({"thinkingLevel": "HIGH"})
        );
        assert_eq!(
            thinking("gemini-3.8-flash", "low"),
            serde_json::json!({"thinkingBudget": 1000})
        );
        assert_eq!(
            thinking("gemini-3.8-flash", "medium"),
            serde_json::json!({"thinkingBudget": 4000})
        );
        assert_eq!(
            thinking("gemini-3.8-flash", "high"),
            serde_json::json!({"thinkingBudget": -1})
        );
        assert_eq!(
            thinking("gpt-oss-120b", "medium"),
            serde_json::json!({"thinkingBudget": 8192})
        );
        assert_eq!(family("gpt-oss-120b").unwrap().thinking.len(), 1);
    }

    #[test]
    fn every_bundled_model_is_a_family_and_one_of_its_efforts() {
        for model in bundled() {
            let (family, effort) =
                split_wire_id(&model.id).unwrap_or_else(|| panic!("{}", model.id));
            assert!(family.thinking.contains_key(&effort), "{}", model.id);
            assert!(model.label.starts_with(&family.label), "{}", model.id);
        }
    }

    #[test]
    fn wire_ids_split_into_a_family_and_an_effort() {
        let split = |model: &str| split_wire_id(model).map(|(f, e)| (f.id, e));
        assert_eq!(
            split("claude-opus-5-5-low"),
            Some(("claude-opus-5-5".to_owned(), "low".to_owned()))
        );
        assert_eq!(
            split("gemini-3.8-flash-high"),
            Some(("gemini-3.8-flash".to_owned(), "high".to_owned()))
        );
        assert_eq!(
            split("gpt-oss-120b-medium"),
            Some(("gpt-oss-120b".to_owned(), "medium".to_owned()))
        );
        assert_eq!(
            split("gpt-oss-120b-low"),
            None,
            "gpt-oss serves medium only"
        );
        assert_eq!(split("claude-opus-5-5"), None, "a family is not a wire id");
        assert_eq!(split("gemini-9-ultra-low"), None);
        assert_eq!(
            family_of_model("claude-opus-5-5").unwrap().id,
            "claude-opus-5-5"
        );
        assert_eq!(
            family_of_model("claude-opus-5-5-high").unwrap().id,
            "claude-opus-5-5"
        );
        assert!(family_of_model("gpt-6-sol").is_none());
    }

    #[test]
    fn efforts_land_on_the_three_antigravity_rungs() {
        use crate::config::ReasoningEffort as E;
        assert_eq!(effort_word(None), "medium");
        assert_eq!(effort_word(Some(E::Low)), "low");
        assert_eq!(effort_word(Some(E::Medium)), "medium");
        for effort in [E::High, E::Xhigh, E::Max, E::Ultra] {
            assert_eq!(effort_word(Some(effort)), "high", "{effort}");
        }
        let gpt = family("gpt-oss-120b").unwrap();
        assert_eq!(wire_model(&gpt, "high"), "gpt-oss-120b-medium");
        assert_eq!(
            wire_model(&family("gemini-4-argon").unwrap(), "low"),
            "gemini-4-argon-low"
        );
    }

    #[test]
    fn a_pinned_effort_picks_the_wire_suffix_at_launch() {
        use crate::config::ReasoningEffort as E;
        assert_eq!(
            wire_model_for("claude-opus-5-5-high", Some(E::Low)),
            "claude-opus-5-5-low"
        );
        assert_eq!(
            wire_model_for("claude-opus-5-5-high", None),
            "claude-opus-5-5-high"
        );
        assert_eq!(
            wire_model_for("gemini-3.8-flash", None),
            "gemini-3.8-flash-medium"
        );
        assert_eq!(
            wire_model_for("gemini-3.8-flash", Some(E::Max)),
            "gemini-3.8-flash-high"
        );
        assert_eq!(
            wire_model_for("gpt-oss-120b-medium", Some(E::High)),
            "gpt-oss-120b-medium"
        );
        assert_eq!(
            wire_model_for("gemini-9-preview", Some(E::Low)),
            "gemini-9-preview"
        );
    }

    #[test]
    fn model_infos_follow_the_context_choice() {
        use crate::config::ContextChoice;
        let standard = model_infos(ContextChoice::Standard);
        assert_eq!(standard.len(), 5);
        assert_eq!(standard[0].id, "claude-opus-5-5");
        assert_eq!(standard[0].name, "Claude Opus 5.5");
        assert_eq!(standard[0].context_window, 256_000);
        assert_eq!(standard[0].max_context_window, 840_000);
        assert_eq!(
            standard[4].supported_efforts,
            [crate::config::ReasoningEffort::Medium]
        );
        let max = model_infos(ContextChoice::Max);
        assert_eq!(max[3].id, "gemini-3.8-flash");
        assert_eq!(max[3].context_window, 1_048_576);
        assert_eq!(max[4].context_window, 131_072);
        let tiers = tiers("claude-sonnet-5-5");
        assert_eq!(tiers.strongest, "claude-opus-5-5");
        assert_eq!(tiers.default, "claude-sonnet-5-5");
        assert_eq!(tiers.cheapest, "gemini-3.8-flash");
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

    /// What `alc doctor` shows: read off the disk, never by asking agy.
    #[test]
    fn the_source_on_disk_is_read_without_running_agy() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(source_on_disk(temp.path()), "bundled");
        models_from(
            temp.path(),
            false,
            Some(PathBuf::from("agy")),
            |_| Ok(REAL_SHAPE.to_owned()),
            1_000,
        );
        assert_eq!(source_on_disk(temp.path()), "cache (agy models)");
        std::fs::write(temp.path().join(CACHE_FILE), "{not json").unwrap();
        assert_eq!(source_on_disk(temp.path()), "bundled");
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
