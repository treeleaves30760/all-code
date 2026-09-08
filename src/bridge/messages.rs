//! `POST /v1/messages` and `POST /v1/messages/count_tokens` — Claude Code.
//!
//! The only surface where the model is not pinned: Claude Code switches model
//! and reasoning effort per request, so both come out of the body and
//! [`BridgeConfig::effort`](super::BridgeConfig::effort) is always `None` here.
//!
//! Everything in this file is measured against the
//! `messages-tool-call-streaming` fixture: an Anthropic request carrying one
//! tool and a system prompt, the upstream request it became, the SSE that came
//! back, and — in `events/*-050-downstream-event.json` — the exact Anthropic
//! stream the working bridge emitted from it. That last set is the golden
//! output; reproducing it is what "correct" means for this file.
//!
//! Two shapes of that capture decide the design of everything below.
//!
//! The upstream stream is not one event per Anthropic frame. Five upstream
//! events produced `message_start`, three `ping`s and one
//! `content_block_start`, because `message_start` is emitted lazily in front of
//! whatever the first client-visible frame turns out to be, and every
//! lifecycle event that carries nothing a client can use becomes a `ping` so a
//! silent model does not look like a dead socket. So [`Turn`] carries the
//! state and yields a list per event, rather than a pure function doing it.
//!
//! And the two token figures in that stream disagree on purpose: 73 on
//! `message_start`, 138 on `message_delta`. The first is this file's own
//! estimate, made before a single upstream byte has arrived, and the second is
//! what the model actually charged. Claude Code reads the first to draw a
//! context meter and the second to decide when to compact.

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::fmt::Write as _;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use axum::response::{IntoResponse, Response};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::upstream::{
    self, ContentPart, Effort, FunctionCallOutput, InputItem, OutputItem, Reasoning, TextOptions,
    Tool, UpstreamEvent, UpstreamRequest, Usage,
};
use super::{BridgeError, BridgeState, sse_frame};

/// The role Codex reserves for everything that is not the conversation: the
/// tool declarations and the system prompt both ride under it.
const DEVELOPER: &str = "developer";

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// An Anthropic Messages request.
///
/// `model` is required here even though the vendored bridge tolerated its
/// absence: with no pinned model there is nothing to fall back to, and a
/// silent default is how `gpt-6-astra` requests ended up somewhere else.
///
/// What is missing is deliberate. `max_tokens`, `temperature`, `top_p`,
/// `stop_sequences` and `metadata` reach no field: chatgpt.com refuses every
/// one of them outright — `{"detail":"Unsupported parameter: temperature"}`,
/// measured 2026-09-09 — so there is nothing here to carry them into.
/// `stop_sequences` is the one that costs something real: a client relying on
/// it gets a turn that runs past its stop string, and the alternative was
/// refusing that client's every request.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MessagesRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<Message>,
    /// A bare string or a list of blocks; Claude Code sends both spellings.
    #[serde(default)]
    pub system: Option<SystemPrompt>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    /// `{"type":"enabled","budget_tokens":N}`. Maps to upstream reasoning
    /// effort, which is a ladder rather than a budget, so the mapping is
    /// lossy by construction.
    #[serde(default)]
    pub thinking: Option<Value>,
    /// `{"effort": "max"}` — how Claude Code actually spells reasoning effort,
    /// and the only thing `alc claude --effort` ends up as on the wire.
    /// Captured from a live session on 2026-09-09, alongside a `thinking` of
    /// `{"type":"adaptive","display":"omitted"}` that carries no budget at
    /// all: without this field the flag is silently ignored.
    #[serde(default)]
    pub output_config: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum SystemPrompt {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl SystemPrompt {
    /// The prompt as the single `input_text` part Codex expects.
    ///
    /// Claude Code splits its system prompt into blocks so it can attach cache
    /// controls to each; upstream has no per-block cache control and no second
    /// developer message, so the blocks are rejoined. A blank line between
    /// them, because they were written as separate paragraphs and running them
    /// together changes what the model reads.
    fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Message {
    /// `user` or `assistant`.
    pub role: String,
    pub content: MessageContent,
}

/// Anthropic allows a bare string wherever a block list is allowed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Both spellings as blocks, so the translation has one shape to walk.
    fn blocks(&self) -> Vec<ContentBlock> {
        match self {
            Self::Text(text) => vec![ContentBlock::Text { text: text.clone() }],
            Self::Blocks(blocks) => blocks.clone(),
        }
    }
}

/// A content block, in either direction.
///
/// `ToolResult` is the one that constrains the translation: its `tool_use_id`
/// is the `call_id` upstream handed out, not the `id` its own output item
/// carried, and confusing the two produces a turn upstream silently ignores.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: Value },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Option<Value>,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    /// A block this bridge has not been taught.
    ///
    /// Anthropic adds block types, and an agent that starts sending one must
    /// not have its whole turn refused by the thing in the middle. Dropped on
    /// the way upstream exactly as `Thinking` is - losing one block is a
    /// smaller failure than losing the conversation.
    #[serde(other)]
    Unsupported,
}

/// A tool, as Claude Code declares it.
///
/// Note `input_schema` here against `parameters` upstream: the rename is the
/// entire difference, and it is why tools cannot simply be forwarded.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ToolDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Absent on some tool declarations, and a missing schema is not a
    /// reason to refuse the turn: `tool()` already substitutes an empty
    /// object schema for anything that is not one.
    #[serde(default)]
    pub input_schema: Value,
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// A non-streaming answer.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MessagesResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub role: &'static str,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: MessagesUsage,
}

/// `input_tokens`/`output_tokens` are the upstream's own figures once the
/// terminal event lands; the cache counters are always zero because Codex
/// reports caching per item, in a shape Anthropic has no field for.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct MessagesUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
}

impl From<Usage> for MessagesUsage {
    fn from(usage: Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: Some(0),
            cache_read_input_tokens: Some(0),
        }
    }
}

impl MessagesUsage {
    /// What `message_start` carries: a guess at the input, nothing spent yet,
    /// and no cache counters at all — the capture omits them there, because
    /// claiming zero cache reads before the turn has run would be a claim.
    fn estimate(input_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }
}

/// The answer to `/v1/messages/count_tokens`.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct CountTokensResponse {
    pub input_tokens: u64,
}

/// One frame of the Anthropic event stream.
///
/// The variant set and field spellings are transcribed from the captured
/// downstream events, including the parts that look redundant: `ping` frames
/// carry no data and are still sent, and `message_start` carries a `usage`
/// whose `input_tokens` is a local estimate because the real figure has not
/// arrived yet (73 estimated, 138 actual, in the captured turn). `message_delta`
/// re-states the corrected numbers, which is where a client that cares reads
/// them.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub(crate) enum MessagesEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessagesResponse },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: ContentDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDelta,
        usage: MessagesUsage,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: Value },
}

