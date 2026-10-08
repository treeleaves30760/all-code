//! Endpoint-only observation plans for launches that do not use alc's bridge.
//!
//! Planning does not start a listener, write a config, or retain a credential.
//! Applying a plan changes only the endpoint seam the builder actually used;
//! the agent keeps its SDK, protocol, models, arguments and authentication.

use std::env;
use std::ffi::{OsStr, OsString};
use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agents::claude_settings;
use crate::config::{Agent, AuthStyle, Provider, ProviderKind, Store};
use crate::launch::{
    FileSetup, LaunchSpec, anthropic_shaped, openai_style_base_url, split_chat_url, toml_string,
};
use crate::usage::forward::{key_digest, sanitized_endpoint};

/// A credential-free recipe carried with a launch, including shared launches.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardPlan {
    pub upstream: String,
    pub key_digests: Vec<String>,
    pub patch: EndpointPatch,
    /// Claude's settings survive supervisor restarts, so its listener must too.
    pub durable: bool,
    /// A limitation to report without claiming coverage of another transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_reason: Option<String>,
}

/// The original endpoint and its exact location, never a whole document that
/// could contain a key. These variants are not protocol adapters.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointPatch {
    ClaudeSettings {
        pointer: String,
        original: String,
    },
    CodexConfig {
        /// The builder's original index, retained as wire metadata. Applying
        /// resolves the exact pair again because shared-launch flags move it.
        argument: usize,
        original: String,
    },
    Environment {
        name: String,
        original: Option<String>,
    },
    OpenCodeConfig {
        pointer: String,
        original: Option<String>,
    },
    GooseChat {
        host: String,
        base_path: String,
    },
    KimiConfig {
        file: usize,
        path: PathBuf,
        fields: Vec<String>,
        original: String,
    },
}

// An ephemeral upstream may have a private query parameter. The wire recipe
// needs the original URL, but neither its Debug output nor its ledger identity
// should reveal it (including copies embedded in an endpoint patch).
impl std::fmt::Debug for ForwardPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwardPlan")
            .field("upstream", &debug_endpoint(&self.upstream))
            .field("key_digests", &self.key_digests)
            .field("patch", &self.patch)
            .field("durable", &self.durable)
            .field("coverage_reason", &self.coverage_reason)
            .finish()
    }
}

impl std::fmt::Debug for EndpointPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClaudeSettings { pointer, original } => f
                .debug_struct("ClaudeSettings")
                .field("pointer", pointer)
                .field("original", &debug_endpoint(original))
                .finish(),
            Self::CodexConfig { argument, .. } => f
                .debug_struct("CodexConfig")
                .field("argument", argument)
                .field("original", &"<endpoint assignment>")
                .finish(),
            Self::Environment { name, original } => f
                .debug_struct("Environment")
                .field("name", name)
                .field("original", &original.as_deref().map(debug_endpoint))
                .finish(),
            Self::OpenCodeConfig { pointer, original } => f
                .debug_struct("OpenCodeConfig")
                .field("pointer", pointer)
                .field("original", &original.as_deref().map(debug_endpoint))
                .finish(),
            Self::GooseChat { .. } => f.debug_struct("GooseChat").finish_non_exhaustive(),
            Self::KimiConfig {
                file,
                path,
                fields,
                original,
            } => f
                .debug_struct("KimiConfig")
                .field("file", file)
                .field("path", path)
                .field("fields", fields)
                .field("original", &debug_endpoint(original))
                .finish(),
        }
    }
}

fn debug_endpoint(raw: &str) -> String {
    sanitized_endpoint(raw).unwrap_or_else(|_| "<endpoint>".to_owned())
}

/// Resolve an explicitly requested metrics launch, or explain why its native
/// endpoint/authentication cannot be observed without changing its behavior.
/// The caller skips this for bridges, whose requests are already observed.
pub fn plan(spec: &LaunchSpec, store: &Store) -> Result<ForwardPlan> {
    if spec.is_bridged() {
        bail!("--metrics: this launch is already observed by alc's native bridge");
    }
    let provider = store
        .config
        .providers
        .get(&spec.provider_name)
        .context("--metrics: the launch's provider profile is no longer available")?;
    if provider.kind != spec.provider_kind {
        bail!("--metrics: the provider changed after the launch was built");
    }
    if provider.auth == AuthStyle::Native {
        bail!(
            "--metrics is unavailable for native OAuth/login authentication; the agent's login is left untouched"
        );
    }
    let result = match spec.agent {
        Agent::Claude => claude_plan(spec, store, provider)?,
        Agent::Codex => codex_plan(spec, provider)?,
        Agent::Opencode => opencode_plan(spec, provider)?,
        Agent::Copilot => copilot_plan(spec, provider)?,
        Agent::Qwen => qwen_plan(spec, provider)?,
        Agent::Goose => goose_plan(spec, provider)?,
        Agent::Kimi => kimi_plan(spec, provider)?,
        Agent::Pi => bail!(
            "--metrics is unavailable for direct Pi launches: its endpoint lives in a persistent shared models.json; alc will not leave an ephemeral metrics endpoint there"
        ),
    };
    validate_endpoint(&result.upstream, result.durable)?;
    Ok(result)
}

/// Replace only a previously matched endpoint with the loopback capability
/// route. In particular, this never adds `/v1`, rewrites a helper, changes an
/// auth type, or disables Responses WebSockets to make metrics possible.
pub fn apply(spec: &mut LaunchSpec, base_url: &str, plan: &ForwardPlan) -> Result<()> {
    if spec.is_bridged() {
        bail!("--metrics: refusing to patch an already observed bridge launch");
    }
    validate_endpoint(base_url, true)?;
    let url = Url::parse(base_url).context("--metrics: invalid local observation endpoint")?;
    let host = url.host_str().unwrap_or_default();
    let loopback = host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !loopback {
        bail!("--metrics: the observation endpoint must be on loopback");
    }
    let base_url = base_url.trim_end_matches('/');
    match &plan.patch {
        EndpointPatch::ClaudeSettings { pointer, original } => {
            if spec.agent != Agent::Claude || pointer != "/env/ANTHROPIC_BASE_URL" {
                bail!("--metrics: the Claude endpoint patch does not match this launch");
            }
            let settings = spec
                .settings_plan
                .as_mut()
                .context("--metrics: Claude's settings plan is no longer available")?;
            let endpoint = settings
                .document
                .pointer_mut(pointer)
                .context("--metrics: Claude's endpoint is no longer available")?;
            if endpoint.as_str() != Some(original) {
                bail!("--metrics: Claude's endpoint changed after planning");
            }
            *endpoint = Value::String(base_url.to_owned());
        }
        EndpointPatch::CodexConfig { original, .. } => {
            let id = codex_id(&spec.provider_name);
            let field = format!("model_providers.{id}.base_url");
            if spec.agent != Agent::Codex
                || original != &format!("{field}={}", toml_string(&plan.upstream))
            {
                bail!("--metrics: the Codex endpoint patch does not match this launch");
            }
            // Shared launch permissions prepend arguments after planning. The
            // serialized index describes the builder's original argv, not the
            // current one: resolve the exact generated pair again, rejecting
            // changed/competing endpoint assignments rather than guessing.
            let argument = resolve_codex_endpoint(&spec.args, &id, original)?;
            spec.args[argument] = OsString::from(format!("{field}={}", toml_string(base_url)));
        }
        EndpointPatch::Environment { name, original } => {
            let allowed = match spec.agent {
                Agent::Copilot => name == "COPILOT_PROVIDER_BASE_URL",
                Agent::Qwen => matches!(name.as_str(), "ANTHROPIC_BASE_URL" | "OPENAI_BASE_URL"),
                Agent::Goose => name == "ANTHROPIC_HOST",
                _ => false,
            };
            if !allowed || (original.is_none() && spec.agent != Agent::Goose) {
                bail!("--metrics: the environment endpoint patch does not match this launch");
            }
            if env_text(spec, name)? != original.as_deref() {
                bail!("--metrics: the launch endpoint changed after planning");
            }
            spec.env
                .insert(OsString::from(name), OsString::from(base_url));
        }
        EndpointPatch::OpenCodeConfig { pointer, original } => {
            let expected = opencode_pointer(&opencode_id(spec.provider_kind, &spec.provider_name));
            if spec.agent != Agent::Opencode || pointer != &expected {
                bail!("--metrics: the OpenCode endpoint patch does not match this launch");
            }
            let mut inline = opencode_document(spec)?;
            if json_endpoint(&inline, pointer)? != original.as_deref() {
                bail!("--metrics: OpenCode's endpoint changed after planning");
            }
            insert_json_endpoint(&mut inline, pointer, base_url)?;
            let contents = serde_json::to_string(&inline)
                .context("--metrics: failed to encode OpenCode's endpoint patch")?;
            spec.env.insert(
                OsString::from("OPENCODE_CONFIG_CONTENT"),
                OsString::from(contents),
            );
        }
        EndpointPatch::GooseChat { host, base_path } => {
            if spec.agent != Agent::Goose
                || env_text(spec, "OPENAI_HOST")? != Some(host)
                || env_text(spec, "OPENAI_BASE_PATH")? != Some(base_path)
            {
                bail!("--metrics: Goose's split endpoint changed after planning");
            }
            // Goose concatenates HOST + '/' + BASE_PATH. The forwarder joins
            // this request suffix onto the original *base*, not a full chat URL.
            let origin = url.origin().ascii_serialization();
            let route = url.path().trim_matches('/');
            let path = if route.is_empty() {
                "chat/completions".to_owned()
            } else {
                format!("{route}/chat/completions")
            };
            spec.env
                .insert(OsString::from("OPENAI_HOST"), OsString::from(origin));
            spec.env
                .insert(OsString::from("OPENAI_BASE_PATH"), OsString::from(path));
        }
        EndpointPatch::KimiConfig {
            file,
            path,
            fields,
            original,
        } => {
            if spec.agent != Agent::Kimi || fields != &kimi_fields(&spec.provider_name) {
                bail!("--metrics: the Kimi endpoint patch does not match this launch");
            }
            let Some(FileSetup::WriteTemp {
                path: actual,
                contents,
                secret: true,
                cleanup: true,
            }) = spec.file_setup.get_mut(*file)
            else {
                bail!("--metrics: Kimi's temporary config plan is no longer available");
            };
            if actual != path {
                bail!("--metrics: Kimi's temporary config path changed after planning");
            }
            let mut document = parse_kimi(contents)?;
            let endpoint = toml_field_mut(&mut document, fields)
                .context("--metrics: Kimi's endpoint is no longer available")?;
            if endpoint.as_str() != Some(original) {
                bail!("--metrics: Kimi's endpoint changed after planning");
            }
            *endpoint = toml::Value::String(base_url.to_owned());
            *contents = toml::to_string_pretty(&document).map_err(|_| {
                anyhow::anyhow!("--metrics: failed to encode Kimi's endpoint patch")
            })?;
        }
    }
    Ok(())
}

