use std::ffi::OsString;

use anyhow::{Context, Result};

use crate::config::{AuthStyle, Provider, ProviderKind, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, anthropic_shaped, has_model_override,
    has_option, key_or_error, prepend_args, resolve_codex_effort, resolve_codex_model,
};

/// Qwen Code (the `qwen` binary) is driven through its own `--auth-type`
/// flag (`openai` | `anthropic` | `gemini`) plus the matching BYOK
/// environment variables — there is no config file to upsert, unlike Pi's
/// `models.json`. Anthropic-shaped providers and the `google` kind each get
/// their own auth type; every other chat-speaking provider falls back to
/// `openai`. Codex is a fourth, bridged branch handled separately below.
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

    if anthropic_shaped(provider) {
        push_auth_type(spec, passthrough, "anthropic");
        let base_url = provider
            .effective_anthropic_base_url()
            .with_context(|| format!("provider '{profile_name}' needs an Anthropic base URL"))?
            .to_owned();
        let key = if provider.auth == AuthStyle::None {
            "alc".to_owned()
        } else {
            key_or_error(
                profile_name,
                provider,
                store.credentials.key_for(profile_name, provider),
            )?
        };
        spec.env.insert(
            OsString::from("ANTHROPIC_BASE_URL"),
            OsString::from(base_url),
        );
        spec.env
            .insert(OsString::from("ANTHROPIC_API_KEY"), OsString::from(key));
    } else if provider.kind == ProviderKind::Google {
        push_auth_type(spec, passthrough, "gemini");
        let key = key_or_error(
            profile_name,
            provider,
            store.credentials.key_for(profile_name, provider),
        )?;
        spec.env
            .insert(OsString::from("GEMINI_API_KEY"), OsString::from(key));
    } else {
        push_auth_type(spec, passthrough, "openai");
        let base_url = provider
            .effective_base_url()
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?
            .to_owned();
        let key = if provider.auth == AuthStyle::None {
            "alc".to_owned()
        } else {
            key_or_error(
                profile_name,
                provider,
                store.credentials.key_for(profile_name, provider),
            )?
        };
        spec.env
            .insert(OsString::from("OPENAI_BASE_URL"), OsString::from(base_url));
        spec.env
            .insert(OsString::from("OPENAI_API_KEY"), OsString::from(key));
    }

    if !has_model_override(passthrough) {
        let model = overrides.model.as_deref().unwrap_or(&provider.model);
        spec.args.push(OsString::from("--model"));
        spec.args.push(OsString::from(model));
    }

    spec.args.extend_from_slice(passthrough);
    Ok(())
}

/// Pushes `--auth-type <value>` onto `spec.args` unless the user already
/// passed their own `--auth-type` in `passthrough`.
fn push_auth_type(spec: &mut LaunchSpec, passthrough: &[OsString], value: &str) {
    if !has_option(passthrough, "--auth-type", "--auth-type") {
        spec.args.push(OsString::from("--auth-type"));
        spec.args.push(OsString::from(value));
    }
}

