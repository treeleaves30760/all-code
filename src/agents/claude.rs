use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::config::{AuthStyle, Provider, ProviderKind, ReasoningEffort, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, has_model_override, has_option,
    key_or_error, missing_key, resolve_codex_effort, resolve_codex_model,
};
use crate::model_catalog::ModelInfo;

pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    clear_cloud_provider_env(spec);

    if provider.kind == ProviderKind::Codex {
        if !overrides.model_options.is_empty()
            && !has_option(passthrough, "--settings", "--settings")
        {
            spec.args.extend([
                OsString::from("--settings"),
                OsString::from(claude_model_picker_settings(&overrides.model_options)?),
            ]);
        }
        let model = overrides
            .model
            .clone()
            .unwrap_or(resolve_codex_model(provider)?);
        let effort = overrides
            .reasoning_effort
            .or(provider.reasoning_effort)
            .or(resolve_codex_effort(provider)?)
            .unwrap_or(ReasoningEffort::Medium);
        spec.bridge = Some(BridgePlan {
            model: model.clone(),
            // Claude Code sends the effort with every request, so pinning it
            // on the bridge would freeze the in-session effort slider.
            effort: None,
            context_window: overrides.context_window,
            options: overrides.model_options.clone(),
            api: BridgeApi::Messages,
        });
        if !has_model_override(passthrough) {
            spec.args
                .extend([OsString::from("--model"), OsString::from(model)]);
        }
        if !has_option(passthrough, "--effort", "--effort") {
            spec.args
                .extend([OsString::from("--effort"), OsString::from(effort.as_str())]);
        }
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    spec.args.extend_from_slice(passthrough);
    if !provider.speaks_anthropic() {
        bail!(
            "provider '{profile_name}' speaks {}, but Claude Code needs Anthropic Messages; use an Anthropic-compatible endpoint, OpenRouter, Ollama, or `alc --codex claude`",
            provider.protocol
        );
    }

    let base_url = claude_base_url(provider)
        .with_context(|| format!("provider '{profile_name}' needs an Anthropic base URL"))?;
    spec.env.insert(
        OsString::from("ANTHROPIC_BASE_URL"),
        OsString::from(base_url),
    );
    let model = overrides.model.as_deref().unwrap_or(&provider.model);
    let small_model = provider
        .small_model
        .as_deref()
        .filter(|value| !value.is_empty());
    spec.env
        .insert(OsString::from("ANTHROPIC_MODEL"), OsString::from(model));
    if let Some(small_model) = small_model {
        spec.env.insert(
            OsString::from("ANTHROPIC_SMALL_FAST_MODEL"),
            OsString::from(small_model),
        );
    }
    if let Some(context_window) = overrides.context_window {
        spec.env.insert(
            OsString::from("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            OsString::from(context_window.to_string()),
        );
    }
    if provider.kind == ProviderKind::Ollama {
        apply_local_server_env(spec, model, small_model);
    }

    let key = store.credentials.key_for(profile_name, provider);
    match provider.auth {
        AuthStyle::ApiKey => {
            if let Some(key) = key {
                spec.set_secret_env("ANTHROPIC_API_KEY", key);
                spec.env_remove.push(OsString::from("ANTHROPIC_AUTH_TOKEN"));
            } else if provider.kind != ProviderKind::Anthropic {
                missing_key(profile_name, provider)?;
            } else {
                // No configured key means the user selected Claude's native
                // login. Do not let an unrelated ambient token override it.
                spec.env_remove.push(OsString::from("ANTHROPIC_API_KEY"));
                spec.env_remove.push(OsString::from("ANTHROPIC_AUTH_TOKEN"));
            }
        }
        AuthStyle::Bearer => {
            let key = key_or_error(profile_name, provider, key)?;
            spec.set_secret_env("ANTHROPIC_AUTH_TOKEN", key);
            // Claude Code and OpenRouter both require this to be explicitly empty.
            spec.env
                .insert(OsString::from("ANTHROPIC_API_KEY"), OsString::new());
        }
        AuthStyle::Native => {
            spec.env_remove.push(OsString::from("ANTHROPIC_API_KEY"));
            spec.env_remove.push(OsString::from("ANTHROPIC_AUTH_TOKEN"));
        }
        AuthStyle::None => {
            let token = if provider.kind == ProviderKind::Ollama {
                "ollama"
            } else {
                "alc"
            };
            spec.env.insert(
                OsString::from("ANTHROPIC_AUTH_TOKEN"),
                OsString::from(token),
            );
            spec.env
                .insert(OsString::from("ANTHROPIC_API_KEY"), OsString::new());
        }
    }
    Ok(())
}

/// Everything Claude Code has to be told about a local Ollama server that it
/// would otherwise assume from Anthropic's API.
fn apply_local_server_env(spec: &mut LaunchSpec, model: &str, small_model: Option<&str>) {
    let small = small_model.unwrap_or(model);
    for (name, value) in [
        // Ollama serves only the models that were pulled, so every alias
        // Claude Code resolves on its own (`haiku` for background work, the
        // picker's Default row, `/model sonnet`) has to land on this one
        // instead of a Claude model id the server answers with 404.
        ("ANTHROPIC_DEFAULT_MODEL", model),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", model),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", model),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", small),
        ("ANTHROPIC_SMALL_FAST_MODEL", small),
        // A local server answers one request at a time, so the session-title
        // and similar side requests would queue ahead of the real one for
        // minutes.
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    ] {
        spec.env.insert(OsString::from(name), OsString::from(value));
    }
    let is_set = |name: &str| env::var_os(name).is_some_and(|value| !value.is_empty());
    for (name, value) in local_server_timeout_env(is_set) {
        spec.env.insert(OsString::from(name), OsString::from(value));
    }
}

