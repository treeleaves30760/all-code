//! `POST /v1/responses` — opencode, pi, kimi.
//!
//! The thin surface. Its clients already speak the OpenAI Responses API, which
//! is the API chatgpt.com's Codex endpoint answers, so there is almost nothing
//! to translate: the body goes up close to as it arrived and the answer comes
//! back untouched. A field alc has never heard of still reaches the model, and
//! a request the server dislikes is refused in the server's own words rather
//! than a bridge's paraphrase of them.
//!
//! Almost. This endpoint is stricter than the API it imitates, and every
//! narrowing below was measured against it rather than reasoned about:
//!
//! * `store` must be `false` and `input` must be a list. Both captured, both
//!   400s: `{"detail":"Store must be set to false"}` and
//!   `{"detail":"Input must be a list"}`. Neither client was wrong about the
//!   public API, and relaying those two verdicts would leave three of alc's
//!   eight agents unable to complete a turn over a spelling.
//! * a dozen ordinary Responses parameters are refused outright, one per
//!   request, as `{"detail":"Unsupported parameter: <name>"}` —
//!   [`REFUSED_PARAMETERS`] is the measured list. OpenCode sends
//!   `max_output_tokens` on every turn, so without this the surface has no
//!   working client at all.
//! * the `responses-lite` header is not sent here. It obliges the *body* to
//!   carry `reasoning.context: "all_turns"` and `parallel_tool_calls: false`,
//!   which is a promise only a body alc wrote can keep. See
//!   [`upstream::Lane`].
//!
//! That is the whole list; everything else is still the client's request and
//! still the server's verdict. And because a list of refused parameters is
//! exactly the kind of thing that goes stale, the server's own naming of one
//! is also acted on: a 400 that names a parameter alc is carrying drops it and
//! retries, so the day this endpoint refuses a thirteenth, the turn still
//! completes.
//!
//! The two shape repairs converge, which is what makes them testable without a
//! captured success: `responses-passthrough-input-list` and
//! `responses-passthrough-input-string` are one prompt sent two ways, and
//! normalising either produces the same legal body.
//!
//! All of the above is the Codex upstream. On Antigravity there is nothing to
//! pass through: Code Assist speaks Gemini, so the body is read into the
//! bridge's internal request ([`antigravity_request`]) and the shared
//! translator carries it from there, the way the other two surfaces travel.
//! What comes back is already Responses events, relayed under their own
//! `event:` names or folded as [`fold_response`] folds chatgpt.com's. A
//! reasoning item's `encrypted_content` is the sealed Gemini signature both
//! ways, so a client that replays its items verbatim keeps the model's chain;
//! one that drops them is signed from the call-id cache instead.

use std::convert::Infallible;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};

use super::upstream::{
    self, ContentPart, Effort, FunctionCallOutput, InputItem, Lane, Reasoning, TextOptions, Tool,
    UpstreamRequest, UpstreamStream,
};
use super::{BridgeError, BridgeState};
use crate::config::ReasoningEffort;
use crate::usage::observer::{RequestObservation, StreamObservation};
use crate::usage::records::Outcome;

/// Responses parameters chatgpt.com's Codex endpoint refuses outright.
///
/// Measured on 2026-09-09 against `gpt-5.6-terra` by posting an otherwise
/// legal body with one extra field: each of these answered
/// `400 {"detail":"Unsupported parameter: <name>"}`, with or without the lite
/// header, while `instructions`, `include`, `text`, `reasoning`, `tools`,
/// `tool_choice`, `parallel_tool_calls`, `prompt_cache_key` and `service_tier`
/// were all served. `stream_options` is here for its members rather than
/// itself: `{"include_usage":true}` came back as
/// `Unknown parameter: 'stream_options.include_usage'`.
///
/// Dropping beats relaying. A client that asked for `temperature` and got a
/// good answer without it is better served than one whose every turn is
/// refused, and these are the parameters no Codex request can carry — not
/// preferences this bridge is overriding.
const REFUSED_PARAMETERS: &[&str] = &[
    "background",
    "max_output_tokens",
    "max_tool_calls",
    "metadata",
    "previous_response_id",
    "safety_identifier",
    "stream_options",
    "temperature",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
];

/// How many times a refusal that names a parameter may be answered by dropping
/// it and trying again.
///
/// Bounded because the loop is driven by the server: a refusal that keeps
/// naming fields alc has already removed must end as a refusal the client
/// sees, not as a bridge posting forever. Three covers a client sending
/// several unknown-to-alc parameters at once and still costs nothing on the
/// overwhelmingly common path, where the static list above is already right.
const REFUSAL_RETRIES: usize = 3;

/// `POST /v1/responses`.
///
/// Every failure this can produce arrives in the OpenAI error envelope, which
/// is what its three clients parse. The one case that does not is an upstream
/// refusal: that body is relayed exactly as chatgpt.com wrote it, because the
/// server is the only party that knows why it said no.
pub(crate) async fn handle(State(state): State<Arc<BridgeState>>, body: Bytes) -> Response {
    match relay(&state, body).await {
        Ok(response) => response,
        Err(error) => error.openai(),
    }
}

async fn relay(state: &BridgeState, body: Bytes) -> Result<Response, BridgeError> {
    let mut request: Value = serde_json::from_slice(&body).map_err(|error| {
        BridgeError::invalid(format!("the request body is not valid JSON: {error}"))
    })?;
    let model = requested_model(&request)?;
    // Read before `normalise` overwrites it. Upstream has no non-streaming
    // mode, so the client's flag no longer selects how alc asks — only whether
    // the stream is relayed or folded back into one object.
    let streaming = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !matches!(state.upstream, super::Upstream::Codex(_)) {
        return relay_antigravity(state, &request, streaming).await;
    }
    normalise(&mut request);
    let observed = upstream::observation(
        state,
        &model,
        streaming,
        None,
        crate::usage::records::OutputBasis::Unknown,
    );

    for attempt in 0..=REFUSAL_RETRIES {
        let payload = serialise(&request, &model)?;
        let response = match upstream::post(state, payload, true, Lane::Plain).await {
            Ok(response) => response,
            Err(error) => {
                if let Some(observation) = &observed {
                    observation.finish(if error.status == StatusCode::GATEWAY_TIMEOUT {
                        Outcome::TimedOut
                    } else {
                        Outcome::Failed
                    });
                }
                return Err(error);
            }
        };
        if response.status() != StatusCode::BAD_REQUEST {
            if !response.status().is_success()
                && let Some(observation) = &observed
            {
                observation.finish(Outcome::Failed);
            }
            return Ok(if streaming {
                relay_response(response, observed)
            } else {
                fold_response(response, observed).await?
            });
        }
        // A 400 is short and already complete, so reading it costs nothing and
        // buys the chance to act on what it says. Anything else is left as a
        // stream, because a refused turn is the only kind that fits in memory.
        let status = response.status();
        let headers = passthrough_headers(response.headers());
        let refusal = response.bytes().await.unwrap_or_default();
        let named = refused_parameter(&refusal)
            .filter(|_| attempt < REFUSAL_RETRIES)
            .filter(|name| drop_parameter(&mut request, name));
        if named.is_none() {
            if let Some(observation) = &observed {
                observation.finish(Outcome::Failed);
            }
            return Ok(buffered_response(status, headers, refusal));
        }
    }
    unreachable!("the loop returns on the last attempt")
}