fn claude_plan(spec: &LaunchSpec, store: &Store, provider: &Provider) -> Result<ForwardPlan> {
    let settings = spec
        .settings_plan
        .as_ref()
        .context("--metrics is unavailable: Claude has no provider settings plan")?;
    let alc = env::current_exe().context("--metrics: cannot resolve alc's credential helper")?;
    let dir = std::path::absolute(&store.dir)
        .context("--metrics: cannot resolve the credential helper's config directory")?;
    let expected_helper = claude_settings::helper_command(
        claude_settings::Shell::HOST,
        &alc,
        &dir,
        &format!("profile:{}", spec.provider_name),
    )?;
    if settings
        .document
        .get("apiKeyHelper")
        .and_then(Value::as_str)
        != Some(&expected_helper)
    {
        bail!(
            "--metrics is unavailable: Claude uses its own login, a keyless endpoint, or an overridden apiKeyHelper; alc only observes its own keyed provider helper"
        );
    }
    for name in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
        if provider.api_key_env.as_deref() == Some(name)
            && env::var(name).is_ok_and(|value| !value.is_empty())
        {
            bail!(
                "--metrics requires Claude to use its sealed apiKeyHelper credential; save this profile's key with `alc config key`, unset its exported Claude authentication variable, and relaunch"
            );
        }
    }
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        if settings
            .document
            .get("env")
            .and_then(|env| env.get(name))
            .is_some_and(|value| value.as_str() != Some(""))
        {
            bail!(
                "--metrics is unavailable: Claude's provider authentication or cloud-provider settings were overridden"
            );
        }
    }
    let base = provider
        .effective_anthropic_base_url()
        .context("--metrics: the provider has no Anthropic endpoint")?
        .trim_end_matches('/');
    let expected = if provider.kind == ProviderKind::Openrouter && base.ends_with("/api/v1") {
        base.trim_end_matches("/v1")
    } else {
        base
    };
    let pointer = "/env/ANTHROPIC_BASE_URL";
    let original = json_endpoint(&settings.document, pointer)?
        .context("--metrics: Claude's provider endpoint is missing")?;
    if original != expected {
        bail!("--metrics is unavailable: Claude's provider endpoint was overridden");
    }
    let key = store
        .credentials
        .key_for(&spec.provider_name, provider)
        .context("--metrics is unavailable: Claude's provider helper has no known API key")?;
    Ok(ForwardPlan {
        upstream: original.to_owned(),
        key_digests: vec![key_digest(&key)],
        patch: EndpointPatch::ClaudeSettings {
            pointer: pointer.to_owned(),
            original: original.to_owned(),
        },
        durable: true,
        coverage_reason: None,
    })
}

fn codex_plan(spec: &LaunchSpec, provider: &Provider) -> Result<ForwardPlan> {
    if matches!(provider.kind, ProviderKind::Codex | ProviderKind::Ollama) {
        bail!(
            "--metrics is unavailable for Codex's native login or native Ollama integration; alc will not change its provider or transport"
        );
    }
    reject_options(
        spec,
        &["--oss", "--local-provider", "--profile", "-p"],
        "Codex selects a native/local provider or a custom profile",
    )?;
    let upstream = openai_style_base_url(provider)
        .context("--metrics: the provider has no Responses endpoint")?;
    let id = codex_id(&spec.provider_name);
    let endpoint = format!("model_providers.{id}.base_url={}", toml_string(&upstream));
    let mut generated = vec![
        format!("model_provider={}", toml_string(&id)),
        format!(
            "model_providers.{id}.name={}",
            toml_string(&format!("alc: {}", spec.provider_name))
        ),
        endpoint.clone(),
        format!("model_providers.{id}.wire_api={}", toml_string("responses")),
        format!("model_providers.{id}.requires_openai_auth=false"),
    ];
    if env_text(spec, "ALC_PROVIDER_API_KEY")?.is_some() {
        generated.push(format!(
            "model_providers.{id}.env_key={}",
            toml_string("ALC_PROVIDER_API_KEY")
        ));
    }
    let mut endpoint_argument = None;
    let mut websocket_disabled = false;
    for (index, assignment) in codex_configs(&spec.args)? {
        let parsed = toml::from_str::<toml::Table>(assignment).ok();
        let reserved = parsed
            .as_ref()
            .map(|table| {
                table.contains_key("model_provider") || table.contains_key("model_providers")
            })
            // A bare TOML string is supported by Codex. Never use a failed
            // parse to overlook a quoted/dotted provider override.
            .unwrap_or_else(|| assignment.contains("model_provider"));
        // Changing base_url changes the WebSocket target too, and the HTTP
        // forwarder cannot tunnel an Upgrade. Require the user's existing
        // typed false override; never force-disable WebSockets or inspect the
        // agent's private config to guess its inherited transport selection.
        // Parsing the key separately admits quoted TOML keys, but not a whole
        // provider-table assignment that could replace unrelated fields.
        let websocket = assignment
            .split_once('=')
            .and_then(|(key, _)| toml::from_str::<toml::Table>(&format!("{key}=false")).ok())
            .is_some_and(|table| codex_websocket_value(&table, &id) == Some(false));
        if websocket {
            if parsed
                .as_ref()
                .and_then(|table| codex_websocket_value(table, &id))
                != Some(false)
            {
                bail!(
                    "--metrics is unavailable for Codex's enabled or unknown Responses WebSocket transport; use an existing --config model_providers.{id}.supports_websockets=false override for HTTP-only observation, or omit --metrics; alc leaves transport settings unchanged"
                );
            }
            websocket_disabled = true;
        }
        if reserved && !websocket && !generated.iter().any(|expected| expected == assignment) {
            bail!(
                "--metrics is unavailable: Codex's model_provider or provider endpoint/authentication was overridden"
            );
        }
        if assignment == endpoint
            && (endpoint_argument.replace(index).is_some()
                || index == 0
                || spec.args.get(index - 1).map(OsString::as_os_str)
                    != Some(OsStr::new("--config")))
        {
            bail!(
                "--metrics is unavailable: Codex's provider endpoint has a user override or duplicate argument"
            );
        }
    }
    let argument = endpoint_argument
        .context("--metrics is unavailable: Codex has no exact alc-generated endpoint argument")?;
    if !websocket_disabled {
        bail!(
            "--metrics is unavailable for Codex unless its HTTP-only transport is explicitly selected with --config model_providers.{id}.supports_websockets=false; inherited/default Responses WebSocket behavior cannot be preserved by the HTTP observer; alc will not change transport settings"
        );
    }
    Ok(ForwardPlan {
        upstream,
        key_digests: env_digests(spec, "ALC_PROVIDER_API_KEY", provider.auth == AuthStyle::None)?,
        patch: EndpointPatch::CodexConfig { argument, original: endpoint },
        durable: false,
        coverage_reason: Some("Only HTTP requests are observed; the user's explicit supports_websockets=false setting is preserved. Responses WebSocket launches are unsupported.".to_owned()),
    })
}

