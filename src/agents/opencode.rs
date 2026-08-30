use std::ffi::OsString;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::config::{AuthStyle, Protocol, Provider, ProviderKind, Store};
use crate::launch::{LaunchOverrides, LaunchSpec, has_model_override};

pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if provider.kind == ProviderKind::Codex {
        bail!(
            "Codex CLI login cannot be injected into OpenCode directly; connect OpenAI inside OpenCode or choose an API/OpenRouter profile"
        );
    }

    let (provider_id, needs_custom_config) = match provider.kind {
        ProviderKind::Anthropic => ("anthropic".to_owned(), custom_kind_url(provider)),
        ProviderKind::Openai => ("openai".to_owned(), custom_kind_url(provider)),
        ProviderKind::Openrouter => ("openrouter".to_owned(), custom_kind_url(provider)),
        // OpenCode documents Ollama as an explicit OpenAI-compatible provider.
        // Always inject it so a fresh OpenCode install does not depend on local
        // provider discovery having run first.
        ProviderKind::Ollama => ("ollama".to_owned(), true),
        ProviderKind::Vllm | ProviderKind::Custom => (format!("alc-{profile_name}"), true),
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
        let base_url = opencode_base_url(provider)
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

fn opencode_base_url(provider: &Provider) -> Option<String> {
    let base = provider.effective_base_url()?.trim_end_matches('/');
    if provider.kind == ProviderKind::Ollama && !base.ends_with("/v1") {
        Some(format!("{base}/v1"))
    } else {
        Some(base.to_owned())
    }
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
