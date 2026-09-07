use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::agents;
use crate::config::{
    Agent, Protocol, Provider, ProviderKind, ReasoningEffort, Store, atomic_write,
};
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

/// Which wire protocol the bundled bridge should serve to the launched agent.
///
/// `Messages` (Claude Code) and `Responses` (OpenCode, Pi) are constructed
/// today; `Chat` is produced by the Copilot builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeApi {
    Messages,
    Responses,
    Chat,
}

/// What the bundled Codex bridge needs to serve a coding-agent session. For
/// Claude Code the model is only the starting point: it switches models and
/// reasoning effort per request, so neither is pinned on the bridge. Every
/// other agent picks one model/effort at launch, which the bridge pins.
#[derive(Debug, Clone)]
pub struct BridgePlan {
    pub model: String,
    /// Pinned via CCP_CODEX_EFFORT for non-Messages clients; ALWAYS None for Claude.
    pub effort: Option<ReasoningEffort>,
    pub context_window: Option<u64>,
    /// Most capable first (catalog order).
    pub options: Vec<ModelInfo>,
    pub api: BridgeApi,
}

/// A file-system side effect a launch needs performed before the agent
/// starts. Contents are never logged; dry runs only name the affected path.
///
/// Both variants are fully handled by `process_file_setup` today. `UpsertJson`
/// is constructed by the Pi builder to merge a provider entry into
/// `models.json`; `WriteTemp` is constructed by the Kimi builder to write a
/// merged `--config-file` document to a fresh temp path.
#[derive(Debug, Clone)]
pub enum FileSetup {
    /// Merge `value` under root[pointer][key] of a JSON file, creating it if
    /// absent; refuses to touch a file that fails to parse.
    UpsertJson {
        path: PathBuf,
        pointer: &'static str,
        key: String,
        value: serde_json::Value,
    },
    /// Write a fresh file (0600 on unix when secret); removed after the run
    /// when cleanup is true.
    WriteTemp {
        path: PathBuf,
        contents: String,
        secret: bool,
        cleanup: bool,
    },
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
    pub bridge: Option<BridgePlan>,
    pub file_setup: Vec<FileSetup>,
    /// The model this session starts on, once the builder has resolved it.
    /// Descriptive only: the agent was already told through args or env.
    pub model: Option<String>,
    /// The reasoning effort this session starts on, where the agent takes one.
    pub effort: Option<ReasoningEffort>,
    /// Environment names this launch filled with credential material.
    ///
    /// `is_secret_env` recognises the conventional spellings, but a provider
    /// profile can name any variable through `api_key_env`, so the builders
    /// mark what they actually wrote rather than leaving redaction to a
    /// pattern that a custom profile can walk straight past.
    pub secret_env: BTreeSet<OsString>,
    /// The literal credential strings this launch handled, so output that
    /// echoes one back can be masked. Never serialised, never logged.
    pub secret_values: Vec<String>,
}

impl LaunchSpec {
    /// Sets `name` to a credential value: writes the environment entry, marks
    /// the name for redaction, and records the value for output scrubbing.
    /// Every builder that puts a key in the environment goes through here.
    pub(crate) fn set_secret_env(&mut self, name: impl Into<OsString>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        self.secret_env.insert(name.clone());
        self.mark_secret_value(&value);
        self.env.insert(name, OsString::from(value));
    }

    /// Records a credential that reaches the agent by some route other than
    /// the environment - Kimi writes one into a temporary config file - so it
    /// can still be masked wherever it turns up.
    pub(crate) fn mark_secret_value(&mut self, value: &str) {
        // A short value would mask unrelated text; a placeholder is not a secret.
        if value.len() < 8 || value == "alc" {
            return;
        }
        if !self.secret_values.iter().any(|known| known == value) {
            self.secret_values.push(value.to_owned());
        }
    }

