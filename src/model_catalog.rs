//! What Codex models alc offers, and where that list comes from.
//!
//! # The shipped catalog is a floor, not a candidate list
//!
//! alc does not route a single turn through the Codex CLI - its bridge posts
//! straight to chatgpt.com - so a Codex that has not heard of a model is
//! evidence about that install and never about the model. alc learned that
//! the expensive way: on a machine whose Codex was one release behind,
//! `codex debug models` did not fail. It succeeded, and returned a list
//! without `gpt-6-astra`, and alc wrote that shorter list over its own
//! bundled one. Posting the same slug to chatgpt.com by hand streamed
//! normally; the model was simply absent from every picker alc builds.
//!
//! So discovery may add to, enrich and extend the models alc ships, and it
//! may never remove one. [`ModelCatalog::apply_floor`] is that rule, and it
//! runs on every read rather than only on the writing path, so a catalog
//! already pruned onto a user's disk repairs itself on the next command -
//! offline, with no upgrade, and with no login.
//!
//! # Two sources, one parser
//!
//! The account rung asks the party that actually serves the models
//! ([`crate::bridge::models`]); the Codex rung shells out to `codex debug
//! models` when that fails. Both answer the same shape and go through the
//! same [`discovered_models`], so no rule can apply to one and not the other,
//! and the floor is put back over whichever of them answered.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::{Provider, ReasoningEffort};

const CACHE_FILE: &str = "codex-models.json";
const REFRESH_INTERVAL_SECONDS: u64 = 24 * 60 * 60;
/// The Codex release alc claims to be when the installed Codex is older, or
/// is not there at all.
///
/// chatgpt.com gates each model on a `minimal_client_version` and believes
/// whatever version the caller declares, so this number decides how much of
/// the catalog comes back: `gpt-6-astra` carries a minimum of 0.153.0, and a
/// caller declaring 0.149.1 is simply not shown it. This is the release alc
/// has seen the whole catalog under. It is a floor rather than a pin, because
/// a user whose Codex is newer should be asking as that newer client - and on
/// the day this constant does go stale nothing breaks, the catalog just stops
/// growing until someone bumps it.
///
/// Nothing in alc can notice that on its own: a model held back by a
/// `minimal_client_version` above this number is absent from the answer, and
/// an absence is exactly what this whole module refuses to draw conclusions
/// from. Neither can the drift job compare the two lists and see it, because
/// with no `codex login` in CI both of its lists come from the same
/// `codex debug models`. So the weekly job checks this constant directly,
/// against the version of the Codex it just installed from npm, and says so
/// when alc has fallen behind it.
const CODEX_CLIENT_VERSION_FLOOR: &str = "0.154.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalog {
    pub schema_version: u32,
    pub refreshed_at: u64,
    pub source: String,
    /// The Codex release that had last filled `models_cache.json` when this
    /// catalog was written, or `None` when that file could not be read.
    ///
    /// `#[serde(default)]`, like every field added after the first release:
    /// [`ModelCatalog`] denies unknown fields and [`ModelCatalog::built_in`]
    /// `expect`s, so a field without a default would stop `models/codex.json`
    /// itself from parsing and panic the binary at startup.
    #[serde(default)]
    pub codex_cache_stamp: Option<String>,
    /// Ids the floor put back because the source that answered did not report
    /// them. Printed by `alc models` and `alc doctor`, so a list that is
    /// complete only because alc insisted is never mistaken for one the
    /// source agreed with.
    #[serde(default)]
    pub unreported: Vec<String>,
    /// Why the account rung did not answer, when the Codex rung had to. One
    /// sentence, carried so the user is told which half is degraded instead
    /// of being left to infer it from a shorter list.
    #[serde(default)]
    pub fallback_reason: Option<String>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_window: u64,
    pub default_effort: ReasoningEffort,
    pub supported_efforts: Vec<ReasoningEffort>,
    /// Codex's own capability rank, ascending, as the source that answered
    /// reported it - `None` for an entry nothing has reported yet, which is
    /// every model in `models/codex.json` and every model a source skipped.
    ///
    /// Carried through the cache rather than used and dropped inside one
    /// refresh, because the rank is what decides where a model alc does not
    /// ship sits in the list, and [`ModelCatalog::apply_floor`] re-decides
    /// that on every read - including reads that never touch a source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
}

/// Where this machine's Codex login and Codex state live.
///
/// Carried rather than looked up inside the catalog, because a profile can
/// pin its own `codex_home`: a refresh has to *ask* the account whose
/// `auth.json` the launch will sign its requests with, or it is reporting one
/// login's entitlements while another login does the talking.
///
/// What it does not do is keep two logins' answers apart on disk. The cache is
/// one `codex-models.json` per config dir, so with two pinned homes whichever
/// profile refreshed last is the list every profile then reads. That is
/// survivable only because of the floor: every model alc ships is present
/// whoever answered, so the worst case is an *extra* entry that the other
/// account cannot use, which chatgpt.com refuses by name at launch - not a
/// missing one, which is the failure this module exists to prevent.
#[derive(Debug, Clone, Default)]
pub struct CodexSource {
    /// `auth.json`, for the account rung.
    pub auth_file: Option<PathBuf>,
    /// The Codex home, for the `models_cache.json` version stamp.
    pub home: Option<PathBuf>,
}

