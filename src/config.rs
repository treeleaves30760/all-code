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
    /// GPT-6.1 Sol, GPT-6 Astra and Sol, and GPT-5.6 Sol and Terra add this
    /// tier above `max`; both Luna models stop at `max`.
    ///
    /// Reachable natively (`alc codex`), but NOT through the bundled
    /// claude-codex bridge: that helper's own effort enum is
    /// `none|low|medium|high|xhigh|max`, so a request carrying `ultra`
    /// is refused. `cli::resolve_codex_defaults` clamps it at that one
    /// boundary, out loud, rather than letting the rejection surface as a
    /// confusing provider error mid-session.
    Ultra,
}

impl ReasoningEffort {
    pub const ALL: [Self; 6] = [
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
        Self::Ultra,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
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
            "ultra" => Ok(Self::Ultra),
            _ => bail!(
                "unknown reasoning effort '{value}'; expected low, medium, high, xhigh, max, or ultra"
            ),
        }
    }
}

/// How much of a model's window a session is given, on the Codex and
/// Antigravity kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextChoice {
    /// The window chatgpt.com advertises as the model's own (272K; 500K for
    /// GPT-6.1 Sol). Codex's own default too. On Antigravity, the family's
    /// standard window (`models/antigravity.json`).
    #[default]
    Standard,
    /// The model's `max_context_window` (872K today; the family's maximum on
    /// Antigravity). Past the standard window a turn uses plan limits
    /// faster, so this is opt-in.
    Max,
}

impl ContextChoice {
    pub const ALL: [Self; 2] = [Self::Standard, Self::Max];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Max => "max",
        }
    }

    pub fn is_standard(&self) -> bool {
        *self == Self::Standard
    }
}

impl std::fmt::Display for ContextChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for ContextChoice {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "standard" | "std" => Ok(Self::Standard),
            "max" | "maximum" => Ok(Self::Max),
            _ => bail!("unknown context choice '{value}'; expected standard or max"),
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
    /// Google's Antigravity CLI, which runs on its own Google login.
    Agy,
}

