//! Any API provider, as the bridge's upstream.
//!
//! The surfaces build an [`UpstreamRequest`] and read Responses events back,
//! whatever serves the turn. This upstream is the one that lets an agent reach
//! a provider it cannot speak to itself - Claude Code on an OpenAI-only
//! endpoint, Codex CLI on a Chat Completions one, agy on either - by retelling
//! the request in the provider's own wire ([`ApiWire`]) and its answer as
//! Responses events.
//!
//! Three wires, one shape each way:
//!
//! * OpenAI Responses: the request is rebuilt in the public API's spelling
//!   (top-level `tools`, the system prompt as `instructions`) and the stream
//!   is relayed event for event;
//! * Chat Completions: messages and `tool_calls` going up, `choices[].delta`
//!   coming back;
//! * Anthropic Messages: content blocks going up, the block events coming
//!   back.
//!
//! Tool calls are held until they are complete and then told whole, as the
//! Antigravity upstream tells them: the Messages surface cannot interleave two
//! open `tool_use` blocks, and a Chat stream may interleave its calls' argument
//! deltas. Reasoning a provider streams is told as a reasoning summary and
//! never replayed: no provider here can read another's.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Map, Value, json};

use super::upstream::{
    ContentPart, Effort, FunctionCallOutput, InputItem, SseDecoder, SseFrame, UpstreamFrame,
    UpstreamRequest, UpstreamStream,
};
use super::{BridgeError, BridgeState};
use crate::config::{ApiEndpoint, ApiWire, AuthStyle};
use crate::usage::observer::RequestObservation;
use crate::usage::records::Outcome;

/// How long to wait for the response headers. A local server reading a long
/// prompt may send nothing until the first token, so this is the stream's
/// own idle limit rather than the minute chatgpt.com gets.
const HEADER_TIMEOUT: Duration = Duration::from_secs(1800);
/// How long a started stream may stay silent: llama.cpp reads a large prompt
/// after its headers, without a byte in between.
const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(1800);
/// The output cap a Messages request must carry when its client named none.
const MESSAGES_DEFAULT_MAX_TOKENS: u64 = 32_000;
/// Request fields a provider may refuse by name; each is dropped and the
/// request sent again, at most [`REFUSAL_RETRIES`] times.
const DROPPABLE: [&str; 8] = [
    "max_completion_tokens",
    "max_output_tokens",
    "max_tokens",
    "temperature",
    "stream_options",
    "parallel_tool_calls",
    "reasoning",
    "tool_choice",
];
const REFUSAL_RETRIES: usize = 3;

/// The live half of [`super::UpstreamKind::Api`].
pub(crate) struct ApiUpstream {
    pub endpoint: ApiEndpoint,
    http: reqwest::Client,
}

impl ApiUpstream {
    pub(crate) fn new(endpoint: ApiEndpoint) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("alc/", env!("CARGO_PKG_VERSION")))
            // A redirect would carry the key somewhere the profile never named.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| anyhow::anyhow!("failed to build the API bridge's client: {error}"))?;
        Ok(Self { endpoint, http })
    }

    fn host(&self) -> String {
        reqwest::Url::parse(&self.endpoint.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| self.endpoint.url.clone())
    }
}

/// Sends `request` to the profile's provider and hands back its answer as
/// Responses events.
pub(crate) async fn send(
    _state: &BridgeState,
    api: &ApiUpstream,
    request: UpstreamRequest,
    observed: Option<RequestObservation>,
) -> Result<UpstreamStream, BridgeError> {
    let opened = open(api, &request).await;
    match opened {
        Ok(response) => Ok(UpstreamStream::api(ApiStream::new(
            response,
            api.endpoint.wire,
            &request.model,
            observed,
        ))),
        Err(error) => {
            if let Some(observation) = &observed {
                observation.finish(outcome_of(&error));
            }
            Err(error)
        }
    }
}

async fn open(
    api: &ApiUpstream,
    request: &UpstreamRequest,
) -> Result<reqwest::Response, BridgeError> {
    let mut body = match api.endpoint.wire {
        ApiWire::Responses => responses_body(request),
        ApiWire::Chat => chat_body(request),
        ApiWire::Messages => messages_body(request),
    };
    for attempt in 0..=REFUSAL_RETRIES {
        let response = post(api, &body).await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let refusal = response.bytes().await.unwrap_or_default();
        if status == StatusCode::BAD_REQUEST
            && attempt < REFUSAL_RETRIES
            && drop_refused(&mut body, &refusal)
        {
            continue;
        }
        return Err(refused(api, status, &refusal));
    }
    unreachable!("the loop returns on the last attempt")
}

async fn post(api: &ApiUpstream, body: &Value) -> Result<reqwest::Response, BridgeError> {
    let endpoint = &api.endpoint;
    let mut builder = api
        .http
        .post(&endpoint.url)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(body);
    if endpoint.wire == ApiWire::Messages {
        builder = builder.header("anthropic-version", "2023-06-01");
    }
    if let Some(key) = endpoint.key.as_deref().filter(|key| !key.is_empty()) {
        builder = match (endpoint.auth, endpoint.wire) {
            (AuthStyle::ApiKey, ApiWire::Messages) => builder.header("x-api-key", key),
            (AuthStyle::ApiKey, _) => builder.header("api-key", key).bearer_auth(key),
            _ => builder.bearer_auth(key),
        };
    }
    match tokio::time::timeout(HEADER_TIMEOUT, builder.send()).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(BridgeError::new(
            StatusCode::BAD_GATEWAY,
            "api_error",
            format!("could not reach {}: {error}", api.host()),
        )),
        Err(_) => Err(BridgeError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "api_error",
            format!(
                "{} did not answer within {}s",
                api.host(),
                HEADER_TIMEOUT.as_secs()
            ),
        )),
    }
}

/// Drops the first field of [`DROPPABLE`] a refusal names, if the body has
/// it. `max_tokens` is checked last among the caps, since the other two
/// contain it.
fn drop_refused(body: &mut Value, refusal: &[u8]) -> bool {
    let text = String::from_utf8_lossy(refusal).to_ascii_lowercase();
    let Some(object) = body.as_object_mut() else {
        return false;
    };
    for field in DROPPABLE {
        if text.contains(field) && object.remove(field).is_some() {
            return true;
        }
    }
    false
}