impl CodexSource {
    /// The machine's own Codex, for the commands that have no profile in
    /// hand: `alc models` and the `alc config` TUI read the catalog for the
    /// machine rather than for one provider.
    pub fn detect() -> Self {
        Self {
            auth_file: crate::launch::default_codex_auth_file(),
            home: crate::launch::default_codex_home(),
        }
    }

    pub fn for_provider(provider: &Provider) -> Self {
        Self {
            auth_file: crate::launch::codex_auth_file(provider).ok(),
            home: crate::launch::codex_home_for(provider),
        }
    }

    /// The Codex release that last filled `models_cache.json`, read straight
    /// out of that file's own `client_version` key.
    ///
    /// Scanned as bytes rather than deserialised: the file is a third of a
    /// megabyte of per-model prompt text, this runs in front of every launch,
    /// and the key sits in the first hundred bytes. It is also the installed
    /// Codex's version for free, on a path that cannot afford a process
    /// spawn. A file that cannot be read is `None`, which makes every rule
    /// that consults it simply not fire - the safe direction, because the
    /// floor already guarantees the list is complete.
    pub fn cache_stamp(&self) -> Option<String> {
        let path = self.home.as_ref()?.join("models_cache.json");
        let mut head = Vec::new();
        fs::File::open(path)
            .ok()?
            .take(4096)
            .read_to_end(&mut head)
            .ok()?;
        let text = String::from_utf8_lossy(&head);
        let after_key = text.split_once("\"client_version\"")?.1;
        let after_quote = after_key.split_once('"')?.1;
        let (value, _) = after_quote.split_once('"')?;
        (!value.is_empty()).then(|| value.to_owned())
    }

    /// The catalog as the account itself states it, plus the client version
    /// alc declared to get it - which `alc models` prints, because it is the
    /// single number that decides how much of the catalog came back.
    fn fetch_account_catalog(&self, stamp: Option<&str>) -> Result<(String, String), String> {
        let auth_file = self
            .auth_file
            .as_deref()
            .ok_or_else(|| "could not resolve the Codex auth path; set CODEX_HOME".to_owned())?;
        let login = crate::usage::accounts::read_codex_login(
            auth_file,
            crate::launch::home_dir().as_deref(),
        )?;
        let declared = declared_client_version(stamp);
        let body = crate::bridge::models::fetch_account_models(
            &login,
            &declared,
            now_unix().saturating_mul(1_000),
        )?;
        Ok((body, declared))
    }
}

impl ModelCatalog {
    pub fn built_in() -> Self {
        serde_json::from_str(include_str!("../models/codex.json"))
            .expect("the bundled Codex model catalog must be valid")
    }

    pub fn load(config_dir: &Path) -> Self {
        let mut catalog = read_cache(config_dir).unwrap_or_else(Self::built_in);
        // The floor is an invariant of every read, not a property of one
        // writer. `--dry-run`, `alc config` and `alc doctor` all reach a
        // catalog without going through `refresh`, and a cache written by an
        // older alc was pruned by a rule this release no longer believes in -
        // so repairing it here is what fixes a machine whose picker is
        // already missing a model, without waiting for a sync that may never
        // succeed.
        catalog.unreported = catalog.apply_floor();
        catalog.retain_routable();
        catalog
    }

    /// Puts back every model alc ships that the answering source did not
    /// report, and returns their ids.
    ///
    /// Order is alc's for the models alc ships, never the source's:
    /// `agents::claude::apply_bridge` turns the first entry into Claude
    /// Code's `opus` alias and the last into `haiku`, and Codex ranks
    /// `gpt-5.6-sol` and `gpt-6-astra` at the same priority - so taking the
    /// order from discovery would hand the `opus` alias to whichever of the
    /// two happened to be listed first that day. Anything discovery added
    /// that alc does not ship is slotted in by its Codex rank rather than
    /// appended, because the end of this list is not a neutral place to put
    /// something: see [`insertion_index`].
    ///
    /// Idempotent and free of I/O, so running it on every read costs a walk
    /// of four entries.
    fn apply_floor(&mut self) -> Vec<String> {
        let floor = Self::built_in().models;
        let mut restored = Vec::new();
        let mut merged = Vec::with_capacity(self.models.len() + floor.len());
        for bundled in &floor {
            match self.models.iter().find(|model| model.id == bundled.id) {
                Some(known) => merged.push(known.clone()),
                None => {
                    restored.push(bundled.id.clone());
                    merged.push(bundled.clone());
                }
            }
        }
        let shipped: BTreeSet<&str> = floor.iter().map(|model| model.id.as_str()).collect();
        for addition in self
            .models
            .iter()
            .filter(|model| !shipped.contains(model.id.as_str()))
        {
            let at = insertion_index(&merged, addition.priority);
            merged.insert(at, addition.clone());
        }
        self.models = merged;
        restored
    }

    /// Drops entries the built-in bridge cannot route.
    ///
    /// `refresh` filters what it writes, but neither catalog that reaches a
    /// screen without going through it does: the bundled fallback is
    /// `models/codex.json` verbatim, and a cache written by an older alc was
    /// filtered by nothing. Both feed the `alc config` model picker and
    /// `alc models` - which is exactly where a launch refused for an
    /// unroutable model tells the user to go.
    ///
    /// An empty result would mean the bridge disagrees with every model alc
    /// offers, which is a broken build rather than a reason to show the user
    /// nothing, so the list is left alone - the same "no opinion" rule as a
    /// bridge that reports nothing at all.
    fn retain_routable(&mut self) {
        self.retain_routable_against(crate::launch::bridge_codex_models());
    }