impl Agent {
    pub const ALL: [Self; 9] = [
        Self::Claude,
        Self::Codex,
        Self::Opencode,
        Self::Pi,
        Self::Copilot,
        Self::Goose,
        Self::Qwen,
        Self::Kimi,
        Self::Agy,
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
            Self::Agy => "agy",
        }
    }

    /// Built-in default provider profile when `[defaults]` has no entry.
    pub fn default_provider(self) -> &'static str {
        match self {
            Self::Claude | Self::Pi => "anthropic",
            Self::Codex => "codex",
            Self::Opencode | Self::Copilot | Self::Goose => "openrouter",
            Self::Qwen | Self::Kimi => "openai",
            Self::Agy => "antigravity",
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
            Self::Agy => {
                "the Antigravity CLI runs only on the antigravity kind (its own Google login) or the \
                 codex kind (through alc's bridge)"
            }
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
            "agy" | "antigravity" | "antigravity-cli" => Ok(Self::Agy),
            _ => bail!(
                "unknown agent '{value}'; expected claude, codex, opencode, pi, copilot, goose, qwen, kimi, or agy"
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
    Llamacpp,
    Deepseek,
    Moonshot,
    Zai,
    Minimax,
    Groq,
    Xai,
    Google,
    Custom,
    /// Google's Antigravity login, which the Antigravity CLI holds itself.
    Antigravity,
}

impl ProviderKind {
    pub const ALL: [Self; 16] = [
        Self::Anthropic,
        Self::Openai,
        Self::Openrouter,
        Self::Codex,
        Self::Ollama,
        Self::Vllm,
        Self::Llamacpp,
        Self::Deepseek,
        Self::Moonshot,
        Self::Zai,
        Self::Minimax,
        Self::Groq,
        Self::Xai,
        Self::Google,
        Self::Custom,
        Self::Antigravity,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Codex => "codex",
            Self::Ollama => "ollama",
            Self::Vllm => "vllm",
            Self::Llamacpp => "llamacpp",
            Self::Deepseek => "deepseek",
            Self::Moonshot => "moonshot",
            Self::Zai => "zai",
            Self::Minimax => "minimax",
            Self::Groq => "groq",
            Self::Xai => "xai",
            Self::Google => "google",
            Self::Custom => "custom",
            Self::Antigravity => "antigravity",
        }
    }

    pub fn default_protocol(self) -> Protocol {
        match self {
            Self::Anthropic => Protocol::AnthropicMessages,
            Self::Openai | Self::Vllm => Protocol::OpenaiResponses,
            Self::Openrouter => Protocol::Dual,
            Self::Codex => Protocol::CodexNative,
            // llama-server answers all three: Chat Completions and Responses
            // under `/v1`, Anthropic Messages beside them.
            Self::Ollama | Self::Llamacpp => Protocol::Dual,
            Self::Deepseek
            | Self::Moonshot
            | Self::Zai
            | Self::Minimax
            | Self::Groq
            | Self::Xai
            | Self::Google => Protocol::OpenaiChat,
            Self::Custom => Protocol::OpenaiResponses,
            Self::Antigravity => Protocol::AntigravityNative,
        }
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("https://api.anthropic.com"),
            Self::Openai => Some("https://api.openai.com/v1"),
            Self::Openrouter => Some("https://openrouter.ai/api/v1"),
            Self::Ollama => Some("http://localhost:11434"),
            Self::Vllm => Some("http://localhost:8000/v1"),
            Self::Llamacpp => Some("http://localhost:8080/v1"),
            Self::Deepseek => Some("https://api.deepseek.com/v1"),
            Self::Moonshot => Some("https://api.moonshot.ai/v1"),
            Self::Zai => Some("https://api.z.ai/api/paas/v4"),
            Self::Minimax => Some("https://api.minimax.io/v1"),
            Self::Groq => Some("https://api.groq.com/openai/v1"),
            Self::Xai => Some("https://api.x.ai/v1"),
            Self::Google => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
            Self::Codex | Self::Custom | Self::Antigravity => None,
        }
    }

    /// Separate Anthropic-compatible base URL for presets that proxy the
    /// Anthropic Messages API alongside their native OpenAI-chat endpoint.
    pub fn default_anthropic_base_url(self) -> Option<&'static str> {
        match self {
            Self::Deepseek => Some("https://api.deepseek.com/anthropic"),
            Self::Moonshot => Some("https://api.moonshot.ai/anthropic"),
            Self::Zai => Some("https://api.z.ai/api/anthropic"),
            Self::Minimax => Some("https://api.minimax.io/anthropic"),
            _ => None,
        }
    }

    pub fn default_key_env(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Openai => Some("OPENAI_API_KEY"),
            Self::Openrouter => Some("OPENROUTER_API_KEY"),
            Self::Deepseek => Some("DEEPSEEK_API_KEY"),
            Self::Moonshot => Some("MOONSHOT_API_KEY"),
            Self::Zai => Some("ZAI_API_KEY"),
            Self::Minimax => Some("MINIMAX_API_KEY"),
            Self::Groq => Some("GROQ_API_KEY"),
            Self::Xai => Some("XAI_API_KEY"),
            Self::Google => Some("GEMINI_API_KEY"),
            // The variable llama-server itself reads `--api-key` from, so a
            // key exported for the server reaches the agents too.
            Self::Llamacpp => Some("LLAMA_API_KEY"),
            Self::Codex | Self::Ollama | Self::Vllm | Self::Custom | Self::Antigravity => None,
        }
    }

    /// A server someone runs for themselves: it serves only the models it
    /// loaded, answers a handful of requests at a time, and has no account to
    /// sign in to. Each of the three also answers Anthropic Messages at its
    /// root, beside the OpenAI routes under `/v1`, so Claude Code can drive it
    /// whichever OpenAI flavour the profile names for the other agents.
    pub fn is_local_server(self) -> bool {
        matches!(self, Self::Ollama | Self::Vllm | Self::Llamacpp)
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
            "llamacpp" | "llama.cpp" | "llama-cpp" | "llama_cpp" | "llama-server" => {
                Ok(Self::Llamacpp)
            }
            "deepseek" => Ok(Self::Deepseek),
            "moonshot" | "moonshotai" => Ok(Self::Moonshot),
            "zai" | "z-ai" => Ok(Self::Zai),
            "minimax" => Ok(Self::Minimax),
            "groq" => Ok(Self::Groq),
            "xai" | "x-ai" | "grok" => Ok(Self::Xai),
            "google" | "gemini" => Ok(Self::Google),
            "custom" => Ok(Self::Custom),
            "antigravity" | "agy" => Ok(Self::Antigravity),
            _ => bail!(
                "unknown provider kind '{value}'; expected anthropic, openai, openrouter, codex, ollama, vllm, llamacpp, deepseek, moonshot, zai, minimax, groq, xai, google, custom, or antigravity"
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
    /// The Antigravity CLI talking to Google with its own login; meaningful
    /// only on the antigravity kind.
    AntigravityNative,
}

impl Protocol {
    pub const ALL: [Self; 6] = [
        Self::AnthropicMessages,
        Self::OpenaiResponses,
        Self::OpenaiChat,
        Self::CodexNative,
        Self::Dual,
        Self::AntigravityNative,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenaiResponses => "openai-responses",
            Self::OpenaiChat => "openai-chat",
            Self::CodexNative => "codex-native",
            Self::Dual => "dual",
            Self::AntigravityNative => "antigravity-native",
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
            "antigravity" | "antigravity-native" => Ok(Self::AntigravityNative),
            _ => bail!(
                "unknown protocol '{value}'; expected anthropic-messages, openai-responses, openai-chat, codex-native, dual, or antigravity-native"
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

/// The wire alc's bridge speaks to a provider for an agent that cannot speak
/// it itself ([`Provider::bridge_wire`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiWire {
    Responses,
    Chat,
    Messages,
}

impl ApiWire {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "OpenAI Responses",
            Self::Chat => "OpenAI Chat Completions",
            Self::Messages => "Anthropic Messages",
        }
    }
}

/// Where the bridge sends a translated turn: the provider's own endpoint for
/// [`ApiEndpoint::wire`], and the key it signs with.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiEndpoint {
    pub kind: ProviderKind,
    pub wire: ApiWire,
    /// The full request URL: `…/responses`, `…/chat/completions` or
    /// `…/v1/messages`.
    pub url: String,
    pub auth: AuthStyle,
    pub key: Option<String>,
}

