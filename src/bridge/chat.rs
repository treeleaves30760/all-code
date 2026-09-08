//! `POST /v1/chat/completions` — copilot, goose, qwen.
//!
//! Chat Completions is a narrower API than the one upstream speaks, so this
//! surface translates in both directions and always streams upstream: the
//! captured request sets `"stream": true` even though the client asked for a
//! plain completion, and the non-streaming answer is assembled from the SSE.
//! There is no non-streaming upstream worth using.
//!
//! Identifier note, straight off the `chat-completions-nonstreaming` fixture:
//! the completion `id` is the upstream response id with `resp_` swapped for
//! `chatcmpl-` — `resp_0232a97d…` became `chatcmpl-0232a97d…` — and `created`
//! is the response's `created_at`. Both are borrowed rather than generated so
//! a bridge answer can be traced back to an upstream turn.
//!
//! The vendored bridge's version of this surface has no tool support at all;
//! it rejects any request carrying `tools` and models only text. alc needs
//! tool calls on all three surfaces, so this file goes further than the crate
//! it replaces, and has no capture to copy for the tool path. What fills the
//! gap is the *other* fixture: `messages-tool-call-streaming` carries the same
//! upstream tool round trip — `additional_tools` up, `function_call` items
//! back — and only the client-facing half differs. The shapes here are that
//! capture's, re-dressed in OpenAI's clothes.
//!
//! The transport is not this file's: [`upstream::send`] posts the body, signs
//! it, rotates a refused token and decodes the SSE, because all three surfaces
//! need exactly that and only one of them should own it. What is left here is
//! the translation on either side of it.
//!
//! One deliberate asymmetry: fields this endpoint has nowhere to put are
//! dropped rather than refused. `temperature`, `top_p`, `max_tokens`, `user`
//! and a message's `name` reach no field here and travel no further —
//! chatgpt.com refuses the first four outright, so there is nothing to relay
//! them into. The bridge being replaced answered 400 for every one of them,
//! which is defensible against a spec and useless against an agent that sends
//! `temperature` on every request and cannot be told not to. The choice is
//! between dropping them and refusing turns over them.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::upstream::{
    self, ContentPart, Effort, FunctionCallOutput, InputItem, OutputItem, Reasoning,
    ResponseObject, TextOptions, Tool, UpstreamEvent, UpstreamFrame, UpstreamRequest,
    UpstreamStream, Usage,
};
use super::{BridgeConfig, BridgeError, BridgeState};
use crate::config::ReasoningEffort;

/// The frame OpenAI's clients wait for before they stop reading.
const DONE_FRAME: &str = "data: [DONE]\n\n";

/// Stands in until `response.created` names the turn.
///
/// Unreachable in practice — the response id arrives in the first frame of
/// every captured stream, before any content the client could see — so this is
/// the shape of a stream that broke before saying anything at all, and it is
/// spelled to be recognisable in a log rather than to look like a real id.
const UNNAMED_COMPLETION: &str = "chatcmpl-unnamed";

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    /// `{"include_usage": true}` asks for a final usage-only chunk.
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    /// `low` | `medium` | `high` — the Chat spelling of upstream's effort
    /// ladder. Absent means medium, matching the captured request.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub response_format: Option<Value>,
    #[serde(default)]
    pub tools: Vec<ChatTool>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