/// Talking to any host other than Anthropic's, Claude Code leaves its
/// runtime's idle timeout on and abandons a request that has not produced a
/// byte after about six minutes; its SDK then caps the wait for response
/// headers at ten. A laptop model can need longer than either just to read
/// the ~30k-token prompt Claude Code opens every session with, and Ollama
/// sends nothing at all until the first generated token.
pub(crate) const LOCAL_SERVER_TIMEOUT_ENV: [(&str, &str); 2] = [
    ("API_FORCE_IDLE_TIMEOUT", "0"),
    ("API_TIMEOUT_MS", "1800000"),
];

/// The timeout variables to inject for a local server, minus any the user
/// already set (`is_set`), whose own value must win.
pub(crate) fn local_server_timeout_env(
    is_set: impl Fn(&str) -> bool,
) -> Vec<(&'static str, &'static str)> {
    LOCAL_SERVER_TIMEOUT_ENV
        .into_iter()
        .filter(|(name, _)| !is_set(name))
        .collect()
}

/// Claude Code lists these rows in `/model`, so the user picks the GPT model
/// inside the session instead of before launch.
fn claude_model_picker_settings(models: &[ModelInfo]) -> Result<String> {
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
    serde_json::to_string(&json!({
        "modelPicker": {
            "options": options,
            // Claude's own lineup cannot be served through the Codex adapter.
            "replaceBuiltInOptions": true,
        }
    }))
    .context("failed to encode the Claude Code model picker")
}

pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    // Claude Code resolves its built-in aliases even when the picker lists GPT
    // models, so every alias has to land on a model the adapter can serve.
    let strongest = plan
        .options
        .first()
        .map_or(plan.model.as_str(), |model| model.id.as_str());
    let cheapest = plan
        .options
        .last()
        .map_or(plan.model.as_str(), |model| model.id.as_str());
    for (name, value) in [
        ("ANTHROPIC_MODEL", plan.model.as_str()),
        // Keeps the picker's Default row on a model the adapter can serve.
        ("ANTHROPIC_DEFAULT_MODEL", plan.model.as_str()),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", plan.model.as_str()),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", strongest),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", cheapest),
        ("ANTHROPIC_SMALL_FAST_MODEL", cheapest),
    ] {
        spec.env.insert(OsString::from(name), OsString::from(value));
    }
    spec.env.insert(
        OsString::from("ANTHROPIC_BASE_URL"),
        OsString::from(base_url),
    );
    // Clients older than the `modelPicker` setting still get one selectable
    // GPT entry from the documented custom-model variables.
    spec.env.insert(
        OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION"),
        OsString::from(plan.model.clone()),
    );
    spec.env.insert(
        OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"),
        OsString::from(format!("{} via Codex", plan.model)),
    );
    spec.env.insert(
        OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"),
        OsString::from("Selected by all-code using your Codex login"),
    );
    if let Some(context_window) = plan.context_window {
        spec.env.insert(
            OsString::from("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            OsString::from(context_window.to_string()),
        );
    }
    spec.env.insert(
        OsString::from("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        OsString::from("1"),
    );
    spec.env_remove.push(OsString::from("ANTHROPIC_API_KEY"));
    spec.env_remove.push(OsString::from("ANTHROPIC_AUTH_TOKEN"));
    Ok(())
}

/// Claude Code's own user-level settings file, which is not alc's to write.
///
/// `CLAUDE_CONFIG_DIR` moves it, and Claude Code requires that to be
/// absolute; anything else is ignored rather than guessed at, because a
/// relative path here would have alc reading some file inside the current
/// repository and calling it the user's settings.
pub(crate) fn user_settings_path() -> Option<PathBuf> {
    resolve_user_settings_path(env::var_os("CLAUDE_CONFIG_DIR"), crate::launch::home_dir())
}

/// The resolution itself, taking its inputs rather than reading them, so a
/// test can pin one arrangement without a process-wide environment variable
/// that every other test in the binary shares.
fn resolve_user_settings_path(
    config_dir: Option<OsString>,
    user_home: Option<PathBuf>,
) -> Option<PathBuf> {
    let dir = match config_dir.filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = PathBuf::from(value);
            path.is_absolute().then_some(path)?
        }
        None => user_home?.join(".claude"),
    };
    Some(dir.join("settings.json"))
}

