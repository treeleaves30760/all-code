//! The chatgpt.com Codex contract: what goes up, and what comes back.
//!
//! Every one of alc's three surfaces ends here. The shapes are transcribed
//! from a captured corpus rather than from documentation, because the endpoint
//! has none — see `tests/fixtures/codex/` and [`super::fixtures`].
//!
//! Three of those shapes are load-bearing and none of them are guessable:
//!
//! * tools ride as a `developer` **input item** of type `additional_tools`,
//!   not as the top-level `tools` array the public Responses API takes;
//! * the system prompt is a `developer` **message**, first after the tools;
//! * `store` must be `false` and `input` must be a list. The server answers
//!   `400 {"detail":"Store must be set to false"}` and
//!   `400 {"detail":"Input must be a list"}` respectively — both captured.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use axum::http::StatusCode;
use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::ReasoningEffort;

use super::auth::Credentials;
use super::{BridgeError, BridgeState};

/// The only upstream alc talks to.
pub(crate) const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Sent as both `originator` and `user-agent`. Codex gates the
/// `responses-lite` behaviour on recognising its own CLI, so this is not
/// cosmetic: a different string gets a different (worse) upstream.
pub(crate) const USER_AGENT: &str = "codex_cli_rs";

/// Request headers observed on every captured upstream call, beside
/// `authorization`, `chatgpt-account-id`, `accept` and `content-type`.
pub(crate) const OPENAI_BETA: &str = "responses=experimental";
pub(crate) const CODEX_BETA_FEATURES: &str = "remote_compaction_v2";
pub(crate) const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";

/// Which upstream lane a request asks for.
///
/// `responses-lite` is not free, and the price is paid by the body rather than
/// the header: with the lite header set, chatgpt.com refuses any request that
/// does not *also* carry `reasoning.context: "all_turns"` and
/// `parallel_tool_calls: false`, naming both in turn. Measured against
/// `gpt-5.6-terra` on 2026-09-09; the same body without the header is served.
///
/// So the two translating surfaces ask for [`Lane::Lite`], because they write
/// bodies that satisfy it. [`super::responses`] forwards a body it did not
/// write, and asking for lite there would refuse every client that had not
/// happened to spell those two fields the way Codex does — including all three
/// of the agents that use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lane {
    /// The cheaper path, for a body built to its constraints.
    Lite,
    /// The plain Responses endpoint, for a body alc is only carrying.
    Plain,
}

/// `client_metadata` over HTTP. The header above carries the flag; this is the
/// body's echo of it.
pub(crate) const LITE_METADATA_KEY: &str = "lite";
/// `client_metadata` over the WebSocket lane, where there are no per-request
/// headers, so the header name is spelled into the key.
pub(crate) const LITE_METADATA_WS_KEY: &str =
    "ws_request_header_x_openai_internal_codex_responses_lite";

/// How long to wait for chatgpt.com's response *headers*.
///
/// Only the handshake is bounded. Once the status line is in, a turn is
/// allowed to think for as long as it likes and [`BODY_IDLE_TIMEOUT`] takes
/// over — the two are separate because "never connected" and "connected and
/// thinking" need opposite answers.
const HEADER_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a started stream may go without producing a byte.
///
/// Five minutes, matching the bridge being replaced: long enough that a deep
/// reasoning turn is never cut off, short enough that a socket the far end
/// silently dropped does not hang an agent forever.
///
/// Shared with [`super::responses`], which relays a body rather than decoding
/// it but is waiting on the same socket for the same reason.
pub(crate) const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// The request body posted to [`CODEX_RESPONSES_URL`].
///
/// Field presence differs by transport and that difference is real: the HTTP
/// lane sends `stream` and omits `type`; the WebSocket lane sends
/// `type: "response.create"` and omits `stream` (the socket is the stream).
/// Both were captured; see the `messages-tool-call-streaming` and
/// `chat-completions-nonstreaming` fixtures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UpstreamRequest {
    pub model: String,
    /// `"response.create"` on the WebSocket lane only.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Always `false`. Anything else is a 400 from the server.
    pub store: bool,
    /// Set on the HTTP lane. The Chat Completions surface sets it `true` even
    /// for a non-streaming client and aggregates the stream itself — upstream
    /// has no non-streaming mode worth using.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// `false` on every captured request. Codex's own CLI serialises tool
    /// calls, and the Messages translation has no way to interleave two
    /// in-flight `tool_use` blocks.
    pub parallel_tool_calls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    pub text: TextOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_metadata: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// Must be a list, and carries the tools, the system prompt and the
    /// conversation in that order.
    pub input: Vec<InputItem>,
}

