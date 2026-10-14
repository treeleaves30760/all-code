//! The bridge's one internal request model, as a Code Assist `request`.
//!
//! The surfaces build an [`UpstreamRequest`] as they always have; this file
//! turns it into Gemini's `contents`/`systemInstruction`/`tools`/
//! `generationConfig`. Two things here are not translation but policy.
//!
//! Signatures. Antigravity may sign a model turn (`thoughtSignature`), and
//! alc replays every signature it is handed, whatever the family. alc carries
//! each one out to the client sealed with the wire model that minted it
//! (`agy1:<wire>:<signature>`), in a Responses `reasoning` item's
//! `encrypted_content`, a Messages `thinking` block's `signature`, and - for
//! every surface - a bounded LRU keyed by the tool call's id. Only Gemini
//! ([`SIGNING_FAMILIES`]) has its tool rounds refused without the signature
//! it minted, so only there is a round in progress with no usable signature
//! refused with [`NEW_SESSION`]; alc never invents one. A Claude turn the
//! server did not sign is replayed as it was received (the live replay of
//! one was served).
//!
//! Schemas. Agents ship JSON Schema richer than Gemini's subset, so schemas
//! are rewritten, not refused: what has a Gemini spelling gets it, the rest
//! is dropped and reported once.
#![allow(dead_code, reason = "wired in by Plan C Tasks 7-12")]

use std::collections::{BTreeSet, HashMap, VecDeque};

use axum::http::StatusCode;
use serde_json::{Map, Value, json};

use super::BridgeError;
use super::tiers::ModelTiers;
use super::upstream::{ContentPart, Effort, FunctionCallOutput, InputItem, Tool, UpstreamRequest};
use crate::antigravity::{self, AgyFamily};

pub(crate) const SIGNATURE_PREFIX: &str = "agy1:";
/// Call ids alc mints when upstream sends none. Never sent back upstream.
pub(crate) const LOCAL_CALL_PREFIX: &str = "ag_";
pub(crate) const SIGNATURE_CACHE_CAPACITY: usize = 4096;
pub(crate) const NEW_SESSION: &str = "this conversation's tool history carries no Antigravity \
    signature alc can replay (it began elsewhere, on another model or effort, or before alc's \
    bridge restarted); start a new session to continue on Antigravity";
/// The output cap for an Antigravity model alc does not ship.
const CUSTOM_OUTPUT_CAP: u64 = 8_192;
/// Families whose tool rounds the server accepts only with the signature it
/// minted. The spike (Task 1) saw Flash sign every model turn and Claude sign
/// none, and the unsigned Claude replay was served; GPT-OSS rounds go
/// unsigned. A signature that does arrive from any family is still replayed.
const SIGNING_FAMILIES: [&str; 1] = ["gemini-"];
const MAX_REF_DEPTH: usize = 8;

pub(crate) fn seal_signature(wire_model: &str, signature: &str) -> String {
    format!("{SIGNATURE_PREFIX}{wire_model}:{signature}")
}

/// The signature inside `sealed`, when it was minted by `wire_model`. A
/// signature from another wire model (another effort) is as good as none.
pub(crate) fn open_signature<'a>(sealed: &'a str, wire_model: &str) -> Option<&'a str> {
    let (model, signature) = sealed.strip_prefix(SIGNATURE_PREFIX)?.split_once(':')?;
    (model == wire_model && !signature.is_empty()).then_some(signature)
}

/// Call id → sealed signature, for the clients that cannot carry one on a
/// tool call (Messages, Chat) or drop reasoning items (some Responses ones).
pub(crate) struct SignatureCache {
    capacity: usize,
    inner: std::sync::Mutex<(HashMap<String, String>, VecDeque<String>)>,
}

impl SignatureCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            inner: std::sync::Mutex::new((HashMap::new(), VecDeque::new())),
        }
    }

    pub(crate) fn remember(&self, call_id: &str, sealed: &str) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (map, order) = &mut *guard;
        if map.insert(call_id.to_owned(), sealed.to_owned()).is_none() {
            order.push_back(call_id.to_owned());
        }
        while order.len() > self.capacity {
            if let Some(oldest) = order.pop_front() {
                map.remove(&oldest);
            }
        }
    }

    pub(crate) fn recall(&self, call_id: &str) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
            .get(call_id)
            .cloned()
    }
}

fn effort_word(effort: Option<Effort>) -> &'static str {
    match effort {
        None | Some(Effort::Medium) => "medium",
        Some(Effort::None | Effort::Low) => "low",
        Some(Effort::High | Effort::Xhigh | Effort::Max) => "high",
    }
}