fn opencode_plan(spec: &LaunchSpec, provider: &Provider) -> Result<ForwardPlan> {
    reject_options(
        spec,
        &["--provider", "--config", "--config-file"],
        "OpenCode selects a custom provider/config outside alc's inline endpoint",
    )?;
    let inline = opencode_document(spec)?;
    let id = opencode_id(provider.kind, &spec.provider_name);
    let prefix = format!("{id}/");
    let models = option_values(&spec.args, &["--model", "-m"])?;
    if models.is_empty() {
        if !inline
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| model.starts_with(&prefix) && model.len() > prefix.len())
        {
            bail!(
                "--metrics is unavailable: OpenCode's selected model does not use alc's provider"
            );
        }
    } else if models.iter().any(|model| {
        !model
            .to_str()
            .is_some_and(|model| model.starts_with(&prefix) && model.len() > prefix.len())
    }) {
        bail!("--metrics is unavailable: OpenCode's model argument selects a different provider");
    }
    let pointer = opencode_pointer(&id);
    let original = json_endpoint(&inline, &pointer)?;
    check_json_objects(&inline, &pointer)?;
    let provider_entry = inline
        .get("provider")
        .and_then(|providers| providers.get(&id));
    let builtin = matches!(
        provider.kind,
        ProviderKind::Anthropic | ProviderKind::Openai | ProviderKind::Openrouter
    ) && provider
        .effective_base_url()
        .zip(provider.kind.default_base_url())
        .is_some_and(|(actual, default)| {
            actual.trim_end_matches('/') == default.trim_end_matches('/')
        });
    let upstream = if let Some(original) = original {
        let expected = openai_style_base_url(provider)
            .context("--metrics: the OpenCode provider has no endpoint")?;
        if original != expected {
            bail!("--metrics is unavailable: OpenCode's inline provider endpoint was overridden");
        }
        original.to_owned()
    } else if builtin {
        // @ai-sdk/anthropic's default base includes /v1 and it appends
        // /messages, unlike Claude Code, which appends /v1/messages itself.
        match provider.kind {
            ProviderKind::Anthropic => "https://api.anthropic.com/v1".to_owned(),
            ProviderKind::Openai => "https://api.openai.com/v1".to_owned(),
            ProviderKind::Openrouter => "https://openrouter.ai/api/v1".to_owned(),
            _ => unreachable!(),
        }
    } else {
        bail!("--metrics is unavailable: OpenCode has no alc-managed provider endpoint");
    };
    if original.is_none() {
        let ambient_name = match provider.kind {
            ProviderKind::Anthropic => "ANTHROPIC_BASE_URL",
            ProviderKind::Openai => "OPENAI_BASE_URL",
            ProviderKind::Openrouter => "OPENROUTER_BASE_URL",
            _ => unreachable!(),
        };
        if env::var_os(ambient_name)
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_some_and(|value| value != OsStr::new(&upstream))
        {
            bail!(
                "--metrics is unavailable: OpenCode's built-in provider inherits a different endpoint; the user's override is left untouched"
            );
        }
    }
    let options_key = provider_entry
        .and_then(|entry| entry.get("options"))
        .and_then(|options| options.get("apiKey"));
    let key_env = if original.is_some() {
        if options_key.is_some_and(|key| key.as_str() != Some("{env:ALC_PROVIDER_API_KEY}")) {
            bail!(
                "--metrics is unavailable: OpenCode's inline provider authentication was overridden"
            );
        }
        if options_key.is_some() {
            "ALC_PROVIDER_API_KEY"
        } else {
            ""
        }
    } else {
        if options_key.is_some() {
            bail!(
                "--metrics is unavailable: OpenCode's built-in provider authentication is not alc-managed"
            );
        }
        match provider.kind {
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Openai => "OPENAI_API_KEY",
            ProviderKind::Openrouter => "OPENROUTER_API_KEY",
            _ => unreachable!(),
        }
    };
    let keyless = provider.auth == AuthStyle::None
        && (provider.kind.is_local_server() || provider.kind == ProviderKind::Custom);
    let key_digests = if key_env.is_empty() {
        if !keyless {
            bail!(
                "--metrics is unavailable: OpenCode's provider has no known API key; agent-managed authentication is left untouched"
            );
        }
        Vec::new()
    } else {
        env_digests(spec, key_env, keyless)?
    };
    Ok(ForwardPlan {
        upstream,
        key_digests,
        patch: EndpointPatch::OpenCodeConfig {
            pointer,
            original: original.map(str::to_owned),
        },
        durable: false,
        coverage_reason: None,
    })
}

fn copilot_plan(spec: &LaunchSpec, provider: &Provider) -> Result<ForwardPlan> {
    reject_options(
        spec,
        &[
            "--provider",
            "--provider-type",
            "--provider-base-url",
            "--provider-api-key",
        ],
        "Copilot overrides alc's BYOK provider",
    )?;
    let expected = if anthropic_shaped(provider) {
        provider.effective_anthropic_base_url()
    } else {
        provider.effective_base_url()
    }
    .context("--metrics: the Copilot provider has no endpoint")?;
    let expected_type = if anthropic_shaped(provider) {
        "anthropic"
    } else {
        "openai"
    };
    if env_text(spec, "COPILOT_PROVIDER_TYPE")? != Some(expected_type) {
        bail!("--metrics is unavailable: Copilot's BYOK provider type changed");
    }
    environment_plan(
        spec,
        "COPILOT_PROVIDER_BASE_URL",
        expected,
        env_digests(
            spec,
            "COPILOT_PROVIDER_API_KEY",
            provider.auth == AuthStyle::None,
        )?,
    )
}

fn qwen_plan(spec: &LaunchSpec, provider: &Provider) -> Result<ForwardPlan> {
    if provider.kind == ProviderKind::Google {
        bail!(
            "--metrics is unavailable for Qwen's Gemini integration: it has no alc-managed endpoint seam"
        );
    }
    reject_options(
        spec,
        &["--base-url", "--openai-base-url", "--api-key", "--provider"],
        "Qwen overrides alc's BYOK endpoint or authentication",
    )?;
    let (auth, endpoint, key, expected) = if anthropic_shaped(provider) {
        (
            "anthropic",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_KEY",
            provider.effective_anthropic_base_url(),
        )
    } else {
        (
            "openai",
            "OPENAI_BASE_URL",
            "OPENAI_API_KEY",
            provider.effective_base_url(),
        )
    };
    let auth_types = option_values(&spec.args, &["--auth-type"])?;
    if auth_types.is_empty() || auth_types.iter().any(|value| *value != OsStr::new(auth)) {
        bail!(
            "--metrics is unavailable: Qwen's --auth-type selects a different authentication/provider branch"
        );
    }
    environment_plan(
        spec,
        endpoint,
        expected.context("--metrics: the Qwen provider has no endpoint")?,
        env_digests(spec, key, false)?,
    )
}

