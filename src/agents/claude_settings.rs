//! What `alc claude` hands Claude Code through `--settings`.
//!
//! Claude Code keeps exactly one channel for a session it moves into the
//! background: the flags it was launched with, `--settings` among them, which
//! it reads from disk again every time its supervisor restarts the session.
//! Exported environment variables do not survive that trip - the supervisor
//! drops a gateway it was not itself started with, and the session comes up
//! on Anthropic's API or not logged in at all. So everything alc used to put
//! in Claude Code's environment lives in a settings document instead, written
//! once to `<config>/claude/settings-<hash>.json`.
//!
//! The document never holds a secret. Where a credential is needed Claude
//! Code asks alc for it through `apiKeyHelper` (`alc claude-credential`),
//! which reads the same `credentials.toml` alc always has - so moving the
//! wiring onto disk costs no second copy of any key.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::bridge::tiers::ModelTiers;
use crate::bridge_host::files::{RouteRecord, valid_route_id};
use crate::config::validate_profile_name;
use crate::model_catalog::ModelInfo;

/// Stands in for the bridge's origin in a Codex document until
/// `launch::prepare` knows the port.
pub(crate) const BRIDGE_ORIGIN: &str = "{alc-bridge-origin}";

/// Where alc keeps the settings documents it hands Claude Code.
pub(crate) fn settings_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("claude")
}

/// The variables that select a cloud provider ahead of every credential Claude
/// Code has. Blanked in every document: a background session inherits what the
/// supervisor's shell exported, and one of these left set would send an alc
/// session to Bedrock.
const CLOUD_PROVIDER_SWITCHES: [&str; 3] = [
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

/// Claude Code features that exist only on Claude models, turned off where the
/// endpoint cannot serve one: a `[1m]` variant would assume a million-token
/// window, fast mode switches the session to Opus, and the advisor is a server
/// tool on Anthropic's API.
const CLAUDE_ONLY_FEATURES_OFF: [(&str, &str); 3] = [
    ("CLAUDE_CODE_DISABLE_1M_CONTEXT", "1"),
    ("CLAUDE_CODE_DISABLE_FAST_MODE", "1"),
    ("CLAUDE_CODE_DISABLE_ADVISOR_TOOL", "1"),
];

/// How often Claude Code asks the helper again on a Codex route. Short, so a
/// bridge that stopped is started again within a minute of a session needing
/// it; Claude Code also reruns the helper on a 401.
const CODEX_HELPER_TTL_MS: &str = "60000";

/// The Codex models Claude's tiers land on: the catalog lists the most capable
/// first and the cheapest last, as the picker shows them.
pub(crate) fn tiers_for(model: &str, options: &[ModelInfo]) -> ModelTiers {
    ModelTiers {
        strongest: options
            .first()
            .map_or(model, |first| first.id.as_str())
            .to_owned(),
        default: model.to_owned(),
        cheapest: options
            .last()
            .map_or(model, |last| last.id.as_str())
            .to_owned(),
    }
}

pub(crate) struct CodexDocument<'a> {
    pub model: &'a str,
    pub options: &'a [ModelInfo],
    pub context_window: Option<u64>,
    pub tiers: &'a ModelTiers,
    pub route: &'a str,
    pub helper: String,
}