/// One chat message.
///
/// `role: "tool"` carries a result and pairs with `tool_call_id`; `role:
/// "system"` and `"developer"` both become upstream's `developer` role.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ChatMessage {
    pub role: String,
    /// A string, a list of parts, or `null` on an assistant turn that only
    /// called tools.
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(default)]
    pub tool_calls: Vec<ChatToolCall>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ChatFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatFunction {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatToolCall {
    /// Upstream's `call_id`, not its `id`. See [`super::messages`].
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ChatFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatFunctionCall {
    pub name: String,
    /// JSON as a string, exactly as upstream sends it.
    pub arguments: String,
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatCompletion {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatChoice {
    pub index: u32,
    pub message: ChatResponseMessage,
    /// `stop`, `tool_calls`, or `length`.
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatResponseMessage {
    pub role: &'static str,
    /// Omitted rather than empty on a tool-call-only turn: some clients treat
    /// `""` as a real (blank) answer and stop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCall>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct ChatUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// One `chat.completion.chunk` frame.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatChunk {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChunkChoice>,
    /// Only on the final chunk, and only when `stream_options.include_usage`
    /// asked for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatChunkChoice {
    pub index: u32,
    pub delta: ChatDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// A streaming delta. Every field is optional: the first chunk carries only
/// `role`, argument chunks carry only a partial `tool_calls` entry, and the
/// last carries nothing but a finish reason.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ChatDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCallDelta>,
}

/// A partial tool call. `index` is what a client reassembles on; `id` and
/// `function.name` appear once, on the first delta of a call, and `arguments`
/// accumulates across the rest.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    pub function: ChatFunctionCallDelta,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ChatFunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Turns a Chat Completions request into the upstream body.
///
/// Reproduces the `chat-completions-nonstreaming` capture exactly: `stream`
/// true whatever the client asked for, `client_metadata` keyed `lite` (the
/// HTTP lane's spelling — the WebSocket one spells the header name into the
/// key), `store` false, `parallel_tool_calls` false, `text.verbosity` low and
/// `reasoning.context` `all_turns`.
///
/// Effort is the request's own `reasoning_effort` first, the launch-pinned
/// [`BridgeConfig::effort`] second, and `medium` last — the client is closer
/// to the turn than the launch was. `none` keeps `reasoning.context` and drops
/// only the effort key, which is how the endpoint is asked for no reasoning
/// without also losing continuity across a tool round trip.
///
/// Tools become the one `additional_tools` developer item the Messages fixture
/// shows, and a prior turn's `tool_calls` / `role: "tool"` messages become
/// `function_call` / `function_call_output` items keyed by the call id the
/// client was given.
pub(crate) fn to_upstream(
    request: &ChatRequest,
    config: &BridgeConfig,
) -> Result<UpstreamRequest, BridgeError> {
    if request.model.trim().is_empty() {
        return Err(BridgeError::invalid(
            "'model' is required and must name a Codex model",
        ));
    }
    if request.messages.is_empty() {
        return Err(BridgeError::invalid(
            "'messages' must contain at least one message",
        ));
    }

    let mut input = Vec::with_capacity(request.messages.len() + 1);
    if !request.tools.is_empty() {
        input.push(InputItem::AdditionalTools {
            role: "developer".to_owned(),
            tools: request
                .tools
                .iter()
                .map(translate_tool)
                .collect::<Result<Vec<_>, _>>()?,
        });
    }
    for (index, message) in request.messages.iter().enumerate() {
        translate_message(index, message, &mut input)?;
    }
    if !input
        .iter()
        .any(|item| !matches!(item, InputItem::AdditionalTools { .. }))
    {
        return Err(BridgeError::invalid(
            "'messages' carried no content to send; every message was empty",
        ));
    }

    let effort = resolve_effort(request.reasoning_effort.as_deref(), config.effort)?;
    Ok(UpstreamRequest {
        model: request.model.clone(),
        kind: None,
        store: false,
        stream: Some(true),
        parallel_tool_calls: false,
        reasoning: Some(Reasoning {
            effort: (effort != Effort::None).then_some(effort),
            context: Some("all_turns".to_owned()),
            summary: None,
        }),
        text: TextOptions {
            verbosity: Some("low".to_owned()),
            format: text_format(request.response_format.as_ref())?,
        },
        client_metadata: Some(BTreeMap::from([(
            upstream::LITE_METADATA_KEY.to_owned(),
            "true".to_owned(),
        )])),
        instructions: None,
        include: None,
        // Only meaningful alongside tools, and a choice naming a function the
        // request never registered is how the endpoint is made to 502.
        tool_choice: if request.tools.is_empty() {
            None
        } else {
            tool_choice(request.tool_choice.as_ref())
        },
        prompt_cache_key: None,
        input,
    })
}

/// `strict` stays false for the reason the [`Tool`] doc gives: agent-supplied
/// schemas are not reliably strict-mode legal, and a schema the server rejects
/// costs the whole turn rather than one bad argument.
fn translate_tool(tool: &ChatTool) -> Result<Tool, BridgeError> {
    if tool.kind != "function" {
        return Err(BridgeError::invalid(format!(
            "unsupported tool type {:?}; Codex takes only 'function' tools",
            tool.kind
        )));
    }
    Ok(Tool {
        kind: "function".to_owned(),
        name: tool.function.name.clone(),
        description: tool.function.description.clone(),
        strict: false,
        // A function declared with no parameters still needs a schema: the
        // server reads `parameters` unconditionally.
        parameters: tool
            .function
            .parameters
            .clone()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
    })
}

/// Appends the upstream items one chat message becomes.
///
/// One message is not one item. An assistant turn that both spoke and called a
/// tool is a `message` followed by a `function_call`, and a message with
/// nothing in it contributes nothing rather than an empty item the server
/// would refuse.
fn translate_message(
    index: usize,
    message: &ChatMessage,
    input: &mut Vec<InputItem>,
) -> Result<(), BridgeError> {
    let param = format!("messages[{index}]");
    match message.role.as_str() {
        "system" | "developer" => {
            push_message(input, "developer", message, false, &param)?;
        }
        "user" => {
            push_message(input, "user", message, false, &param)?;
        }
        "assistant" => {
            push_message(input, "assistant", message, true, &param)?;
            for call in &message.tool_calls {
                input.push(InputItem::FunctionCall {
                    // Upstream's own item handle is not ours to invent, and it
                    // is optional on the way up; `call_id` is the identifier
                    // that has to survive the round trip.
                    id: None,
                    call_id: call.id.clone(),
                    name: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                });
            }
        }
        "tool" | "function" => {
            let call_id = message
                .tool_call_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    BridgeError::invalid(format!(
                        "'{param}.tool_call_id' is required on a tool result"
                    ))
                })?;
            input.push(InputItem::FunctionCallOutput {
                call_id: call_id.to_owned(),
                output: tool_output(message.content.as_ref()),
            });
        }
        other => {
            return Err(BridgeError::invalid(format!(
                "'{param}.role' is {other:?}; expected system, developer, user, assistant or tool"
            )));
        }
    }
    Ok(())
}

fn push_message(
    input: &mut Vec<InputItem>,
    role: &str,
    message: &ChatMessage,
    assistant: bool,
    param: &str,
) -> Result<(), BridgeError> {
    let content = content_parts(message.content.as_ref(), assistant, param)?;
    if !content.is_empty() {
        input.push(InputItem::Message {
            role: role.to_owned(),
            content,
        });
    }
    Ok(())
}

/// The `input_text`/`output_text` split is not cosmetic: upstream types its
/// content parts by direction, and an assistant turn replayed as `input_text`
/// comes back read as something the user said.
fn content_parts(
    content: Option<&Value>,
    assistant: bool,
    param: &str,
) -> Result<Vec<ContentPart>, BridgeError> {
    let text_part = |text: String| {
        if assistant {
            ContentPart::OutputText { text }
        } else {
            ContentPart::InputText { text }
        }
    };
    match content {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) if text.is_empty() => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(vec![text_part(text.clone())]),
        Some(Value::Array(parts)) => {
            let mut translated = Vec::with_capacity(parts.len());
            for (index, part) in parts.iter().enumerate() {
                let param = format!("{param}.content[{index}]");
                let object = part
                    .as_object()
                    .ok_or_else(|| BridgeError::invalid(format!("'{param}' must be an object")))?;
                match object.get("type").and_then(Value::as_str) {
                    Some("text" | "input_text" | "output_text") => {
                        let text = object
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !text.is_empty() {
                            translated.push(text_part(text.to_owned()));
                        }
                    }
                    Some("image_url") => {
                        translated.push(image_part(object.get("image_url"), &param)?)
                    }
                    Some(other) => {
                        return Err(BridgeError::invalid(format!(
                            "'{param}.type' is {other:?}; Codex takes text and image_url parts"
                        )));
                    }
                    None => {
                        return Err(BridgeError::invalid(format!("'{param}' has no 'type'")));
                    }
                }
            }
            Ok(translated)
        }
        Some(_) => Err(BridgeError::invalid(format!(
            "'{param}.content' must be a string or a list of content parts"
        ))),
    }
}

/// Both spellings clients use: the documented object, and the bare string some
/// send instead. A data URI and an https URL are the same field to upstream.
fn image_part(image: Option<&Value>, param: &str) -> Result<ContentPart, BridgeError> {
    match image {
        Some(Value::String(url)) => Ok(ContentPart::InputImage {
            image_url: url.clone(),
            detail: None,
        }),
        Some(Value::Object(image)) => {
            let url = image
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
                .ok_or_else(|| {
                    BridgeError::invalid(format!("'{param}.image_url.url' is required"))
                })?;
            Ok(ContentPart::InputImage {
                image_url: url.to_owned(),
                detail: image
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        }
        _ => Err(BridgeError::invalid(format!(
            "'{param}.image_url' must be a string or an object with a url"
        ))),
    }
}

/// A tool result always goes up as a string.
///
/// [`FunctionCallOutput::Parts`] exists for what upstream sends, not for what
/// it accepts here: the captured `function_call_output` items are strings, and
/// a client's structured result is stringified rather than reshaped into a
/// part list nothing has been observed to take.
fn tool_output(content: Option<&Value>) -> FunctionCallOutput {
    let text = match content {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
    };
    FunctionCallOutput::Text(text)
}

/// The request's effort, then the pinned one, then medium.
fn resolve_effort(
    requested: Option<&str>,
    pinned: Option<ReasoningEffort>,
) -> Result<Effort, BridgeError> {
    match requested {
        Some(requested) => parse_effort(requested),
        None => Ok(pinned.map_or(Effort::Medium, Effort::from)),
    }
}

fn parse_effort(value: &str) -> Result<Effort, BridgeError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(Effort::None),
        "low" => Ok(Effort::Low),
        "medium" => Ok(Effort::Medium),
        "high" => Ok(Effort::High),
        "xhigh" | "x-high" => Ok(Effort::Xhigh),
        "max" => Ok(Effort::Max),
        // alc's own top tier, clamped the same way a pinned one is: the word
        // reaches GPT-6 natively but this endpoint refuses it.
        "ultra" => Ok(Effort::Max),
        other => Err(BridgeError::invalid(format!(
            "'reasoning_effort' is {other:?}; expected none, low, medium, high, xhigh or max"
        ))),
    }
}

/// `response_format` becomes `text.format`.
///
/// `{"type":"text"}` is dropped rather than forwarded: it is the default, and
/// the captured request omits `text.format` entirely.
fn text_format(value: Option<&Value>) -> Result<Option<Value>, BridgeError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| BridgeError::invalid("'response_format' must be an object"))?;
    match object.get("type").and_then(Value::as_str) {
        None | Some("text") => Ok(None),
        Some("json_object") => Ok(Some(json!({ "type": "json_object" }))),
        Some("json_schema") => {
            let schema = object
                .get("json_schema")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    BridgeError::invalid("'response_format.json_schema' must be an object")
                })?;
            let name = schema
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    BridgeError::invalid("'response_format.json_schema.name' is required")
                })?;
            let mut format = json!({
                "type": "json_schema",
                "name": name,
                "schema": schema.get("schema").cloned().unwrap_or_else(|| json!({"type": "object"})),
            });
            if let Some(strict) = schema.get("strict").and_then(Value::as_bool) {
                format["strict"] = Value::Bool(strict);
            }
            Ok(Some(format))
        }
        Some(other) => Err(BridgeError::invalid(format!(
            "'response_format.type' is {other:?}; expected text, json_object or json_schema"
        ))),
    }
}