impl std::fmt::Debug for ApiEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiEndpoint")
            .field("kind", &self.kind)
            .field("wire", &self.wire)
            .field("url", &self.url)
            .field("auth", &self.auth)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Provider {
    pub kind: ProviderKind,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Whether a Codex session gets the model's standard window or its
    /// maximum. Only a codex profile may leave the standard default.
    #[serde(default, skip_serializing_if = "ContextChoice::is_standard")]
    pub context: ContextChoice,
    pub small_model: Option<String>,
    pub base_url: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub protocol: Protocol,
    pub auth: AuthStyle,
    pub api_key_env: Option<String>,
    pub codex_profile: Option<String>,
    /// The Codex home holding this profile's login (`auth.json`, `config.toml`).
    ///
    /// Absolute. `None` leaves the choice to the environment (`CODEX_HOME`,
    /// then `~/.codex`), which is what every profile did before this field
    /// existed. Setting it is how a second ChatGPT login becomes a second
    /// profile: `alc usage` reads that account's quota and a launch through
    /// this profile exports the same home, so the account you see is the one
    /// you use.
    pub codex_home: Option<String>,
    /// The Claude Code config directory holding this profile's login.
    ///
    /// Absolute, and the same idea as [`Provider::codex_home`] for an
    /// Anthropic profile that runs on Claude Code's own login rather than an
    /// API key.
    pub claude_config_dir: Option<String>,
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
            ProviderKind::Openai => "gpt-6.1-sol",
            ProviderKind::Openrouter => "anthropic/claude-sonnet-4.6",
            ProviderKind::Codex => "",
            ProviderKind::Ollama => "qwen3-coder",
            ProviderKind::Deepseek => "deepseek-v4-pro",
            ProviderKind::Moonshot => "kimi-k3",
            ProviderKind::Zai => "glm-5.3",
            ProviderKind::Minimax => "MiniMax-M3",
            ProviderKind::Groq => "llama-3.3-70b-versatile",
            ProviderKind::Xai => "grok-build-0.1",
            ProviderKind::Google => "gemini-3.7-flash",
            ProviderKind::Antigravity => "claude-opus-5-5-high",
            ProviderKind::Vllm | ProviderKind::Llamacpp | ProviderKind::Custom => "",
        };
        let auth = match kind {
            ProviderKind::Anthropic => AuthStyle::ApiKey,
            ProviderKind::Openai
            | ProviderKind::Openrouter
            | ProviderKind::Deepseek
            | ProviderKind::Moonshot
            | ProviderKind::Zai
            | ProviderKind::Minimax
            | ProviderKind::Groq
            | ProviderKind::Xai
            | ProviderKind::Google => AuthStyle::Bearer,
            ProviderKind::Codex | ProviderKind::Antigravity => AuthStyle::Native,
            ProviderKind::Ollama
            | ProviderKind::Vllm
            | ProviderKind::Llamacpp
            | ProviderKind::Custom => AuthStyle::None,
        };
        Self {
            kind,
            model: model.to_owned(),
            reasoning_effort: (kind == ProviderKind::Openai).then_some(ReasoningEffort::Low),
            context: ContextChoice::Standard,
            small_model: (kind == ProviderKind::Deepseek).then(|| "deepseek-v4-flash".to_owned()),
            base_url: kind.default_base_url().map(str::to_owned),
            anthropic_base_url: kind.default_anthropic_base_url().map(str::to_owned),
            protocol: kind.default_protocol(),
            auth,
            api_key_env: kind.default_key_env().map(str::to_owned),
            codex_profile: None,
            codex_home: None,
            claude_config_dir: None,
            enabled: true,
        }
    }

    /// The account directory this profile pins, if it pins one.
    ///
    /// Empty strings read as unset so `alc config upsert --codex-home ''`
    /// clears the field the same way the other optional strings clear.
    pub fn pinned_codex_home(&self) -> Option<&str> {
        self.codex_home.as_deref().filter(|value| !value.is_empty())
    }

    /// See [`Provider::pinned_codex_home`].
    pub fn pinned_claude_config_dir(&self) -> Option<&str> {
        self.claude_config_dir
            .as_deref()
            .filter(|value| !value.is_empty())
    }

    /// Refuses an account directory that names the wrong kind of login, or
    /// that is relative.
    ///
    /// A relative path would resolve against whichever directory the launch
    /// happened to start in, and a Codex home on an Anthropic profile would
    /// silently never be read — both fail later, somewhere less obvious.
    pub fn validate_account_dirs(&self, name: &str) -> Result<()> {
        for (value, field, kind) in [
            (self.pinned_codex_home(), "codex_home", ProviderKind::Codex),
            (
                self.pinned_claude_config_dir(),
                "claude_config_dir",
                ProviderKind::Anthropic,
            ),
        ] {
            let Some(value) = value else { continue };
            if self.kind != kind {
                bail!("provider '{name}' sets {field} but is not a {kind} profile");
            }
            if !Path::new(value).is_absolute() {
                bail!("provider '{name}' {field} must be an absolute path");
            }
        }
        Ok(())
    }

    pub fn effective_base_url(&self) -> Option<&str> {
        self.base_url.as_deref().filter(|value| !value.is_empty())
    }

    pub fn effective_anthropic_base_url(&self) -> Option<&str> {
        let url = self
            .anthropic_base_url
            .as_deref()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                if self.protocol.supports_anthropic() || self.kind.is_local_server() {
                    self.effective_base_url()
                } else {
                    None
                }
            })?;
        if !self.kind.is_local_server() {
            return Some(url);
        }
        // A local server's Anthropic surface is its root, and every client of
        // it appends `/v1/messages` itself. The `/v1` an OpenAI-style base URL
        // ends in would have them ask for `/v1/v1/messages`.
        let url = url.trim_end_matches('/');
        Some(url.strip_suffix("/v1").unwrap_or(url))
    }

    pub fn speaks_anthropic(&self) -> bool {
        self.protocol.supports_anthropic()
            || self.kind.is_local_server()
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
        self.supports_natively(agent) || self.bridge_wire(agent).is_some()
    }

    /// Whether `agent` reaches this profile without alc translating: on its
    /// own wire, or (Codex and Antigravity) through the bridge that exists
    /// for that login.
    pub fn supports_natively(&self, agent: Agent) -> bool {
        if !self.enabled {
            return false;
        }
        if self.kind == ProviderKind::Antigravity {
            // agy runs natively on its own login; every other agent reaches
            // the same login through alc's bridge (`bridge::agy_upstream`).
            return true;
        }
        if agent == Agent::Agy {
            // agy reaches the Codex login through the bridge's Code Assist
            // surface (`bridge::code_assist`).
            return self.kind == ProviderKind::Codex;
        }
        if self.protocol == Protocol::AntigravityNative {
            // antigravity-native is only meaningful on the antigravity kind.
            return false;
        }
        if self.kind == ProviderKind::Codex {
            // Native for Codex CLI; the bundled bridge for every other agent.
            return true;
        }
        if self.protocol == Protocol::CodexNative {
            // codex-native is only meaningful on the codex kind.
            return false;
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
            // Answered above.
            Agent::Agy => false,
        }
    }

    /// The wire alc's in-process bridge speaks to this profile for an agent
    /// that cannot reach it itself, or `None` when the agent needs no
    /// translation or the profile offers nothing to translate to. Responses
    /// where the profile says that is its API; Chat Completions where it
    /// offers both, as the surface every dual server implements most fully;
    /// Messages last.
    pub fn bridge_wire(&self, agent: Agent) -> Option<ApiWire> {
        if !self.enabled
            || matches!(self.kind, ProviderKind::Codex | ProviderKind::Antigravity)
            || matches!(
                self.protocol,
                Protocol::CodexNative | Protocol::AntigravityNative
            )
            || !matches!(agent, Agent::Claude | Agent::Codex | Agent::Agy)
            || self.supports_natively(agent)
        {
            return None;
        }
        let openai = self.effective_base_url().is_some();
        if openai && self.protocol == Protocol::OpenaiResponses {
            Some(ApiWire::Responses)
        } else if openai && self.speaks_chat() {
            Some(ApiWire::Chat)
        } else if self.speaks_anthropic() && self.effective_anthropic_base_url().is_some() {
            Some(ApiWire::Messages)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Defaults(BTreeMap<Agent, String>);

impl Default for Defaults {
    /// Empty: a fresh config names no default explicitly, so every agent
    /// resolves through `get`'s `Agent::default_provider()` fallback and
    /// `is_explicit` stays `false` until something actually writes a key
    /// (`config set-default`, the TUI, or an older alc's saved config.toml).
    fn default() -> Self {
        Self(BTreeMap::new())
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
            ("antigravity", ProviderKind::Antigravity),
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
            if provider.context != ContextChoice::Standard
                && !matches!(
                    provider.kind,
                    ProviderKind::Codex | ProviderKind::Antigravity
                )
            {
                bail!(
                    "provider '{name}' sets context = \"{}\", which only a codex or antigravity profile can use",
                    provider.context
                );
            }
            provider.validate_account_dirs(name)?;
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
            // Said before compatibility: a disabled profile supports nothing,
            // and blaming its protocol - the starter vLLM template's, say -
            // sends someone looking for a problem it does not have.
            if !provider.enabled {
                let model = if provider.model.trim().is_empty() {
                    " --model <id>"
                } else {
                    ""
                };
                bail!(
                    "provider '{name}' is turned off; turn it on with `alc config upsert {name} --enable{model}`"
                );
            }
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
            // A config.toml from before the antigravity kind has no profile
            // for it, and `alc config` alone does not say which one to add.
            // Not when the name is taken: `upsert` would rewrite that
            // profile for the new kind, base URL and model gone.
            [] if kind == Some(ProviderKind::Antigravity)
                && !self.providers.contains_key("antigravity") =>
            {
                bail!(
                    "provider profile or kind '{requested}' was not found for {agent}; {ADD_ANTIGRAVITY}"
                )
            }
            [] => bail!(
                "provider profile or kind '{requested}' was not found for {agent}; run `alc config`"
            ),
            _ => bail!(
                "more than one '{requested}' profile exists; choose one with `--provider <name>`"
            ),
        }
    }
}

/// What adds the Antigravity profile to a config.toml written before it.
pub const ADD_ANTIGRAVITY: &str = "run `alc config upsert antigravity --kind antigravity`";

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
    /// Statistics do not need keys. In offline mode even an unreadable
    /// credential store must not stop a report or prompt a Keychain read.
    pub(crate) fn load_without_credentials(override_dir: Option<PathBuf>) -> Result<Self> {
        let dir = match override_dir {
            Some(dir) => dir,
            None => config_dir()?,
        };
        let path = dir.join("config.toml");
        let config = if path.exists() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?
        } else {
            Config::default()
        };
        Ok(Self {
            dir,
            config,
            credentials: Credentials::default(),
        })
    }

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