/// A Responses request, for Antigravity: unlike chatgpt.com, Code Assist
/// speaks nothing of the Responses API, so the body is read into the
/// bridge's internal model and translated rather than forwarded.
///
/// Only function tools travel - hosted ones (`web_search`, `file_search`)
/// have no Gemini counterpart behind this endpoint. The request's
/// `reasoning.effort` outranks the launch pin, as on the other surfaces.
pub(crate) fn antigravity_request(
    body: &Value,
    pinned: Option<ReasoningEffort>,
) -> Result<UpstreamRequest, BridgeError> {
    let model = requested_model(body)?;
    let mut input = Vec::new();
    let tools: Vec<Tool> = body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("function"))
        .filter_map(|tool| {
            Some(Tool {
                kind: "function".to_owned(),
                name: tool.get("name")?.as_str()?.to_owned(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                strict: false,
                parameters: tool
                    .get("parameters")
                    .filter(|parameters| parameters.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            })
        })
        .collect();
    if !tools.is_empty() {
        input.push(InputItem::AdditionalTools {
            role: "developer".to_owned(),
            tools,
        });
    }
    match body.get("input") {
        Some(Value::String(text)) => input.push(InputItem::Message {
            role: "user".to_owned(),
            content: vec![ContentPart::InputText { text: text.clone() }],
        }),
        Some(Value::Array(items)) => input.extend(items.iter().filter_map(input_item)),
        Some(item @ Value::Object(_)) => input.extend(input_item(item)),
        _ => return Err(BridgeError::invalid("the request has no 'input' to send")),
    }
    let effort = match body.pointer("/reasoning/effort").and_then(Value::as_str) {
        // A word alc has no rung for (`auto`) says nothing, so the pin stands.
        Some(word) => effort_word(word).or_else(|| pinned.map(Effort::from)),
        None => pinned.map(Effort::from),
    };
    Ok(UpstreamRequest {
        model,
        kind: None,
        store: false,
        stream: Some(true),
        parallel_tool_calls: false,
        reasoning: Some(Reasoning {
            effort,
            context: None,
            summary: None,
        }),
        text: TextOptions {
            verbosity: None,
            format: None,
        },
        client_metadata: None,
        instructions: body
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::to_owned),
        include: None,
        tool_choice: None,
        prompt_cache_key: None,
        input,
        max_output_tokens: body.get("max_output_tokens").and_then(Value::as_u64),
        temperature: body.get("temperature").and_then(Value::as_f64),
    })
}

/// A Responses effort word on alc's ladder; one it does not know is `None`,
/// which leaves the launch pin (or the model's default) in force rather than
/// refusing the turn.
fn effort_word(word: &str) -> Option<Effort> {
    match word.trim().to_ascii_lowercase().as_str() {
        "none" => Some(Effort::None),
        "minimal" | "low" => Some(Effort::Low),
        "medium" => Some(Effort::Medium),
        "high" => Some(Effort::High),
        "xhigh" => Some(Effort::Xhigh),
        "max" | "ultra" => Some(Effort::Max),
        _ => None,
    }
}

