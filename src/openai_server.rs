//! Best-effort questions for a local server that speaks OpenAI's API natively:
//! llama.cpp's `llama-server`, and vLLM.
//!
//! Like [`crate::ollama`], every answer here only adds information - the
//! context window Claude Code should assume, a few lines in `alc doctor` - and
//! degrades to nothing, so a launch and a dry run behave the same whether or
//! not the server is up.
//!
//! Unlike Ollama these servers are often shared and started with a key, so
//! every question carries the profile's key when it has one: a server started
//! with `--api-key` refuses even `/v1/models` without it.

use std::time::Duration;

use serde_json::{Value, json};

use crate::config::Provider;

/// Long enough for a busy server, or one behind an SSH tunnel, to answer a
/// metadata call; short enough that a stopped one does not make `alc claude`
/// feel slow.
const TIMEOUT: Duration = Duration::from_millis(1500);

/// The server root: the profile's base URL without the `/v1` its OpenAI
/// routes sit under. llama.cpp's `/props` and vLLM's `/version` live here.
pub fn root(provider: &Provider) -> Option<String> {
    let base = provider.effective_base_url()?.trim_end_matches('/');
    Some(base.strip_suffix("/v1").unwrap_or(base).to_owned())
}

/// How a server answered one request.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// 2xx with a JSON body.
    Json(Value),
    /// Any other status, 4xx and 5xx included.
    Status(u16),
    /// No answer at all: refused, timed out, or not JSON.
    Unreachable,
}

/// What `/v1/models` says, with llama.cpp's `/props` filling in what vLLM
/// puts in the model list itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerFacts {
    /// Every name the server answers to, aliases included.
    pub models: Vec<String>,
    /// The context one request gets: vLLM's `max_model_len` for the model,
    /// else llama.cpp's per-slot `n_ctx`. Never the model's training context,
    /// which a server is free to configure below.
    pub context_length: Option<u64>,
    /// `llama.cpp b11100-7ab4ee7ba` or `vLLM 0.11.0`, when the server says.
    pub engine: Option<String>,
}

impl ServerFacts {
    /// Whether `model` is one of the names the server lists. Not the same as
    /// whether a request for it is served: llama-server with one model loaded
    /// answers under any name, while vLLM, and llama-server routing several
    /// models, answer a name they do not list with 404.
    pub fn serves(&self, model: &str) -> bool {
        self.models.iter().any(|name| name == model)
    }
}

/// What asking the server for its models came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Reach {
    Answered(ServerFacts),
    /// It answered `/v1/models` with this status: 401 or 403 is the key.
    Refused(u16),
    Unreachable,
}

/// The context window Claude Code should assume for `model` on this
/// profile's server, or `None` when the server is down or does not say.
pub fn context_window(provider: &Provider, key: Option<&str>, model: &str) -> Option<u64> {
    let root = root(provider)?;
    let Answer::Json(models) = get(&format!("{root}/v1/models"), key) else {
        return None;
    };
    listed_context(&models, model).or_else(|| slot_context(&root, key))
}

/// Everything `alc doctor` shows about the server behind a profile.
pub fn inspect(provider: &Provider, key: Option<&str>, model: &str) -> Reach {
    let Some(root) = root(provider) else {
        return Reach::Unreachable;
    };
    let models = match get(&format!("{root}/v1/models"), key) {
        Answer::Json(models) => models,
        Answer::Status(status) => return Reach::Refused(status),
        Answer::Unreachable => return Reach::Unreachable,
    };
    let props = match get(&format!("{root}/props"), key) {
        Answer::Json(props) => Some(props),
        _ => None,
    };
    let engine = props
        .as_ref()
        .and_then(|props| props.get("build_info")?.as_str())
        .map(|build| format!("llama.cpp {build}"))
        .or_else(|| match get(&format!("{root}/version"), key) {
            Answer::Json(version) => version
                .get("version")?
                .as_str()
                .map(|version| format!("vLLM {version}")),
            _ => None,
        });
    Reach::Answered(ServerFacts {
        models: model_names(&models),
        context_length: listed_context(&models, model)
            .or_else(|| props.as_ref().and_then(context_from_props)),
        engine,
    })
}

/// Whether the server has an Anthropic Messages route, which Claude Code
/// needs and the other agents do not. Asked with an empty body, so a server
/// that has the route refuses the request before any model reads a token -
/// llama.cpp with a 500 naming the missing `messages`, vLLM with a 400 - and
/// only one without it answers 404. `None` when the answer says nothing
/// about the route: no answer, or a refused key.
pub fn serves_messages(provider: &Provider, key: Option<&str>) -> Option<bool> {
    let root = root(provider)?;
    match post(&format!("{root}/v1/messages"), key, &json!({})) {
        Answer::Json(_) => Some(true),
        Answer::Status(401 | 403) | Answer::Unreachable => None,
        Answer::Status(status) => Some(status != 404),
    }
}