impl MessagesEvent {
    /// The `event:` name this frame is sent under, which Anthropic requires to
    /// match the payload's own `type`.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::MessageStart { .. } => "message_start",
            Self::ContentBlockStart { .. } => "content_block_start",
            Self::ContentBlockDelta { .. } => "content_block_delta",
            Self::ContentBlockStop { .. } => "content_block_stop",
            Self::MessageDelta { .. } => "message_delta",
            Self::MessageStop => "message_stop",
            Self::Ping => "ping",
            Self::Error { .. } => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub(crate) enum ContentDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    /// Tool arguments arrive as a string of partial JSON, never as an object.
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MessageDelta {
    /// `end_turn`, `tool_use`, `max_tokens`, or `stop_sequence`.
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Turns an Anthropic request into the upstream body.
///
/// `input` is built in the order the capture fixes and the server depends on:
/// the tools as one `additional_tools` developer item, the system prompt as a
/// developer message, then the conversation. User text becomes `input_text`,
/// assistant text `output_text`, `tool_use` becomes a `function_call` whose
/// `arguments` is JSON *as a string*, and `tool_result` becomes a
/// `function_call_output` keyed by `tool_use_id`.
///
/// Reasoning effort comes from the request and from nowhere else — first
/// `output_config.effort`, then a `thinking` budget. `BridgeConfig` is not a
/// parameter for exactly that reason: its `effort` is `None` on this surface
/// by contract, and taking it here would invite a later edit to start reading
/// it.
///
/// The body produced is framed for the WebSocket lane — `type:
/// "response.create"`, no `stream`, the long `client_metadata` key — because
/// that is the lane the capture was taken on and matching it exactly is what
/// makes the golden test worth having. [`UpstreamRequest::over_http`], which
/// the send path applies, re-frames it for the transport this bridge actually
/// speaks.
pub(crate) fn to_upstream(request: &MessagesRequest) -> Result<UpstreamRequest, BridgeError> {
    if request.model.trim().is_empty() {
        return Err(BridgeError::invalid(
            "`model` is required: this bridge has no pinned model to fall back to",
        ));
    }
    if request.messages.is_empty() {
        return Err(BridgeError::invalid(
            "`messages` must not be empty: there is no turn to take",
        ));
    }

    let mut input = Vec::new();
    if !request.tools.is_empty() {
        input.push(InputItem::AdditionalTools {
            role: DEVELOPER.to_owned(),
            tools: request.tools.iter().map(tool).collect(),
        });
    }
    if let Some(system) = request
        .system
        .as_ref()
        .map(SystemPrompt::text)
        .filter(|text| !text.is_empty())
    {
        input.push(InputItem::Message {
            role: DEVELOPER.to_owned(),
            content: vec![ContentPart::InputText { text: system }],
        });
    }
    for message in &request.messages {
        append_message(message, &mut input);
    }

    Ok(UpstreamRequest {
        model: request.model.clone(),
        kind: Some("response.create".to_owned()),
        store: false,
        stream: None,
        parallel_tool_calls: false,
        reasoning: Some(Reasoning {
            effort: effort(request.output_config.as_ref(), request.thinking.as_ref()),
            context: Some("all_turns".to_owned()),
            summary: None,
        }),
        text: TextOptions {
            verbosity: Some("low".to_owned()),
            format: None,
        },
        client_metadata: Some(BTreeMap::from([(
            upstream::LITE_METADATA_WS_KEY.to_owned(),
            "true".to_owned(),
        )])),
        instructions: None,
        include: None,
        tool_choice: request.tool_choice.as_ref().and_then(tool_choice),
        prompt_cache_key: None,
        input,
    })
}

/// One Anthropic message, appended as the one-or-more upstream items it is.
///
/// A single Anthropic message can hold text and a tool call, or text and two
/// tool results; upstream those are separate top-level items and their order
/// relative to the text is meaningful. Hence the flush: text accumulates until
/// something that is not text forces it out, keeping the sequence the client
/// wrote.
fn append_message(message: &Message, input: &mut Vec<InputItem>) {
    let assistant = message.role == "assistant";
    let mut parts: Vec<ContentPart> = Vec::new();
    let role = if assistant { "assistant" } else { "user" };

    for block in message.content.blocks() {
        match block {
            ContentBlock::Text { text } => parts.push(if assistant {
                ContentPart::OutputText { text }
            } else {
                ContentPart::InputText { text }
            }),
            ContentBlock::Image { source } => {
                if let Some(image_url) = image_url(&source) {
                    parts.push(ContentPart::InputImage {
                        image_url,
                        detail: None,
                    });
                }
            }
            ContentBlock::ToolUse {
                id,
                name,
                input: arguments,
            } => {
                flush(role, &mut parts, input);
                input.push(InputItem::FunctionCall {
                    // Upstream's own `id` for the call is not ours to invent,
                    // and it is optional; `call_id` is the handle both sides
                    // agreed on and the only one the server matches against.
                    id: None,
                    call_id: id,
                    name,
                    arguments: serde_json::to_string(&arguments)
                        .unwrap_or_else(|_| "{}".to_owned()),
                });
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                flush(role, &mut parts, input);
                input.push(InputItem::FunctionCallOutput {
                    call_id: tool_use_id,
                    output: tool_result(content.as_ref(), is_error.unwrap_or(false)),
                });
            }
            // Thinking has no upstream container on this path: Codex carries
            // its own reasoning across turns through `reasoning.context`, and
            // replaying an Anthropic thinking block as text would present the
            // model's own scratchpad back to it as if a user had written it.
            // Dropped rather than relayed: upstream has no place for them, and
            // an unknown block is one this bridge has not been taught yet.
            ContentBlock::Thinking { .. }
            | ContentBlock::RedactedThinking { .. }
            | ContentBlock::Unsupported => {}
        }
    }
    flush(role, &mut parts, input);
}

fn flush(role: &str, parts: &mut Vec<ContentPart>, input: &mut Vec<InputItem>) {
    if parts.is_empty() {
        return;
    }
    input.push(InputItem::Message {
        role: role.to_owned(),
        content: std::mem::take(parts),
    });
}

fn tool(definition: &ToolDefinition) -> Tool {
    Tool {
        kind: "function".to_owned(),
        name: definition.name.clone(),
        description: definition.description.clone(),
        // Never strict: the server would then validate arguments against this
        // schema, and agent-supplied schemas are not reliably strict-legal.
        strict: false,
        parameters: if definition.input_schema.is_object() {
            definition.input_schema.clone()
        } else {
            // The server requires an object schema. A tool declared without
            // one still works — it just takes no arguments — so substituting
            // is kinder than refusing the whole turn over one tool.
            json!({"type": "object", "properties": {}})
        },
    }
}

/// Anthropic's tool choice in the Responses API's spelling.
///
/// Returns `None` for anything unrecognised, which leaves the field off and
/// lets upstream apply its own default rather than refusing a request over a
/// preference.
fn tool_choice(choice: &Value) -> Option<Value> {
    match choice.get("type").and_then(Value::as_str)? {
        "auto" => Some(json!("auto")),
        "any" | "required" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => choice
            .get("name")
            .and_then(Value::as_str)
            .map(|name| json!({"type": "function", "name": name})),
        _ => None,
    }
}

/// The reasoning rung a request asks for.
///
/// `output_config.effort` is checked first because it is the one Claude Code
/// sends and the one `alc claude --effort` becomes: a live session on
/// 2026-09-09 carried `{"effort":"max"}` there while its `thinking` was
/// `{"type":"adaptive","display":"omitted"}`, which names no budget. Reading
/// only the budget would drop the user's choice on the floor and run every
/// turn at whatever upstream defaults to.
///
/// `ultra` clamps to `max` for the same reason [`Effort`] has no variant for
/// it: alc's ladder has a rung chatgpt.com does not, and a request naming it
/// should be served at the top of the real ladder rather than refused.
///
/// A `thinking` budget is the fallback, for the Anthropic-shaped clients that
/// send one. The two ladders do not correspond — one is a token budget, the
/// other is five words — so the boundaries are placed where Claude Code's own
/// tiers fall: 4000 ("think") lands on `low`, 10000 ("think hard") on
/// `medium`, 31999 ("ultrathink") on `high`, and the wider budgets an SDK
/// caller can ask for climb from there.
///
/// Neither present sends no effort at all. Anthropic's default is no
/// *extended* thinking, which is not the same claim as "do not reason", and
/// `none` upstream is the second one — a downgrade from the medium the capture
/// shows upstream choosing for itself.
fn effort(output_config: Option<&Value>, thinking: Option<&Value>) -> Option<Effort> {
    if let Some(named) = output_config
        .and_then(|config| config.get("effort"))
        .and_then(Value::as_str)
    {
        return match named.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Effort::None),
            "low" => Some(Effort::Low),
            "medium" => Some(Effort::Medium),
            "high" => Some(Effort::High),
            "xhigh" => Some(Effort::Xhigh),
            "max" | "ultra" => Some(Effort::Max),
            // An unknown rung is not worth refusing a turn over; upstream's
            // own default is a better answer than a 400.
            _ => None,
        };
    }
    let thinking = thinking?;
    if thinking.get("type").and_then(Value::as_str) != Some("enabled") {
        return None;
    }
    let budget = thinking.get("budget_tokens").and_then(Value::as_u64)?;
    Some(if budget <= 4_096 {
        Effort::Low
    } else if budget <= 16_384 {
        Effort::Medium
    } else if budget <= 32_768 {
        Effort::High
    } else if budget <= 65_536 {
        Effort::Xhigh
    } else {
        Effort::Max
    })
}