/// A Claude Code session on the Codex login: every model it can name lands on
/// a Codex model, and every feature only a Claude model has is off.
pub(crate) fn codex_document(inputs: &CodexDocument<'_>) -> Value {
    let mut env = Map::new();
    put(
        &mut env,
        "ANTHROPIC_BASE_URL",
        format!("{BRIDGE_ORIGIN}/r/{}", inputs.route),
    );
    put(&mut env, "ANTHROPIC_API_KEY", "");
    put(&mut env, "ANTHROPIC_AUTH_TOKEN", "");
    for name in CLOUD_PROVIDER_SWITCHES {
        put(&mut env, name, "");
    }
    put(
        &mut env,
        "CLAUDE_CODE_API_KEY_HELPER_TTL_MS",
        CODEX_HELPER_TTL_MS,
    );
    put(&mut env, "ANTHROPIC_MODEL", inputs.model);
    // Keeps the picker's Default row on a model the adapter can serve.
    put(&mut env, "ANTHROPIC_DEFAULT_MODEL", inputs.model);
    put(
        &mut env,
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        inputs.tiers.strongest.as_str(),
    );
    put(
        &mut env,
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        inputs.tiers.strongest.as_str(),
    );
    put(
        &mut env,
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        inputs.tiers.default.as_str(),
    );
    put(
        &mut env,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        inputs.tiers.cheapest.as_str(),
    );
    put(
        &mut env,
        "ANTHROPIC_SMALL_FAST_MODEL",
        inputs.tiers.cheapest.as_str(),
    );
    // Clients older than the `modelPicker` setting still get one selectable
    // GPT entry from the documented custom-model variables.
    put(&mut env, "ANTHROPIC_CUSTOM_MODEL_OPTION", inputs.model);
    put(
        &mut env,
        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
        format!("{} via Codex", inputs.model),
    );
    put(
        &mut env,
        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        "Selected by all-code using your Codex login",
    );
    if let Some(window) = inputs.context_window {
        put(
            &mut env,
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            window.to_string(),
        );
    }
    for (name, value) in CLAUDE_ONLY_FEATURES_OFF {
        put(&mut env, name, value);
    }
    put(&mut env, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");

    let mut document = json!({
        // The `model` setting is what agent view reads for its dispatch
        // default when it is opened from inside a session with `←`.
        "model": inputs.model,
        "apiKeyHelper": inputs.helper,
        "env": env,
    });
    if !inputs.options.is_empty() {
        document["modelPicker"] = model_picker(inputs.options);
    }
    document
}

pub(crate) struct KeyedDocument<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub small_model: Option<&'a str>,
    pub context_window: Option<u64>,
    /// The variable this profile reads its key from. When it is one of Claude
    /// Code's own it is left unblanked, so a key exported for this profile
    /// still reaches Claude Code directly, as it always has.
    pub key_env: Option<&'a str>,
    pub helper: String,
}

/// A hosted provider with an API key. It serves its own models or maps
/// Claude's, so Claude Code keeps its aliases.
pub(crate) fn keyed_document(inputs: &KeyedDocument<'_>) -> Value {
    let mut env = Map::new();
    put(&mut env, "ANTHROPIC_BASE_URL", inputs.base_url);
    for name in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
        if inputs.key_env != Some(name) {
            put(&mut env, name, "");
        }
    }
    for name in CLOUD_PROVIDER_SWITCHES {
        put(&mut env, name, "");
    }
    put(&mut env, "ANTHROPIC_MODEL", inputs.model);
    if let Some(small) = inputs.small_model {
        put(&mut env, "ANTHROPIC_SMALL_FAST_MODEL", small);
    }
    if let Some(window) = inputs.context_window {
        put(
            &mut env,
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            window.to_string(),
        );
    }
    json!({ "apiKeyHelper": inputs.helper, "env": env })
}

pub(crate) struct NativeDocument<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub small_model: Option<&'a str>,
    pub context_window: Option<u64>,
}

/// Claude Code on its own login. The credential is Claude Code's, so there is
/// no helper - but the model the session was launched with, its small model
/// and its context window are alc's, and a background session keeps only what
/// its settings say.
///
/// The key variables are left out rather than blanked. The other documents
/// blank them because a helper answers in their place; here nothing would, and
/// an empty key is no stand-in for a login. alc still removes a stray one from
/// the environment of the process it starts, as it always has.
pub(crate) fn native_document(inputs: &NativeDocument<'_>) -> Value {
    let mut env = Map::new();
    put(&mut env, "ANTHROPIC_BASE_URL", inputs.base_url);
    for name in CLOUD_PROVIDER_SWITCHES {
        put(&mut env, name, "");
    }
    put(&mut env, "ANTHROPIC_MODEL", inputs.model);
    if let Some(small) = inputs.small_model {
        put(&mut env, "ANTHROPIC_SMALL_FAST_MODEL", small);
    }
    if let Some(window) = inputs.context_window {
        put(
            &mut env,
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            window.to_string(),
        );
    }
    json!({ "env": env })
}

pub(crate) struct LocalDocument<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub small_model: Option<&'a str>,
    pub context_window: Option<u64>,
    /// `ollama` for Ollama, `alc` for anything else that wants a non-empty
    /// token and authenticates nothing.
    pub placeholder: &'a str,
    /// Ollama serves only what it has pulled, so every alias lands on this
    /// model and the Claude-only features go off. A keyless custom endpoint
    /// may be a proxy that does serve Claude, so it keeps Claude's aliases.
    pub local_server: bool,
    /// The timeout variables to include: the ones the user has not set.
    pub timeouts: &'a [(&'static str, &'static str)],
}