/// Every id and alias `/v1/models` lists.
pub(crate) fn model_names(models: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for entry in entries(models) {
        let aliases = entry
            .get("aliases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for name in entry.get("id").into_iter().chain(aliases) {
            if let Some(name) = name.as_str()
                && !names.iter().any(|known| known == name)
            {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// vLLM's `max_model_len` for `model`, or for the only model listed.
pub(crate) fn listed_context(models: &Value, model: &str) -> Option<u64> {
    let entries: Vec<&Value> = entries(models).collect();
    let entry = entries
        .iter()
        .find(|entry| {
            entry.get("id").and_then(Value::as_str) == Some(model)
                || entry
                    .get("aliases")
                    .and_then(Value::as_array)
                    .is_some_and(|aliases| aliases.iter().any(|alias| alias == model))
        })
        .or(match entries.as_slice() {
            [only] => Some(only),
            _ => None,
        })?;
    entry.get("max_model_len")?.as_u64()
}

/// llama.cpp's per-slot context from `/props`: what one request can use,
/// whatever the server holds across all its slots.
pub(crate) fn context_from_props(props: &Value) -> Option<u64> {
    props
        .get("default_generation_settings")?
        .get("n_ctx")?
        .as_u64()
        .filter(|tokens| *tokens > 0)
}

fn slot_context(root: &str, key: Option<&str>) -> Option<u64> {
    match get(&format!("{root}/props"), key) {
        Answer::Json(props) => context_from_props(&props),
        _ => None,
    }
}

fn entries(models: &Value) -> impl Iterator<Item = &Value> {
    models
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

fn get(url: &str, key: Option<&str>) -> Answer {
    let mut request = agent().get(url);
    if let Some(key) = key {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    answer(request.call())
}

fn post(url: &str, key: Option<&str>, body: &Value) -> Answer {
    let mut request = agent().post(url).header("Content-Type", "application/json");
    if let Some(key) = key {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    answer(request.send(body.to_string().as_bytes()))
}

fn answer(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Answer {
    match result {
        Ok(mut response) => response
            .body_mut()
            .read_to_string()
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .map_or(Answer::Unreachable, Answer::Json),
        Err(ureq::Error::StatusCode(status)) => Answer::Status(status),
        Err(_) => Answer::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::ProviderKind;

    #[test]
    fn the_root_drops_the_openai_suffix() {
        let mut provider = Provider::for_kind(ProviderKind::Llamacpp);
        assert_eq!(root(&provider).as_deref(), Some("http://localhost:8080"));
        provider.base_url = Some("http://127.0.0.1:18080/v1/".into());
        assert_eq!(root(&provider).as_deref(), Some("http://127.0.0.1:18080"));
        provider.base_url = Some("http://gpu-box:8000".into());
        assert_eq!(root(&provider).as_deref(), Some("http://gpu-box:8000"));
    }

    /// Trimmed from what a llama-server b11100 serving one model answered.
    fn llamacpp_models() -> Value {
        json!({
            "models": [{ "name": "qwen3.8-27b", "model": "qwen3.8-27b" }],
            "object": "list",
            "data": [{
                "id": "qwen3.8-27b",
                "aliases": ["qwen3.8-27b"],
                "object": "model",
                "owned_by": "llamacpp",
                "meta": { "n_ctx": 262144, "n_ctx_train": 262144 }
            }]
        })
    }

    #[test]
    fn llama_cpp_lists_its_model_but_leaves_the_context_to_props() {
        let models = llamacpp_models();
        assert_eq!(model_names(&models), ["qwen3.8-27b"]);
        assert_eq!(
            listed_context(&models, "qwen3.8-27b"),
            None,
            "the per-slot context comes from /props, not the model's metadata"
        );
        let props = json!({
            "default_generation_settings": { "n_ctx": 65536 },
            "total_slots": 4,
            "build_info": "b11100-7ab4ee7ba"
        });
        assert_eq!(context_from_props(&props), Some(65_536));
        assert_eq!(context_from_props(&json!({})), None);
    }

    #[test]
    fn vllm_states_each_models_length_in_the_list() {
        let models = json!({
            "object": "list",
            "data": [
                { "id": "Qwen/Qwen3.8-27B", "max_model_len": 131072 },
                { "id": "lora-coder", "max_model_len": 32768, "parent": "Qwen/Qwen3.8-27B" }
            ]
        });
        assert_eq!(model_names(&models), ["Qwen/Qwen3.8-27B", "lora-coder"]);
        assert_eq!(listed_context(&models, "lora-coder"), Some(32_768));
        assert_eq!(
            listed_context(&models, "something-else"),
            None,
            "two models and neither is the one asked for"
        );
        let one = json!({ "data": [{ "id": "served-name", "max_model_len": 40960 }] });
        assert_eq!(
            listed_context(&one, "another-name"),
            Some(40_960),
            "a server with one model answers every name with it"
        );
    }

    #[test]
    fn a_server_serves_the_names_it_lists() {
        let facts = ServerFacts {
            models: model_names(&llamacpp_models()),
            ..ServerFacts::default()
        };
        assert!(facts.serves("qwen3.8-27b"));
        assert!(!facts.serves("claude-haiku-4-5"));
    }

    #[test]
    fn a_stopped_server_answers_nothing_and_blocks_nothing() {
        let mut provider = Provider::for_kind(ProviderKind::Llamacpp);
        // Nothing listens on port 9 (discard) on a developer machine or CI.
        provider.base_url = Some("http://127.0.0.1:9/v1".into());
        assert_eq!(context_window(&provider, None, "m"), None);
        assert_eq!(inspect(&provider, Some("k"), "m"), Reach::Unreachable);
        assert_eq!(serves_messages(&provider, None), None);
    }
}
