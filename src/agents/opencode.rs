use std::ffi::OsString;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::config::{AuthStyle, Protocol, Provider, ProviderKind, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, has_model_override, openai_style_base_url,
    resolve_codex_effort, resolve_codex_model,
};

pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if provider.kind == ProviderKind::Codex {
        spec.bridge = Some(BridgePlan {
            model: overrides
                .model
                .clone()
                .map_or_else(|| resolve_codex_model(provider), Ok)?,
            effort: overrides
                .reasoning_effort
                .or(provider.reasoning_effort)
                .or(resolve_codex_effort(provider)?),
            context_window: overrides.context_window,
            options: overrides.model_options.clone(),
            api: BridgeApi::Responses,
        });
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    let (provider_id, needs_custom_config) = match provider.kind {
        ProviderKind::Anthropic => ("anthropic".to_owned(), custom_kind_url(provider)),
        ProviderKind::Openai => ("openai".to_owned(), custom_kind_url(provider)),
        ProviderKind::Openrouter => ("openrouter".to_owned(), custom_kind_url(provider)),
        // OpenCode documents Ollama as an explicit OpenAI-compatible provider.
        // Always inject it so a fresh OpenCode install does not depend on local
        // provider discovery having run first.
        ProviderKind::Ollama => ("ollama".to_owned(), true),
        // The seven OpenAI-chat presets have no bespoke OpenCode provider id
        // of their own, so they are injected the same way as vLLM/Custom.
        ProviderKind::Vllm
        | ProviderKind::Deepseek
        | ProviderKind::Moonshot
        | ProviderKind::Zai
        | ProviderKind::Minimax
        | ProviderKind::Groq
        | ProviderKind::Xai
        | ProviderKind::Google
        | ProviderKind::Custom => (format!("alc-{profile_name}"), true),
        ProviderKind::Codex => unreachable!(),
    };

    let key = store.credentials.key_for(profile_name, provider);
    if let Some(key) = &key {
        let env_name = provider
            .api_key_env
            .as_deref()
            .unwrap_or(match provider.kind {
                ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
                ProviderKind::Openai => "OPENAI_API_KEY",
                ProviderKind::Openrouter => "OPENROUTER_API_KEY",
                _ => "ALC_PROVIDER_API_KEY",
            });
        spec.env
            .insert(OsString::from(env_name), OsString::from(key));
        spec.env
            .insert(OsString::from("ALC_PROVIDER_API_KEY"), OsString::from(key));
    }

    let model = overrides.model.as_deref().unwrap_or(&provider.model);
    let model_reference = format!("{provider_id}/{model}");
    let mut inline = json!({
        "$schema": "https://opencode.ai/config.json"
    });
    if !has_model_override(passthrough) {
        inline["model"] = Value::String(model_reference);
    }

    if needs_custom_config {
        let base_url = openai_style_base_url(provider)
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?;
        let package = match (provider.kind, provider.protocol) {
            (ProviderKind::Ollama, _) => "@ai-sdk/openai-compatible",
            (_, Protocol::AnthropicMessages) => "@ai-sdk/anthropic",
            (_, Protocol::OpenaiResponses | Protocol::Dual) => "@ai-sdk/openai",
            (_, Protocol::OpenaiChat) => "@ai-sdk/openai-compatible",
            (_, Protocol::CodexNative) => {
                bail!("provider '{profile_name}' uses codex-native, which OpenCode cannot load")
            }
        };
        let mut options = Map::new();
        options.insert("baseURL".to_owned(), Value::String(base_url.to_owned()));
        if provider.auth != AuthStyle::None {
            options.insert(
                "apiKey".to_owned(),
                Value::String("{env:ALC_PROVIDER_API_KEY}".to_owned()),
            );
        }
        inline["provider"] = json!({
                &provider_id: {
                    "npm": package,
                    "name": format!("alc: {profile_name}"),
                    "options": options,
                    "models": {
                        (model): { "name": model }
                    }
                }
        });
    }

    spec.env.insert(
        OsString::from("OPENCODE_CONFIG_CONTENT"),
        OsString::from(serde_json::to_string(&inline)?),
    );
    spec.args.extend_from_slice(passthrough);
    Ok(())
}

/// Wires the bundled Codex bridge (listening on `base_url`) into an OpenCode
/// session as an `@ai-sdk/openai` provider named `alc-codex`, pointed at the
/// loopback bridge instead of api.openai.com.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    let models: Map<String, Value> = if plan.options.is_empty() {
        let mut fallback = Map::new();
        fallback.insert(plan.model.clone(), json!({ "name": plan.model }));
        fallback
    } else {
        plan.options
            .iter()
            .map(|model| (model.id.clone(), json!({ "name": model.name })))
            .collect()
    };

    let mut inline = json!({
        "$schema": "https://opencode.ai/config.json"
    });
    if !has_model_override(&spec.args) {
        inline["model"] = Value::String(format!("alc-codex/{}", plan.model));
    }
    inline["provider"] = json!({
        "alc-codex": {
            "npm": "@ai-sdk/openai",
            "name": "alc: Codex subscription",
            "options": {
                // The loopback bridge ignores auth entirely; the SDK still
                // requires some non-empty apiKey value to be configured.
                "baseURL": format!("{base_url}/v1"),
                "apiKey": "alc",
            },
            "models": models,
        }
    });

    spec.env.insert(
        OsString::from("OPENCODE_CONFIG_CONTENT"),
        OsString::from(serde_json::to_string(&inline)?),
    );
    Ok(())
}

fn custom_kind_url(provider: &Provider) -> bool {
    match (
        provider.effective_base_url(),
        provider.kind.default_base_url(),
    ) {
        (Some(actual), Some(default)) => {
            actual.trim_end_matches('/') != default.trim_end_matches('/')
        }
        (Some(_), None) => true,
        _ => false,
    }
}