fn refused(api: &ApiUpstream, status: StatusCode, body: &[u8]) -> BridgeError {
    let detail = super::upstream::error_message(body)
        .unwrap_or_else(|| format!("{} refused the request with status {status}", api.host()));
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => BridgeError::new(
            status,
            "authentication_error",
            format!("{detail} (check this profile's API key with `alc config`)"),
        ),
        StatusCode::TOO_MANY_REQUESTS => BridgeError::new(status, "rate_limit_error", detail),
        _ => BridgeError::upstream(status, detail),
    }
}

fn outcome_of(error: &BridgeError) -> Outcome {
    if error.status == StatusCode::GATEWAY_TIMEOUT {
        Outcome::TimedOut
    } else {
        Outcome::Failed
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// The request's system prompt: its `instructions`, then every developer or
/// system message, in order.
fn system_prompt(request: &UpstreamRequest) -> String {
    let mut parts: Vec<String> = request
        .instructions
        .iter()
        .filter(|text| !text.is_empty())
        .cloned()
        .collect();
    for item in &request.input {
        if let InputItem::Message { role, content } = item
            && is_system(role)
        {
            let text = texts(content);
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }
    parts.join("\n\n")
}

fn is_system(role: &str) -> bool {
    matches!(role, "developer" | "system")
}

fn texts(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::InputText { text } | ContentPart::OutputText { text } => {
                Some(text.as_str())
            }
            ContentPart::InputImage { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn output_text(output: &FunctionCallOutput) -> String {
    match output {
        FunctionCallOutput::Text(text) => text.clone(),
        FunctionCallOutput::Parts(parts) => texts(parts),
    }
}

fn tools(request: &UpstreamRequest) -> Vec<&super::upstream::Tool> {
    request
        .input
        .iter()
        .filter_map(|item| match item {
            InputItem::AdditionalTools { tools, .. } => Some(tools.iter()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn arguments(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// The function a `tool_choice` names, in any of the spellings the surfaces
/// hand over.
fn chosen_function(choice: &Value) -> Option<&str> {
    choice
        .get("name")
        .or_else(|| choice.pointer("/function/name"))
        .and_then(Value::as_str)
}

fn insert_common(object: &mut Map<String, Value>, request: &UpstreamRequest, cap: &str) {
    if let Some(max) = request.max_output_tokens {
        object.insert(cap.to_owned(), json!(max));
    }
    if let Some(temperature) = request.temperature {
        object.insert("temperature".to_owned(), json!(temperature));
    }
}

/// The public Responses API's spelling of the bridge's request.
pub(crate) fn responses_body(request: &UpstreamRequest) -> Value {
    let mut input = Vec::new();
    for item in &request.input {
        match item {
            InputItem::Message { role, content } if !is_system(role) => {
                input.push(json!({"type": "message", "role": role, "content": content}));
            }
            InputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => input.push(json!({"type": "function_call", "call_id": call_id,
                "name": name, "arguments": arguments})),
            InputItem::FunctionCallOutput { call_id, output } => input
                .push(json!({"type": "function_call_output", "call_id": call_id,
                       "output": output_text(output)})),
            _ => {}
        }
    }
    let mut object = Map::new();
    object.insert("model".to_owned(), json!(request.model));
    object.insert("stream".to_owned(), json!(true));
    object.insert("store".to_owned(), json!(false));
    let system = system_prompt(request);
    if !system.is_empty() {
        object.insert("instructions".to_owned(), json!(system));
    }
    object.insert("input".to_owned(), Value::Array(input));
    let tools: Vec<Value> = tools(request)
        .into_iter()
        .map(|tool| {
            json!({"type": "function", "name": tool.name, "description": tool.description,
                   "parameters": tool.parameters})
        })
        .collect();
    if !tools.is_empty() {
        object.insert("tools".to_owned(), Value::Array(tools));
        if let Some(choice) = &request.tool_choice {
            object.insert("tool_choice".to_owned(), choice.clone());
        }
    }
    if let Some(effort) = request
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.effort)
    {
        // The public API's top rung is `xhigh`.
        let word = match effort {
            Effort::Max => "xhigh",
            other => effort_word(other),
        };
        object.insert("reasoning".to_owned(), json!({"effort": word}));
    }
    insert_common(&mut object, request, "max_output_tokens");
    Value::Object(object)
}

fn effort_word(effort: Effort) -> &'static str {
    match effort {
        Effort::None => "none",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    }
}

/// The request as Chat Completions.
pub(crate) fn chat_body(request: &UpstreamRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let system = system_prompt(request);
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    for item in &request.input {
        match item {
            InputItem::Message { role, content } if role == "user" => {
                messages.push(json!({"role": "user", "content": chat_user_content(content)}));
            }
            InputItem::Message { role, content } if role == "assistant" => {
                let text = texts(content);
                if !text.is_empty() {
                    messages.push(json!({"role": "assistant", "content": text}));
                }
            }
            InputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                let call = json!({"id": call_id, "type": "function",
                    "function": {"name": name, "arguments": arguments}});
                // Calls of one turn ride its assistant message; a call after
                // a tool result starts a new one.
                match messages.last_mut() {
                    Some(last) if last["role"] == "assistant" => {
                        if let Some(calls) =
                            last.get_mut("tool_calls").and_then(Value::as_array_mut)
                        {
                            calls.push(call);
                        } else {
                            last["tool_calls"] = json!([call]);
                        }
                    }
                    _ => messages
                        .push(json!({"role": "assistant", "content": null, "tool_calls": [call]})),
                }
            }
            InputItem::FunctionCallOutput { call_id, output } => messages.push(
                json!({"role": "tool", "tool_call_id": call_id, "content": output_text(output)}),
            ),
            _ => {}
        }
    }
    let mut object = Map::new();
    object.insert("model".to_owned(), json!(request.model));
    object.insert("messages".to_owned(), Value::Array(messages));
    object.insert("stream".to_owned(), json!(true));
    object.insert("stream_options".to_owned(), json!({"include_usage": true}));
    let tools: Vec<Value> = tools(request)
        .into_iter()
        .map(|tool| {
            json!({"type": "function", "function": {"name": tool.name,
                   "description": tool.description, "parameters": tool.parameters}})
        })
        .collect();
    if !tools.is_empty() {
        object.insert("tools".to_owned(), Value::Array(tools));
        if let Some(choice) = &request.tool_choice {
            let choice = match chosen_function(choice) {
                Some(name) => json!({"type": "function", "function": {"name": name}}),
                None => choice.clone(),
            };
            object.insert("tool_choice".to_owned(), choice);
        }
    }
    insert_common(&mut object, request, "max_tokens");
    Value::Object(object)
}

fn chat_user_content(content: &[ContentPart]) -> Value {
    if content
        .iter()
        .all(|part| !matches!(part, ContentPart::InputImage { .. }))
    {
        return json!(texts(content));
    }
    Value::Array(
        content
            .iter()
            .map(|part| match part {
                ContentPart::InputText { text } | ContentPart::OutputText { text } => {
                    json!({"type": "text", "text": text})
                }
                ContentPart::InputImage { image_url, .. } => {
                    json!({"type": "image_url", "image_url": {"url": image_url}})
                }
            })
            .collect(),
    )
}

/// The request as Anthropic Messages.
pub(crate) fn messages_body(request: &UpstreamRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let mut push = |role: &str, block: Value| match messages.last_mut() {
        Some(last) if last["role"] == role => {
            if let Some(blocks) = last["content"].as_array_mut() {
                blocks.push(block);
            }
        }
        _ => messages.push(json!({"role": role, "content": [block]})),
    };
    for item in &request.input {
        match item {
            InputItem::Message { role, content } if !is_system(role) => {
                let role = if role == "assistant" {
                    "assistant"
                } else {
                    "user"
                };
                for part in content {
                    match part {
                        ContentPart::InputText { text } | ContentPart::OutputText { text } => {
                            if !text.is_empty() {
                                push(role, json!({"type": "text", "text": text}));
                            }
                        }
                        ContentPart::InputImage { image_url, .. } => {
                            push(role, anthropic_image(image_url));
                        }
                    }
                }
            }
            InputItem::FunctionCall {
                call_id,
                name,
                arguments: raw,
                ..
            } => push(
                "assistant",
                json!({"type": "tool_use", "id": call_id, "name": name, "input": arguments(raw)}),
            ),
            InputItem::FunctionCallOutput { call_id, output } => push(
                "user",
                json!({"type": "tool_result", "tool_use_id": call_id,
                       "content": output_text(output)}),
            ),
            _ => {}
        }
    }
    let mut object = Map::new();
    object.insert("model".to_owned(), json!(request.model));
    object.insert(
        "max_tokens".to_owned(),
        json!(
            request
                .max_output_tokens
                .unwrap_or(MESSAGES_DEFAULT_MAX_TOKENS)
        ),
    );
    object.insert("stream".to_owned(), json!(true));
    let system = system_prompt(request);
    if !system.is_empty() {
        object.insert("system".to_owned(), json!(system));
    }
    object.insert("messages".to_owned(), Value::Array(messages));
    let tools: Vec<Value> = tools(request)
        .into_iter()
        .map(|tool| {
            let mut entry = json!({"name": tool.name, "input_schema": tool.parameters});
            if let Some(description) = &tool.description {
                entry["description"] = json!(description);
            }
            entry
        })
        .collect();
    if !tools.is_empty() {
        object.insert("tools".to_owned(), Value::Array(tools));
        if let Some(choice) = &request.tool_choice {
            let choice = match (chosen_function(choice), choice.as_str()) {
                (Some(name), _) => Some(json!({"type": "tool", "name": name})),
                (None, Some("required")) => Some(json!({"type": "any"})),
                (None, Some("none")) => Some(json!({"type": "none"})),
                (None, Some("auto")) => Some(json!({"type": "auto"})),
                _ => None,
            };
            if let Some(choice) = choice {
                object.insert("tool_choice".to_owned(), choice);
            }
        }
    }
    if let Some(temperature) = request.temperature {
        object.insert("temperature".to_owned(), json!(temperature));
    }
    Value::Object(object)
}

fn anthropic_image(url: &str) -> Value {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((media_type, data)) = rest.split_once(";base64,")
    {
        return json!({"type": "image", "source": {"type": "base64", "media_type": media_type,
                      "data": data}});
    }
    json!({"type": "image", "source": {"type": "url", "url": url}})
}

// ---------------------------------------------------------------------------
// Answers
// ---------------------------------------------------------------------------

enum Open {
    Message {
        index: u32,
        id: String,
        text: String,
    },
    Reasoning {
        index: u32,
        id: String,
        text: String,
    },
}

/// How a turn ended.
enum Finish {
    Completed,
    Incomplete,
    Failed(String),
}

/// Writes Responses events for one turn: text and reasoning streamed as they
/// arrive, each tool call told whole.
struct Emitter {
    model: String,
    response_id: String,
    created_at: u64,
    started: bool,
    next_index: u32,
    open: Option<Open>,
    finished: bool,
}

impl Emitter {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            response_id: format!("resp_{}", super::agy_upstream::random_hex(12)),
            created_at: crate::usage::ledger::now_unix(),
            started: false,
            next_index: 0,
            open: None,
            finished: false,
        }
    }

    fn envelope(&self, status: &str) -> Value {
        json!({"id": self.response_id, "object": "response", "created_at": self.created_at,
               "model": self.model, "status": status, "output": []})
    }

    fn start(&mut self, out: &mut Vec<Value>) {
        if !self.started {
            self.started = true;
            out.push(json!({"type": "response.created", "response": self.envelope("in_progress")}));
        }
    }

    fn next(&mut self) -> u32 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }

    fn text(&mut self, delta: &str, out: &mut Vec<Value>) {
        if delta.is_empty() {
            return;
        }
        self.start(out);
        if !matches!(self.open, Some(Open::Message { .. })) {
            self.close(out);
            let index = self.next();
            let id = format!("msg_{index}");
            out.push(
                json!({"type": "response.output_item.added", "output_index": index,
                "item": {"type": "message", "id": id, "role": "assistant",
                         "status": "in_progress", "content": []}}),
            );
            out.push(
                json!({"type": "response.content_part.added", "output_index": index,
                "item_id": id, "content_index": 0,
                "part": {"type": "output_text", "text": "", "annotations": []}}),
            );
            self.open = Some(Open::Message {
                index,
                id,
                text: String::new(),
            });
        }
        if let Some(Open::Message { index, id, text }) = &mut self.open {
            text.push_str(delta);
            out.push(
                json!({"type": "response.output_text.delta", "output_index": *index,
                "item_id": id, "content_index": 0, "delta": delta}),
            );
        }
    }

    fn reasoning(&mut self, delta: &str, out: &mut Vec<Value>) {
        if delta.is_empty() {
            return;
        }
        self.start(out);
        if !matches!(self.open, Some(Open::Reasoning { .. })) {
            self.close(out);
            let index = self.next();
            let id = format!("rs_{index}");
            out.push(
                json!({"type": "response.output_item.added", "output_index": index,
                "item": {"type": "reasoning", "id": id, "summary": []}}),
            );
            out.push(json!({"type": "response.reasoning_summary_part.added",
                "output_index": index, "item_id": id, "summary_index": 0,
                "part": {"type": "summary_text", "text": ""}}));
            self.open = Some(Open::Reasoning {
                index,
                id,
                text: String::new(),
            });
        }
        if let Some(Open::Reasoning { index, id, text }) = &mut self.open {
            text.push_str(delta);
            out.push(json!({"type": "response.reasoning_summary_text.delta",
                "output_index": *index, "item_id": id, "summary_index": 0, "delta": delta}));
        }
    }

    fn close(&mut self, out: &mut Vec<Value>) {
        match self.open.take() {
            Some(Open::Message { index, id, text }) => {
                out.push(
                    json!({"type": "response.output_text.done", "output_index": index,
                    "item_id": id, "content_index": 0, "text": text}),
                );
                out.push(
                    json!({"type": "response.content_part.done", "output_index": index,
                    "item_id": id, "content_index": 0,
                    "part": {"type": "output_text", "text": text, "annotations": []}}),
                );
                out.push(json!({"type": "response.output_item.done", "output_index": index,
                    "item": {"type": "message", "id": id, "role": "assistant",
                             "status": "completed",
                             "content": [{"type": "output_text", "text": text, "annotations": []}]}}));
            }
            Some(Open::Reasoning { index, id, text }) => {
                out.push(json!({"type": "response.reasoning_summary_text.done",
                    "output_index": index, "item_id": id, "summary_index": 0, "text": text}));
                out.push(json!({"type": "response.reasoning_summary_part.done",
                    "output_index": index, "item_id": id, "summary_index": 0,
                    "part": {"type": "summary_text", "text": text}}));
                // Never replayed: an empty `encrypted_content` reads as unsigned.
                out.push(
                    json!({"type": "response.output_item.done", "output_index": index,
                    "item": {"type": "reasoning", "id": id,
                             "summary": [{"type": "summary_text", "text": text}],
                             "encrypted_content": ""}}),
                );
            }
            None => {}
        }
    }

    fn call(&mut self, call_id: &str, name: &str, arguments: &str, out: &mut Vec<Value>) {
        self.start(out);
        self.close(out);
        let arguments = if arguments.trim().is_empty() {
            "{}"
        } else {
            arguments
        };
        let index = self.next();
        let id = format!("fc_{index}");
        out.push(
            json!({"type": "response.output_item.added", "output_index": index,
            "item": {"type": "function_call", "id": id, "call_id": call_id, "name": name,
                     "arguments": "", "status": "in_progress"}}),
        );
        out.push(
            json!({"type": "response.function_call_arguments.delta", "output_index": index,
            "item_id": id, "delta": arguments}),
        );
        out.push(
            json!({"type": "response.function_call_arguments.done", "output_index": index,
            "item_id": id, "arguments": arguments}),
        );
        out.push(
            json!({"type": "response.output_item.done", "output_index": index,
            "item": {"type": "function_call", "id": id, "call_id": call_id, "name": name,
                     "arguments": arguments, "status": "completed"}}),
        );
    }

    fn finish(&mut self, finish: Finish, usage: Value, out: &mut Vec<Value>) {
        if self.finished {
            return;
        }
        self.start(out);
        self.close(out);
        let event = match finish {
            Finish::Completed => {
                let mut response = self.envelope("completed");
                response["usage"] = usage;
                json!({"type": "response.completed", "response": response})
            }
            Finish::Incomplete => {
                let mut response = self.envelope("incomplete");
                response["incomplete_details"] = json!({"reason": "max_output_tokens"});
                response["usage"] = usage;
                json!({"type": "response.incomplete", "response": response})
            }
            Finish::Failed(message) => {
                let mut response = self.envelope("failed");
                response["error"] = json!({"code": "upstream_error", "message": message});
                response["usage"] = usage;
                json!({"type": "response.failed", "response": response})
            }
        };
        out.push(event);
        self.finished = true;
    }
}

fn minted_call_id() -> String {
    format!("call_{}", super::agy_upstream::random_hex(12))
}

/// A Chat Completions stream, chunk by chunk.
struct ChatTurn {
    emitter: Emitter,
    /// Calls by their index in the stream: id, name, arguments so far.
    calls: BTreeMap<u64, (String, String, String)>,
    finish_reason: Option<String>,
    usage: Option<Value>,
    produced: bool,
}

impl ChatTurn {
    fn accept(&mut self, chunk: &Value, out: &mut Vec<Value>) {
        if let Some(error) = chunk.get("error").filter(|error| !error.is_null()) {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| error.to_string(), str::to_owned);
            self.emitter
                .finish(Finish::Failed(message), Value::Null, out);
            return;
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            self.usage = Some(usage.clone());
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        let delta = choice.get("delta").or_else(|| choice.get("message"));
        if let Some(delta) = delta {
            for key in ["reasoning_content", "reasoning"] {
                if let Some(text) = delta.get(key).and_then(Value::as_str) {
                    self.produced = true;
                    self.emitter.reasoning(text, out);
                }
            }
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                self.produced = true;
                self.emitter.text(text, out);
            }
            for call in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.produced = true;
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(self.calls.len() as u64);
                let entry = self.calls.entry(index).or_default();
                if let Some(id) = call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    id.clone_into(&mut entry.0);
                }
                if let Some(name) = call
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                {
                    name.clone_into(&mut entry.1);
                }
                match call.pointer("/function/arguments") {
                    Some(Value::String(part)) => entry.2.push_str(part),
                    Some(object @ Value::Object(_)) => entry.2.push_str(&object.to_string()),
                    _ => {}
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_owned());
        }
    }

    fn end(&mut self, out: &mut Vec<Value>) -> Result<(), BridgeError> {
        if self.emitter.finished {
            return Ok(());
        }
        if self.finish_reason.is_none() && !self.produced {
            return Err(BridgeError::upstream(
                StatusCode::BAD_GATEWAY,
                "the provider ended the stream without answering",
            ));
        }
        for (_, (id, name, arguments)) in std::mem::take(&mut self.calls) {
            let id = if id.is_empty() { minted_call_id() } else { id };
            self.emitter.call(&id, &name, &arguments, out);
        }
        let usage = self.usage.as_ref().map_or(Value::Null, chat_usage);
        let finish = match self.finish_reason.as_deref() {
            Some("length") => Finish::Incomplete,
            Some("content_filter") => Finish::Failed(
                "the provider's content filter stopped the answer (content_filter)".to_owned(),
            ),
            _ => Finish::Completed,
        };
        self.emitter.finish(finish, usage, out);
        Ok(())
    }
}

fn chat_usage(usage: &Value) -> Value {
    let read = |pointer: &str| usage.pointer(pointer).and_then(Value::as_u64);
    let input = read("/prompt_tokens").unwrap_or(0);
    let output = read("/completion_tokens").unwrap_or(0);
    let mut mapped = json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": read("/total_tokens").unwrap_or(input + output),
    });
    if let Some(cached) =
        read("/prompt_tokens_details/cached_tokens").or_else(|| read("/prompt_cache_hit_tokens"))
    {
        mapped["input_tokens_details"] = json!({"cached_tokens": cached});
    }
    if let Some(reasoning) = read("/completion_tokens_details/reasoning_tokens") {
        mapped["output_tokens_details"] = json!({"reasoning_tokens": reasoning});
    }
    mapped
}

