//! `POST /v1internal:*` — the Antigravity CLI, on the Codex login.
//!
//! agy speaks Google's Code Assist API, and reads its endpoint from
//! `CLOUD_CODE_URL`. Pointed here, it reaches a Code Assist that answers in
//! two voices:
//!
//! * a generation request naming a Codex model (one this surface added to
//!   agy's picker) is translated to the Codex upstream and its answer back
//!   into Gemini's stream, the way the other surfaces translate theirs;
//! * everything else - agy's own models, its login checks, its quota, its
//!   helper turns (titles, checkpoints, commit messages) - is relayed to
//!   Google untouched, signed with the token agy itself sent. alc reads no
//!   credential of agy's and keeps nothing it relays.
//!
//! So agy keeps every model it had and gains the Codex ones, and a session
//! can move between the two with agy's own `/model`.
//!
//! `fetchAvailableModels` is the one relayed answer that is edited: the Codex
//! models are added to it, one entry per effort as agy lists its own
//! (`gpt-6.1-sol-high`), first in the picker. agy parses the entry's `model`
//! as an enum it keys sessions by, so each Codex entry carries a number of its
//! own far outside the values Google hands out; a name agy does not know reads
//! as no model at all and agy refuses the turn.
//!
//! What agy's history carries that Codex cannot read is dropped rather than
//! refused: Gemini thoughts and their signatures (Codex reasoning is not
//! replayed on this login by any surface), and non-image inline data.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt as _;
use serde_json::{Map, Value, json};

use super::upstream::{
    self, ContentPart, Effort, FunctionCallOutput, InputItem, OutputItem, Reasoning, TextOptions,
    Tool, UpstreamEvent, UpstreamFrame, UpstreamRequest, UpstreamStream, Usage,
};
use super::{BridgeError, BridgeState};
use crate::config::ReasoningEffort;
use crate::model_catalog::ModelInfo;

/// Where agy's Code Assist lives when nothing overrides it: the endpoint agy
/// itself was seen calling.
pub(crate) const GOOGLE_CODE_ASSIST: &str = "https://daily-cloudcode-pa.googleapis.com";
/// The enum values the Codex entries carry. Google's run in the hundreds;
/// these cannot meet one.
const MODEL_ENUM_BASE: u64 = 1_900_000;
/// agy's own cap on a Codex turn's output; chatgpt.com sets the real one.
const CODEX_OUTPUT_CAP: u64 = 128_000;
const RPC_PREFIX: &str = "/v1internal:";
/// Headers that describe one hop rather than the request.
const HOP_HEADERS: [&str; 6] = [
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "accept-encoding",
    "keep-alive",
];

/// What the surface needs beyond the bridge's own config.
#[derive(Debug, Clone)]
pub(crate) struct CodeAssistConfig {
    /// The Codex models agy's picker gains, most capable first.
    pub models: Vec<ModelInfo>,
    /// Where everything not answered here is relayed.
    pub google_base: String,
}

impl CodeAssistConfig {
    pub(crate) fn new(models: Vec<ModelInfo>) -> Self {
        Self {
            models,
            google_base: std::env::var("ALC_CODE_ASSIST_UPSTREAM")
                .ok()
                .filter(|base| !base.trim().is_empty())
                .unwrap_or_else(|| GOOGLE_CODE_ASSIST.to_owned()),
        }
    }

    /// The Codex model and effort an agy model id names, when it names one.
    pub(crate) fn resolve(&self, id: &str) -> Option<(String, Option<ReasoningEffort>)> {
        self.models.iter().find_map(|model| {
            if model.id == id {
                return Some((model.id.clone(), None));
            }
            let rest = id.strip_prefix(model.id.as_str())?.strip_prefix('-')?;
            efforts_of(model)
                .into_iter()
                .find(|effort| effort.as_str() == rest)
                .map(|effort| (model.id.clone(), Some(effort)))
        })
    }
}

/// The efforts a model is listed at, lowest first. A model that names none
/// is listed at its default.
fn efforts_of(model: &ModelInfo) -> Vec<ReasoningEffort> {
    // agy's ladder stops at max; ultra has no spelling chatgpt.com takes.
    let ladder = [
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::Xhigh,
        ReasoningEffort::Max,
    ];
    let efforts: Vec<ReasoningEffort> = ladder
        .into_iter()
        .filter(|effort| model.supported_efforts.contains(effort))
        .collect();
    if efforts.is_empty() {
        return vec![crate::agents::agy::clamp_for_agy(model.default_effort)];
    }
    efforts
}

/// The agy id for a Codex model at an effort: `gpt-6.1-sol-high`.
pub(crate) fn wire_id(model: &str, effort: ReasoningEffort) -> String {
    format!(
        "{model}-{}",
        crate::agents::agy::clamp_for_agy(effort).as_str()
    )
}