impl UpstreamRequest {
    /// Re-frames a body for the HTTP lane, which is the only transport this
    /// bridge speaks.
    ///
    /// Three fields differ between the lanes and none of the differences are
    /// cosmetic: `type: "response.create"` is the WebSocket envelope's own
    /// discriminator and is meaningless to the HTTP endpoint, `stream` says
    /// nothing on a socket that already is one, and the `client_metadata` flag
    /// spells the header name into its key only where there are no headers to
    /// carry it.
    ///
    /// [`send`] applies this rather than trusting callers to, because a
    /// surface built against the captured WebSocket body — the one its golden
    /// test compares against — would otherwise post that body verbatim and
    /// fail every real turn while its tests stayed green. Idempotent, so a
    /// caller that already framed for HTTP loses nothing.
    ///
    /// `stream` goes to `true` unconditionally: every surface reads the SSE
    /// and aggregates for a client that did not ask to stream, because the
    /// non-streaming upstream returns the same content later and with no way
    /// to report progress.
    pub(crate) fn over_http(mut self) -> Self {
        self.kind = None;
        self.stream = Some(true);
        if let Some(metadata) = self.client_metadata.as_mut()
            && let Some(value) = metadata.remove(LITE_METADATA_WS_KEY)
        {
            metadata.insert(LITE_METADATA_KEY.to_owned(), value);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Reasoning {
    /// Omitted for Claude Code, which sends its own per request; pinned for
    /// every other agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// `"all_turns"` on every captured request. Dropping it loses reasoning
    /// continuity across a tool-call round trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// The effort ladder chatgpt.com accepts.
///
/// Narrower than [`crate::config::ReasoningEffort`], which also has `Ultra`:
/// that tier is reachable through `alc codex` natively but not through this
/// endpoint, and `cli::resolve_codex_defaults` already clamps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Effort {
    None,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Clamps alc's ladder onto the one chatgpt.com accepts.
///
/// [`ReasoningEffort::Ultra`] exists on GPT-6 through `alc codex` speaking to
/// the model directly, but this endpoint refuses the word. Clamping to `max`
/// keeps the session running at the highest tier that is actually reachable;
/// forwarding it would turn a preference into a refused turn.
impl From<ReasoningEffort> for Effort {
    fn from(effort: ReasoningEffort) -> Self {
        match effort {
            ReasoningEffort::Low => Self::Low,
            ReasoningEffort::Medium => Self::Medium,
            ReasoningEffort::High => Self::High,
            ReasoningEffort::Xhigh => Self::Xhigh,
            ReasoningEffort::Max | ReasoningEffort::Ultra => Self::Max,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TextOptions {
    /// `"low"` on every captured request: the surfaces above want the model's
    /// answer, not its narration of the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Value>,
}

/// One entry of `input`.
///
/// Internally tagged on `type`, which is what makes `additional_tools` and
/// `message` distinguishable despite both carrying `role: "developer"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum InputItem {
    /// The tools. Always `role: "developer"`, always first.
    #[serde(rename = "additional_tools")]
    AdditionalTools { role: String, tools: Vec<Tool> },
    /// `role` is `developer` (system prompt), `user`, or `assistant`.
    #[serde(rename = "message")]
    Message {
        role: String,
        content: Vec<ContentPart>,
    },
    /// An assistant turn's tool call, replayed on the next request.
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        call_id: String,
        name: String,
        /// JSON, as a string. Not an object — the server rejects an object.
        arguments: String,
    },
    /// The result the client fed back for a `function_call`.
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        call_id: String,
        output: FunctionCallOutput,
    },
    /// Opaque reasoning state handed back so a multi-turn tool loop keeps its
    /// chain of thought. `encrypted_content` is never inspected.
    #[serde(rename = "reasoning")]
    Reasoning {
        id: String,
        #[serde(default)]
        summary: Vec<Value>,
        encrypted_content: String,
    },
}

/// `output` of a `function_call_output`: a bare string, or content parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum FunctionCallOutput {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ContentPart {
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(rename = "input_image")]
    InputImage {
        image_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// A function tool, inside [`InputItem::AdditionalTools`].
///
/// `strict` was `false` on the captured request. Setting it `true` makes the
/// server validate arguments against `parameters`, and agent-supplied schemas
/// are not reliably strict-mode legal (missing `additionalProperties: false`,
/// optional fields absent from `required`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Tool {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub strict: bool,
    /// The JSON Schema, verbatim from the client.
    pub parameters: Value,
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// One `data:` payload off the upstream stream.
///
/// Internally tagged on `type` rather than on the SSE `event:` name, because
/// the two transports disagree about whether there is one: the HTTP lane sends
/// `event:` lines, the WebSocket lane sends bare `data:`. Both captures are in
/// the corpus and both must parse.
///
/// [`UpstreamEvent::Other`] absorbs everything not listed. New event types
/// appear upstream without warning and a stream must not fail over one.
///
/// Several payload fields below have no reader today, and are kept anyway:
/// this endpoint has no published schema, so the decoded shape here is alc's
/// only written record of what the corpus showed arriving. A field deleted for
/// being unread is a fact the next person has to re-capture.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum UpstreamEvent {
    #[serde(rename = "response.created")]
    Created { response: ResponseObject },
    #[serde(rename = "response.in_progress")]
    InProgress { response: ResponseObject },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { output_index: u32, item: OutputItem },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone { output_index: u32, item: OutputItem },
    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        output_index: u32,
        content_index: u32,
        part: Value,
    },
    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        output_index: u32,
        content_index: u32,
        part: Value,
    },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        output_index: u32,
        content_index: u32,
        text: String,
    },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        output_index: u32,
        item_id: String,
        delta: String,
    },
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        output_index: u32,
        item_id: String,
        arguments: String,
    },
    #[serde(rename = "response.completed")]
    Completed { response: ResponseObject },
    /// Sent when the turn stopped short (token cap, upstream cutoff). The
    /// Messages surface must map it to `stop_reason: "max_tokens"`, not to a
    /// clean stop.
    #[serde(rename = "response.incomplete")]
    Incomplete { response: ResponseObject },
    #[serde(rename = "response.failed")]
    Failed { response: ResponseObject },
    /// The upstream's own error frame, outside a response envelope.
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        error: Value,
    },
    /// Headers replayed into the stream on the WebSocket lane, where the
    /// client never saw the HTTP response headers.
    #[serde(rename = "codex.response.metadata")]
    Metadata {
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    /// Quota state. `rate_limits.limit_reached` with no credits is a terminal
    /// 429 and must be surfaced as one rather than silently emptying a stream.
    #[serde(rename = "codex.rate_limits")]
    RateLimits(Box<RateLimits>),
    /// Timing telemetry. Carries nothing a client needs; drop it.
    #[serde(rename = "responsesapi.websocket_timing")]
    WebsocketTiming,
    #[serde(other)]
    Other,
}