/// The wire id a request is served by. A wire id is taken as it is; a family
/// gets the effort's suffix; a Claude id or alias lands on its tier (Claude
/// Code); an id alc does not ship but Antigravity might passes through.
pub(crate) fn wire_model(
    requested: &str,
    effort: Option<Effort>,
    tiers: Option<&ModelTiers>,
) -> Result<String, BridgeError> {
    let asked = requested.trim();
    if antigravity::split_wire_id(asked).is_some() {
        return Ok(asked.to_owned());
    }
    let family_id = match antigravity::family(asked) {
        Some(family) => family.id,
        None => match tiers.and_then(|tiers| tiers.serve_as(asked)) {
            Some(served) => served.to_owned(),
            None if ["gemini-", "claude-", "gpt-oss-"]
                .iter()
                .any(|prefix| asked.starts_with(prefix)) =>
            {
                return Ok(asked.to_owned());
            }
            None => {
                return Err(BridgeError::invalid(format!(
                    "Antigravity serves Claude, Gemini and GPT-OSS models, and '{asked}' is none of them; \
                     `alc models --agy` lists what this login can run"
                )));
            }
        },
    };
    if antigravity::split_wire_id(&family_id).is_some() {
        return Ok(family_id);
    }
    match antigravity::family(&family_id) {
        Some(family) => Ok(antigravity::wire_model(&family, effort_word(effort))),
        None => Ok(family_id),
    }
}

pub(crate) struct Translated {
    pub request: Value,
    /// Schema keywords dropped because Gemini has no spelling for them.
    pub dropped_schema_keys: BTreeSet<String>,
}

pub(crate) fn translate(
    request: &UpstreamRequest,
    wire_model: &str,
    signatures: &SignatureCache,
) -> Result<Translated, BridgeError> {
    let split = antigravity::split_wire_id(wire_model);
    let mut system = Vec::new();
    if let Some(instructions) = request
        .instructions
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        system.push(json!({"text": instructions}));
    }
    let mut declarations = Vec::new();
    let mut dropped = BTreeSet::new();
    let mut contents = Contents::new(wire_model, signatures);
    for item in &request.input {
        match item {
            InputItem::AdditionalTools { tools, .. } => {
                declarations.extend(tools.iter().map(|tool| declaration(tool, &mut dropped)));
            }
            InputItem::Message { role, content } if role == "developer" || role == "system" => {
                system.extend(
                    content
                        .iter()
                        .filter_map(part_text)
                        .map(|text| json!({"text": text})),
                );
            }
            InputItem::Message { role, content } => {
                contents.message(role == "assistant", content)?
            }
            InputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => contents.call(call_id, name, arguments),
            InputItem::FunctionCallOutput { call_id, output } => {
                contents.output(call_id, output)?
            }
            InputItem::Reasoning {
                summary,
                encrypted_content,
                ..
            } => contents.reasoning(summary, encrypted_content),
        }
    }
    let mut inner = json!({
        "contents": contents.finish()?,
        "generationConfig": generation_config(request, split.as_ref()),
    });
    if !system.is_empty() {
        inner["systemInstruction"] = json!({"parts": system});
    }
    if !declarations.is_empty() {
        inner["tools"] = json!([{"functionDeclarations": declarations}]);
    }
    Ok(Translated {
        request: inner,
        dropped_schema_keys: dropped,
    })
}

fn part_text(part: &ContentPart) -> Option<&str> {
    match part {
        ContentPart::InputText { text } | ContentPart::OutputText { text } => Some(text.as_str()),
        ContentPart::InputImage { .. } => None,
    }
}

fn declaration(tool: &Tool, dropped: &mut BTreeSet<String>) -> Value {
    let mut declaration =
        json!({"name": tool.name, "parameters": translate_schema(&tool.parameters, dropped)});
    if let Some(description) = tool.description.as_deref().filter(|text| !text.is_empty()) {
        declaration["description"] = json!(description);
    }
    declaration
}

fn inline_image(url: &str) -> Result<Value, BridgeError> {
    let refused = || {
        BridgeError::invalid(
            "Antigravity takes images only inline, as a base64 data: URL; this request linked one - attach the image itself",
        )
    };
    let rest = url.strip_prefix("data:").ok_or_else(refused)?;
    let (meta, data) = rest.split_once(',').ok_or_else(refused)?;
    let mime = meta.strip_suffix(";base64").ok_or_else(refused)?;
    Ok(json!({"inlineData": {"mimeType": mime, "data": data}}))
}

struct Turn {
    role: &'static str,
    /// (sort key, part). Only `user` turns are sorted: tool results by the
    /// order their calls were made, everything else after them.
    parts: Vec<(usize, Value)>,
}

struct Contents<'a> {
    wire_model: &'a str,
    signatures: &'a SignatureCache,
    turns: Vec<Turn>,
    /// A signature with no part yet to sign: the next model part takes it.
    pending: Option<String>,
    /// call id → (name, the order it was called in).
    calls: HashMap<String, (String, usize)>,
}

impl<'a> Contents<'a> {
    fn new(wire_model: &'a str, signatures: &'a SignatureCache) -> Self {
        Self {
            wire_model,
            signatures,
            turns: Vec::new(),
            pending: None,
            calls: HashMap::new(),
        }
    }

    fn append(&mut self, role: &'static str, key: usize, part: Value) {
        match self.turns.last_mut() {
            Some(turn) if turn.role == role => turn.parts.push((key, part)),
            _ => self.turns.push(Turn {
                role,
                parts: vec![(key, part)],
            }),
        }
    }