/// A keyless endpoint: a local server, or a custom one with no key.
pub(crate) fn local_document(inputs: &LocalDocument<'_>) -> Value {
    let mut env = Map::new();
    put(&mut env, "ANTHROPIC_BASE_URL", inputs.base_url);
    put(&mut env, "ANTHROPIC_AUTH_TOKEN", inputs.placeholder);
    put(&mut env, "ANTHROPIC_API_KEY", "");
    for name in CLOUD_PROVIDER_SWITCHES {
        put(&mut env, name, "");
    }
    put(&mut env, "ANTHROPIC_MODEL", inputs.model);
    if let Some(small) = inputs.small_model {
        put(&mut env, "ANTHROPIC_SMALL_FAST_MODEL", small);
    }
    if let Some(window) = inputs.context_window {
        put(
            &mut env,
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            window.to_string(),
        );
    }
    let mut document = json!({ "env": env });
    if inputs.local_server {
        pin_local_server(
            &mut document,
            inputs.model,
            inputs.small_model,
            inputs.timeouts,
        );
    }
    document
}

/// Adds what an Ollama server needs on top of any document: every alias on its
/// one model, non-essential traffic off, the Claude-only features off, and the
/// timeouts the user has not set. Follows the provider's kind, not its auth
/// style: an Ollama behind an authenticating proxy still serves only what it
/// has pulled.
pub(crate) fn pin_local_server(
    document: &mut Value,
    model: &str,
    small_model: Option<&str>,
    timeouts: &[(&'static str, &'static str)],
) {
    let Some(env) = document.get_mut("env").and_then(Value::as_object_mut) else {
        return;
    };
    let small = small_model.unwrap_or(model);
    for (name, value) in [
        // Ollama serves only the models that were pulled, so every alias
        // Claude Code resolves on its own has to land on this one instead of a
        // Claude model id the server answers with 404.
        ("ANTHROPIC_DEFAULT_MODEL", model),
        ("ANTHROPIC_DEFAULT_FABLE_MODEL", model),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", model),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", model),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", small),
        ("ANTHROPIC_SMALL_FAST_MODEL", small),
        // One request at a time: side requests would queue ahead of the real
        // one for minutes.
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    ] {
        put(env, name, value);
    }
    for (name, value) in CLAUDE_ONLY_FEATURES_OFF {
        put(env, name, value);
    }
    for (name, value) in timeouts {
        put(env, name, *value);
    }
}

/// The shell Claude Code runs `apiKeyHelper` through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shell {
    Cmd,
    Sh,
}

impl Shell {
    /// `cmd` on Windows and `/bin/sh` everywhere else, as Claude Code does.
    pub(crate) const HOST: Self = if cfg!(windows) { Self::Cmd } else { Self::Sh };
}

/// The `apiKeyHelper` line. Claude Code runs it and reads its standard output
/// as the credential, so the absolute path of this alc and its configuration
/// directory are spelled out: a background session's environment belongs to
/// whichever shell started Claude Code's supervisor.
pub(crate) fn helper_command(
    shell: Shell,
    alc: &Path,
    config_dir: &Path,
    route: &str,
) -> Result<String> {
    let alc = alc
        .to_str()
        .context("alc's own path is not valid UTF-8, which Claude Code's settings cannot carry")?;
    let dir = config_dir.to_str().context(
        "alc's configuration directory is not valid UTF-8, which Claude Code's settings cannot carry",
    )?;
    check_route(route)?;
    match shell {
        Shell::Cmd => {
            for (what, value) in [
                ("alc's own path", alc),
                ("alc's configuration directory", dir),
            ] {
                if value.contains(['%', '"']) {
                    bail!(
                        "{what} ({value}) contains % or \", which cmd cannot pass through to \
                         Claude Code's apiKeyHelper unchanged; move it somewhere without them"
                    );
                }
            }
            Ok(format!(
                "{} --config-dir {} claude-credential {route}",
                cmd_quote(alc),
                cmd_quote(dir)
            ))
        }
        Shell::Sh => Ok(format!(
            "{} --config-dir {} claude-credential {}",
            sh_quote(alc),
            sh_quote(dir),
            sh_quote(route)
        )),
    }
}

/// Refuses a route alc did not mint.
///
/// A route is `codex-` and twelve hex digits, or `profile:` and a profile
/// name. The charset of a profile name is checked by `Config::validate`,
/// which runs on a save, in `doctor` and in the TUI - but never on
/// `Store::load`, so a hand-edited `config.toml` carries whatever it says all
/// the way here. cmd would split `my profile` into two arguments, expand
/// `prod%USERNAME%` and run `or&calc` as a command of its own, every time
/// Claude Code refreshed the credential.
///
/// Refused rather than quoted, for both shells alike: a route alc did not
/// mint is a bug in alc, not something a user typed, and the charset that
/// survives this check needs no quoting in either shell.
fn check_route(route: &str) -> Result<()> {
    let named = route
        .strip_prefix("profile:")
        .is_some_and(|profile| validate_profile_name(profile).is_ok());
    if !named && !valid_route_id(route) {
        bail!(
            "refusing to build an apiKeyHelper for route {route:?}: alc's routes are `codex-` and \
             twelve hex digits, or `profile:` and a profile name of letters, numbers, '-' and '_'"
        );
    }
    Ok(())
}