impl UpstreamEvent {
    /// Whether this event ends the turn.
    ///
    /// A surface has to know, because the stream does not always close after
    /// its last meaningful frame: the WebSocket-transcoded lane trails
    /// telemetry, and a connection that is being pooled upstream stays open.
    /// Waiting for EOF instead of for this would add the idle timeout to the
    /// end of every turn.
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. }
                | Self::Incomplete { .. }
                | Self::Failed { .. }
                | Self::Error { .. }
        )
    }
}

/// The `response` envelope carried by the lifecycle events.
///
/// Deliberately partial: the captured object has thirty-odd fields and alc
/// reads six. Unknown fields are ignored, so an upstream addition is a no-op.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ResponseObject {
    pub id: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub created_at: Option<u64>,
    /// `in_progress`, `completed`, `incomplete`, `failed`.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub error: Option<Value>,
}

/// Token counts, arriving only on the terminal event.
///
/// `input_tokens` here is the upstream's real figure. The Messages surface has
/// already sent `message_start` with a local estimate by the time it lands
/// (73 estimated against 138 actual, in the captured turn), which is why the
/// corrected numbers are re-sent on `message_delta`.
#[allow(dead_code)] // see UpstreamEvent's note on unread wire fields
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(crate) struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

/// An item of the response's output, on `output_item.added` / `.done`.
#[allow(dead_code)] // see UpstreamEvent's note on unread wire fields
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum OutputItem {
    /// A tool call. `call_id` is what the client answers with; `id` is the
    /// upstream's handle, and the two are different strings — the argument
    /// deltas key off `id`, the client's `tool_result` keys off `call_id`.
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        #[serde(default)]
        arguments: String,
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(rename = "message")]
    Message {
        id: String,
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        content: Vec<Value>,
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        id: String,
        #[serde(default)]
        summary: Vec<Value>,
        #[serde(default)]
        encrypted_content: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[allow(dead_code)] // see UpstreamEvent's note on unread wire fields
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RateLimits {
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default)]
    pub rate_limits: Option<RateLimitWindows>,
    #[serde(default)]
    pub credits: Option<Credits>,
}

#[allow(dead_code)] // see UpstreamEvent's note on unread wire fields
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RateLimitWindows {
    #[serde(default)]
    pub allowed: Option<bool>,
    #[serde(default)]
    pub limit_reached: Option<bool>,
    #[serde(default)]
    pub primary: Option<RateLimitWindow>,
    #[serde(default)]
    pub secondary: Option<RateLimitWindow>,
}

#[allow(dead_code)] // see UpstreamEvent's note on unread wire fields
#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct RateLimitWindow {
    #[serde(default)]
    pub used_percent: f64,
    #[serde(default)]
    pub window_minutes: u64,
    #[serde(default)]
    pub reset_after_seconds: u64,
}

#[allow(dead_code)] // see UpstreamEvent's note on unread wire fields
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Credits {
    #[serde(default)]
    pub has_credits: bool,
    #[serde(default)]
    pub unlimited: bool,
    #[serde(default)]
    pub balance: Option<String>,
}

impl RateLimits {
    /// True when the account is out of quota with nothing to fall back on.
    ///
    /// Credits override an exhausted window: the event still says
    /// `limit_reached`, and the turn still succeeds.
    pub(crate) fn is_terminal(&self) -> bool {
        let reached = self
            .rate_limits
            .as_ref()
            .and_then(|windows| windows.limit_reached)
            .unwrap_or(false);
        let rescued = self
            .credits
            .as_ref()
            .is_some_and(|credits| credits.has_credits || credits.unlimited);
        reached && !rescued
    }
}

// ---------------------------------------------------------------------------
// SSE framing
// ---------------------------------------------------------------------------

/// One parsed Server-Sent Events frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseFrame {
    /// The `event:` line, absent on the WebSocket-transcoded lane.
    pub event: Option<String>,
    /// The joined `data:` lines.
    pub data: String,
}