    /// A signature nothing claimed rides an empty part, as it arrived.
    fn flush_pending(&mut self) {
        if let Some(signature) = self.pending.take() {
            self.append(
                "model",
                usize::MAX,
                json!({"text": "", "thoughtSignature": signature}),
            );
        }
    }

    fn model_part(&mut self, mut part: Value) {
        if let Some(signature) = self.pending.take() {
            part["thoughtSignature"] = json!(signature);
        }
        self.append("model", usize::MAX, part);
    }

    fn user_part(&mut self, key: usize, part: Value) {
        self.flush_pending();
        self.append("user", key, part);
    }

    fn message(&mut self, assistant: bool, content: &[ContentPart]) -> Result<(), BridgeError> {
        for part in content {
            let value = match part {
                ContentPart::InputText { text } | ContentPart::OutputText { text }
                    if text.is_empty() =>
                {
                    continue;
                }
                ContentPart::InputText { text } | ContentPart::OutputText { text } => {
                    json!({"text": text})
                }
                ContentPart::InputImage { image_url, .. } => inline_image(image_url)?,
            };
            if assistant {
                self.model_part(value);
            } else {
                self.user_part(usize::MAX, value);
            }
        }
        Ok(())
    }

    fn reasoning(&mut self, summary: &[Value], sealed: &str) {
        let Some(signature) = open_signature(sealed, self.wire_model) else {
            return; // unsigned, or another model's: dropped, never forged
        };
        let text: String = summary
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect();
        self.flush_pending();
        if text.is_empty() {
            self.pending = Some(signature.to_owned());
        } else {
            self.append(
                "model",
                usize::MAX,
                json!({"text": text, "thought": true, "thoughtSignature": signature}),
            );
        }
    }

    fn call(&mut self, call_id: &str, name: &str, arguments: &str) {
        let order = self.calls.len();
        self.calls
            .insert(call_id.to_owned(), (name.to_owned(), order));
        let args = serde_json::from_str::<Value>(arguments)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let mut call = json!({"name": name, "args": args});
        if !call_id.starts_with(LOCAL_CALL_PREFIX) {
            call["id"] = json!(call_id);
        }
        let mut part = json!({"functionCall": call});
        if self.pending.is_none()
            && let Some(sealed) = self.signatures.recall(call_id)
            && let Some(signature) = open_signature(&sealed, self.wire_model)
        {
            part["thoughtSignature"] = json!(signature);
        }
        self.model_part(part);
    }