/// `value` in double quotes for a line cmd runs. cmd hands the quotes on as
/// they are, and alc reads its arguments by the Microsoft C runtime's rules,
/// which Rust's standard library follows: a backslash is literal unless a run
/// of them ends at a quote, where each pair stands for one backslash and an
/// odd one out escapes the quote. So a trailing run is doubled - `"C:\alc\"`
/// would reach alc as `C:\alc"` and the rest of the line - and never trimmed,
/// because `C:\` without it is `C:`, the current directory on that drive.
fn cmd_quote(value: &str) -> String {
    let body = value.trim_end_matches('\\');
    let run = &value[body.len()..];
    format!("\"{body}{run}{run}\"")
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn put(env: &mut Map<String, Value>, name: &str, value: impl Into<String>) {
    env.insert(name.to_owned(), Value::String(value.into()));
}

fn model_picker(models: &[ModelInfo]) -> Value {
    let options: Vec<Value> = models
        .iter()
        .map(|model| {
            json!({
                "model": model.id,
                "label": model.name,
                "description": model.description,
            })
        })
        .collect();
    // Claude's own lineup cannot be served through the Codex adapter.
    json!({ "options": options, "replaceBuiltInOptions": true })
}

/// A launch's settings document: built in the user's shell, finished and
/// written by `launch::prepare`, which is also where `--settings` is put in
/// the arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SettingsPlan {
    /// The document. On a Codex route its `ANTHROPIC_BASE_URL` starts with
    /// [`BRIDGE_ORIGIN`] until the bridge's port is known.
    pub document: Value,
    /// The Claude Code subcommand `--settings` follows, when there is one
    /// (`agents`). Otherwise it goes first. Found again in the arguments at
    /// the last moment rather than remembered as an index, because a shared
    /// session puts its permission flag in front of everything after `build`.
    pub subcommand: Option<String>,
    /// The Codex route this launch runs on, when it runs on the bridge.
    pub route: Option<RouteRecord>,
    /// The model ids alc offered this session, for the default-model guard.
    pub offered: Vec<String>,
}

/// Where a finished document lives: named by its contents, so identical
/// launches share one file and no file a live session reads is rewritten.
pub(crate) fn settings_path(config_dir: &Path, bytes: &[u8]) -> PathBuf {
    let digest = Sha256::digest(bytes);
    let name: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    settings_dir(config_dir).join(format!("settings-{name}.json"))
}

/// The document's final bytes, with the bridge's origin filled in.
pub(crate) fn finish(plan: &SettingsPlan, origin: Option<&str>) -> Result<Vec<u8>> {
    let mut document = plan.document.clone();
    if let Some(origin) = origin
        && let Some(url) = document.pointer_mut("/env/ANTHROPIC_BASE_URL")
    {
        let filled = url.as_str().map(|text| text.replace(BRIDGE_ORIGIN, origin));
        if let Some(filled) = filled {
            *url = Value::String(filled);
        }
    }
    serde_json::to_vec_pretty(&document).context("failed to encode Claude Code's settings")
}

/// Writes a finished document unless an identical one is already there, and
/// answers its path. Owner-only: it names the user's endpoints and paths.
pub(crate) fn write_settings(config_dir: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let path = settings_path(config_dir, bytes);
    if fs::read(&path).is_ok_and(|existing| existing == bytes) {
        return Ok(path);
    }
    let dir = settings_dir(config_dir);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    crate::config::atomic_write(&path, bytes, true)?;
    Ok(path)
}

/// Where `--settings <path>` goes in `args`.
pub(crate) fn insertion_point(plan: &SettingsPlan, args: &[OsString]) -> usize {
    plan.subcommand
        .as_deref()
        .and_then(|name| args.iter().position(|arg| arg == name))
        .map_or(0, |at| at + 1)
}

/// Takes a `--settings` the user passed out of their arguments and answers
/// what it said. Claude Code honours only the last `--settings`, so leaving
/// it in would silently replace alc's wiring with theirs.
pub(crate) fn take_user_settings(args: &[OsString]) -> Result<(Vec<OsString>, Option<Value>)> {
    let mut kept = Vec::with_capacity(args.len());
    let mut found = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let text = arg.to_string_lossy();
        let value = if text == "--settings" {
            Some(
                iter.next()
                    .cloned()
                    .context("`--settings` needs a file or a JSON object after it")?,
            )
        } else {
            text.strip_prefix("--settings=").map(OsString::from)
        };
        match value {
            Some(value) => found = Some(read_user_settings(&value)?),
            None => kept.push(arg.clone()),
        }
    }
    Ok((kept, found))
}

