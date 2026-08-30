use std::ffi::OsString;

use anyhow::{Context, Result};

use crate::config::{AuthStyle, Provider, ProviderKind, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, anthropic_shaped, key_or_error,
    resolve_codex_effort, resolve_codex_model, split_chat_url,
};

/// goose's own fallback when `ANTHROPIC_HOST` is unset; alc only emits the
/// env var when a configured provider's Anthropic base URL differs from it.
const ANTHROPIC_DEFAULT_HOST: &str = "https://api.anthropic.com";

/// Goose is driven entirely through documented `GOOSE_PROVIDER` / `GOOSE_MODEL`
/// / provider-specific BYOK environment variables — there is no config file
/// to upsert, unlike Pi's `models.json`. Verified current against goose's own
/// docs (`guides/environment-variables.md` and `getting-started/providers.md`)
/// as of 2026-08-30: `OPENAI_HOST` + `OPENAI_BASE_PATH` are both still the
/// documented way to point goose's OpenAI provider at a non-default endpoint,
/// so a chat-style provider needs both split out of alc's single base URL —
/// `split_chat_url` does that split (see its doc comment for the motivating
/// zai example).
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
        apply_args(spec, passthrough);
        return Ok(());
    }

    if provider.kind == ProviderKind::Openrouter {
        let key = key_or_error(
            profile_name,
            provider,
            store.credentials.key_for(profile_name, provider),
        )?;
        spec.env.insert(
            OsString::from("GOOSE_PROVIDER"),
            OsString::from("openrouter"),
        );
        spec.env
            .insert(OsString::from("OPENROUTER_API_KEY"), OsString::from(key));
    } else if provider.kind == ProviderKind::Ollama {
        let base_url = provider
            .effective_base_url()
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?;
        spec.env
            .insert(OsString::from("GOOSE_PROVIDER"), OsString::from("ollama"));
        spec.env
            .insert(OsString::from("OLLAMA_HOST"), OsString::from(base_url));
    } else if anthropic_shaped(provider) {
        let base_url = provider
            .effective_anthropic_base_url()
            .with_context(|| format!("provider '{profile_name}' needs an Anthropic base URL"))?;
        // Unlike Pi's native /login fallback, goose has no bespoke
        // subscription login to fall back to when no key is stored, so the
        // key is required here even for Anthropic-kind providers.
        let key = key_or_error(
            profile_name,
            provider,
            store.credentials.key_for(profile_name, provider),
        )?;
        spec.env.insert(
            OsString::from("GOOSE_PROVIDER"),
            OsString::from("anthropic"),
        );
        spec.env
            .insert(OsString::from("ANTHROPIC_API_KEY"), OsString::from(key));
        if base_url != ANTHROPIC_DEFAULT_HOST {
            spec.env
                .insert(OsString::from("ANTHROPIC_HOST"), OsString::from(base_url));
        }
    } else {
        let base_url = provider
            .effective_base_url()
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?;
        let key_value = if provider.auth == AuthStyle::None {
            "alc".to_owned()
        } else {
            key_or_error(
                profile_name,
                provider,
                store.credentials.key_for(profile_name, provider),
            )?
        };
        let (host, base_path) = split_chat_url(base_url);
        spec.env
            .insert(OsString::from("GOOSE_PROVIDER"), OsString::from("openai"));
        spec.env
            .insert(OsString::from("OPENAI_API_KEY"), OsString::from(key_value));
        spec.env
            .insert(OsString::from("OPENAI_HOST"), OsString::from(host));
        spec.env.insert(
            OsString::from("OPENAI_BASE_PATH"),
            OsString::from(base_path),
        );
    }

    let model = overrides.model.as_deref().unwrap_or(&provider.model);
    spec.env
        .insert(OsString::from("GOOSE_MODEL"), OsString::from(model));
    if let Some(small_model) = provider
        .small_model
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        spec.env.insert(
            OsString::from("GOOSE_FAST_MODEL"),
            OsString::from(small_model),
        );
    }

    apply_args(spec, passthrough);
    Ok(())
}

/// Wires the bundled Codex bridge (listening on `base_url`) into Goose as an
/// OpenAI-shaped provider speaking Chat Completions. The loopback bridge
/// ignores auth entirely, so `OPENAI_API_KEY` is a fixed placeholder.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    spec.env
        .insert(OsString::from("GOOSE_PROVIDER"), OsString::from("openai"));
    spec.env
        .insert(OsString::from("OPENAI_API_KEY"), OsString::from("alc"));
    spec.env
        .insert(OsString::from("OPENAI_HOST"), OsString::from(base_url));
    spec.env.insert(
        OsString::from("OPENAI_BASE_PATH"),
        OsString::from("v1/chat/completions"),
    );
    spec.env.insert(
        OsString::from("GOOSE_MODEL"),
        OsString::from(plan.model.clone()),
    );
    Ok(())
}