    /// The filter itself, taking the bridge's answer rather than asking for
    /// it, so a test can pin one bridge's behaviour without reaching for a
    /// process-wide environment variable.
    pub(crate) fn retain_routable_against(&mut self, routable: Option<BTreeSet<String>>) {
        let Some(routable) = routable else {
            return;
        };
        let kept: Vec<_> = self
            .models
            .iter()
            .filter(|model| routable.contains(&model.id))
            .cloned()
            .collect();
        if !kept.is_empty() {
            self.models = kept;
        }
    }

    pub fn load_and_refresh_if_due(config_dir: &Path, source: &CodexSource) -> Self {
        let stamp = source.cache_stamp();
        Self::refresh_if_due(config_dir, stamp, now_unix(), || {
            Self::refresh_with(config_dir, source)
        })
    }

    /// The due-and-failed rules on their own, with the sync itself passed in.
    ///
    /// Split out because the case worth pinning is the one where the sync
    /// fails, and reaching that through the real one would mean a test with
    /// no network and a `codex` binary on PATH deciding the outcome.
    fn refresh_if_due(
        config_dir: &Path,
        stamp: Option<String>,
        now: u64,
        refresh: impl FnOnce() -> Result<Self>,
    ) -> Self {
        let cached = Self::load(config_dir);
        if !refresh_due(&cached, stamp.as_deref(), now) {
            return cached;
        }
        match refresh() {
            Ok(catalog) => catalog,
            Err(_) => {
                // Neither source answered. What stops a machine with no route
                // paying the connect timeout on every single launch is
                // recording the attempt against *both* rules that made it
                // due: the clock, and the Codex release it was attempted
                // under. Bumping only the clock suppresses nothing, because
                // the stamp mismatch is checked first and would still be
                // there - which is how every `alc --codex claude`, `alc
                // config` and `alc models` ended up re-running a full sync,
                // forever, on exactly the machines where it could not work.
                //
                // The stamp therefore means "the Codex release this catalog
                // was last asked about", not "was last built from": a later
                // upgrade still produces a new stamp and still re-fires
                // immediately, and the floor means the list kept meanwhile is
                // complete anyway. A Codex home alc could not read says
                // nothing at all, so it must not erase what the catalog
                // already knows. A cache that cannot be written is not worth
                // failing a launch over.
                let mut kept = cached;
                kept.refreshed_at = now;
                if stamp.is_some() {
                    kept.codex_cache_stamp = stamp;
                }
                let _ = write_cache(config_dir, &kept);
                kept
            }
        }
    }

    /// Asks the account, then the installed Codex, then gives up and keeps
    /// what is already on disk.
    ///
    /// The account rung is first because it is the party that actually serves
    /// the turns: a model chatgpt.com will stream on request belongs in the
    /// picker whatever the local Codex has heard of, and that difference is
    /// the whole bug. The Codex rung is a real fallback rather than a fig
    /// leaf - it is what answers on a machine with no route, an expired
    /// token, or a proxy that eats TLS - and its output goes through the same
    /// filter and the same floor, so the worst either rung can do is fail to
    /// add something.
    pub fn refresh_with(config_dir: &Path, source: &CodexSource) -> Result<Self> {
        let stamp = source.cache_stamp();
        let floor = Self::built_in().models;

        let asked = source
            .fetch_account_catalog(stamp.as_deref())
            .and_then(|(body, declared)| {
                let payload = parse_payload(body.as_bytes()).ok_or_else(|| {
                    "chatgpt.com sent something that is not the expected JSON".to_owned()
                })?;
                let models = discovered_models(&payload, &floor);
                if models.is_empty() {
                    return Err("chatgpt.com listed no usable model for this account".to_owned());
                }
                Ok((
                    models,
                    format!("your ChatGPT account (chatgpt.com, client_version {declared})"),
                ))
            });

        let (models, label, fallback_reason) = match asked {
            Ok((models, label)) => (models, label, None),
            Err(account_reason) => match from_codex_cli(&floor, stamp.as_deref()) {
                Ok((models, label)) => (models, label, Some(account_reason)),
                Err(codex_reason) => bail!(
                    "could not reach your ChatGPT account ({account_reason}), and the installed \
                     Codex CLI could not be asked either ({codex_reason}); keeping the previous \
                     catalog"
                ),
            },
        };

        let mut catalog = Self {
            schema_version: 1,
            refreshed_at: now_unix(),
            source: label,
            codex_cache_stamp: stamp,
            unreported: Vec::new(),
            fallback_reason,
            models,
        };
        catalog.unreported = catalog.apply_floor();
        write_cache(config_dir, &catalog)?;
        Ok(catalog)
    }

    pub fn find(&self, id: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|model| model.id == id)
    }
}