/// A `tool_result` block's content, as the upstream output.
///
/// Text goes across as a bare string, which is what the capture and the Codex
/// CLI both send. The parts form is used only when an image is in there, since
/// a data URL flattened into text is bytes the model cannot see.
fn tool_result(content: Option<&Value>, is_error: bool) -> FunctionCallOutput {
    let mut text: Vec<String> = Vec::new();
    let mut images: Vec<String> = Vec::new();

    if is_error {
        // The client knew this failed and upstream has no `is_error` field, so
        // saying so in the output is the only way the model finds out.
        text.push("[tool execution error]".to_owned());
    }
    match content {
        None => {}
        Some(Value::String(raw)) => text.push(raw.clone()),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => text.push(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    ),
                    Some("image") => match block.get("source").and_then(image_url) {
                        Some(url) => images.push(url),
                        None => text.push("[unreadable image omitted]".to_owned()),
                    },
                    Some(other) => text.push(format!("[unsupported block omitted: {other}]")),
                    None => text.push(block.to_string()),
                }
            }
        }
        Some(other) => text.push(other.to_string()),
    }

    if images.is_empty() {
        return FunctionCallOutput::Text(text.join("\n"));
    }
    let mut parts = vec![ContentPart::InputText {
        text: text.join("\n"),
    }];
    parts.extend(images.into_iter().map(|image_url| ContentPart::InputImage {
        image_url,
        detail: None,
    }));
    FunctionCallOutput::Parts(parts)
}