fn read_user_settings(value: &OsStr) -> Result<Value> {
    let text = value.to_string_lossy();
    let inline = without_byte_order_mark(&text);
    let parsed: Value = if inline.trim_start().starts_with('{') {
        serde_json::from_str(inline).context("the `--settings` JSON you passed does not parse")?
    } else {
        let path = Path::new(value);
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read the `--settings` file {}", path.display()))?;
        serde_json::from_str(without_byte_order_mark(&raw))
            .with_context(|| format!("{} is not valid JSON", path.display()))?
    };
    if !parsed.is_object() {
        bail!("`--settings` must be a JSON object");
    }
    // Claude Code's `env` is an object. Any other `env` would win the merge
    // over alc's whole block - the endpoint, the blanked credentials, the
    // model pins - and Claude Code would then send what alc's `apiKeyHelper`
    // prints to its own default endpoint. The values inside are its to check.
    if parsed.get("env").is_some_and(|env| !env.is_object()) {
        bail!(
            "`--settings` has an `env` that is not an object; Claude Code's `env` maps \
             variable names to values"
        );
    }
    Ok(parsed)
}

/// `text` without the one byte-order mark it may start with. Windows
/// PowerShell 5.1 writes one at the start of every file it saves as UTF-8,
/// and JSON lets a reader ignore it, but `serde_json` refuses it. A user's own
/// settings must not be turned away for how their editor saved them.
fn without_byte_order_mark(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Folds the user's settings over alc's: their keys win, and inside `env`
/// each variable they set wins over alc's.
pub(crate) fn merge_user_settings(document: &mut Value, user: &Value) {
    let (Some(ours), Some(theirs)) = (document.as_object_mut(), user.as_object()) else {
        return;
    };
    for (key, value) in theirs {
        if key == "env"
            && let (Some(Value::Object(env)), Value::Object(their_env)) =
                (ours.get_mut("env"), value)
        {
            for (name, setting) in their_env {
                env.insert(name.clone(), setting.clone());
            }
            continue;
        }
        ours.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_catalog::ModelCatalog;
    use std::ffi::OsString;

    fn env(document: &Value) -> &Map<String, Value> {
        document["env"].as_object().expect("an env block")
    }

    fn codex() -> Value {
        let options = ModelCatalog::built_in().models;
        let tiers = tiers_for("gpt-5.6-terra", &options);
        codex_document(&CodexDocument {
            model: "gpt-5.6-terra",
            options: &options,
            context_window: Some(272_000),
            tiers: &tiers,
            route: "codex-0123456789ab",
            helper: "HELPER".to_owned(),
        })
    }

    /// The guideline, row by row: every model Claude Code can name lands on a
    /// Codex model, and every Claude-only feature is off.
    #[test]
    fn a_codex_document_turns_every_claude_model_into_a_codex_model() {
        let document = codex();
        let env = env(&document);
        let value = |name: &str| env[name].as_str().unwrap_or_default().to_owned();

        assert_eq!(
            value("ANTHROPIC_BASE_URL"),
            format!("{BRIDGE_ORIGIN}/r/codex-0123456789ab")
        );
        assert_eq!(value("ANTHROPIC_MODEL"), "gpt-5.6-terra");
        assert_eq!(value("ANTHROPIC_DEFAULT_MODEL"), "gpt-5.6-terra");
        assert_eq!(value("ANTHROPIC_DEFAULT_FABLE_MODEL"), "gpt-6-astra");
        assert_eq!(value("ANTHROPIC_DEFAULT_OPUS_MODEL"), "gpt-6-astra");
        assert_eq!(value("ANTHROPIC_DEFAULT_SONNET_MODEL"), "gpt-5.6-terra");
        assert_eq!(value("ANTHROPIC_DEFAULT_HAIKU_MODEL"), "gpt-5.6-luna");
        assert_eq!(value("ANTHROPIC_SMALL_FAST_MODEL"), "gpt-5.6-luna");
        assert_eq!(value("CLAUDE_CODE_MAX_CONTEXT_TOKENS"), "272000");
        for name in [
            "CLAUDE_CODE_DISABLE_1M_CONTEXT",
            "CLAUDE_CODE_DISABLE_FAST_MODE",
            "CLAUDE_CODE_DISABLE_ADVISOR_TOOL",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        ] {
            assert_eq!(value(name), "1", "{name}");
        }
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ] {
            assert_eq!(value(name), "", "{name} is blanked");
        }
        assert_eq!(value("CLAUDE_CODE_API_KEY_HELPER_TTL_MS"), "60000");
        assert_eq!(document["model"], "gpt-5.6-terra");
        assert_eq!(document["apiKeyHelper"], "HELPER");
        assert_eq!(document["modelPicker"]["replaceBuiltInOptions"], true);
        let rows = document["modelPicker"]["options"]
            .as_array()
            .expect("the picker's rows");
        assert_eq!(rows[0]["model"], "gpt-6-astra");
        assert_eq!(
            rows[0]["label"], "GPT-6 Astra",
            "the most capable model is listed first"
        );
        assert!(
            rows.iter().all(|row| row.get("capabilities").is_none()),
            "the setting schema only accepts model, label, and description"
        );
        assert!(
            !env.keys().any(|name| name.contains("EFFORT")),
            "Claude Code sends its effort per request; pinning one would freeze the slider"
        );
        assert!(
            !env.contains_key("CLAUDE_CODE_SUBAGENT_MODEL"),
            "subagents follow the model chosen in the session"
        );
    }

    #[test]
    fn a_keyed_document_blanks_every_credential_but_the_profiles_own() {
        let document = keyed_document(&KeyedDocument {
            base_url: "https://openrouter.ai/api",
            model: "anthropic/claude-sonnet-5",
            small_model: None,
            context_window: None,
            key_env: Some("OPENROUTER_API_KEY"),
            helper: "HELPER".to_owned(),
        });
        let env = env(&document);
        assert_eq!(env["ANTHROPIC_BASE_URL"], "https://openrouter.ai/api");
        assert_eq!(env["ANTHROPIC_MODEL"], "anthropic/claude-sonnet-5");
        assert_eq!(env["ANTHROPIC_API_KEY"], "");
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "");
        assert_eq!(document["apiKeyHelper"], "HELPER");
        // A hosted provider serves real Claude models, or maps their names.
        for name in [
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "CLAUDE_CODE_DISABLE_1M_CONTEXT",
        ] {
            assert!(!env.contains_key(name), "{name}");
        }

        // The anthropic kind reads its key from ANTHROPIC_API_KEY, so a key
        // exported for it must still reach Claude Code.
        let anthropic = keyed_document(&KeyedDocument {
            base_url: "https://api.anthropic.com",
            model: "claude-sonnet-5",
            small_model: None,
            context_window: None,
            key_env: Some("ANTHROPIC_API_KEY"),
            helper: "HELPER".to_owned(),
        });
        assert!(!self::env(&anthropic).contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(self::env(&anthropic)["ANTHROPIC_AUTH_TOKEN"], "");
    }

    #[test]
    fn an_ollama_document_pins_every_alias_to_the_local_model() {
        let document = local_document(&LocalDocument {
            base_url: "http://localhost:11434",
            model: "qwen3-coder",
            small_model: Some("qwen3:4b"),
            context_window: Some(65_536),
            placeholder: "ollama",
            local_server: true,
            timeouts: &[("API_FORCE_IDLE_TIMEOUT", "0")],
        });
        let env = env(&document);
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "ollama");
        assert_eq!(env["ANTHROPIC_API_KEY"], "");
        for name in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
        ] {
            assert_eq!(env[name], "qwen3-coder", "{name}");
        }
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "qwen3:4b");
        assert_eq!(env["ANTHROPIC_SMALL_FAST_MODEL"], "qwen3:4b");
        assert_eq!(env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "65536");
        assert_eq!(env["CLAUDE_CODE_DISABLE_FAST_MODE"], "1");
        assert_eq!(env["API_FORCE_IDLE_TIMEOUT"], "0");
        assert!(
            !env.contains_key("API_TIMEOUT_MS"),
            "a timeout the user set is theirs"
        );
        assert!(document.get("apiKeyHelper").is_none(), "nothing to fetch");

        let custom = local_document(&LocalDocument {
            base_url: "http://gateway.test",
            model: "m",
            small_model: None,
            context_window: None,
            placeholder: "alc",
            local_server: false,
            timeouts: &[],
        });
        assert_eq!(self::env(&custom)["ANTHROPIC_AUTH_TOKEN"], "alc");
        assert!(!self::env(&custom).contains_key("ANTHROPIC_DEFAULT_OPUS_MODEL"));
    }

    #[test]
    fn the_helper_is_quoted_for_the_shell_claude_code_uses() {
        let alc = Path::new(r"C:\Program Files\alc\alc.exe");
        let dir = Path::new(r"C:\Users\Ada Lovelace\.config\alc");
        assert_eq!(
            helper_command(Shell::Cmd, alc, dir, "codex-0123456789ab").unwrap(),
            r#""C:\Program Files\alc\alc.exe" --config-dir "C:\Users\Ada Lovelace\.config\alc" claude-credential codex-0123456789ab"#
        );
        // Backslashes right before a quote are read as escapes, so a trailing
        // run is doubled to keep the quote closing the argument - and kept,
        // not trimmed, so `C:\` still names the drive root.
        assert_eq!(
            helper_command(Shell::Cmd, alc, Path::new(r"C:\alc\"), "codex-0123456789ab").unwrap(),
            r#""C:\Program Files\alc\alc.exe" --config-dir "C:\alc\\" claude-credential codex-0123456789ab"#
        );
        assert_eq!(
            helper_command(Shell::Cmd, alc, Path::new(r"C:\"), "codex-0123456789ab").unwrap(),
            r#""C:\Program Files\alc\alc.exe" --config-dir "C:\\" claude-credential codex-0123456789ab"#
        );
        assert_eq!(
            helper_command(
                Shell::Sh,
                Path::new("/home/o'neil/.local/bin/alc"),
                Path::new("/home/o'neil/.config/alc"),
                "profile:openrouter"
            )
            .unwrap(),
            r#"'/home/o'\''neil/.local/bin/alc' --config-dir '/home/o'\''neil/.config/alc' claude-credential 'profile:openrouter'"#
        );
        // cmd expands %NAME% even inside quotes; there is no spelling that
        // survives, so such a path is refused rather than mangled.
        let error = helper_command(
            Shell::Cmd,
            Path::new(r"C:\100%\alc.exe"),
            dir,
            "codex-0123456789ab",
        )
        .unwrap_err();
        assert!(error.to_string().contains('%'), "{error}");
    }

    /// The route is the one argument alc builds rather than reads from the
    /// user's disk - except that a profile name reaches `Store::load` without
    /// ever passing `Config::validate`, so a hand-edited `config.toml` can put
    /// a space, a `%` or an `&` in one. cmd would split the first into two
    /// arguments, expand the second and run the third as a command of its own,
    /// every time Claude Code refreshed the credential.
    #[test]
    fn a_route_alc_did_not_mint_is_refused_for_either_shell() {
        let alc = Path::new("/usr/local/bin/alc");
        let dir = Path::new("/home/ada/.config/alc");
        for shell in [Shell::Cmd, Shell::Sh] {
            for route in [
                "codex-0123456789ab",
                "profile:openrouter",
                "profile:open_router-2",
            ] {
                let line = helper_command(shell, alc, dir, route).unwrap();
                let tail = match shell {
                    Shell::Cmd => format!("claude-credential {route}"),
                    Shell::Sh => format!("claude-credential '{route}'"),
                };
                assert!(line.ends_with(&tail), "{shell:?}: {line}");
            }
            for route in [
                "profile:my profile",
                "profile:or&calc",
                "profile:prod%USERNAME%",
                r#"profile:quoted"name"#,
                "profile:",
                "codex-0123456789abc",
                "codex-0123456789AB",
                "../../etc/passwd",
                "",
            ] {
                let error = helper_command(shell, alc, dir, route)
                    .unwrap_err()
                    .to_string();
                // Spelled as the refusal spells it, so a route whose own
                // characters would be read as punctuation still reads back.
                assert!(
                    error.contains(&format!("{route:?}")),
                    "{shell:?} {route:?}: {error}"
                );
            }
        }
    }

    fn plan(document: Value, subcommand: Option<&str>) -> SettingsPlan {
        SettingsPlan {
            document,
            subcommand: subcommand.map(str::to_owned),
            route: None,
            offered: Vec::new(),
        }
    }

    #[test]
    fn a_finished_document_names_the_bridge_and_its_file_names_its_contents() {
        let codex_plan = plan(codex(), None);
        let bytes = finish(&codex_plan, Some("http://127.0.0.1:24817")).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.contains("\"http://127.0.0.1:24817/r/codex-0123456789ab\""),
            "{text}"
        );
        assert!(!text.contains(BRIDGE_ORIGIN));

        let dir = Path::new("config");
        let path = settings_path(dir, &bytes);
        assert_eq!(path.parent(), Some(dir.join("claude").as_path()));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("settings-") && name.ends_with(".json") && name.len() == 30,
            "{name}"
        );
        assert_eq!(
            settings_path(dir, &bytes),
            path,
            "the same contents, the same file"
        );
        let other = finish(&codex_plan, Some("http://127.0.0.1:24818")).unwrap();
        assert_ne!(
            settings_path(dir, &other),
            path,
            "other contents, another file"
        );
    }

    #[test]
    fn writing_a_document_twice_leaves_one_owner_only_file() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = finish(&plan(codex(), None), Some("http://127.0.0.1:24817")).unwrap();
        let first = write_settings(temp.path(), &bytes).unwrap();
        let second = write_settings(temp.path(), &bytes).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), bytes);
        assert_eq!(
            std::fs::read_dir(settings_dir(temp.path()))
                .unwrap()
                .count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn settings_go_first_or_right_after_agents() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            insertion_point(&plan(json!({}), None), &args(&["--model", "m"])),
            0
        );
        assert_eq!(
            insertion_point(
                &plan(json!({}), Some("agents")),
                &args(&["agents", "--model", "m"])
            ),
            1
        );
        // A shared session puts its permission flag in front of everything.
        assert_eq!(
            insertion_point(
                &plan(json!({}), Some("agents")),
                &args(&["--permission-mode", "default", "agents"])
            ),
            3
        );
    }

    #[test]
    fn the_users_own_settings_are_taken_out_and_win_the_merge() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("mine.json");
        std::fs::write(
            &file,
            r#"{"env": {"ANTHROPIC_MODEL": "mine"}, "theme": "dark"}"#,
        )
        .unwrap();
        let args = vec![
            OsString::from("--settings"),
            file.clone().into_os_string(),
            OsString::from("-p"),
            OsString::from("hi"),
        ];
        let (kept, user) = take_user_settings(&args).unwrap();
        assert_eq!(kept, vec![OsString::from("-p"), OsString::from("hi")]);
        let user = user.expect("the file was read");

        let mut document = codex();
        merge_user_settings(&mut document, &user);
        assert_eq!(
            document["env"]["ANTHROPIC_MODEL"], "mine",
            "their variable wins"
        );
        assert_eq!(
            document["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "gpt-5.6-luna",
            "ours stay"
        );
        assert_eq!(document["theme"], "dark");

        let (_, inline) =
            take_user_settings(&[OsString::from(r#"--settings={"model":"x"}"#)]).unwrap();
        assert_eq!(inline.unwrap()["model"], "x");

        let broken =
            take_user_settings(&[OsString::from("--settings"), OsString::from("{not json")]);
        assert!(broken.is_err());
        let missing = take_user_settings(&[OsString::from("--settings")]);
        assert!(missing.is_err());
    }

    /// Windows PowerShell 5.1 starts every file it saves as UTF-8 with a
    /// byte-order mark. A user's own settings are not refused for one, from a
    /// file or as JSON on the command line.
    #[test]
    fn settings_saved_with_a_byte_order_mark_are_still_read_and_merged() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("saved-by-powershell.json");
        let mut contents = vec![0xEF, 0xBB, 0xBF];
        contents.extend_from_slice(br#"{"theme":"dark"}"#);
        std::fs::write(&file, &contents).unwrap();

        let (kept, found) =
            take_user_settings(&[OsString::from("--settings"), file.into_os_string()]).unwrap();
        assert!(kept.is_empty(), "{kept:?}");
        let user = found.expect("the file was read");
        let mut document = codex();
        merge_user_settings(&mut document, &user);
        assert_eq!(document["theme"], "dark");
        assert_eq!(
            document["env"]["ANTHROPIC_MODEL"], "gpt-5.6-terra",
            "ours stay"
        );

        let (_, inline) =
            take_user_settings(&[OsString::from("--settings=\u{feff}{\"theme\":\"dark\"}")])
                .unwrap();
        assert_eq!(inline.expect("the JSON was read")["theme"], "dark");
    }

    /// Claude Code's `env` maps variable names to values. Any other `env`
    /// would replace alc's whole block in the merge, endpoint and all, so it
    /// is refused, inline or from a file. An empty one changes nothing.
    #[test]
    fn settings_whose_env_is_not_an_object_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        // The same JSON passed both ways a user can pass it.
        let forms = |json: &str, name: &str| {
            let file = temp.path().join(name);
            std::fs::write(&file, json).unwrap();
            [
                vec![OsString::from(format!("--settings={json}"))],
                vec![OsString::from("--settings"), file.into_os_string()],
            ]
        };
        for (json, name) in [
            (r#"{"env": null}"#, "null.json"),
            (r#"{"env": []}"#, "array.json"),
            (r#"{"env": "x"}"#, "string.json"),
        ] {
            for args in forms(json, name) {
                let error = format!("{:#}", take_user_settings(&args).unwrap_err());
                assert!(error.contains("`env`"), "{args:?}: {error}");
            }
        }

        for args in forms(r#"{"env": {}}"#, "empty.json") {
            let (_, found) = take_user_settings(&args).unwrap();
            let mut document = codex();
            merge_user_settings(&mut document, &found.expect("the settings were read"));
            assert_eq!(document["env"], codex()["env"], "{args:?}");
        }
    }
}
