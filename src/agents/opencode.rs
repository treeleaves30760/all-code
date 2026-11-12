use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::config::{AuthStyle, Protocol, Provider, ProviderKind, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, bridge_plan, has_model_override,
    openai_style_base_url,
};

pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if let Some(plan) = bridge_plan(provider, overrides, BridgeApi::Responses)? {
        spec.bridge = Some(plan);
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    let (provider_id, needs_custom_config) = match provider.kind {
        ProviderKind::Anthropic => ("anthropic".to_owned(), custom_kind_url(provider)),
        ProviderKind::Openai => ("openai".to_owned(), custom_kind_url(provider)),
        ProviderKind::Openrouter => ("openrouter".to_owned(), custom_kind_url(provider)),
        // OpenCode documents Ollama as an explicit OpenAI-compatible provider.
        // Always inject it so a fresh OpenCode install does not depend on local
        // provider discovery having run first.
        ProviderKind::Ollama => ("ollama".to_owned(), true),
        // The seven OpenAI-chat presets have no bespoke OpenCode provider id
        // of their own, so they are injected the same way as vLLM/Custom.
        ProviderKind::Vllm
        | ProviderKind::Llamacpp
        | ProviderKind::Deepseek
        | ProviderKind::Moonshot
        | ProviderKind::Zai
        | ProviderKind::Minimax
        | ProviderKind::Groq
        | ProviderKind::Xai
        | ProviderKind::Google
        | ProviderKind::Custom => (format!("alc-{profile_name}"), true),
        ProviderKind::Codex => unreachable!(),
        // Taken by `bridge_plan` above; said here too so a path that skips
        // it gets an error rather than a broken config.
        ProviderKind::Antigravity => bail!(
            "provider '{profile_name}' is an Antigravity login, which alc reaches only through its \
             bridge; this is an alc bug - please report it"
        ),
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
        spec.set_secret_env(env_name, key.clone());
        spec.set_secret_env("ALC_PROVIDER_API_KEY", key.clone());
    }

    let model = overrides.model.as_deref().unwrap_or(&provider.model);
    let model_reference = format!("{provider_id}/{model}");
    let mut inline = inline_document(&spec.env);
    if !has_model_override(passthrough) {
        inline["model"] = Value::String(model_reference);
    }

    if needs_custom_config {
        let base_url = openai_style_base_url(provider)
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?;
        let package = match (provider.kind, provider.protocol) {
            // llama-server's streamed Responses events are not the ones the
            // OpenAI SDK waits for (OpenCode fails with "text part … not
            // found"), and its Chat Completions route is the one it has always
            // had, so OpenCode takes that one as it does on Ollama.
            (ProviderKind::Ollama | ProviderKind::Llamacpp, _) => "@ai-sdk/openai-compatible",
            (_, Protocol::AnthropicMessages) => "@ai-sdk/anthropic",
            (_, Protocol::OpenaiResponses | Protocol::Dual) => "@ai-sdk/openai",
            (_, Protocol::OpenaiChat) => "@ai-sdk/openai-compatible",
            (_, Protocol::CodexNative) => {
                bail!("provider '{profile_name}' uses codex-native, which OpenCode cannot load")
            }
            (_, Protocol::AntigravityNative) => bail!(
                "provider '{profile_name}' uses antigravity-native, which OpenCode cannot load"
            ),
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

/// Wires the bundled bridge (listening on `base_url`) into an OpenCode
/// session as an `@ai-sdk/openai` provider named `alc-codex`, pointed at the
/// loopback bridge instead of api.openai.com - on the Codex login or agy's.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    let models: Map<String, Value> = if plan.options.is_empty() {
        let mut fallback = Map::new();
        fallback.insert(plan.model.clone(), json!({ "name": plan.model }));
        fallback
    } else {
        plan.options
            .iter()
            .map(|model| (model.id.clone(), json!({ "name": model.name })))
            .collect()
    };

    // Merged rather than replaced: a shared session's `default_agent` was
    // written here before the bridge's port was known.
    let mut inline = inline_document(&spec.env);
    if !has_model_override(&spec.args) {
        inline["model"] = Value::String(format!("alc-codex/{}", plan.model));
    }
    inline["provider"] = json!({
        "alc-codex": {
            "npm": "@ai-sdk/openai",
            "name": if spec.provider_kind == ProviderKind::Antigravity {
                "alc: Antigravity login"
            } else {
                "alc: Codex subscription"
            },
            "options": {
                // The loopback bridge ignores auth entirely; the SDK still
                // requires some non-empty apiKey value to be configured.
                "baseURL": format!("{base_url}/v1"),
                "apiKey": "alc",
            },
            "models": models,
        }
    });

    spec.env.insert(
        OsString::from("OPENCODE_CONFIG_CONTENT"),
        OsString::from(serde_json::to_string(&inline)?),
    );
    Ok(())
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

const INLINE_CONFIG: &str = "OPENCODE_CONFIG_CONTENT";

/// The inline config document a spec already carries, or a fresh one.
fn inline_document(env: &BTreeMap<OsString, OsString>) -> Value {
    env.get(&OsString::from(INLINE_CONFIG))
        .and_then(|text| serde_json::from_str::<Value>(&text.to_string_lossy()).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({ "$schema": "https://opencode.ai/config.json" }))
}

/// Sets one top-level key of the inline config, keeping every other key.
pub(crate) fn set_inline_key(env: &mut BTreeMap<OsString, OsString>, key: &str, value: &str) {
    let mut inline = inline_document(env);
    inline[key] = Value::String(value.to_owned());
    env.insert(
        OsString::from(INLINE_CONFIG),
        OsString::from(inline.to_string()),
    );
}

/// OpenCode 2 runs every session inside one background service by default,
/// and a service that is already up never sees this launch's environment:
/// the provider, model and key alc injects through `OPENCODE_CONFIG_CONTENT`
/// would silently be someone else's. `--standalone` gives the session a
/// private server started with this environment, which is what every
/// OpenCode 1 session was. OpenCode 1 rejects the flag, so the installed
/// binary is asked its version first.
pub(crate) fn finish_for_binary(spec: &mut LaunchSpec, program: &Path) {
    if !wants_private_server(&spec.args) {
        return;
    }
    if major_version(program).is_some_and(|major| major >= 2) {
        spec.args.insert(0, OsString::from("--standalone"));
    }
}

/// Commands that never open a session (or manage the service itself) are
/// left exactly as typed, as is a launch that already picks its server.
fn wants_private_server(args: &[OsString]) -> bool {
    const NO_SESSION: &[&str] = &[
        "upgrade",
        "update",
        "uninstall",
        "acp",
        "api",
        "debug",
        "auth",
        "mcp",
        "plugin",
        "service",
        "reload",
        "pair",
        "serve",
        "web",
        "completion",
        "-h",
        "--help",
        "-v",
        "--version",
    ];
    if args.iter().any(|arg| {
        let arg = arg.to_string_lossy();
        arg == "--standalone" || arg == "--server" || arg.starts_with("--server=")
    }) {
        return false;
    }
    args.first()
        .is_none_or(|first| !NO_SESSION.contains(&first.to_string_lossy().as_ref()))
}

/// `opencode v2.2.7` -> 2. None when the binary does not answer promptly
/// with something that reads as a version.
fn major_version(program: &Path) -> Option<u64> {
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    parse_major(&String::from_utf8_lossy(&output.stdout))
}

fn parse_major(text: &str) -> Option<u64> {
    let version = text.split_whitespace().next_back()?;
    version
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_major_version_out_of_both_spellings() {
        assert_eq!(parse_major("opencode v2.2.7\n"), Some(2));
        assert_eq!(parse_major("1.14.3"), Some(1));
        assert_eq!(parse_major("not a version"), None);
        assert_eq!(parse_major(""), None);
    }

    #[test]
    fn only_session_launches_get_a_private_server() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(wants_private_server(&args(&[])));
        assert!(wants_private_server(&args(&["run", "hello"])));
        assert!(wants_private_server(&args(&["mini"])));
        assert!(wants_private_server(&args(&["models"])));
        assert!(wants_private_server(&args(&["."])));
        assert!(!wants_private_server(&args(&["service", "stop"])));
        assert!(!wants_private_server(&args(&["--version"])));
        assert!(!wants_private_server(&args(&["--standalone"])));
        assert!(!wants_private_server(&args(&["--server", "http://x"])));
    }

    #[test]
    fn a_default_agent_survives_the_provider_config_written_after_it() {
        let mut env = BTreeMap::new();
        set_inline_key(&mut env, "default_agent", "plan");
        let mut spec = LaunchSpec::for_test();
        spec.env = env;
        let plan = BridgePlan {
            model: "gpt-6.1-sol".to_owned(),
            effort: None,
            context_window: None,
            options: Vec::new(),
            api: BridgeApi::Responses,
            upstream: None,
        };
        apply_bridge(&mut spec, "http://127.0.0.1:1", &plan).unwrap();
        let inline: Value =
            serde_json::from_str(&spec.env[&OsString::from(INLINE_CONFIG)].to_string_lossy())
                .unwrap();
        assert_eq!(inline["default_agent"], "plan");
        assert_eq!(inline["model"], "alc-codex/gpt-6.1-sol");
        assert!(inline["provider"]["alc-codex"].is_object());
    }
}