/// An Anthropic Messages stream, event by event.
#[derive(Default)]
struct MessagesTurn {
    /// The open `tool_use` blocks by index: id, name, input JSON so far.
    calls: BTreeMap<u64, (String, String, String)>,
    stop_reason: Option<String>,
    input_tokens: u64,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    output_tokens: u64,
    stopped: bool,
}

impl MessagesTurn {
    fn accept(&mut self, emitter: &mut Emitter, event: &Value, out: &mut Vec<Value>) {
        let read = |value: &Value, key: &str| value.get(key).and_then(Value::as_u64);
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.input_tokens = read(usage, "input_tokens").unwrap_or(0);
                    self.cache_read = read(usage, "cache_read_input_tokens");
                    self.cache_write = read(usage, "cache_creation_input_tokens");
                    self.output_tokens = read(usage, "output_tokens").unwrap_or(0);
                }
                emitter.start(out);
            }
            Some("content_block_start") => {
                let index = read(event, "index").unwrap_or(0);
                let block = &event["content_block"];
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_use") => {
                        let input = block
                            .get("input")
                            .filter(|input| input.as_object().is_some_and(|map| !map.is_empty()))
                            .map(Value::to_string)
                            .unwrap_or_default();
                        self.calls.insert(
                            index,
                            (
                                block["id"].as_str().unwrap_or_default().to_owned(),
                                block["name"].as_str().unwrap_or_default().to_owned(),
                                input,
                            ),
                        );
                    }
                    Some("text") => {
                        emitter.text(block["text"].as_str().unwrap_or_default(), out);
                    }
                    Some("thinking") => {
                        emitter.reasoning(block["thinking"].as_str().unwrap_or_default(), out);
                    }
                    _ => {}
                }
            }
            Some("content_block_delta") => {
                let index = read(event, "index").unwrap_or(0);
                let delta = &event["delta"];
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        emitter.text(delta["text"].as_str().unwrap_or_default(), out);
                    }
                    Some("thinking_delta") => {
                        emitter.reasoning(delta["thinking"].as_str().unwrap_or_default(), out);
                    }
                    Some("input_json_delta") => {
                        if let Some(call) = self.calls.get_mut(&index) {
                            call.2
                                .push_str(delta["partial_json"].as_str().unwrap_or_default());
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let index = read(event, "index").unwrap_or(0);
                if let Some((id, name, input)) = self.calls.remove(&index) {
                    let id = if id.is_empty() { minted_call_id() } else { id };
                    emitter.call(&id, &name, &input, out);
                }
            }
            Some("message_delta") => {
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.to_owned());
                }
                if let Some(output) = event
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                {
                    self.output_tokens = output;
                }
            }
            Some("message_stop") => self.terminate(emitter, out),
            Some("error") => {
                let message = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map_or_else(|| event.to_string(), str::to_owned);
                emitter.finish(Finish::Failed(message), Value::Null, out);
            }
            _ => {}
        }
    }

    fn terminate(&mut self, emitter: &mut Emitter, out: &mut Vec<Value>) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        for (_, (id, name, input)) in std::mem::take(&mut self.calls) {
            let id = if id.is_empty() { minted_call_id() } else { id };
            emitter.call(&id, &name, &input, out);
        }
        let input =
            self.input_tokens + self.cache_read.unwrap_or(0) + self.cache_write.unwrap_or(0);
        let mut usage = json!({
            "input_tokens": input,
            "output_tokens": self.output_tokens,
            "total_tokens": input + self.output_tokens,
        });
        // A count the provider left out is left out: Codex reads a `null`
        // here as a malformed completion and drops the turn.
        let mut details = Map::new();
        if let Some(read) = self.cache_read {
            details.insert("cached_tokens".to_owned(), json!(read));
        }
        if let Some(write) = self.cache_write {
            details.insert("cache_write_tokens".to_owned(), json!(write));
        }
        if !details.is_empty() {
            usage["input_tokens_details"] = Value::Object(details);
        }
        let finish = match self.stop_reason.as_deref() {
            Some("max_tokens") => Finish::Incomplete,
            Some("refusal") => Finish::Failed("the model refused to answer (refusal)".to_owned()),
            _ => Finish::Completed,
        };
        emitter.finish(finish, usage, out);
    }
}