/// Chat's tool choice in the Responses spelling: the modes are bare strings,
/// and a named function drops the `function` wrapper.
///
/// A choice this cannot read becomes `None` rather than an error — the default
/// is `auto`, which is what an unreadable choice most likely meant, and
/// refusing the turn over it helps nobody.
fn tool_choice(value: Option<&Value>) -> Option<Value> {
    match value? {
        Value::String(mode) => mode_choice(mode),
        Value::Object(choice) => match choice.get("type").and_then(Value::as_str) {
            Some("function") => {
                let name = choice
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .or_else(|| choice.get("name"))
                    .and_then(Value::as_str)?;
                Some(json!({ "type": "function", "name": name }))
            }
            Some(mode) => mode_choice(mode),
            None => None,
        },
        _ => None,
    }
}

fn mode_choice(mode: &str) -> Option<Value> {
    match mode {
        "auto" => Some(json!("auto")),
        "none" => Some(json!("none")),
        "required" | "any" => Some(json!("required")),
        _ => None,
    }
}

/// The completion id for an upstream response id.
///
/// Swaps the `resp_` prefix for `chatcmpl-` and leaves the rest alone, so
/// `resp_0232a97d…` becomes `chatcmpl-0232a97d…` as captured. An id without
/// that prefix keeps its whole self after `chatcmpl-`.
pub(crate) fn completion_id(response_id: &str) -> String {
    if response_id.starts_with("chatcmpl-") {
        return response_id.to_owned();
    }
    format!(
        "chatcmpl-{}",
        response_id.strip_prefix("resp_").unwrap_or(response_id)
    )
}

// ---------------------------------------------------------------------------
// Folding the upstream stream
// ---------------------------------------------------------------------------

/// One turn's events.
///
/// A seam over [`UpstreamStream`], which owns a live `reqwest::Response` and
/// so cannot exist without a socket. The fold and the streamer below are the
/// half of this file worth testing, and this is what lets the captured SSE be
/// replayed through exactly the code a real turn runs.
enum Turn {
    Live(UpstreamStream),
    #[cfg(test)]
    Replay(VecDeque<Result<UpstreamFrame, BridgeError>>),
}

impl Turn {
    async fn next(&mut self) -> Option<Result<UpstreamFrame, BridgeError>> {
        match self {
            Self::Live(stream) => stream.next().await,
            #[cfg(test)]
            Self::Replay(frames) => frames.pop_front(),
        }
    }
}

/// One tool call, assembled across the events that describe it.
///
/// `item_id` and `call_id` are both kept because upstream uses them for
/// different things: the argument deltas key off the item id, and the client's
/// next `tool` message keys off the call id.
#[derive(Debug, Clone)]
struct PendingCall {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
}

/// The upstream turn, folded into what a Chat Completions client is owed.
///
/// Shared by both answer shapes on purpose: the streaming and non-streaming
/// paths differ only in whether they forward the deltas [`observe`] hands back
/// or throw them away and read the final state.
///
/// [`observe`]: CompletionState::observe
#[derive(Debug, Clone)]
struct CompletionState {
    id: Option<String>,
    created: u64,
    model: String,
    text: String,
    saw_text_delta: bool,
    calls: Vec<PendingCall>,
    usage: Usage,
    finish_reason: &'static str,
    completed: bool,
}

impl CompletionState {
    fn new(model: &str) -> Self {
        Self {
            id: None,
            created: unix_seconds(),
            model: model.to_owned(),
            text: String::new(),
            saw_text_delta: false,
            calls: Vec::new(),
            usage: Usage::default(),
            finish_reason: "stop",
            completed: false,
        }
    }

    fn id(&self) -> &str {
        self.id.as_deref().unwrap_or(UNNAMED_COMPLETION)
    }