fn goose_plan(spec: &LaunchSpec, provider: &Provider) -> Result<ForwardPlan> {
    if matches!(
        provider.kind,
        ProviderKind::Openrouter | ProviderKind::Ollama
    ) {
        bail!(
            "--metrics is unavailable for Goose's native OpenRouter/Ollama integration; alc will not switch its provider to obtain metrics"
        );
    }
    reject_options(
        spec,
        &["--provider", "--base-url", "--api-key"],
        "Goose overrides alc's provider endpoint or authentication",
    )?;
    if anthropic_shaped(provider) {
        if env_text(spec, "GOOSE_PROVIDER")? != Some("anthropic") {
            bail!("--metrics is unavailable: Goose's Anthropic provider changed");
        }
        let expected = provider
            .effective_anthropic_base_url()
            .context("--metrics: the Goose provider has no Anthropic endpoint")?;
        let original = env_text(spec, "ANTHROPIC_HOST")?;
        if let Some(original) = original {
            if original != expected {
                bail!("--metrics is unavailable: Goose's Anthropic host was overridden");
            }
        } else {
            let ambient = env::var_os("ANTHROPIC_HOST");
            if expected != "https://api.anthropic.com"
                || ambient
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .is_some_and(|value| value != OsStr::new(expected))
            {
                bail!(
                    "--metrics is unavailable: Goose's inherited Anthropic host does not match its provider"
                );
            }
        }
        Ok(ForwardPlan {
            upstream: expected.to_owned(),
            key_digests: env_digests(spec, "ANTHROPIC_API_KEY", false)?,
            patch: EndpointPatch::Environment {
                name: "ANTHROPIC_HOST".to_owned(),
                original: original.map(str::to_owned),
            },
            durable: false,
            coverage_reason: None,
        })
    } else {
        if env_text(spec, "GOOSE_PROVIDER")? != Some("openai") {
            bail!("--metrics is unavailable: Goose's OpenAI provider changed");
        }
        let expected = provider
            .effective_base_url()
            .context("--metrics: the Goose provider has no chat endpoint")?;
        if expected.contains(['?', '#']) {
            bail!(
                "--metrics is unavailable: Goose's split chat endpoint contains a query or fragment whose request semantics cannot be preserved; the original endpoint is left untouched"
            );
        }
        let (expected_host, expected_path) = split_chat_url(expected);
        if env_text(spec, "OPENAI_HOST")? != Some(&expected_host)
            || env_text(spec, "OPENAI_BASE_PATH")? != Some(&expected_path)
        {
            bail!("--metrics is unavailable: Goose's split chat endpoint was overridden");
        }
        let prefix = expected_path
            .strip_suffix("/chat/completions")
            .context("--metrics: Goose's chat request path cannot be observed safely")?;
        let upstream = format!(
            "{}/{}",
            expected_host.trim_end_matches('/'),
            prefix.trim_matches('/')
        );
        Ok(ForwardPlan {
            upstream: upstream.trim_end_matches('/').to_owned(),
            key_digests: env_digests(spec, "OPENAI_API_KEY", false)?,
            patch: EndpointPatch::GooseChat {
                host: expected_host,
                base_path: expected_path,
            },
            durable: false,
            coverage_reason: None,
        })
    }
}

fn kimi_plan(spec: &LaunchSpec, provider: &Provider) -> Result<ForwardPlan> {
    reject_options(
        spec,
        &[
            "--config",
            "--model",
            "-m",
            "--provider",
            "--base-url",
            "--api-key",
        ],
        "Kimi selects a user config/model outside alc's temporary provider",
    )?;
    let config_files = option_values(&spec.args, &["--config-file"])?;
    if config_files.len() != 1 {
        bail!("--metrics is unavailable: Kimi has no single alc-generated --config-file");
    }
    let fields = kimi_fields(&spec.provider_name);
    let id = format!("alc-{}", spec.provider_name);
    let expected_type = if anthropic_shaped(provider) {
        "anthropic"
    } else if provider.kind == ProviderKind::Openai {
        "openai_responses"
    } else {
        "openai_legacy"
    };
    let expected = if anthropic_shaped(provider) {
        provider.effective_anthropic_base_url()
    } else {
        provider.effective_base_url()
    }
    .context("--metrics: the Kimi provider has no endpoint")?;
    let mut result = None;
    for (file, setup) in spec.file_setup.iter().enumerate() {
        let FileSetup::WriteTemp {
            path,
            contents,
            secret: true,
            cleanup: true,
        } = setup
        else {
            continue;
        };
        if path.as_os_str() != config_files[0] {
            continue;
        }
        if result.is_some() {
            bail!("--metrics is unavailable: Kimi has multiple competing temporary config plans");
        }
        let document = parse_kimi(contents)?;
        let entry = document
            .get("providers")
            .and_then(|providers| providers.get(&id))
            .context("--metrics: Kimi's alc provider entry is missing")?;
        let original = entry
            .get("base_url")
            .and_then(toml::Value::as_str)
            .context("--metrics: Kimi's provider endpoint is missing")?;
        if original != expected
            || entry.get("type").and_then(toml::Value::as_str) != Some(expected_type)
            || document.get("default_model").and_then(toml::Value::as_str) != Some(&id)
            || document
                .get("models")
                .and_then(|models| models.get(&id))
                .and_then(|model| model.get("provider"))
                .and_then(toml::Value::as_str)
                != Some(&id)
        {
            bail!(
                "--metrics is unavailable: Kimi's selected provider, endpoint or type was overridden"
            );
        }
        let key = entry
            .get("api_key")
            .and_then(toml::Value::as_str)
            .filter(|key| !key.is_empty())
            .context(
                "--metrics is unavailable: Kimi has no known API key or keyless placeholder",
            )?;
        result = Some(ForwardPlan {
            upstream: original.to_owned(),
            key_digests: vec![key_digest(key)],
            patch: EndpointPatch::KimiConfig {
                file,
                path: path.clone(),
                fields: fields.clone(),
                original: original.to_owned(),
            },
            durable: false,
            coverage_reason: None,
        });
    }
    result.context(
        "--metrics is unavailable: Kimi uses a user config rather than alc's temporary config plan",
    )
}

fn environment_plan(
    spec: &LaunchSpec,
    name: &str,
    expected: &str,
    key_digests: Vec<String>,
) -> Result<ForwardPlan> {
    let original = env_text(spec, name)?.context(
        "--metrics is unavailable: the launch has no alc-managed endpoint environment variable",
    )?;
    if original != expected {
        bail!("--metrics is unavailable: the launch's provider endpoint was overridden");
    }
    Ok(ForwardPlan {
        upstream: original.to_owned(),
        key_digests,
        patch: EndpointPatch::Environment {
            name: name.to_owned(),
            original: Some(original.to_owned()),
        },
        durable: false,
        coverage_reason: None,
    })
}

fn env_text<'a>(spec: &'a LaunchSpec, name: &str) -> Result<Option<&'a str>> {
    spec.env
        .get(OsStr::new(name))
        .map(|value| {
            value
                .to_str()
                .context("--metrics: a required launch environment value is not UTF-8")
        })
        .transpose()
}

fn env_digests(spec: &LaunchSpec, name: &str, keyless: bool) -> Result<Vec<String>> {
    match env_text(spec, name)?.filter(|key| !key.is_empty()) {
        Some(key) => Ok(vec![key_digest(key)]),
        None if keyless => {
            if env::var_os(name).is_some_and(|value| !value.is_empty()) {
                bail!(
                    "--metrics is unavailable: a keyless provider inherits an API key outside alc's launch plan; authentication is left untouched"
                );
            }
            Ok(Vec::new())
        }
        None => bail!(
            "--metrics is unavailable: the agent has no alc-managed API key; agent-managed authentication is left untouched"
        ),
    }
}

