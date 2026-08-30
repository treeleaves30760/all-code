use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::config::{AuthStyle, Provider, ProviderKind, ReasoningEffort, Store};
use crate::launch::{
    BridgeApi, BridgePlan, FileSetup, LaunchOverrides, LaunchSpec, has_model_override, has_option,
    key_or_error, openai_style_base_url, resolve_codex_effort, resolve_codex_model,
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
        let model = overrides
            .model
            .clone()
            .map_or_else(|| resolve_codex_model(provider), Ok)?;
        let effort = overrides
            .reasoning_effort
            .or(provider.reasoning_effort)
            .or(resolve_codex_effort(provider)?);
        spec.bridge = Some(BridgePlan {
            model: model.clone(),
            effort,
            context_window: overrides.context_window,
            options: overrides.model_options.clone(),
            api: BridgeApi::Responses,
        });
        inject_launch_args(spec, "alc-codex", &model, effort, passthrough);
        return Ok(());
    }

    let key = store.credentials.key_for(profile_name, provider);
    if provider.kind == ProviderKind::Anthropic && key.is_none() {
        // No stored key: rely on Pi's own `/login` (subscription) credentials
        // rather than writing a models.json entry that would go unused.
        let model = overrides.model.as_deref().unwrap_or(&provider.model);
        let effort = overrides.reasoning_effort.or(provider.reasoning_effort);
        inject_launch_args(spec, "anthropic", model, effort, passthrough);
        return Ok(());
    }

    let (api, base_url) = provider_entry_url(profile_name, provider)?;
    let model = overrides.model.as_deref().unwrap_or(&provider.model);
    let effort = overrides.reasoning_effort.or(provider.reasoning_effort);

    let api_key_value = if provider.auth == AuthStyle::None {
        Value::String("alc".to_owned())
    } else {
        let key = key_or_error(profile_name, provider, key)?;
        spec.env
            .insert(OsString::from("ALC_PROVIDER_API_KEY"), OsString::from(key));
        Value::String("$ALC_PROVIDER_API_KEY".to_owned())
    };

    let value = json!({
        "baseUrl": base_url,
        "api": api,
        "apiKey": api_key_value,
        "models": [{
            "id": model,
            "name": model,
            "reasoning": effort.is_some(),
        }],
    });
    let provider_id = format!("alc-{profile_name}");
    spec.file_setup.push(FileSetup::UpsertJson {
        path: agent_dir().join("models.json"),
        pointer: "providers",
        key: provider_id.clone(),
        value,
    });

    inject_launch_args(spec, &provider_id, model, effort, passthrough);
    Ok(())
}

/// Wires the bundled Codex bridge (listening on `base_url`) into Pi's
/// `models.json` as an `alc-codex` OpenAI-Responses provider, pinning every
/// catalog model's context window so Pi's own picker can offer them.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    let models: Vec<Value> = if plan.options.is_empty() {
        vec![bridge_model_entry(
            &plan.model,
            &plan.model,
            plan.context_window,
        )]
    } else {
        plan.options
            .iter()
            .map(|model| bridge_model_entry(&model.id, &model.name, Some(model.context_window)))
            .collect()
    };

    let value = json!({
        "baseUrl": format!("{base_url}/v1"),
        "api": "openai-responses",
        "apiKey": "alc",
        "models": models,
    });
    spec.file_setup.push(FileSetup::UpsertJson {
        path: agent_dir().join("models.json"),
        pointer: "providers",
        key: "alc-codex".to_owned(),
        value,
    });
    Ok(())
}

fn bridge_model_entry(id: &str, name: &str, context_window: Option<u64>) -> Value {
    let mut entry = json!({
        "id": id,
        "name": name,
        "reasoning": true,
    });
    if let Some(context_window) = context_window {
        entry["contextWindow"] = json!(context_window);
    }
    entry
}

/// Appends `--provider <id> --model <model>` and, when an effort is
/// configured, `--thinking <effort>` ahead of `passthrough` — each guarded so
/// an explicit passthrough flag is never duplicated.
fn inject_launch_args(
    spec: &mut LaunchSpec,
    provider_id: &str,
    model: &str,
    effort: Option<ReasoningEffort>,
    passthrough: &[OsString],
) {
    if !has_option(passthrough, "--provider", "--provider") {
        spec.args
            .extend([OsString::from("--provider"), OsString::from(provider_id)]);
    }
    if !has_model_override(passthrough) {
        spec.args
            .extend([OsString::from("--model"), OsString::from(model)]);
    }
    if !has_option(passthrough, "--thinking", "--thinking")
        && let Some(effort) = effort
    {
        spec.args.extend([
            OsString::from("--thinking"),
            OsString::from(effort.as_str()),
        ]);
    }
    spec.args.extend_from_slice(passthrough);
}

