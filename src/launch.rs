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

use crate::config::{Agent, AuthStyle, Protocol, Provider, ProviderKind, Store};

pub const CLAUDE_CODEX_HELPER_VERSION: &str = "0.3.1";

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub env_remove: Vec<OsString>,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub agent: Agent,
    pub needs_codex_proxy: bool,
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
        needs_codex_proxy: false,
    };

    if let Some(override_path) = agent_binary_override(agent) {
        spec.program = override_path;
    }

    match agent {
        Agent::Claude => build_claude(&mut spec, store, profile_name, provider, passthrough)?,
        Agent::Codex => build_codex(&mut spec, store, profile_name, provider, passthrough)?,
        Agent::Opencode => build_opencode(&mut spec, store, profile_name, provider, passthrough)?,
    }
    Ok(spec)
}

pub fn execute(mut spec: LaunchSpec) -> Result<u8> {
    let _proxy = if spec.needs_codex_proxy {
        let proxy = CodexProxy::start()?;
        let model = spec
            .env
            .get(OsStr::new("ALC_CODEX_MODEL"))
            .cloned()
            .context("internal error: Codex proxy model is missing")?;
        spec.env.remove(OsStr::new("ALC_CODEX_MODEL"));
        configure_claude_proxy_env(&mut spec, proxy.base_url(), &model);
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
) -> Result<()> {
    spec.args.extend_from_slice(passthrough);
    clear_cloud_provider_env(spec);

    if provider.kind == ProviderKind::Codex {
        spec.needs_codex_proxy = true;
        let model = resolve_codex_model(provider)?;
        spec.env
            .insert(OsString::from("ALC_CODEX_MODEL"), OsString::from(model));
        return Ok(());
    }

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
        OsString::from(&provider.model),
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

fn build_codex(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
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
        if !provider.model.is_empty() && !has_model_override(passthrough) {
            spec.args
                .extend([OsString::from("--model"), OsString::from(&provider.model)]);
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
            spec.args
                .extend([OsString::from("--model"), OsString::from(&provider.model)]);
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
        spec.args
            .extend([OsString::from("--model"), OsString::from(&provider.model)]);
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

    let model_reference = format!("{provider_id}/{}", provider.model);
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
                        &provider.model: { "name": &provider.model }
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

fn configure_claude_proxy_env(spec: &mut LaunchSpec, base_url: String, model: &OsStr) {
    for name in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ] {
        spec.env.insert(OsString::from(name), model.to_os_string());
    }
    spec.env.insert(
        OsString::from("ANTHROPIC_BASE_URL"),
        OsString::from(base_url),
    );
    spec.env.insert(
        OsString::from("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"),
        OsString::from("1"),
    );
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

fn resolve_codex_model(provider: &Provider) -> Result<String> {
    if !provider.model.trim().is_empty() {
        return Ok(provider.model.clone());
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
            if let Some(model) = read_codex_model(&path)? {
                return Ok(model);
            }
        }
    }
    Ok("gpt-5.6".to_owned())
}

fn read_codex_model(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Codex config {}", path.display()))?;
    let document: toml::Value = toml::from_str(&text)
        .with_context(|| format!("failed to parse Codex config {}", path.display()))?;
    Ok(document
        .get("model")
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
}