/// Splits an SSE body into frames.
///
/// Shared rather than per-surface because all three read the same stream, and
/// because the two lanes frame differently: `event:` + `data:` over HTTP, bare
/// `data:` over the WebSocket transcode. Callers dispatch on the JSON `type`
/// field, so a missing `event:` line costs nothing.
pub(crate) fn parse_sse(body: &str) -> Vec<SseFrame> {
    let mut frames = Vec::new();
    let mut event: Option<String> = None;
    let mut data: Vec<&str> = Vec::new();

    for line in body.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if !data.is_empty() {
                frames.push(SseFrame {
                    event: event.take(),
                    data: data.join("\n"),
                });
                data.clear();
            }
            event = None;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => event = Some(value.to_owned()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if !data.is_empty() {
        frames.push(SseFrame {
            event,
            data: data.join("\n"),
        });
    }
    frames
}

/// Splits a *live* byte stream into frames.
///
/// [`parse_sse`] cannot do this alone: a chunk ends wherever TCP decided to
/// cut it, routinely mid-frame and sometimes mid-character. So the split
/// happens on bytes — the separator is ASCII and cannot appear inside a
/// multi-byte sequence, so a completed frame is always whole text — and the
/// decode happens afterwards, through the one parser.
#[derive(Debug, Default)]
pub(crate) struct SseDecoder {
    buffer: Vec<u8>,
    /// How far into `buffer` the search has already looked without finding a
    /// separator. Without it every chunk rescans the whole pending frame,
    /// which is quadratic on a turn that arrives one token per chunk — which
    /// is every turn.
    scanned: usize,
}

impl SseDecoder {
    /// Feeds one chunk and returns whatever frames it completed.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        loop {
            let from = self.scanned;
            let Some((end, next)) = frame_boundary(&self.buffer[from..]) else {
                // The separator is three bytes at its longest and can straddle
                // the join, so the next search resumes two bytes back.
                self.scanned = self.buffer.len().saturating_sub(2);
                return frames;
            };
            let text = String::from_utf8_lossy(&self.buffer[..from + end]).into_owned();
            self.buffer.drain(..from + next);
            self.scanned = 0;
            frames.extend(parse_sse(&text));
        }
    }

    /// Flushes a trailing frame that arrived without its blank line.
    ///
    /// Every captured stream terminates its last frame properly, but a
    /// connection cut between the final `data:` line and the separator would
    /// otherwise drop a `response.completed` on the floor and leave the
    /// surface reporting a turn that actually finished as a truncated one.
    pub(crate) fn finish(&mut self) -> Vec<SseFrame> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();
        self.scanned = 0;
        parse_sse(&text)
    }
}

/// Where the first complete frame's text ends, and where the next one starts.
fn frame_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer.iter().enumerate().find_map(|(index, byte)| {
        if *byte != b'\n' {
            return None;
        }
        match (buffer.get(index + 1), buffer.get(index + 2)) {
            (Some(&b'\n'), _) => Some((index, index + 2)),
            (Some(&b'\r'), Some(&b'\n')) => Some((index, index + 3)),
            _ => None,
        }
    })
}

/// One decoded upstream event, with the payload it came from.
///
/// The raw text rides along because [`UpstreamEvent::Other`] is otherwise
/// opaque: a surface meeting an event type nobody has modelled yet can at
/// least name it, and an `error` frame's detail lives in fields no struct here
/// claims.
#[derive(Debug, Clone)]
pub(crate) struct UpstreamFrame {
    pub raw: String,
    pub event: UpstreamEvent,
}

/// What one SSE frame turned out to be.
#[derive(Debug)]
enum Decoded {
    Event(Box<UpstreamFrame>),
    /// The `[DONE]` sentinel: nothing follows.
    End,
    /// A frame carrying no payload — a keepalive, or a comment.
    Empty,
}

/// Reads one frame.
///
/// `[DONE]` never appeared in the captured corpus, because a Codex turn ends
/// on `response.completed`, but it is how every other OpenAI-shaped stream
/// signs off and feeding it to `serde_json` would fail a turn that had already
/// succeeded.
fn decode_frame(frame: &SseFrame) -> Result<Decoded, BridgeError> {
    let data = frame.data.trim();
    if data.is_empty() {
        return Ok(Decoded::Empty);
    }
    if data == "[DONE]" {
        return Ok(Decoded::End);
    }
    let event = serde_json::from_str(data).map_err(|error| {
        BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("chatgpt.com sent a stream frame that is not JSON: {error}"),
        )
    })?;
    Ok(Decoded::Event(Box::new(UpstreamFrame {
        raw: data.to_owned(),
        event,
    })))
}

/// A turn in progress: chatgpt.com's SSE, decoded as the bytes land.
pub(crate) struct UpstreamStream {
    response: reqwest::Response,
    decoder: SseDecoder,
    ready: VecDeque<SseFrame>,
    finished: bool,
}