/// The `api` id and base URL for a non-bridge provider's `models.json` entry:
/// `Openai` kind speaks OpenAI Responses; any other chat-capable provider
/// (Ollama's `/v1` suffix included) speaks OpenAI Completions; everything
/// else falls back to Anthropic Messages.
fn provider_entry_url(profile_name: &str, provider: &Provider) -> Result<(&'static str, String)> {
    if provider.kind == ProviderKind::Openai {
        let base_url = provider
            .effective_base_url()
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?;
        Ok(("openai-responses", base_url.to_owned()))
    } else if provider.speaks_chat() {
        let base_url = openai_style_base_url(provider)
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?;
        Ok(("openai-completions", base_url))
    } else {
        let base_url = provider
            .effective_anthropic_base_url()
            .with_context(|| format!("provider '{profile_name}' needs an Anthropic base URL"))?;
        Ok(("anthropic-messages", base_url.to_owned()))
    }
}

/// `<PI_CODING_AGENT_DIR>` when set, else `<home>/.pi/agent`. Pure path
/// arithmetic; never touches disk itself.
pub(crate) fn agent_dir() -> PathBuf {
    env::var_os("PI_CODING_AGENT_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::launch::home_dir()
                .unwrap_or_default()
                .join(".pi/agent")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    use crate::config::{Agent, Config, Credentials};
    use crate::launch;
    use crate::model_catalog::ModelCatalog;

    fn store(config: Config, credentials: Credentials) -> Store {
        Store {
            dir: PathBuf::from("test"),
            config,
            credentials,
        }
    }

    fn option_value(args: &[OsString], name: &str) -> Option<String> {
        let index = args.iter().position(|arg| arg.to_string_lossy() == name)?;
        args.get(index + 1)
            .map(|value| value.to_string_lossy().into_owned())
    }

    /// Panics unless `spec` queued exactly one `UpsertJson` entry; returns its
    /// `(path, pointer, key, value)` for the caller to assert against.
    fn only_upsert(spec: &LaunchSpec) -> (PathBuf, &'static str, String, Value) {
        match spec.file_setup.as_slice() {
            [
                FileSetup::UpsertJson {
                    path,
                    pointer,
                    key,
                    value,
                },
            ] => (path.clone(), pointer, key.clone(), value.clone()),
            other => panic!("expected exactly one UpsertJson entry, got {other:?}"),
        }
    }

    // (a) openrouter profile with a saved key.
    #[test]
    fn openrouter_upserts_models_json_and_injects_provider_args() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Pi,
            Some("openrouter"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            option_value(&spec.args, "--provider").as_deref(),
            Some("alc-openrouter")
        );
        assert_eq!(
            option_value(&spec.args, "--model").as_deref(),
            Some("anthropic/claude-sonnet-4.6")
        );
        assert!(
            option_value(&spec.args, "--thinking").is_none(),
            "openrouter has no configured reasoning effort by default"
        );
        assert_eq!(
            spec.env[OsStr::new("ALC_PROVIDER_API_KEY")],
            OsString::from("secret")
        );

        let (path, pointer, key, value) = only_upsert(&spec);
        assert_eq!(
            path.file_name().and_then(OsStr::to_str),
            Some("models.json")
        );
        assert_eq!(pointer, "providers");
        assert_eq!(key, "alc-openrouter");
        assert_eq!(
            value,
            json!({
                "baseUrl": "https://openrouter.ai/api/v1",
                "api": "openai-completions",
                "apiKey": "$ALC_PROVIDER_API_KEY",
                "models": [{
                    "id": "anthropic/claude-sonnet-4.6",
                    "name": "anthropic/claude-sonnet-4.6",
                    "reasoning": false,
                }],
            })
        );
    }

    // (b) ollama profile: openai-completions with a /v1 suffix and a literal key.
    #[test]
    fn ollama_uses_openai_completions_with_v1_suffix_and_literal_key() {
        let spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Pi,
            Some("ollama"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            option_value(&spec.args, "--provider").as_deref(),
            Some("alc-ollama")
        );
        assert_eq!(
            option_value(&spec.args, "--model").as_deref(),
            Some("qwen3-coder")
        );
        assert!(
            !spec.env.contains_key(OsStr::new("ALC_PROVIDER_API_KEY")),
            "AuthStyle::None must not require or set an API key"
        );

        let (_, _, _, value) = only_upsert(&spec);
        assert_eq!(value["api"], json!("openai-completions"));
        assert_eq!(value["baseUrl"], json!("http://localhost:11434/v1"));
        assert_eq!(value["apiKey"], json!("alc"));
        assert_eq!(value["models"][0]["reasoning"], json!(false));
    }

    // Openai kind maps to openai-responses, and a configured effort both
    // marks the model "reasoning": true and injects --thinking.
    #[test]
    fn openai_kind_uses_openai_responses_and_injects_thinking_when_effort_is_configured() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openai".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Pi,
            Some("openai"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        let (_, _, _, value) = only_upsert(&spec);
        assert_eq!(value["api"], json!("openai-responses"));
        assert_eq!(value["baseUrl"], json!("https://api.openai.com/v1"));
        assert_eq!(value["apiKey"], json!("$ALC_PROVIDER_API_KEY"));
        // The `openai` preset ships with a default reasoning effort.
        assert_eq!(value["models"][0]["reasoning"], json!(true));
        assert_eq!(
            option_value(&spec.args, "--thinking").as_deref(),
            Some("medium")
        );
    }

    // Anthropic kind WITH a stored key takes the general path: it falls
    // through both the Openai and speaks_chat() branches to anthropic-messages.
    #[test]
    fn anthropic_with_a_stored_key_uses_anthropic_messages() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("anthropic".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Pi,
            Some("anthropic"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert_eq!(
            spec.env[OsStr::new("ALC_PROVIDER_API_KEY")],
            OsString::from("secret")
        );
        let (_, _, key, value) = only_upsert(&spec);
        assert_eq!(key, "alc-anthropic");
        assert_eq!(value["api"], json!("anthropic-messages"));
        assert_eq!(value["baseUrl"], json!("https://api.anthropic.com"));
        assert_eq!(value["apiKey"], json!("$ALC_PROVIDER_API_KEY"));
        assert_eq!(
            option_value(&spec.args, "--provider").as_deref(),
            Some("alc-anthropic")
        );
    }

    // Edge case: anthropic kind with no stored key relies on Pi's own /login.
    #[test]
    fn anthropic_without_a_stored_key_skips_models_json_and_uses_native_login() {
        let spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Pi,
            Some("anthropic"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert!(
            spec.file_setup.is_empty(),
            "no stored key means pi's own /login should be used, not a models.json entry"
        );
        assert!(!spec.env.contains_key(OsStr::new("ALC_PROVIDER_API_KEY")));
        assert_eq!(
            option_value(&spec.args, "--provider").as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            option_value(&spec.args, "--model").as_deref(),
            Some("sonnet")
        );
    }

    // Explicit passthrough flags win; alc must not inject a duplicate.
    #[test]
    fn explicit_passthrough_flags_are_not_duplicated() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openai".into(), "secret".into());
        let passthrough = [
            OsString::from("--provider"),
            OsString::from("custom"),
            OsString::from("--model"),
            OsString::from("o1"),
            OsString::from("--thinking"),
            OsString::from("low"),
        ];
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Pi,
            Some("openai"),
            &passthrough,
            &LaunchOverrides::default(),
        )
        .unwrap();

        let rendered = spec
            .args
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(rendered, "--provider custom --model o1 --thinking low");
    }

    // (c) codex bridge: apply_bridge upserts alc-codex with the full catalog.
    #[test]
    fn codex_bridge_upserts_all_catalog_models_with_reasoning_and_context_window() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-terra".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let mut spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Pi,
            Some("codex"),
            &[],
            &overrides,
        )
        .unwrap();
        let plan = spec.bridge.clone().expect("codex bridge plan");
        assert_eq!(plan.api, BridgeApi::Responses);
        assert_eq!(plan.model, "gpt-5.6-terra");

        // build() injects the launch args up front (port-independent).
        assert_eq!(
            option_value(&spec.args, "--provider").as_deref(),
            Some("alc-codex")
        );
        assert_eq!(
            option_value(&spec.args, "--model").as_deref(),
            Some("gpt-5.6-terra")
        );
        assert_eq!(
            option_value(&spec.args, "--thinking").as_deref(),
            Some("medium")
        );

        super::apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();

        let (_, pointer, key, value) = only_upsert(&spec);
        assert_eq!(pointer, "providers");
        assert_eq!(key, "alc-codex");
        assert_eq!(value["baseUrl"], json!("http://127.0.0.1:9/v1"));
        assert_eq!(value["api"], json!("openai-responses"));
        assert_eq!(value["apiKey"], json!("alc"));

        let models = value["models"].as_array().expect("models array");
        assert_eq!(models.len(), 3);
        for model in models {
            assert_eq!(model["reasoning"], json!(true));
            assert!(model["contextWindow"].is_u64());
        }
        let ids: Vec<_> = models
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]);
    }

    // (d) agent_dir() honors the override / falls back correctly, and
    // building a spec (the dry-run path) never performs file I/O: writes
    // only happen in `launch::execute`, which this test never calls.
    //
    // This is the only test in the suite that reads or writes
    // PI_CODING_AGENT_DIR, so it never races another test over that variable.
    #[test]
    fn agent_dir_honors_override_and_building_a_spec_never_writes() {
        // SAFETY: sole owner of PI_CODING_AGENT_DIR across the test suite.
        unsafe { env::remove_var("PI_CODING_AGENT_DIR") };
        assert_eq!(
            agent_dir(),
            crate::launch::home_dir()
                .unwrap_or_default()
                .join(".pi/agent"),
            "unset falls back to home/.pi/agent"
        );

        let temp = tempfile::tempdir().unwrap();
        // SAFETY: same sole-owner justification as above.
        unsafe { env::set_var("PI_CODING_AGENT_DIR", temp.path()) };
        assert_eq!(agent_dir(), temp.path(), "the override is honored verbatim");

        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Pi,
            Some("openrouter"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();
        assert!(
            !spec.file_setup.is_empty(),
            "sanity: build queued a models.json upsert"
        );
        assert!(
            !temp.path().join("models.json").exists(),
            "building a spec must never perform file I/O; writes happen only in execute()"
        );

        // SAFETY: same sole-owner justification as above.
        unsafe { env::remove_var("PI_CODING_AGENT_DIR") };
    }
}