/// A third-party Responses stream, retold rather than relayed: servers that
/// implement the API (llama.cpp, vLLM, Ollama) leave out fields chatgpt.com
/// always sends (`output_index`, `content_index`) and name reasoning
/// `reasoning_text`, so only what each event means is kept.
#[derive(Default)]
struct ResponsesTurn {
    /// Calls already told, by `call_id`: a server may finish one twice (on
    /// `output_item.done` and in `response.completed`'s output).
    told: std::collections::HashSet<String>,
}

impl ResponsesTurn {
    fn accept(&mut self, emitter: &mut Emitter, event: &Value, out: &mut Vec<Value>) {
        let text = |key: &str| event.get(key).and_then(Value::as_str).unwrap_or_default();
        match event.get("type").and_then(Value::as_str) {
            Some("response.created") => emitter.start(out),
            Some("response.output_text.delta") => emitter.text(text("delta"), out),
            Some("response.reasoning_text.delta" | "response.reasoning_summary_text.delta") => {
                emitter.reasoning(text("delta"), out);
            }
            Some("response.output_item.done") => self.item(emitter, &event["item"], out),
            Some(kind @ ("response.completed" | "response.incomplete" | "response.failed")) => {
                let response = &event["response"];
                for item in response
                    .get("output")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    self.item(emitter, item, out);
                }
                let usage = response.get("usage").cloned().unwrap_or(Value::Null);
                let finish = match kind {
                    "response.completed" => Finish::Completed,
                    "response.incomplete" => Finish::Incomplete,
                    _ => Finish::Failed(
                        response
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("the provider failed the turn")
                            .to_owned(),
                    ),
                };
                emitter.finish(finish, usage, out);
            }
            Some("error") => {
                let message = event
                    .pointer("/error/message")
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .map_or_else(|| event.to_string(), str::to_owned);
                emitter.finish(Finish::Failed(message), Value::Null, out);
            }
            _ => {}
        }
    }

    fn item(&mut self, emitter: &mut Emitter, item: &Value, out: &mut Vec<Value>) {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map_or_else(minted_call_id, str::to_owned);
        if !self.told.insert(call_id.clone()) {
            return;
        }
        emitter.call(
            &call_id,
            item["name"].as_str().unwrap_or_default(),
            item["arguments"].as_str().unwrap_or_default(),
            out,
        );
    }
}