/// An Anthropic image source as the URL upstream takes.
///
/// Base64 payloads are re-wrapped as a data URL rather than decoded: the bytes
/// are the client's, this bridge has no reason to look at them, and decoding
/// only to re-encode would double the memory a screenshot costs.
fn image_url(source: &Value) -> Option<String> {
    match source.get("type").and_then(Value::as_str)? {
        "url" => source.get("url").and_then(Value::as_str).map(str::to_owned),
        "base64" => {
            let media_type = source.get("media_type").and_then(Value::as_str)?;
            let data = source.get("data").and_then(Value::as_str)?;
            let compact: String = data.split_whitespace().collect();
            (!compact.is_empty()).then(|| format!("data:{media_type};base64,{compact}"))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Upstream stream -> Anthropic stream
// ---------------------------------------------------------------------------

/// One block Anthropic has open, keyed by the upstream `output_index` that
/// feeds it.
///
/// Two indices exist and they are not the same number: upstream's
/// `output_index` counts its own items including the reasoning ones a client
/// never sees, and Anthropic's `index` counts only the blocks that were
/// emitted. Keying the map on the first and storing the second is what keeps
/// them from being confused.
enum OpenBlock {
    Text {
        index: u32,
    },
    Tool {
        index: u32,
        arguments: String,
        emitted: bool,
    },
}

impl OpenBlock {
    fn index(&self) -> u32 {
        match self {
            Self::Text { index } | Self::Tool { index, .. } => *index,
        }
    }
}

/// The state one `/v1/messages` turn accumulates while its upstream stream
/// runs.
///
/// Both response modes drive this: streaming writes each frame out as it comes,
/// non-streaming folds the same frames into one message. Sharing it is what
/// stops the two from disagreeing about, say, whether a tool call ended the
/// turn.
pub(crate) struct Turn {
    message_id: String,
    model: String,
    estimated_input_tokens: u64,
    started: bool,
    finished: bool,
    /// Anthropic's next block index. Only ever counts blocks actually emitted.
    next_index: u32,
    blocks: BTreeMap<u32, OpenBlock>,
    saw_tool_use: bool,
}

impl Turn {
    pub(crate) fn new(message_id: String, model: String, estimated_input_tokens: u64) -> Self {
        Self {
            message_id,
            model,
            estimated_input_tokens,
            started: false,
            finished: false,
            next_index: 0,
            blocks: BTreeMap::new(),
            saw_tool_use: false,
        }
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// Translates one upstream event into the Anthropic frames it produces.
    ///
    /// A list, because the mapping is not one-to-one in either direction: the
    /// first event of a stream also drags `message_start` out in front of
    /// itself, `response.completed` produces two frames, and
    /// `codex.response.metadata` produces none.
    ///
    /// The captured pairing is the specification:
    /// `output_item.added(function_call)` -> `content_block_start(tool_use)`
    /// with `input: {}` and `id` set to the **call_id**;
    /// `function_call_arguments.delta` -> `content_block_delta(input_json_delta)`
    /// carrying the delta verbatim; `output_item.done` -> `content_block_stop`;
    /// `response.completed` -> `message_delta` with `stop_reason: "tool_use"`
    /// and the upstream usage, then `message_stop`.
    pub(crate) fn accept(
        &mut self,
        event: &UpstreamEvent,
    ) -> Result<Vec<MessagesEvent>, BridgeError> {
        let mut out = Vec::new();
        if self.finished {
            return Ok(out);
        }
        match event {
            UpstreamEvent::RateLimits(limits) => {
                if limits.is_terminal() {
                    return Err(BridgeError::new(
                        StatusCode::TOO_MANY_REQUESTS,
                        "rate_limit_error",
                        "the ChatGPT plan's Codex quota is exhausted and the account has no \
                         credits left; the window resets on its own, or `alc` can be pointed \
                         at another provider",
                    ));
                }
                self.ping(&mut out);
            }
            // Nothing a client can render, but a turn that thinks for a minute
            // before its first token still has to look alive.
            UpstreamEvent::Created { .. } | UpstreamEvent::InProgress { .. } => {
                self.ping(&mut out);
            }
            UpstreamEvent::OutputItemAdded { output_index, item } => {
                self.item_added(*output_index, item, &mut out);
            }
            UpstreamEvent::OutputTextDelta {
                output_index,
                delta,
                ..
            } => self.text_delta(*output_index, delta, &mut out),
            UpstreamEvent::FunctionCallArgumentsDelta {
                output_index,
                delta,
                ..
            } => self.tool_delta(*output_index, delta, &mut out),
            // The arguments are already downstream as deltas; this event only
            // matters when they were not, so it seeds and emits nothing.
            UpstreamEvent::FunctionCallArgumentsDone {
                output_index,
                arguments,
                ..
            } => self.tool_arguments(*output_index, arguments),
            UpstreamEvent::OutputItemDone { output_index, item } => {
                self.item_done(*output_index, item, &mut out);
            }
            UpstreamEvent::Completed { response } => {
                self.finish(response.usage, false, &mut out);
            }
            UpstreamEvent::Incomplete { response } => {
                self.finish(response.usage, true, &mut out);
            }
            UpstreamEvent::Failed { response } => {
                return Err(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "chatgpt.com failed the turn: {}",
                        detail(response.error.as_ref())
                    ),
                ));
            }
            UpstreamEvent::Error { error } => {
                return Err(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    format!("chatgpt.com returned an error: {}", detail(Some(error))),
                ));
            }
            // Text-part boundaries, telemetry, replayed headers, and whatever
            // upstream adds next. A new event type must never fail a turn.
            _ => {}
        }
        Ok(out)
    }

    /// The frames that close a turn which failed after it had already begun.
    ///
    /// Once `message_start` is on the wire the status code is spent, so the
    /// only honest way left to name the cause is an Anthropic `error` frame.
    pub(crate) fn fail(&mut self, error: &BridgeError) -> Vec<MessagesEvent> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        self.close_blocks(&mut out);
        self.start(&mut out);
        out.push(MessagesEvent::Error {
            error: json!({"type": error.kind, "message": error.message}),
        });
        self.finished = true;
        out
    }

    /// Closes a turn whose stream ended without a terminal event.
    ///
    /// The socket dropping mid-turn is not success, but the content already
    /// delivered is real, so it is ended as a turn rather than discarded.
    pub(crate) fn close(&mut self) -> Vec<MessagesEvent> {
        let mut out = Vec::new();
        if !self.finished {
            self.finish(None, false, &mut out);
        }
        out
    }

    fn start(&mut self, out: &mut Vec<MessagesEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        out.push(MessagesEvent::MessageStart {
            message: MessagesResponse {
                id: self.message_id.clone(),
                kind: "message",
                role: "assistant",
                model: self.model.clone(),
                content: Vec::new(),
                stop_reason: None,
                stop_sequence: None,
                usage: MessagesUsage::estimate(self.estimated_input_tokens),
            },
        });
    }

    fn ping(&mut self, out: &mut Vec<MessagesEvent>) {
        self.start(out);
        out.push(MessagesEvent::Ping);
    }

    fn open_text(&mut self, output_index: u32, out: &mut Vec<MessagesEvent>) -> u32 {
        let index = self.next_index;
        self.next_index += 1;
        self.blocks.insert(output_index, OpenBlock::Text { index });
        self.start(out);
        out.push(MessagesEvent::ContentBlockStart {
            index,
            content_block: ContentBlock::Text {
                text: String::new(),
            },
        });
        index
    }

    fn item_added(&mut self, output_index: u32, item: &OutputItem, out: &mut Vec<MessagesEvent>) {
        match item {
            OutputItem::Message { .. } => {
                self.open_text(output_index, out);
            }
            OutputItem::FunctionCall { call_id, name, .. } => {
                let index = self.next_index;
                self.next_index += 1;
                self.saw_tool_use = true;
                self.blocks.insert(
                    output_index,
                    OpenBlock::Tool {
                        index,
                        arguments: String::new(),
                        emitted: false,
                    },
                );
                self.start(out);
                out.push(MessagesEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlock::ToolUse {
                        // `call_id`, never the item's `id`: this is the string
                        // the client will send back as `tool_use_id`, and the
                        // one upstream matches the result against.
                        id: call_id.clone(),
                        name: name.clone(),
                        input: json!({}),
                    },
                });
            }
            // Reasoning items carry encrypted state for a replay this bridge
            // does not do, and `Other` is whatever upstream added last week.
            OutputItem::Reasoning { .. } | OutputItem::Other => {}
        }
    }

    fn text_delta(&mut self, output_index: u32, delta: &str, out: &mut Vec<MessagesEvent>) {
        if delta.is_empty() {
            return;
        }
        // Upstream has been observed to start streaming text without an
        // `output_item.added` in front of it; opening the block here costs
        // nothing and losing the first sentence is not recoverable.
        let index = match self.blocks.get(&output_index) {
            Some(block) => block.index(),
            None => self.open_text(output_index, out),
        };
        out.push(MessagesEvent::ContentBlockDelta {
            index,
            delta: ContentDelta::Text {
                text: delta.to_owned(),
            },
        });
    }

    fn tool_delta(&mut self, output_index: u32, delta: &str, out: &mut Vec<MessagesEvent>) {
        if delta.is_empty() {
            return;
        }
        let Some(OpenBlock::Tool {
            index,
            arguments,
            emitted,
        }) = self.blocks.get_mut(&output_index)
        else {
            return;
        };
        arguments.push_str(delta);
        *emitted = true;
        out.push(MessagesEvent::ContentBlockDelta {
            index: *index,
            delta: ContentDelta::InputJson {
                partial_json: delta.to_owned(),
            },
        });
    }

    fn tool_arguments(&mut self, output_index: u32, complete: &str) {
        let Some(OpenBlock::Tool { arguments, .. }) = self.blocks.get_mut(&output_index) else {
            return;
        };
        if arguments.is_empty() {
            *arguments = complete.to_owned();
        }
    }

    fn item_done(&mut self, output_index: u32, item: &OutputItem, out: &mut Vec<MessagesEvent>) {
        let Some(block) = self.blocks.remove(&output_index) else {
            return;
        };
        match block {
            OpenBlock::Text { index } => out.push(MessagesEvent::ContentBlockStop { index }),
            OpenBlock::Tool {
                index,
                mut arguments,
                emitted,
            } => {
                // A tool call whose arguments never streamed still has them on
                // the finished item. Without this the client sees a `tool_use`
                // block with `input: {}` and calls the tool with nothing.
                if !emitted {
                    if let OutputItem::FunctionCall {
                        arguments: complete,
                        ..
                    } = item
                        && !complete.is_empty()
                    {
                        arguments = complete.clone();
                    }
                    if !arguments.is_empty() {
                        out.push(MessagesEvent::ContentBlockDelta {
                            index,
                            delta: ContentDelta::InputJson {
                                partial_json: arguments,
                            },
                        });
                    }
                }
                out.push(MessagesEvent::ContentBlockStop { index });
            }
        }
    }

    fn close_blocks(&mut self, out: &mut Vec<MessagesEvent>) {
        // Ascending index order: a client tracking open blocks closes them in
        // the order it opened them.
        let mut open: Vec<u32> = std::mem::take(&mut self.blocks)
            .into_values()
            .map(|block| block.index())
            .collect();
        open.sort_unstable();
        for index in open {
            out.push(MessagesEvent::ContentBlockStop { index });
        }
    }

    fn finish(&mut self, usage: Option<Usage>, incomplete: bool, out: &mut Vec<MessagesEvent>) {
        self.close_blocks(out);
        self.start(out);
        let stop_reason = if incomplete {
            "max_tokens"
        } else if self.saw_tool_use {
            "tool_use"
        } else {
            "end_turn"
        };
        out.push(MessagesEvent::MessageDelta {
            delta: MessageDelta {
                stop_reason: Some(stop_reason.to_owned()),
                stop_sequence: None,
            },
            usage: usage.map(MessagesUsage::from).unwrap_or_default(),
        });
        out.push(MessagesEvent::MessageStop);
        self.finished = true;
    }
}

