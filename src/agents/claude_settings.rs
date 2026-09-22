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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::bridge::tiers::ModelTiers;
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
    if inputs.local_server {
        let small = inputs.small_model.unwrap_or(inputs.model);
        for (name, value) in [
            // Ollama serves only the models that were pulled, so every alias
            // Claude Code resolves on its own has to land on this one instead
            // of a Claude model id the server answers with 404.
            ("ANTHROPIC_DEFAULT_MODEL", inputs.model),
            ("ANTHROPIC_DEFAULT_FABLE_MODEL", inputs.model),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", inputs.model),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", inputs.model),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", small),
            ("ANTHROPIC_SMALL_FAST_MODEL", small),
            // One request at a time: side requests would queue ahead of the
            // real one for minutes.
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
        ] {
            put(&mut env, name, value);
        }
        for (name, value) in CLAUDE_ONLY_FEATURES_OFF {
            put(&mut env, name, value);
        }
        for (name, value) in inputs.timeouts {
            put(&mut env, name, *value);
        }
    }
    json!({ "env": env })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_catalog::ModelCatalog;

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
        assert_eq!(
            document["modelPicker"]["options"][0]["model"],
            "gpt-6-astra"
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
        let error =
            helper_command(Shell::Cmd, Path::new(r"C:\100%\alc.exe"), dir, "r").unwrap_err();
        assert!(error.to_string().contains('%'), "{error}");
    }
}