/// The model Claude Code will start every session on when nothing overrides
/// it, read from `settings.json`.
///
/// Worth reading because of where that value comes from: picking a model in
/// `/model` writes it there as "your default for new sessions", so a GPT
/// model chosen inside `alc --codex claude` becomes the default for plain
/// `claude` too - and plain `claude` has no bridge, so the next session
/// outside alc asks api.anthropic.com for a model it has never heard of and
/// is told so. alc cannot stop Claude Code writing its own settings, so
/// `alc doctor` reads them and says what happened.
///
/// A file that is missing, unreadable, or not JSON reads as "no opinion":
/// this is a diagnostic, and a parse error in somebody else's config is not
/// alc's to report.
pub(crate) fn pinned_model(settings: &Path) -> Option<String> {
    let text = fs::read_to_string(settings).ok()?;
    let document: Value = serde_json::from_str(&text).ok()?;
    document
        .get("model")?
        .as_str()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
}

fn clear_cloud_provider_env(spec: &mut LaunchSpec) {
    for name in [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        spec.env_remove.push(OsString::from(name));
    }
}

fn claude_base_url(provider: &Provider) -> Option<String> {
    let base = provider
        .effective_anthropic_base_url()?
        .trim_end_matches('/');
    if provider.kind == ProviderKind::Openrouter && base.ends_with("/api/v1") {
        Some(base.trim_end_matches("/v1").to_owned())
    } else {
        Some(base.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_server_timeouts_are_injected_when_unset() {
        assert_eq!(
            local_server_timeout_env(|_| false),
            vec![
                ("API_FORCE_IDLE_TIMEOUT", "0"),
                ("API_TIMEOUT_MS", "1800000")
            ]
        );
    }

    #[test]
    fn the_settings_file_follows_claude_codes_own_rules() {
        // Built rather than spelled: `/work/claude` is an absolute path on
        // unix and a relative one on Windows, which would make this test
        // assert the opposite of itself on one of the two platforms.
        let home = env::temp_dir().join("ada");
        let elsewhere = env::temp_dir().join("work-claude");
        assert!(home.is_absolute() && elsewhere.is_absolute());

        assert_eq!(
            resolve_user_settings_path(None, Some(home.clone())),
            Some(home.join(".claude").join("settings.json"))
        );
        assert_eq!(
            resolve_user_settings_path(
                Some(elsewhere.clone().into_os_string()),
                Some(home.clone())
            ),
            Some(elsewhere.join("settings.json"))
        );
        // Claude Code requires an absolute CLAUDE_CONFIG_DIR and ignores
        // anything else; guessing would have alc reading a file inside
        // whatever repository it happens to be standing in.
        assert_eq!(
            resolve_user_settings_path(Some(OsString::from("relative/claude")), Some(home.clone())),
            None
        );
        assert_eq!(resolve_user_settings_path(None, None), None);
    }

    #[test]
    fn a_pinned_model_is_read_and_anything_unreadable_is_no_opinion() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");

        assert_eq!(pinned_model(&settings), None, "a file that is not there");

        fs::write(
            &settings,
            r#"{"model": "gpt-5.6-sol", "tui": "fullscreen"}"#,
        )
        .unwrap();
        assert_eq!(pinned_model(&settings), Some("gpt-5.6-sol".to_owned()));

        fs::write(&settings, r#"{"tui": "fullscreen"}"#).unwrap();
        assert_eq!(pinned_model(&settings), None, "no model key");

        fs::write(&settings, r#"{"model": "   "}"#).unwrap();
        assert_eq!(pinned_model(&settings), None, "blank is not a model");

        // Someone else's malformed config is not alc's to report.
        fs::write(&settings, "{not json").unwrap();
        assert_eq!(pinned_model(&settings), None);
    }

    #[test]
    fn a_user_set_timeout_variable_is_left_alone() {
        assert_eq!(
            local_server_timeout_env(|name| name == "API_TIMEOUT_MS"),
            vec![("API_FORCE_IDLE_TIMEOUT", "0")]
        );
        assert!(local_server_timeout_env(|_| true).is_empty());
    }
}