/// One Responses input item, or `None` for a kind Antigravity has no place
/// for (hosted tool calls, item references).
///
/// A reasoning item whose `encrypted_content` is absent or `null` is read as
/// unsigned (`""`), as [`InputItem`]'s own parse reads it; the translator
/// drops an unsigned one rather than forging a signature for it.
fn input_item(item: &Value) -> Option<InputItem> {
    let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
    match item.get("type").and_then(Value::as_str) {
        Some("message") | None => {
            let role = text("role")?;
            let assistant = role == "assistant";
            let to_part = |text: String| {
                if assistant {
                    ContentPart::OutputText { text }
                } else {
                    ContentPart::InputText { text }
                }
            };
            let content: Vec<ContentPart> = match item.get("content") {
                Some(Value::String(body)) => vec![to_part(body.clone())],
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                        Some("input_text" | "output_text" | "text") => part
                            .get("text")
                            .and_then(Value::as_str)
                            .map(|text| to_part(text.to_owned())),
                        Some("input_image") => part
                            .get("image_url")
                            .and_then(|url| url.as_str().or_else(|| url.get("url")?.as_str()))
                            .map(|url| ContentPart::InputImage {
                                image_url: url.to_owned(),
                                detail: None,
                            }),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            (!content.is_empty()).then_some(InputItem::Message { role, content })
        }
        Some("function_call") => Some(InputItem::FunctionCall {
            id: None,
            call_id: text("call_id")?,
            name: text("name")?,
            arguments: text("arguments").unwrap_or_else(|| "{}".to_owned()),
        }),
        Some("function_call_output") => {
            let output = match item.get("output") {
                Some(Value::String(body)) => body.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            Some(InputItem::FunctionCallOutput {
                call_id: text("call_id")?,
                output: FunctionCallOutput::Text(output),
            })
        }
        Some("reasoning") => Some(InputItem::Reasoning {
            id: text("id").unwrap_or_else(|| "rs".to_owned()),
            summary: item
                .get("summary")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            encrypted_content: text("encrypted_content").unwrap_or_default(),
        }),
        _ => None,
    }
}

/// The Antigravity half of [`relay`]: translate, send, then relay or fold.
///
/// The wire id is resolved here, before the ledger row is opened, so the row
/// names the model that actually ran (`gemini-3.8-flash-low`, not the
/// family the client asked for); [`upstream::send_with_affinity`] resolving
/// it again is a no-op on a wire id.
async fn relay_antigravity(
    state: &BridgeState,
    body: &Value,
    streaming: bool,
) -> Result<Response, BridgeError> {
    let mut request = antigravity_request(body, state.config.effort)?;
    if matches!(state.upstream, super::Upstream::Antigravity(_)) {
        request.model = super::gemini::wire_model(
            &request.model,
            request
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.effort),
            state.config.claude_tiers.as_ref(),
        )?;
    }
    let observed = upstream::observation(
        state,
        &request.model,
        streaming,
        None,
        crate::usage::records::OutputBasis::Unknown,
    );
    let stream = upstream::send_with_affinity(state, request, None, observed).await?;
    if streaming {
        Ok(stream_antigravity(stream))
    } else {
        fold_antigravity(stream).await
    }
}

/// The translated events as Responses SSE, each under its own `event:` name
/// and stamped with a `sequence_number` counting from 0, as chatgpt.com's
/// frames are.
///
/// The stamp is not decoration: `@ai-sdk/openai` (OpenCode's Responses
/// client) validates each chunk against a schema that requires it, and a
/// chunk that fails is ignored rather than refused - an `error` event without
/// one would vanish. For the same reader, a `response.failed` (a `SAFETY` or
/// `RECITATION` stop) is preceded by an `error` event carrying its code and
/// message: that client finishes on `completed`/`incomplete` and reports
/// `error`, so a stop told only as `response.failed` could show as an empty
/// turn. A failure after the first frame can only be told in the stream
/// itself, as that same `error` event; nothing is retried by then (see
/// [`super::gemini_stream::AgyStream`]).
fn stream_antigravity(stream: UpstreamStream) -> Response {
    let relay = SequencedRelay {
        stream: Some(stream),
        sequence: 0,
        queued: std::collections::VecDeque::new(),
    };
    let body = futures_util::stream::unfold(relay, |mut relay| async move {
        let frame = relay.next().await?;
        Some((Ok::<_, Infallible>(frame), relay))
    });
    let mut response = Response::new(Body::from_stream(body));
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/event-stream"),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    response
}

/// [`stream_antigravity`]'s state: the turn, the next sequence number, and
/// the frames already written but not yet handed out.
struct SequencedRelay {
    /// `None` once the turn has ended, in success or failure.
    stream: Option<UpstreamStream>,
    sequence: u64,
    queued: std::collections::VecDeque<String>,
}

impl SequencedRelay {
    async fn next(&mut self) -> Option<String> {
        loop {
            if let Some(frame) = self.queued.pop_front() {
                return Some(frame);
            }
            let stream = self.stream.as_mut()?;
            match stream.next().await {
                Some(Ok(frame)) => match serde_json::from_str::<Value>(&frame.raw) {
                    Ok(event) if event.is_object() => {
                        if event.get("type").and_then(Value::as_str) == Some("response.failed") {
                            let error = event.pointer("/response/error");
                            let read = |key: &str| {
                                error
                                    .and_then(|error| error.get(key))
                                    .cloned()
                                    .unwrap_or(Value::Null)
                            };
                            self.push(json!({"type": "error", "code": read("code"),
                                "message": read("message"), "param": null}));
                        }
                        self.push(event);
                    }
                    // Unreachable: every frame is built from a JSON object.
                    _ => self.fail("alc relayed an Antigravity event it could not re-read"),
                },
                Some(Err(error)) => {
                    let event = json!({"type": "error", "code": error.kind,
                        "message": error.message, "param": null});
                    self.push(event);
                    self.stream = None;
                }
                None => self.stream = None,
            }
        }
    }

    fn push(&mut self, mut event: Value) {
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message")
            .to_owned();
        event["sequence_number"] = json!(self.sequence);
        self.sequence += 1;
        self.queued
            .push_back(format!("event: {kind}\ndata: {event}\n\n"));
    }

    fn fail(&mut self, message: &str) {
        self.push(json!({"type": "error", "code": "api_error", "message": message, "param": null}));
        self.stream = None;
    }
}

/// The same events folded into one Responses object, as [`fold_response`]
/// does for chatgpt.com: a failed or incomplete turn is that object with its
/// `status` and `error` saying so, not an empty answer.
async fn fold_antigravity(mut stream: UpstreamStream) -> Result<Response, BridgeError> {
    let mut output = Vec::new();
    let mut completed = None;
    while let Some(frame) = stream.next().await {
        collect_frame(&frame?.raw, &mut output, &mut completed);
        if completed.is_some() {
            break;
        }
    }
    let Some(mut response) = completed else {
        return Err(BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            "Antigravity ended the stream without completing the response",
        ));
    };
    if let Some(object) = response.as_object_mut() {
        object.insert("output".to_owned(), Value::Array(output));
    }
    Ok(axum::Json(response).into_response())
}

fn serialise(request: &Value, model: &str) -> Result<Bytes, BridgeError> {
    serde_json::to_vec(request)
        .map(Bytes::from)
        .map_err(|error| {
            BridgeError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                format!("could not re-serialise the request for {model}: {error}"),
            )
        })
}

/// Extracts the model a passthrough body is asking for.
///
/// The only two things this surface refuses locally. A body that is not an
/// object has no request in it to forward, and a body with no model routes
/// nowhere — upstream's own complaint about that one is generic enough to send
/// its reader looking in the wrong place. There is no allowlist beyond this:
/// which slugs exist is upstream's to know, and a stale local copy of that
/// list is the reason this module exists.
pub(crate) fn requested_model(body: &Value) -> Result<String, BridgeError> {
    let object = body
        .as_object()
        .ok_or_else(|| BridgeError::invalid("the request body must be a JSON object"))?;
    match object.get("model") {
        Some(Value::String(model)) if !model.trim().is_empty() => Ok(model.clone()),
        Some(Value::String(_)) => Err(BridgeError::invalid("'model' is empty")),
        Some(_) => Err(BridgeError::invalid("'model' must be a string")),
        None => Err(BridgeError::invalid("the request body has no 'model'")),
    }
}