/// The human-readable half of an upstream error object.
///
/// Acceptance criterion 5 lives here: an unknown model comes back as
/// `{"detail": "..."}`, and relaying that sentence is the difference between a
/// user fixing their model name and a user filing a bug. The digging is
/// [`upstream::error_message`]'s, because a mid-stream `error` frame spells its
/// reason the same four ways a refused POST does and there is no reason for
/// this surface to know a fifth.
fn detail(error: Option<&Value>) -> String {
    error
        .filter(|error| !error.is_null())
        .and_then(|error| serde_json::to_vec(error).ok())
        .and_then(|body| upstream::error_message(&body))
        .unwrap_or_else(|| "no reason given".to_owned())
}

/// Folds a finished turn's frames into the single message a non-streaming
/// client asked for.
///
/// Built from the stream rather than beside it so the two modes cannot drift:
/// whatever a streaming client sees block-by-block is exactly what a
/// non-streaming one gets in one piece.
fn collect(events: Vec<MessagesEvent>) -> Result<MessagesResponse, BridgeError> {
    let mut message: Option<MessagesResponse> = None;
    let mut partial_json: BTreeMap<u32, String> = BTreeMap::new();

    for event in events {
        match event {
            MessagesEvent::MessageStart { message: start } => message = Some(start),
            MessagesEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                if let Some(message) = message.as_mut() {
                    // Indices are handed out in order, so an out-of-order one
                    // means the stream is not what this code believes it is.
                    if message.content.len() as u32 == index {
                        message.content.push(content_block);
                    }
                }
            }
            MessagesEvent::ContentBlockDelta { index, delta } => match delta {
                ContentDelta::Text { text } => {
                    if let Some(ContentBlock::Text { text: existing }) = message
                        .as_mut()
                        .and_then(|message| message.content.get_mut(index as usize))
                    {
                        existing.push_str(&text);
                    }
                }
                ContentDelta::InputJson { partial_json: raw } => {
                    partial_json.entry(index).or_default().push_str(&raw);
                }
            },
            MessagesEvent::ContentBlockStop { index } => {
                if let Some(raw) = partial_json.remove(&index)
                    && let Some(ContentBlock::ToolUse { input, .. }) = message
                        .as_mut()
                        .and_then(|message| message.content.get_mut(index as usize))
                {
                    // A truncated stream leaves unparseable JSON. An empty
                    // object is a tool call the client can refuse cleanly;
                    // a broken string is one it cannot even read.
                    *input = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
                }
            }
            MessagesEvent::MessageDelta { delta, usage } => {
                if let Some(message) = message.as_mut() {
                    message.stop_reason = delta.stop_reason;
                    message.stop_sequence = delta.stop_sequence;
                    message.usage = usage;
                }
            }
            MessagesEvent::Error { error } => {
                return Err(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    detail(Some(&error)),
                ));
            }
            MessagesEvent::MessageStop | MessagesEvent::Ping => {}
        }
    }

    message.ok_or_else(|| {
        BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            "chatgpt.com closed the stream without sending anything",
        )
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/messages`.
///
/// Raw bytes rather than `Json<T>` so that a body this bridge cannot read is
/// refused in Anthropic's own error envelope, naming what was wrong with it,
/// instead of in axum's.
pub(crate) async fn handle_messages(
    State(state): State<Arc<BridgeState>>,
    body: Bytes,
) -> Response {
    match messages(&state, &body).await {
        Ok(response) => response,
        Err(error) => error.anthropic(),
    }
}

async fn messages(state: &BridgeState, body: &Bytes) -> Result<Response, BridgeError> {
    let request = parse(body)?;
    let upstream_request = to_upstream(&request)?;
    let turn = Turn::new(
        message_id(),
        request.model.clone(),
        estimate_input_tokens(&upstream_request),
    );
    let response = upstream::send(state, upstream_request).await?;

    if request.stream {
        return Ok(stream(response, turn));
    }
    Ok(axum::Json(gather(response, turn).await?).into_response())
}

/// `POST /v1/messages/count_tokens`.
///
/// Answers locally and always. Claude Code asks before every turn, so a round
/// trip here would put chatgpt.com's latency in front of each keystroke's
/// worth of typing — and the answer is only used to decide whether to compact,
/// not to bill anyone.
pub(crate) async fn handle_count_tokens(
    State(_state): State<Arc<BridgeState>>,
    body: Bytes,
) -> Response {
    match parse(&body).and_then(|request| to_upstream(&request)) {
        Ok(request) => axum::Json(CountTokensResponse {
            input_tokens: estimate_input_tokens(&request),
        })
        .into_response(),
        Err(error) => error.anthropic(),
    }
}

fn parse(body: &Bytes) -> Result<MessagesRequest, BridgeError> {
    serde_json::from_slice(body).map_err(|error| {
        BridgeError::invalid(format!(
            "this is not an Anthropic Messages request: {error}"
        ))
    })
}

/// An Anthropic message id: `msg_` and sixteen random bytes in hex.
///
/// Nothing keys off the value — it exists so a client can correlate the frames
/// of one stream — so a starved RNG produces a duplicate id rather than a
/// failed turn.
fn message_id() -> String {
    let mut bytes = [0_u8; 16];
    let _ = getrandom::fill(&mut bytes);
    let mut id = String::with_capacity(36);
    id.push_str("msg_");
    for byte in bytes {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

/// Reads a whole upstream stream and answers with one message.
///
/// Stops on the terminal event rather than on end-of-body: chatgpt.com trails
/// telemetry after `response.completed` and keeps a pooled connection open, so
/// waiting for EOF would add the idle timeout to the end of every turn.
async fn gather(
    mut response: upstream::UpstreamStream,
    mut turn: Turn,
) -> Result<MessagesResponse, BridgeError> {
    let mut events = Vec::new();
    while let Some(frame) = response.next().await {
        events.extend(turn.accept(&frame?.event)?);
        if turn.is_finished() {
            break;
        }
    }
    events.extend(turn.close());
    collect(events)
}

/// Relays the upstream stream as Anthropic Server-Sent Events.
fn stream(response: upstream::UpstreamStream, turn: Turn) -> Response {
    let body = Body::from_stream(frames(response, turn));
    let mut out = Response::new(body);
    out.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    // Claude Code talks to this over loopback, but a proxy that buffered the
    // stream would turn a live turn into a long pause and one big burst.
    out.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    out
}

/// The pump: upstream events in, encoded Anthropic frames out.
///
/// A hand-rolled `unfold` rather than a spawned task and a channel, so that a
/// client hanging up stops reading, which stops polling, which drops the
/// upstream connection. A task would keep draining chatgpt.com for a turn
/// nobody is listening to.
fn frames(
    response: upstream::UpstreamStream,
    turn: Turn,
) -> impl Stream<Item = Result<String, Infallible>> + Send + 'static {
    struct Pump {
        response: upstream::UpstreamStream,
        turn: Turn,
        pending: VecDeque<MessagesEvent>,
        done: bool,
    }

    let pump = Pump {
        response,
        turn,
        pending: VecDeque::new(),
        done: false,
    };

    futures_util::stream::unfold(pump, |mut pump| async move {
        loop {
            if let Some(event) = pump.pending.pop_front() {
                let frame = sse_frame(event.name(), &event);
                return Some((Ok(frame), pump));
            }
            if pump.done {
                return None;
            }
            match pump.response.next().await {
                Some(Ok(frame)) => match pump.turn.accept(&frame.event) {
                    Ok(events) => {
                        pump.pending.extend(events);
                        // Stop on the terminal event, not on end-of-body:
                        // upstream trails telemetry and pools the connection,
                        // so reading to EOF would hold the turn open after the
                        // client already has every frame of it.
                        pump.done = pump.turn.is_finished();
                    }
                    Err(error) => {
                        pump.pending.extend(pump.turn.fail(&error));
                        pump.done = true;
                    }
                },
                Some(Err(error)) => {
                    pump.pending.extend(pump.turn.fail(&error));
                    pump.done = true;
                }
                None => {
                    pump.pending.extend(pump.turn.close());
                    pump.done = true;
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Token estimate
// ---------------------------------------------------------------------------

/// What one image costs, as a flat number.
///
/// Anthropic bills an image by its pixel dimensions and Codex by its own
/// tiling, neither of which is knowable from a base64 blob without decoding
/// it. A fixed figure in the right order of magnitude beats decoding a
/// megabyte of PNG to answer a question nobody is billed for.
const IMAGE_TOKENS: u64 = 2_000;

/// Framing charged per input item — role, type and delimiters.
const ITEM_OVERHEAD: u64 = 4;

/// Estimates the input tokens of a translated request.
///
/// Counted over the *translated* body, not the Anthropic one, because that is
/// what upstream will actually charge for: the tool schemas as JSON, the
/// system prompt as a developer message, the tool results as their rendered
/// text.
///
/// No tokeniser. A real BPE table would add a multi-megabyte dependency to
/// answer a question whose two consumers are a progress meter and a
/// compact-now decision; four bytes per token is within a few percent of
/// `o200k` on English prose and code, and it is measured against the capture
/// in this file's tests.
pub(crate) fn estimate_input_tokens(request: &UpstreamRequest) -> u64 {
    let mut total = approximate_tokens(&request.model);
    if let Some(instructions) = &request.instructions {
        total += approximate_tokens(instructions);
    }
    for item in &request.input {
        total += ITEM_OVERHEAD;
        total += match item {
            InputItem::AdditionalTools { tools, .. } => tools
                .iter()
                .map(|tool| approximate_tokens(&serde_json::to_string(tool).unwrap_or_default()))
                .sum(),
            InputItem::Message { content, .. } => content.iter().map(part_tokens).sum(),
            InputItem::FunctionCall {
                name, arguments, ..
            } => approximate_tokens(name) + approximate_tokens(arguments),
            InputItem::FunctionCallOutput { output, .. } => match output {
                FunctionCallOutput::Text(text) => approximate_tokens(text),
                FunctionCallOutput::Parts(parts) => parts.iter().map(part_tokens).sum(),
            },
            // Encrypted reasoning is base64 of a payload the model expands
            // again; four bytes of base64 per three of content, and the
            // envelope is fixed overhead worth subtracting.
            InputItem::Reasoning {
                encrypted_content, ..
            } => (encrypted_content.len() as u64 * 3 / 4).saturating_sub(650) / 4,
        };
    }
    total.max(1)
}

fn part_tokens(part: &ContentPart) -> u64 {
    match part {
        ContentPart::InputText { text } | ContentPart::OutputText { text } => {
            approximate_tokens(text)
        }
        ContentPart::InputImage { .. } => IMAGE_TOKENS,
    }
}

/// Bytes, not characters, divided by four.
///
/// Bytes because it degrades in the right direction outside English: a CJK
/// character is three UTF-8 bytes and close to one token, so bytes/4
/// under-counts it by a quarter where characters/4 would under-count it
/// fourfold.
fn approximate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {

    /// Anthropic adds content-block types. The bridge in the middle must not
    /// turn one it has not been taught into a refused turn.
    #[test]
    fn an_unknown_content_block_is_dropped_rather_than_refusing_the_turn() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "before" },
                { "type": "some_future_block", "whatever": 1 },
                { "type": "text", "text": "after" }
            ]
        }))
        .expect("an unknown block must still parse");

        let blocks = message.content.blocks();
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[1], ContentBlock::Unsupported));
    }

    /// A tool declared without a schema is a tool alc can still pass on.
    #[test]
    fn a_tool_without_a_schema_still_parses() {
        let tool: ToolDefinition = serde_json::from_value(serde_json::json!({
            "name": "no_schema",
            "description": "takes nothing"
        }))
        .expect("a missing input_schema must not refuse the turn");
        assert_eq!(tool.name, "no_schema");
    }
    use super::*;
    use crate::bridge::fixtures::Case;

    /// The captured turn's client request, as a [`MessagesRequest`].
    fn captured_request(case: &str) -> MessagesRequest {
        serde_json::from_value(Case::load(case).client_request()).expect("the capture parses")
    }

    /// Replays a captured upstream stream through a fresh [`Turn`].
    fn replay(case: &str, turn: &mut Turn) -> Vec<MessagesEvent> {
        let sse = Case::load(case).upstream_sse();
        let mut out = Vec::new();
        for frame in upstream::parse_sse(&sse) {
            let event: UpstreamEvent =
                serde_json::from_str(&frame.data).expect("the capture parses");
            out.extend(turn.accept(&event).expect("the captured turn succeeded"));
        }
        out
    }

    #[test]
    fn the_captured_request_translates_into_the_captured_upstream_body() {
        let built = to_upstream(&captured_request("messages-tool-call-streaming")).unwrap();
        assert_eq!(
            serde_json::to_value(&built).unwrap(),
            Case::load("messages-tool-call-streaming").upstream_request()
        );
    }

    #[test]
    fn the_captured_stream_replays_the_captured_anthropic_events() {
        let case = Case::load("messages-tool-call-streaming");
        let expected = case.downstream_events();
        // The message id is random per turn and the `message_start` estimate is
        // this file's own arithmetic, so both are seeded from the capture. Every
        // other byte of every frame has to match on its own.
        let seeded = expected[0]["data"]["message"].clone();
        let mut turn = Turn::new(
            seeded["id"].as_str().unwrap().to_owned(),
            seeded["model"].as_str().unwrap().to_owned(),
            seeded["usage"]["input_tokens"].as_u64().unwrap(),
        );

        let actual: Vec<Value> = replay("messages-tool-call-streaming", &mut turn)
            .iter()
            .map(|event| json!({"event": event.name(), "data": event}))
            .collect();

        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(actual, expected, "frame {index}");
        }
    }

    #[test]
    fn the_same_stream_folds_into_one_non_streaming_message() {
        let mut turn = Turn::new("msg_test".to_owned(), "gpt-5.6-terra".to_owned(), 73);
        let events = replay("messages-tool-call-streaming", &mut turn);
        let message = collect(events).unwrap();

        assert_eq!(message.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(message.usage.input_tokens, 138);
        assert_eq!(message.usage.output_tokens, 21);
        match message.content.as_slice() {
            [ContentBlock::ToolUse { id, name, input }] => {
                assert_eq!(id, "call_piubHZQo0iWT8WOQgeAzt2tW");
                assert_eq!(name, "read_file");
                assert_eq!(input, &json!({"path": "/etc/hosts"}));
            }
            other => panic!("expected one tool_use block, got {other:?}"),
        }
    }

    #[test]
    fn the_local_estimate_lands_near_the_figure_the_working_bridge_sent() {
        let built = to_upstream(&captured_request("messages-tool-call-streaming")).unwrap();
        let estimate = estimate_input_tokens(&built);
        // The capture's bridge tokenised with o200k and got 73. This one counts
        // bytes; the test exists to catch an estimator that has drifted into
        // uselessness, not to pin a number no client reads as exact.
        assert!(
            (60..=90).contains(&estimate),
            "estimated {estimate} against the captured 73"
        );
    }

    #[test]
    fn count_tokens_answers_from_the_body_alone() {
        let built = to_upstream(&captured_request("messages-count-tokens")).unwrap();
        let estimate = estimate_input_tokens(&built);
        assert!(estimate >= 1);
        // "hello" plus the model name plus one item's framing: small, but a
        // count of zero would tell Claude Code the context is empty.
        assert!(estimate < 20, "estimated {estimate} for a five-byte turn");
    }

    #[test]
    fn a_tool_result_turn_replays_the_call_id_upstream() {
        let request: MessagesRequest = serde_json::from_value(json!({
            "model": "gpt-6-astra",
            "messages": [
                {"role": "user", "content": "read it"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "on it"},
                    {"type": "tool_use", "id": "call_abc", "name": "read_file",
                     "input": {"path": "/etc/hosts"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_abc",
                     "content": "127.0.0.1 localhost"}
                ]}
            ]
        }))
        .unwrap();

        let built = to_upstream(&request).unwrap();
        let input = serde_json::to_value(&built).unwrap()["input"].clone();

        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        // The assistant's text is its own item and precedes the call, because
        // upstream reads them in order and "on it" was said first.
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_abc");
        assert_eq!(input[2]["arguments"], "{\"path\":\"/etc/hosts\"}");
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_abc");
        assert_eq!(input[3]["output"], "127.0.0.1 localhost");
    }

    #[test]
    fn a_failed_tool_result_says_so_in_the_only_field_upstream_reads() {
        let output = tool_result(Some(&json!("no such file")), true);
        let rendered = serde_json::to_value(&output).unwrap();
        assert_eq!(rendered, "[tool execution error]\nno such file");
    }

    #[test]
    fn a_thinking_budget_picks_a_rung_and_its_absence_picks_none() {
        assert_eq!(effort(None, None), None);
        assert_eq!(effort(None, Some(&json!({"type": "disabled"}))), None);
        for (budget, rung) in [
            (4000, Effort::Low),
            (10000, Effort::Medium),
            (31999, Effort::High),
            (200000, Effort::Max),
        ] {
            assert_eq!(
                effort(
                    None,
                    Some(&json!({"type": "enabled", "budget_tokens": budget}))
                ),
                Some(rung),
                "{budget}"
            );
        }
    }

    /// The exact pair a live `alc --codex claude --effort max` session sent on
    /// 2026-09-09: the effort is in `output_config`, and the `thinking` beside
    /// it names no budget at all. Reading only the budget silently ran every
    /// turn at upstream's default.
    #[test]
    fn claude_codes_own_spelling_of_effort_wins_over_the_thinking_block() {
        assert_eq!(
            effort(
                Some(&json!({"effort": "max"})),
                Some(&json!({"type": "adaptive", "display": "omitted"}))
            ),
            Some(Effort::Max)
        );
        // The budget is only consulted when no rung was named.
        assert_eq!(
            effort(
                Some(&json!({"format": {"type": "text"}})),
                Some(&json!({"type": "enabled", "budget_tokens": 4000}))
            ),
            Some(Effort::Low)
        );
    }

    #[test]
    fn every_rung_alc_can_ask_for_reaches_the_ladder_upstream_has() {
        for (named, rung) in [
            ("none", Effort::None),
            ("low", Effort::Low),
            ("medium", Effort::Medium),
            ("high", Effort::High),
            ("xhigh", Effort::Xhigh),
            ("max", Effort::Max),
            // alc's ladder has a rung chatgpt.com does not.
            ("ultra", Effort::Max),
            ("MAX", Effort::Max),
        ] {
            assert_eq!(
                effort(Some(&json!({ "effort": named })), None),
                Some(rung),
                "{named}"
            );
        }
        // An unrecognised rung leaves the choice upstream rather than failing
        // the turn over a word.
        assert_eq!(effort(Some(&json!({"effort": "turbo"})), None), None);
    }

    #[test]
    fn a_system_prompt_in_blocks_becomes_one_developer_message() {
        let request: MessagesRequest = serde_json::from_value(json!({
            "model": "gpt-6-astra",
            "system": [{"type": "text", "text": "one"}, {"type": "text", "text": "two"}],
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .unwrap();
        let input = serde_json::to_value(to_upstream(&request).unwrap()).unwrap()["input"].clone();
        assert_eq!(input[0]["role"], "developer");
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["content"][0]["text"], "one\n\ntwo");
        assert_eq!(input.as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_request_this_bridge_cannot_read_is_refused_with_the_reason() {
        let error = parse(&Bytes::from_static(b"{\"messages\": []}")).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(
            error.message.contains("model"),
            "the refusal must name the missing field, got {:?}",
            error.message
        );

        let error = to_upstream(
            &serde_json::from_value(json!({
                "model": "gpt-6-astra",
                "messages": []
            }))
            .unwrap(),
        )
        .unwrap_err();
        assert!(error.message.contains("messages"), "{:?}", error.message);
    }

    #[test]
    fn an_exhausted_quota_ends_the_turn_instead_of_emptying_it() {
        let event: UpstreamEvent = serde_json::from_value(json!({
            "type": "codex.rate_limits",
            "rate_limits": {"limit_reached": true},
            "credits": {"has_credits": false, "unlimited": false}
        }))
        .unwrap();
        let mut turn = Turn::new("msg_test".to_owned(), "gpt-6-astra".to_owned(), 1);
        let error = turn.accept(&event).unwrap_err();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error.kind, "rate_limit_error");

        // And the client is told, rather than being handed a stream that just
        // stops: an error frame, after the message it is attached to.
        let frames = turn.fail(&error);
        assert!(matches!(frames[0], MessagesEvent::MessageStart { .. }));
        assert!(matches!(frames[1], MessagesEvent::Error { .. }));
    }

    #[test]
    fn a_refused_turn_relays_the_reason_chatgpt_com_gave() {
        let event: UpstreamEvent = serde_json::from_value(json!({
            "type": "response.failed",
            "response": {
                "id": "resp_1",
                "error": {"code": "model_not_found", "message": "Unknown model: gpt-6-astra"}
            }
        }))
        .unwrap();
        let mut turn = Turn::new("msg_test".to_owned(), "gpt-6-astra".to_owned(), 1);
        let error = turn.accept(&event).unwrap_err();
        // Acceptance criterion 5: the model name the server objected to has to
        // survive into the message, or the user is left guessing at it.
        assert!(
            error.message.contains("Unknown model: gpt-6-astra"),
            "{:?}",
            error.message
        );
    }

    #[test]
    fn a_tool_call_whose_arguments_never_streamed_still_carries_them() {
        let mut turn = Turn::new("msg_test".to_owned(), "gpt-6-astra".to_owned(), 1);
        let added: UpstreamEvent = serde_json::from_value(json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1",
                     "name": "read_file", "arguments": ""}
        }))
        .unwrap();
        let done: UpstreamEvent = serde_json::from_value(json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1",
                     "name": "read_file", "arguments": "{\"path\":\"/tmp\"}"}
        }))
        .unwrap();

        let mut events = turn.accept(&added).unwrap();
        events.extend(turn.accept(&done).unwrap());
        let message = collect(events).unwrap();
        match message.content.as_slice() {
            [ContentBlock::ToolUse { input, .. }] => {
                assert_eq!(input, &json!({"path": "/tmp"}));
            }
            other => panic!("expected one tool_use block, got {other:?}"),
        }
    }
}