fn validate_endpoint(raw: &str, durable: bool) -> Result<()> {
    // Do not attach URL/TOML parse errors: they can include secret-bearing
    // source text. A clear reason is sufficient, and never echoes the URL.
    let url = Url::parse(raw).map_err(|_| {
        anyhow::anyhow!(
            "--metrics is unavailable: the provider endpoint is not an absolute HTTP(S) URL"
        )
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("--metrics is unavailable: the provider endpoint must use HTTP(S)");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("--metrics is unavailable: credentials in endpoint URLs cannot be captured safely");
    }
    if url.fragment().is_some() {
        bail!("--metrics is unavailable: endpoint URL fragments cannot be forwarded");
    }
    if durable && url.query().is_some() {
        bail!(
            "--metrics is unavailable: a durable Claude route cannot persist an endpoint query string"
        );
    }
    // Use the forwarder's transport checks as well, so planning cannot promise
    // a launch its listener will later refuse (for example non-loopback HTTP).
    sanitized_endpoint(raw).context("--metrics is unavailable for this endpoint")?;
    Ok(())
}

fn resolve_codex_endpoint(args: &[OsString], id: &str, original: &str) -> Result<usize> {
    let field = format!("model_providers.{id}.base_url");
    let provider_table = format!("model_providers.{id}");
    let mut found = None;
    for (index, assignment) in codex_configs(args)? {
        // Detect quoted keys and parent table replacements too. A raw-text
        // fallback errs on refusal if a late malformed override cannot be
        // parsed, rather than applying beneath an ambiguous user selection.
        let touches_endpoint = toml::from_str::<toml::Table>(assignment)
            .map(|table| {
                table.get("model_providers").is_some_and(|providers| {
                    !providers.is_table()
                        || providers.get(id).is_some_and(|provider| {
                            !provider.is_table() || provider.get("base_url").is_some()
                        })
                })
            })
            .unwrap_or_else(|_| {
                assignment.contains(&field)
                    || assignment.split_once('=').is_some_and(|(key, _)| {
                        matches!(key.trim(), "model_providers") || key.trim() == provider_table
                    })
            });
        if assignment == original {
            if found.replace(index).is_some()
                || index == 0
                || args.get(index - 1).map(OsString::as_os_str) != Some(OsStr::new("--config"))
            {
                bail!(
                    "--metrics: Codex's generated endpoint argument was duplicated or changed after planning"
                );
            }
        } else if touches_endpoint {
            bail!("--metrics: Codex's provider endpoint changed after planning");
        }
    }
    found.context("--metrics: Codex's generated endpoint argument is no longer available")
}

fn codex_websocket_value(table: &toml::Table, id: &str) -> Option<bool> {
    table
        .get("model_providers")?
        .get(id)?
        .get("supports_websockets")?
        .as_bool()
}

fn codex_id(profile: &str) -> String {
    format!("alc_{}", profile.replace('-', "_"))
}

fn opencode_id(kind: ProviderKind, profile: &str) -> String {
    match kind {
        ProviderKind::Anthropic => "anthropic".to_owned(),
        ProviderKind::Openai => "openai".to_owned(),
        ProviderKind::Openrouter => "openrouter".to_owned(),
        ProviderKind::Ollama => "ollama".to_owned(),
        _ => format!("alc-{profile}"),
    }
}

fn opencode_pointer(id: &str) -> String {
    format!(
        "/provider/{}/options/baseURL",
        id.replace('~', "~0").replace('/', "~1")
    )
}

fn opencode_document(spec: &LaunchSpec) -> Result<Value> {
    let contents = env_text(spec, "OPENCODE_CONFIG_CONTENT")?
        .context("--metrics is unavailable: OpenCode has no alc-managed inline config")?;
    serde_json::from_str(contents)
        .map_err(|_| anyhow::anyhow!("--metrics: OpenCode's inline config is not valid JSON"))
}

fn json_endpoint<'a>(document: &'a Value, pointer: &str) -> Result<Option<&'a str>> {
    document
        .pointer(pointer)
        .map(|value| {
            value
                .as_str()
                .context("--metrics: the endpoint field is not a string")
        })
        .transpose()
}

fn pointer_fields(pointer: &str) -> impl Iterator<Item = String> + '_ {
    pointer
        .split('/')
        .skip(1)
        .map(|field| field.replace("~1", "/").replace("~0", "~"))
}

fn check_json_objects(document: &Value, pointer: &str) -> Result<()> {
    let fields: Vec<_> = pointer_fields(pointer).collect();
    let mut current = document;
    for field in fields.iter().take(fields.len().saturating_sub(1)) {
        let object = current
            .as_object()
            .context("--metrics: the endpoint's JSON parent is not an object")?;
        match object.get(field) {
            Some(next) => current = next,
            None => return Ok(()),
        }
    }
    if !current.is_object() {
        bail!("--metrics: the endpoint's JSON parent is not an object");
    }
    Ok(())
}

fn insert_json_endpoint(document: &mut Value, pointer: &str, endpoint: &str) -> Result<()> {
    check_json_objects(document, pointer)?;
    let mut fields = pointer_fields(pointer).peekable();
    let mut current = document;
    while let Some(field) = fields.next() {
        let object = current
            .as_object_mut()
            .context("--metrics: the endpoint's JSON parent is not an object")?;
        if fields.peek().is_none() {
            object.insert(field, Value::String(endpoint.to_owned()));
            return Ok(());
        }
        current = object
            .entry(field)
            .or_insert_with(|| Value::Object(Map::new()));
    }
    bail!("--metrics: the endpoint JSON pointer is empty")
}

fn kimi_fields(profile: &str) -> Vec<String> {
    vec![
        "providers".to_owned(),
        format!("alc-{profile}"),
        "base_url".to_owned(),
    ]
}

fn parse_kimi(contents: &str) -> Result<toml::Value> {
    toml::from_str(contents)
        .map_err(|_| anyhow::anyhow!("--metrics: Kimi's temporary config is not valid TOML"))
}

fn toml_field_mut<'a>(
    document: &'a mut toml::Value,
    fields: &[String],
) -> Option<&'a mut toml::Value> {
    let mut current = document;
    for field in fields {
        current = current.get_mut(field)?;
    }
    Some(current)
}

/// Returns only real option values (not prompt text after `--`).
fn option_values<'a>(args: &'a [OsString], names: &[&str]) -> Result<Vec<&'a OsStr>> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if let Some(name) = names.iter().find(|name| arg == **name) {
            index += 1;
            values.push(
                args.get(index)
                    .map(OsString::as_os_str)
                    .with_context(|| format!("--metrics: {name} has no value"))?,
            );
        } else if let Some(text) = arg.to_str() {
            for name in names {
                if let Some(value) = text
                    .strip_prefix(*name)
                    .and_then(|rest| rest.strip_prefix('='))
                {
                    values.push(OsStr::new(value));
                    break;
                }
            }
        }
        index += 1;
    }
    Ok(values)
}

fn reject_options(spec: &LaunchSpec, names: &[&str], reason: &str) -> Result<()> {
    for arg in &spec.args {
        if arg == "--" {
            break;
        }
        if arg.to_str().is_some_and(|arg| {
            names.iter().any(|name| {
                arg == *name
                    || arg
                        .strip_prefix(*name)
                        .is_some_and(|rest| rest.starts_with('='))
            })
        }) {
            bail!("--metrics is unavailable: {reason}; the user's selection is left untouched");
        }
    }
    Ok(())
}