/// Applies the repairs the Codex endpoint requires and nothing else.
///
/// `store` is overwritten rather than defaulted: `true` is the public API's
/// default and the only value this endpoint refuses, so a client that spelled
/// it out is the one most in need of the correction.
///
/// A string `input` becomes the item shape the sibling capture sent and the
/// server did not object to — `role` plus one `input_text` part, with no
/// `type` on the item itself. That is a transcription, not a guess: adding the
/// `type: "message"` the public API documents would make this body differ from
/// the one thing in the corpus that got past the input check.
///
/// `stream` is forced `true` for the same reason `store` is forced `false`:
/// `400 {"detail":"Stream must be set to true"}` is the whole of upstream's
/// non-streaming mode. A client that did not ask to stream gets its single
/// object from [`fold_response`] instead.
///
/// Anything else — a missing `input`, a null, an array already — is left for
/// upstream to judge. Repairs whose correct form has not been observed are
/// worse than a clear refusal from the party that decides.
fn normalise(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    object.insert("store".to_owned(), Value::Bool(false));
    object.insert("stream".to_owned(), Value::Bool(true));
    for refused in REFUSED_PARAMETERS {
        object.remove(*refused);
    }
    let listed = match object.get("input") {
        Some(Value::String(text)) => Some(json!([{
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        }])),
        Some(item @ Value::Object(_)) => Some(Value::Array(vec![item.clone()])),
        _ => None,
    };
    if let Some(listed) = listed {
        object.insert("input".to_owned(), listed);
    }
}

/// The parameter a refusal names, if it names one this file could remove.
///
/// Both spellings the endpoint uses are read: the bare
/// `{"detail":"Unsupported parameter: max_output_tokens"}` and the OpenAI
/// envelope's `param` field. A dotted path is ignored rather than guessed at —
/// `reasoning.context` arrives that way when the *value* is wrong, and
/// deleting `reasoning` over a complaint about one of its members would answer
/// a refusal with a different request rather than a corrected one.
fn refused_parameter(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let named = value
        .get("detail")
        .and_then(Value::as_str)
        .and_then(|detail| detail.strip_prefix("Unsupported parameter: "))
        .map(str::to_owned)
        .or_else(|| {
            let error = value.get("error")?;
            let code = error.get("code").and_then(Value::as_str);
            matches!(code, Some("unknown_parameter" | "unsupported_parameter"))
                .then(|| error.get("param")?.as_str().map(str::to_owned))
                .flatten()
        })?;
    let named = named.trim().trim_matches('\'').to_owned();
    (!named.is_empty() && !named.contains('.')).then_some(named)
}

/// Removes a top-level parameter, reporting whether there was one to remove.
///
/// The answer is what stops the retry loop: a refusal naming something alc is
/// not sending cannot be fixed by sending less, and must reach the client.
fn drop_parameter(request: &mut Value, name: &str) -> bool {
    request
        .as_object_mut()
        .is_some_and(|object| object.remove(name).is_some())
}

/// Hands the upstream answer to the client with its status, its content type
/// and its bytes intact.
fn relay_response(upstream: reqwest::Response, observed: Option<RequestObservation>) -> Response {
    let status = upstream.status();
    let headers = passthrough_headers(upstream.headers());
    let mut response = Response::new(Body::from_stream(observe(
        guard_idle(upstream.bytes_stream()),
        observed,
    )));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

/// Folds a streamed turn back into the single Responses object a client that
/// did not ask to stream is waiting for.
///
/// Upstream has no non-streaming mode to relay: `stream: false` comes back as
/// `400 {"detail":"Stream must be set to true"}` (measured 2026-09-09), which
/// is why [`normalise`] sets the flag whatever the client said.
///
/// The fold is not a copy of the last event either. `response.completed`
/// carries the whole response object — model, usage, status, every parameter
/// the server resolved — but its `output` array arrives empty, with the actual
/// items delivered one at a time as `response.output_item.done`. So the items
/// are collected in arrival order and spliced back in, which is the only place
/// this function invents anything.
async fn fold_response(
    upstream: reqwest::Response,
    observed: Option<RequestObservation>,
) -> Result<Response, BridgeError> {
    let headers = passthrough_headers(upstream.headers());
    let mut body = Box::pin(upstream.bytes_stream());
    let mut decoder = upstream::SseDecoder::default();
    let mut output = Vec::new();
    let mut completed = None;
    let mut observation = observed.map(StreamObservation::new);

    loop {
        let chunk = match tokio::time::timeout(upstream::BODY_IDLE_TIMEOUT, body.next()).await {
            Ok(Some(Ok(chunk))) => Some(chunk),
            Ok(Some(Err(error))) => {
                if let Some(observation) = &observation {
                    observation.request.finish(Outcome::Truncated);
                }
                return Err(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    format!("the Codex response body failed mid-stream: {error}"),
                ));
            }
            Ok(None) => None,
            Err(_) => {
                if let Some(observation) = &observation {
                    observation.request.finish(Outcome::TimedOut);
                }
                return Err(BridgeError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "api_error",
                    format!(
                        "chatgpt.com sent nothing for {}s; giving up on the response body",
                        upstream::BODY_IDLE_TIMEOUT.as_secs()
                    ),
                ));
            }
        };
        if let Some(observation) = &mut observation {
            match &chunk {
                Some(chunk) => observation.push(chunk),
                None => observation.eof(),
            }
        }
        let frames = match &chunk {
            Some(chunk) => decoder.push(chunk),
            None => decoder.finish(),
        };
        for frame in frames {
            collect_frame(&frame.data, &mut output, &mut completed);
        }
        if chunk.is_none() || completed.is_some() {
            break;
        }
    }

    let Some(mut response) = completed else {
        return Err(BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            "chatgpt.com ended the stream without completing the response",
        ));
    };
    if let Some(object) = response.as_object_mut() {
        object.insert("output".to_owned(), Value::Array(output));
    }
    let body = serde_json::to_vec(&response).map_err(|error| {
        BridgeError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            format!("could not re-serialise the completed Codex response: {error}"),
        )
    })?;
    let mut headers = headers;
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    Ok(buffered_response(
        StatusCode::OK,
        headers,
        Bytes::from(body),
    ))
}

/// Reads one frame into the fold.
///
/// `response.failed` and `response.incomplete` are terminal too and carry the
/// same object, so a refused or truncated turn comes back as the server
/// described it rather than as "the stream ended".
fn collect_frame(data: &str, output: &mut Vec<Value>, completed: &mut Option<Value>) {
    if data.trim() == "[DONE]" {
        return;
    }
    let Ok(event) = serde_json::from_str::<Value>(data) else {
        return;
    };
    match event.get("type").and_then(Value::as_str) {
        Some("response.output_item.done") => {
            if let Some(item) = event.get("item") {
                output.push(item.clone());
            }
        }
        Some("response.completed" | "response.failed" | "response.incomplete") => {
            if let Some(response) = event.get("response") {
                *completed = Some(response.clone());
            }
        }
        _ => {}
    }
}

/// The same relay for an answer already read into memory, which is how a
/// refusal arrives once it has been inspected for a parameter to drop.
fn buffered_response(status: StatusCode, headers: HeaderMap, body: Bytes) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