impl UpstreamStream {
    fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            decoder: SseDecoder::default(),
            ready: VecDeque::new(),
            finished: false,
        }
    }

    /// The next event, or `None` once the stream is spent.
    ///
    /// An event type this bridge has never seen arrives as
    /// [`UpstreamEvent::Other`] rather than as an error — upstream adds types
    /// without warning, and a turn it answered correctly must not fail on one.
    /// A frame that is not JSON at all is a different thing and does fail,
    /// because past that point nothing downstream can be trusted.
    pub(crate) async fn next(&mut self) -> Option<Result<UpstreamFrame, BridgeError>> {
        loop {
            while let Some(frame) = self.ready.pop_front() {
                match decode_frame(&frame) {
                    Ok(Decoded::Event(frame)) => return Some(Ok(*frame)),
                    Ok(Decoded::End) => self.finished = true,
                    Ok(Decoded::Empty) => {}
                    Err(error) => {
                        self.finished = true;
                        return Some(Err(error));
                    }
                }
            }
            if self.finished {
                return None;
            }
            match tokio::time::timeout(BODY_IDLE_TIMEOUT, self.response.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    let frames = self.decoder.push(&chunk);
                    self.ready.extend(frames);
                }
                Ok(Ok(None)) => {
                    self.finished = true;
                    let frames = self.decoder.finish();
                    self.ready.extend(frames);
                }
                Ok(Err(error)) => {
                    self.finished = true;
                    return Some(Err(BridgeError::upstream(
                        StatusCode::BAD_GATEWAY,
                        format!("the Codex stream broke: {}", describe(&error)),
                    )));
                }
                Err(_) => {
                    self.finished = true;
                    return Some(Err(BridgeError::new(
                        StatusCode::GATEWAY_TIMEOUT,
                        "api_error",
                        format!(
                            "chatgpt.com sent nothing for {}s; the Codex stream is stalled",
                            BODY_IDLE_TIMEOUT.as_secs()
                        ),
                    )));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

/// The header set observed on every captured HTTP-lane call, and nothing else.
///
/// `originator` and `user-agent` both carry [`USER_AGENT`] because Codex gates
/// the cheaper `responses-lite` behaviour on recognising its own CLI in both.
/// The lite header is what selects the lane on this transport — the capture
/// carries it on a request whose body had no `client_metadata` at all — which
/// is why it is [`Lane`]'s decision and not a constant. Every other header is
/// the same on both lanes.
pub(crate) fn request_headers(
    credentials: &Credentials,
    streaming: bool,
    lane: Lane,
) -> Result<HeaderMap, BridgeError> {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert(
        "accept",
        HeaderValue::from_static(if streaming {
            "text/event-stream"
        } else {
            "application/json"
        }),
    );
    headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
    headers.insert("originator", HeaderValue::from_static(USER_AGENT));
    headers.insert("openai-beta", HeaderValue::from_static(OPENAI_BETA));
    headers.insert(
        "x-codex-beta-features",
        HeaderValue::from_static(CODEX_BETA_FEATURES),
    );
    if lane == Lane::Lite {
        headers.insert(RESPONSES_LITE_HEADER, HeaderValue::from_static("true"));
    }
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {}", credentials.access_token)).map_err(|_| {
            BridgeError::auth(
                "the stored Codex access token cannot be sent as a header; run `codex login`",
            )
        })?,
    );
    // Codex refuses a request without this even when the bearer token is
    // good, so a credential set that lacks it is worth naming here rather than
    // relaying a 400 that says nothing about credentials.
    let account_id = credentials.account_id.as_deref().ok_or_else(|| {
        BridgeError::auth(
            "the Codex credentials carry no ChatGPT account id; run `codex login` to refresh them",
        )
    })?;
    headers.insert(
        "chatgpt-account-id",
        HeaderValue::from_str(account_id).map_err(|_| {
            BridgeError::auth("the stored ChatGPT account id cannot be sent as a header")
        })?,
    );
    Ok(headers)
}

/// Posts a body to chatgpt.com and hands back whatever it answered, refusal
/// included.
///
/// Deliberately does not judge the status: `/v1/responses` is a passthrough
/// and its clients are owed the server's own verdict, byte for byte. Callers
/// that translate use [`send`], which does judge it.
pub(crate) async fn post(
    state: &BridgeState,
    body: Bytes,
    streaming: bool,
    lane: Lane,
) -> Result<reqwest::Response, BridgeError> {
    let credentials = state.auth.credentials(&state.http).await?;
    let response = post_once(state, &credentials, body.clone(), streaming, lane).await?;
    if response.status() != StatusCode::UNAUTHORIZED {
        return Ok(response);
    }
    // A token that passed the local expiry check can still be refused: revoked
    // upstream, or rotated by another Codex process since this one cached it.
    // One forced rotation and one retry is the difference between a session
    // that keeps working and a turn the user has to type again.
    let refreshed = state
        .auth
        .force_refresh(&state.http, Some(&credentials.access_token))
        .await?;
    post_once(state, &refreshed, body, streaming, lane).await
}