pub(crate) fn atomic_write(path: &Path, bytes: &[u8], secret: bool) -> Result<()> {
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
    // A secret's temp path is unguessable and is created rather than
    // opened. The predictable `.<name>.tmp` is fine for a config file, but a
    // credential written through it can be captured by a symlink planted at
    // that path beforehand: `create(true)` follows one, so the secret lands
    // in the attacker's file and the rename then leaves the real path a
    // symlink to it, catching every later read and rotate too.
    let temp = if secret {
        let mut suffix = [0_u8; 9];
        getrandom::fill(&mut suffix)
            .map_err(|error| anyhow::anyhow!("failed to read system randomness: {error}"))?;
        let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
        parent.join(format!(".{file_name}.{suffix}.tmp"))
    } else {
        parent.join(format!(".{file_name}.tmp"))
    };

    let mut options = OpenOptions::new();
    options.write(true);
    if secret {
        // Fails rather than opens if anything is already there, so there is
        // nothing to plant.
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if secret { 0o600 } else { 0o644 });
        if secret {
            options.custom_flags(libc::O_NOFOLLOW);
        }
    }
    let mut file = options
        .open(&temp)
        .with_context(|| format!("failed to open {}", temp.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    // `rename` replaces an existing file on every platform alc supports - on
    // Windows through `MOVEFILE_REPLACE_EXISTING` - so no remove comes first,
    // and none is wanted: it would only open a moment with no file at `path`,
    // and race a second writer of the same file into access-denied errors.
    let renamed = fs::rename(&temp, path)
        .with_context(|| format!("failed to move {} to {}", temp.display(), path.display()));
    if renamed.is_err() {
        // The unguessable temp name means a failed rename would otherwise
        // leave a readable copy of the secret behind under a name nothing
        // will ever clean up.
        let _ = fs::remove_file(&temp);
    }
    renamed?;

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
    fn default_config_has_no_explicit_defaults() {
        let config = Config::default();
        for agent in Agent::ALL {
            assert!(
                !config.defaults.is_explicit(agent),
                "{agent} should not be explicit on a fresh config"
            );
        }
        // `get` still resolves through `Agent::default_provider()`.
        assert_eq!(config.defaults.get(Agent::Claude), "anthropic");
        assert_eq!(config.defaults.get(Agent::Qwen), "openai");
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
        assert!(config.defaults.is_explicit(Agent::Claude));
        assert!(config.defaults.is_explicit(Agent::Codex));
        assert!(config.defaults.is_explicit(Agent::Opencode));
        assert!(!config.defaults.is_explicit(Agent::Pi));
        // openai profile is absent, so the implicit qwen fallback must not fail validation
        config.validate().unwrap();
    }

    #[test]
    fn native_compatibility_matrix_matches_the_documented_capabilities() {
        let by_kind = |kind| Provider::for_kind(kind);
        // The Antigravity CLI runs on its own login and the Codex one.
        let every_agent_but_agy: Vec<Agent> = Agent::ALL
            .into_iter()
            .filter(|agent| *agent != Agent::Agy)
            .collect();
        let cases: &[(Provider, &[Agent])] = &[
            // codex login reaches every agent, agy included, through the bridge
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
            // openrouter (dual): every agent but agy
            (
                by_kind(ProviderKind::Openrouter),
                every_agent_but_agy.as_slice(),
            ),
            // antigravity: agy natively, every other agent through the bridge
            (by_kind(ProviderKind::Antigravity), &Agent::ALL),
        ];
        for (provider, expected) in cases {
            for agent in Agent::ALL {
                assert_eq!(
                    provider.supports_natively(agent),
                    expected.contains(&agent),
                    "{} × {agent}",
                    provider.kind
                );
            }
        }
    }

    #[test]
    fn every_preset_reaches_every_agent_natively_or_through_the_bridge() {
        for kind in ProviderKind::ALL {
            let provider = Provider::for_kind(kind);
            for agent in Agent::ALL {
                // A custom profile has no URL until the user gives it one.
                assert_eq!(
                    provider.supports(agent),
                    kind != ProviderKind::Custom || provider.supports_natively(agent),
                    "{kind} × {agent}"
                );
            }
        }
        let wire = |kind, agent| Provider::for_kind(kind).bridge_wire(agent);
        assert_eq!(
            wire(ProviderKind::Openai, Agent::Claude),
            Some(ApiWire::Responses)
        );
        assert_eq!(wire(ProviderKind::Groq, Agent::Claude), Some(ApiWire::Chat));
        assert_eq!(wire(ProviderKind::Groq, Agent::Codex), Some(ApiWire::Chat));
        assert_eq!(
            wire(ProviderKind::Anthropic, Agent::Codex),
            Some(ApiWire::Messages)
        );
        assert_eq!(
            wire(ProviderKind::Anthropic, Agent::Agy),
            Some(ApiWire::Messages)
        );
        assert_eq!(wire(ProviderKind::Ollama, Agent::Agy), Some(ApiWire::Chat));
        // Nothing to translate for an agent that speaks the wire itself.
        assert_eq!(wire(ProviderKind::Anthropic, Agent::Claude), None);
        assert_eq!(wire(ProviderKind::Groq, Agent::Pi), None);
        assert_eq!(wire(ProviderKind::Codex, Agent::Claude), None);
        assert_eq!(wire(ProviderKind::Antigravity, Agent::Codex), None);
    }

    #[test]
    fn preset_kinds_carry_urls_keys_and_claude_support() {
        let cases = [
            (
                ProviderKind::Deepseek,
                "https://api.deepseek.com/v1",
                Some("https://api.deepseek.com/anthropic"),
                "DEEPSEEK_API_KEY",
                "deepseek-v4-pro",
            ),
            (
                ProviderKind::Moonshot,
                "https://api.moonshot.ai/v1",
                Some("https://api.moonshot.ai/anthropic"),
                "MOONSHOT_API_KEY",
                "kimi-k3",
            ),
            (
                ProviderKind::Zai,
                "https://api.z.ai/api/paas/v4",
                Some("https://api.z.ai/api/anthropic"),
                "ZAI_API_KEY",
                "glm-5.3",
            ),
            (
                ProviderKind::Minimax,
                "https://api.minimax.io/v1",
                Some("https://api.minimax.io/anthropic"),
                "MINIMAX_API_KEY",
                "MiniMax-M3",
            ),
            (
                ProviderKind::Groq,
                "https://api.groq.com/openai/v1",
                None,
                "GROQ_API_KEY",
                "llama-3.3-70b-versatile",
            ),
            (
                ProviderKind::Xai,
                "https://api.x.ai/v1",
                None,
                "XAI_API_KEY",
                "grok-build-0.1",
            ),
            (
                ProviderKind::Google,
                "https://generativelanguage.googleapis.com/v1beta/openai",
                None,
                "GEMINI_API_KEY",
                "gemini-3.7-flash",
            ),
        ];
        for (kind, base, anthropic, key_env, model) in cases {
            let provider = Provider::for_kind(kind);
            assert_eq!(provider.base_url.as_deref(), Some(base), "{kind}");
            assert_eq!(provider.anthropic_base_url.as_deref(), anthropic, "{kind}");
            assert_eq!(provider.api_key_env.as_deref(), Some(key_env), "{kind}");
            assert_eq!(provider.model, model, "{kind}");
            assert_eq!(provider.protocol, Protocol::OpenaiChat, "{kind}");
            assert_eq!(provider.auth, AuthStyle::Bearer, "{kind}");
            // Anthropic-compatible presets serve Claude Code themselves; the
            // rest reach it through the bridge.
            assert_eq!(
                provider.supports_natively(Agent::Claude),
                anthropic.is_some(),
                "{kind}"
            );
            assert!(provider.supports(Agent::Claude), "{kind}");
            // None of the seven speak the Responses API, so Codex CLI runs
            // on the bridge's Chat Completions translation.
            assert!(!provider.supports_natively(Agent::Codex), "{kind}");
            assert_eq!(
                provider.bridge_wire(Agent::Codex),
                Some(ApiWire::Chat),
                "{kind}"
            );
            assert!(
                provider.supports(Agent::Pi) && provider.supports(Agent::Opencode),
                "{kind}"
            );
        }
    }

    #[test]
    fn llama_cpp_is_a_kind_of_its_own_with_llama_servers_defaults() {
        for spelling in [
            "llamacpp",
            "llama.cpp",
            "Llama-CPP",
            "llama_cpp",
            "llama-server",
        ] {
            assert_eq!(
                spelling.parse::<ProviderKind>().unwrap(),
                ProviderKind::Llamacpp,
                "{spelling}"
            );
        }
        let provider = Provider::for_kind(ProviderKind::Llamacpp);
        assert_eq!(provider.kind.to_string(), "llamacpp");
        assert_eq!(
            provider.base_url.as_deref(),
            Some("http://localhost:8080/v1")
        );
        assert_eq!(provider.protocol, Protocol::Dual);
        assert_eq!(provider.auth, AuthStyle::None);
        assert_eq!(provider.api_key_env.as_deref(), Some("LLAMA_API_KEY"));
        assert!(
            provider.model.is_empty(),
            "the model is the server's to name"
        );
        let saved = toml::to_string(&provider).unwrap();
        assert!(saved.contains("kind = \"llamacpp\""), "{saved}");
    }

    /// llama.cpp, vLLM and Ollama all answer Anthropic Messages at their root,
    /// so Claude Code is theirs whichever OpenAI flavour a profile names for
    /// the other agents - including the chat-only vLLM profiles people wrote
    /// for a llama-server before alc knew the kind.
    #[test]
    fn a_local_server_serves_claude_at_its_root_whatever_its_protocol() {
        let model = |kind, base: &str, protocol| {
            let mut provider = Provider::for_kind(kind);
            provider.model = "qwen3.8-27b".into();
            provider.base_url = Some(base.into());
            provider.protocol = protocol;
            provider
        };
        for (provider, root) in [
            (
                model(
                    ProviderKind::Llamacpp,
                    "http://127.0.0.1:8080/v1",
                    Protocol::Dual,
                ),
                "http://127.0.0.1:8080",
            ),
            (
                model(
                    ProviderKind::Vllm,
                    "http://gpu:8000/v1/",
                    Protocol::OpenaiChat,
                ),
                "http://gpu:8000",
            ),
            (
                model(
                    ProviderKind::Vllm,
                    "http://gpu:8000/v1",
                    Protocol::OpenaiResponses,
                ),
                "http://gpu:8000",
            ),
            (
                model(
                    ProviderKind::Ollama,
                    "http://gpu-box:11434/v1",
                    Protocol::Dual,
                ),
                "http://gpu-box:11434",
            ),
            (
                model(
                    ProviderKind::Ollama,
                    "http://localhost:11434",
                    Protocol::Dual,
                ),
                "http://localhost:11434",
            ),
        ] {
            assert!(provider.speaks_anthropic(), "{}", provider.kind);
            assert!(provider.supports(Agent::Claude), "{}", provider.kind);
            assert_eq!(
                provider.effective_anthropic_base_url(),
                Some(root),
                "{}",
                provider.kind
            );
        }

        // A hosted endpoint keeps its URL exactly as configured.
        let mut openrouter = Provider::for_kind(ProviderKind::Openrouter);
        openrouter.base_url = Some("https://openrouter.ai/api/v1".into());
        assert_eq!(
            openrouter.effective_anthropic_base_url(),
            Some("https://openrouter.ai/api/v1")
        );
        assert!(!Provider::for_kind(ProviderKind::Openai).speaks_anthropic());
    }

    #[test]
    fn llama_cpp_reaches_every_agent() {
        let mut provider = Provider::for_kind(ProviderKind::Llamacpp);
        provider.model = "qwen3.8-27b".into();
        for agent in Agent::ALL {
            // The Antigravity CLI through the bridge; everything else itself.
            assert_eq!(
                provider.supports_natively(agent),
                agent != Agent::Agy,
                "{agent}"
            );
            assert!(provider.supports(agent), "{agent}");
        }
        // A chat-only profile reaches Codex, which needs Responses, through
        // the bridge.
        provider.protocol = Protocol::OpenaiChat;
        assert!(!provider.supports_natively(Agent::Codex));
        assert_eq!(provider.bridge_wire(Agent::Codex), Some(ApiWire::Chat));
        assert!(provider.supports(Agent::Claude));
    }

    #[test]
    fn a_turned_off_profile_says_so_rather_than_blaming_its_protocol() {
        let config = Config::default();
        let error = config
            .resolve(Agent::Claude, Some("vllm"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("provider 'vllm' is turned off")
                && error.contains("alc config upsert vllm --enable --model <id>"),
            "{error}"
        );
        assert!(!error.contains("not compatible"), "{error}");
    }

    #[test]
    fn context_choice_round_trips_and_defaults_to_standard() {
        let provider: Provider = toml::from_str("kind = \"codex\"").unwrap();
        assert_eq!(provider.context, ContextChoice::Standard);
        let provider: Provider = toml::from_str("kind = \"codex\"\ncontext = \"max\"").unwrap();
        assert_eq!(provider.context, ContextChoice::Max);
        let text = toml::to_string(&Provider::for_kind(ProviderKind::Codex)).unwrap();
        assert!(
            !text.contains("context ="),
            "standard is the absent default: {text}"
        );
        assert_eq!("MAX".parse::<ContextChoice>().unwrap(), ContextChoice::Max);
        assert!("huge".parse::<ContextChoice>().is_err());
    }

    #[test]
    fn the_antigravity_kind_and_the_agy_agent_round_trip() {
        assert_eq!("agy".parse::<Agent>().unwrap(), Agent::Agy);
        assert_eq!("antigravity".parse::<Agent>().unwrap(), Agent::Agy);
        assert_eq!("antigravity-cli".parse::<Agent>().unwrap(), Agent::Agy);
        assert_eq!(Agent::Agy.as_str(), "agy");
        assert_eq!(Agent::Agy.default_provider(), "antigravity");
        assert_eq!(
            "antigravity".parse::<ProviderKind>().unwrap(),
            ProviderKind::Antigravity
        );
        assert_eq!(
            "agy".parse::<ProviderKind>().unwrap(),
            ProviderKind::Antigravity
        );
        assert_eq!(ProviderKind::Antigravity.as_str(), "antigravity");
        assert_eq!(Protocol::AntigravityNative.as_str(), "antigravity-native");
        let provider = Provider::for_kind(ProviderKind::Antigravity);
        assert_eq!(provider.model, "claude-opus-5-5-high");
        assert_eq!(provider.auth, AuthStyle::Native);
        assert_eq!(provider.protocol, Protocol::AntigravityNative);
        assert!(provider.base_url.is_none() && provider.api_key_env.is_none());
        assert!(Config::default().providers.contains_key("antigravity"));
    }

    #[test]
    fn the_antigravity_kind_runs_every_agent_and_agy_runs_natively_only_on_it_and_codex() {
        let antigravity = Provider::for_kind(ProviderKind::Antigravity);
        for agent in Agent::ALL {
            assert!(antigravity.supports(agent), "{agent}");
        }
        assert!(Provider::for_kind(ProviderKind::Codex).supports(Agent::Agy));
        for kind in ProviderKind::ALL
            .into_iter()
            .filter(|kind| !matches!(kind, ProviderKind::Antigravity | ProviderKind::Codex))
        {
            assert!(
                !Provider::for_kind(kind).supports_natively(Agent::Agy),
                "{kind}"
            );
        }
        let mut wrong = Provider::for_kind(ProviderKind::Openai);
        wrong.protocol = Protocol::AntigravityNative;
        assert!(
            !wrong.supports(Agent::Codex),
            "antigravity-native is meaningless off its kind"
        );
    }

    #[test]
    fn another_agent_resolves_on_the_antigravity_profile() {
        let config = Config::default();
        let (name, provider) = config.resolve(Agent::Claude, Some("antigravity")).unwrap();
        assert_eq!(name, "antigravity");
        assert_eq!(provider.kind, ProviderKind::Antigravity);
        assert_eq!(
            config.resolve(Agent::Opencode, Some("agy")).unwrap().0,
            "antigravity"
        );
        let mut config = Config::default();
        config.defaults.set(Agent::Codex, "antigravity");
        config.validate().unwrap();
    }

    #[test]
    fn a_context_choice_is_allowed_on_codex_and_antigravity_only() {
        let mut config = Config::default();
        config.providers.get_mut("antigravity").unwrap().context = ContextChoice::Max;
        config.providers.get_mut("codex").unwrap().context = ContextChoice::Max;
        config.validate().unwrap();
        let mut provider = Provider::for_kind(ProviderKind::Openrouter);
        provider.context = ContextChoice::Max;
        config.providers.insert("work".to_owned(), provider);
        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("work") && error.contains("codex or antigravity"),
            "{error}"
        );
    }

    #[test]
    fn a_config_from_before_the_antigravity_kind_names_the_command_that_adds_it() {
        let mut config = Config::default();
        config.providers.remove("antigravity");
        for requested in [None, Some("antigravity"), Some("agy")] {
            let error = config
                .resolve(Agent::Agy, requested)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("run `alc config upsert antigravity --kind antigravity`"),
                "{requested:?}: {error}"
            );
        }
        // Every agent now runs on that login, so another agent is sent to add
        // it too.
        let error = config
            .resolve(Agent::Claude, Some("antigravity"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("run `alc config upsert antigravity --kind antigravity`"),
            "{error}"
        );
        // Every other missing kind keeps the general pointer.
        let error = config
            .resolve(Agent::Claude, Some("deepseek"))
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("was not found for claude; run `alc config`"),
            "{error}"
        );
    }

    #[test]
    fn a_profile_named_antigravity_of_another_kind_is_not_offered_the_upsert_that_replaces_it() {
        // `upsert` on an existing name rewrites it for the new kind, so the
        // hint would cost a custom gateway its base URL and model.
        let mut config = Config::default();
        config.providers.insert(
            "antigravity".to_owned(),
            Provider::for_kind(ProviderKind::Openrouter),
        );
        for requested in ["agy", "ANTIGRAVITY"] {
            let error = config
                .resolve(Agent::Agy, Some(requested))
                .unwrap_err()
                .to_string();
            assert!(
                !error.contains("upsert antigravity"),
                "{requested}: {error}"
            );
            assert!(
                error.ends_with("was not found for agy; run `alc config`"),
                "{requested}: {error}"
            );
        }
    }
}