fn effort_label(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Low => "Low",
        ReasoningEffort::Medium => "Medium",
        ReasoningEffort::High => "High",
        ReasoningEffort::Xhigh => "XHigh",
        ReasoningEffort::Max | ReasoningEffort::Ultra => "Max",
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Every `/v1internal:*` request agy makes.
pub(crate) async fn handle(
    State(state): State<Arc<BridgeState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(config) = state.config.code_assist.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(rpc) = uri.path().strip_prefix(RPC_PREFIX) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let result = match rpc {
        "streamGenerateContent" | "generateContent" => match codex_target(&config, &body) {
            Some((model, effort)) => {
                generate(&state, &body, model, effort, rpc == "streamGenerateContent").await
            }
            None => relay(&state, &config, &method, &uri, &headers, body).await,
        },
        "countTokens" if codex_target(&config, &body).is_some() => Ok(count_tokens(&body)),
        "fetchAvailableModels" => {
            available_models(&state, &config, &method, &uri, &headers, body).await
        }
        _ => relay(&state, &config, &method, &uri, &headers, body).await,
    };
    result.unwrap_or_else(|error| google_error(&error))
}

/// The Codex model a generation request names, if it names one.
fn codex_target(
    config: &CodeAssistConfig,
    body: &[u8],
) -> Option<(String, Option<ReasoningEffort>)> {
    let request: Value = serde_json::from_slice(body).ok()?;
    config.resolve(request.get("model")?.as_str()?)
}

/// Google's error envelope, which agy parses into its own message.
fn google_error(error: &BridgeError) -> Response {
    let status = match error.status.as_u16() {
        400 => "INVALID_ARGUMENT",
        401 => "UNAUTHENTICATED",
        403 => "PERMISSION_DENIED",
        404 => "NOT_FOUND",
        429 => "RESOURCE_EXHAUSTED",
        503 => "UNAVAILABLE",
        _ => "INTERNAL",
    };
    let body = json!({
        "error": {
            "code": error.status.as_u16(),
            "message": error.message,
            "status": status,
        }
    });
    (error.status, axum::Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Relay
// ---------------------------------------------------------------------------

async fn forward(
    state: &BridgeState,
    config: &CodeAssistConfig,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<reqwest::Response, BridgeError> {
    let path = uri
        .path_and_query()
        .map_or(uri.path(), |path| path.as_str());
    let url = format!("{}{path}", config.google_base.trim_end_matches('/'));
    let method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .map_err(|error| BridgeError::invalid(format!("unsupported method: {error}")))?;
    let mut request = state.http.request(method, &url).body(body);
    for (name, value) in headers {
        if HOP_HEADERS.contains(&name.as_str()) {
            continue;
        }
        request = request.header(name.as_str(), value.as_bytes());
    }
    request.send().await.map_err(|error| {
        BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("could not reach Google's Code Assist for agy: {error}"),
        )
    })
}

/// Hands Google's answer back as it arrives: a relayed generation streams.
async fn relay(
    state: &BridgeState,
    config: &CodeAssistConfig,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response, BridgeError> {
    let upstream = forward(state, config, method, uri, headers, body).await?;
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        if HOP_HEADERS.contains(&name.as_str()) || name.as_str() == "content-encoding" {
            continue;
        }
        response = response.header(name.as_str(), value.as_bytes());
    }
    let stream = upstream
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    response
        .body(Body::from_stream(stream))
        .map_err(|error| BridgeError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))
}

/// Google's model list, with the Codex models added first.
async fn available_models(
    state: &BridgeState,
    config: &CodeAssistConfig,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response, BridgeError> {
    let upstream = forward(state, config, method, uri, headers, body).await?;
    let status = upstream.status();
    let bytes = upstream
        .bytes()
        .await
        .map_err(|error| BridgeError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let status = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if !status.is_success() {
        return Ok((status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response());
    }
    let mut answer: Value = serde_json::from_slice(&bytes).map_err(|error| {
        BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("Google's model list is not JSON: {error}"),
        )
    })?;
    add_codex_models(&mut answer, &config.models);
    Ok(axum::Json(answer).into_response())
}

/// Adds one entry per Codex model and effort, and lists them first.
pub(crate) fn add_codex_models(answer: &mut Value, models: &[ModelInfo]) {
    let Some(object) = answer.as_object_mut() else {
        return;
    };
    let mut ids = Vec::new();
    let mut entries = Map::new();
    let mut ordinal = 0;
    for model in models {
        for effort in efforts_of(model) {
            let id = wire_id(&model.id, effort);
            entries.insert(
                id.clone(),
                json!({
                    "displayName": format!("{} · Codex ({})", model.name, effort_label(effort)),
                    "supportsImages": true,
                    "supportsThinking": true,
                    "recommended": true,
                    "maxTokens": model.context_window,
                    "maxOutputTokens": CODEX_OUTPUT_CAP,
                    "quotaInfo": {"remainingFraction": 1},
                    "model": MODEL_ENUM_BASE + ordinal,
                    "apiProvider": "API_PROVIDER_OPENAI_VERTEX",
                    "modelProvider": "MODEL_PROVIDER_OPENAI",
                    "tagTitle": "Codex",
                    "tagDescription": "Served by your Codex login through alc",
                }),
            );
            ids.push(Value::String(id));
            ordinal += 1;
        }
    }
    let catalog = object
        .entry("models")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(catalog) = catalog.as_object_mut() {
        catalog.extend(entries);
    }
    let sorts = object
        .entry("agentModelSorts")
        .or_insert_with(|| json!([{"displayName": "Recommended", "groups": [{"modelIds": []}]}]));
    if let Some(group) = sorts
        .get_mut(0)
        .and_then(|sort| sort.get_mut("groups"))
        .and_then(|groups| groups.get_mut(0))
        .and_then(|group| group.get_mut("modelIds"))
        .and_then(Value::as_array_mut)
    {
        group.splice(0..0, ids);
    }
}

// ---------------------------------------------------------------------------
// Request translation
// ---------------------------------------------------------------------------

/// A Code Assist `generateContent` body, as the Codex upstream's.
pub(crate) fn to_upstream(
    body: &Value,
    model: &str,
    effort: Option<ReasoningEffort>,
    pinned: Option<ReasoningEffort>,
) -> Result<UpstreamRequest, BridgeError> {
    let request = body.get("request").ok_or_else(|| {
        BridgeError::invalid("a Code Assist body carries its turn under 'request'")
    })?;
    let mut input = Vec::new();

    let tools = translate_tools(request.get("tools"));
    if !tools.is_empty() {
        input.push(InputItem::AdditionalTools {
            role: "developer".to_owned(),
            tools,
        });
    }

    let system = request
        .get("systemInstruction")
        .map(texts_of)
        .unwrap_or_default();
    if !system.trim().is_empty() {
        input.push(InputItem::Message {
            role: "developer".to_owned(),
            content: vec![ContentPart::InputText { text: system }],
        });
    }

    let contents = request
        .get("contents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    translate_contents(contents, &mut input);
    if !input
        .iter()
        .any(|item| !matches!(item, InputItem::AdditionalTools { .. }))
    {
        return Err(BridgeError::invalid(
            "the request carried no content to send",
        ));
    }

    let effort = effort.or(pinned).map_or(Effort::Medium, Effort::from);
    let cache_key = request
        .get("sessionId")
        .and_then(Value::as_str)
        .or_else(|| {
            request
                .pointer("/labels/cascade_id")
                .and_then(Value::as_str)
        })
        .map(|key| format!("agy-{key}"));
    Ok(UpstreamRequest {
        model: model.to_owned(),
        kind: None,
        store: false,
        stream: Some(true),
        parallel_tool_calls: false,
        reasoning: Some(Reasoning {
            effort: Some(effort),
            context: Some("all_turns".to_owned()),
            // agy shows thinking as it arrives; ask Codex for the summary.
            summary: Some("auto".to_owned()),
        }),
        text: TextOptions {
            verbosity: Some("low".to_owned()),
            format: None,
        },
        client_metadata: Some(std::collections::BTreeMap::from([(
            upstream::LITE_METADATA_KEY.to_owned(),
            "true".to_owned(),
        )])),
        instructions: None,
        include: None,
        tool_choice: None,
        prompt_cache_key: cache_key,
        input,
        max_output_tokens: None,
        temperature: None,
    })
}

fn texts_of(content: &Value) -> String {
    content
        .get("parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| !is_thought(part))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

fn is_thought(part: &Value) -> bool {
    part.get("thought").and_then(Value::as_bool) == Some(true)
}

fn translate_tools(tools: Option<&Value>) -> Vec<Tool> {
    let Some(tools) = tools.and_then(Value::as_array) else {
        return Vec::new();
    };
    tools
        .iter()
        .filter_map(|tool| tool.get("functionDeclarations").and_then(Value::as_array))
        .flatten()
        .filter_map(|declaration| {
            let name = declaration.get("name")?.as_str()?.to_owned();
            let schema = declaration
                .get("parametersJsonSchema")
                .or_else(|| declaration.get("parameters"))
                .map(json_schema)
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            Some(Tool {
                kind: "function".to_owned(),
                name,
                description: declaration
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                strict: false,
                parameters: object_schema(schema),
            })
        })
        .collect()
}

/// The server reads `parameters` as an object schema, always.
fn object_schema(mut schema: Value) -> Value {
    if let Some(object) = schema.as_object_mut() {
        object
            .entry("type")
            .or_insert_with(|| Value::String("object".to_owned()));
        if object.get("type").and_then(Value::as_str) == Some("object") {
            object
                .entry("properties")
                .or_insert_with(|| Value::Object(Map::new()));
        }
        return schema;
    }
    json!({"type": "object", "properties": {}})
}

/// Gemini's schema dialect as JSON Schema: `OBJECT` is `object`, `nullable`
/// is a type that admits null, and Gemini's ordering hint goes.
fn json_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(object) => {
            let nullable = object.get("nullable").and_then(Value::as_bool) == Some(true);
            let mut out = Map::new();
            for (key, value) in object {
                match key.as_str() {
                    "nullable" | "propertyOrdering" => {}
                    "type" => {
                        let kind = match value {
                            Value::String(kind) => Value::String(kind.to_ascii_lowercase()),
                            other => other.clone(),
                        };
                        let kind = match (nullable, kind) {
                            (true, Value::String(kind)) => json!([kind, "null"]),
                            (_, kind) => kind,
                        };
                        out.insert(key.clone(), kind);
                    }
                    "properties" => {
                        let properties = value
                            .as_object()
                            .map(|properties| {
                                properties
                                    .iter()
                                    .map(|(name, schema)| (name.clone(), json_schema(schema)))
                                    .collect::<Map<_, _>>()
                            })
                            .unwrap_or_default();
                        out.insert(key.clone(), Value::Object(properties));
                    }
                    _ => {
                        out.insert(key.clone(), json_schema(value));
                    }
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(json_schema).collect()),
        other => other.clone(),
    }
}

/// The conversation, as Codex input items.
///
/// A Gemini turn is typed by its parts, not its role: agy sends a function
/// response under `role: "model"`. Every call is answered before Codex sees
/// it - one agy cancelled is answered with a note saying so - because Codex
/// refuses a call with no output, and an output with no call reads as text.
fn translate_contents(contents: &[Value], input: &mut Vec<InputItem>) {
    let mut pending: Option<(String, Vec<ContentPart>)> = None;
    let mut minted: HashMap<String, VecDeque<String>> = HashMap::new();
    let mut open_calls: Vec<String> = Vec::new();
    let mut counter = 0usize;

    let flush = |pending: &mut Option<(String, Vec<ContentPart>)>, input: &mut Vec<InputItem>| {
        if let Some((role, content)) = pending.take()
            && !content.is_empty()
        {
            input.push(InputItem::Message { role, content });
        }
    };

    for content in contents {
        let assistant = content.get("role").and_then(Value::as_str) == Some("model");
        let role = if assistant { "assistant" } else { "user" };
        let Some(parts) = content.get("parts").and_then(Value::as_array) else {
            continue;
        };
        for part in parts {
            if is_thought(part) {
                continue;
            }
            if let Some(call) = part.get("functionCall") {
                flush(&mut pending, input);
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let call_id = match call.get("id").and_then(Value::as_str) {
                    Some(id) if !id.is_empty() => id.to_owned(),
                    _ => {
                        counter += 1;
                        let id = format!("agy_call_{counter}");
                        minted
                            .entry(name.clone())
                            .or_default()
                            .push_back(id.clone());
                        id
                    }
                };
                let arguments = call.get("args").cloned().unwrap_or_else(|| json!({}));
                close_unanswered(&mut open_calls, input, false);
                input.push(InputItem::FunctionCall {
                    id: None,
                    call_id: call_id.clone(),
                    name,
                    arguments: arguments.to_string(),
                });
                open_calls.push(call_id);
                continue;
            }
            if let Some(result) = part.get("functionResponse") {
                flush(&mut pending, input);
                let name = result
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let call_id = match result.get("id").and_then(Value::as_str) {
                    Some(id) if !id.is_empty() => Some(id.to_owned()),
                    _ => minted.get_mut(name).and_then(VecDeque::pop_front),
                };
                let output = tool_output(result.get("response"));
                match call_id.filter(|id| open_calls.contains(id)) {
                    Some(call_id) => {
                        open_calls.retain(|open| *open != call_id);
                        input.push(InputItem::FunctionCallOutput {
                            call_id,
                            output: FunctionCallOutput::Text(output),
                        });
                    }
                    None => {
                        input.push(InputItem::Message {
                            role: "user".to_owned(),
                            content: vec![ContentPart::InputText {
                                text: format!("Result of {name}:\n{output}"),
                            }],
                        });
                    }
                }
                continue;
            }
            let Some(translated) = translate_part(part, assistant) else {
                continue;
            };
            match &mut pending {
                Some((open_role, content)) if open_role == role => content.push(translated),
                _ => {
                    flush(&mut pending, input);
                    close_unanswered(&mut open_calls, input, true);
                    pending = Some((role.to_owned(), vec![translated]));
                }
            }
        }
    }
    flush(&mut pending, input);
    close_unanswered(&mut open_calls, input, true);
}

/// Answers the calls still open. Only once the conversation moves on: calls
/// made back to back are answered together after them.
fn close_unanswered(open_calls: &mut Vec<String>, input: &mut Vec<InputItem>, moved_on: bool) {
    if !moved_on {
        return;
    }
    for call_id in open_calls.drain(..) {
        input.push(InputItem::FunctionCallOutput {
            call_id,
            output: FunctionCallOutput::Text("(no result: the call was cancelled)".to_owned()),
        });
    }
}

fn translate_part(part: &Value, assistant: bool) -> Option<ContentPart> {
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        if text.is_empty() {
            return None;
        }
        return Some(if assistant {
            ContentPart::OutputText {
                text: text.to_owned(),
            }
        } else {
            ContentPart::InputText {
                text: text.to_owned(),
            }
        });
    }
    if let Some(data) = part.get("inlineData") {
        let mime = data.get("mimeType").and_then(Value::as_str)?;
        let bytes = data.get("data").and_then(Value::as_str)?;
        if assistant || !mime.starts_with("image/") {
            return None;
        }
        return Some(ContentPart::InputImage {
            image_url: format!("data:{mime};base64,{bytes}"),
            detail: None,
        });
    }
    if let Some(file) = part.get("fileData") {
        let uri = file.get("fileUri").and_then(Value::as_str)?;
        let text = format!("[file: {uri}]");
        return Some(if assistant {
            ContentPart::OutputText { text }
        } else {
            ContentPart::InputText { text }
        });
    }
    None
}

/// A function response's payload as the text Codex reads: agy's `output`
/// field when that is all it says, the JSON otherwise.
fn tool_output(response: Option<&Value>) -> String {
    match response {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(object))
            if object.len() == 1 && object.get("output").is_some_and(Value::is_string) =>
        {
            object["output"].as_str().unwrap_or_default().to_owned()
        }
        Some(other) => other.to_string(),
    }
}

/// A rough count for agy's context meter: four bytes a token.
fn count_tokens(body: &[u8]) -> Response {
    let total = (body.len() / 4) as u64;
    axum::Json(json!({"totalTokens": total})).into_response()
}

// ---------------------------------------------------------------------------
// Answer translation
// ---------------------------------------------------------------------------

/// One Codex turn, seen as the Gemini chunks agy reads.
struct GeminiAnswer {
    model: String,
    response_id: String,
    calls: HashMap<String, (String, String)>,
    usage: Option<Usage>,
    finished: bool,
}

impl GeminiAnswer {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            response_id: format!("codex-{}", super::agy_upstream::random_hex(8)),
            calls: HashMap::new(),
            usage: None,
            finished: false,
        }
    }

    fn chunk(&self, parts: Vec<Value>, finish: Option<&str>) -> Value {
        let mut candidate = json!({"content": {"role": "model", "parts": parts}});
        if let Some(reason) = finish {
            candidate["finishReason"] = Value::String(reason.to_owned());
        }
        let mut response = json!({
            "candidates": [candidate],
            "modelVersion": self.model,
            "responseId": self.response_id,
        });
        if let Some(usage) = &self.usage {
            let cached = usage
                .input_tokens_details
                .and_then(|details| details.cached_tokens)
                .unwrap_or(0);
            let mut metadata = json!({
                "promptTokenCount": usage.input_tokens,
                "candidatesTokenCount": usage.output_tokens,
                "totalTokenCount": usage.input_tokens + usage.output_tokens,
            });
            if cached > 0 {
                metadata["cachedContentTokenCount"] = json!(cached);
            }
            response["usageMetadata"] = metadata;
        }
        json!({"response": response, "metadata": {}})
    }

    /// The chunks one upstream event becomes.
    fn observe(&mut self, frame: &UpstreamFrame) -> Result<Vec<Value>, BridgeError> {
        Ok(match &frame.event {
            UpstreamEvent::Created { response } => {
                self.response_id = response.id.clone();
                Vec::new()
            }
            UpstreamEvent::OutputTextDelta { delta, .. } if !delta.is_empty() => {
                vec![self.chunk(vec![json!({"text": delta})], None)]
            }
            UpstreamEvent::ReasoningSummaryTextDelta { delta, .. } if !delta.is_empty() => {
                vec![self.chunk(vec![json!({"text": delta, "thought": true})], None)]
            }
            UpstreamEvent::OutputItemAdded {
                item:
                    OutputItem::FunctionCall {
                        id, call_id, name, ..
                    },
                ..
            } => {
                self.calls
                    .insert(id.clone(), (call_id.clone(), name.clone()));
                Vec::new()
            }
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
                self.calls.remove(id);
                let args: Value = serde_json::from_str(arguments)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({}));
                vec![self.chunk(
                    vec![json!({"functionCall": {"name": name, "args": args, "id": call_id}})],
                    None,
                )]
            }
            UpstreamEvent::Completed { response } | UpstreamEvent::Incomplete { response } => {
                self.usage = response.usage;
                self.finished = true;
                let reason = if matches!(frame.event, UpstreamEvent::Incomplete { .. }) {
                    "MAX_TOKENS"
                } else {
                    "STOP"
                };
                vec![self.chunk(vec![json!({"text": ""})], Some(reason))]
            }
            UpstreamEvent::Failed { response } => {
                return Err(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    response
                        .error
                        .as_ref()
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("the Codex turn failed")
                        .to_owned(),
                ));
            }
            UpstreamEvent::Error { error } => {
                return Err(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the Codex turn failed")
                        .to_owned(),
                ));
            }
            UpstreamEvent::RateLimits(limits) if limits.is_terminal() => {
                return Err(BridgeError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limit_error",
                    "the Codex login is out of quota for now; `alc --codex usage` says when it \
                     resets",
                ));
            }
            _ => Vec::new(),
        })
    }
}