/// Sends a translated request and hands back the events it produced.
///
/// Always streams, whatever the client asked for: the surfaces aggregate a
/// non-streaming answer themselves, because upstream's non-streaming mode
/// returns the same content later with nothing to show in the meantime.
pub(crate) async fn send(
    state: &BridgeState,
    request: UpstreamRequest,
) -> Result<UpstreamStream, BridgeError> {
    let request = request.over_http();
    let body = serde_json::to_vec(&request).map_err(|error| {
        BridgeError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            format!(
                "could not serialise the Codex request for {}: {error}",
                request.model
            ),
        )
    })?;
    let response = post(state, Bytes::from(body), true, Lane::Lite).await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.bytes().await.unwrap_or_default();
        return Err(status_error(status, &body));
    }
    Ok(UpstreamStream::new(response))
}

async fn post_once(
    state: &BridgeState,
    credentials: &Credentials,
    body: Bytes,
    streaming: bool,
    lane: Lane,
) -> Result<reqwest::Response, BridgeError> {
    let request = state
        .http
        .post(CODEX_RESPONSES_URL)
        .headers(request_headers(credentials, streaming, lane)?)
        .body(body);
    match tokio::time::timeout(HEADER_TIMEOUT, request.send()).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(BridgeError::new(
            StatusCode::BAD_GATEWAY,
            "api_error",
            format!("could not reach chatgpt.com: {}", describe(&error)),
        )),
        Err(_) => Err(BridgeError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "api_error",
            format!(
                "chatgpt.com did not answer within {}s",
                HEADER_TIMEOUT.as_secs()
            ),
        )),
    }
}

/// Turns a refused response into an error that names the refusal.
///
/// The status is relayed rather than flattened into a 500, and the taxonomy
/// follows it: agents retry a `rate_limit_error` and re-authenticate on an
/// `authentication_error`, so mislabelling either turns a recoverable refusal
/// into an abandoned session.
pub(crate) fn status_error(status: StatusCode, body: &[u8]) -> BridgeError {
    let detail = error_message(body)
        .unwrap_or_else(|| format!("chatgpt.com refused the request with status {status}"));
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => BridgeError::new(
            status,
            "authentication_error",
            format!("{detail} (if this persists, run `codex login`)"),
        ),
        StatusCode::TOO_MANY_REQUESTS => BridgeError::new(status, "rate_limit_error", detail),
        _ => BridgeError::upstream(status, detail),
    }
}

/// Digs the server's own words out of a refusal.
///
/// It spells its complaint four ways, and which arrives depends on how far the
/// request got: `{"detail": …}` from the endpoint's own validation (both
/// captured `/v1/responses` refusals are this), `{"error":{"message": …}}`
/// from the model gateway, a bare `{"message": …}` from the edge, and — when
/// the request was accepted and then failed — an `error` frame inside a body
/// that had already returned 200.
///
/// Falling back to the raw text is the point of the last branch: an HTML error
/// page from an intercepting proxy is ugly, but "your requests are being
/// filtered" told plainly beats a bridge inventing "the upstream failed".
pub(crate) fn error_message(body: &[u8]) -> Option<String> {
    if let Ok(value) = serde_json::from_slice::<Value>(body)
        && let Some(message) = json_error_message(&value)
    {
        return Some(message);
    }
    let text = std::str::from_utf8(body).ok()?;
    for frame in parse_sse(text) {
        if let Ok(value) = serde_json::from_str::<Value>(&frame.data)
            && let Some(message) = json_error_message(&value)
        {
            return Some(message);
        }
    }
    let text = text.trim();
    (!text.is_empty()).then(|| truncate(text, 200))
}

/// The pointers a Codex refusal has been observed to put its reason behind, in
/// the order of how specific each one is.
fn json_error_message(value: &Value) -> Option<String> {
    [
        "/error/message",
        "/response/error/message",
        "/detail",
        "/message",
        "/error",
    ]
    .into_iter()
    .find_map(|pointer| match value.pointer(pointer) {
        Some(Value::String(message)) if !message.trim().is_empty() => Some(message.clone()),
        _ => None,
    })
}

/// A `reqwest::Error` displays as "error sending request"; the reason — DNS,
/// TLS, refused, reset — is one or more `source`s below it, and that reason is
/// the whole of what a stuck user needs.
fn describe(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(inner) = source {
        parts.push(inner.to_string());
        source = inner.source();
    }
    parts.join(": ")
}

