use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::config::{Agent, AuthStyle, Protocol, Provider, ProviderKind, ReasoningEffort, Store};
use crate::model_catalog::ModelInfo;

pub const CLAUDE_CODEX_HELPER_VERSION: &str = "0.3.1";

#[derive(Debug, Clone, Default)]
pub struct LaunchOverrides {
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub context_window: Option<u64>,
    /// Models the agent's own picker should offer for this session.
    pub model_options: Vec<ModelInfo>,
}

/// What the bundled Codex adapter needs for a Claude Code session. The model
/// is only the starting point: Claude Code switches models and reasoning
/// effort per request, so neither is pinned on the adapter.
#[derive(Debug, Clone)]
pub struct CodexPlan {
    pub model: String,
    pub context_window: Option<u64>,
    /// Ordered from the most capable model to the cheapest one.
    pub options: Vec<ModelInfo>,
}

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub env_remove: Vec<OsString>,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub agent: Agent,
    pub codex_plan: Option<CodexPlan>,
}

impl LaunchSpec {
    pub fn redacted_command(&self) -> String {
        let mut parts = Vec::new();
        for (name, value) in &self.env {
            let rendered = if is_secret_env(name) {
                "<redacted>".to_owned()
            } else {
                shell_quote(value)
            };
            parts.push(format!("{}={rendered}", name.to_string_lossy()));
        }
        parts.push(shell_quote(&self.program));
        parts.extend(self.args.iter().map(|value| shell_quote(value.as_os_str())));
        parts.join(" ")
    }
}