/// goose's interactive entry point is the `session` subcommand; alc injects
/// it only when the user supplied no passthrough args of their own, so an
/// explicit invocation (e.g. `alc goose run ...`) is never second-guessed.
fn apply_args(spec: &mut LaunchSpec, passthrough: &[OsString]) {
    if passthrough.is_empty() {
        spec.args.push(OsString::from("session"));
    } else {
        spec.args.extend_from_slice(passthrough);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use crate::config::{Agent, Config, Credentials, Protocol, ProviderKind, ReasoningEffort};
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

    // (1) zai preset: chat-first even though it also exposes an
    // Anthropic-compatible surface; its base URL has a path segment,
    // exercising the split_chat_url helper (this is the doc comment's
    // motivating example).
    #[test]
    fn zai_preset_splits_the_chat_url_into_host_and_base_path() {
        let mut config = Config::default();
        config
            .providers
            .insert("zai".into(), Provider::for_kind(ProviderKind::Zai));
        let mut credentials = Credentials::default();
        credentials.api_keys.insert("zai".into(), "secret".into());
        let spec = launch::build(
            &store(config, credentials),
            Agent::Goose,
            Some("zai"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "GOOSE_PROVIDER").as_deref(),
            Some("openai")
        );
        assert_eq!(
            env_value(&spec, "OPENAI_HOST").as_deref(),
            Some("https://api.z.ai")
        );
        assert_eq!(
            env_value(&spec, "OPENAI_BASE_PATH").as_deref(),
            Some("api/paas/v4/chat/completions")
        );
        assert_eq!(
            env_value(&spec, "OPENAI_API_KEY").as_deref(),
            Some("secret")
        );
    }

    // (2) openrouter profile with a saved key.
    #[test]
    fn openrouter_sets_openrouter_provider_and_key_env() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Goose,
            Some("openrouter"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "GOOSE_PROVIDER").as_deref(),
            Some("openrouter")
        );
        assert_eq!(
            env_value(&spec, "OPENROUTER_API_KEY").as_deref(),
            Some("secret")
        );
        assert_eq!(
            env_value(&spec, "GOOSE_MODEL").as_deref(),
            Some("anthropic/claude-sonnet-4.6")
        );
    }

    // (3) ollama: OLLAMA_HOST is the verbatim root URL and no key env at all.
    #[test]
    fn ollama_sets_ollama_host_verbatim_and_no_key_env() {
        let spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Goose,
            Some("ollama"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "GOOSE_PROVIDER").as_deref(),
            Some("ollama")
        );
        assert_eq!(
            env_value(&spec, "OLLAMA_HOST").as_deref(),
            Some("http://localhost:11434")
        );
        assert!(
            !spec
                .env
                .keys()
                .any(|key| key.to_string_lossy().contains("API_KEY")),
            "AuthStyle::None must not require or set any API key env"
        );
    }

    // (4) anthropic kind with a stored key: default base URL means no
    // ANTHROPIC_HOST override is emitted.
    #[test]
    fn anthropic_kind_with_key_sets_no_host_override_at_the_default_url() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("anthropic".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Goose,
            Some("anthropic"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "GOOSE_PROVIDER").as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            env_value(&spec, "ANTHROPIC_API_KEY").as_deref(),
            Some("secret")
        );
        assert!(
            env_value(&spec, "ANTHROPIC_HOST").is_none(),
            "the default Anthropic URL must not be echoed back as a host override"
        );
    }

    // (4b) positive-path counterpart: an Anthropic-shaped provider whose
    // base URL is NOT the default must have ANTHROPIC_HOST set to it.
    // Custom kind + an explicit AnthropicMessages protocol makes
    // anthropic_shaped() true via its second disjunct; setting
    // anthropic_base_url directly (rather than relying on the
    // effective_base_url() fallback) keeps the fixture unambiguous about
    // which URL is under test.
    #[test]
    fn anthropic_shaped_provider_with_a_non_default_url_sets_anthropic_host() {
        let mut config = Config::default();
        let mut provider = Provider::for_kind(ProviderKind::Custom);
        provider.protocol = Protocol::AnthropicMessages;
        provider.anthropic_base_url = Some("https://gateway.example.com".into());
        config.providers.insert("gateway".into(), provider);
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("gateway".into(), "secret".into());
        let spec = launch::build(
            &store(config, credentials),
            Agent::Goose,
            Some("gateway"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "GOOSE_PROVIDER").as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            env_value(&spec, "ANTHROPIC_API_KEY").as_deref(),
            Some("secret")
        );
        assert_eq!(
            env_value(&spec, "ANTHROPIC_HOST").as_deref(),
            Some("https://gateway.example.com"),
            "a non-default Anthropic base URL must be surfaced as a host override"
        );
    }

    // (5) deepseek preset: dual chat+anthropic surface, but chat-first, so
    // anthropic_shaped() must be false and GOOSE_PROVIDER must stay openai.
    #[test]
    fn deepseek_preset_takes_the_chat_route_not_anthropic() {
        let mut config = Config::default();
        config
            .providers
            .insert("ds".into(), Provider::for_kind(ProviderKind::Deepseek));
        let mut credentials = Credentials::default();
        credentials.api_keys.insert("ds".into(), "secret".into());
        let spec = launch::build(
            &store(config, credentials),
            Agent::Goose,
            Some("ds"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "GOOSE_PROVIDER").as_deref(),
            Some("openai")
        );
        assert!(env_value(&spec, "ANTHROPIC_API_KEY").is_none());
    }

    // (6a) empty passthrough defaults to goose's interactive `session` entry.
    #[test]
    fn empty_passthrough_defaults_to_session() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Goose,
            Some("openrouter"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(spec.args, vec![OsString::from("session")]);
    }

    // (6b) non-empty passthrough is forwarded verbatim; alc never injects
    // "session" alongside an explicit invocation.
    #[test]
    fn non_empty_passthrough_is_forwarded_verbatim() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let passthrough = [
            OsString::from("run"),
            OsString::from("--name"),
            OsString::from("x"),
        ];
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Goose,
            Some("openrouter"),
            &passthrough,
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            spec.args,
            vec![
                OsString::from("run"),
                OsString::from("--name"),
                OsString::from("x")
            ]
        );
    }

    // (7) codex bridge: BridgeApi::Chat, and apply_bridge sets every env.
    #[test]
    fn codex_bridge_uses_chat_api_and_apply_bridge_sets_the_envs() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-terra".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let mut spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Goose,
            Some("codex"),
            &[],
            &overrides,
        )
        .unwrap();
        let plan = spec.bridge.clone().expect("codex bridge plan");
        assert_eq!(plan.api, BridgeApi::Chat);
        assert_eq!(plan.model, "gpt-5.6-terra");
        assert_eq!(
            spec.args,
            vec![OsString::from("session")],
            "codex branch obeys the same empty-passthrough -> session rule"
        );

        super::apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();

        assert_eq!(
            env_value(&spec, "GOOSE_PROVIDER").as_deref(),
            Some("openai")
        );
        assert_eq!(env_value(&spec, "OPENAI_API_KEY").as_deref(), Some("alc"));
        assert_eq!(
            env_value(&spec, "OPENAI_HOST").as_deref(),
            Some("http://127.0.0.1:9")
        );
        assert_eq!(
            env_value(&spec, "OPENAI_BASE_PATH").as_deref(),
            Some("v1/chat/completions")
        );
        assert_eq!(
            env_value(&spec, "GOOSE_MODEL").as_deref(),
            Some("gpt-5.6-terra")
        );
    }

    // (8) GOOSE_FAST_MODEL is set when small_model is configured (deepseek
    // preset ships with one); absent otherwise (openrouter has none).
    #[test]
    fn goose_fast_model_set_when_small_model_configured() {
        let mut config = Config::default();
        config
            .providers
            .insert("ds".into(), Provider::for_kind(ProviderKind::Deepseek));
        let mut credentials = Credentials::default();
        credentials.api_keys.insert("ds".into(), "secret".into());
        let spec = launch::build(
            &store(config, credentials),
            Agent::Goose,
            Some("ds"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            env_value(&spec, "GOOSE_FAST_MODEL").as_deref(),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            env_value(&spec, "GOOSE_MODEL").as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn goose_fast_model_absent_when_small_model_not_configured() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Goose,
            Some("openrouter"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert!(env_value(&spec, "GOOSE_FAST_MODEL").is_none());
    }
}