/// An allowlist rather than a copy, because a handful of upstream headers
/// describe a message this bridge is re-framing: `content-length` and
/// `content-encoding` belong to bytes hyper will re-count, `transfer-encoding`
/// is hop-by-hop, and `set-cookie` would hand the agent chatgpt.com's session
/// cookie for a host it is not talking to.
fn passthrough_headers(upstream: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in upstream {
        if relayable(name) {
            headers.append(name.clone(), value.clone());
        }
    }
    headers
}

fn relayable(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "content-type"
            | "cache-control"
            | "retry-after"
            | "x-request-id"
            | "openai-processing-ms"
            | "openai-version"
    ) || name.as_str().starts_with("x-ratelimit-")
}

/// Wraps the upstream body so a stalled connection ends as a read error the
/// client can see, instead of a stream that hangs until the agent gives up.
///
/// The error travels in the body, after the status line is already sent, which
/// is the only channel left once a relay has started — and why the timeout
/// message says what it was waiting for.
/// Shows every frame to the ledger on its way to the client.
///
/// This surface relays bytes rather than decoding them, so it is the one place
/// the shared `UpstreamStream` hook cannot reach. The stream is not buffered:
/// each chunk is handed on as it arrives, and the decoder only keeps whatever
/// partial frame is in flight.
fn observe(
    body: impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    observed: Option<RequestObservation>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    futures_util::stream::unfold(
        (Box::pin(body), observed.map(StreamObservation::new)),
        |(mut body, mut observation)| async move {
            match body.next().await {
                Some(item) => {
                    if let Some(observation) = &mut observation {
                        match &item {
                            Ok(chunk) => observation.push(chunk),
                            Err(error) => observation.request.finish(
                                if error.kind() == std::io::ErrorKind::TimedOut {
                                    Outcome::TimedOut
                                } else {
                                    Outcome::Truncated
                                },
                            ),
                        }
                    }
                    Some((item, (body, observation)))
                }
                None => {
                    if let Some(observation) = &mut observation {
                        observation.eof();
                    }
                    None
                }
            }
        },
    )
}

