//! Best-effort questions for a local Ollama server.
//!
//! Every query here degrades to `None`: alc launches an agent (and prints a
//! dry run) the same way whether or not the server is up, so these answers
//! only ever add information — a context window for Claude Code, a line in
//! `alc doctor` — and never block or fail a launch.

use std::time::Duration;

use serde_json::{Value, json};

use crate::config::Provider;

/// Long enough for a busy local server to answer a metadata call, short
/// enough that a stopped server does not make `alc claude` feel slow.
const TIMEOUT: Duration = Duration::from_millis(1500);

/// The Ollama HTTP API root: the profile's base URL without the `/v1` suffix
/// an OpenAI-style client would add.
pub fn api_root(provider: &Provider) -> Option<String> {
    let base = provider.effective_base_url()?.trim_end_matches('/');
    Some(base.strip_suffix("/v1").unwrap_or(base).to_owned())
}

/// What the Claude launcher and `alc doctor` want to know about one model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelFacts {
    /// The window the running server allocated when the model is loaded
    /// (`/api/ps`); otherwise the model's own limit from `/api/show`, or an
    /// explicit `num_ctx` in its Modelfile.
    pub context_length: Option<u64>,
    /// `/api/show` capabilities such as `completion`, `tools`, `thinking`.
    pub capabilities: Vec<String>,
}

impl ModelFacts {
    /// Coding agents drive the model through tool calls, so a model without
    /// the `tools` capability cannot run any of them.
    pub fn supports_tools(&self) -> bool {
        self.capabilities.iter().any(|name| name == "tools")
    }
}

/// The server version from `/api/version`; `None` when it is unreachable.
pub fn server_version(root: &str) -> Option<String> {
    get_json(&format!("{root}/api/version"))?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// Facts about a pulled model; `None` when the server is unreachable or the
/// model has not been pulled.
pub fn model_facts(root: &str, model: &str) -> Option<ModelFacts> {
    let show = post_json(&format!("{root}/api/show"), &json!({ "model": model }))?;
    let mut facts = facts_from_show(&show);
    if let Some(loaded) =
        get_json(&format!("{root}/api/ps")).and_then(|ps| context_length_from_ps(&ps, model))
    {
        facts.context_length = Some(loaded);
    }
    Some(facts)
}

/// The context window Claude Code should assume for `model` on this profile.
pub fn context_window(provider: &Provider, model: &str) -> Option<u64> {
    let root = api_root(provider)?;
    model_facts(&root, model)?.context_length
}

pub(crate) fn facts_from_show(show: &Value) -> ModelFacts {
    let capabilities = show
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    // A Modelfile `num_ctx` is what the server will actually allocate; the
    // architecture's context_length is only the ceiling.
    let context_length = num_ctx_parameter(show).or_else(|| {
        show.get("model_info")?
            .as_object()?
            .iter()
            .find(|(key, _)| key.ends_with(".context_length"))
            .and_then(|(_, value)| value.as_u64())
    });
    ModelFacts {
        context_length,
        capabilities,
    }
}

fn num_ctx_parameter(show: &Value) -> Option<u64> {
    show.get("parameters")?
        .as_str()?
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            (parts.next()? == "num_ctx").then(|| parts.next()?.parse().ok())?
        })
        .next()
}

/// The context length the server allocated for `model`, when it is loaded.
pub(crate) fn context_length_from_ps(ps: &Value, model: &str) -> Option<u64> {
    ps.get("models")?
        .as_array()?
        .iter()
        .find(|entry| {
            ["name", "model"]
                .iter()
                .filter_map(|key| entry.get(key)?.as_str())
                .any(|name| same_model(name, model))
        })?
        .get("context_length")?
        .as_u64()
}

/// Ollama spells an untagged model as `name:latest`.
fn same_model(a: &str, b: &str) -> bool {
    a == b
        || a.strip_suffix(":latest").is_some_and(|base| base == b)
        || b.strip_suffix(":latest").is_some_and(|base| base == a)
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

fn get_json(url: &str) -> Option<Value> {
    let mut response = agent().get(url).call().ok()?;
    let text = response.body_mut().read_to_string().ok()?;
    serde_json::from_str(&text).ok()
}

fn post_json(url: &str, body: &Value) -> Option<Value> {
    let mut response = agent()
        .post(url)
        .header("Content-Type", "application/json")
        .send(body.to_string().as_bytes())
        .ok()?;
    let text = response.body_mut().read_to_string().ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::ProviderKind;

    #[test]
    fn api_root_strips_the_openai_style_suffix() {
        let mut provider = Provider::for_kind(ProviderKind::Ollama);
        assert_eq!(
            api_root(&provider).as_deref(),
            Some("http://localhost:11434")
        );
        provider.base_url = Some("http://gpu-box:11434/v1/".into());
        assert_eq!(api_root(&provider).as_deref(), Some("http://gpu-box:11434"));
    }

    #[test]
    fn show_reports_capabilities_and_the_architecture_context_length() {
        let show = json!({
            "capabilities": ["completion", "vision", "tools", "thinking"],
            "model_info": {
                "gemma4.block_count": 48,
                "gemma4.context_length": 262144,
                "general.architecture": "gemma4"
            },
            "parameters": "temperature                    1\ntop_k                          64"
        });
        let facts = facts_from_show(&show);
        assert_eq!(facts.context_length, Some(262_144));
        assert!(facts.supports_tools());
    }

    #[test]
    fn a_modelfile_num_ctx_wins_over_the_architecture_limit() {
        let show = json!({
            "capabilities": ["completion"],
            "model_info": { "llama.context_length": 131072 },
            "parameters": "num_ctx                        65536\nstop                           \"<|eot_id|>\""
        });
        let facts = facts_from_show(&show);
        assert_eq!(facts.context_length, Some(65_536));
        assert!(!facts.supports_tools());
    }

    #[test]
    fn a_loaded_model_reports_the_window_the_server_allocated() {
        let ps = json!({
            "models": [
                { "name": "qwen3-coder:latest", "model": "qwen3-coder:latest", "context_length": 32768 },
                { "name": "gemma4:12b", "model": "gemma4:12b", "context_length": 262144 }
            ]
        });
        assert_eq!(context_length_from_ps(&ps, "gemma4:12b"), Some(262_144));
        assert_eq!(context_length_from_ps(&ps, "qwen3-coder"), Some(32_768));
        assert_eq!(
            context_length_from_ps(&ps, "qwen3-coder:latest"),
            Some(32_768)
        );
        assert_eq!(context_length_from_ps(&ps, "phi4"), None);
    }

    #[test]
    fn missing_fields_degrade_to_nothing() {
        assert_eq!(facts_from_show(&json!({})), ModelFacts::default());
        assert_eq!(
            context_length_from_ps(&json!({ "models": [] }), "gemma4:12b"),
            None
        );
        assert_eq!(
            context_length_from_ps(&json!("not an object"), "gemma4:12b"),
            None
        );
    }
}
