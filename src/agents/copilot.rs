use std::ffi::OsString;

use anyhow::{Context, Result};

use crate::config::{AuthStyle, Provider, ProviderKind, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, anthropic_shaped, has_option, key_or_error,
    resolve_codex_effort, resolve_codex_model,
};

/// Copilot CLI is driven entirely through documented `COPILOT_PROVIDER_*` and
/// `COPILOT_MODEL` BYOK environment variables: there is no config file to
/// upsert and no bespoke provider id to mint, unlike Pi's `models.json` or
/// OpenCode's inline config.
pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if provider.kind == ProviderKind::Codex {
        let model = overrides
            .model
            .clone()
            .map_or_else(|| resolve_codex_model(provider), Ok)?;
        let effort = overrides
            .reasoning_effort
            .or(provider.reasoning_effort)
            .or(resolve_codex_effort(provider)?);
        spec.bridge = Some(BridgePlan {
            model,
            effort,
            context_window: overrides.context_window,
            options: overrides.model_options.clone(),
            api: BridgeApi::Chat,
        });
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    let (provider_type, base_url) = if anthropic_shaped(provider) {
        let base_url = provider
            .effective_anthropic_base_url()
            .with_context(|| format!("provider '{profile_name}' needs an Anthropic base URL"))?
            .to_owned();
        ("anthropic", base_url)
    } else {
        // Verbatim, unlike the Pi/OpenCode builders' openai_style_base_url:
        // Copilot's own docs use http://localhost:11434 for Ollama's root and
        // https://api.openai.com/v1 for OpenAI, both matching alc's stored URLs.
        let base_url = provider
            .effective_base_url()
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?
            .to_owned();
        ("openai", base_url)
    };

    spec.env.insert(
        OsString::from("COPILOT_PROVIDER_TYPE"),
        OsString::from(provider_type),
    );
    spec.env.insert(
        OsString::from("COPILOT_PROVIDER_BASE_URL"),
        OsString::from(base_url),
    );

    // Copilot documents the key as optional for unauthenticated providers
    // (e.g. a local Ollama server); every other auth style requires one.
    if provider.auth != AuthStyle::None {
        let key = key_or_error(
            profile_name,
            provider,
            store.credentials.key_for(profile_name, provider),
        )?;
        spec.env.insert(
            OsString::from("COPILOT_PROVIDER_API_KEY"),
            OsString::from(key),
        );
    }

    if !has_option(passthrough, "--model", "--model") {
        let model = overrides.model.as_deref().unwrap_or(&provider.model);
        spec.env
            .insert(OsString::from("COPILOT_MODEL"), OsString::from(model));
    }

    spec.args.extend_from_slice(passthrough);
    Ok(())
}

/// Wires the bundled Codex bridge (listening on `base_url`) into Copilot's
/// environment as an OpenAI-shaped provider. The loopback bridge ignores
/// auth entirely, so `COPILOT_PROVIDER_API_KEY` is a fixed placeholder.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    spec.env.insert(
        OsString::from("COPILOT_PROVIDER_TYPE"),
        OsString::from("openai"),
    );
    spec.env.insert(
        OsString::from("COPILOT_PROVIDER_BASE_URL"),
        OsString::from(format!("{base_url}/v1")),
    );
    spec.env.insert(
        OsString::from("COPILOT_PROVIDER_API_KEY"),
        OsString::from("alc"),
    );
    if !has_option(&spec.args, "--model", "--model") {
        spec.env.insert(
            OsString::from("COPILOT_MODEL"),
            OsString::from(plan.model.clone()),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use crate::config::{Agent, Config, Credentials, ProviderKind, ReasoningEffort};
    use crate::launch::{self, BridgeApi};
    use crate::model_catalog::ModelCatalog;

    fn store(config: Config, credentials: Credentials) -> Store {
        Store {
            dir: PathBuf::from("test"),
            config,
            credentials,
        }
    }

    fn env_value(spec: &LaunchSpec, name: &str) -> Option<String> {
        spec.env
            .get(OsStr::new(name))
            .map(|value| value.to_string_lossy().into_owned())
    }

    // (1) openrouter profile with a saved key.
    #[test]
    fn openrouter_sets_openai_provider_env_and_model() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Copilot,
            Some("openrouter"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_TYPE").as_deref(),
            Some("openai")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_BASE_URL").as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_API_KEY").as_deref(),
            Some("secret")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_MODEL").as_deref(),
            Some("anthropic/claude-sonnet-4.6")
        );
    }

    // (2) anthropic kind with a stored key.
    #[test]
    fn anthropic_kind_uses_anthropic_provider_type_and_base_url() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("anthropic".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Copilot,
            Some("anthropic"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_TYPE").as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_BASE_URL").as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_API_KEY").as_deref(),
            Some("secret")
        );
    }

    // (3) deepseek preset: dual chat+anthropic surface, but chat-first, so
    // anthropic_shaped() must be false and the chat base URL must be used.
    #[test]
    fn deepseek_preset_uses_the_chat_base_url_not_anthropic() {
        let mut config = Config::default();
        config
            .providers
            .insert("ds".into(), Provider::for_kind(ProviderKind::Deepseek));
        let mut credentials = Credentials::default();
        credentials.api_keys.insert("ds".into(), "secret".into());
        let spec = launch::build(
            &store(config, credentials),
            Agent::Copilot,
            Some("ds"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_TYPE").as_deref(),
            Some("openai")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_BASE_URL").as_deref(),
            Some("https://api.deepseek.com/v1")
        );
    }

    // (4) ollama: AuthStyle::None must skip the API key env entirely, and the
    // base URL is passed verbatim (no /v1 suffix, unlike the Pi builder).
    #[test]
    fn ollama_skips_the_api_key_env_entirely() {
        let spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Copilot,
            Some("ollama"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert!(
            env_value(&spec, "COPILOT_PROVIDER_API_KEY").is_none(),
            "AuthStyle::None must not require or set an API key"
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_TYPE").as_deref(),
            Some("openai")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_BASE_URL").as_deref(),
            Some("http://localhost:11434")
        );
    }

    // (5) codex bridge: BridgeApi::Chat, and apply_bridge sets all four envs.
    #[test]
    fn codex_bridge_uses_chat_api_and_apply_bridge_sets_four_envs() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-terra".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let mut spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Copilot,
            Some("codex"),
            &[],
            &overrides,
        )
        .unwrap();
        let plan = spec.bridge.clone().expect("codex bridge plan");
        assert_eq!(plan.api, BridgeApi::Chat);
        assert_eq!(plan.model, "gpt-5.6-terra");

        super::apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();

        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_TYPE").as_deref(),
            Some("openai")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_BASE_URL").as_deref(),
            Some("http://127.0.0.1:9/v1")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_PROVIDER_API_KEY").as_deref(),
            Some("alc")
        );
        assert_eq!(
            env_value(&spec, "COPILOT_MODEL").as_deref(),
            Some("gpt-5.6-terra")
        );
    }

    // (6) explicit user passthrough --model wins; alc must not also set
    // COPILOT_MODEL.
    #[test]
    fn explicit_model_passthrough_suppresses_copilot_model_env() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let passthrough = [OsString::from("--model"), OsString::from("x")];
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Copilot,
            Some("openrouter"),
            &passthrough,
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert!(env_value(&spec, "COPILOT_MODEL").is_none());
        assert_eq!(
            spec.args,
            vec![OsString::from("--model"), OsString::from("x")]
        );
    }
}