    /// A minimal spec for tests in this crate. Kept beside the real
    /// fields so a new one cannot be forgotten here.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            program: OsString::from("true"),
            args: Vec::new(),
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            provider_name: "test".to_owned(),
            provider_kind: ProviderKind::Codex,
            agent: Agent::Codex,
            bridge: None,
            file_setup: Vec::new(),
            model: None,
            effort: None,
            secret_env: BTreeSet::new(),
            secret_values: Vec::new(),
        }
    }

    pub fn redacted_command(&self) -> String {
        let mut parts = Vec::new();
        for (name, value) in &self.env {
            let rendered = if self.secret_env.contains(name) || is_secret_env(name) {
                "<redacted>".to_owned()
            } else {
                self.mask(&shell_quote(value))
            };
            parts.push(format!("{}={rendered}", name.to_string_lossy()));
        }
        parts.push(shell_quote(&self.program));
        parts.extend(
            self.args
                .iter()
                .map(|value| self.mask(&shell_quote(value.as_os_str()))),
        );
        parts.join(" ")
    }

    /// Replaces any recorded credential value inside `rendered`. Arguments
    /// are masked by value because an agent can take a key as an argument,
    /// where no environment name exists to recognise.
    fn mask(&self, rendered: &str) -> String {
        let mut rendered = rendered.to_owned();
        for secret in &self.secret_values {
            if rendered.contains(secret.as_str()) {
                rendered = rendered.replace(secret.as_str(), "<redacted>");
            }
        }
        rendered
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
        bridge: None,
        file_setup: Vec::new(),
        model: None,
        effort: None,
        secret_env: BTreeSet::new(),
        secret_values: Vec::new(),
    };

    if let Some(override_path) = agent_binary_override(agent) {
        spec.program = override_path;
    }

    agents::build(
        agent,
        &mut spec,
        store,
        profile_name,
        provider,
        passthrough,
        overrides,
    )?;

    // Recorded once here rather than at each builder's own resolution site:
    // a bridged launch runs on the plan's model, and every other launch on
    // the override or the profile default, which is what the builders each
    // computed. Descriptive only - the agent has already been told.
    spec.model = spec
        .bridge
        .as_ref()
        .map(|plan| plan.model.clone())
        .or_else(|| overrides.model.clone())
        .or_else(|| (!provider.model.trim().is_empty()).then(|| provider.model.clone()));
    spec.effort = overrides.reasoning_effort.or(provider.reasoning_effort);
    Ok(spec)
}

/// The side effects a running session owns: the Codex bridge child and the
/// temporary files written for the agent. Both must outlive the agent
/// process and be torn down when it exits, so they travel together.
pub(crate) struct SessionGuards {
    #[allow(
        dead_code,
        reason = "held for its Drop; the bridge dies with the session"
    )]
    bridge: Option<Bridge>,
    cleanup: CleanupFiles,
}

impl SessionGuards {
    /// Guards for a launch that has nothing to tear down, so tests can build
    /// a session without a bridge or a temporary file.
    #[cfg(test)]
    pub(crate) fn none() -> Self {
        Self {
            bridge: None,
            cleanup: CleanupFiles(Vec::new()),
        }
    }
}

/// A launch resolved down to the point just before spawning.
pub(crate) struct Prepared {
    pub program: PathBuf,
    pub spec: LaunchSpec,
    pub guards: SessionGuards,
}

impl Prepared {
    /// The temporary files this launch wrote that must not outlive it.
    ///
    /// Recorded to disk by the hub before the agent starts, so a hub that is
    /// killed outright does not leave the Kimi builder's plaintext key file
    /// sitting there - `SessionGuards` only runs on a clean exit.
    pub(crate) fn cleanup_paths(&self) -> Vec<String> {
        self.guards
            .cleanup
            .0
            .iter()
            .map(|path| path.display().to_string())
            .collect()
    }
}

/// Starts the bridge, wires it into `spec`, performs the file setup, and
/// resolves the program path - everything `execute` used to do inline before
/// spawning. Splitting it out lets a caller spawn the child itself (under a
/// pseudo-terminal, say) while keeping the ordering this sequence depends on:
/// `apply_bridge` must run before `process_file_setup`, because the Pi and
/// Kimi builders write files whose contents name the bridge's base URL.
pub(crate) fn prepare(mut spec: LaunchSpec) -> Result<Prepared> {
    let bridge = if let Some(plan) = spec.bridge.clone() {
        let bridge = Bridge::start(&plan)?;
        agents::apply_bridge(&mut spec, &bridge.base_url(), &plan)?;
        Some(bridge)
    } else {
        None
    };

    // Held until the child exits so a failed launch still cleans up.
    let cleanup = CleanupFiles(process_file_setup(&spec)?);
    let program = resolve_program(&spec.program, spec.agent)?;
    Ok(Prepared {
        program,
        spec,
        guards: SessionGuards { bridge, cleanup },
    })
}