/// Wires the bundled Codex bridge (listening on `base_url`) into Qwen Code
/// as an OpenAI-shaped provider speaking Chat Completions. `build` already
/// copied the user's passthrough into `spec.args` verbatim on the codex
/// branch (the bridge-resolved model is not known until now), so the
/// injected flags are prepended ahead of it instead of pushed. The loopback
/// bridge ignores auth entirely, so `OPENAI_API_KEY` is a fixed placeholder.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    if !has_option(&spec.args, "--auth-type", "--auth-type") {
        prepend_args(spec, &["--auth-type", "openai"]);
    }
    if !has_model_override(&spec.args) {
        prepend_args(spec, &["--model", &plan.model]);
    }
    spec.env.insert(
        OsString::from("OPENAI_BASE_URL"),
        OsString::from(format!("{base_url}/v1")),
    );
    spec.env
        .insert(OsString::from("OPENAI_API_KEY"), OsString::from("alc"));
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

    // (1) openai kind with a stored key: openai auth type, model flag, and
    // the OpenAI BYOK envs.
    #[test]
    fn openai_kind_uses_openai_auth_type_and_key_env() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openai".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Qwen,
            Some("openai"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            spec.args,
            vec![
                OsString::from("--auth-type"),
                OsString::from("openai"),
                OsString::from("--model"),
                OsString::from("gpt-5.6-terra"),
            ]
        );
        assert_eq!(
            env_value(&spec, "OPENAI_BASE_URL").as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            env_value(&spec, "OPENAI_API_KEY").as_deref(),
            Some("secret")
        );
    }

    // (2) deepseek preset: dual chat+anthropic surface, but chat-first, so
    // anthropic_shaped() must be false and the openai auth type/base URL
    // must be used, not the anthropic one.
    #[test]
    fn deepseek_preset_uses_the_chat_route_not_anthropic() {
        let mut config = Config::default();
        config
            .providers
            .insert("ds".into(), Provider::for_kind(ProviderKind::Deepseek));
        let mut credentials = Credentials::default();
        credentials.api_keys.insert("ds".into(), "secret".into());
        let spec = launch::build(
            &store(config, credentials),
            Agent::Qwen,
            Some("ds"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            spec.args,
            vec![
                OsString::from("--auth-type"),
                OsString::from("openai"),
                OsString::from("--model"),
                OsString::from("deepseek-v4-pro"),
            ]
        );
        assert_eq!(
            env_value(&spec, "OPENAI_BASE_URL").as_deref(),
            Some("https://api.deepseek.com/v1")
        );
        assert!(env_value(&spec, "ANTHROPIC_API_KEY").is_none());
    }

    // (3) anthropic kind with a stored key.
    #[test]
    fn anthropic_kind_uses_anthropic_auth_type_and_key_env() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("anthropic".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Qwen,
            Some("anthropic"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            spec.args,
            vec![
                OsString::from("--auth-type"),
                OsString::from("anthropic"),
                OsString::from("--model"),
                OsString::from("sonnet"),
            ]
        );
        assert_eq!(
            env_value(&spec, "ANTHROPIC_BASE_URL").as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(
            env_value(&spec, "ANTHROPIC_API_KEY").as_deref(),
            Some("secret")
        );
    }

    // (4) google kind: gemini auth type and GEMINI_API_KEY, no base URL env.
    #[test]
    fn google_kind_uses_gemini_auth_type_and_key_env() {
        let mut config = Config::default();
        config
            .providers
            .insert("gg".into(), Provider::for_kind(ProviderKind::Google));
        let mut credentials = Credentials::default();
        credentials.api_keys.insert("gg".into(), "secret".into());
        let spec = launch::build(
            &store(config, credentials),
            Agent::Qwen,
            Some("gg"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            spec.args,
            vec![
                OsString::from("--auth-type"),
                OsString::from("gemini"),
                OsString::from("--model"),
                OsString::from("gemini-3.7-flash"),
            ]
        );
        assert_eq!(
            env_value(&spec, "GEMINI_API_KEY").as_deref(),
            Some("secret")
        );
    }

    // (5) explicit user passthrough --auth-type and --model both win; alc
    // must not inject its own (no duplicates).
    #[test]
    fn explicit_auth_type_and_model_suppress_injection() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openai".into(), "secret".into());
        let passthrough = [
            OsString::from("--auth-type"),
            OsString::from("custom"),
            OsString::from("--model"),
            OsString::from("x"),
        ];
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Qwen,
            Some("openai"),
            &passthrough,
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            spec.args,
            vec![
                OsString::from("--auth-type"),
                OsString::from("custom"),
                OsString::from("--model"),
                OsString::from("x"),
            ]
        );
    }

    // (6) ollama: AuthStyle::None must not require a stored/env key; the
    // placeholder "alc" is used for OPENAI_API_KEY instead.
    #[test]
    fn ollama_uses_alc_placeholder_key_for_auth_style_none() {
        let spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Qwen,
            Some("ollama"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(env_value(&spec, "OPENAI_API_KEY").as_deref(), Some("alc"));
        assert_eq!(
            env_value(&spec, "OPENAI_BASE_URL").as_deref(),
            Some("http://localhost:11434")
        );
    }

    // (7) codex bridge: BridgeApi::Chat, and apply_bridge prepends its flags
    // ahead of the passthrough `build` already copied verbatim, in a fixed
    // deterministic order (model flag ends up first because it is prepended
    // last), and sets both envs.
    #[test]
    fn codex_bridge_uses_chat_api_and_prepends_flags_before_passthrough() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-terra".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let passthrough = [OsString::from("some-positional")];
        let mut spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Qwen,
            Some("codex"),
            &passthrough,
            &overrides,
        )
        .unwrap();
        let plan = spec.bridge.clone().expect("codex bridge plan");
        assert_eq!(plan.api, BridgeApi::Chat);
        assert_eq!(plan.model, "gpt-5.6-terra");
        assert_eq!(spec.args, vec![OsString::from("some-positional")]);

        super::apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();

        assert_eq!(
            spec.args,
            vec![
                OsString::from("--model"),
                OsString::from("gpt-5.6-terra"),
                OsString::from("--auth-type"),
                OsString::from("openai"),
                OsString::from("some-positional"),
            ]
        );
        assert_eq!(
            env_value(&spec, "OPENAI_BASE_URL").as_deref(),
            Some("http://127.0.0.1:9/v1")
        );
        assert_eq!(env_value(&spec, "OPENAI_API_KEY").as_deref(), Some("alc"));
    }
}