    /// Folds one upstream frame in, returning the deltas a streaming client
    /// should see for it. Most frames produce none: the lifecycle events carry
    /// metadata, and Chat Completions has no field for reasoning items at all.
    ///
    /// Takes the whole frame rather than its decoded event because a refusal's
    /// reason lives in fields no struct here claims, and
    /// [`upstream::error_message`] reads it off the raw payload.
    fn observe(&mut self, frame: &UpstreamFrame) -> Result<Vec<ChatDelta>, BridgeError> {
        match &frame.event {
            UpstreamEvent::Created { response } | UpstreamEvent::InProgress { response } => {
                self.metadata(response);
            }
            UpstreamEvent::OutputItemAdded {
                item:
                    OutputItem::FunctionCall {
                        id,
                        call_id,
                        name,
                        arguments,
                        ..
                    },
                ..
            } => return Ok(vec![self.open_call(id, call_id, name, arguments)]),
            UpstreamEvent::OutputItemDone {
                item:
                    OutputItem::FunctionCall {
                        id,
                        call_id,
                        name,
                        arguments,
                        ..
                    },
                ..
            } => {
                return Ok(match self.position(id) {
                    Some(index) => self.settle(index, arguments),
                    // No `output_item.added` was seen for this call, so
                    // emitting it whole here cannot duplicate one.
                    None => vec![self.open_call(id, call_id, name, arguments)],
                });
            }
            UpstreamEvent::OutputTextDelta { delta, .. } => {
                self.text.push_str(delta);
                self.saw_text_delta = true;
                return Ok(vec![ChatDelta {
                    content: Some(delta.clone()),
                    ..ChatDelta::default()
                }]);
            }
            // The deltas are authoritative when they arrived; this is the
            // fallback for a stream that only announced its text once.
            UpstreamEvent::OutputTextDone { text, .. } if !self.saw_text_delta => {
                self.text.push_str(text);
                return Ok(vec![ChatDelta {
                    content: Some(text.clone()),
                    ..ChatDelta::default()
                }]);
            }
            UpstreamEvent::FunctionCallArgumentsDelta { item_id, delta, .. } => {
                if let Some(index) = self.position(item_id) {
                    self.calls[index].arguments.push_str(delta);
                    return Ok(vec![argument_delta(index, delta)]);
                }
            }
            UpstreamEvent::FunctionCallArgumentsDone {
                item_id, arguments, ..
            } => {
                if let Some(index) = self.position(item_id) {
                    return Ok(self.settle(index, arguments));
                }
            }
            UpstreamEvent::Completed { response } => {
                self.metadata(response);
                match response.status.as_deref() {
                    Some("failed") => return Err(turn_failure(frame)),
                    Some("incomplete") => self.finish_reason = "length",
                    _ => {}
                }
                self.completed = true;
            }
            UpstreamEvent::Incomplete { response } => {
                self.metadata(response);
                self.finish_reason = "length";
                self.completed = true;
            }
            UpstreamEvent::Failed { response } => {
                self.metadata(response);
                return Err(turn_failure(frame));
            }
            UpstreamEvent::Error { .. } => return Err(turn_failure(frame)),
            // A limit reached with credits left is not terminal: the turn
            // succeeds, and the event is only saying the window is used up.
            UpstreamEvent::RateLimits(limits) if limits.is_terminal() => {
                return Err(BridgeError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limit_error",
                    "the ChatGPT plan behind the Codex credentials is out of quota and has no \
                     credits left",
                ));
            }
            _ => {}
        }
        Ok(Vec::new())
    }

    fn metadata(&mut self, response: &ResponseObject) {
        self.id = Some(completion_id(&response.id));
        if let Some(model) = response.model.as_deref() {
            self.model = model.to_owned();
        }
        if let Some(created) = response.created_at {
            self.created = created;
        }
        if let Some(usage) = response.usage {
            self.usage = usage;
        }
    }

    fn position(&self, item_id: &str) -> Option<usize> {
        self.calls.iter().position(|call| call.item_id == item_id)
    }

    fn open_call(
        &mut self,
        item_id: &str,
        call_id: &str,
        name: &str,
        arguments: &str,
    ) -> ChatDelta {
        let index = self.calls.len();
        self.calls.push(PendingCall {
            item_id: item_id.to_owned(),
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        });
        ChatDelta {
            tool_calls: vec![ChatToolCallDelta {
                index: index as u32,
                // Both appear once, on the opening delta of a call, and the id
                // is the *call* id: it is what the client sends back.
                id: Some(call_id.to_owned()),
                kind: Some("function"),
                function: ChatFunctionCallDelta {
                    name: Some(name.to_owned()),
                    arguments: Some(arguments.to_owned()),
                },
            }],
            ..ChatDelta::default()
        }
    }

    /// Takes upstream's final word on a call's arguments and streams only what
    /// the client has not already been sent.
    ///
    /// Upstream restates the whole argument string on `.done` after streaming
    /// it in pieces. Forwarding that verbatim would double every tool call, so
    /// only the unsent tail goes out — and when the restatement is not an
    /// extension of what was streamed, nothing does: the client cannot unsend
    /// the deltas it already appended.
    fn settle(&mut self, index: usize, arguments: &str) -> Vec<ChatDelta> {
        if arguments.is_empty() {
            return Vec::new();
        }
        let call = &mut self.calls[index];
        let tail = arguments
            .strip_prefix(call.arguments.as_str())
            .map(str::to_owned);
        call.arguments = arguments.to_owned();
        match tail {
            Some(tail) if !tail.is_empty() => vec![argument_delta(index, &tail)],
            _ => Vec::new(),
        }
    }

    /// `tool_calls` outranks `stop`: a turn that called a tool did not finish
    /// talking, and a client told otherwise never sends the result back.
    fn finish_reason(&self) -> &'static str {
        if self.finish_reason == "stop" && !self.calls.is_empty() {
            "tool_calls"
        } else {
            self.finish_reason
        }
    }

    fn usage(&self) -> ChatUsage {
        ChatUsage {
            prompt_tokens: self.usage.input_tokens,
            completion_tokens: self.usage.output_tokens,
            total_tokens: self
                .usage
                .input_tokens
                .saturating_add(self.usage.output_tokens),
        }
    }

    fn tool_calls(&self) -> Vec<ChatToolCall> {
        self.calls
            .iter()
            .map(|call| ChatToolCall {
                id: call.call_id.clone(),
                kind: "function".to_owned(),
                function: ChatFunctionCall {
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                },
            })
            .collect()
    }

    /// True when the turn produced nothing a client could act on. Distinct
    /// from "did not complete": this one completed and said nothing.
    fn is_empty(&self) -> bool {
        self.text.is_empty() && self.calls.is_empty()
    }

    fn completion(&self) -> ChatCompletion {
        ChatCompletion {
            id: self.id().to_owned(),
            object: "chat.completion",
            created: self.created,
            model: self.model.clone(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatResponseMessage {
                    role: "assistant",
                    content: (!self.text.is_empty()).then(|| self.text.clone()),
                    tool_calls: self.tool_calls(),
                },
                finish_reason: Some(self.finish_reason().to_owned()),
            }],
            usage: Some(self.usage()),
        }
    }
}

fn argument_delta(index: usize, arguments: &str) -> ChatDelta {
    ChatDelta {
        tool_calls: vec![ChatToolCallDelta {
            index: index as u32,
            id: None,
            kind: None,
            function: ChatFunctionCallDelta {
                name: None,
                arguments: Some(arguments.to_owned()),
            },
        }],
        ..ChatDelta::default()
    }
}

