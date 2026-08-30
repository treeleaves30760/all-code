use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" | "x-high" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => bail!(
                "unknown reasoning effort '{value}'; expected low, medium, high, xhigh, or max"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
    Pi,
    Copilot,
    Goose,
    Qwen,
    Kimi,
}

impl Agent {
    pub const ALL: [Self; 8] = [
        Self::Claude,
        Self::Codex,
        Self::Opencode,
        Self::Pi,
        Self::Copilot,
        Self::Goose,
        Self::Qwen,
        Self::Kimi,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Copilot => "copilot",
            Self::Goose => "goose",
            Self::Qwen => "qwen",
            Self::Kimi => "kimi",
        }
    }

    /// Built-in default provider profile when `[defaults]` has no entry.
    pub fn default_provider(self) -> &'static str {
        match self {
            Self::Claude | Self::Pi => "anthropic",
            Self::Codex => "codex",
            Self::Opencode | Self::Copilot | Self::Goose => "openrouter",
            Self::Qwen | Self::Kimi => "openai",
        }
    }

    /// One-line protocol requirement used in compatibility errors.
    pub fn requirement(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code needs an Anthropic-compatible endpoint",
            Self::Codex => "Codex needs the OpenAI Responses API",
            Self::Opencode => "OpenCode needs an API-compatible provider",
            Self::Pi => "Pi needs an Anthropic-, OpenAI-, or OpenAI-compatible endpoint",
            Self::Copilot => "Copilot CLI needs an OpenAI- or Anthropic-compatible endpoint",
            Self::Goose => "Goose needs an OpenAI- or Anthropic-compatible endpoint",
            Self::Qwen => "Qwen Code needs an OpenAI-, Anthropic-, or Gemini-compatible endpoint",
            Self::Kimi => "Kimi Code CLI needs an OpenAI- or Anthropic-compatible endpoint",
        }
    }
}

impl std::fmt::Display for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for Agent {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "opencode" | "open-code" => Ok(Self::Opencode),
            "pi" => Ok(Self::Pi),
            "copilot" | "copilot-cli" => Ok(Self::Copilot),
            "goose" => Ok(Self::Goose),
            "qwen" | "qwen-code" => Ok(Self::Qwen),
            "kimi" | "kimi-cli" | "kimi-code" => Ok(Self::Kimi),
            _ => bail!(
                "unknown agent '{value}'; expected claude, codex, opencode, pi, copilot, goose, qwen, or kimi"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    Openrouter,
    Codex,
    Ollama,
    Vllm,
    Custom,
}

impl ProviderKind {
    pub const ALL: [Self; 7] = [
        Self::Anthropic,
        Self::Openai,
        Self::Openrouter,
        Self::Codex,
        Self::Ollama,
        Self::Vllm,
        Self::Custom,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Codex => "codex",
            Self::Ollama => "ollama",
            Self::Vllm => "vllm",
            Self::Custom => "custom",
        }
    }

    pub fn default_protocol(self) -> Protocol {
        match self {
            Self::Anthropic => Protocol::AnthropicMessages,
            Self::Openai | Self::Vllm => Protocol::OpenaiResponses,
            Self::Openrouter => Protocol::Dual,
            Self::Codex => Protocol::CodexNative,
            Self::Ollama => Protocol::Dual,
            Self::Custom => Protocol::OpenaiResponses,
        }
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("https://api.anthropic.com"),
            Self::Openai => Some("https://api.openai.com/v1"),
            Self::Openrouter => Some("https://openrouter.ai/api/v1"),
            Self::Ollama => Some("http://localhost:11434"),
            Self::Vllm => Some("http://localhost:8000/v1"),
            Self::Codex | Self::Custom => None,
        }
    }

    pub fn default_key_env(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Openai => Some("OPENAI_API_KEY"),
            Self::Openrouter => Some("OPENROUTER_API_KEY"),
            Self::Codex | Self::Ollama | Self::Vllm | Self::Custom => None,
        }
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for ProviderKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "openai" | "open-ai" => Ok(Self::Openai),
            "openrouter" | "open-router" => Ok(Self::Openrouter),
            "codex" => Ok(Self::Codex),
            "ollama" => Ok(Self::Ollama),
            "vllm" | "v-llm" => Ok(Self::Vllm),
            "custom" => Ok(Self::Custom),
            _ => bail!(
                "unknown provider kind '{value}'; expected anthropic, openai, openrouter, codex, ollama, vllm, or custom"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    AnthropicMessages,
    OpenaiResponses,
    OpenaiChat,
    CodexNative,
    Dual,
}