/// Bounds a relayed message so an HTML page cannot become the error.
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::fixtures::Case;
    use serde_json::json;

    #[test]
    fn the_websocket_lane_frames_without_an_event_line() {
        let frames = parse_sse(&Case::load("messages-tool-call-streaming").upstream_sse());
        assert!(frames.iter().all(|frame| frame.event.is_none()));
        assert_eq!(frames.len(), 17);
    }

    #[test]
    fn the_http_lane_frames_with_one() {
        let frames = parse_sse(&Case::load("chat-completions-nonstreaming").upstream_sse());
        assert_eq!(
            frames.first().unwrap().event.as_deref(),
            Some("response.created")
        );
        assert_eq!(frames.len(), 10);
    }

    #[test]
    fn every_captured_event_parses_into_the_enum() {
        for name in [
            "messages-tool-call-streaming",
            "chat-completions-nonstreaming",
        ] {
            for frame in parse_sse(&Case::load(name).upstream_sse()) {
                let event: UpstreamEvent = serde_json::from_str(&frame.data)
                    .unwrap_or_else(|error| panic!("{name}: {error}: {}", frame.data));
                assert!(
                    !matches!(event, UpstreamEvent::Other),
                    "{name}: unmodelled event type in {}",
                    frame.data
                );
            }
        }
    }

    #[test]
    fn the_captured_upstream_requests_round_trip_unchanged() {
        for name in [
            "messages-tool-call-streaming",
            "chat-completions-nonstreaming",
        ] {
            let captured = Case::load(name).upstream_request();
            let parsed: UpstreamRequest = serde_json::from_value(captured.clone()).unwrap();
            assert_eq!(serde_json::to_value(&parsed).unwrap(), captured, "{name}");
        }
    }

    fn credentials() -> Credentials {
        Credentials {
            access_token: "token".to_owned(),
            refresh_token: "refresh".to_owned(),
            account_id: Some("acct-1".to_owned()),
            expires_at_ms: u64::MAX,
        }
    }

    fn header(headers: &HeaderMap, name: &str) -> String {
        headers
            .get(name)
            .unwrap_or_else(|| panic!("no {name} header"))
            .to_str()
            .expect("the header is text")
            .to_owned()
    }

    #[test]
    fn the_upstream_headers_are_the_set_the_capture_shows() {
        let headers = request_headers(&credentials(), true, Lane::Lite).unwrap();
        let metadata = Case::load("chat-completions-nonstreaming").upstream_metadata();
        let captured = metadata["headers"].as_object().unwrap();

        let mut sent: Vec<&str> = headers.keys().map(|name| name.as_str()).collect();
        sent.sort_unstable();
        let mut expected: Vec<&str> = captured.keys().map(String::as_str).collect();
        expected.sort_unstable();
        assert_eq!(sent, expected);

        for (name, value) in captured {
            let captured_value = value.as_str().unwrap();
            // The capture masked the two secrets and left everything else
            // verbatim, so everything else has to match exactly.
            if captured_value.starts_with("[redacted") {
                continue;
            }
            assert_eq!(header(&headers, name), captured_value, "{name}");
        }
        assert_eq!(header(&headers, "authorization"), "Bearer token");
        assert_eq!(header(&headers, "chatgpt-account-id"), "acct-1");
    }

    #[test]
    fn a_non_streaming_call_asks_for_json_rather_than_events() {
        let metadata = Case::load("responses-passthrough-input-list").upstream_metadata();
        assert_eq!(metadata["headers"]["accept"], "application/json");
        let headers = request_headers(&credentials(), false, Lane::Lite).unwrap();
        assert_eq!(header(&headers, "accept"), "application/json");
    }

    /// The lite header is the *only* difference between the lanes, and it is
    /// the one the passthrough surface cannot afford: with it set, chatgpt.com
    /// demands two more fields in a body that surface did not write.
    #[test]
    fn the_plain_lane_differs_from_the_lite_one_by_exactly_the_lite_header() {
        let lite = request_headers(&credentials(), true, Lane::Lite).unwrap();
        let plain = request_headers(&credentials(), true, Lane::Plain).unwrap();
        assert!(plain.get(RESPONSES_LITE_HEADER).is_none());
        let mut dropped: Vec<&str> = lite
            .keys()
            .map(|name| name.as_str())
            .filter(|name| plain.get(*name).is_none())
            .collect();
        dropped.sort_unstable();
        assert_eq!(dropped, [RESPONSES_LITE_HEADER]);
        for (name, value) in &plain {
            assert_eq!(lite.get(name), Some(value), "{name}");
        }
    }

    #[test]
    fn credentials_without_an_account_id_are_refused_by_name() {
        let mut credentials = credentials();
        credentials.account_id = None;
        let error = request_headers(&credentials, true, Lane::Lite).unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert!(error.message.contains("account id"), "{}", error.message);
        assert!(error.message.contains("codex login"), "{}", error.message);
    }

    #[test]
    fn the_websocket_body_is_reframed_for_the_http_lane() {
        let captured = Case::load("messages-tool-call-streaming").upstream_request();
        let request: UpstreamRequest = serde_json::from_value(captured).unwrap();
        assert_eq!(request.kind.as_deref(), Some("response.create"));

        let framed = serde_json::to_value(request.over_http()).unwrap();
        assert!(framed.get("type").is_none());
        assert_eq!(framed["stream"], true);
        assert_eq!(framed["client_metadata"], json!({ "lite": "true" }));
        // Everything the transport does not own survives the re-framing.
        assert_eq!(framed["store"], false);
        assert_eq!(framed["input"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn reframing_a_body_that_is_already_http_changes_nothing() {
        let captured = Case::load("chat-completions-nonstreaming").upstream_request();
        let request: UpstreamRequest = serde_json::from_value(captured.clone()).unwrap();
        assert_eq!(serde_json::to_value(request.over_http()).unwrap(), captured);
    }

    #[test]
    fn a_stream_cut_into_chunks_decodes_to_the_same_frames() {
        for name in [
            "messages-tool-call-streaming",
            "chat-completions-nonstreaming",
        ] {
            let body = Case::load(name).upstream_sse();
            let whole = parse_sse(&body);
            for size in [1, 7, 64, 4096] {
                let mut decoder = SseDecoder::default();
                let mut frames = Vec::new();
                for chunk in body.as_bytes().chunks(size) {
                    frames.extend(decoder.push(chunk));
                }
                frames.extend(decoder.finish());
                assert_eq!(frames, whole, "{name} in {size}-byte chunks");
            }
        }
    }

    #[test]
    fn a_final_frame_with_no_blank_line_after_it_is_still_delivered() {
        let mut decoder = SseDecoder::default();
        assert!(
            decoder
                .push(b"data: {\"type\":\"response.completed\"")
                .is_empty()
        );
        let frames = decoder.push(b",\"response\":{\"id\":\"resp_1\"}}");
        assert!(frames.is_empty());
        let frames = decoder.finish();
        assert_eq!(frames.len(), 1);
        assert!(matches!(
            decode_frame(&frames[0]).unwrap(),
            Decoded::Event(_)
        ));
    }

    #[test]
    fn a_refusal_is_reported_in_the_servers_own_words() {
        for (name, detail) in [
            (
                "responses-passthrough-input-list",
                "Store must be set to false",
            ),
            ("responses-passthrough-input-string", "Input must be a list"),
        ] {
            let body = serde_json::to_vec(&Case::load(name).upstream_body()).unwrap();
            assert_eq!(error_message(&body).as_deref(), Some(detail), "{name}");
            let error = status_error(StatusCode::BAD_REQUEST, &body);
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert_eq!(error.message, detail, "{name}");
        }
    }

    #[test]
    fn a_refusal_that_arrived_as_a_stream_frame_still_names_its_reason() {
        let body = b"event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"model not found\"}}\n\n";
        assert_eq!(error_message(body).as_deref(), Some("model not found"));

        // The other spelling: a turn that started, then failed inside its own
        // response envelope after the status line said 200.
        let failed = br#"data: {"type":"response.failed","response":{"id":"resp_1","error":{"message":"the model stopped responding"}}}

"#;
        assert_eq!(
            error_message(failed).as_deref(),
            Some("the model stopped responding")
        );
    }

    #[test]
    fn a_refusal_that_is_not_json_at_all_is_relayed_rather_than_paraphrased() {
        let error = status_error(StatusCode::BAD_GATEWAY, b"<html>blocked by policy</html>");
        assert!(
            error.message.contains("blocked by policy"),
            "{}",
            error.message
        );
        // An error page long enough to flood a client's log is cut short.
        let long = "x".repeat(5_000);
        assert!(error_message(long.as_bytes()).unwrap().chars().count() <= 201);
    }

    #[test]
    fn an_expired_token_is_refused_as_an_auth_problem_naming_the_fix() {
        let error = status_error(
            StatusCode::UNAUTHORIZED,
            br#"{"error":{"message":"Missing or invalid token"}}"#,
        );
        assert_eq!(error.kind, "authentication_error");
        assert!(
            error.message.contains("Missing or invalid token"),
            "{}",
            error.message
        );
        assert!(error.message.contains("codex login"), "{}", error.message);
    }

    #[test]
    fn ultra_effort_is_clamped_to_the_ladder_upstream_accepts() {
        assert_eq!(Effort::from(ReasoningEffort::Ultra), Effort::Max);
        assert_eq!(Effort::from(ReasoningEffort::Max), Effort::Max);
        assert_eq!(
            serde_json::to_value(Effort::from(ReasoningEffort::Medium)).unwrap(),
            "medium"
        );
    }

    #[test]
    fn each_captured_stream_ends_on_exactly_one_terminal_event() {
        for name in [
            "messages-tool-call-streaming",
            "chat-completions-nonstreaming",
        ] {
            let body = Case::load(name).upstream_sse();
            let events: Vec<UpstreamEvent> = parse_sse(&body)
                .iter()
                .map(|frame| serde_json::from_str(&frame.data).unwrap())
                .collect();
            assert_eq!(
                events.iter().filter(|event| event.is_terminal()).count(),
                1,
                "{name}"
            );
            assert!(events.last().unwrap().is_terminal(), "{name}");
        }
    }

    #[test]
    fn the_done_sentinel_ends_a_stream_instead_of_failing_it() {
        let frames = parse_sse("data: [DONE]\n\n");
        assert!(matches!(decode_frame(&frames[0]).unwrap(), Decoded::End));
    }

    #[test]
    fn an_event_type_nobody_has_modelled_is_carried_rather_than_refused() {
        let frames = parse_sse("data: {\"type\":\"codex.something.new\",\"n\":1}\n\n");
        let Decoded::Event(frame) = decode_frame(&frames[0]).unwrap() else {
            panic!("an unknown event must still decode");
        };
        assert!(matches!(frame.event, UpstreamEvent::Other));
        assert!(!frame.event.is_terminal());
        // The payload survives, so a surface can say what it dropped.
        assert!(frame.raw.contains("codex.something.new"));
    }

    #[test]
    fn a_frame_that_is_not_json_fails_the_stream() {
        let frames = parse_sse("data: not json\n\n");
        let error = decode_frame(&frames[0]).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("not JSON"), "{}", error.message);
    }
}