enum Translator {
    Responses(Box<(Emitter, ResponsesTurn)>),
    Chat(Box<ChatTurn>),
    Messages(Box<(Emitter, MessagesTurn)>),
}

impl Translator {
    fn new(wire: ApiWire, model: &str) -> Self {
        match wire {
            ApiWire::Responses => {
                Self::Responses(Box::new((Emitter::new(model), ResponsesTurn::default())))
            }
            ApiWire::Chat => Self::Chat(Box::new(ChatTurn {
                emitter: Emitter::new(model),
                calls: BTreeMap::new(),
                finish_reason: None,
                usage: None,
                produced: false,
            })),
            ApiWire::Messages => {
                Self::Messages(Box::new((Emitter::new(model), MessagesTurn::default())))
            }
        }
    }

    fn accept(&mut self, value: Value) -> Vec<Value> {
        let mut out = Vec::new();
        match self {
            Self::Responses(turn) => {
                let (emitter, responses) = &mut **turn;
                responses.accept(emitter, &value, &mut out);
            }
            Self::Chat(turn) => turn.accept(&value, &mut out),
            Self::Messages(turn) => {
                let (emitter, messages) = &mut **turn;
                messages.accept(emitter, &value, &mut out);
            }
        }
        out
    }

    fn is_finished(&self) -> bool {
        match self {
            Self::Responses(turn) => turn.0.finished,
            Self::Chat(turn) => turn.emitter.finished,
            Self::Messages(turn) => turn.0.finished,
        }
    }