    fn output(&mut self, call_id: &str, output: &FunctionCallOutput) -> Result<(), BridgeError> {
        let (name, order) = self.calls.get(call_id).cloned().ok_or_else(|| {
            BridgeError::invalid(format!(
                "a tool result answers call '{call_id}', which no tool call earlier in this conversation made"
            ))
        })?;
        let text = match output {
            FunctionCallOutput::Text(text) => text.clone(),
            FunctionCallOutput::Parts(parts) => parts
                .iter()
                .filter_map(part_text)
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let mut response = json!({"name": name, "response": {"output": text}});
        if !call_id.starts_with(LOCAL_CALL_PREFIX) {
            response["id"] = json!(call_id);
        }
        self.user_part(order, json!({"functionResponse": response}));
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<Value>, BridgeError> {
        self.flush_pending();
        for turn in &mut self.turns {
            if turn.role == "user" {
                turn.parts.sort_by_key(|(key, _)| *key);
            }
        }
        // The round in progress: every turn after the last user turn that
        // says something other than tool results.
        let active_from = self
            .turns
            .iter()
            .rposition(|turn| {
                turn.role == "user"
                    && turn
                        .parts
                        .iter()
                        .any(|(_, part)| part.get("functionResponse").is_none())
            })
            .unwrap_or(0);
        // Only a signing family's unsigned rounds need care; every other
        // family's turns are replayed exactly as they were received.
        let signing = SIGNING_FAMILIES
            .iter()
            .any(|prefix| self.wire_model.starts_with(prefix));
        let unsigned_round = |turn: &Turn| {
            turn.role == "model"
                && turn
                    .parts
                    .iter()
                    .any(|(_, part)| part.get("functionCall").is_some())
                && !turn
                    .parts
                    .iter()
                    .any(|(_, part)| part.get("thoughtSignature").is_some())
        };
        let mut as_text = vec![false; self.turns.len()];
        for (index, turn) in self.turns.iter().enumerate() {
            if signing && unsigned_round(turn) {
                if index > active_from {
                    return Err(BridgeError::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        NEW_SESSION,
                    ));
                }
                as_text[index] = true;
                if let Some(next) = as_text.get_mut(index + 1) {
                    *next = true;
                }
            }
        }
        Ok(self
            .turns
            .into_iter()
            .zip(as_text)
            .map(|(turn, as_text)| {
                let parts: Vec<Value> = turn
                    .parts
                    .into_iter()
                    .map(|(_, part)| if as_text { history_text(part) } else { part })
                    .collect();
                json!({"role": turn.role, "parts": parts})
            })
            .collect())
    }
}

/// A closed tool round alc cannot replay signed, as the plain text it was.
fn history_text(part: Value) -> Value {
    if let Some(call) = part.get("functionCall") {
        let name = call["name"].as_str().unwrap_or("tool");
        return json!({"text": format!("[called tool {name} with {}]", call["args"])});
    }
    if let Some(response) = part.get("functionResponse") {
        let name = response["name"].as_str().unwrap_or("tool");
        let output = response
            .pointer("/response/output")
            .and_then(Value::as_str)
            .unwrap_or("");
        return json!({"text": format!("[tool {name} returned]\n{output}")});
    }
    part
}

fn generation_config(request: &UpstreamRequest, split: Option<&(AgyFamily, String)>) -> Value {
    let cap = split.map_or(CUSTOM_OUTPUT_CAP, |(family, _)| family.max_output_tokens);
    let max_output = request
        .max_output_tokens
        .map_or(cap, |asked| asked.clamp(1, cap));
    let mut config = json!({"maxOutputTokens": max_output});
    if let Some((family, effort)) = split
        && let Some(thinking) = family.thinking.get(effort).and_then(Value::as_object)
    {
        let mut thinking = thinking.clone();
        thinking.insert("includeThoughts".to_owned(), json!(true));
        if let Some(budget) = thinking.get("thinkingBudget").and_then(Value::as_u64)
            && budget > 0
            && max_output <= budget
        {
            let floor: u64 = if family.id.starts_with("claude-") {
                1024
            } else {
                128
            };
            thinking.insert(
                "thinkingBudget".to_owned(),
                json!(floor.max(max_output / 2)),
            );
        }
        config["thinkingConfig"] = Value::Object(thinking);
    }
    if let Some(temperature) = request.temperature {
        config["temperature"] = json!(temperature);
    }
    config
}

/// A client's JSON Schema in Gemini's subset.
pub(crate) fn translate_schema(schema: &Value, dropped: &mut BTreeSet<String>) -> Value {
    let definitions = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut out = convert(schema, &definitions, 0, dropped);
    if let Some(object) = out.as_object_mut()
        && !object.contains_key("type")
        && !object.contains_key("anyOf")
    {
        object.insert("type".to_owned(), json!("OBJECT"));
    }
    out
}

fn convert(
    node: &Value,
    definitions: &Value,
    depth: usize,
    dropped: &mut BTreeSet<String>,
) -> Value {
    let Some(object) = node.as_object() else {
        return json!({});
    };
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let target = reference
            .strip_prefix("#/$defs/")
            .or_else(|| reference.strip_prefix("#/definitions/"))
            .and_then(|name| definitions.get(name));
        return match target {
            Some(target) if depth < MAX_REF_DEPTH => {
                convert(target, definitions, depth + 1, dropped)
            }
            _ => {
                dropped.insert("$ref".to_owned());
                json!({"type": "OBJECT"})
            }
        };
    }
    let mut out = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "type" => apply_type(value, &mut out),
            "properties" => {
                if let Some(properties) = value.as_object() {
                    let converted = properties
                        .iter()
                        .map(|(name, schema)| {
                            (name.clone(), convert(schema, definitions, depth, dropped))
                        })
                        .collect();
                    out.insert("properties".to_owned(), Value::Object(converted));
                }
            }
            "items" => {
                let first = match value {
                    Value::Array(items) => items.first().cloned().unwrap_or_else(|| json!({})),
                    other => other.clone(),
                };
                out.insert(
                    "items".to_owned(),
                    convert(&first, definitions, depth, dropped),
                );
            }
            "anyOf" | "oneOf" => {
                if let Some(options) = value.as_array() {
                    let converted = options
                        .iter()
                        .map(|option| convert(option, definitions, depth, dropped))
                        .collect();
                    out.insert("anyOf".to_owned(), Value::Array(converted));
                }
            }
            "const" => {
                out.insert("enum".to_owned(), json!([value]));
            }
            "required" | "enum" | "description" | "nullable" | "minimum" | "maximum"
            | "minItems" | "maxItems" | "format" => {
                out.insert(key.clone(), value.clone());
            }
            "$schema"
            | "additionalProperties"
            | "default"
            | "title"
            | "examples"
            | "$defs"
            | "definitions" => {}
            other => {
                dropped.insert(other.to_owned());
            }
        }
    }
    Value::Object(out)
}

