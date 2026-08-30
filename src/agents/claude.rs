use std::ffi::OsString;

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
    spec.env.insert(
        OsString::from("ANTHROPIC_MODEL"),
        OsString::from(overrides.model.as_deref().unwrap_or(&provider.model)),
    );
    if let Some(small_model) = provider
        .small_model
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        spec.env.insert(
            OsString::from("ANTHROPIC_SMALL_FAST_MODEL"),
            OsString::from(small_model),
        );
    }

    let key = store.credentials.key_for(profile_name, provider);
    match provider.auth {
        AuthStyle::ApiKey => {
            if let Some(key) = key {
                spec.env
                    .insert(OsString::from("ANTHROPIC_API_KEY"), OsString::from(key));
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
            spec.env
                .insert(OsString::from("ANTHROPIC_AUTH_TOKEN"), OsString::from(key));
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