pub fn execute(spec: LaunchSpec) -> Result<u8> {
    let prepared = prepare(spec)?;
    let Prepared {
        program,
        spec,
        guards,
    } = prepared;

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

    // `spawn` + `wait` rather than `status` so the child has a pid a caller
    // can report, and so the guards below drop after the agent has exited
    // rather than at an unspecified point during it.
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to launch {}", program.display()))?;
    let status = child
        .wait()
        .context("failed to wait for the coding agent")?;
    drop(guards);
    Ok(exit_code(status))
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
                    // alc intentionally presents low as its simplest choice.
                    "none" => ReasoningEffort::Low,
                    // `ultra` parses on its own now; whether it can actually
                    // be used depends on the transport, and that decision
                    // belongs at the bridge boundary rather than here.
                    _ => value.parse().with_context(|| {
                        format!(
                            "invalid model_reasoning_effort in {}; expected low, medium, high, xhigh, max, or ultra",
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

pub(crate) fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let name = "USERPROFILE";
    #[cfg(not(windows))]
    let name = "HOME";
    env::var_os(name).map(PathBuf::from)
}

/// A provider's base URL as an OpenAI-compatible client expects it: Ollama's
/// default `/api` root does not itself serve the OpenAI-shaped routes, so an
/// `/v1` suffix is appended when the configured URL does not already end in
/// one. Every other provider kind is returned unchanged (trailing slash
/// trimmed). Shared by every agent builder that speaks OpenAI chat/responses.
pub(crate) fn openai_style_base_url(provider: &Provider) -> Option<String> {
    let base = provider.effective_base_url()?.trim_end_matches('/');
    if provider.kind == ProviderKind::Ollama && !base.ends_with("/v1") {
        Some(format!("{base}/v1"))
    } else {
        Some(base.to_owned())
    }
}

/// Splits a chat-style provider's base URL into the root host Goose expects
/// in `OPENAI_HOST` and the request path it expects in `OPENAI_BASE_PATH`.
///
/// "https://api.z.ai/api/paas/v4" -> ("https://api.z.ai", "api/paas/v4/chat/completions")
pub(crate) fn split_chat_url(base: &str) -> (String, String) {
    let trimmed = base.trim_end_matches('/');
    let after_scheme = trimmed.find("://").map(|i| i + 3).unwrap_or(0);
    match trimmed[after_scheme..].find('/') {
        Some(slash) => {
            let origin = &trimmed[..after_scheme + slash];
            let path = &trimmed[after_scheme + slash + 1..];
            (origin.to_owned(), format!("{path}/chat/completions"))
        }
        None => (trimmed.to_owned(), "v1/chat/completions".to_owned()),
    }
}

/// Whether a provider should be driven through its Anthropic-compatible
/// surface rather than an OpenAI-style one.
pub(crate) fn anthropic_shaped(provider: &Provider) -> bool {
    provider.kind == ProviderKind::Anthropic
        || provider.protocol == Protocol::AnthropicMessages
        || (!provider.speaks_chat() && provider.speaks_anthropic())
}

pub(crate) fn key_or_error(
    profile_name: &str,
    provider: &Provider,
    key: Option<String>,
) -> Result<String> {
    if let Some(key) = key.filter(|value| !value.is_empty()) {
        return Ok(key);
    }
    missing_key(profile_name, provider)?;
    unreachable!()
}

pub(crate) fn missing_key(profile_name: &str, provider: &Provider) -> Result<()> {
    let hint = provider
        .api_key_env
        .as_deref()
        .map(|name| format!("set {name} or "))
        .unwrap_or_default();
    bail!("provider '{profile_name}' has no API key; {hint}run `alc config` to save one")
}

pub(crate) fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

pub(crate) fn has_model_override(args: &[OsString]) -> bool {
    args.iter().any(|arg| {
        let value = arg.to_string_lossy();
        matches!(value.as_ref(), "--model" | "-m")
            || value.starts_with("--model=")
            || value.starts_with("-m=")
    })
}

pub(crate) fn has_effort_override(args: &[OsString]) -> bool {
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

pub(crate) fn has_option(args: &[OsString], long: &str, short: &str) -> bool {
    args.iter().any(|arg| {
        let value = arg.to_string_lossy();
        value == long || value == short || value.starts_with(&format!("{long}="))
    })
}

/// Inserts `args` at the very front of `spec.args`, preserving their given
/// order, ahead of anything already there. Unlike every other builder (which
/// injects its own flags before extending with the user's passthrough inside
/// `build`), a bridged agent copies passthrough into `spec.args` verbatim at
/// build time because the bridge-resolved model is not known until
/// `apply_bridge` runs later against the started bridge. `prepend_args` lets
/// `apply_bridge` still land its flags ahead of that already-copied
/// passthrough instead of after it.
pub(crate) fn prepend_args(spec: &mut LaunchSpec, args: &[&str]) {
    for (offset, arg) in args.iter().enumerate() {
        spec.args.insert(offset, OsString::from(*arg));
    }
}

fn agent_binary_override(agent: Agent) -> Option<OsString> {
    let name = match agent {
        Agent::Claude => "ALC_CLAUDE_BIN",
        Agent::Codex => "ALC_CODEX_BIN",
        Agent::Opencode => "ALC_OPENCODE_BIN",
        Agent::Pi => "ALC_PI_BIN",
        Agent::Copilot => "ALC_COPILOT_BIN",
        Agent::Goose => "ALC_GOOSE_BIN",
        Agent::Qwen => "ALC_QWEN_BIN",
        Agent::Kimi => "ALC_KIMI_BIN",
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

/// Conditional environment additions the bridge child process needs on top
/// of the constant `PORT`/`CCP_LOG_STDERR`/`CCP_CODEX_AUTH_FILE` envs. Pure
/// and side-effect-free so it can be unit tested directly.
fn bridge_child_env(plan: &BridgePlan) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    // UltraCode can open several bridge sessions concurrently. If Codex rejects
    // a WebSocket upgrade, auto transport retries the request over HTTP.
    env.insert("CCP_CODEX_TRANSPORT".to_owned(), "auto".to_owned());
    if plan.api != BridgeApi::Messages {
        env.insert("CCP_CODEX_RESPONSES_API".to_owned(), "1".to_owned());
        if let Some(effort) = plan.effort {
            env.insert("CCP_CODEX_EFFORT".to_owned(), effort.as_str().to_owned());
        }
    }
    env
}

pub(crate) struct Bridge {
    child: Child,
    port: u16,
}

impl Bridge {
    fn start(plan: &BridgePlan) -> Result<Self> {
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
            .envs(bridge_child_env(plan))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start {}", helper.display()))?;
        let mut bridge = Self { child, port };
        bridge.wait_until_ready()?;
        Ok(bridge)
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

impl Drop for Bridge {
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

/// Performs every `spec.file_setup` entry, returning the `WriteTemp` paths
/// that asked to be removed once the child process exits.
///
/// Accumulates into a `CleanupFiles` guard as it goes (not just at the end):
/// if a later entry fails, the guard is still holding the `cleanup: true`
/// paths written by earlier entries in this same call, so its `Drop` removes
/// those partials instead of orphaning them. On success the accumulated
/// paths are moved out to the caller, leaving the guard empty (a no-op drop).
fn process_file_setup(spec: &LaunchSpec) -> Result<Vec<PathBuf>> {
    let mut guard = CleanupFiles(Vec::new());
    for entry in &spec.file_setup {
        match entry {
            FileSetup::UpsertJson {
                path,
                pointer,
                key,
                value,
            } => {
                upsert_json_key(path, pointer, key, value.clone())?;
            }
            FileSetup::WriteTemp {
                path,
                contents,
                secret,
                cleanup,
            } => {
                atomic_write(path, contents.as_bytes(), *secret)?;
                if *cleanup {
                    guard.0.push(path.clone());
                }
            }
        }
    }
    Ok(std::mem::take(&mut guard.0))
}

/// Merges `value` under `root[pointer][key]` of the JSON file at `path`,
/// creating the file and any intermediate objects along `pointer` as needed.
/// A file that exists but fails to parse is left untouched and this returns
/// an error.
///
/// On unix, a pre-existing file's permission bits are restored after the
/// write: `atomic_write` always creates its temp file at 0644 before renaming
/// it over `path`, which would otherwise silently relax a mode-restricted
/// target (e.g. a 0600 agent config holding a token) to 0644.
fn upsert_json_key(path: &Path, pointer: &str, key: &str, value: serde_json::Value) -> Result<()> {
    #[cfg(unix)]
    let previous_mode = unix_mode(path)?;

    let mut document: serde_json::Value = if path.exists() {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("failed to parse {} as JSON", path.display()))?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    {
        let target = json_pointer_object_mut(&mut document, pointer).with_context(|| {
            format!(
                "{} does not have a JSON object at {pointer:?}",
                path.display()
            )
        })?;
        target.insert(key.to_owned(), value);
    }

    let encoded = serde_json::to_vec_pretty(&document).context("failed to encode JSON")?;
    atomic_write(path, &encoded, false)?;

    #[cfg(unix)]
    if let Some(mode) = previous_mode {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("failed to restore permissions on {}", path.display()))?;
    }
    Ok(())
}

/// The pre-existing unix permission bits of `path`, or `None` when it does
/// not exist yet (a fresh file keeps whatever `atomic_write` gives it).
#[cfg(unix)]
fn unix_mode(path: &Path) -> Result<Option<u32>> {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.permissions().mode())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to stat {}", path.display())),
    }
}

/// Walks `pointer` (JSON-Pointer-style, `/`-separated segments), creating an
/// empty object at each missing segment, and returns the object at the end
/// of the path. Bails if an existing segment along the way is not an object.
fn json_pointer_object_mut<'a>(
    document: &'a mut serde_json::Value,
    pointer: &str,
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>> {
    if document.is_null() {
        *document = serde_json::Value::Object(serde_json::Map::new());
    }
    let mut current = document;
    for segment in pointer.split('/').filter(|part| !part.is_empty()) {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        let map = current
            .as_object_mut()
            .context("expected a JSON object along the pointer path")?;
        current = map
            .entry(segment)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    }
    current
        .as_object_mut()
        .context("pointer does not resolve to a JSON object")
}

/// Deletes the wrapped paths when dropped, so a `WriteTemp { cleanup: true }`
/// file is removed after the child exits, even if the launch failed.
pub(crate) struct CleanupFiles(Vec<PathBuf>);

impl Drop for CleanupFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

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
    fn anthropic_shaped_is_true_for_anthropic_kind() {
        assert!(anthropic_shaped(&Provider::for_kind(
            ProviderKind::Anthropic
        )));
    }

    #[test]
    fn anthropic_shaped_is_false_for_a_dual_surface_chat_preset() {
        // Deepseek speaks both chat and an Anthropic-compatible surface, but
        // it is chat-first, so it must not be routed through the Anthropic skin.
        assert!(!anthropic_shaped(&Provider::for_kind(
            ProviderKind::Deepseek
        )));
    }

    #[test]
    fn anthropic_shaped_is_true_for_an_explicit_anthropic_messages_protocol() {
        let mut provider = Provider::for_kind(ProviderKind::Custom);
        provider.protocol = Protocol::AnthropicMessages;
        assert!(anthropic_shaped(&provider));
    }

    #[test]
    fn anthropic_shaped_is_false_for_openai_kind() {
        assert!(!anthropic_shaped(&Provider::for_kind(ProviderKind::Openai)));
    }

    // Goose's chat-style provider needs a host and a request path split out
    // of alc's single base URL; this is the case that motivated the helper.
    #[test]
    fn split_chat_url_splits_zai_style_paths() {
        assert_eq!(
            split_chat_url("https://api.z.ai/api/paas/v4"),
            (
                "https://api.z.ai".to_owned(),
                "api/paas/v4/chat/completions".to_owned()
            )
        );
    }

    #[test]
    fn split_chat_url_defaults_v1_for_a_bare_origin() {
        assert_eq!(
            split_chat_url("https://api.openai.com"),
            (
                "https://api.openai.com".to_owned(),
                "v1/chat/completions".to_owned()
            )
        );
    }

    #[test]
    fn split_chat_url_splits_the_standard_v1_suffix() {
        assert_eq!(
            split_chat_url("https://api.openai.com/v1"),
            (
                "https://api.openai.com".to_owned(),
                "v1/chat/completions".to_owned()
            )
        );
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
        assert_eq!(
            ids,
            [
                "gpt-6-astra",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna"
            ]
        );
        assert_eq!(
            rows[0]["label"].as_str(),
            Some("GPT-6 Astra"),
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

        let plan = spec.bridge.expect("Codex adapter plan");
        assert_eq!(plan.model, "gpt-5.6-sol");
        assert_eq!(plan.context_window, Some(272_000));
        assert_eq!(plan.api, BridgeApi::Messages);
        assert_eq!(plan.effort, None);
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
        let plan = spec.bridge.clone().expect("Codex adapter plan");

        agents::claude::apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();

        let value = |name: &str| spec.env[OsStr::new(name)].to_string_lossy().into_owned();
        assert_eq!(value("ANTHROPIC_MODEL"), "gpt-5.6-terra");
        // The picker's Default row would otherwise resolve to a Claude model
        // that the Codex adapter cannot serve.
        assert_eq!(value("ANTHROPIC_DEFAULT_MODEL"), "gpt-5.6-terra");
        assert_eq!(value("ANTHROPIC_DEFAULT_HAIKU_MODEL"), "gpt-5.6-luna");
        assert_eq!(value("ANTHROPIC_DEFAULT_SONNET_MODEL"), "gpt-5.6-terra");
        // `opus` follows the catalog's most capable model, so a new
        // generation reaches the alias without another mapping to maintain.
        assert_eq!(value("ANTHROPIC_DEFAULT_OPUS_MODEL"), "gpt-6-astra");
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

    fn ollama_claude_env(
        config: Config,
        overrides: &LaunchOverrides,
    ) -> BTreeMap<OsString, OsString> {
        build(
            &store(config, Credentials::default()),
            Agent::Claude,
            Some("ollama"),
            &[],
            overrides,
        )
        .unwrap()
        .env
    }

    #[test]
    fn ollama_claude_pins_every_alias_to_the_local_model() {
        let env = ollama_claude_env(Config::default(), &LaunchOverrides::default());
        let value = |name: &str| env[OsStr::new(name)].to_string_lossy().into_owned();
        assert_eq!(value("ANTHROPIC_BASE_URL"), "http://localhost:11434");
        assert_eq!(value("ANTHROPIC_AUTH_TOKEN"), "ollama");
        assert_eq!(value("ANTHROPIC_API_KEY"), "");
        // Ollama serves only pulled models, so none of Claude Code's own
        // aliases may reach it as a Claude model id.
        for name in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
        ] {
            assert_eq!(value(name), "qwen3-coder", "{name}");
        }
        assert_eq!(value("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"), "1");
        assert!(!env.contains_key(OsStr::new("CLAUDE_CODE_MAX_CONTEXT_TOKENS")));
    }

    #[test]
    fn ollama_claude_uses_the_small_model_for_the_haiku_tier() {
        let mut config = Config::default();
        config.providers.get_mut("ollama").unwrap().small_model = Some("qwen3:4b".into());
        let env = ollama_claude_env(config, &LaunchOverrides::default());
        let value = |name: &str| env[OsStr::new(name)].to_string_lossy().into_owned();
        assert_eq!(value("ANTHROPIC_DEFAULT_SONNET_MODEL"), "qwen3-coder");
        assert_eq!(value("ANTHROPIC_DEFAULT_OPUS_MODEL"), "qwen3-coder");
        assert_eq!(value("ANTHROPIC_DEFAULT_HAIKU_MODEL"), "qwen3:4b");
        assert_eq!(value("ANTHROPIC_SMALL_FAST_MODEL"), "qwen3:4b");
    }

    #[test]
    fn ollama_claude_passes_the_probed_context_window() {
        let overrides = LaunchOverrides {
            context_window: Some(65_536),
            ..LaunchOverrides::default()
        };
        let env = ollama_claude_env(Config::default(), &overrides);
        assert_eq!(
            env[OsStr::new("CLAUDE_CODE_MAX_CONTEXT_TOKENS")],
            OsString::from("65536")
        );
    }

    #[test]
    fn hosted_anthropic_compatible_providers_keep_claudes_own_aliases() {
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
        for name in [
            "ANTHROPIC_DEFAULT_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            "API_FORCE_IDLE_TIMEOUT",
            "API_TIMEOUT_MS",
        ] {
            assert!(!spec.env.contains_key(OsStr::new(name)), "{name}");
        }
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
        assert!(error.to_string().contains("Anthropic-compatible endpoint"));
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
    fn codex_to_opencode_uses_the_bridge_with_the_full_catalog() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-terra".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let mut spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Opencode,
            Some("codex"),
            &[],
            &overrides,
        )
        .unwrap();
        let plan = spec.bridge.clone().expect("bridge plan");
        assert_eq!(plan.api, BridgeApi::Responses);
        assert_eq!(plan.model, "gpt-5.6-terra");

        agents::apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();
        let inline = spec.env[OsStr::new("OPENCODE_CONFIG_CONTENT")]
            .to_string_lossy()
            .into_owned();
        let parsed: Value = serde_json::from_str(&inline).unwrap();
        assert_eq!(parsed["model"], json!("alc-codex/gpt-5.6-terra"));
        assert_eq!(
            parsed["provider"]["alc-codex"]["npm"],
            json!("@ai-sdk/openai")
        );
        assert_eq!(
            parsed["provider"]["alc-codex"]["options"]["baseURL"],
            json!("http://127.0.0.1:9/v1")
        );
        assert!(parsed["provider"]["alc-codex"]["models"]["gpt-5.6-sol"].is_object());
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

    #[test]
    fn non_claude_bridges_enable_the_responses_api_and_pin_effort() {
        let plan = BridgePlan {
            model: "gpt-5.6-terra".into(),
            effort: Some(ReasoningEffort::High),
            context_window: None,
            options: Vec::new(),
            api: BridgeApi::Responses,
        };
        let env = bridge_child_env(&plan);
        assert_eq!(
            env.get("CCP_CODEX_TRANSPORT").map(String::as_str),
            Some("auto")
        );
        assert_eq!(
            env.get("CCP_CODEX_RESPONSES_API").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            env.get("CCP_CODEX_EFFORT").map(String::as_str),
            Some("high")
        );

        let claude = BridgePlan {
            api: BridgeApi::Messages,
            effort: None,
            ..plan
        };
        let env = bridge_child_env(&claude);
        assert_eq!(
            env.get("CCP_CODEX_TRANSPORT").map(String::as_str),
            Some("auto")
        );
        assert!(!env.contains_key("CCP_CODEX_RESPONSES_API"));
        assert!(!env.contains_key("CCP_CODEX_EFFORT"));
    }

    #[test]
    fn upsert_json_key_creates_a_fresh_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");

        upsert_json_key(&path, "/servers", "alc", json!({"url": "http://x"})).unwrap();

        let document: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .expect("valid JSON was written");
        assert_eq!(document["servers"]["alc"]["url"], json!("http://x"));
    }

    #[test]
    fn upsert_json_key_preserves_unrelated_keys() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"servers":{"other":{"url":"http://keep-me"}},"unrelated":true}"#,
        )
        .unwrap();

        upsert_json_key(&path, "/servers", "alc", json!({"url": "http://x"})).unwrap();

        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(document["servers"]["other"]["url"], json!("http://keep-me"));
        assert_eq!(document["unrelated"], json!(true));
        assert_eq!(document["servers"]["alc"]["url"], json!("http://x"));
    }

    #[test]
    fn upsert_json_key_replaces_the_same_key() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        upsert_json_key(&path, "/servers", "alc", json!({"url": "http://old"})).unwrap();

        upsert_json_key(&path, "/servers", "alc", json!({"url": "http://new"})).unwrap();

        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(document["servers"]["alc"]["url"], json!("http://new"));
    }

    #[test]
    fn upsert_json_key_bails_leaving_invalid_json_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, "not valid json").unwrap();

        let error =
            upsert_json_key(&path, "/servers", "alc", json!({"url": "http://x"})).unwrap_err();

        assert!(
            error.to_string().to_lowercase().contains("json")
                || error.to_string().to_lowercase().contains("pars"),
            "unexpected error: {error}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not valid json");
    }

    #[cfg(unix)]
    #[test]
    fn upsert_json_key_preserves_an_existing_files_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        upsert_json_key(&path, "/servers", "alc", json!({"url": "http://x"})).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "upsert must not relax an existing file's permissions"
        );
    }

    #[test]
    fn build_records_the_resolved_model_for_every_agent_that_takes_one() {
        // The remote session card names the model, and it reads it from the
        // spec rather than re-deriving what each builder already resolved.
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openrouter".into(), "secret".into());
        let store = store(Config::default(), credentials);
        let expected = Provider::for_kind(ProviderKind::Openrouter).model;

        let mut checked = 0;
        for agent in Agent::ALL {
            if !store.config.providers["openrouter"].supports(agent) {
                continue;
            }
            let spec = build(
                &store,
                agent,
                Some("openrouter"),
                &[],
                &LaunchOverrides::default(),
            )
            .unwrap();
            assert_eq!(spec.model.as_deref(), Some(expected.as_str()), "{agent}");
            checked += 1;
        }
        assert!(
            checked >= 6,
            "expected most agents to support openrouter, got {checked}"
        );
    }

    #[test]
    fn build_records_the_bridge_model_for_a_codex_backed_session() {
        // A bridged launch runs on the plan's model, not the profile's.
        let spec = build(
            &store(Config::default(), Credentials::default()),
            Agent::Opencode,
            Some("codex"),
            &[],
            &LaunchOverrides {
                model: Some("gpt-5.6-luna".to_owned()),
                reasoning_effort: Some(ReasoningEffort::High),
                ..LaunchOverrides::default()
            },
        )
        .unwrap();

        assert_eq!(
            spec.bridge.as_ref().map(|plan| plan.model.as_str()),
            Some("gpt-5.6-luna")
        );
        assert_eq!(spec.model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(spec.effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn redacted_command_masks_a_custom_api_key_env_name() {
        // A provider profile can name any variable through `api_key_env`, so
        // redaction cannot rely on the conventional spellings alone.
        let mut spec = empty_spec();
        spec.set_secret_env("MY_GATEWAY_CREDENTIAL", "never-print-this");

        let rendered = spec.redacted_command();
        assert!(
            rendered.contains("MY_GATEWAY_CREDENTIAL=<redacted>"),
            "{rendered}"
        );
        assert!(!rendered.contains("never-print-this"), "{rendered}");
    }

    #[test]
    fn redacted_command_masks_a_secret_value_that_appears_in_an_argument() {
        let mut spec = empty_spec();
        spec.mark_secret_value("never-print-this");
        spec.args.push(OsString::from("--api-key=never-print-this"));

        let rendered = spec.redacted_command();
        assert!(rendered.contains("--api-key=<redacted>"), "{rendered}");
        assert!(!rendered.contains("never-print-this"), "{rendered}");
    }

    #[test]
    fn redacted_command_leaves_an_env_name_argument_alone() {
        // The Codex builder passes the NAME of the variable holding the key
        // as an argument. A value-based mask must not mistake it for the key.
        let mut spec = empty_spec();
        spec.set_secret_env("ALC_PROVIDER_API_KEY", "never-print-this");
        spec.args.push(OsString::from(
            "model_providers.alc.env_key=\"ALC_PROVIDER_API_KEY\"",
        ));

        let rendered = spec.redacted_command();
        assert!(rendered.contains("ALC_PROVIDER_API_KEY"), "{rendered}");
        assert!(!rendered.contains("never-print-this"), "{rendered}");
    }

    #[test]
    fn mark_secret_value_ignores_placeholders_that_are_not_credentials() {
        let mut spec = empty_spec();
        // `alc` is the stand-in the builders pass to servers that want a
        // non-empty key but authenticate nothing; masking it would redact
        // unrelated text everywhere the program name appears.
        spec.mark_secret_value("alc");
        spec.mark_secret_value("short");
        assert!(spec.secret_values.is_empty());
    }

    #[test]
    fn mark_secret_value_does_not_record_the_same_key_twice() {
        let mut spec = empty_spec();
        // OpenCode writes one key under two names.
        spec.set_secret_env("ALC_PROVIDER_API_KEY", "never-print-this");
        spec.set_secret_env("OPENAI_API_KEY", "never-print-this");
        assert_eq!(spec.secret_values.len(), 1);
    }

    fn empty_spec() -> LaunchSpec {
        LaunchSpec::for_test()
    }

    #[test]
    fn process_file_setup_applies_both_variants_and_only_flags_write_temp_for_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let json_path = temp.path().join("config.json");
        let temp_path = temp.path().join("secret.json");

        let mut spec = empty_spec();
        spec.file_setup = vec![
            FileSetup::UpsertJson {
                path: json_path.clone(),
                pointer: "/mcpServers",
                key: "alc".to_owned(),
                value: json!({"url": "http://127.0.0.1:1"}),
            },
            FileSetup::WriteTemp {
                path: temp_path.clone(),
                contents: "shh".to_owned(),
                secret: true,
                cleanup: true,
            },
        ];

        let cleanup = process_file_setup(&spec).unwrap();
        assert_eq!(cleanup, vec![temp_path.clone()]);

        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
        assert_eq!(
            document["mcpServers"]["alc"]["url"],
            json!("http://127.0.0.1:1")
        );
        assert_eq!(std::fs::read_to_string(&temp_path).unwrap(), "shh");

        drop(CleanupFiles(cleanup));
        assert!(
            !temp_path.exists(),
            "CleanupFiles must remove cleanup:true WriteTemp paths on drop"
        );
        assert!(
            json_path.exists(),
            "UpsertJson output is not a temp file and must survive"
        );
    }

    #[test]
    fn process_file_setup_does_not_queue_write_temp_without_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("kept.json");
        let mut spec = empty_spec();
        spec.file_setup = vec![FileSetup::WriteTemp {
            path: path.clone(),
            contents: "keep-me".to_owned(),
            secret: false,
            cleanup: false,
        }];

        let cleanup = process_file_setup(&spec).unwrap();

        assert!(cleanup.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep-me");
    }

    #[test]
    fn process_file_setup_removes_partial_write_temp_files_when_a_later_entry_fails() {
        let temp = tempfile::tempdir().unwrap();
        let temp_path = temp.path().join("secret.json");
        let broken_json_path = temp.path().join("broken.json");
        std::fs::write(&broken_json_path, "not valid json").unwrap();

        let mut spec = empty_spec();
        spec.file_setup = vec![
            FileSetup::WriteTemp {
                path: temp_path.clone(),
                contents: "shh".to_owned(),
                secret: true,
                cleanup: true,
            },
            FileSetup::UpsertJson {
                path: broken_json_path.clone(),
                pointer: "/servers",
                key: "alc".to_owned(),
                value: json!({"url": "http://x"}),
            },
        ];

        assert!(!temp_path.exists(), "sanity: not written yet");
        let error = process_file_setup(&spec).unwrap_err();

        assert!(
            error.to_string().to_lowercase().contains("json")
                || error.to_string().to_lowercase().contains("pars"),
            "unexpected error: {error}"
        );
        assert!(
            !temp_path.exists(),
            "a cleanup:true WriteTemp from an earlier entry must not be orphaned when a later entry fails"
        );
        assert_eq!(
            std::fs::read_to_string(&broken_json_path).unwrap(),
            "not valid json",
            "the failing entry's own file is left untouched"
        );
    }

    #[test]
    fn the_matrix_and_the_builders_agree() {
        // Every combination the matrix approves must build, and every one it
        // rejects must fail to build, for providers with credentials in place.
        let mut config = Config::default();
        if let Some(vllm) = config.providers.get_mut("vllm") {
            vllm.enabled = true;
            vllm.model = "test-model".into();
        }
        for kind in [
            ProviderKind::Deepseek,
            ProviderKind::Moonshot,
            ProviderKind::Zai,
            ProviderKind::Minimax,
            ProviderKind::Groq,
            ProviderKind::Xai,
            ProviderKind::Google,
        ] {
            config
                .providers
                .insert(kind.as_str().to_owned(), Provider::for_kind(kind));
        }
        let mut fake_native = Provider::for_kind(ProviderKind::Custom);
        fake_native.base_url = Some("https://example.test/v1".into());
        fake_native.model = "m".into();
        fake_native.protocol = Protocol::CodexNative;
        config.providers.insert("fake-native".into(), fake_native);

        let mut credentials = Credentials::default();
        for name in config.providers.keys() {
            credentials.api_keys.insert(name.clone(), "k".into());
        }
        let store = store(config, credentials);

        for (name, provider) in &store.config.providers {
            for agent in Agent::ALL {
                let supported = provider.supports(agent);
                let built =
                    build(&store, agent, Some(name), &[], &LaunchOverrides::default()).is_ok();
                assert_eq!(
                    supported, built,
                    "matrix vs builder disagree for {name} × {agent}"
                );
            }
        }
    }
}