/// The reason a turn failed, in the server's own words.
///
/// Read off the raw frame rather than the parsed one: upstream spells its
/// complaint four different ways depending on how far the request got, and
/// [`upstream::error_message`] knows all four. Acceptance criterion 5 turns on
/// finding whichever one is there.
fn turn_failure(frame: &UpstreamFrame) -> BridgeError {
    BridgeError::upstream(
        StatusCode::BAD_GATEWAY,
        upstream::error_message(frame.raw.as_bytes())
            .unwrap_or_else(|| "chatgpt.com ended the turn without saying why".to_owned()),
    )
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Reads the whole turn and answers one completion.
///
/// Stops at the terminal event rather than at end-of-stream: upstream keeps a
/// pooled connection open after `response.completed`, so waiting for EOF would
/// add the idle timeout to the end of every non-streaming request.
async fn aggregate(mut turn: Turn, model: &str) -> Result<ChatCompletion, BridgeError> {
    let mut state = CompletionState::new(model);
    while let Some(frame) = turn.next().await {
        let frame = frame?;
        state.observe(&frame)?;
        if frame.event.is_terminal() {
            break;
        }
    }
    if !state.completed {
        return Err(BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            "chatgpt.com closed the stream before the turn completed",
        ));
    }
    if state.is_empty() {
        return Err(BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            "chatgpt.com completed the turn without producing any output",
        ));
    }
    Ok(state.completion())
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Chat Completions frames carry no `event:` name.
///
/// This is why they do not go through [`super::sse_frame`], which always
/// writes one: OpenAI's own stream is bare `data:` lines, and a client
/// listening for the default SSE message event never sees a named one. The
/// Anthropic surface has the opposite requirement, which is why the shared
/// helper exists at all.
fn data_frame(value: &impl Serialize) -> String {
    let data = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned());
    format!("data: {data}\n\n")
}

/// The streaming half: one upstream frame in, zero or more chunks out.
struct StreamPump {
    turn: Turn,
    queued: VecDeque<String>,
    state: CompletionState,
    role_sent: bool,
    include_usage: bool,
    ended: bool,
}

impl StreamPump {
    fn chunk(&self, delta: ChatDelta, finish_reason: Option<&'static str>) -> String {
        data_frame(&ChatChunk {
            id: self.state.id().to_owned(),
            object: "chat.completion.chunk",
            created: self.state.created,
            model: self.state.model.clone(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta,
                finish_reason: finish_reason.map(str::to_owned),
            }],
            // Usage rides the final chunk only, and only when asked for: a
            // client that did not ask reads an extra field as an extra choice.
            usage: finish_reason
                .filter(|_| self.include_usage)
                .map(|_| self.state.usage()),
        })
    }

    /// Queues one delta, preceded once by the role-only chunk every OpenAI
    /// client expects to open a stream.
    fn push(&mut self, delta: ChatDelta) {
        if !self.role_sent {
            self.role_sent = true;
            let opening = self.chunk(
                ChatDelta {
                    role: Some("assistant"),
                    ..ChatDelta::default()
                },
                None,
            );
            self.queued.push_back(opening);
        }
        let chunk = self.chunk(delta, None);
        self.queued.push_back(chunk);
    }

    fn observe(&mut self, frame: &UpstreamFrame) {
        match self.state.observe(frame) {
            Ok(deltas) => {
                for delta in deltas {
                    self.push(delta);
                }
                if self.state.completed {
                    self.close();
                }
            }
            Err(error) => self.fail(error),
        }
    }

    fn close(&mut self) {
        if self.ended {
            return;
        }
        if !self.state.completed {
            self.fail(BridgeError::upstream(
                StatusCode::BAD_GATEWAY,
                "chatgpt.com closed the stream before the turn completed",
            ));
            return;
        }
        if self.state.is_empty() {
            self.fail(BridgeError::upstream(
                StatusCode::BAD_GATEWAY,
                "chatgpt.com completed the turn without producing any output",
            ));
            return;
        }
        let terminal = self.chunk(ChatDelta::default(), Some(self.state.finish_reason()));
        self.queued.push_back(terminal);
        self.queued.push_back(DONE_FRAME.to_owned());
        self.ended = true;
    }

    /// A failure after the 200 has gone out cannot change the status, so it
    /// travels down the stream in the envelope the client is already parsing.
    fn fail(&mut self, error: BridgeError) {
        if self.ended {
            return;
        }
        self.queued.push_back(data_frame(&json!({
            "error": {
                "message": error.message,
                "type": error.kind,
                "param": Value::Null,
                "code": Value::Null,
            },
        })));
        self.queued.push_back(DONE_FRAME.to_owned());
        self.ended = true;
    }
}