pub fn build(
    store: &Store,
    agent: Agent,
    requested_provider: Option<&str>,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<LaunchSpec> {
    let (profile_name, provider) = store.config.resolve(agent, requested_provider)?;
    let mut spec = LaunchSpec {
        program: OsString::from(agent.as_str()),
        args: Vec::new(),
        env: BTreeMap::new(),
        env_remove: Vec::new(),
        provider_name: profile_name.to_owned(),
        provider_kind: provider.kind,
        agent,
        codex_plan: None,
    };

    if let Some(override_path) = agent_binary_override(agent) {
        spec.program = override_path;
    }

    match agent {
        Agent::Claude => build_claude(
            &mut spec,
            store,
            profile_name,
            provider,
            passthrough,
            overrides,
        )?,
        Agent::Codex => build_codex(
            &mut spec,
            store,
            profile_name,
            provider,
            passthrough,
            overrides,
        )?,
        Agent::Opencode => build_opencode(
            &mut spec,
            store,
            profile_name,
            provider,
            passthrough,
            overrides,
        )?,
    }
    Ok(spec)
}

pub fn execute(mut spec: LaunchSpec) -> Result<u8> {
    let _proxy = if let Some(plan) = spec.codex_plan.clone() {
        let proxy = CodexProxy::start()?;
        configure_claude_proxy_env(&mut spec, proxy.base_url(), &plan);
        Some(proxy)
    } else {
        None
    };

    let program = resolve_program(&spec.program, spec.agent)?;
    let mut command = Command::new(&program);
    command
        .args(&spec.args)
        .envs(&spec.env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for name in &spec.env_remove {
        command.env_remove(name);
    }

    let status = command
        .status()
        .with_context(|| format!("failed to launch {}", program.display()))?;
    Ok(exit_code(status))
}

fn build_claude(
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
        spec.codex_plan = Some(CodexPlan {
            model: model.clone(),
            context_window: overrides.context_window,
            options: overrides.model_options.clone(),
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
    if !provider.protocol.supports_anthropic() {
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

fn build_codex(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if provider.kind == ProviderKind::Codex {
        if !has_option(passthrough, "--profile", "-p")
            && let Some(profile) = provider
                .codex_profile
                .as_deref()
                .filter(|value| !value.is_empty())
        {
            spec.args
                .extend([OsString::from("--profile"), OsString::from(profile)]);
        }
        let model = overrides.model.as_deref().unwrap_or(&provider.model);
        if !model.is_empty() && !has_model_override(passthrough) {
            spec.args
                .extend([OsString::from("--model"), OsString::from(model)]);
        }
        if !has_effort_override(passthrough)
            && let Some(effort) = overrides.reasoning_effort.or(provider.reasoning_effort)
        {
            push_codex_config(
                &mut spec.args,
                "model_reasoning_effort",
                toml_string(effort.as_str()),
            );
        }
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    if provider.kind == ProviderKind::Ollama {
        if !has_option(passthrough, "--oss", "--oss") {
            spec.args.push(OsString::from("--oss"));
        }
        if !has_option(passthrough, "--local-provider", "--local-provider") {
            spec.args
                .extend([OsString::from("--local-provider"), OsString::from("ollama")]);
        }
        if !has_model_override(passthrough) {
            spec.args.extend([
                OsString::from("--model"),
                OsString::from(overrides.model.as_deref().unwrap_or(&provider.model)),
            ]);
        }
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    if !provider.protocol.supports_responses() {
        bail!(
            "provider '{profile_name}' speaks {}, but Codex requires the OpenAI Responses API",
            provider.protocol
        );
    }
    let base_url = provider
        .effective_base_url()
        .with_context(|| format!("provider '{profile_name}' needs a base URL"))?;
    let provider_id = codex_provider_id(profile_name);

    if !has_model_override(passthrough) {
        spec.args.extend([
            OsString::from("--model"),
            OsString::from(overrides.model.as_deref().unwrap_or(&provider.model)),
        ]);
    }
    if !has_effort_override(passthrough)
        && let Some(effort) = overrides.reasoning_effort.or(provider.reasoning_effort)
    {
        push_codex_config(
            &mut spec.args,
            "model_reasoning_effort",
            toml_string(effort.as_str()),
        );
    }
    push_codex_config(&mut spec.args, "model_provider", toml_string(&provider_id));
    push_codex_config(
        &mut spec.args,
        &format!("model_providers.{provider_id}.name"),
        toml_string(&format!("alc: {profile_name}")),
    );
    push_codex_config(
        &mut spec.args,
        &format!("model_providers.{provider_id}.base_url"),
        toml_string(base_url),
    );
    push_codex_config(
        &mut spec.args,
        &format!("model_providers.{provider_id}.wire_api"),
        toml_string("responses"),
    );
    push_codex_config(
        &mut spec.args,
        &format!("model_providers.{provider_id}.requires_openai_auth"),
        "false".to_owned(),
    );

    if provider.auth != AuthStyle::None {
        let key = key_or_error(
            profile_name,
            provider,
            store.credentials.key_for(profile_name, provider),
        )?;
        let env_name = "ALC_PROVIDER_API_KEY";
        spec.env
            .insert(OsString::from(env_name), OsString::from(key));
        push_codex_config(
            &mut spec.args,
            &format!("model_providers.{provider_id}.env_key"),
            toml_string(env_name),
        );
    }
    spec.args.extend_from_slice(passthrough);
    Ok(())
}

fn build_opencode(
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

fn configure_claude_proxy_env(spec: &mut LaunchSpec, base_url: String, plan: &CodexPlan) {
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

pub(crate) fn resolve_codex_model(provider: &Provider) -> Result<String> {
    if !provider.model.trim().is_empty() {
        return Ok(normalize_codex_model(&provider.model));
    }

    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".codex")));
    if let Some(home) = codex_home {
        let profile_path = provider
            .codex_profile
            .as_deref()
            .filter(|profile| !profile.is_empty())
            .map(|profile| home.join(format!("{profile}.config.toml")));
        for path in profile_path.into_iter().chain([home.join("config.toml")]) {
            if let Some(model) = read_codex_preference(&path, "model")? {
                return Ok(normalize_codex_model(&model));
            }
        }
    }
    Ok("gpt-5.6-terra".to_owned())
}

pub(crate) fn normalize_codex_model(model: &str) -> String {
    // The OpenAI API exposes gpt-5.6 as a Sol alias, while the pinned bridge
    // accepts the explicit family member names.
    if model == "gpt-5.6" {
        "gpt-5.6-sol".to_owned()
    } else {
        model.to_owned()
    }
}

pub(crate) fn resolve_codex_effort(provider: &Provider) -> Result<Option<ReasoningEffort>> {
    if let Some(effort) = provider.reasoning_effort {
        return Ok(Some(effort));
    }

    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".codex")));
    if let Some(home) = codex_home {
        let profile_path = provider
            .codex_profile
            .as_deref()
            .filter(|profile| !profile.is_empty())
            .map(|profile| home.join(format!("{profile}.config.toml")));
        for path in profile_path.into_iter().chain([home.join("config.toml")]) {
            if let Some(value) = read_codex_preference(&path, "model_reasoning_effort")? {
                let effort = match value.as_str() {
                    // The helper currently tops out at max, while newer Codex
                    // builds may persist the additional ultra tier.
                    "ultra" => ReasoningEffort::Max,
                    // alc intentionally presents low as its simplest choice.
                    "none" => ReasoningEffort::Low,
                    _ => value.parse().with_context(|| {
                        format!(
                            "invalid model_reasoning_effort in {}; expected low, medium, high, xhigh, or max",
                            path.display()
                        )
                    })?,
                };
                return Ok(Some(effort));
            }
        }
    }
    Ok(None)
}

fn read_codex_preference(path: &Path, key: &str) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Codex config {}", path.display()))?;
    let document: toml::Value = toml::from_str(&text)
        .with_context(|| format!("failed to parse Codex config {}", path.display()))?;
    Ok(document
        .get(key)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned))
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let name = "USERPROFILE";
    #[cfg(not(windows))]
    let name = "HOME";
    env::var_os(name).map(PathBuf::from)
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

fn opencode_base_url(provider: &Provider) -> Option<String> {
    let base = provider.effective_base_url()?.trim_end_matches('/');
    if provider.kind == ProviderKind::Ollama && !base.ends_with("/v1") {
        Some(format!("{base}/v1"))
    } else {
        Some(base.to_owned())
    }
}

fn key_or_error(profile_name: &str, provider: &Provider, key: Option<String>) -> Result<String> {
    if let Some(key) = key.filter(|value| !value.is_empty()) {
        return Ok(key);
    }
    missing_key(profile_name, provider)?;
    unreachable!()
}

fn missing_key(profile_name: &str, provider: &Provider) -> Result<()> {
    let hint = provider
        .api_key_env
        .as_deref()
        .map(|name| format!("set {name} or "))
        .unwrap_or_default();
    bail!("provider '{profile_name}' has no API key; {hint}run `alc config` to save one")
}

fn push_codex_config(args: &mut Vec<OsString>, key: &str, value: String) {
    args.push(OsString::from("--config"));
    args.push(OsString::from(format!("{key}={value}")));
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

fn codex_provider_id(profile: &str) -> String {
    format!("alc_{}", profile.replace('-', "_"))
}

fn has_model_override(args: &[OsString]) -> bool {
    args.iter().any(|arg| {
        let value = arg.to_string_lossy();
        matches!(value.as_ref(), "--model" | "-m")
            || value.starts_with("--model=")
            || value.starts_with("-m=")
    })
}

fn has_effort_override(args: &[OsString]) -> bool {
    args.iter().enumerate().any(|(index, arg)| {
        let value = arg.to_string_lossy();
        value.starts_with("--config=model_reasoning_effort=")
            || value.starts_with("-c=model_reasoning_effort=")
            || (matches!(value.as_ref(), "--config" | "-c")
                && args.get(index + 1).is_some_and(|next| {
                    next.to_string_lossy()
                        .starts_with("model_reasoning_effort=")
                }))
    })
}

fn has_option(args: &[OsString], long: &str, short: &str) -> bool {
    args.iter().any(|arg| {
        let value = arg.to_string_lossy();
        value == long || value == short || value.starts_with(&format!("{long}="))
    })
}

fn agent_binary_override(agent: Agent) -> Option<OsString> {
    let name = match agent {
        Agent::Claude => "ALC_CLAUDE_BIN",
        Agent::Codex => "ALC_CODEX_BIN",
        Agent::Opencode => "ALC_OPENCODE_BIN",
    };
    env::var_os(name).filter(|value| !value.is_empty())
}

fn resolve_program(program: &OsStr, agent: Agent) -> Result<PathBuf> {
    let as_path = PathBuf::from(program);
    if as_path.components().count() > 1 || as_path.is_absolute() {
        if as_path.exists() {
            return Ok(as_path);
        }
        bail!(
            "configured {agent} binary does not exist: {}",
            as_path.display()
        );
    }
    which::which(program).with_context(|| {
        format!(
            "'{agent}' is not installed or not on PATH; install it first, then retry `alc {agent}`"
        )
    })
}

fn exit_code(status: ExitStatus) -> u8 {
    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

fn is_secret_env(name: &OsStr) -> bool {
    let upper = name.to_string_lossy().to_ascii_uppercase();
    upper.contains("API_KEY") || upper.contains("AUTH_TOKEN") || upper.ends_with("_TOKEN")
}

fn shell_quote(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    if value.is_empty() {
        return "''".to_owned();
    }
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-._/:=@".contains(character))
    {
        value.into_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

struct CodexProxy {
    child: Child,
    port: u16,
}

impl CodexProxy {
    fn start() -> Result<Self> {
        let helper = find_helper()?;
        let auth_file = codex_auth_file()?;
        if !auth_file.is_file() {
            bail!(
                "Codex credentials were not found at {}; run `codex login` and retry",
                auth_file.display()
            );
        }
        let listener = TcpListener::bind("127.0.0.1:0")
            .context("failed to reserve a loopback port for the Codex adapter")?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let child = Command::new(&helper)
            .arg("serve")
            .env("PORT", port.to_string())
            .env("CCP_LOG_STDERR", "0")
            // The helper's fallback does not use USERPROFILE on Windows, so
            // pass the same Codex home resolution used by the official CLI.
            .env("CCP_CODEX_AUTH_FILE", auth_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start {}", helper.display()))?;
        let mut proxy = Self { child, port };
        proxy.wait_until_ready()?;
        Ok(proxy)
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn wait_until_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let address = SocketAddr::from(([127, 0, 0, 1], self.port));
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                bail!(
                    "Codex adapter exited before it became ready (status {status}); run `codex login` and `alc doctor`"
                );
            }
            if health_check(address) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!(
            "Codex adapter did not become ready on 127.0.0.1:{} within 10 seconds",
            self.port
        )
    }
}

fn codex_auth_file() -> Result<PathBuf> {
    resolve_codex_auth_file(
        env::var_os("CCP_CODEX_AUTH_FILE"),
        env::var_os("CODEX_HOME"),
        home_dir(),
    )
}

fn resolve_codex_auth_file(
    explicit: Option<OsString>,
    codex_home: Option<OsString>,
    user_home: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = explicit.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(home) = codex_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home).join("auth.json"));
    }
    user_home
        .map(|home| home.join(".codex/auth.json"))
        .context("could not resolve the Codex auth path; set CODEX_HOME")
}

impl Drop for CodexProxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find_helper() -> Result<PathBuf> {
    if let Some(path) = env::var_os("ALC_CLAUDE_CODEX_BIN").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
        bail!(
            "ALC_CLAUDE_CODEX_BIN points to a missing file: {}",
            path.display()
        );
    }

    let file_name = if cfg!(windows) {
        "claude-codex.exe"
    } else {
        "claude-codex"
    };
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let sibling = parent.join(file_name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    if let Ok(path) = which::which("claude-codex") {
        return Ok(path);
    }
    bail!(
        "the bundled claude-codex {CLAUDE_CODEX_HELPER_VERSION} helper is missing; reinstall alc with the one-line installer or install `claude-codex` on PATH"
    )
}

fn health_check(address: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(200)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    let request = b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(request).is_err() {
        return false;
    }
    let mut response = [0_u8; 128];
    let Ok(read) = stream.read(&mut response) else {
        return false;
    };
    String::from_utf8_lossy(&response[..read]).contains(" 200 ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Credentials};
    use crate::model_catalog::ModelCatalog;

    fn store(config: Config, credentials: Credentials) -> Store {
        Store {
            dir: PathBuf::from("test"),
            config,
            credentials,
        }
    }

    #[test]
    fn codex_native_uses_profile_before_passthrough() {
        let mut config = Config::default();
        config.providers.get_mut("codex").unwrap().codex_profile = Some("work".into());
        let spec = build(
            &store(config, Credentials::default()),
            Agent::Codex,
            Some("codex"),
            &[OsString::from("exec"), OsString::from("hello")],
            &LaunchOverrides::default(),
        )
        .unwrap();
        assert_eq!(
            spec.args,
            ["--profile", "work", "exec", "hello"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }

    fn option_value(args: &[OsString], name: &str) -> Option<String> {
        let index = args.iter().position(|arg| arg.to_string_lossy() == name)?;
        args.get(index + 1)
            .map(|value| value.to_string_lossy().into_owned())
    }

    #[test]
    fn codex_to_claude_lists_every_model_in_the_claude_picker() {
        let catalog = ModelCatalog::built_in();
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-terra".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
            model_options: catalog.models.clone(),
            ..LaunchOverrides::default()
        };
        let spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Claude,
            Some("codex"),
            &[],
            &overrides,
        )
        .unwrap();

        let settings = option_value(&spec.args, "--settings").expect("--settings is injected");
        let parsed: Value = serde_json::from_str(&settings).expect("valid settings JSON");
        let picker = &parsed["modelPicker"];
        let rows = picker["options"]
            .as_array()
            .expect("modelPicker options")
            .clone();
        let ids: Vec<_> = rows
            .iter()
            .map(|row| row["model"].as_str().expect("model id"))
            .collect();
        assert_eq!(ids, ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]);
        assert_eq!(
            rows[0]["label"].as_str(),
            Some("GPT-5.6 Sol"),
            "the most capable model is listed first"
        );
        assert_eq!(
            picker["replaceBuiltInOptions"],
            json!(true),
            "the Claude lineup cannot be served through the Codex adapter"
        );
        assert!(
            rows.iter().all(|row| row.get("capabilities").is_none()),
            "the setting schema only accepts model, label, and description"
        );
    }

    #[test]
    fn codex_to_claude_starts_claude_on_the_configured_default() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-sol".into()),
            reasoning_effort: Some(ReasoningEffort::Max),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Claude,
            Some("codex"),
            &[],
            &overrides,
        )
        .unwrap();

        assert_eq!(
            option_value(&spec.args, "--model").as_deref(),
            Some("gpt-5.6-sol")
        );
        assert_eq!(option_value(&spec.args, "--effort").as_deref(), Some("max"));
    }

    #[test]
    fn explicit_claude_arguments_win_over_the_injected_defaults() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-sol".into()),
            reasoning_effort: Some(ReasoningEffort::Max),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let passthrough = [
            OsString::from("--model"),
            OsString::from("gpt-5.6-luna"),
            OsString::from("--effort"),
            OsString::from("low"),
            OsString::from("--settings"),
            OsString::from("{}"),
        ];
        let spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Claude,
            Some("codex"),
            &passthrough,
            &overrides,
        )
        .unwrap();

        let rendered = spec
            .args
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(rendered, "--model gpt-5.6-luna --effort low --settings {}");
    }

    #[test]
    fn codex_to_claude_hands_the_adapter_a_model_but_never_a_fixed_effort() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-sol".into()),
            reasoning_effort: Some(ReasoningEffort::Max),
            context_window: Some(272_000),
            model_options: ModelCatalog::built_in().models,
        };
        let spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Claude,
            Some("codex"),
            &[],
            &overrides,
        )
        .unwrap();

        let plan = spec.codex_plan.expect("Codex adapter plan");
        assert_eq!(plan.model, "gpt-5.6-sol");
        assert_eq!(plan.context_window, Some(272_000));
        // Claude Code sends the effort with every request, so pinning it here
        // would freeze the in-session effort slider.
        assert!(
            !spec
                .env
                .keys()
                .any(|name| name.to_string_lossy().contains("EFFORT")),
            "no environment variable may pin reasoning effort"
        );
    }

    #[test]
    fn claude_aliases_map_onto_the_codex_model_tiers() {
        let mut spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Claude,
            Some("codex"),
            &[],
            &LaunchOverrides {
                model: Some("gpt-5.6-terra".into()),
                model_options: ModelCatalog::built_in().models,
                ..LaunchOverrides::default()
            },
        )
        .unwrap();
        let plan = spec.codex_plan.clone().expect("Codex adapter plan");

        configure_claude_proxy_env(&mut spec, "http://127.0.0.1:9".to_owned(), &plan);

        let value = |name: &str| spec.env[OsStr::new(name)].to_string_lossy().into_owned();
        assert_eq!(value("ANTHROPIC_MODEL"), "gpt-5.6-terra");
        // The picker's Default row would otherwise resolve to a Claude model
        // that the Codex adapter cannot serve.
        assert_eq!(value("ANTHROPIC_DEFAULT_MODEL"), "gpt-5.6-terra");
        assert_eq!(value("ANTHROPIC_DEFAULT_HAIKU_MODEL"), "gpt-5.6-luna");
        assert_eq!(value("ANTHROPIC_DEFAULT_SONNET_MODEL"), "gpt-5.6-terra");
        assert_eq!(value("ANTHROPIC_DEFAULT_OPUS_MODEL"), "gpt-5.6-sol");
        assert!(
            !spec
                .env
                .contains_key(OsStr::new("CLAUDE_CODE_SUBAGENT_MODEL")),
            "subagents follow the model chosen in the session"
        );
        assert!(
            !spec
                .env
                .contains_key(OsStr::new("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY")),
            "gateway discovery drops non-Claude model IDs, so alc lists them itself"
        );
    }

    #[test]
    fn codex_passthrough_effort_takes_precedence() {
        let spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Codex,
            Some("codex"),
            &[
                OsString::from("--config"),
                OsString::from("model_reasoning_effort=\"high\""),
            ],
            &LaunchOverrides::default(),
        )
        .unwrap();
        let occurrences = spec
            .args
            .iter()
            .filter(|value| {
                value
                    .to_string_lossy()
                    .starts_with("model_reasoning_effort=")
            })
            .count();
        assert_eq!(occurrences, 1);
    }

    #[test]
    fn openrouter_claude_uses_anthropic_skin() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let spec = build(
            &store(Config::default(), credentials),
            Agent::Claude,
            Some("openrouter"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();
        assert_eq!(
            spec.env[OsStr::new("ANTHROPIC_BASE_URL")],
            OsString::from("https://openrouter.ai/api")
        );
        assert_eq!(
            spec.env[OsStr::new("ANTHROPIC_AUTH_TOKEN")],
            OsString::from("secret")
        );
    }

    #[test]
    fn codex_api_provider_uses_responses_config() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openai".into(), "secret".into());
        let spec = build(
            &store(Config::default(), credentials),
            Agent::Codex,
            Some("openai"),
            &[OsString::from("--version")],
            &LaunchOverrides::default(),
        )
        .unwrap();
        let joined = spec
            .args
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("model_providers.alc_openai.wire_api=\"responses\""));
        assert!(!joined.contains("secret"));
        assert!(spec.redacted_command().contains("<redacted>"));
    }

    #[test]
    fn openai_is_rejected_for_claude_without_messages_gateway() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openai".into(), "secret".into());
        let error = build(
            &store(Config::default(), credentials),
            Agent::Claude,
            Some("openai"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Anthropic Messages"));
    }

    #[test]
    fn ollama_opencode_injects_documented_compatible_provider() {
        let spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Opencode,
            Some("ollama"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();
        let inline = spec
            .env
            .get(OsStr::new("OPENCODE_CONFIG_CONTENT"))
            .expect("inline OpenCode provider")
            .to_string_lossy();
        assert!(inline.contains("@ai-sdk/openai-compatible"));
        assert!(inline.contains("http://localhost:11434/v1"));
        assert!(inline.contains("\"model\":\"ollama/qwen3-coder\""));
        assert!(spec.args.is_empty());
    }

    #[test]
    fn opencode_management_subcommands_are_forwarded_without_model_flags() {
        let spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Opencode,
            Some("ollama"),
            &[OsString::from("models"), OsString::from("ollama")],
            &LaunchOverrides::default(),
        )
        .unwrap();
        assert_eq!(spec.args, ["models", "ollama"].map(OsString::from));
    }

    #[test]
    fn codex_auth_path_falls_back_to_the_platform_user_home() {
        let path = resolve_codex_auth_file(None, None, Some(PathBuf::from("user-home"))).unwrap();
        assert_eq!(path, PathBuf::from("user-home/.codex/auth.json"));

        let explicit = resolve_codex_auth_file(
            Some(OsString::from("selected-auth.json")),
            Some(OsString::from("ignored-codex-home")),
            Some(PathBuf::from("ignored-user-home")),
        )
        .unwrap();
        assert_eq!(explicit, PathBuf::from("selected-auth.json"));
    }

    #[test]
    fn generic_gpt_56_alias_maps_to_bridge_supported_sol() {
        assert_eq!(normalize_codex_model("gpt-5.6"), "gpt-5.6-sol");
        assert_eq!(normalize_codex_model("gpt-5.6-terra"), "gpt-5.6-terra");
    }
}