fn apply_type(value: &Value, out: &mut Map<String, Value>) {
    match value {
        Value::String(kind) => {
            out.insert("type".to_owned(), json!(kind.to_ascii_uppercase()));
        }
        Value::Array(kinds) => {
            let named: Vec<&str> = kinds.iter().filter_map(Value::as_str).collect();
            let real: Vec<&str> = named
                .iter()
                .copied()
                .filter(|kind| *kind != "null")
                .collect();
            if real.len() < named.len() {
                out.insert("nullable".to_owned(), json!(true));
            }
            match real.as_slice() {
                [] => {}
                [one] => {
                    out.insert("type".to_owned(), json!(one.to_ascii_uppercase()));
                }
                many => {
                    let options = many
                        .iter()
                        .map(|kind| json!({"type": kind.to_ascii_uppercase()}))
                        .collect();
                    out.insert("anyOf".to_owned(), Value::Array(options));
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::*;
    use crate::bridge::fixtures;
    use crate::bridge::upstream::{TextOptions, Tool};

    pub(crate) const SYSTEM: &str = "You are a terse assistant. Use tools when asked.";
    pub(crate) const PROMPT: &str = "Call get_weather for Paris, then stop.";
    pub(crate) const TOOL_OUTPUT: &str = "Sunny, 21 C";

    pub(crate) fn request(model: &str, input: Vec<InputItem>) -> UpstreamRequest {
        UpstreamRequest {
            model: model.to_owned(),
            kind: None,
            store: false,
            stream: Some(true),
            parallel_tool_calls: false,
            reasoning: None,
            text: TextOptions {
                verbosity: None,
                format: None,
            },
            client_metadata: None,
            instructions: None,
            include: None,
            tool_choice: None,
            prompt_cache_key: None,
            input,
            max_output_tokens: Some(2048),
            temperature: None,
        }
    }

    /// What the spike sent on its first turn, as the bridge's internal model.
    pub(crate) fn weather_turn() -> Vec<InputItem> {
        vec![
            InputItem::AdditionalTools {
                role: "developer".to_owned(),
                tools: vec![Tool {
                    kind: "function".to_owned(),
                    name: "get_weather".to_owned(),
                    description: Some("Current weather for a city".to_owned()),
                    strict: false,
                    parameters: json!({"type": "object", "properties": {"city": {"type": "string", "description": "City name"}}, "required": ["city"]}),
                }],
            },
            InputItem::Message {
                role: "developer".to_owned(),
                content: vec![ContentPart::InputText {
                    text: SYSTEM.to_owned(),
                }],
            },
            InputItem::Message {
                role: "user".to_owned(),
                content: vec![ContentPart::InputText {
                    text: PROMPT.to_owned(),
                }],
            },
        ]
    }

    fn call(call_id: &str, arguments: &str) -> InputItem {
        InputItem::FunctionCall {
            id: None,
            call_id: call_id.to_owned(),
            name: "get_weather".to_owned(),
            arguments: arguments.to_owned(),
        }
    }

    fn output(call_id: &str, text: &str) -> InputItem {
        InputItem::FunctionCallOutput {
            call_id: call_id.to_owned(),
            output: FunctionCallOutput::Text(text.to_owned()),
        }
    }

    fn signed(wire: &str, signature: &str) -> InputItem {
        InputItem::Reasoning {
            id: "rs".to_owned(),
            summary: Vec::new(),
            encrypted_content: seal_signature(wire, signature),
        }
    }

    fn text(role: &str, text: &str) -> InputItem {
        let part = if role == "assistant" {
            ContentPart::OutputText {
                text: text.to_owned(),
            }
        } else {
            ContentPart::InputText {
                text: text.to_owned(),
            }
        };
        InputItem::Message {
            role: role.to_owned(),
            content: vec![part],
        }
    }

    fn translated(model: &str, input: Vec<InputItem>) -> Result<Value, BridgeError> {
        translate(&request(model, input), model, &SignatureCache::new(16)).map(|t| t.request)
    }

    /// The spike's replay: its first turn, the call the model made, the result.
    fn weather_replay(call_id: &str, signature: Option<(&str, &str)>) -> Vec<InputItem> {
        let mut input = weather_turn();
        if let Some((wire, signature)) = signature {
            input.push(signed(wire, signature));
        }
        input.push(call(call_id, "{\"city\":\"Paris\"}"));
        input.push(output(call_id, TOOL_OUTPUT));
        input
    }

    #[test]
    fn the_claude_tool_turn_translates_into_the_body_the_live_endpoint_accepted() {
        let model = "claude-sonnet-5-5-low";
        assert_eq!(
            translated(model, weather_turn()).unwrap(),
            fixtures::antigravity_json("claude-tool-turn.request.json")
        );
    }

    #[test]
    fn the_flash_tool_turn_translates_into_the_body_the_live_endpoint_accepted() {
        let model = "gemini-3.8-flash-low";
        assert_eq!(
            translated(model, weather_turn()).unwrap(),
            fixtures::antigravity_json("flash-tool-turn.request.json")
        );
    }

    #[test]
    fn the_unsigned_claude_replay_translates_into_the_body_the_live_endpoint_accepted() {
        let model = "claude-sonnet-5-5-low";
        assert_eq!(
            translated(model, weather_replay("call-claude-1", None)).unwrap(),
            fixtures::antigravity_json("claude-tool-replay.request.json")
        );
    }

    #[test]
    fn the_signed_flash_replay_translates_into_the_body_the_live_endpoint_accepted() {
        let model = "gemini-3.8-flash-low";
        let expected = fixtures::antigravity_json("flash-tool-replay.request.json");
        let from_reasoning = translated(
            model,
            weather_replay("call-flash-1", Some((model, "sig-flash-1"))),
        )
        .unwrap();
        assert_eq!(from_reasoning, expected);

        let cache = SignatureCache::new(4);
        cache.remember("call-flash-1", &seal_signature(model, "sig-flash-1"));
        let from_lru = translate(
            &request(model, weather_replay("call-flash-1", None)),
            model,
            &cache,
        )
        .unwrap()
        .request;
        assert_eq!(from_lru, expected);
    }

    #[test]
    fn a_claude_round_without_a_signature_is_replayed_as_received() {
        let wire = "claude-opus-5-5-high";
        let body = translated(
            wire,
            vec![
                text("user", "go"),
                call("call-1", "{\"city\":\"Paris\"}"),
                output("call-1", "Sunny"),
                text("assistant", "It is sunny."),
                text("user", "again"),
                call("call-2", "{\"city\":\"Rome\"}"),
                output("call-2", "Rain"),
            ],
        )
        .unwrap();
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(
            contents[1]["parts"][0]["functionCall"]["id"], "call-1",
            "closed history stays a call"
        );
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["id"], "call-1");
        assert_eq!(
            contents[5]["parts"][0]["functionCall"]["id"], "call-2",
            "the round in progress is not refused"
        );
        assert_eq!(contents[6]["parts"][0]["functionResponse"]["id"], "call-2");
        assert!(
            !body.to_string().contains("thoughtSignature"),
            "no signature is ever invented"
        );
    }

    #[test]
    fn schemas_are_translated_rather_than_refused() {
        let mut dropped = BTreeSet::new();
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "title": "Args",
            "additionalProperties": false,
            "properties": {
                "mode": {"const": "fast", "description": "Speed"},
                "path": {"type": ["string", "null"], "default": "."},
                "either": {"oneOf": [{"type": "string"}, {"type": "integer", "minimum": 0}]},
                "items": {"type": "array", "items": {"$ref": "#/$defs/Item"}, "minItems": 1},
                "pattern": {"type": "string", "pattern": "^a", "minLength": 2, "examples": ["ab"]},
                "many": {"type": ["string", "number"]}
            },
            "required": ["mode"],
            "$defs": {"Item": {"type": "object", "properties": {"id": {"type": "integer", "format": "int64"}}}}
        });
        assert_eq!(
            translate_schema(&schema, &mut dropped),
            json!({
                "type": "OBJECT",
                "properties": {
                    "mode": {"enum": ["fast"], "description": "Speed"},
                    "path": {"type": "STRING", "nullable": true},
                    "either": {"anyOf": [{"type": "STRING"}, {"type": "INTEGER", "minimum": 0}]},
                    "items": {"type": "ARRAY", "items": {"type": "OBJECT", "properties": {"id": {"type": "INTEGER", "format": "int64"}}}, "minItems": 1},
                    "pattern": {"type": "STRING"},
                    "many": {"anyOf": [{"type": "STRING"}, {"type": "NUMBER"}]}
                },
                "required": ["mode"]
            })
        );
        assert_eq!(
            dropped,
            BTreeSet::from(["minLength".to_owned(), "pattern".to_owned()])
        );
    }

    #[test]
    fn a_self_referencing_schema_stops_instead_of_looping() {
        let mut dropped = BTreeSet::new();
        let schema = json!({"type": "object", "properties": {"next": {"$ref": "#/definitions/Node"}},
            "definitions": {"Node": {"type": "object", "properties": {"next": {"$ref": "#/definitions/Node"}}}}});
        let out = translate_schema(&schema, &mut dropped);
        assert_eq!(out["properties"]["next"]["type"], "OBJECT");
        assert!(dropped.contains("$ref"));
        assert_eq!(
            translate_schema(&json!({}), &mut dropped),
            json!({"type": "OBJECT"})
        );
    }

    #[test]
    fn effort_picks_the_wire_suffix_and_a_full_wire_id_is_kept() {
        let tiers = crate::antigravity::tiers("claude-sonnet-5-5");
        let wire = |model: &str, effort: Option<Effort>, tiers: Option<&ModelTiers>| {
            wire_model(model, effort, tiers).unwrap()
        };
        assert_eq!(
            wire("claude-opus-5-5", Some(Effort::Max), None),
            "claude-opus-5-5-high"
        );
        assert_eq!(
            wire("claude-opus-5-5", Some(Effort::None), None),
            "claude-opus-5-5-low"
        );
        assert_eq!(
            wire("claude-opus-5-5", None, None),
            "claude-opus-5-5-medium"
        );
        assert_eq!(
            wire("claude-opus-5-5-low", Some(Effort::High), None),
            "claude-opus-5-5-low"
        );
        assert_eq!(
            wire("sonnet", Some(Effort::Low), Some(&tiers)),
            "claude-sonnet-5-5-low"
        );
        assert_eq!(
            wire("opus[1m]", None, Some(&tiers)),
            "claude-opus-5-5-medium"
        );
        assert_eq!(
            wire("claude-haiku-4-5", Some(Effort::High), Some(&tiers)),
            "gemini-3.8-flash-high"
        );
        assert_eq!(
            wire("gpt-oss-120b", Some(Effort::Low), None),
            "gpt-oss-120b-medium"
        );
        assert_eq!(wire("gemini-9-preview", None, None), "gemini-9-preview");
        let refused = wire_model("gpt-6-sol", None, None).unwrap_err();
        assert_eq!(refused.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(refused.message.contains("alc models --agy"), "{refused}");
    }

    #[test]
    fn max_tokens_is_clamped_to_the_family_cap_and_shrinks_a_budget() {
        let config = |model: &str, max: Option<u64>| {
            let mut asked = request(model, vec![text("user", "hi")]);
            asked.max_output_tokens = max;
            translate(&asked, model, &SignatureCache::new(1))
                .unwrap()
                .request["generationConfig"]
                .clone()
        };
        assert_eq!(
            config("gemini-3.8-flash-medium", None),
            json!({"maxOutputTokens": 65536, "thinkingConfig": {"includeThoughts": true, "thinkingBudget": 4000}})
        );
        assert_eq!(
            config("gemini-3.8-flash-medium", Some(3000)),
            json!({"maxOutputTokens": 3000, "thinkingConfig": {"includeThoughts": true, "thinkingBudget": 1500}})
        );
        assert_eq!(
            config("gemini-3.8-flash-medium", Some(100)),
            json!({"maxOutputTokens": 100, "thinkingConfig": {"includeThoughts": true, "thinkingBudget": 128}})
        );
        assert_eq!(
            config("gemini-3.8-flash-high", Some(100))["thinkingConfig"]["thinkingBudget"],
            -1
        );
        assert_eq!(
            config("gpt-oss-120b-medium", Some(4000))["thinkingConfig"]["thinkingBudget"],
            2000
        );
        assert_eq!(
            config("claude-opus-5-5-high", Some(999_999))["maxOutputTokens"],
            128_000
        );
        assert_eq!(
            config("gemini-9-preview", None),
            json!({"maxOutputTokens": 8192})
        );
        let mut warm = request("gemini-3.8-flash-low", vec![text("user", "hi")]);
        warm.temperature = Some(0.2);
        assert_eq!(
            translate(&warm, "gemini-3.8-flash-low", &SignatureCache::new(1))
                .unwrap()
                .request["generationConfig"]["temperature"],
            0.2
        );
    }

    #[test]
    fn images_ride_inline_and_a_link_is_refused() {
        let image = |url: &str| {
            vec![InputItem::Message {
                role: "user".to_owned(),
                content: vec![ContentPart::InputImage {
                    image_url: url.to_owned(),
                    detail: None,
                }],
            }]
        };
        let body = translated("gemini-3.8-flash-low", image("data:image/png;base64,AAAA")).unwrap();
        assert_eq!(
            body["contents"][0]["parts"][0],
            json!({"inlineData": {"mimeType": "image/png", "data": "AAAA"}})
        );
        let error =
            translated("gemini-3.8-flash-low", image("https://example.com/a.png")).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(error.message.contains("data: URL"), "{error}");
    }

    #[test]
    fn tool_results_of_one_round_share_one_user_turn_in_call_order() {
        let wire = "gpt-oss-120b-medium";
        let body = translated(
            wire,
            vec![
                text("user", "go"),
                call("call-a", "{\"city\":\"A\"}"),
                call("call-b", "{\"city\":\"B\"}"),
                output("call-b", "b"),
                output("call-a", "a"),
            ],
        )
        .unwrap();
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"].as_array().unwrap().len(), 2);
        let ids: Vec<_> = contents[2]["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part["functionResponse"]["id"].clone())
            .collect();
        assert_eq!(ids, [json!("call-a"), json!("call-b")]);
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"],
            json!({"output": "a"})
        );
    }

    #[test]
    fn local_call_ids_are_never_sent_upstream() {
        let body = translated(
            "gpt-oss-120b-medium",
            vec![
                text("user", "go"),
                call("ag_0123", "{}"),
                output("ag_0123", "x"),
            ],
        )
        .unwrap();
        assert!(
            body["contents"][1]["parts"][0]["functionCall"]
                .get("id")
                .is_none()
        );
        assert!(
            body["contents"][2]["parts"][0]["functionResponse"]
                .get("id")
                .is_none()
        );
    }

    #[test]
    fn a_signed_round_replays_its_signature_from_the_reasoning_item_or_the_lru() {
        let wire = "gemini-3.8-flash-low";
        let body = translated(
            wire,
            vec![
                text("user", "go"),
                signed(wire, "S1"),
                call("call-1", "{}"),
                output("call-1", "x"),
            ],
        )
        .unwrap();
        assert_eq!(body["contents"][1]["parts"][0]["thoughtSignature"], "S1");

        let cache = SignatureCache::new(4);
        cache.remember("toolu_1", &seal_signature(wire, "S2"));
        let asked = request(
            wire,
            vec![
                text("user", "go"),
                call("toolu_1", "{}"),
                output("toolu_1", "x"),
            ],
        );
        let body = translate(&asked, wire, &cache).unwrap().request;
        assert_eq!(body["contents"][1]["parts"][0]["thoughtSignature"], "S2");
    }

    #[test]
    fn a_signed_thought_replays_as_a_thought_part() {
        let wire = "claude-sonnet-5-5-low";
        let thought = InputItem::Reasoning {
            id: "rs".to_owned(),
            summary: vec![json!({"type": "summary_text", "text": "Need the tool."})],
            encrypted_content: seal_signature(wire, "S1"),
        };
        let body = translated(
            wire,
            vec![
                text("user", "go"),
                thought,
                call("call-1", "{}"),
                output("call-1", "x"),
            ],
        )
        .unwrap();
        assert_eq!(
            body["contents"][1]["parts"][0],
            json!({"text": "Need the tool.", "thought": true, "thoughtSignature": "S1"})
        );
        assert!(
            body["contents"][1]["parts"][1]
                .get("thoughtSignature")
                .is_none()
        );
    }

    #[test]
    fn an_unsigned_tool_round_in_progress_is_refused_with_the_new_session_message() {
        let error = translated(
            "gemini-3.8-flash-low",
            vec![
                text("user", "go"),
                call("call-1", "{}"),
                output("call-1", "x"),
            ],
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(error.message, NEW_SESSION);
    }

    #[test]
    fn a_signature_from_another_wire_model_counts_as_none() {
        let other = seal_signature("gemini-3.8-flash-high", "S1");
        let input = vec![
            text("user", "go"),
            InputItem::Reasoning {
                id: "rs".to_owned(),
                summary: Vec::new(),
                encrypted_content: other,
            },
            call("call-1", "{}"),
            output("call-1", "x"),
        ];
        assert_eq!(
            translated("gemini-3.8-flash-low", input)
                .unwrap_err()
                .message,
            NEW_SESSION
        );
    }

    #[test]
    fn unsigned_history_is_replayed_as_text() {
        let body = translated(
            "gemini-3.8-flash-low",
            vec![
                text("user", "go"),
                call("call-1", "{\"city\":\"Paris\"}"),
                output("call-1", "Sunny"),
                text("assistant", "It is sunny."),
                text("user", "thanks"),
            ],
        )
        .unwrap();
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(
            contents[1]["parts"][0],
            json!({"text": "[called tool get_weather with {\"city\":\"Paris\"}]"})
        );
        assert_eq!(
            contents[2]["parts"][0],
            json!({"text": "[tool get_weather returned]\nSunny"})
        );
        assert!(
            !body.to_string().contains("thoughtSignature"),
            "no signature is ever invented"
        );
    }

    #[test]
    fn gpt_oss_tool_rounds_go_unsigned() {
        let body = translated(
            "gpt-oss-120b-medium",
            vec![
                text("user", "go"),
                call("call-1", "{}"),
                output("call-1", "x"),
            ],
        )
        .unwrap();
        assert!(
            body["contents"][1]["parts"][0]
                .get("functionCall")
                .is_some()
        );
    }

    #[test]
    fn a_pending_signature_with_nothing_after_it_rides_an_empty_part() {
        let wire = "gemini-3.8-flash-low";
        let body = translated(
            wire,
            vec![
                text("user", "go"),
                text("assistant", "Done."),
                signed(wire, "S9"),
                text("user", "next"),
            ],
        )
        .unwrap();
        assert_eq!(
            body["contents"][1]["parts"],
            json!([{"text": "Done."}, {"text": "", "thoughtSignature": "S9"}])
        );
    }

    #[test]
    fn instructions_and_system_messages_become_the_system_instruction() {
        let mut asked = request(
            "gemini-3.8-flash-low",
            vec![
                InputItem::Message {
                    role: "system".to_owned(),
                    content: vec![ContentPart::InputText {
                        text: "B".to_owned(),
                    }],
                },
                text("user", "hi"),
            ],
        );
        asked.instructions = Some("A".to_owned());
        let body = translate(&asked, "gemini-3.8-flash-low", &SignatureCache::new(1))
            .unwrap()
            .request;
        assert_eq!(
            body["systemInstruction"],
            json!({"parts": [{"text": "A"}, {"text": "B"}]})
        );
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn a_tool_result_for_a_call_nobody_made_is_refused() {
        let error = translated(
            "gpt-oss-120b-medium",
            vec![text("user", "go"), output("call-9", "x")],
        )
        .unwrap_err();
        assert!(error.message.contains("call-9"), "{error}");
    }

    #[test]
    fn seal_and_open_bind_a_signature_to_its_wire_model() {
        let sealed = seal_signature("claude-opus-5-5-high", "abc/+=");
        assert_eq!(sealed, "agy1:claude-opus-5-5-high:abc/+=");
        assert_eq!(
            open_signature(&sealed, "claude-opus-5-5-high"),
            Some("abc/+=")
        );
        assert_eq!(open_signature(&sealed, "claude-opus-5-5-low"), None);
        assert_eq!(
            open_signature("gAAAAA-codex-blob", "claude-opus-5-5-high"),
            None
        );
    }

    #[test]
    fn the_lru_forgets_the_oldest_entries_past_its_capacity() {
        let cache = SignatureCache::new(2);
        cache.remember("a", "1");
        cache.remember("b", "2");
        cache.remember("c", "3");
        assert_eq!(cache.recall("a"), None);
        assert_eq!(cache.recall("b").as_deref(), Some("2"));
        assert_eq!(cache.recall("c").as_deref(), Some("3"));
    }
}