#[cfg(all(test, unix))]
mod secret_write_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A credential's temp file must not be capturable by a symlink planted
    /// at a predictable path. Without `O_NOFOLLOW` and an unguessable name,
    /// the secret lands in the attacker's file and the rename then leaves
    /// the real path pointing at it, catching every later read too.
    #[test]
    fn a_planted_symlink_cannot_capture_a_secret_write() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("captured");
        let secret_path = temp.path().join("token");
        std::os::unix::fs::symlink(&target, temp.path().join(".token.tmp")).unwrap();

        atomic_write(&secret_path, b"a-real-credential", true).unwrap();

        assert!(
            !target.exists(),
            "the secret was written through the symlink"
        );
        assert_eq!(
            fs::read_to_string(&secret_path).unwrap(),
            "a-real-credential"
        );
        assert!(!fs::symlink_metadata(&secret_path).unwrap().is_symlink());
    }

    #[test]
    fn a_secret_file_is_owner_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("token");
        atomic_write(&path, b"a-real-credential", true).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_secret_write_leaves_no_temp_file_behind() {
        let temp = tempfile::tempdir().unwrap();
        atomic_write(&temp.path().join("token"), b"a-real-credential", true).unwrap();
        let strays: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(strays.is_empty(), "left {} temp files", strays.len());
    }

    #[test]
    fn a_non_secret_write_still_replaces_an_existing_file() {
        // The predictable temp path is deliberately kept for config files:
        // it is what makes a crashed write recoverable by hand.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        atomic_write(&path, b"first", false).unwrap();
        atomic_write(&path, b"second", false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
    }
}