fn stream_response(turn: Turn, model: String, include_usage: bool) -> Response {
    let pump = StreamPump {
        turn,
        queued: VecDeque::new(),
        state: CompletionState::new(&model),
        role_sent: false,
        include_usage,
        ended: false,
    };
    let frames =
        futures_util::stream::unfold(Some(pump), |pump| async move { next_frame(pump).await });
    let mut response = Response::new(Body::from_stream(frames));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// Pulls until there is a frame to hand out, or the turn is over.
///
/// The `Option` in both positions is what `unfold` uses to end a stream: `None`
/// as the next state means this frame was the last one.
#[allow(clippy::type_complexity)]
async fn next_frame(
    pump: Option<StreamPump>,
) -> Option<(Result<Bytes, std::io::Error>, Option<StreamPump>)> {
    let mut pump = pump?;
    loop {
        if let Some(frame) = pump.queued.pop_front() {
            let next = if pump.ended && pump.queued.is_empty() {
                None
            } else {
                Some(pump)
            };
            return Some((Ok(Bytes::from(frame)), next));
        }
        if pump.ended {
            return None;
        }
        match pump.turn.next().await {
            Some(Ok(frame)) => pump.observe(&frame),
            Some(Err(error)) => pump.fail(error),
            None => pump.close(),
        }
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /v1/chat/completions`.
///
/// Every refusal leaves through [`BridgeError::openai`], which is the envelope
/// copilot, goose and qwen parse; nothing reaches the client as a bare status.
pub(crate) async fn handle(State(state): State<Arc<BridgeState>>, body: Bytes) -> Response {
    match respond(&state, body).await {
        Ok(response) => response,
        Err(error) => error.openai(),
    }
}

async fn respond(state: &BridgeState, body: Bytes) -> Result<Response, BridgeError> {
    let request: ChatRequest = serde_json::from_slice(&body).map_err(|error| {
        BridgeError::invalid(format!(
            "the request body is not a Chat Completions request: {error}"
        ))
    })?;
    let upstream_request = to_upstream(&request, &state.config)?;
    // The model the *answer* is labelled with, before upstream names its own:
    // a client that pinned a slug should see that slug come back even on the
    // paths where the response object never arrives.
    let model = upstream_request.model.clone();
    let turn = Turn::Live(upstream::send(state, upstream_request).await?);

    if request.stream {
        let include_usage = request
            .stream_options
            .is_some_and(|options| options.include_usage);
        return Ok(stream_response(turn, model, include_usage));
    }
    Ok(axum::Json(aggregate(turn, &model).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::fixtures::Case;
    use std::path::PathBuf;

    fn config(effort: Option<ReasoningEffort>) -> BridgeConfig {
        BridgeConfig {
            auth_file: PathBuf::from("/nonexistent/auth.json"),
            effort,
            responses_api: true,
        }
    }

    impl Turn {
        /// Replays a canned body through the same decoder a live turn uses.
        ///
        /// The chunking is part of the test: a body handed over whole never
        /// exercises the frame that arrives in three pieces, which is every
        /// frame on a real socket.
        fn replay(chunks: &[&str]) -> Self {
            let mut decoder = upstream::SseDecoder::default();
            let mut frames: Vec<_> = chunks
                .iter()
                .flat_map(|chunk| decoder.push(chunk.as_bytes()))
                .collect();
            frames.extend(decoder.finish());
            Self::Replay(
                frames
                    .iter()
                    .map(|frame| frame.data.trim())
                    .filter(|data| !data.is_empty() && *data != "[DONE]")
                    .map(|data| {
                        serde_json::from_str(data)
                            .map(|event| UpstreamFrame {
                                raw: data.to_owned(),
                                event,
                            })
                            .map_err(|error| {
                                BridgeError::upstream(
                                    StatusCode::BAD_GATEWAY,
                                    format!("not an upstream event: {error}"),
                                )
                            })
                    })
                    .collect(),
            )
        }
    }

    /// Frames events the way the HTTP lane does, `event:` line and all.
    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {}\n\n",
                    event["type"].as_str().unwrap(),
                    serde_json::to_string(event).unwrap()
                )
            })
            .collect()
    }

    /// One turn that calls a tool, transcribed from the shapes in
    /// `messages-tool-call-streaming` — the only capture of this upstream
    /// exchange there is.
    fn tool_events() -> Vec<Value> {
        vec![
            json!({"type": "response.created", "response": {
                "id": "resp_tool", "model": "gpt-6-astra",
                "created_at": 1788901776, "status": "in_progress"}}),
            json!({"type": "response.output_item.added", "output_index": 0, "item": {
                "id": "fc_1", "type": "function_call", "status": "in_progress",
                "arguments": "", "call_id": "call_abc", "name": "read_file"}}),
            json!({"type": "response.function_call_arguments.delta",
                "output_index": 0, "item_id": "fc_1", "delta": "{\"path\":"}),
            json!({"type": "response.function_call_arguments.delta",
                "output_index": 0, "item_id": "fc_1", "delta": "\"/etc/hosts\"}"}),
            json!({"type": "response.function_call_arguments.done", "output_index": 0,
                "item_id": "fc_1", "arguments": "{\"path\":\"/etc/hosts\"}"}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {
                "id": "fc_1", "type": "function_call", "status": "completed",
                "arguments": "{\"path\":\"/etc/hosts\"}", "call_id": "call_abc",
                "name": "read_file"}}),
            json!({"type": "response.completed", "response": {
                "id": "resp_tool", "model": "gpt-6-astra", "created_at": 1788901776,
                "status": "completed",
                "usage": {"input_tokens": 41, "output_tokens": 9, "total_tokens": 50}}}),
        ]
    }

    /// The streamed body, split back into the chunk objects a client parses.
    /// Asserts the terminator on the way through, because a stream missing it
    /// hangs a client rather than failing it.
    async fn streamed(response: Response) -> Vec<Value> {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the stream completes");
        let text = String::from_utf8(body.to_vec()).expect("frames are UTF-8");
        assert!(text.ends_with(DONE_FRAME), "no [DONE] terminator in {text}");
        upstream::parse_sse(&text)
            .into_iter()
            .filter(|frame| frame.data != "[DONE]")
            .map(|frame| serde_json::from_str(&frame.data).expect("a frame is JSON"))
            .collect()
    }

    #[test]
    fn the_captured_request_becomes_the_captured_upstream_body() {
        let case = Case::load("chat-completions-nonstreaming");
        let request: ChatRequest = serde_json::from_value(case.client_request()).unwrap();
        let built = to_upstream(&request, &config(None)).unwrap();
        assert_eq!(
            serde_json::to_value(&built).unwrap(),
            case.upstream_request()
        );
    }

    #[tokio::test]
    async fn the_captured_stream_becomes_the_captured_completion() {
        let case = Case::load("chat-completions-nonstreaming");
        let sse = case.upstream_sse();
        let completion = aggregate(Turn::replay(&[&sse]), "gpt-5.6-terra")
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&completion).unwrap(),
            case.client_response()
        );
    }

    #[test]
    fn the_completion_id_borrows_the_upstream_response_id() {
        assert_eq!(completion_id("resp_0232a97d"), "chatcmpl-0232a97d");
        assert_eq!(completion_id("chatcmpl-kept"), "chatcmpl-kept");
        assert_eq!(completion_id("bare"), "chatcmpl-bare");
    }

    #[test]
    fn tools_ride_as_one_developer_additional_tools_item_ahead_of_the_conversation() {
        let request: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-6-astra",
            "messages": [
                {"role": "system", "content": "Be terse."},
                {"role": "user", "content": "Read /etc/hosts using the tool."},
            ],
            "tools": [{"type": "function", "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {"type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]},
            }}],
        }))
        .unwrap();
        let built = serde_json::to_value(to_upstream(&request, &config(None)).unwrap()).unwrap();
        // The same three items, in the same order, as the captured Messages
        // request that used this tool.
        assert_eq!(
            built["input"],
            json!([
                {"type": "additional_tools", "role": "developer", "tools": [{
                    "type": "function", "name": "read_file",
                    "description": "Read a file", "strict": false,
                    "parameters": {"type": "object",
                        "properties": {"path": {"type": "string"}},
                        "required": ["path"]},
                }]},
                {"type": "message", "role": "developer",
                 "content": [{"type": "input_text", "text": "Be terse."}]},
                {"type": "message", "role": "user",
                 "content": [{"type": "input_text", "text": "Read /etc/hosts using the tool."}]},
            ])
        );
        assert!(built.get("tools").is_none(), "tools must not go top level");
    }

    #[test]
    fn a_prior_tool_round_trip_replays_as_a_function_call_and_its_output() {
        let request: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-6-astra",
            "messages": [
                {"role": "user", "content": "read it"},
                {"role": "assistant", "content": "Looking.", "tool_calls": [{
                    "id": "call_abc", "type": "function",
                    "function": {"name": "read_file",
                                 "arguments": "{\"path\":\"/etc/hosts\"}"},
                }]},
                {"role": "tool", "tool_call_id": "call_abc", "content": "127.0.0.1 localhost"},
            ],
            "tools": [{"type": "function", "function": {"name": "read_file"}}],
        }))
        .unwrap();
        let built = serde_json::to_value(to_upstream(&request, &config(None)).unwrap()).unwrap();
        let input = built["input"].as_array().unwrap();
        // The assistant turn is two items: what it said, then what it called.
        assert_eq!(input[2]["type"], "message");
        assert_eq!(input[2]["role"], "assistant");
        assert_eq!(input[2]["content"][0]["type"], "output_text");
        assert_eq!(
            input[3],
            json!({"type": "function_call", "call_id": "call_abc", "name": "read_file",
                   "arguments": "{\"path\":\"/etc/hosts\"}"})
        );
        assert_eq!(
            input[4],
            json!({"type": "function_call_output", "call_id": "call_abc",
                   "output": "127.0.0.1 localhost"})
        );
        // A tool with no schema still gets one; the server reads the field
        // whether or not the client sent it.
        assert_eq!(
            input[0]["tools"][0]["parameters"],
            json!({"type": "object", "properties": {}})
        );
    }

    #[test]
    fn an_assistant_turn_that_only_called_a_tool_contributes_no_message() {
        let request: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-6-astra",
            "messages": [
                {"role": "user", "content": "go"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_abc", "type": "function",
                    "function": {"name": "read_file", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "call_abc", "content": ""},
            ],
        }))
        .unwrap();
        let built = serde_json::to_value(to_upstream(&request, &config(None)).unwrap()).unwrap();
        let kinds: Vec<&str> = built["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["message", "function_call", "function_call_output"]);
    }

    #[test]
    fn a_tool_result_without_its_call_id_is_refused_by_name() {
        let request: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-6-astra",
            "messages": [{"role": "tool", "content": "orphan"}],
        }))
        .unwrap();
        let error = to_upstream(&request, &config(None)).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(
            error.message.contains("messages[0].tool_call_id"),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_requests_effort_outranks_the_pinned_one_and_ultra_clamps_to_max() {
        let body = json!({"model": "gpt-6-astra",
                          "messages": [{"role": "user", "content": "hi"}]});
        let pinned: ChatRequest = serde_json::from_value(body.clone()).unwrap();
        let built = serde_json::to_value(
            to_upstream(&pinned, &config(Some(ReasoningEffort::High))).unwrap(),
        )
        .unwrap();
        assert_eq!(built["reasoning"]["effort"], "high");

        let mut asked = body.clone();
        asked["reasoning_effort"] = json!("low");
        let asked: ChatRequest = serde_json::from_value(asked).unwrap();
        let built = serde_json::to_value(
            to_upstream(&asked, &config(Some(ReasoningEffort::High))).unwrap(),
        )
        .unwrap();
        assert_eq!(built["reasoning"]["effort"], "low");

        // Ultra has no upstream spelling; clamping keeps the turn alive at the
        // highest tier this endpoint knows.
        let built = serde_json::to_value(
            to_upstream(&pinned, &config(Some(ReasoningEffort::Ultra))).unwrap(),
        )
        .unwrap();
        assert_eq!(built["reasoning"]["effort"], "max");

        // `none` drops the effort and keeps the continuity.
        let mut none = body;
        none["reasoning_effort"] = json!("none");
        let none: ChatRequest = serde_json::from_value(none).unwrap();
        let built = serde_json::to_value(to_upstream(&none, &config(None)).unwrap()).unwrap();
        assert!(built["reasoning"].get("effort").is_none());
        assert_eq!(built["reasoning"]["context"], "all_turns");
    }

    #[test]
    fn a_json_schema_response_format_becomes_the_texts_format() {
        let request: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-6-astra",
            "messages": [{"role": "user", "content": "hi"}],
            "response_format": {"type": "json_schema", "json_schema": {
                "name": "answer", "strict": true, "schema": {"type": "object"}}},
        }))
        .unwrap();
        let built = serde_json::to_value(to_upstream(&request, &config(None)).unwrap()).unwrap();
        assert_eq!(
            built["text"],
            json!({"verbosity": "low", "format": {"type": "json_schema", "name": "answer",
                                                  "schema": {"type": "object"}, "strict": true}})
        );
    }

    #[test]
    fn a_tool_choice_only_travels_alongside_the_tools_it_names() {
        let body = json!({
            "model": "gpt-6-astra",
            "messages": [{"role": "user", "content": "hi"}],
            "tool_choice": {"type": "function", "function": {"name": "read_file"}},
        });
        let toolless: ChatRequest = serde_json::from_value(body.clone()).unwrap();
        let built = serde_json::to_value(to_upstream(&toolless, &config(None)).unwrap()).unwrap();
        assert!(built.get("tool_choice").is_none());

        let mut armed = body;
        armed["tools"] = json!([{"type": "function", "function": {"name": "read_file"}}]);
        let armed: ChatRequest = serde_json::from_value(armed).unwrap();
        let built = serde_json::to_value(to_upstream(&armed, &config(None)).unwrap()).unwrap();
        // Responses spells a named choice without the `function` wrapper.
        assert_eq!(
            built["tool_choice"],
            json!({"type": "function", "name": "read_file"})
        );
    }

    #[tokio::test]
    async fn a_tool_call_turn_answers_with_the_call_id_the_client_replies_to() {
        let sse = sse(&tool_events());
        let completion = aggregate(Turn::replay(&[&sse]), "gpt-6-astra")
            .await
            .unwrap();
        let answer = serde_json::to_value(&completion).unwrap();
        assert_eq!(answer["choices"][0]["finish_reason"], "tool_calls");
        // `call_abc`, not `fc_1`: the item id keys the deltas, the call id
        // keys the client's reply, and sending the wrong one produces a turn
        // upstream silently ignores.
        assert_eq!(
            answer["choices"][0]["message"]["tool_calls"],
            json!([{"id": "call_abc", "type": "function",
                    "function": {"name": "read_file",
                                 "arguments": "{\"path\":\"/etc/hosts\"}"}}])
        );
        assert!(
            answer["choices"][0]["message"].get("content").is_none(),
            "a tool-only turn must not claim it also said something"
        );
        assert_eq!(answer["usage"]["total_tokens"], 50);
    }

    #[tokio::test]
    async fn a_streamed_tool_call_opens_once_and_appends_arguments_by_index() {
        let sse = sse(&tool_events());
        let response = stream_response(Turn::replay(&[&sse]), "gpt-6-astra".to_owned(), false);
        let chunks = streamed(response).await;
        let deltas: Vec<&Value> = chunks
            .iter()
            .map(|chunk| &chunk["choices"][0]["delta"])
            .collect();
        assert_eq!(deltas[0], &json!({"role": "assistant"}));
        assert_eq!(
            deltas[1],
            &json!({"tool_calls": [{"index": 0, "id": "call_abc", "type": "function",
                                    "function": {"name": "read_file", "arguments": ""}}]})
        );
        // Argument deltas carry neither id nor name — only the index the
        // client reassembles on.
        assert_eq!(
            deltas[2],
            &json!({"tool_calls": [{"index": 0,
                                    "function": {"arguments": "{\"path\":"}}]})
        );
        assert_eq!(
            deltas[3],
            &json!({"tool_calls": [{"index": 0,
                                    "function": {"arguments": "\"/etc/hosts\"}"}}]})
        );
        // `.done` restates the whole string; nothing of it is resent.
        assert_eq!(deltas.len(), 5);
        assert_eq!(deltas[4], &json!({}));
        assert_eq!(
            chunks.last().unwrap()["choices"][0]["finish_reason"],
            "tool_calls"
        );
        let arguments: String = chunks
            .iter()
            .filter_map(|chunk| {
                chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"].as_str()
            })
            .collect();
        assert_eq!(arguments, "{\"path\":\"/etc/hosts\"}");
    }

    #[tokio::test]
    async fn a_streamed_text_turn_replays_the_deltas_and_names_its_model() {
        let case = Case::load("chat-completions-nonstreaming");
        let sse = case.upstream_sse();
        let response = stream_response(Turn::replay(&[&sse]), "gpt-5.6-terra".to_owned(), false);
        let chunks = streamed(response).await;
        let text: String = chunks
            .iter()
            .filter_map(|chunk| chunk["choices"][0]["delta"]["content"].as_str())
            .collect();
        assert_eq!(text, "chat-ok");
        assert!(chunks.iter().all(|chunk| {
            chunk["object"] == "chat.completion.chunk"
                && chunk["id"] == "chatcmpl-0232a97d66c825fd016aa07990c10087d2a3b4b254084ba176"
                && chunk["model"] == "gpt-5.6-terra"
                && chunk["created"] == 1_788_901_776_u64
        }));
        assert_eq!(
            chunks.last().unwrap()["choices"][0]["finish_reason"],
            "stop"
        );
        assert!(
            chunks.iter().all(|chunk| chunk.get("usage").is_none()),
            "usage travels only when stream_options asked for it"
        );
    }

    #[tokio::test]
    async fn include_usage_puts_the_counts_on_the_final_chunk_only() {
        let case = Case::load("chat-completions-nonstreaming");
        let sse = case.upstream_sse();
        let response = stream_response(Turn::replay(&[&sse]), "gpt-5.6-terra".to_owned(), true);
        let chunks = streamed(response).await;
        let (last, rest) = chunks.split_last().unwrap();
        assert!(rest.iter().all(|chunk| chunk.get("usage").is_none()));
        assert_eq!(
            last["usage"],
            json!({"prompt_tokens": 12, "completion_tokens": 6, "total_tokens": 18})
        );
    }

    #[tokio::test]
    async fn a_frame_split_across_chunks_is_still_one_event() {
        let case = Case::load("chat-completions-nonstreaming");
        let body = case.upstream_sse();
        // Cut mid-frame, twice, the way a socket does.
        let (head, tail) = body.split_at(body.len() / 3);
        let (middle, rest) = tail.split_at(tail.len() / 2);
        let completion = aggregate(Turn::replay(&[head, middle, rest]), "gpt-5.6-terra")
            .await
            .unwrap();
        assert_eq!(
            completion.choices[0].message.content.as_deref(),
            Some("chat-ok")
        );
    }

    #[tokio::test]
    async fn a_refused_turn_carries_the_upstream_reason_rather_than_a_paraphrase() {
        let events = vec![
            json!({"type": "response.created", "response": {"id": "resp_x"}}),
            json!({"type": "response.failed", "response": {"id": "resp_x", "status": "failed",
                   "error": {"message": "model gpt-9-nowhere does not exist"}}}),
        ];
        let sse = sse(&events);
        let error = aggregate(Turn::replay(&[&sse]), "gpt-9-nowhere")
            .await
            .unwrap_err();
        assert_eq!(error.message, "model gpt-9-nowhere does not exist");

        // The same failure mid-stream cannot change a status already sent, so
        // it arrives in the body the client is parsing.
        let response = stream_response(Turn::replay(&[&sse]), "gpt-9-nowhere".to_owned(), false);
        let chunks = streamed(response).await;
        assert_eq!(
            chunks[0]["error"]["message"],
            "model gpt-9-nowhere does not exist"
        );
    }

    #[tokio::test]
    async fn an_exhausted_plan_is_a_rate_limit_and_a_rescued_one_is_not() {
        let exhausted = json!({"type": "codex.rate_limits",
            "rate_limits": {"limit_reached": true},
            "credits": {"has_credits": false, "unlimited": false}});
        let spent = sse(std::slice::from_ref(&exhausted));
        let error = aggregate(Turn::replay(&[&spent]), "gpt-6-astra")
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(error.message.contains("out of quota"), "{}", error.message);

        let mut rescued = exhausted;
        rescued["credits"] = json!({"has_credits": true});
        let mut events = vec![rescued];
        events.extend(tool_events());
        let rescued = sse(&events);
        assert!(
            aggregate(Turn::replay(&[&rescued]), "gpt-6-astra")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_stream_that_stops_before_completing_is_not_reported_as_a_turn() {
        let truncated = vec![
            json!({"type": "response.created", "response": {"id": "resp_x"}}),
            json!({"type": "response.output_text.delta", "output_index": 0,
                   "content_index": 0, "delta": "half"}),
        ];
        let sse = sse(&truncated);
        let error = aggregate(Turn::replay(&[&sse]), "gpt-6-astra")
            .await
            .unwrap_err();
        assert!(
            error.message.contains("before the turn completed"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn an_incomplete_turn_finishes_with_length() {
        let events = vec![
            json!({"type": "response.created", "response": {"id": "resp_x"}}),
            json!({"type": "response.output_text.delta", "output_index": 0,
                   "content_index": 0, "delta": "half"}),
            json!({"type": "response.incomplete",
                   "response": {"id": "resp_x", "status": "incomplete"}}),
        ];
        let sse = sse(&events);
        let completion = aggregate(Turn::replay(&[&sse]), "gpt-6-astra")
            .await
            .unwrap();
        assert_eq!(
            completion.choices[0].finish_reason.as_deref(),
            Some("length")
        );
    }

    #[tokio::test]
    async fn a_stream_that_only_announced_its_text_once_still_carries_it() {
        let events = vec![
            json!({"type": "response.created", "response": {"id": "resp_x"}}),
            json!({"type": "response.output_text.done", "output_index": 0,
                   "content_index": 0, "text": "no deltas"}),
            json!({"type": "response.completed",
                   "response": {"id": "resp_x", "status": "completed"}}),
        ];
        let sse = sse(&events);
        let completion = aggregate(Turn::replay(&[&sse]), "gpt-6-astra")
            .await
            .unwrap();
        assert_eq!(
            completion.choices[0].message.content.as_deref(),
            Some("no deltas")
        );
    }

    #[tokio::test]
    async fn a_body_this_surface_cannot_read_is_refused_before_a_credential_is_touched() {
        // The auth file does not exist, so anything that reaches the token
        // path fails as 401 instead. A 400 is the proof it never got there.
        let state = Arc::new(BridgeState::new(config(None)).unwrap());
        for body in [
            &b"not json at all"[..],
            &b"{\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"[..],
            &b"{\"model\":\"gpt-6-astra\",\"messages\":[]}"[..],
        ] {
            let response = handle(State(state.clone()), Bytes::from_static(body)).await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[tokio::test]
    async fn an_unmodelled_event_type_does_not_fail_a_turn() {
        let events = vec![
            json!({"type": "response.created", "response": {"id": "resp_x"}}),
            json!({"type": "response.something.nobody.has.seen", "payload": {"a": 1}}),
            json!({"type": "response.output_text.delta", "output_index": 0,
                   "content_index": 0, "delta": "fine"}),
            json!({"type": "response.completed",
                   "response": {"id": "resp_x", "status": "completed"}}),
        ];
        let sse = sse(&events);
        let completion = aggregate(Turn::replay(&[&sse]), "gpt-6-astra")
            .await
            .unwrap();
        assert_eq!(
            completion.choices[0].message.content.as_deref(),
            Some("fine")
        );
    }
}