/// Where a model alc does not ship belongs among the models it does.
///
/// The end of the catalog is a slot, not a spare seat:
/// `agents::claude::apply_bridge` hands the last entry to Claude Code as
/// ANTHROPIC_DEFAULT_HAIKU_MODEL and ANTHROPIC_SMALL_FAST_MODEL, and the
/// picker badges it `budget`. Appending would therefore aim every title,
/// summary and compaction call at whatever OpenAI shipped most recently - the
/// day a new frontier model appears, the most expensive model on the list
/// would quietly become the cheap one. So an addition is placed ahead of the
/// first model alc ships that Codex ranks strictly worse than it, and a
/// model whose rank nothing has reported goes immediately before the last
/// entry: the cheapest model alc ships keeps the cheap aliases either way.
///
/// `priority` is ascending, so a smaller number is the more capable model.
fn insertion_index(merged: &[ModelInfo], priority: Option<i64>) -> usize {
    let before_cheapest = merged.len().saturating_sub(1);
    let Some(priority) = priority else {
        return before_cheapest;
    };
    merged
        .iter()
        .position(|model| model.priority.is_some_and(|ranked| ranked > priority))
        .unwrap_or(before_cheapest)
}

/// `codex debug models`, parsed by exactly the rules the account payload gets.
///
/// Reports its reason as a plain `String` rather than an `anyhow` chain,
/// because this is one rung of a ladder: the sentence ends up inside the
/// message the last rung bails with, where "the Codex CLI was not found on
/// PATH" has to sit next to why chatgpt.com could not be asked either.
fn from_codex_cli(
    floor: &[ModelInfo],
    stamp: Option<&str>,
) -> Result<(Vec<ModelInfo>, String), String> {
    let codex = env::var_os("ALC_CODEX_BIN")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| which::which("codex").ok())
        .ok_or_else(|| "the Codex CLI was not found on PATH".to_owned())?;
    let output = Command::new(&codex)
        .args(["debug", "models"])
        .output()
        .map_err(|error| format!("failed to run {} debug models: {error}", codex.display()))?;
    if !output.status.success() {
        return Err(format!(
            "`codex debug models` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let payload = parse_payload(&output.stdout)
        .ok_or_else(|| "the installed Codex returned an invalid model catalog".to_owned())?;
    let models = discovered_models(&payload, floor);
    if models.is_empty() {
        return Err("the installed Codex reported no usable model".to_owned());
    }
    let label = match stamp {
        Some(version) => format!("installed Codex CLI {version} (`codex debug models`)"),
        None => "installed Codex CLI (`codex debug models`)".to_owned(),
    };
    Ok((models, label))
}

/// One discovery payload turned into catalog entries, whichever source
/// produced it.
///
/// Every rule here is about what may be *added* and none about what may be
/// taken away - the caller puts the floor back afterwards - because a source
/// that answered partially is indistinguishable from one that answered in
/// full, and alc has already shipped the version that could not tell the
/// difference.
///
/// Membership is decided by Codex's own `visibility` rather than by a
/// denylist of slugs: the first entry of a real `codex debug models` answer
/// is `gpt-reserve`, an internal model, and `agents::claude::apply_bridge`
/// maps the first entry of this list onto Claude Code's `opus` alias. A
/// denylist that went stale would silently point `opus` at whatever internal
/// slug shipped next, so the question asked is the one Codex answers itself.
fn discovered_models(raw: &CodexCatalogPayload, floor: &[ModelInfo]) -> Vec<ModelInfo> {
    let bundled = |slug: &str| floor.iter().find(|model| model.id == slug);

    // The band a model alc does not ship has to fall inside to be offered.
    //
    // `priority` is Codex's own capability rank, ascending, and a real answer
    // carries superseded generations (`gpt-5.5` at 12) next to current ones
    // (1 through 8). Appending those would not merely lengthen the picker:
    // the last entry becomes ANTHROPIC_DEFAULT_HAIKU_MODEL, so a previous
    // generation would quietly become Claude Code's cheap model. This is a
    // heuristic over an undocumented field, so it errs towards adding
    // nothing - when the source reports none of the models alc ships there is
    // nothing to measure against and nothing is appended.
    let worst_shipped_priority = raw
        .models
        .iter()
        .filter(|model| bundled(&model.slug).is_some())
        .filter_map(|model| model.priority)
        .max();

    let mut known = Vec::new();
    let mut added: Vec<(i64, ModelInfo)> = Vec::new();
    for model in &raw.models {
        if model.slug.is_empty() {
            continue;
        }
        let supported: Vec<ReasoningEffort> = model
            .supported_reasoning_levels
            .iter()
            .filter_map(|level| level.effort.parse().ok())
            .collect();
        match bundled(&model.slug) {
            Some(base) => {
                // Absence of information must never remove anything, so a
                // model alc ships stays unless Codex says outright to hide it.
                if model.visibility.as_deref() == Some("hide") {
                    continue;
                }
                known.push(enrich(model, base, supported));
            }
            None => {
                // The asymmetry with the arm above is deliberate: no
                // `visibility` at all means "still visible" for a model alc
                // ships and "not addable" for one it does not, because an
                // addition has to be justified and a removal does not get to
                // happen by silence.
                if model.visibility.as_deref() != Some("list")
                    || supported.is_empty()
                    || model.context_window == 0
                {
                    continue;
                }
                let Some(priority) = model.priority else {
                    continue;
                };
                if worst_shipped_priority.is_none_or(|worst| priority > worst) {
                    continue;
                }
                let default_effort = model
                    .default_reasoning_level
                    .parse()
                    .ok()
                    .filter(|effort| supported.contains(effort))
                    .unwrap_or(ReasoningEffort::Medium);
                added.push((
                    priority,
                    ModelInfo {
                        id: model.slug.clone(),
                        name: non_empty(&model.display_name).unwrap_or_else(|| model.slug.clone()),
                        description: model.description.clone(),
                        context_window: model.context_window,
                        default_effort,
                        supported_efforts: supported,
                        priority: Some(priority),
                    },
                ));
            }
        }
    }
    added.sort_by_key(|(priority, _)| *priority);
    known
        .into_iter()
        .chain(added.into_iter().map(|(_, model)| model))
        .collect()
}

/// A shipped model's entry, updated field by field rather than wholesale.
///
/// A source alc only half understands must not be able to blank a description
/// or zero a context window alc already knows: `context_window` becomes
/// CLAUDE_CODE_MAX_CONTEXT_TOKENS, and a zero there is a session that
/// compacts itself before the first reply. Each field is replaced only when
/// the answer carried something usable in it.
fn enrich(model: &CodexModel, base: &ModelInfo, supported: Vec<ReasoningEffort>) -> ModelInfo {
    let supported_efforts = if supported.is_empty() {
        base.supported_efforts.clone()
    } else {
        supported
    };
    let default_effort = model
        .default_reasoning_level
        .parse()
        .ok()
        .filter(|effort| supported_efforts.contains(effort))
        .or_else(|| {
            supported_efforts
                .contains(&base.default_effort)
                .then_some(base.default_effort)
        })
        .unwrap_or(ReasoningEffort::Medium);
    ModelInfo {
        id: model.slug.clone(),
        name: non_empty(&model.display_name).unwrap_or_else(|| base.name.clone()),
        description: non_empty(&model.description).unwrap_or_else(|| base.description.clone()),
        context_window: if model.context_window == 0 {
            base.context_window
        } else {
            model.context_window
        },
        default_effort,
        supported_efforts,
        // The one field the bundled entry cannot supply: `models/codex.json`
        // carries alc's own order and no ranks, so a source that reports a
        // rank is the only way the models alc ships become something an
        // addition can be measured against.
        priority: model.priority.or(base.priority),
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
}

/// The Codex release alc claims to be when it asks chatgpt.com for a catalog.
///
/// The newer of what alc ships knowing works and what the installed Codex
/// last used - never alc's own `CARGO_PKG_VERSION`. alc's 1.x line only beats
/// Codex's 0.x line by accident, and on the day the two cross alc would start
/// silently under-declaring and lose a model again, which is the exact
/// failure this module exists to end.
fn declared_client_version(stamp: Option<&str>) -> String {
    let Some(stamp) = stamp else {
        return CODEX_CLIENT_VERSION_FLOOR.to_owned();
    };
    match (
        semver::Version::parse(stamp),
        semver::Version::parse(CODEX_CLIENT_VERSION_FLOOR),
    ) {
        (Ok(installed), Ok(known)) if installed > known => stamp.to_owned(),
        _ => CODEX_CLIENT_VERSION_FLOOR.to_owned(),
    }
}

/// Both sources answer `{"models": [...]}` today, but the account endpoint is
/// undocumented and outside alc's control, so a bare array is accepted as
/// well rather than costing the whole refresh.
fn parse_payload(bytes: &[u8]) -> Option<CodexCatalogPayload> {
    if let Ok(payload) = serde_json::from_slice::<CodexCatalogPayload>(bytes) {
        return Some(payload);
    }
    serde_json::from_slice::<Vec<CodexModel>>(bytes)
        .ok()
        .map(|models| CodexCatalogPayload { models })
}

/// The discovery payload, with every field defaulted.
///
/// Neither source is documented and neither is alc's. A model carrying a key
/// alc has never seen, or missing one it expects, has to cost that one
/// field - not the entire catalog, which is how a machine ends up back on a
/// list that is missing a model.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexCatalogPayload {
    models: Vec<CodexModel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexModel {
    slug: String,
    display_name: String,
    description: String,
    context_window: u64,
    default_reasoning_level: String,
    supported_reasoning_levels: Vec<CodexEffort>,
    /// `"list"` or `"hide"`. `gpt-reserve` and `codex-auto-review` come back
    /// `"hide"`, and without this alc would put an internal review model into
    /// every picker it builds.
    visibility: Option<String>,
    /// Codex's own capability rank, ascending.
    priority: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexEffort {
    effort: String,
}

fn read_cache(config_dir: &Path) -> Option<ModelCatalog> {
    let text = fs::read_to_string(config_dir.join(CACHE_FILE)).ok()?;
    let catalog: ModelCatalog = serde_json::from_str(&text).ok()?;
    (catalog.schema_version == 1 && !catalog.models.is_empty()).then_some(catalog)
}

fn write_cache(config_dir: &Path, catalog: &ModelCatalog) -> Result<()> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    let path = config_dir.join(CACHE_FILE);
    let temporary = config_dir.join(format!(".{CACHE_FILE}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(catalog)?)
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

/// Whether the catalog on disk is worth replacing.
///
/// Time is the weak rule. The strong one is the Codex release stamp: a user
/// who upgraded Codex used to keep yesterday's list for up to a day, which on
/// the machine this was reported from meant a whole day of a newly visible
/// model staying hidden. The stamp comes out of a file alc already reads, so
/// the launch path can afford to check it every single time - no
/// `codex --version` spawn in front of a session.
fn refresh_due(catalog: &ModelCatalog, stamp: Option<&str>, now: u64) -> bool {
    if stamp.is_some() && stamp != catalog.codex_cache_stamp.as_deref() {
        return true;
    }
    now.saturating_sub(catalog.refreshed_at) >= REFRESH_INTERVAL_SECONDS
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `codex debug models` answer: the slugs, the
    /// `visibility` and `priority` values and their order are what a current
    /// Codex actually returns.
    const CODEX_CURRENT: &str = r#"{"models":[
      {"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol","description":"Latest frontier agentic coding model.","context_window":272000,"default_reasoning_level":"low","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}],"visibility":"list","priority":1},
      {"slug":"gpt-6-astra","display_name":"GPT-6-Astra","description":"Most capable model.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}],"visibility":"list","priority":1},
      {"slug":"gpt-reserve","display_name":"GPT-Reserve","description":"Internal.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"medium"}],"visibility":"hide","priority":3},
      {"slug":"gpt-5.6-terra","display_name":"GPT-5.6-Terra","description":"Balanced agentic coding model.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}],"visibility":"list","priority":7},
      {"slug":"gpt-5.6-luna","display_name":"GPT-5.6-Luna","description":"Fast and affordable.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"}],"visibility":"list","priority":8},
      {"slug":"gpt-5.5","display_name":"GPT-5.5","description":"Previous generation.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"}],"visibility":"list","priority":12},
      {"slug":"codex-auto-review","display_name":"Codex Auto Review","description":"Internal.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"medium"}],"visibility":"hide","priority":43}
    ]}"#;

    /// What Codex 0.149.1 answered on the reporting user's machine: a
    /// success, with no `gpt-6-astra` anywhere in it.
    const CODEX_ONE_RELEASE_BEHIND: &str = r#"{"models":[
      {"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol","description":"Reliable agentic workhorse.","context_window":272000,"default_reasoning_level":"low","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}],"visibility":"list","priority":1},
      {"slug":"gpt-5.6-terra","display_name":"GPT-5.6-Terra","description":"Balanced agentic coding model.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}],"visibility":"list","priority":7},
      {"slug":"gpt-5.6-luna","display_name":"GPT-5.6-Luna","description":"Fast and affordable.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"}],"visibility":"list","priority":8},
      {"slug":"gpt-5.5","display_name":"GPT-5.5","description":"Previous generation.","context_window":272000,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"}],"visibility":"list","priority":12}
    ]}"#;

    fn ids(catalog: &ModelCatalog) -> Vec<&str> {
        catalog
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect()
    }

    /// One discovery answer taken all the way to the catalog a picker would
    /// be built from, without touching the network, a `codex` binary or disk.
    fn catalog_from(json: &str) -> ModelCatalog {
        let floor = ModelCatalog::built_in().models;
        let payload = parse_payload(json.as_bytes()).expect("a parsable payload");
        let mut catalog = ModelCatalog {
            schema_version: 1,
            refreshed_at: now_unix(),
            source: "test".to_owned(),
            codex_cache_stamp: None,
            unreported: Vec::new(),
            fallback_reason: None,
            models: discovered_models(&payload, &floor),
        };
        catalog.unreported = catalog.apply_floor();
        catalog
    }

    #[test]
    fn the_catalog_lists_the_most_capable_model_first() {
        let ids: Vec<_> = ModelCatalog::built_in()
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        assert_eq!(
            ids,
            [
                "gpt-6-astra",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna"
            ]
        );
    }

    #[test]
    fn bundled_catalog_has_requested_models_and_efforts() {
        let catalog = ModelCatalog::built_in();
        assert_eq!(catalog.models.len(), 4);
        for model in &catalog.models {
            let id = &model.id;
            // Not every model has every tier - `ultra` arrived with GPT-6
            // and the newer GPT-5.6 models - so the floor is what all of
            // them share.
            for effort in [
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Xhigh,
                ReasoningEffort::Max,
            ] {
                assert!(
                    model.supported_efforts.contains(&effort),
                    "{id} should offer {effort}"
                );
            }
            assert!(
                model.context_window > 0,
                "{id} should include its context window"
            );
        }
    }

    /// The reported bug. A Codex one release behind does not fail - it
    /// answers without `gpt-6-astra`, and alc used to write that answer over
    /// its own list.
    #[test]
    fn a_source_that_never_mentions_gpt_6_still_leaves_it_offered() {
        let catalog = catalog_from(CODEX_ONE_RELEASE_BEHIND);
        assert_eq!(
            catalog
                .find("gpt-6-astra")
                .map(|model| model.context_window),
            Some(272_000)
        );
        assert_eq!(ids(&catalog)[0], "gpt-6-astra");
        assert_eq!(catalog.unreported, ["gpt-6-astra"]);
    }

    #[test]
    fn a_pruned_cache_on_disk_regains_what_an_older_alc_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut pruned = ModelCatalog::built_in();
        pruned.models.retain(|model| model.id != "gpt-6-astra");
        pruned.source = "installed Codex CLI (`codex debug models`)".to_owned();
        write_cache(dir.path(), &pruned).unwrap();

        // No refresh, no network and no Codex: the plain load path repairs
        // it, which is what fixes the machine the report came from.
        let loaded = ModelCatalog::load(dir.path());
        assert_eq!(ids(&loaded)[0], "gpt-6-astra");
        assert_eq!(loaded.unreported, ["gpt-6-astra"]);
    }

    #[test]
    fn a_model_codex_hides_is_never_offered() {
        let catalog = catalog_from(CODEX_CURRENT);
        assert!(catalog.find("gpt-reserve").is_none());
        assert!(catalog.find("codex-auto-review").is_none());
    }

    /// `apply_bridge` turns the last entry into Claude Code's cheap alias, so
    /// a superseded generation arriving at the bottom of the list is not a
    /// longer picker, it is a quiet downgrade.
    #[test]
    fn a_model_ranked_below_everything_alc_ships_is_not_appended() {
        let catalog = catalog_from(CODEX_CURRENT);
        assert!(catalog.find("gpt-5.5").is_none());
        assert_eq!(
            ids(&catalog),
            [
                "gpt-6-astra",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna"
            ]
        );
    }

    /// Codex ranks Sol and Astra identically and lists Sol first, so an order
    /// taken from the source would settle the `opus` alias by coin toss.
    #[test]
    fn the_order_alc_ships_survives_a_source_that_lists_another_model_first() {
        let catalog = catalog_from(CODEX_CURRENT);
        assert_eq!(ids(&catalog)[0], "gpt-6-astra");
        assert!(catalog.unreported.is_empty());
    }

    /// A model alc does not ship yet, ranked between two that it does, has to
    /// land between them. The tail of this list is Claude Code's
    /// `haiku`/small-fast alias, so appending a new frontier model would aim
    /// every background call at the most expensive thing on offer and badge
    /// it `budget` in the picker.
    #[test]
    fn a_new_model_is_ranked_among_the_models_alc_ships() {
        let json = CODEX_CURRENT
            .replace("\"gpt-5.5\"", "\"gpt-7-nova\"")
            .replace("\"priority\":12", "\"priority\":5")
            .replace("\"context_window\":272000,\"default_reasoning_level\":\"medium\",\"supported_reasoning_levels\":[{\"effort\":\"low\"},{\"effort\":\"medium\"},{\"effort\":\"high\"}]", "\"context_window\":400000,\"default_reasoning_level\":\"high\",\"supported_reasoning_levels\":[{\"effort\":\"low\"},{\"effort\":\"medium\"},{\"effort\":\"high\"}]");
        let catalog = catalog_from(&json);
        let entry = catalog
            .find("gpt-7-nova")
            .expect("a model Codex ranks alongside alc's own");
        assert_eq!(entry.context_window, 400_000);
        assert_eq!(entry.default_effort, ReasoningEffort::High);
        // Priority 5 sits between Sol (1) and Terra (7), and the cheapest
        // model alc ships keeps the last slot.
        assert_eq!(
            ids(&catalog),
            [
                "gpt-6-astra",
                "gpt-5.6-sol",
                "gpt-7-nova",
                "gpt-5.6-terra",
                "gpt-5.6-luna"
            ]
        );
    }

    /// A cache written before alc recorded ranks, or by a source that
    /// reported none, still must not hand the cheap aliases to a model alc
    /// knows nothing about.
    #[test]
    fn an_addition_with_no_rank_still_cannot_take_the_cheap_slot() {
        let dir = tempfile::tempdir().unwrap();
        let mut cached = ModelCatalog::built_in();
        cached.source = "test".to_owned();
        cached.models.push(ModelInfo {
            id: "gpt-7-nova".to_owned(),
            name: "GPT-7 Nova".to_owned(),
            description: "Brand new.".to_owned(),
            context_window: 400_000,
            default_effort: ReasoningEffort::Medium,
            supported_efforts: vec![ReasoningEffort::Medium],
            priority: None,
        });
        write_cache(dir.path(), &cached).unwrap();

        let loaded = ModelCatalog::load(dir.path());
        assert_eq!(
            ids(&loaded),
            [
                "gpt-6-astra",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-7-nova",
                "gpt-5.6-luna"
            ]
        );
    }

    /// The whole point of the ordering rules, stated once in the terms
    /// `agents::claude::apply_bridge` reads them in: the head becomes
    /// Claude Code's `opus` alias and the tail becomes `haiku`.
    #[test]
    fn discovery_can_never_move_a_new_model_into_the_haiku_slot() {
        for priority in ["1", "5", "8"] {
            let json = CODEX_CURRENT
                .replace("\"gpt-5.5\"", "\"gpt-7-nova\"")
                .replace("\"priority\":12", &format!("\"priority\":{priority}"));
            let catalog = catalog_from(&json);
            assert!(catalog.find("gpt-7-nova").is_some());
            assert_eq!(*ids(&catalog).first().unwrap(), "gpt-6-astra");
            assert_eq!(*ids(&catalog).last().unwrap(), "gpt-5.6-luna");
        }
    }

    #[test]
    fn discovery_replaces_only_the_fields_it_can_supply() {
        let json = r#"{"models":[{"slug":"gpt-6-astra","display_name":"","description":"Fresh copy.","context_window":0,"default_reasoning_level":"","supported_reasoning_levels":[],"visibility":"list","priority":1}]}"#;
        let catalog = catalog_from(json);
        let astra = catalog.find("gpt-6-astra").expect("the model alc ships");
        assert_eq!(astra.name, "GPT-6 Astra");
        assert_eq!(astra.description, "Fresh copy.");
        assert_eq!(astra.context_window, 272_000);
        assert_eq!(astra.default_effort, ReasoningEffort::Medium);
        assert!(astra.supported_efforts.contains(&ReasoningEffort::Ultra));
    }

    /// `load` runs the floor over a catalog `refresh` had already floored, so
    /// a second pass that duplicated an entry or reordered one would show up
    /// as a doubled row in every picker.
    #[test]
    fn applying_the_floor_twice_changes_nothing() {
        let mut catalog = catalog_from(CODEX_ONE_RELEASE_BEHIND);
        assert_eq!(catalog.unreported, ["gpt-6-astra"]);
        let before = catalog.models.clone();
        let restored = catalog.apply_floor();
        assert_eq!(catalog.models, before);
        assert!(restored.is_empty());
    }

    #[test]
    fn a_fresh_catalog_is_not_due() {
        let mut catalog = ModelCatalog::built_in();
        catalog.refreshed_at = now_unix();
        catalog.codex_cache_stamp = Some("0.154.0".to_owned());
        assert!(!refresh_due(&catalog, Some("0.154.0"), now_unix()));
    }

    #[test]
    fn upgrading_codex_makes_the_catalog_due_the_same_day() {
        let mut catalog = ModelCatalog::built_in();
        catalog.refreshed_at = now_unix();
        catalog.codex_cache_stamp = Some("0.149.1".to_owned());
        assert!(refresh_due(&catalog, Some("0.154.0"), now_unix()));
        // A Codex home alc cannot read says nothing, so it must not make
        // every single command re-sync.
        assert!(!refresh_due(&catalog, None, now_unix()));
    }

    /// A sync that cannot succeed must still cost at most one attempt a day.
    /// The Codex stamp is checked before the clock, so recording only the
    /// clock left the catalog permanently due and made every launch, every
    /// `alc config` and every `alc models` pay the connect timeout again.
    #[test]
    fn a_sync_that_fails_is_not_retried_by_every_later_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut stale = ModelCatalog::built_in();
        stale.codex_cache_stamp = Some("0.149.1".to_owned());
        write_cache(dir.path(), &stale).unwrap();

        let now = now_unix();
        let mut attempts = 0;
        let kept =
            ModelCatalog::refresh_if_due(dir.path(), Some("0.154.0".to_owned()), now, || {
                attempts += 1;
                bail!("no route to chatgpt.com, and no Codex to ask either")
            });
        assert_eq!(attempts, 1);
        assert_eq!(kept.codex_cache_stamp.as_deref(), Some("0.154.0"));
        // The floor still holds: a failed sync costs freshness, never a model.
        assert_eq!(ids(&kept)[0], "gpt-6-astra");

        let again =
            ModelCatalog::refresh_if_due(dir.path(), Some("0.154.0".to_owned()), now + 60, || {
                attempts += 1;
                bail!("the second command must not try again")
            });
        assert_eq!(attempts, 1);
        assert_eq!(again.codex_cache_stamp.as_deref(), Some("0.154.0"));

        // Tomorrow the clock rule takes over, which is the ceiling this is
        // meant to restore rather than remove.
        ModelCatalog::refresh_if_due(
            dir.path(),
            Some("0.154.0".to_owned()),
            now + REFRESH_INTERVAL_SECONDS,
            || {
                attempts += 1;
                bail!("due again, a day later")
            },
        );
        assert_eq!(attempts, 2);
    }

    /// A Codex home alc cannot read reports nothing, and nothing must not
    /// overwrite the release the catalog does know it was asked about.
    #[test]
    fn a_failed_sync_with_no_readable_codex_keeps_the_stamp_it_had() {
        let dir = tempfile::tempdir().unwrap();
        let mut stale = ModelCatalog::built_in();
        stale.codex_cache_stamp = Some("0.154.0".to_owned());
        write_cache(dir.path(), &stale).unwrap();

        let kept = ModelCatalog::refresh_if_due(dir.path(), None, now_unix(), || {
            bail!("no route, and no Codex home to fall back to")
        });
        assert_eq!(kept.codex_cache_stamp.as_deref(), Some("0.154.0"));
    }

    #[test]
    fn alc_never_declares_a_client_version_older_than_the_one_it_knows_works() {
        assert_eq!(declared_client_version(None), CODEX_CLIENT_VERSION_FLOOR);
        assert_eq!(
            declared_client_version(Some("0.149.1")),
            CODEX_CLIENT_VERSION_FLOOR
        );
        assert_eq!(declared_client_version(Some("0.160.2")), "0.160.2");
        assert_eq!(
            declared_client_version(Some("not-a-version")),
            CODEX_CLIENT_VERSION_FLOOR
        );
    }

    #[test]
    fn the_codex_version_comes_out_of_its_own_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("models_cache.json"),
            r#"{"fetched_at":"2026-09-16T17:32:32Z","etag":"W/\"x\"","client_version":"0.154.0","models":[]}"#,
        )
        .unwrap();
        let source = CodexSource {
            auth_file: None,
            home: Some(dir.path().to_path_buf()),
        };
        assert_eq!(source.cache_stamp().as_deref(), Some("0.154.0"));

        let missing = CodexSource {
            auth_file: None,
            home: Some(dir.path().join("nowhere")),
        };
        assert_eq!(missing.cache_stamp(), None);
    }
}