fn guard_idle(
    body: impl Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    futures_util::stream::unfold(Some(Box::pin(body)), |state| async move {
        let mut body = state?;
        match tokio::time::timeout(upstream::BODY_IDLE_TIMEOUT, body.next()).await {
            Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some(body))),
            Ok(Some(Err(error))) => Some((
                Err(std::io::Error::other(format!(
                    "the Codex response body failed mid-stream: {error}"
                ))),
                None,
            )),
            Ok(None) => None,
            Err(_) => Some((
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "chatgpt.com sent nothing for {}s; giving up on the response body",
                        upstream::BODY_IDLE_TIMEOUT.as_secs()
                    ),
                )),
                None,
            )),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeConfig;
    use crate::bridge::auth::Credentials;
    use crate::bridge::fixtures::Case;
    use crate::bridge::upstream::{RESPONSES_LITE_HEADER, request_headers};
    use axum::http::{HeaderValue, header};

    fn normalised(case: &str) -> Value {
        let mut body = Case::load(case).client_request();
        normalise(&mut body);
        body
    }

    fn credentials() -> Credentials {
        Credentials {
            access_token: "access-token".to_owned(),
            refresh_token: "refresh-token".to_owned(),
            account_id: Some("account-id".to_owned()),
            expires_at_ms: 0,
        }
    }

    #[test]
    fn the_two_captured_refusals_normalise_onto_one_body() {
        let from_list = normalised("responses-passthrough-input-list");
        let from_string = normalised("responses-passthrough-input-string");
        assert_eq!(from_list, from_string);
        assert_eq!(from_list["store"], false);
        assert_eq!(from_list["model"], "gpt-5.6-terra");
        assert_eq!(
            from_list["input"][0]["content"][0]["text"],
            "Reply with exactly: responses-ok"
        );
    }

    /// The proof that nothing else is injected: what goes up is the captured
    /// upstream request plus exactly the two flags the endpoint demands, and
    /// no `client_metadata`, `reasoning`, `instructions` or rewritten model.
    #[test]
    fn a_list_body_goes_up_as_captured_apart_from_the_two_flags() {
        let mut expected = Case::load("responses-passthrough-input-list").upstream_request();
        expected["store"] = Value::Bool(false);
        expected["stream"] = Value::Bool(true);
        assert_eq!(normalised("responses-passthrough-input-list"), expected);
    }

    #[test]
    fn a_string_input_becomes_the_item_shape_the_sibling_capture_sent() {
        let captured = Case::load("responses-passthrough-input-list").client_request();
        assert_eq!(
            normalised("responses-passthrough-input-string")["input"],
            captured["input"]
        );
    }

    #[test]
    fn a_client_that_asked_to_store_the_turn_is_overruled_rather_than_refused() {
        let mut body = json!({ "model": "gpt-5.6-terra", "store": true, "input": [] });
        normalise(&mut body);
        assert_eq!(body["store"], false);
    }

    #[test]
    fn nothing_the_client_did_not_send_is_added_to_the_body() {
        let mut body = json!({ "model": "gpt-5.6-terra", "input": [], "verbosity": "high" });
        normalise(&mut body);
        assert_eq!(
            body,
            json!({
                "model": "gpt-5.6-terra",
                "input": [],
                "verbosity": "high",
                "store": false,
                "stream": true,
            })
        );
    }

    /// OpenCode's every turn carries `max_output_tokens`, and the endpoint
    /// answers `400 {"detail":"Unsupported parameter: max_output_tokens"}`.
    /// Before this, the surface had no working client.
    #[test]
    fn the_parameters_this_endpoint_refuses_are_dropped_rather_than_relayed() {
        let mut body = json!({
            "model": "gpt-6-astra",
            "input": [],
            "max_output_tokens": 4096,
            "temperature": 0.7,
            "top_p": 0.9,
            "user": "someone",
            "metadata": { "run": "1" },
            "stream_options": { "include_usage": true },
            "instructions": "Be terse.",
            "tools": [],
        });
        normalise(&mut body);
        assert_eq!(
            body,
            json!({
                "model": "gpt-6-astra",
                "input": [],
                "store": false,
                "stream": true,
                // Both measured as accepted, so both still travel.
                "instructions": "Be terse.",
                "tools": [],
            })
        );
    }

    /// The fold's whole job, in the shape upstream actually sends it:
    /// `response.completed` carries the object but an empty `output`, and the
    /// items arrive one `response.output_item.done` at a time.
    #[test]
    fn the_folded_answer_is_the_completed_object_with_its_items_put_back() {
        let mut output = Vec::new();
        let mut completed = None;
        for data in [
            r#"{"type":"response.created","response":{"id":"resp_1","output":[]}}"#,
            r#"{"type":"response.output_text.delta","delta":"probe"}"#,
            r#"{"type":"response.output_item.done","item":{"id":"msg_1","type":"message"}}"#,
            r#"{"type":"response.output_item.done","item":{"id":"fc_1","type":"function_call"}}"#,
            r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[],"usage":{"input_tokens":4}}}"#,
            "[DONE]",
        ] {
            collect_frame(data, &mut output, &mut completed);
        }
        let mut response = completed.expect("the stream completed");
        response["output"] = Value::Array(output);
        assert_eq!(
            response,
            json!({
                "id": "resp_1",
                "status": "completed",
                "usage": { "input_tokens": 4 },
                "output": [
                    { "id": "msg_1", "type": "message" },
                    { "id": "fc_1", "type": "function_call" },
                ],
            })
        );
    }

    /// A turn upstream refused or cut short is terminal too, and its object is
    /// the honest answer — better than reporting that the stream just ended.
    #[test]
    fn a_failed_or_truncated_turn_still_ends_the_fold() {
        for kind in ["response.failed", "response.incomplete"] {
            let mut output = Vec::new();
            let mut completed = None;
            collect_frame(
                &format!(r#"{{"type":"{kind}","response":{{"status":"failed"}}}}"#),
                &mut output,
                &mut completed,
            );
            assert_eq!(completed, Some(json!({ "status": "failed" })), "{kind}");
        }
    }

    #[test]
    fn every_refused_parameter_is_named_once_and_stays_sorted() {
        let mut sorted = REFUSED_PARAMETERS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, REFUSED_PARAMETERS);
    }

    /// The list above will go stale, so the server's own naming of a
    /// parameter is read too. Both spellings it uses are covered.
    #[test]
    fn a_refusal_that_names_a_parameter_is_read_in_either_spelling() {
        assert_eq!(
            refused_parameter(br#"{"detail":"Unsupported parameter: seed"}"#).as_deref(),
            Some("seed")
        );
        assert_eq!(
            refused_parameter(
                br#"{"error":{"message":"Unknown parameter: 'seed'.","code":"unknown_parameter","param":"seed"}}"#
            )
            .as_deref(),
            Some("seed")
        );
    }

    /// A dotted `param` is a complaint about a member, and `reasoning.context`
    /// is the one that actually arrives that way: deleting `reasoning` in
    /// answer to it would send a different request rather than a fixed one.
    #[test]
    fn a_refusal_about_a_nested_field_is_left_for_the_client_to_see() {
        assert!(
            refused_parameter(
                br#"{"error":{"message":"requires `reasoning.context` to be `all_turns`.","code":"unsupported_value","param":"reasoning.context"}}"#
            )
            .is_none()
        );
        assert!(refused_parameter(br#"{"detail":"Store must be set to false"}"#).is_none());
        assert!(refused_parameter(b"not json at all").is_none());
    }

    /// What ends the retry loop. A refusal naming something alc is not
    /// sending cannot be answered by sending less, so it must reach the
    /// client rather than cost another round trip.
    #[test]
    fn dropping_a_parameter_the_request_never_had_reports_no_progress() {
        let mut body = json!({ "model": "gpt-6-astra", "seed": 7 });
        assert!(drop_parameter(&mut body, "seed"));
        assert!(!drop_parameter(&mut body, "seed"));
        assert_eq!(body, json!({ "model": "gpt-6-astra" }));
    }

    #[test]
    fn an_input_that_is_already_a_list_is_left_exactly_as_it_arrived() {
        let mut body = json!({
            "model": "gpt-5.6-terra",
            "input": [{ "role": "user", "content": "hi", "type": "message" }],
        });
        normalise(&mut body);
        assert_eq!(
            body["input"],
            json!([{ "role": "user", "content": "hi", "type": "message" }])
        );
    }

    #[test]
    fn a_lone_input_item_is_wrapped_rather_than_rewritten() {
        let item = json!({ "role": "user", "content": [{ "type": "input_text", "text": "hi" }] });
        let mut body = json!({ "model": "gpt-5.6-terra", "input": item.clone() });
        normalise(&mut body);
        assert_eq!(body["input"], json!([item]));
    }

    #[test]
    fn a_body_with_no_model_is_refused_by_name() {
        let error = requested_model(&json!({ "input": [] })).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("'model'"), "{}", error.message);
    }

    #[test]
    fn a_body_that_is_not_an_object_is_refused_before_upstream() {
        let error = requested_model(&json!(["gpt-5.6-terra"])).unwrap_err();
        assert!(error.message.contains("JSON object"), "{}", error.message);
    }

    #[test]
    fn an_empty_or_mistyped_model_is_refused_too() {
        assert!(requested_model(&json!({ "model": "  " })).is_err());
        assert!(requested_model(&json!({ "model": 7 })).is_err());
    }

    #[test]
    fn a_model_that_no_allowlist_knows_is_still_forwarded() {
        assert_eq!(
            requested_model(&json!({ "model": "gpt-6-astra" })).unwrap(),
            "gpt-6-astra"
        );
    }

    #[test]
    fn the_stream_flag_picks_the_accept_header() {
        let streaming = request_headers(&credentials(), true, Lane::Plain).unwrap();
        assert_eq!(streaming[header::ACCEPT], "text/event-stream");
        let buffered = request_headers(&credentials(), false, Lane::Plain).unwrap();
        assert_eq!(buffered[header::ACCEPT], "application/json");
    }

    /// The capture is evidence of what the bridge being replaced *sent*, and
    /// that request was refused. So this surface matches it header for header
    /// with exactly one subtraction, and the subtraction is the point: the
    /// lite header obliges the body to carry `reasoning.context` and
    /// `parallel_tool_calls`, which a body alc is only carrying cannot
    /// promise.
    #[test]
    fn the_upstream_headers_match_the_captured_set_less_the_lite_one() {
        let headers = request_headers(&credentials(), false, Lane::Plain).unwrap();
        let captured = Case::load("responses-passthrough-input-list").upstream_metadata();
        let captured = captured["headers"].as_object().unwrap();
        let mut sent: Vec<&str> = headers.keys().map(HeaderName::as_str).collect();
        sent.sort_unstable();
        let mut expected: Vec<&str> = captured
            .keys()
            .map(String::as_str)
            .filter(|name| *name != RESPONSES_LITE_HEADER)
            .collect();
        expected.sort_unstable();
        assert_eq!(sent, expected);
        assert!(captured.contains_key(RESPONSES_LITE_HEADER));
        for (name, value) in captured {
            // The capture redacted the two credential headers; every other
            // value is a constant this file has to reproduce exactly.
            if name == "authorization"
                || name == "chatgpt-account-id"
                || name == RESPONSES_LITE_HEADER
            {
                continue;
            }
            assert_eq!(headers[name.as_str()], value.as_str().unwrap(), "{name}");
        }
    }

    #[test]
    fn credentials_without_an_account_id_fail_as_an_auth_error() {
        let mut credentials = credentials();
        credentials.account_id = None;
        let error = request_headers(&credentials, false, Lane::Plain).unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert!(error.message.contains("codex login"), "{}", error.message);
    }

    #[test]
    fn the_upstreams_own_headers_are_relayed_but_its_session_is_not() {
        let mut upstream = HeaderMap::new();
        upstream.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        upstream.insert(
            header::SET_COOKIE,
            HeaderValue::from_static("session=secret"),
        );
        upstream.insert(header::CONTENT_LENGTH, HeaderValue::from_static("39"));
        upstream.insert(
            HeaderName::from_static("x-ratelimit-remaining-requests"),
            HeaderValue::from_static("42"),
        );
        let relayed = passthrough_headers(&upstream);
        assert_eq!(relayed[header::CONTENT_TYPE], "text/event-stream");
        assert_eq!(relayed["x-ratelimit-remaining-requests"], "42");
        assert!(relayed.get(header::SET_COOKIE).is_none());
        assert!(relayed.get(header::CONTENT_LENGTH).is_none());
    }

    #[tokio::test]
    async fn an_unreadable_body_is_refused_in_the_openai_envelope() {
        let state = Arc::new(
            BridgeState::new(BridgeConfig {
                auth_file: std::path::PathBuf::from("/nonexistent/auth.json"),
                effort: None,
                responses_api: true,
                agent: crate::config::Agent::Claude,
                provider: "codex".to_owned(),
                ledger: None,
                code_assist: None,
                claude_tiers: None,
                upstream: crate::bridge::UpstreamKind::Codex,
            })
            .expect("the state builds without touching the auth file"),
        );
        let response = handle(State(state), Bytes::from_static(b"{not json")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not valid JSON"),
            "{body}"
        );
    }

    use crate::bridge::agy_fake::{FakeCodeAssist, Reply, agy_bridge, post_json};
    use crate::bridge::upstream::InputItem;

    const CALL: &str = "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"Paris\"},\"id\":\"call-1\"},\"thoughtSignature\":\"S2\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":9,\"candidatesTokenCount\":4,\"totalTokenCount\":13}}}\n\n";
    const DONE: &str = "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Sunny.\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":12,\"candidatesTokenCount\":2,\"totalTokenCount\":14}}}\n\n";

    fn project() -> Reply {
        Reply::json(
            200,
            serde_json::json!({"cloudaicompanionProject": "test-project"}),
        )
    }

    fn tools() -> Value {
        serde_json::json!([{"type": "function", "name": "get_weather", "description": "Weather",
            "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}},
            {"type": "web_search"}])
    }

    fn generated(fake: &FakeCodeAssist) -> Vec<Value> {
        fake.requests()
            .into_iter()
            .filter(|request| request.path.contains("streamGenerate"))
            .map(|request| request.body)
            .collect()
    }

    /// The streamed body's frames as (event name, payload). Asserts what every
    /// Responses client may rely on: each payload's `type` is its event name,
    /// and `sequence_number` is on every frame, counting up from 0.
    fn frames(body: &str) -> Vec<(String, Value)> {
        let frames: Vec<(String, Value)> = upstream::parse_sse(body)
            .into_iter()
            .map(|frame| {
                let payload: Value = serde_json::from_str(&frame.data).unwrap();
                (frame.event.unwrap_or_default(), payload)
            })
            .collect();
        assert!(!frames.is_empty(), "no frames in {body}");
        for (index, (event, payload)) in frames.iter().enumerate() {
            assert_eq!(payload["type"], event.as_str(), "{body}");
            assert_eq!(payload["sequence_number"], index as u64, "{body}");
        }
        frames
    }

    fn names(frames: &[(String, Value)]) -> Vec<&str> {
        frames.iter().map(|(event, _)| event.as_str()).collect()
    }

    #[tokio::test]
    async fn a_responses_client_runs_a_signed_tool_round_on_antigravity() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(CALL), Reply::sse(DONE)]);
        let state = agy_bridge(&fake, None, None);
        let first = serde_json::json!({"model": "gemini-3.8-flash", "stream": true, "tools": tools(),
            "reasoning": {"effort": "low"}, "input": "Weather in Paris?"});
        let (status, body) = post_json(Arc::clone(&state), "/v1/responses", first).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("event: response.output_item.done"), "{body}");
        assert!(
            body.contains("\"encrypted_content\":\"agy1:gemini-3.8-flash-low:S2\""),
            "{body}"
        );
        assert!(body.contains("event: response.completed"), "{body}");
        let streamed = frames(&body);
        assert_eq!(names(&streamed).last(), Some(&"response.completed"));

        let second = serde_json::json!({"model": "gemini-3.8-flash", "stream": false, "tools": tools(),
            "reasoning": {"effort": "low"}, "input": [
                {"role": "user", "content": [{"type": "input_text", "text": "Weather in Paris?"}]},
                {"type": "reasoning", "id": "rs_0", "summary": [], "encrypted_content": "agy1:gemini-3.8-flash-low:S2"},
                {"type": "function_call", "call_id": "call-1", "name": "get_weather", "arguments": "{\"city\":\"Paris\"}"},
                {"type": "function_call_output", "call_id": "call-1", "output": "Sunny, 21 C"}]});
        let (status, body) = post_json(state, "/v1/responses", second).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let folded: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(folded["status"], "completed");
        assert_eq!(folded["output"][0]["content"][0]["text"], "Sunny.");
        let sent = generated(&fake).into_iter().nth(1).unwrap();
        assert_eq!(sent["model"], "gemini-3.8-flash-low");
        assert_eq!(
            sent["request"]["contents"][1]["parts"][0]["thoughtSignature"],
            "S2"
        );
        assert_eq!(
            sent["request"]["tools"][0]["functionDeclarations"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "non-function tools are not sent"
        );
    }

    #[tokio::test]
    async fn a_client_that_drops_reasoning_items_still_replays_through_the_lru() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(CALL), Reply::sse(DONE)]);
        let state = agy_bridge(&fake, None, None);
        let first = serde_json::json!({"model": "gemini-3.8-flash-low", "tools": tools(), "input": "Weather in Paris?"});
        post_json(Arc::clone(&state), "/v1/responses", first).await;
        let second = serde_json::json!({"model": "gemini-3.8-flash-low", "tools": tools(), "input": [
            {"role": "user", "content": "Weather in Paris?"},
            {"type": "function_call", "call_id": "call-1", "name": "get_weather", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call-1", "output": "Sunny"}]});
        let (status, body) = post_json(state, "/v1/responses", second).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let sent = generated(&fake).into_iter().nth(1).unwrap();
        assert_eq!(
            sent["request"]["contents"][1]["parts"][0]["thoughtSignature"],
            "S2"
        );
    }

    #[tokio::test]
    async fn effort_and_output_cap_reach_the_wire_and_a_pin_applies_when_none_is_asked() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(DONE)]);
        let asked = serde_json::json!({"model": "gemini-3.8-flash", "reasoning": {"effort": "xhigh"},
            "max_output_tokens": 500, "temperature": 0.3, "instructions": "Be brief.", "input": "hi"});
        post_json(agy_bridge(&fake, None, None), "/v1/responses", asked).await;
        let pinned = serde_json::json!({"model": "gemini-3.8-flash", "input": "hi"});
        post_json(
            agy_bridge(&fake, None, Some(crate::config::ReasoningEffort::Low)),
            "/v1/responses",
            pinned,
        )
        .await;
        let sent = generated(&fake);
        assert_eq!(sent[0]["model"], "gemini-3.8-flash-high");
        assert_eq!(
            sent[0]["request"]["generationConfig"]["maxOutputTokens"],
            500
        );
        assert_eq!(sent[0]["request"]["generationConfig"]["temperature"], 0.3);
        assert_eq!(
            sent[0]["request"]["systemInstruction"]["parts"][0]["text"],
            "Be brief."
        );
        assert_eq!(sent[1]["model"], "gemini-3.8-flash-low");
    }

    #[tokio::test]
    async fn an_effort_word_alc_does_not_know_leaves_the_launch_pin_in_force() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(DONE)]);
        let asked = serde_json::json!({"model": "gemini-3.8-flash", "reasoning": {"effort": "auto"},
            "input": "hi"});
        let (status, body) = post_json(
            agy_bridge(&fake, None, Some(crate::config::ReasoningEffort::High)),
            "/v1/responses",
            asked,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(generated(&fake)[0]["model"], "gemini-3.8-flash-high");
    }

    #[tokio::test]
    async fn a_stopped_stream_says_error_before_response_failed_so_either_reader_sees_it() {
        for (reason, words) in [
            ("RECITATION", "recited protected text"),
            ("SAFETY", "safety filter"),
        ] {
            let stopped = format!(
                "data: {{\"response\":{{\"candidates\":[{{\"content\":{{\"role\":\"model\",\"parts\":[{{\"text\":\"x\"}}]}},\"finishReason\":\"{reason}\"}}],\"usageMetadata\":{{}}}}}}\n\n"
            );
            let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(&stopped)]);
            let (status, body) = post_json(
                agy_bridge(&fake, None, None),
                "/v1/responses",
                serde_json::json!({"model": "gemini-3.8-flash", "stream": true, "input": "hi"}),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let streamed = frames(&body);
            let names = names(&streamed);
            assert_eq!(
                &names[names.len() - 2..],
                ["error", "response.failed"],
                "{body}"
            );
            let (_, error) = &streamed[streamed.len() - 2];
            assert_eq!(error["code"], reason, "{body}");
            assert!(error["param"].is_null(), "{body}");
            assert!(error["message"].as_str().unwrap().contains(words), "{body}");
            let (_, failed) = streamed.last().unwrap();
            assert_eq!(failed["response"]["error"]["code"], reason, "{body}");
        }
    }

    #[tokio::test]
    async fn a_folded_turn_that_safety_stopped_comes_back_failed_and_saying_why() {
        let stopped = "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"\"}]},\"finishReason\":\"SAFETY\"}],\"usageMetadata\":{}}}\n\n";
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(stopped)]);
        let (status, body) = post_json(
            agy_bridge(&fake, None, None),
            "/v1/responses",
            serde_json::json!({"model": "gemini-3.8-flash", "input": "hi"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let folded: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(folded["status"], "failed", "{body}");
        assert_eq!(folded["error"]["code"], "SAFETY", "{body}");
        assert!(
            folded["error"]["message"]
                .as_str()
                .unwrap()
                .contains("safety filter"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_stream_cut_off_after_its_first_frame_ends_with_an_error_event_not_a_retry() {
        let partial = "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Sun\"}]}}]}}\n\n";
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(partial).truncated()]);
        let (status, body) = post_json(
            agy_bridge(&fake, None, None),
            "/v1/responses",
            serde_json::json!({"model": "gemini-3.8-flash", "stream": true, "input": "hi"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("event: response.output_text.delta"), "{body}");
        let streamed = frames(&body);
        assert_eq!(names(&streamed).last(), Some(&"error"), "{body}");
        assert!(!body.contains("event: response.completed"), "{body}");
        assert_eq!(
            fake.count("streamGenerateContent"),
            1,
            "no retry after output"
        );
    }

    #[tokio::test]
    async fn a_model_antigravity_cannot_serve_is_refused_in_the_openai_envelope() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(DONE)]);
        let (status, body) = post_json(
            agy_bridge(&fake, None, None),
            "/v1/responses",
            serde_json::json!({"model": "gpt-6-sol", "input": "hi"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(fake.count("streamGenerateContent"), 0);
    }

    #[test]
    fn a_reasoning_item_whose_encrypted_content_is_null_reads_as_unsigned() {
        let body = serde_json::json!({"model": "gemini-3.8-flash", "input": [
            {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": null},
            {"type": "reasoning", "id": "rs_2", "summary": []},
            {"role": "user", "content": "hi"}]});
        let request = antigravity_request(&body, None).unwrap();
        let sealed: Vec<&str> = request
            .input
            .iter()
            .filter_map(|item| match item {
                InputItem::Reasoning {
                    encrypted_content, ..
                } => Some(encrypted_content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(sealed, ["", ""]);
    }

    #[test]
    fn a_body_without_input_is_refused_before_upstream() {
        let error = antigravity_request(&serde_json::json!({"model": "gemini-3.8-flash"}), None)
            .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("input"), "{}", error.message);
    }
}