fn codex_configs(args: &[OsString]) -> Result<Vec<(usize, &str)>> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--" {
            break;
        }
        if args[index] == "--config" || args[index] == "-c" {
            index += 1;
            let value = args
                .get(index)
                .and_then(|arg| arg.to_str())
                .context("--metrics: Codex's --config argument is missing or not UTF-8")?;
            values.push((index, value));
        } else if let Some(arg) = args[index].to_str() {
            if let Some(value) = arg
                .strip_prefix("--config=")
                .or_else(|| arg.strip_prefix("-c="))
            {
                values.push((index, value));
            } else if let Some(value) = arg.strip_prefix("-c").filter(|value| !value.is_empty()) {
                values.push((index, value));
            }
        }
        index += 1;
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Credentials, Protocol, ReasoningEffort};
    use crate::launch::{self, LaunchOverrides};
    use serde_json::json;

    const KEY: &str = "metrics-fixture-not-a-real-key";
    const LOCAL: &str = "http://127.0.0.1:34567/cap/fixture-capability";
    const PROFILE: &str = "metrics-fixture";
    const HTTP_ONLY: &str = "model_providers.alc_metrics_fixture.supports_websockets=false";

    struct Fixture {
        root: tempfile::TempDir,
        store: Store,
    }

    impl Fixture {
        fn new(kind: ProviderKind, keyed: bool) -> Self {
            let root = tempfile::tempdir().unwrap();
            let mut provider = Provider::for_kind(kind);
            // Host exports must not select credentials for a test profile.
            provider.api_key_env = None;
            provider.model = "fixture-model".to_owned();
            provider.reasoning_effort = Some(ReasoningEffort::Medium);
            if kind == ProviderKind::Codex {
                provider.codex_home = Some(
                    root.path()
                        .join("missing-codex-home")
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
            }
            if kind == ProviderKind::Custom {
                provider.base_url = Some("https://gateway.example.com/api/v1".to_owned());
            }
            let mut config = Config::default();
            config.providers.insert(PROFILE.to_owned(), provider);
            let mut credentials = Credentials::default();
            if keyed {
                credentials
                    .api_keys
                    .insert(PROFILE.to_owned(), KEY.to_owned());
            }
            let store = Store {
                dir: root.path().join("not-written"),
                config,
                credentials,
            };
            Self { root, store }
        }

        fn build(&self, agent: Agent, args: &[&str]) -> LaunchSpec {
            let args: Vec<_> = args.iter().map(OsString::from).collect();
            launch::build(
                &self.store,
                agent,
                Some(PROFILE),
                &args,
                &LaunchOverrides::default(),
            )
            .unwrap()
        }

        fn provider_mut(&mut self) -> &mut Provider {
            self.store.config.providers.get_mut(PROFILE).unwrap()
        }
    }

    #[test]
    fn claude_keyed_plan_is_durable_secret_free_and_changes_only_endpoint() {
        let fixture = Fixture::new(ProviderKind::Anthropic, true);
        let mut spec = fixture.build(
            Agent::Claude,
            &[
                "--settings",
                r#"{"permissions":{"allow":["Read"]},"env":{"CUSTOM_SETTING":"keep"}}"#,
                "--model",
                "user-model",
            ],
        );
        let before = spec.clone();
        let route = plan(&spec, &fixture.store).unwrap();
        assert!(route.durable);
        assert_eq!(route.upstream, "https://api.anthropic.com");
        assert_eq!(route.key_digests, vec![key_digest(KEY)]);
        let encoded = serde_json::to_string(&route).unwrap();
        assert!(!encoded.contains(KEY));
        assert_eq!(
            serde_json::from_str::<ForwardPlan>(&encoded).unwrap(),
            route
        );
        assert!(!fixture.store.dir.exists());
        assert_eq!(spec, before);

        apply(&mut spec, LOCAL, &route).unwrap();
        let mut expected = before;
        expected.settings_plan.as_mut().unwrap().document["env"]["ANTHROPIC_BASE_URL"] =
            json!(LOCAL);
        assert_eq!(spec, expected);
        assert!(!fixture.store.dir.exists());
    }

    #[test]
    fn claude_openrouter_retains_its_messages_root_convention() {
        let fixture = Fixture::new(ProviderKind::Openrouter, true);
        let mut spec = fixture.build(Agent::Claude, &[]);
        let route = plan(&spec, &fixture.store).unwrap();
        assert_eq!(route.upstream, "https://openrouter.ai/api");
        let helper = spec.settings_plan.as_ref().unwrap().document["apiKeyHelper"].clone();
        apply(&mut spec, LOCAL, &route).unwrap();
        let settings = &spec.settings_plan.as_ref().unwrap().document;
        assert_eq!(settings["env"]["ANTHROPIC_BASE_URL"], LOCAL);
        assert_eq!(settings["apiKeyHelper"], helper);
    }

    #[test]
    fn claude_user_endpoint_helper_and_auth_overrides_are_not_rerouted() {
        let fixture = Fixture::new(ProviderKind::Anthropic, true);
        for settings in [
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://other.example.com"}}"#,
            r#"{"apiKeyHelper":"my-helper"}"#,
            r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"user-token"}}"#,
            r#"{"env":{"CLAUDE_CODE_USE_BEDROCK":"1"}}"#,
        ] {
            let spec = fixture.build(Agent::Claude, &["--settings", settings]);
            let before = spec.clone();
            assert!(
                plan(&spec, &fixture.store)
                    .unwrap_err()
                    .to_string()
                    .contains("--metrics")
            );
            assert_eq!(spec, before);
        }
    }

    #[test]
    fn durable_queries_and_endpoint_credentials_are_refused_without_echoing_secrets() {
        for endpoint in [
            "https://gateway.example.com?api_key=never-echo-this",
            "https://user:never-echo-this@gateway.example.com",
            "https://gateway.example.com#never-echo-this",
            "file:///never-echo-this",
        ] {
            let mut fixture = Fixture::new(ProviderKind::Anthropic, true);
            fixture.provider_mut().base_url = Some(endpoint.to_owned());
            let spec = fixture.build(Agent::Claude, &[]);
            let error = plan(&spec, &fixture.store).unwrap_err().to_string();
            assert!(error.contains("--metrics"));
            assert!(!error.contains("never-echo-this"));
        }
    }

    #[test]
    fn ephemeral_query_is_forwarded_but_never_exposed_by_plan_debug() {
        let mut fixture = Fixture::new(ProviderKind::Openai, true);
        fixture.provider_mut().base_url =
            Some("https://gateway.example.com/v1?private=never-echo-this".to_owned());
        let spec = fixture.build(Agent::Codex, &["--config", HTTP_ONLY]);
        let route = plan(&spec, &fixture.store).unwrap();
        assert!(route.upstream.contains("never-echo-this"));
        assert!(!format!("{route:?}").contains("never-echo-this"));
        assert!(!format!("{:?}", route.patch).contains("never-echo-this"));
    }

    #[test]
    fn native_and_already_bridged_launches_remain_unchanged() {
        let mut native = Fixture::new(ProviderKind::Anthropic, false);
        native.provider_mut().auth = AuthStyle::Native;
        let spec = native.build(Agent::Claude, &[]);
        let before = spec.clone();
        assert!(
            plan(&spec, &native.store)
                .unwrap_err()
                .to_string()
                .contains("native OAuth")
        );
        assert_eq!(spec, before);

        let bridged = Fixture::new(ProviderKind::Codex, false);
        let spec = bridged.build(Agent::Copilot, &[]);
        let before = spec.clone();
        assert!(spec.is_bridged());
        assert!(
            plan(&spec, &bridged.store)
                .unwrap_err()
                .to_string()
                .contains("already observed")
        );
        assert_eq!(spec, before);

        let spec = bridged.build(Agent::Codex, &[]);
        assert!(plan(&spec, &bridged.store).is_err());
        let local = Fixture::new(ProviderKind::Ollama, false);
        let spec = local.build(Agent::Claude, &[]);
        assert!(
            plan(&spec, &local.store)
                .unwrap_err()
                .to_string()
                .contains("keyless endpoint")
        );
    }

    #[test]
    fn codex_patch_preserves_argument_order_models_auth_and_explicit_http_only() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let websocket = HTTP_ONLY;
        let mut spec = fixture.build(
            Agent::Codex,
            &[
                "--model",
                "user-model",
                "--config",
                websocket,
                "exec",
                "--",
                "prompt stays here",
            ],
        );
        let before = spec.clone();
        let route = plan(&spec, &fixture.store).unwrap();
        assert!(!route.durable);
        assert_eq!(route.key_digests, vec![key_digest(KEY)]);
        assert!(
            route
                .coverage_reason
                .as_deref()
                .unwrap()
                .contains("WebSocket")
        );
        let EndpointPatch::CodexConfig { argument, .. } = &route.patch else {
            panic!("config patch")
        };
        let mut expected = before;
        expected.args[*argument] = OsString::from(format!(
            "model_providers.alc_metrics_fixture.base_url={}",
            toml_string(LOCAL)
        ));
        apply(&mut spec, LOCAL, &route).unwrap();
        assert_eq!(spec, expected);
        assert!(spec.args.iter().any(|arg| arg == websocket));
    }

    #[test]
    fn codex_apply_resolves_endpoint_after_shared_permission_flags_are_prepended() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let mut spec = fixture.build(Agent::Codex, &["--config", HTTP_ONLY]);
        let route = plan(&spec, &fixture.store).unwrap();
        let EndpointPatch::CodexConfig { argument, original } = &route.patch else {
            panic!("config patch")
        };
        // The shared-launch permission seam inserts flags ahead of these
        // arguments after metrics planning; keep the private remote API out
        // of this module's fixture and reproduce the argv shift directly.
        spec.args.splice(
            0..0,
            [
                OsString::from("--ask-for-approval"),
                OsString::from("on-request"),
            ],
        );
        let actual = spec
            .args
            .iter()
            .position(|arg| arg == original.as_str())
            .unwrap();
        assert_ne!(actual, *argument);
        let mut expected = spec.clone();
        expected.args[actual] = OsString::from(format!(
            "model_providers.alc_metrics_fixture.base_url={}",
            toml_string(LOCAL)
        ));
        apply(&mut spec, LOCAL, &route).unwrap();
        assert_eq!(spec, expected);
    }

    #[test]
    fn codex_apply_accepts_an_intact_generated_pair_moved_with_unrelated_args() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let mut spec = fixture.build(Agent::Codex, &["--config", HTTP_ONLY]);
        let route = plan(&spec, &fixture.store).unwrap();
        let EndpointPatch::CodexConfig { argument, .. } = route.patch.clone() else {
            panic!("config patch")
        };
        let endpoint_pair: Vec<_> = spec.args.drain(argument - 1..=argument).collect();
        spec.args.extend(endpoint_pair);
        let mut expected = spec.clone();
        let actual = spec.args.len() - 1;
        expected.args[actual] = OsString::from(format!(
            "model_providers.alc_metrics_fixture.base_url={}",
            toml_string(LOCAL)
        ));
        apply(&mut spec, LOCAL, &route).unwrap();
        assert_eq!(spec, expected);
    }

    #[test]
    fn codex_apply_refuses_changed_or_competing_endpoint_pairs_without_mutation() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let base = fixture.build(Agent::Codex, &["--config", HTTP_ONLY]);
        let route = plan(&base, &fixture.store).unwrap();
        let EndpointPatch::CodexConfig { argument, original } = &route.patch else {
            panic!("config patch")
        };
        let mut changed = base.clone();
        changed.args[*argument] = OsString::from(
            r#"model_providers.alc_metrics_fixture.base_url="https://different.example.com/v1""#,
        );
        let before = changed.clone();
        assert!(apply(&mut changed, LOCAL, &route).is_err());
        assert_eq!(changed, before);
        for extra in [
            original.as_str(),
            r#"model_providers.alc_metrics_fixture.base_url="https://different.example.com/v1""#,
            r#""model_providers"."alc_metrics_fixture"."base_url"="https://different.example.com/v1""#,
        ] {
            let mut duplicate = base.clone();
            duplicate
                .args
                .extend([OsString::from("--config"), OsString::from(extra)]);
            let before = duplicate.clone();
            assert!(apply(&mut duplicate, LOCAL, &route).is_err());
            assert_eq!(duplicate, before);
        }
    }

    #[test]
    fn codex_websocket_true_unknown_and_inherited_transports_are_refused_without_changes() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        for args in [
            vec![],
            vec![
                "--config",
                "model_providers.alc_metrics_fixture.supports_websockets=true",
            ],
            vec![
                "--config",
                r#"model_providers.alc_metrics_fixture.supports_websockets="false""#,
            ],
            vec![
                "--config",
                HTTP_ONLY,
                "-c",
                "model_providers.alc_metrics_fixture.supports_websockets=true",
            ],
            vec![
                "--config",
                "model_providers.alc_metrics_fixture.supports_websockets=true",
                "-c",
                HTTP_ONLY,
            ],
            vec!["--", "--config", HTTP_ONLY],
        ] {
            let spec = fixture.build(Agent::Codex, &args);
            let before = spec.clone();
            let error = plan(&spec, &fixture.store).unwrap_err().to_string();
            assert!(error.contains("--metrics"));
            assert!(error.contains("supports_websockets=false"));
            assert_eq!(spec, before);
        }
    }

    #[test]
    fn codex_quoted_toml_false_key_is_accepted_and_preserved() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let quoted = r#""model_providers"."alc_metrics_fixture"."supports_websockets" = false"#;
        let mut spec = fixture.build(Agent::Codex, &["-c", quoted]);
        let route = plan(&spec, &fixture.store).unwrap();
        let before = spec.args.clone();
        apply(&mut spec, LOCAL, &route).unwrap();
        assert!(spec.args.iter().any(|arg| arg == quoted));
        let EndpointPatch::CodexConfig { argument, .. } = route.patch else {
            panic!("config patch")
        };
        for (index, (actual, original)) in spec.args.iter().zip(&before).enumerate() {
            if index != argument {
                assert_eq!(actual, original);
            }
        }
    }

    #[test]
    fn codex_user_provider_and_endpoint_overrides_fail_instead_of_rerouting() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        for override_arg in [
            r#"model_provider="other""#,
            r#"model_providers.alc_metrics_fixture.base_url="https://other.example.com""#,
            r#"model_providers.alc_metrics_fixture.env_key="OTHER_KEY""#,
            r#""model_providers"."alc_metrics_fixture"."base_url"="https://other.example.com""#,
        ] {
            let spec = fixture.build(Agent::Codex, &["--config", HTTP_ONLY, "-c", override_arg]);
            let before = spec.clone();
            assert!(plan(&spec, &fixture.store).is_err());
            assert_eq!(spec, before);
        }
        let generated = format!(
            "model_providers.alc_metrics_fixture.base_url={}",
            toml_string("https://api.openai.com/v1")
        );
        let spec = fixture.build(
            Agent::Codex,
            &["--config", HTTP_ONLY, "--config", &generated],
        );
        assert!(plan(&spec, &fixture.store).is_err());
    }

    #[test]
    fn opencode_custom_patch_keeps_package_auth_models_and_unrelated_json() {
        let fixture = Fixture::new(ProviderKind::Custom, true);
        let mut spec = fixture.build(Agent::Opencode, &[]);
        let mut inline = opencode_document(&spec).unwrap();
        inline["tools"] = json!({ "custom": true });
        inline["provider"]["alc-metrics-fixture"]["options"]["headers"] =
            json!({ "custom": "keep" });
        spec.env.insert(
            OsString::from("OPENCODE_CONFIG_CONTENT"),
            OsString::from(serde_json::to_string(&inline).unwrap()),
        );
        let before = spec.clone();
        let route = plan(&spec, &fixture.store).unwrap();
        assert_eq!(route.upstream, "https://gateway.example.com/api/v1");
        assert_eq!(route.key_digests, vec![key_digest(KEY)]);
        assert_eq!(spec, before);
        apply(&mut spec, LOCAL, &route).unwrap();
        inline["provider"]["alc-metrics-fixture"]["options"]["baseURL"] = json!(LOCAL);
        assert_eq!(opencode_document(&spec).unwrap(), inline);
        spec.env.insert(
            OsString::from("OPENCODE_CONFIG_CONTENT"),
            before.env[OsStr::new("OPENCODE_CONFIG_CONTENT")].clone(),
        );
        assert_eq!(spec, before);
    }

    #[test]
    fn opencode_builtin_patch_adds_only_endpoint_without_choosing_an_sdk() {
        let fixture = Fixture::new(ProviderKind::Openrouter, true);
        let mut spec = fixture.build(Agent::Opencode, &[]);
        let before = spec.clone();
        let mut expected = opencode_document(&spec).unwrap();
        assert!(expected.get("provider").is_none());
        // Applying the missing-object patch is independent of host SDK env
        // defaults; planning separately refuses an inherited endpoint override.
        let route = ForwardPlan {
            upstream: "https://openrouter.ai/api/v1".to_owned(),
            key_digests: vec![key_digest(KEY)],
            patch: EndpointPatch::OpenCodeConfig {
                pointer: "/provider/openrouter/options/baseURL".to_owned(),
                original: None,
            },
            durable: false,
            coverage_reason: None,
        };
        apply(&mut spec, LOCAL, &route).unwrap();
        expected["provider"] = json!({ "openrouter": { "options": { "baseURL": LOCAL } } });
        assert_eq!(opencode_document(&spec).unwrap(), expected);
        spec.env.insert(
            OsString::from("OPENCODE_CONFIG_CONTENT"),
            before.env[OsStr::new("OPENCODE_CONFIG_CONTENT")].clone(),
        );
        assert_eq!(spec, before);
    }

    #[test]
    fn opencode_missing_key_and_user_model_provider_are_unavailable() {
        let missing = Fixture::new(ProviderKind::Openai, false);
        let spec = missing.build(Agent::Opencode, &[]);
        assert!(plan(&spec, &missing.store).is_err());
        let fixture = Fixture::new(ProviderKind::Custom, true);
        let spec = fixture.build(Agent::Opencode, &["--model", "other/model"]);
        assert!(
            plan(&spec, &fixture.store)
                .unwrap_err()
                .to_string()
                .contains("different provider")
        );
    }

    #[test]
    fn copilot_and_qwen_patch_only_the_endpoint_not_auth_or_protocol() {
        for agent in [Agent::Copilot, Agent::Qwen] {
            for kind in [ProviderKind::Anthropic, ProviderKind::Openai] {
                let fixture = Fixture::new(kind, true);
                let mut spec = fixture.build(agent, &["--model", "user-model"]);
                let before = spec.clone();
                let route = plan(&spec, &fixture.store).unwrap();
                let EndpointPatch::Environment { name, .. } = &route.patch else {
                    panic!("env patch")
                };
                let mut expected = before;
                expected
                    .env
                    .insert(OsString::from(name), OsString::from(LOCAL));
                apply(&mut spec, LOCAL, &route).unwrap();
                assert_eq!(spec, expected);
            }
        }
    }

    #[test]
    fn qwen_explicit_auth_type_and_google_native_branch_are_unavailable() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let spec = fixture.build(Agent::Qwen, &["--auth-type", "qwen-oauth"]);
        let before = spec.clone();
        assert!(
            plan(&spec, &fixture.store)
                .unwrap_err()
                .to_string()
                .contains("--auth-type")
        );
        assert_eq!(spec, before);
        let google = Fixture::new(ProviderKind::Google, true);
        let spec = google.build(Agent::Qwen, &[]);
        assert!(
            plan(&spec, &google.store)
                .unwrap_err()
                .to_string()
                .contains("Gemini")
        );
    }

    #[test]
    fn goose_chat_split_normalizes_original_base_and_keeps_auth_provider_model() {
        for (base, normalized) in [
            (
                "https://gateway.example.com/api/paas/v4",
                "https://gateway.example.com/api/paas/v4",
            ),
            (
                "https://gateway.example.com",
                "https://gateway.example.com/v1",
            ),
        ] {
            let mut fixture = Fixture::new(ProviderKind::Custom, true);
            fixture.provider_mut().base_url = Some(base.to_owned());
            fixture.provider_mut().protocol = Protocol::OpenaiChat;
            let mut spec = fixture.build(Agent::Goose, &["session"]);
            let before = spec.clone();
            let route = plan(&spec, &fixture.store).unwrap();
            assert_eq!(route.upstream, normalized);
            let mut expected = before;
            expected.env.insert(
                OsString::from("OPENAI_HOST"),
                OsString::from("http://127.0.0.1:34567"),
            );
            expected.env.insert(
                OsString::from("OPENAI_BASE_PATH"),
                OsString::from("cap/fixture-capability/chat/completions"),
            );
            apply(&mut spec, LOCAL, &route).unwrap();
            assert_eq!(spec, expected);
        }
    }

    #[test]
    fn goose_split_chat_query_and_fragment_are_refused_without_reinterpretation() {
        for endpoint in [
            "https://gateway.example.com/v1?key=never-echo-this",
            "https://gateway.example.com/v1#never-echo-this",
            "https://gateway.example.com?key=never-echo-this",
        ] {
            let mut fixture = Fixture::new(ProviderKind::Custom, true);
            fixture.provider_mut().base_url = Some(endpoint.to_owned());
            fixture.provider_mut().protocol = Protocol::OpenaiChat;
            let spec = fixture.build(Agent::Goose, &["session"]);
            let before = spec.clone();
            let error = plan(&spec, &fixture.store).unwrap_err().to_string();
            assert!(error.contains("--metrics"));
            assert!(error.contains("query or fragment"));
            assert!(!error.contains("never-echo-this"));
            assert_eq!(spec, before);
        }
    }

    #[test]
    fn goose_anthropic_host_changes_only_endpoint_and_native_integrations_fail() {
        let mut fixture = Fixture::new(ProviderKind::Anthropic, true);
        fixture.provider_mut().base_url = Some("https://gateway.example.com/anthropic".to_owned());
        let mut spec = fixture.build(Agent::Goose, &[]);
        let route = plan(&spec, &fixture.store).unwrap();
        let mut expected = spec.clone();
        expected
            .env
            .insert(OsString::from("ANTHROPIC_HOST"), OsString::from(LOCAL));
        apply(&mut spec, LOCAL, &route).unwrap();
        assert_eq!(spec, expected);
        for kind in [ProviderKind::Openrouter, ProviderKind::Ollama] {
            let fixture = Fixture::new(kind, true);
            let spec = fixture.build(Agent::Goose, &[]);
            assert!(plan(&spec, &fixture.store).is_err());
        }
    }

    fn kimi_fixture() -> (Fixture, LaunchSpec) {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let path = fixture.root.path().join("unwritten-kimi.toml");
        // An explicit config flag skips Kimi's host config read. Queue the
        // equivalent generated file entirely in memory for endpoint tests.
        let mut spec = fixture.build(Agent::Kimi, &["--config-file", path.to_str().unwrap()]);
        let contents = format!(
            "default_model = 'alc-metrics-fixture'\ncustom = 'keep'\n\n[providers.alc-metrics-fixture]\ntype = 'openai_responses'\nbase_url = 'https://api.openai.com/v1'\napi_key = '{KEY}'\n\n[models.alc-metrics-fixture]\nprovider = 'alc-metrics-fixture'\nmodel = 'fixture-model'\ncontext_window = 1234\n\n[providers.unrelated]\ntype = 'anthropic'\nbase_url = 'https://other.example.com'\napi_key = 'another-fixture-key'\n"
        );
        spec.mark_secret_value(KEY);
        spec.file_setup.push(FileSetup::WriteTemp {
            path,
            contents,
            secret: true,
            cleanup: true,
        });
        (fixture, spec)
    }

    #[test]
    fn kimi_toml_patch_preserves_type_model_key_and_unrelated_tables_without_writing() {
        let (fixture, mut spec) = kimi_fixture();
        let before = spec.clone();
        let route = plan(&spec, &fixture.store).unwrap();
        assert_eq!(route.key_digests, vec![key_digest(KEY)]);
        assert!(!serde_json::to_string(&route).unwrap().contains(KEY));
        assert_eq!(spec, before);
        let FileSetup::WriteTemp { path, contents, .. } = &before.file_setup[0] else {
            panic!("temp file")
        };
        let path = path.clone();
        let mut expected = parse_kimi(contents).unwrap();
        expected["providers"]["alc-metrics-fixture"]["base_url"] =
            toml::Value::String(LOCAL.to_owned());
        apply(&mut spec, LOCAL, &route).unwrap();
        let FileSetup::WriteTemp {
            contents,
            secret,
            cleanup,
            ..
        } = &spec.file_setup[0]
        else {
            panic!("temp file")
        };
        assert_eq!(parse_kimi(contents).unwrap(), expected);
        assert!(*secret && *cleanup);
        assert_eq!(spec.args, before.args);
        assert_eq!(spec.secret_values, before.secret_values);
        assert!(!path.exists());
        assert!(!fixture.store.dir.exists());
    }

    #[test]
    fn kimi_explicit_user_config_has_no_patch_and_pi_shared_config_is_deferred() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let path = fixture.root.path().join("user-config.toml");
        let spec = fixture.build(Agent::Kimi, &["--config-file", path.to_str().unwrap()]);
        assert!(
            plan(&spec, &fixture.store)
                .unwrap_err()
                .to_string()
                .contains("user config")
        );
        assert!(!path.exists());
        let spec = fixture.build(Agent::Pi, &[]);
        let before = spec.clone();
        assert!(
            plan(&spec, &fixture.store)
                .unwrap_err()
                .to_string()
                .contains("persistent shared models.json")
        );
        assert_eq!(spec, before);
    }

    #[test]
    fn keyless_sdk_placeholder_and_real_key_on_auth_none_follow_actual_launch() {
        let fixture = Fixture::new(ProviderKind::Custom, false);
        let mut spec = fixture.build(Agent::Qwen, &[]);
        let route = plan(&spec, &fixture.store).unwrap();
        assert_eq!(route.key_digests, vec![key_digest("alc")]);
        apply(&mut spec, LOCAL, &route).unwrap();
        assert_eq!(env_text(&spec, "OPENAI_API_KEY").unwrap(), Some("alc"));

        let fixture = Fixture::new(ProviderKind::Custom, true);
        assert_eq!(
            fixture.store.config.providers[PROFILE].auth,
            AuthStyle::None
        );
        let spec = fixture.build(Agent::Codex, &["--config", HTTP_ONLY]);
        let route = plan(&spec, &fixture.store).unwrap();
        assert_eq!(route.key_digests, vec![key_digest(KEY)]);
    }

    #[test]
    fn stale_endpoint_patch_and_non_loopback_target_leave_spec_unchanged() {
        let fixture = Fixture::new(ProviderKind::Openai, true);
        let mut spec = fixture.build(Agent::Copilot, &[]);
        let route = plan(&spec, &fixture.store).unwrap();
        let before = spec.clone();
        assert!(apply(&mut spec, "https://outside.example.com/cap/token", &route).is_err());
        assert_eq!(spec, before);
        spec.env.insert(
            OsString::from("COPILOT_PROVIDER_BASE_URL"),
            OsString::from("https://user.example.com"),
        );
        let changed = spec.clone();
        assert!(apply(&mut spec, LOCAL, &route).is_err());
        assert_eq!(spec, changed);
    }
}