    fn end(&mut self) -> Result<Vec<Value>, BridgeError> {
        let mut out = Vec::new();
        match self {
            Self::Responses(turn) if !turn.0.finished => {
                return Err(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    "the provider ended the stream without finishing the turn",
                ));
            }
            Self::Responses(_) => {}
            Self::Chat(turn) => turn.end(&mut out)?,
            Self::Messages(turn) => {
                let (emitter, messages) = &mut **turn;
                if !emitter.finished {
                    if !emitter.started {
                        return Err(BridgeError::upstream(
                            StatusCode::BAD_GATEWAY,
                            "the provider ended the stream without answering",
                        ));
                    }
                    messages.terminate(emitter, &mut out);
                }
            }
        }
        Ok(out)
    }
}

/// A provider's turn in progress, read as Responses events.
pub(crate) struct ApiStream {
    response: reqwest::Response,
    decoder: SseDecoder,
    frames: VecDeque<SseFrame>,
    ready: VecDeque<Value>,
    translator: Translator,
    eof: bool,
    finished: bool,
    observation: Option<RequestObservation>,
}

impl ApiStream {
    fn new(
        response: reqwest::Response,
        wire: ApiWire,
        model: &str,
        observation: Option<RequestObservation>,
    ) -> Self {
        Self {
            response,
            decoder: SseDecoder::default(),
            frames: VecDeque::new(),
            ready: VecDeque::new(),
            translator: Translator::new(wire, model),
            eof: false,
            finished: false,
            observation,
        }
    }

    fn fail(&mut self, error: BridgeError) -> Option<Result<UpstreamFrame, BridgeError>> {
        self.finished = true;
        if let Some(observation) = &self.observation {
            observation.finish(outcome_of(&error));
        }
        Some(Err(error))
    }