impl Protocol {
    pub const ALL: [Self; 5] = [
        Self::AnthropicMessages,
        Self::OpenaiResponses,
        Self::OpenaiChat,
        Self::CodexNative,
        Self::Dual,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenaiResponses => "openai-responses",
            Self::OpenaiChat => "openai-chat",
            Self::CodexNative => "codex-native",
            Self::Dual => "dual",
        }
    }

    pub fn supports_anthropic(self) -> bool {
        matches!(self, Self::AnthropicMessages | Self::Dual)
    }

    pub fn supports_responses(self) -> bool {
        matches!(self, Self::OpenaiResponses | Self::Dual)
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for Protocol {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "anthropic" | "anthropic-messages" | "messages" => Ok(Self::AnthropicMessages),
            "openai" | "openai-responses" | "responses" => Ok(Self::OpenaiResponses),
            "openai-chat" | "chat" | "chat-completions" => Ok(Self::OpenaiChat),
            "codex" | "codex-native" | "native" => Ok(Self::CodexNative),
            "dual" | "both" => Ok(Self::Dual),
            _ => bail!(
                "unknown protocol '{value}'; expected anthropic-messages, openai-responses, openai-chat, codex-native, or dual"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthStyle {
    ApiKey,
    Bearer,
    Native,
    None,
}

impl AuthStyle {
    pub const ALL: [Self; 4] = [Self::ApiKey, Self::Bearer, Self::Native, Self::None];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api-key",
            Self::Bearer => "bearer",
            Self::Native => "native",
            Self::None => "none",
        }
    }
}

impl std::fmt::Display for AuthStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for AuthStyle {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "api-key" | "apikey" | "key" => Ok(Self::ApiKey),
            "bearer" | "token" => Ok(Self::Bearer),
            "native" => Ok(Self::Native),
            "none" | "no-auth" => Ok(Self::None),
            _ => bail!("unknown auth style '{value}'; expected api-key, bearer, native, or none"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Provider {
    pub kind: ProviderKind,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub small_model: Option<String>,
    pub base_url: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub protocol: Protocol,
    pub auth: AuthStyle,
    pub api_key_env: Option<String>,
    pub codex_profile: Option<String>,
    pub enabled: bool,
}

impl Default for Provider {
    fn default() -> Self {
        Self::for_kind(ProviderKind::Custom)
    }
}

impl Provider {
    pub fn for_kind(kind: ProviderKind) -> Self {
        let model = match kind {
            ProviderKind::Anthropic => "sonnet",
            ProviderKind::Openai => "gpt-5.6-terra",
            ProviderKind::Openrouter => "anthropic/claude-sonnet-4.6",
            ProviderKind::Codex => "",
            ProviderKind::Ollama => "qwen3-coder",
            ProviderKind::Vllm | ProviderKind::Custom => "",
        };
        let auth = match kind {
            ProviderKind::Anthropic => AuthStyle::ApiKey,
            ProviderKind::Openai | ProviderKind::Openrouter => AuthStyle::Bearer,
            ProviderKind::Codex => AuthStyle::Native,
            ProviderKind::Ollama | ProviderKind::Vllm | ProviderKind::Custom => AuthStyle::None,
        };
        Self {
            kind,
            model: model.to_owned(),
            reasoning_effort: (kind == ProviderKind::Openai).then_some(ReasoningEffort::Medium),
            small_model: None,
            base_url: kind.default_base_url().map(str::to_owned),
            anthropic_base_url: None,
            protocol: kind.default_protocol(),
            auth,
            api_key_env: kind.default_key_env().map(str::to_owned),
            codex_profile: None,
            enabled: true,
        }
    }

    pub fn effective_base_url(&self) -> Option<&str> {
        self.base_url.as_deref().filter(|value| !value.is_empty())
    }

    pub fn effective_anthropic_base_url(&self) -> Option<&str> {
        self.anthropic_base_url
            .as_deref()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                if self.protocol.supports_anthropic() {
                    self.effective_base_url()
                } else {
                    None
                }
            })
    }

    pub fn speaks_anthropic(&self) -> bool {
        self.protocol.supports_anthropic()
            || self
                .anthropic_base_url
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    }

    pub fn speaks_responses(&self) -> bool {
        self.protocol.supports_responses()
    }

    /// Documented assumption: every known Responses endpoint also serves
    /// Chat Completions (OpenAI, OpenRouter, vLLM, the bundled bridge).
    pub fn speaks_chat(&self) -> bool {
        matches!(
            self.protocol,
            Protocol::OpenaiChat | Protocol::OpenaiResponses | Protocol::Dual
        )
    }

    pub fn supports(&self, agent: Agent) -> bool {
        if !self.enabled {
            return false;
        }
        if self.kind == ProviderKind::Codex || self.protocol == Protocol::CodexNative {
            // Native for Codex CLI; the bundled bridge for every other agent.
            return true;
        }
        match agent {
            Agent::Claude => self.speaks_anthropic(),
            Agent::Codex => self.kind == ProviderKind::Ollama || self.speaks_responses(),
            Agent::Opencode => true,
            Agent::Pi | Agent::Kimi => {
                self.speaks_chat() || self.speaks_responses() || self.speaks_anthropic()
            }
            Agent::Copilot | Agent::Goose | Agent::Qwen => {
                self.speaks_chat() || self.speaks_anthropic()
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Defaults(BTreeMap<Agent, String>);

impl Default for Defaults {
    fn default() -> Self {
        Self(
            Agent::ALL
                .into_iter()
                .map(|agent| (agent, agent.default_provider().to_owned()))
                .collect(),
        )
    }
}

impl Defaults {
    pub fn get(&self, agent: Agent) -> &str {
        self.0
            .get(&agent)
            .map(String::as_str)
            .unwrap_or_else(|| agent.default_provider())
    }

    pub fn set(&mut self, agent: Agent, provider: impl Into<String>) {
        self.0.insert(agent, provider.into());
    }

    /// Whether the config file names this default itself (vs. built-in fallback).
    pub fn is_explicit(&self, agent: Agent) -> bool {
        self.0.contains_key(&agent)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub defaults: Defaults,
    pub providers: BTreeMap<String, Provider>,
}

impl Default for Config {
    fn default() -> Self {
        let mut providers: BTreeMap<String, Provider> = [
            ("anthropic", ProviderKind::Anthropic),
            ("openai", ProviderKind::Openai),
            ("openrouter", ProviderKind::Openrouter),
            ("codex", ProviderKind::Codex),
            ("ollama", ProviderKind::Ollama),
            ("vllm", ProviderKind::Vllm),
        ]
        .into_iter()
        .map(|(name, kind)| (name.to_owned(), Provider::for_kind(kind)))
        .collect();
        // vLLM model IDs are deployment-specific, so keep the starter profile as a template.
        if let Some(vllm) = providers.get_mut("vllm") {
            vllm.enabled = false;
        }

        Self {
            version: CONFIG_VERSION,
            defaults: Defaults::default(),
            providers,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.version != CONFIG_VERSION {
            bail!(
                "unsupported config version {}; this alc supports version {CONFIG_VERSION}",
                self.version
            );
        }
        if self.providers.is_empty() {
            bail!("at least one provider profile is required");
        }
        for (name, provider) in &self.providers {
            validate_profile_name(name)?;
            if !provider.enabled {
                continue;
            }
            if provider.kind != ProviderKind::Codex && provider.model.trim().is_empty() {
                bail!("provider '{name}' needs a model");
            }
            if provider.kind == ProviderKind::Custom && provider.effective_base_url().is_none() {
                bail!("custom provider '{name}' needs a base_url");
            }
        }
        for agent in Agent::ALL {
            let default_name = self.defaults.get(agent);
            match self.providers.get(default_name) {
                Some(provider) if provider.supports(agent) => {}
                Some(provider) if self.defaults.is_explicit(agent) => bail!(
                    "provider '{default_name}' ({}) cannot be used with {agent}; {}",
                    provider.kind,
                    agent.requirement(),
                ),
                None if self.defaults.is_explicit(agent) => {
                    bail!("default {agent} provider '{default_name}' does not exist")
                }
                // Implicit fallback that is absent or incompatible: only an error
                // once the user actually launches that agent (resolve reports it).
                _ => {}
            }
        }
        Ok(())
    }

    pub fn resolve<'a>(
        &'a self,
        agent: Agent,
        requested: Option<&str>,
    ) -> Result<(&'a str, &'a Provider)> {
        let requested = requested.unwrap_or_else(|| self.defaults.get(agent));
        if let Some((name, provider)) = self.providers.get_key_value(requested) {
            if !provider.supports(agent) {
                bail!(
                    "provider '{name}' ({}) is not compatible with {agent}; {}. Run `alc doctor` for the compatibility matrix",
                    provider.kind,
                    agent.requirement()
                );
            }
            return Ok((name.as_str(), provider));
        }

        let kind = requested.parse::<ProviderKind>().ok();
        let matches: Vec<_> = self
            .providers
            .iter()
            .filter(|(_, provider)| kind == Some(provider.kind) && provider.supports(agent))
            .collect();
        match matches.as_slice() {
            [(name, provider)] => Ok((name.as_str(), *provider)),
            [] => bail!(
                "provider profile or kind '{requested}' was not found for {agent}; run `alc config`"
            ),
            _ => bail!(
                "more than one '{requested}' profile exists; choose one with `--provider <name>`"
            ),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Credentials {
    pub version: u32,
    pub api_keys: BTreeMap<String, String>,
}

impl Credentials {
    fn normalized(mut self) -> Self {
        if self.version == 0 {
            self.version = CONFIG_VERSION;
        }
        self
    }

    pub fn key_for(&self, profile: &str, provider: &Provider) -> Option<String> {
        provider
            .api_key_env
            .as_deref()
            .and_then(|name| env::var(name).ok())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                self.api_keys
                    .get(profile)
                    .filter(|value| !value.is_empty())
                    .cloned()
            })
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
    pub config: Config,
    pub credentials: Credentials,
}

impl Store {
    pub fn load(override_dir: Option<PathBuf>) -> Result<Self> {
        let dir = override_dir.unwrap_or(config_dir()?);
        let config_path = dir.join("config.toml");
        let credentials_path = dir.join("credentials.toml");

        let config = if config_path.exists() {
            let text = fs::read_to_string(&config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            toml::from_str(&text)
                .with_context(|| format!("failed to parse {}", config_path.display()))?
        } else {
            Config::default()
        };

        let credentials = if credentials_path.exists() {
            let text = fs::read_to_string(&credentials_path)
                .with_context(|| format!("failed to read {}", credentials_path.display()))?;
            toml::from_str::<Credentials>(&text)
                .with_context(|| format!("failed to parse {}", credentials_path.display()))?
                .normalized()
        } else {
            Credentials {
                version: CONFIG_VERSION,
                ..Credentials::default()
            }
        };

        Ok(Self {
            dir,
            config,
            credentials,
        })
    }

    pub fn ensure_saved(&self) -> Result<()> {
        if !self.config_path().exists() {
            self.save()?;
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        self.config.validate()?;
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("failed to create {}", self.dir.display()))?;

        let config_text =
            toml::to_string_pretty(&self.config).context("failed to encode config")?;
        atomic_write(&self.config_path(), config_text.as_bytes(), false)?;

        let credentials_text =
            toml::to_string_pretty(&self.credentials).context("failed to encode credentials")?;
        atomic_write(&self.credentials_path(), credentials_text.as_bytes(), true)?;
        Ok(())
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.dir.join("credentials.toml")
    }

    pub fn set_key(&mut self, profile: &str, value: String) {
        if value.is_empty() {
            self.credentials.api_keys.remove(profile);
        } else {
            self.credentials.api_keys.insert(profile.to_owned(), value);
        }
    }

    pub fn move_key(&mut self, from: &str, to: &str) {
        if from != to
            && let Some(value) = self.credentials.api_keys.remove(from)
        {
            self.credentials.api_keys.insert(to.to_owned(), value);
        }
    }
}

pub fn config_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("ALC_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    #[cfg(windows)]
    {
        let base = env::var_os("APPDATA")
            .or_else(|| {
                env::var_os("USERPROFILE")
                    .map(|home| PathBuf::from(home).join("AppData/Roaming").into_os_string())
            })
            .context("APPDATA and USERPROFILE are both unavailable")?;
        Ok(PathBuf::from(base).join("alc"))
    }

    #[cfg(not(windows))]
    {
        if let Some(base) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(base).join("alc"));
        }
        let home = env::var_os("HOME").context("HOME is unavailable")?;
        Ok(PathBuf::from(home).join(".config/alc"))
    }
}

pub fn validate_profile_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("provider profile name cannot be empty");
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("invalid provider profile '{name}'; use only letters, numbers, '-' and '_'");
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8], secret: bool) -> Result<()> {
    #[cfg(windows)]
    let _ = secret;
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("config path has a non-UTF-8 file name")?;
    let temp = parent.join(format!(".{file_name}.tmp"));

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if secret { 0o600 } else { 0o644 });
    }
    let mut file = options
        .open(&temp)
        .with_context(|| format!("failed to open {}", temp.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to replace {}", path.display()))?;
    }
    fs::rename(&temp, path)
        .with_context(|| format!("failed to move {} to {}", temp.display(), path.display()))?;

    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn provider_kind_fallback_resolves_unique_profile() {
        let mut config = Config::default();
        config.providers.remove("openrouter");
        config.providers.insert(
            "work".to_owned(),
            Provider::for_kind(ProviderKind::Openrouter),
        );
        config.defaults.set(Agent::Opencode, "work");

        let (name, _) = config.resolve(Agent::Claude, Some("openrouter")).unwrap();
        assert_eq!(name, "work");
    }

    #[test]
    fn credentials_prefer_environment() {
        let profile = format!("test-{}", std::process::id());
        let env_name = format!("ALC_TEST_KEY_{}", std::process::id());
        let mut provider = Provider::for_kind(ProviderKind::Openai);
        provider.api_key_env = Some(env_name.clone());
        let mut credentials = Credentials::default();
        credentials.api_keys.insert(profile.clone(), "saved".into());

        // SAFETY: this test uses a process-unique variable and does not run code that reads it concurrently.
        unsafe { env::set_var(&env_name, "environment") };
        assert_eq!(
            credentials.key_for(&profile, &provider).as_deref(),
            Some("environment")
        );
        // SAFETY: same process-unique test variable as above.
        unsafe { env::remove_var(&env_name) };
    }

    #[test]
    fn save_round_trip_keeps_credentials_separate() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::load(Some(temp.path().to_owned())).unwrap();
        store.set_key("openai", "top-secret".to_owned());
        store.save().unwrap();

        let config_text = fs::read_to_string(store.config_path()).unwrap();
        assert!(!config_text.contains("top-secret"));
        let loaded = Store::load(Some(temp.path().to_owned())).unwrap();
        assert_eq!(loaded.credentials.api_keys["openai"], "top-secret");
    }

    #[test]
    fn provider_configs_from_v01_load_without_reasoning_effort() {
        let provider: Provider = toml::from_str(
            r#"
kind = "codex"
model = ""
protocol = "codex-native"
auth = "native"
enabled = true
"#,
        )
        .unwrap();
        assert_eq!(provider.kind, ProviderKind::Codex);
        assert_eq!(provider.reasoning_effort, None);
    }

    #[test]
    fn legacy_three_key_defaults_still_load_and_fall_back() {
        let config: Config = toml::from_str(
            r#"
version = 1
[defaults]
claude = "anthropic"
codex = "codex"
opencode = "openrouter"
[providers.anthropic]
kind = "anthropic"
model = "sonnet"
protocol = "anthropic-messages"
auth = "api-key"
enabled = true
[providers.codex]
kind = "codex"
model = ""
protocol = "codex-native"
auth = "native"
enabled = true
[providers.openrouter]
kind = "openrouter"
model = "anthropic/claude-sonnet-4.6"
protocol = "dual"
auth = "bearer"
enabled = true
"#,
        )
        .unwrap();
        assert_eq!(config.defaults.get(Agent::Claude), "anthropic");
        assert_eq!(config.defaults.get(Agent::Pi), "anthropic"); // implicit fallback
        assert!(!config.defaults.is_explicit(Agent::Pi));
        // openai profile is absent, so the implicit qwen fallback must not fail validation
        config.validate().unwrap();
    }

    #[test]
    fn compatibility_matrix_matches_the_documented_capabilities() {
        let by_kind = |kind| Provider::for_kind(kind);
        let cases: &[(Provider, &[Agent])] = &[
            // codex login reaches every agent through the bridge
            (by_kind(ProviderKind::Codex), &Agent::ALL),
            // anthropic: everything except codex CLI
            (
                by_kind(ProviderKind::Anthropic),
                &[
                    Agent::Claude,
                    Agent::Opencode,
                    Agent::Pi,
                    Agent::Copilot,
                    Agent::Goose,
                    Agent::Qwen,
                    Agent::Kimi,
                ],
            ),
            // openai (responses+chat): everything except claude
            (
                by_kind(ProviderKind::Openai),
                &[
                    Agent::Codex,
                    Agent::Opencode,
                    Agent::Pi,
                    Agent::Copilot,
                    Agent::Goose,
                    Agent::Qwen,
                    Agent::Kimi,
                ],
            ),
            // openrouter (dual): all eight
            (by_kind(ProviderKind::Openrouter), &Agent::ALL),
        ];
        for (provider, expected) in cases {
            for agent in Agent::ALL {
                assert_eq!(
                    provider.supports(agent),
                    expected.contains(&agent),
                    "{} × {agent}",
                    provider.kind
                );
            }
        }
    }
}