fn sse(value: &Value) -> Bytes {
    Bytes::from(format!("data: {value}\r\n\r\n"))
}

async fn generate(
    state: &BridgeState,
    body: &[u8],
    model: String,
    effort: Option<ReasoningEffort>,
    streaming: bool,
) -> Result<Response, BridgeError> {
    let body: Value = serde_json::from_slice(body)
        .map_err(|error| BridgeError::invalid(format!("the request body is not JSON: {error}")))?;
    let request = to_upstream(&body, &model, effort, state.config.effort)?;
    let observed = upstream::observation(
        state,
        &model,
        streaming,
        None,
        crate::usage::records::OutputBasis::Unknown,
    );
    let stream = upstream::send_with_affinity(state, request, None, observed).await?;
    let answer = GeminiAnswer::new(&model);
    if streaming {
        return Ok(stream_answer(stream, answer));
    }
    aggregate(stream, answer).await
}

fn stream_answer(stream: UpstreamStream, answer: GeminiAnswer) -> Response {
    let frames = futures_util::stream::unfold(
        Some((stream, answer, VecDeque::<Bytes>::new())),
        |state| async move {
            let (mut stream, mut answer, mut queued) = state?;
            loop {
                if let Some(frame) = queued.pop_front() {
                    return Some((
                        Ok::<_, std::io::Error>(frame),
                        Some((stream, answer, queued)),
                    ));
                }
                if answer.finished {
                    return None;
                }
                match stream.next().await {
                    Some(Ok(frame)) => match answer.observe(&frame) {
                        Ok(chunks) => queued.extend(chunks.iter().map(sse)),
                        Err(error) => {
                            answer.finished = true;
                            queued.push_back(sse(&error_chunk(&error)));
                        }
                    },
                    Some(Err(error)) => {
                        answer.finished = true;
                        queued.push_back(sse(&error_chunk(&error)));
                    }
                    None => {
                        // The stream ended without saying the turn was over.
                        answer.finished = true;
                        let chunk = answer.chunk(vec![json!({"text": ""})], Some("STOP"));
                        queued.push_back(sse(&chunk));
                    }
                }
            }
        },
    );
    let mut response = Response::new(Body::from_stream(frames));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

fn error_chunk(error: &BridgeError) -> Value {
    json!({"error": {"code": error.status.as_u16(), "message": error.message}})
}

async fn aggregate(
    mut stream: UpstreamStream,
    mut answer: GeminiAnswer,
) -> Result<Response, BridgeError> {
    let mut parts: Vec<Value> = Vec::new();
    let mut finish = "STOP";
    while let Some(frame) = stream.next().await {
        for chunk in answer.observe(&frame?)? {
            let candidate = &chunk["response"]["candidates"][0];
            if let Some(reason) = candidate["finishReason"].as_str() {
                finish = if reason == "MAX_TOKENS" {
                    "MAX_TOKENS"
                } else {
                    "STOP"
                };
            }
            for part in candidate["content"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
            {
                match (parts.last_mut(), part.get("text").and_then(Value::as_str)) {
                    (_, Some("")) => {}
                    (Some(last), Some(text))
                        if last.get("thought") == part.get("thought")
                            && last.get("text").is_some() =>
                    {
                        let joined = format!("{}{text}", last["text"].as_str().unwrap_or_default());
                        last["text"] = Value::String(joined);
                    }
                    _ => parts.push(part.clone()),
                }
            }
        }
        if answer.finished {
            break;
        }
    }
    Ok(axum::Json(answer.chunk(parts, Some(finish))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sol() -> ModelInfo {
        ModelInfo {
            id: "gpt-6.1-sol".to_owned(),
            name: "GPT-6.1 Sol".to_owned(),
            description: String::new(),
            context_window: 400_000,
            max_context_window: 0,
            default_effort: ReasoningEffort::Medium,
            supported_efforts: vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Ultra,
            ],
            priority: None,
        }
    }

    fn config() -> CodeAssistConfig {
        CodeAssistConfig {
            models: vec![sol()],
            google_base: GOOGLE_CODE_ASSIST.to_owned(),
        }
    }

    #[test]
    fn an_agy_id_names_a_codex_model_and_its_effort() {
        let config = config();
        assert_eq!(
            config.resolve("gpt-6.1-sol-high"),
            Some(("gpt-6.1-sol".to_owned(), Some(ReasoningEffort::High)))
        );
        assert_eq!(
            config.resolve("gpt-6.1-sol"),
            Some(("gpt-6.1-sol".to_owned(), None))
        );
        assert_eq!(config.resolve("gpt-6.1-sol-ultra"), None);
        assert_eq!(config.resolve("gemini-3.8-flash-low"), None);
    }

    #[test]
    fn the_codex_models_join_agys_list_first_one_entry_per_effort() {
        let mut answer = json!({
            "models": {"gemini-3.8-flash-low": {"displayName": "Gemini 3.8 Flash (Low)"}},
            "agentModelSorts": [{"displayName": "Recommended",
                "groups": [{"modelIds": ["gemini-3.8-flash-low"]}]}],
        });
        add_codex_models(&mut answer, &[sol()]);
        assert_eq!(
            answer["agentModelSorts"][0]["groups"][0]["modelIds"],
            json!([
                "gpt-6.1-sol-low",
                "gpt-6.1-sol-medium",
                "gpt-6.1-sol-high",
                "gemini-3.8-flash-low"
            ])
        );
        let entry = &answer["models"]["gpt-6.1-sol-high"];
        assert_eq!(entry["displayName"], "GPT-6.1 Sol · Codex (High)");
        assert_eq!(entry["maxTokens"], 400_000);
        assert!(entry["model"].as_u64().unwrap() >= MODEL_ENUM_BASE);
        assert!(answer["models"]["gemini-3.8-flash-low"].is_object());
    }

    fn agy_turn() -> Value {
        json!({
            "model": "gpt-6.1-sol-high",
            "request": {
                "systemInstruction": {"role": "user", "parts": [{"text": "You are Antigravity."}]},
                "tools": [{"functionDeclarations": [{
                    "name": "view_file",
                    "description": "View a file",
                    "parameters": {"type": "OBJECT", "properties": {
                        "AbsolutePath": {"type": "STRING"},
                        "Lines": {"type": "ARRAY", "items": {"type": "INTEGER"}, "nullable": true},
                    }, "propertyOrdering": ["AbsolutePath"]},
                }]}],
                "contents": [
                    {"role": "user", "parts": [{"text": "read note.txt"}]},
                    {"role": "model", "parts": [
                        {"text": "", "thought": true, "thoughtSignature": "abc"},
                        {"functionCall": {"id": "call_1", "name": "view_file",
                            "args": {"AbsolutePath": "/tmp/note.txt"}}},
                    ]},
                    {"role": "model", "parts": [{"functionResponse": {"id": "call_1",
                        "name": "view_file", "response": {"output": "1: hello"}}}]},
                ],
                "sessionId": "-375",
            },
        })
    }

    #[test]
    fn an_agy_turn_becomes_the_codex_body() {
        let built = to_upstream(
            &agy_turn(),
            "gpt-6.1-sol",
            Some(ReasoningEffort::High),
            None,
        )
        .unwrap();
        let value = serde_json::to_value(&built).unwrap();
        assert_eq!(value["model"], "gpt-6.1-sol");
        assert_eq!(value["reasoning"]["effort"], "high");
        assert_eq!(value["prompt_cache_key"], "agy--375");
        assert_eq!(
            value["input"][0]["tools"][0]["parameters"],
            json!({"type": "object", "properties": {
                "AbsolutePath": {"type": "string"},
                "Lines": {"type": ["array", "null"], "items": {"type": "integer"}},
            }})
        );
        assert_eq!(
            value["input"][1],
            json!({"type": "message", "role": "developer",
                   "content": [{"type": "input_text", "text": "You are Antigravity."}]})
        );
        assert_eq!(value["input"][2]["role"], "user");
        assert_eq!(
            value["input"][3],
            json!({"type": "function_call", "call_id": "call_1", "name": "view_file",
                   "arguments": "{\"AbsolutePath\":\"/tmp/note.txt\"}"})
        );
        assert_eq!(
            value["input"][4],
            json!({"type": "function_call_output", "call_id": "call_1", "output": "1: hello"})
        );
        assert_eq!(value["input"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn a_call_agy_never_answered_is_answered_before_the_next_turn() {
        let mut turn = agy_turn();
        let contents = turn["request"]["contents"].as_array_mut().unwrap();
        contents.pop();
        contents.push(json!({"role": "user", "parts": [{"text": "never mind"}]}));
        let built = to_upstream(&turn, "gpt-6.1-sol", None, None).unwrap();
        let value = serde_json::to_value(&built).unwrap();
        assert_eq!(value["input"][4]["type"], "function_call_output");
        assert_eq!(value["input"][4]["call_id"], "call_1");
        assert_eq!(value["input"][5]["role"], "user");
        assert_eq!(value["reasoning"]["effort"], "medium");
    }

    #[test]
    fn a_response_without_ids_pairs_with_its_call_by_name() {
        let turn = json!({"request": {"contents": [
            {"role": "user", "parts": [{"text": "go"}]},
            {"role": "model", "parts": [{"functionCall": {"name": "run", "args": {}}}]},
            {"role": "user", "parts": [{"functionResponse": {"name": "run",
                "response": {"exit": 0}}}]},
        ]}});
        let value = serde_json::to_value(to_upstream(&turn, "m", None, None).unwrap()).unwrap();
        assert_eq!(value["input"][1]["call_id"], "agy_call_1");
        assert_eq!(value["input"][2]["call_id"], "agy_call_1");
        assert_eq!(value["input"][2]["output"], "{\"exit\":0}");
    }

    fn frame(event: Value) -> UpstreamFrame {
        UpstreamFrame {
            raw: event.to_string(),
            event: serde_json::from_value(event).unwrap(),
        }
    }

    #[test]
    fn a_codex_turn_reads_as_gemini_chunks() {
        let mut answer = GeminiAnswer::new("gpt-6.1-sol");
        let mut chunks = Vec::new();
        for event in [
            json!({"type": "response.created", "response": {"id": "resp_1"}}),
            json!({"type": "response.reasoning_summary_text.delta", "output_index": 0,
                   "delta": "Thinking"}),
            json!({"type": "response.output_text.delta", "output_index": 1,
                   "content_index": 0, "delta": "Reading."}),
            json!({"type": "response.output_item.added", "output_index": 2, "item": {
                "type": "function_call", "id": "fc_1", "call_id": "call_9",
                "name": "view_file", "arguments": ""}}),
            json!({"type": "response.output_item.done", "output_index": 2, "item": {
                "type": "function_call", "id": "fc_1", "call_id": "call_9",
                "name": "view_file", "arguments": "{\"AbsolutePath\":\"/x\"}"}}),
            json!({"type": "response.completed", "response": {"id": "resp_1", "usage": {
                "input_tokens": 100, "output_tokens": 7, "total_tokens": 107,
                "input_tokens_details": {"cached_tokens": 64}}}}),
        ] {
            chunks.extend(answer.observe(&frame(event)).unwrap());
        }
        assert!(answer.finished);
        assert_eq!(chunks.len(), 4);
        assert_eq!(
            chunks[0]["response"]["candidates"][0]["content"]["parts"][0],
            json!({"text": "Thinking", "thought": true})
        );
        assert_eq!(chunks[1]["response"]["responseId"], "resp_1");
        assert_eq!(
            chunks[2]["response"]["candidates"][0]["content"]["parts"][0],
            json!({"functionCall": {"name": "view_file", "args": {"AbsolutePath": "/x"},
                   "id": "call_9"}})
        );
        let last = &chunks[3]["response"];
        assert_eq!(last["candidates"][0]["finishReason"], "STOP");
        assert_eq!(last["usageMetadata"]["promptTokenCount"], 100);
        assert_eq!(last["usageMetadata"]["cachedContentTokenCount"], 64);
    }

    #[test]
    fn a_failed_codex_turn_is_an_error() {
        let mut answer = GeminiAnswer::new("m");
        let error = answer
            .observe(&frame(json!({"type": "response.failed", "response": {
                "id": "r", "error": {"message": "model overloaded"}}})))
            .unwrap_err();
        assert_eq!(error.message, "model overloaded");
    }
}