    pub(crate) async fn next(&mut self) -> Option<Result<UpstreamFrame, BridgeError>> {
        loop {
            if let Some(value) = self.ready.pop_front() {
                let raw = value.to_string();
                if let Some(observation) = &self.observation {
                    observation.frame_at(&raw, observation.elapsed_us());
                }
                return Some(
                    serde_json::from_value(value)
                        .map(|event| UpstreamFrame { raw, event })
                        .map_err(|error| {
                            BridgeError::upstream(
                                StatusCode::BAD_GATEWAY,
                                format!("the provider sent an event alc cannot read: {error}"),
                            )
                        }),
                );
            }
            if self.finished {
                return None;
            }
            if self.translator.is_finished() {
                self.finished = true;
                continue;
            }
            if let Some(frame) = self.frames.pop_front() {
                let data = frame.data.trim();
                if data.is_empty() {
                    continue;
                }
                if data == "[DONE]" {
                    self.eof = true;
                    self.frames.clear();
                    continue;
                }
                match serde_json::from_str::<Value>(data) {
                    Ok(value) => {
                        let produced = self.translator.accept(value);
                        self.ready.extend(produced);
                    }
                    Err(error) => {
                        return self.fail(BridgeError::upstream(
                            StatusCode::BAD_GATEWAY,
                            format!("the provider sent a stream frame that is not JSON: {error}"),
                        ));
                    }
                }
                continue;
            }
            if self.eof {
                match self.translator.end() {
                    Ok(events) => {
                        self.ready.extend(events);
                        self.finished = true;
                        continue;
                    }
                    Err(error) => return self.fail(error),
                }
            }
            match tokio::time::timeout(BODY_IDLE_TIMEOUT, self.response.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    let frames = self.decoder.push(&chunk);
                    self.frames.extend(frames);
                }
                Ok(Ok(None)) => {
                    self.eof = true;
                    let frames = self.decoder.finish();
                    self.frames.extend(frames);
                }
                Ok(Err(error)) => {
                    return self.fail(BridgeError::upstream(
                        StatusCode::BAD_GATEWAY,
                        format!("the provider's stream broke: {error}"),
                    ));
                }
                Err(_) => {
                    return self.fail(BridgeError::new(
                        StatusCode::GATEWAY_TIMEOUT,
                        "api_error",
                        format!(
                            "the provider sent nothing for {}s; the stream is stalled",
                            BODY_IDLE_TIMEOUT.as_secs()
                        ),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::upstream::{Reasoning, TextOptions, Tool};

    fn request(input: Vec<InputItem>) -> UpstreamRequest {
        UpstreamRequest {
            model: "m".to_owned(),
            kind: None,
            store: false,
            stream: Some(true),
            parallel_tool_calls: false,
            reasoning: Some(Reasoning {
                effort: Some(Effort::High),
                context: Some("all_turns".to_owned()),
                summary: None,
            }),
            text: TextOptions {
                verbosity: Some("low".to_owned()),
                format: None,
            },
            client_metadata: None,
            instructions: None,
            include: None,
            tool_choice: None,
            prompt_cache_key: None,
            input,
            max_output_tokens: Some(1000),
            temperature: None,
        }
    }

    fn conversation() -> Vec<InputItem> {
        vec![
            InputItem::AdditionalTools {
                role: "developer".to_owned(),
                tools: vec![Tool {
                    kind: "function".to_owned(),
                    name: "read".to_owned(),
                    description: Some("Read a file".to_owned()),
                    strict: false,
                    parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                }],
            },
            InputItem::Message {
                role: "developer".to_owned(),
                content: vec![ContentPart::InputText {
                    text: "Be brief.".to_owned(),
                }],
            },
            InputItem::Message {
                role: "user".to_owned(),
                content: vec![ContentPart::InputText {
                    text: "Open a.txt".to_owned(),
                }],
            },
            InputItem::Reasoning {
                id: "rs".to_owned(),
                summary: Vec::new(),
                encrypted_content: "agy1:x:sig".to_owned(),
            },
            InputItem::Message {
                role: "assistant".to_owned(),
                content: vec![ContentPart::OutputText {
                    text: "Reading.".to_owned(),
                }],
            },
            InputItem::FunctionCall {
                id: Some("fc_1".to_owned()),
                call_id: "call_1".to_owned(),
                name: "read".to_owned(),
                arguments: "{\"path\":\"a.txt\"}".to_owned(),
            },
            InputItem::FunctionCallOutput {
                call_id: "call_1".to_owned(),
                output: FunctionCallOutput::Text("hello".to_owned()),
            },
        ]
    }

    #[test]
    fn chat_carries_the_system_prompt_calls_and_results() {
        let body = chat_body(&request(conversation()));
        assert_eq!(
            body["messages"],
            json!([
                {"role": "system", "content": "Be brief."},
                {"role": "user", "content": "Open a.txt"},
                {"role": "assistant", "content": "Reading.", "tool_calls": [
                    {"id": "call_1", "type": "function",
                     "function": {"name": "read", "arguments": "{\"path\":\"a.txt\"}"}}]},
                {"role": "tool", "tool_call_id": "call_1", "content": "hello"},
            ])
        );
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(body["max_tokens"], 1000);
        assert!(
            !body.to_string().contains("agy1:"),
            "no reasoning is replayed"
        );
    }

    #[test]
    fn messages_alternate_roles_and_pair_tool_use_with_its_result() {
        let body = messages_body(&request(conversation()));
        assert_eq!(body["system"], "Be brief.");
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [{"type": "text", "text": "Open a.txt"}]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "Reading."},
                    {"type": "tool_use", "id": "call_1", "name": "read",
                     "input": {"path": "a.txt"}}]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "hello"}]},
            ])
        );
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["max_tokens"], 1000);
    }

    #[test]
    fn responses_use_the_public_api_spelling() {
        let body = responses_body(&request(conversation()));
        assert_eq!(body["instructions"], "Be brief.");
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["reasoning"], json!({"effort": "high"}));
        assert_eq!(body["store"], false);
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 4, "{input:?}");
        assert!(input.iter().all(|item| item.get("id").is_none()));
        assert!(!body.to_string().contains("additional_tools"));
        assert!(!body.to_string().contains("verbosity"));
    }

    #[test]
    fn a_refused_field_is_dropped_once() {
        let mut body = json!({"model": "m", "max_tokens": 5, "temperature": 0.2});
        assert!(drop_refused(
            &mut body,
            br#"{"error":{"message":"Unsupported parameter: 'max_tokens'"}}"#
        ));
        assert!(body.get("max_tokens").is_none());
        assert!(!drop_refused(&mut body, b"something else"));
    }

    fn events(translator: &mut Translator, chunks: &[Value]) -> Vec<Value> {
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(translator.accept(chunk.clone()));
        }
        out.extend(translator.end().unwrap());
        out
    }

    fn types(events: &[Value]) -> Vec<&str> {
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn a_chat_stream_reads_as_responses_events_with_whole_calls() {
        let mut translator = Translator::new(ApiWire::Chat, "m");
        let out = events(
            &mut translator,
            &[
                json!({"choices": [{"delta": {"role": "assistant", "content": "Hi"}}]}),
                json!({"choices": [{"delta": {"tool_calls": [
                    {"index": 0, "id": "call_a", "function": {"name": "read", "arguments": "{\"pa"}}]}}]}),
                json!({"choices": [{"delta": {"tool_calls": [
                    {"index": 0, "function": {"arguments": "th\":\"a\"}"}}]}}]}),
                json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
                json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 4,
                       "total_tokens": 14}}),
            ],
        );
        for event in &out {
            serde_json::from_value::<super::super::upstream::UpstreamEvent>(event.clone()).unwrap();
        }
        let done = out
            .iter()
            .find(|event| {
                event["type"] == "response.output_item.done"
                    && event["item"]["type"] == "function_call"
            })
            .unwrap();
        assert_eq!(done["item"]["call_id"], "call_a");
        assert_eq!(done["item"]["arguments"], "{\"path\":\"a\"}");
        let last = out.last().unwrap();
        assert_eq!(last["type"], "response.completed");
        assert_eq!(last["response"]["usage"]["input_tokens"], 10);
        assert_eq!(types(&out)[0], "response.created");
    }

    #[test]
    fn a_chat_stream_cut_by_length_is_incomplete() {
        let mut translator = Translator::new(ApiWire::Chat, "m");
        let out = events(
            &mut translator,
            &[json!({"choices": [{"delta": {"content": "x"}, "finish_reason": "length"}]})],
        );
        assert_eq!(out.last().unwrap()["type"], "response.incomplete");
    }

    #[test]
    fn a_chat_stream_that_says_nothing_is_an_error() {
        let mut translator = Translator::new(ApiWire::Chat, "m");
        assert!(translator.end().is_err());
    }

    #[test]
    fn a_messages_stream_reads_as_responses_events() {
        let mut translator = Translator::new(ApiWire::Messages, "m");
        let out = events(
            &mut translator,
            &[
                json!({"type": "message_start", "message": {"usage": {"input_tokens": 7,
                       "cache_read_input_tokens": 3, "output_tokens": 1}}}),
                json!({"type": "content_block_start", "index": 0,
                       "content_block": {"type": "thinking", "thinking": ""}}),
                json!({"type": "content_block_delta", "index": 0,
                       "delta": {"type": "thinking_delta", "thinking": "hmm"}}),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "content_block_start", "index": 1,
                       "content_block": {"type": "tool_use", "id": "toolu_1", "name": "read",
                                         "input": {}}}),
                json!({"type": "content_block_delta", "index": 1,
                       "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}}),
                json!({"type": "content_block_delta", "index": 1,
                       "delta": {"type": "input_json_delta", "partial_json": "\"a\"}"}}),
                json!({"type": "content_block_stop", "index": 1}),
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"},
                       "usage": {"output_tokens": 9}}),
                json!({"type": "message_stop"}),
            ],
        );
        for event in &out {
            serde_json::from_value::<super::super::upstream::UpstreamEvent>(event.clone()).unwrap();
        }
        assert!(types(&out).contains(&"response.reasoning_summary_text.delta"));
        let call = out
            .iter()
            .find(|event| event["type"] == "response.function_call_arguments.done")
            .unwrap();
        assert_eq!(call["arguments"], "{\"path\":\"a\"}");
        let last = out.last().unwrap();
        assert_eq!(last["type"], "response.completed");
        assert_eq!(last["response"]["usage"]["input_tokens"], 10);
        assert_eq!(last["response"]["usage"]["output_tokens"], 9);
        assert_eq!(
            last["response"]["usage"]["input_tokens_details"],
            json!({"cached_tokens": 3}),
            "an absent cache-write count is not sent as null"
        );
    }

    /// llama.cpp's own stream, as captured: no `output_index`, reasoning as
    /// `reasoning_text`, a call finished both on its item and in the
    /// completed response.
    #[test]
    fn a_third_party_responses_stream_is_retold_in_full() {
        let mut translator = Translator::new(ApiWire::Responses, "m");
        let call = json!({"id": "fc_x", "type": "function_call", "status": "completed",
            "arguments": "{\"path\":\"a\"}", "call_id": "call_x", "name": "read"});
        let out = events(
            &mut translator,
            &[
                json!({"type": "response.created", "response": {"id": "r", "object": "response",
                       "status": "in_progress"}}),
                json!({"type": "response.output_item.added", "item": {"id": "rs_1",
                       "type": "reasoning", "summary": [], "content": []}}),
                json!({"type": "response.reasoning_text.delta", "delta": "Hm", "item_id": "rs_1"}),
                json!({"type": "response.output_text.delta", "item_id": "msg_1", "delta": "Hi"}),
                json!({"type": "response.output_item.done", "item": call.clone()}),
                json!({"type": "response.completed", "response": {"id": "r", "status": "completed",
                       "output": [call],
                       "usage": {"input_tokens": 5, "output_tokens": 2, "total_tokens": 7}}}),
            ],
        );
        for event in &out {
            serde_json::from_value::<super::super::upstream::UpstreamEvent>(event.clone())
                .unwrap_or_else(|error| panic!("{event}: {error}"));
        }
        let kinds = types(&out);
        assert!(kinds.contains(&"response.reasoning_summary_text.delta"));
        assert!(kinds.contains(&"response.output_text.delta"));
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == "response.function_call_arguments.done")
                .count(),
            1,
            "the call is told once"
        );
        let last = out.last().unwrap();
        assert_eq!(last["type"], "response.completed");
        assert_eq!(last["response"]["usage"]["input_tokens"], 5);
    }
}
