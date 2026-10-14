//! Antigravity's answer, as the Responses events the bridge's surfaces read.
//!
//! The surfaces already consume chatgpt.com's Responses stream, so Gemini's
//! parts are re-told in that vocabulary: text becomes an output message,
//! thoughts a reasoning item whose `encrypted_content` is the sealed
//! signature, a function call a function_call item. A signature that sits on
//! a call or on text rides a reasoning item placed just before the part it
//! signs - which is where the request side looks for it again - and a call's
//! signature is also remembered by call id for the clients that drop
//! reasoning items.
#![allow(dead_code, reason = "wired in by Plan C Tasks 8-12")]

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::BridgeError;
use super::agy_upstream::{random_hex, reason as error_reason};
use super::gemini::{LOCAL_CALL_PREFIX, SignatureCache, seal_signature};

/// Call ids minted in this process, for [`LOCAL_CALL_PREFIX`] ids.
static MINTED_CALLS: AtomicU64 = AtomicU64::new(0);

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

/// One turn's translation state.
pub(crate) struct GeminiTurn {
    wire_model: String,
    response_id: String,
    created_at: u64,
    started: bool,
    next_index: u32,
    open: Option<Open>,
    /// A signature from a part with nothing to sign, waiting for the tool
    /// call it belongs to.
    unclaimed: Option<String>,
    seen_calls: HashSet<String>,
    usage: Option<Value>,
    finish_reason: Option<String>,
    /// Why the turn failed, when Antigravity said so in words rather than
    /// with a finish reason.
    failure: Option<String>,
    finished: bool,
}

impl GeminiTurn {
    pub(crate) fn new(wire_model: &str) -> Self {
        Self {
            wire_model: wire_model.to_owned(),
            response_id: format!("resp_{}", random_hex(12)),
            created_at: crate::usage::ledger::now_unix(),
            started: false,
            next_index: 0,
            open: None,
            unclaimed: None,
            seen_calls: HashSet::new(),
            usage: None,
            finish_reason: None,
            failure: None,
            finished: false,
        }
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// One decoded Gemini event (already out of its `response` envelope).
    pub(crate) fn accept(
        &mut self,
        event: &Value,
        signatures: &SignatureCache,
    ) -> Result<Vec<Value>, BridgeError> {
        let mut out = Vec::new();
        if self.finished {
            return Ok(out);
        }
        self.start(&mut out);
        if let Some(error) = event.get("error").filter(|error| !error.is_null()) {
            let detail = error_reason(json!({ "error": error }).to_string().as_bytes());
            self.finish_reason = Some("ERROR".to_owned());
            self.failure = Some(format!("Antigravity reported an error mid-turn: {detail}"));
            self.terminate(&mut out);
            return Ok(out);
        }
        if let Some(reason) = event
            .pointer("/promptFeedback/blockReason")
            .and_then(Value::as_str)
        {
            self.finish_reason = Some(reason.to_owned());
            self.terminate(&mut out);
            return Ok(out);
        }
        let candidates = event.get("candidates").and_then(Value::as_array);
        if candidates.is_some_and(|candidates| candidates.len() > 1) {
            return Err(BridgeError::upstream(
                StatusCode::BAD_GATEWAY,
                "Antigravity answered with more than one candidate, which alc cannot relay",
            ));
        }
        if let Some(candidate) = candidates.and_then(|candidates| candidates.first()) {
            if let Some(parts) = candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
            {
                for part in parts {
                    self.part(part, signatures, &mut out)?;
                }
            }
            if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_owned());
            }
        }
        let usage = event.get("usageMetadata").filter(|usage| usage.is_object());
        if let Some(usage) = usage {
            self.usage = Some(usage.clone());
        }
        // Only counts sent with the finish, or after it, are the turn's
        // final ones; a finish without them waits for the next counts or the
        // end of the stream.
        if self.finish_reason.is_some() && usage.is_some() {
            self.terminate(&mut out);
        }
        Ok(out)
    }

    /// The stream closed. A turn that never said why it stopped did not
    /// finish, whatever it delivered.
    pub(crate) fn end_of_stream(&mut self) -> Result<Vec<Value>, BridgeError> {
        let mut out = Vec::new();
        if self.finished {
            return Ok(out);
        }
        if self.finish_reason.is_none() {
            return Err(BridgeError::upstream(
                StatusCode::BAD_GATEWAY,
                "Antigravity ended the stream without finishing the turn",
            ));
        }
        self.start(&mut out);
        self.terminate(&mut out);
        Ok(out)
    }

    fn envelope(&self, status: &str) -> Value {
        json!({"id": self.response_id, "object": "response", "created_at": self.created_at,
               "model": self.wire_model, "status": status, "output": []})
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

    fn part(
        &mut self,
        part: &Value,
        signatures: &SignatureCache,
        out: &mut Vec<Value>,
    ) -> Result<(), BridgeError> {
        let signature = part
            .get("thoughtSignature")
            .and_then(Value::as_str)
            .filter(|signature| !signature.is_empty())
            .map(|signature| seal_signature(&self.wire_model, signature));
        if let Some(call) = part.get("functionCall") {
            self.close(out);
            if let Some(sealed) = &signature {
                self.signature_item(sealed, out);
            }
            return self.function_call(call, signatures, out);
        }
        let text = part.get("text").and_then(Value::as_str).unwrap_or("");
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            if text.is_empty() && signature.is_none() {
                return Ok(()); // a thought with nothing in it
            }
            if !matches!(self.open, Some(Open::Reasoning { .. })) {
                self.close(out);
                self.open_reasoning(out);
            }
            if !text.is_empty() {
                self.reasoning_delta(text, out);
            }
            if let Some(sealed) = signature {
                self.close_reasoning(Some(sealed), out);
            }
            return Ok(());
        }
        if part.get("text").is_none() && signature.is_none() {
            return Ok(()); // a part kind the surfaces have no place for
        }
        if let Some(sealed) = &signature {
            self.close(out);
            self.signature_item(sealed, out);
        }
        if text.is_empty() {
            return Ok(());
        }
        if !matches!(self.open, Some(Open::Message { .. })) {
            self.close(out);
            self.open_message(out);
        }
        self.unclaimed = None;
        self.text_delta(text, out);
        Ok(())
    }

    /// A signature on its own, ahead of the part it signs.
    fn signature_item(&mut self, sealed: &str, out: &mut Vec<Value>) {
        let index = self.next();
        let id = format!("rs_{index}");
        out.push(
            json!({"type": "response.output_item.added", "output_index": index,
            "item": {"type": "reasoning", "id": id, "summary": []}}),
        );
        out.push(
            json!({"type": "response.output_item.done", "output_index": index,
            "item": {"type": "reasoning", "id": id, "summary": [], "encrypted_content": sealed}}),
        );
        self.unclaimed = Some(sealed.to_owned());
    }

    fn function_call(
        &mut self,
        call: &Value,
        signatures: &SignatureCache,
        out: &mut Vec<Value>,
    ) -> Result<(), BridgeError> {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    "Antigravity sent a tool call with no name",
                )
            })?;
        // The signature cache is keyed by the call id alone and outlives
        // this turn: the counter keeps minted ids unique in this process
        // whatever the randomness did, the random tail across restarts.
        let call_id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map_or_else(
                || {
                    let n = MINTED_CALLS.fetch_add(1, Ordering::Relaxed);
                    format!("{LOCAL_CALL_PREFIX}{n}_{}", random_hex(8))
                },
                str::to_owned,
            );
        if !self.seen_calls.insert(call_id.clone()) {
            return Err(BridgeError::upstream(
                StatusCode::BAD_GATEWAY,
                format!("Antigravity sent tool call '{call_id}' twice in one turn"),
            ));
        }
        let empty = json!({});
        let arguments = serde_json::to_string(
            call.get("args")
                .filter(|args| args.is_object())
                .unwrap_or(&empty),
        )
        .unwrap_or_else(|_| "{}".to_owned());
        if let Some(sealed) = self.unclaimed.take() {
            signatures.remember(&call_id, &sealed);
        }
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
        Ok(())
    }

    fn open_reasoning(&mut self, out: &mut Vec<Value>) {
        let index = self.next();
        let id = format!("rs_{index}");
        out.push(
            json!({"type": "response.output_item.added", "output_index": index,
            "item": {"type": "reasoning", "id": id, "summary": []}}),
        );
        out.push(
            json!({"type": "response.reasoning_summary_part.added", "output_index": index,
            "item_id": id, "summary_index": 0, "part": {"type": "summary_text", "text": ""}}),
        );
        self.open = Some(Open::Reasoning {
            index,
            id,
            text: String::new(),
        });
    }

    fn reasoning_delta(&mut self, delta: &str, out: &mut Vec<Value>) {
        if let Some(Open::Reasoning { index, id, text }) = &mut self.open {
            text.push_str(delta);
            out.push(
                json!({"type": "response.reasoning_summary_text.delta", "output_index": *index,
                "item_id": id, "summary_index": 0, "delta": delta}),
            );
        }
    }

    fn close_reasoning(&mut self, sealed: Option<String>, out: &mut Vec<Value>) {
        let Some(Open::Reasoning { index, id, text }) = self.open.take() else {
            return;
        };
        out.push(
            json!({"type": "response.reasoning_summary_text.done", "output_index": index,
            "item_id": id, "summary_index": 0, "text": text}),
        );
        out.push(
            json!({"type": "response.reasoning_summary_part.done", "output_index": index,
            "item_id": id, "summary_index": 0, "part": {"type": "summary_text", "text": text}}),
        );
        let summary = if text.is_empty() {
            json!([])
        } else {
            json!([{"type": "summary_text", "text": text}])
        };
        // An unsigned thought carries an empty `encrypted_content`, never
        // null: clients replay the item, and the request side reads "" as
        // unsigned.
        let encrypted = sealed.as_deref().unwrap_or("");
        out.push(json!({"type": "response.output_item.done", "output_index": index,
            "item": {"type": "reasoning", "id": id, "summary": summary, "encrypted_content": encrypted}}));
        // A signature-only thought signs what follows it, as on the request
        // side; one that came with text stays on its thought.
        if text.is_empty() && sealed.is_some() {
            self.unclaimed = sealed;
        }
    }

    fn open_message(&mut self, out: &mut Vec<Value>) {
        let index = self.next();
        let id = format!("msg_{index}");
        out.push(
            json!({"type": "response.output_item.added", "output_index": index,
            "item": {"type": "message", "id": id, "role": "assistant", "status": "in_progress",
                     "content": []}}),
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

    fn text_delta(&mut self, delta: &str, out: &mut Vec<Value>) {
        if let Some(Open::Message { index, id, text }) = &mut self.open {
            text.push_str(delta);
            out.push(
                json!({"type": "response.output_text.delta", "output_index": *index,
                "item_id": id, "content_index": 0, "delta": delta}),
            );
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
                    "item": {"type": "message", "id": id, "role": "assistant", "status": "completed",
                             "content": [{"type": "output_text", "text": text, "annotations": []}]}}));
            }
            Some(reasoning @ Open::Reasoning { .. }) => {
                self.open = Some(reasoning);
                self.close_reasoning(None, out);
            }
            None => {}
        }
    }

    fn terminate(&mut self, out: &mut Vec<Value>) {
        self.close(out);
        let reason = self.finish_reason.clone().unwrap_or_default();
        let usage = self.usage.as_ref().map_or(Value::Null, responses_usage);
        let event = match reason.as_str() {
            "STOP" => {
                let mut response = self.envelope("completed");
                response["usage"] = usage;
                json!({"type": "response.completed", "response": response})
            }
            "MAX_TOKENS" => {
                let mut response = self.envelope("incomplete");
                response["incomplete_details"] = json!({"reason": "max_output_tokens"});
                response["usage"] = usage;
                json!({"type": "response.incomplete", "response": response})
            }
            other => {
                let message = self.failure.take().unwrap_or_else(|| stop_message(other));
                let mut response = self.envelope("failed");
                response["error"] = json!({"code": other, "message": message});
                response["usage"] = usage;
                json!({"type": "response.failed", "response": response})
            }
        };
        out.push(event);
        self.finished = true;
    }
}

/// Why Antigravity stopped a turn, said so a person can act on it.
fn stop_message(reason: &str) -> String {
    match reason {
        "SAFETY" => "Antigravity's safety filter stopped the answer (SAFETY)".to_owned(),
        "RECITATION" => {
            "Antigravity stopped the answer because it recited protected text (RECITATION)"
                .to_owned()
        }
        "MALFORMED_FUNCTION_CALL" => "the model produced a tool call Antigravity could not parse \
            (MALFORMED_FUNCTION_CALL); try again"
            .to_owned(),
        other => format!("Antigravity stopped the turn ({other})"),
    }
}

/// Gemini's counts in the Responses shape. Output includes the thoughts, as
/// OpenAI's does; reasoning is the part of it that was thinking. A count
/// Gemini leaves out (the spike saw no thoughts and no cache) is left out
/// here too, which every reader takes as zero.
pub(crate) fn responses_usage(usage: &Value) -> Value {
    let read = |key: &str| usage.get(key).and_then(Value::as_u64);
    let prompt = read("promptTokenCount").unwrap_or(0);
    let candidates = read("candidatesTokenCount").unwrap_or(0);
    let thoughts = read("thoughtsTokenCount");
    let output = candidates + thoughts.unwrap_or(0);
    let mut mapped = json!({
        "input_tokens": prompt,
        "output_tokens": output,
        "total_tokens": read("totalTokenCount").unwrap_or(prompt + output),
    });
    if let Some(cached) = read("cachedContentTokenCount") {
        mapped["input_tokens_details"] = json!({"cached_tokens": cached});
    }
    if let Some(thoughts) = thoughts {
        mapped["output_tokens_details"] = json!({"reasoning_tokens": thoughts});
    }
    mapped
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;
    use crate::bridge::agy_upstream::{Decoded, decode_event};
    use crate::bridge::fixtures;
    use crate::bridge::gemini::tests::{TOOL_OUTPUT, request, weather_turn};
    use crate::bridge::gemini::{SignatureCache, translate};
    use crate::bridge::upstream::{FunctionCallOutput, InputItem, UpstreamEvent, parse_sse};

    /// Feeds a captured SSE body through the translator, end of stream
    /// included, and checks every event parses as the bridge's own type.
    pub(crate) fn replay_events(sse: &str, wire: &str, signatures: &SignatureCache) -> Vec<Value> {
        let mut turn = GeminiTurn::new(wire);
        let mut events = Vec::new();
        for frame in parse_sse(sse) {
            match decode_event(&frame.data).unwrap() {
                Decoded::Event(event) => events.extend(turn.accept(&event, signatures).unwrap()),
                Decoded::Done => break,
                Decoded::Empty => {}
            }
        }
        events.extend(turn.end_of_stream().unwrap());
        for event in &events {
            serde_json::from_value::<UpstreamEvent>(event.clone())
                .unwrap_or_else(|error| panic!("{event}: {error}"));
        }
        events
    }

    /// The finished output items of a turn, as the client replays them.
    pub(crate) fn items_from(events: &[Value]) -> Vec<InputItem> {
        events
            .iter()
            .filter(|event| event["type"] == "response.output_item.done")
            .filter_map(|event| {
                let item = &event["item"];
                match item["type"].as_str()? {
                    // As a client replays it: the item itself, through serde.
                    "reasoning" => Some(
                        serde_json::from_value(item.clone())
                            .unwrap_or_else(|error| panic!("{item}: {error}")),
                    ),
                    "function_call" => Some(InputItem::FunctionCall {
                        id: item["id"].as_str().map(str::to_owned),
                        call_id: item["call_id"].as_str()?.to_owned(),
                        name: item["name"].as_str()?.to_owned(),
                        arguments: item["arguments"].as_str()?.to_owned(),
                    }),
                    "message" => Some(InputItem::Message {
                        role: "assistant".to_owned(),
                        content: vec![crate::bridge::upstream::ContentPart::OutputText {
                            text: item["content"][0]["text"].as_str()?.to_owned(),
                        }],
                    }),
                    _ => None,
                }
            })
            .collect()
    }

    fn the_call(events: &[Value]) -> Value {
        let calls: Vec<_> = events
            .iter()
            .filter(|event| {
                event["type"] == "response.output_item.done"
                    && event["item"]["type"] == "function_call"
            })
            .map(|event| event["item"].clone())
            .collect();
        assert_eq!(calls.len(), 1, "{events:#?}");
        calls[0].clone()
    }

    /// `signed`: whether the captured turn carried a signature. Task 1's
    /// ledger: Flash signed its call, Claude (at LOW) signed nothing - and
    /// alc must not invent one.
    fn check_tool_turn(name: &str, wire: &str, signed: bool) {
        let signatures = SignatureCache::new(16);
        let events = replay_events(
            &fixtures::antigravity(&format!("{name}-tool-turn.sse")),
            wire,
            &signatures,
        );
        assert_eq!(events[0]["type"], "response.created");
        let call = the_call(&events);
        assert_eq!(call["name"], "get_weather");
        let arguments: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
        assert!(
            arguments["city"].as_str().unwrap().contains("Paris"),
            "{arguments}"
        );
        let last = events.last().unwrap();
        assert_eq!(last["type"], "response.completed");
        assert!(last["response"]["usage"]["input_tokens"].as_u64().unwrap() > 0);
        let sealed = format!("agy1:{wire}:");
        let remembered = signatures
            .recall(call["call_id"].as_str().unwrap())
            .is_some_and(|s| s.starts_with(&sealed));
        let carried = events.iter().any(|event| {
            event["item"]["encrypted_content"]
                .as_str()
                .is_some_and(|s| s.starts_with(&sealed))
        });
        assert_eq!(remembered, signed, "{events:#?}");
        assert_eq!(carried, signed, "{events:#?}");
    }

    #[test]
    fn the_claude_tool_turn_becomes_responses_events() {
        check_tool_turn("claude", "claude-sonnet-5-5-low", false);
    }

    #[test]
    fn the_flash_tool_turn_becomes_responses_events() {
        check_tool_turn("flash", "gemini-3.8-flash-low", true);
    }

    /// Builds the second request of the spike's round from turn one's items
    /// (`keep` filters what the client replays) and the tool's answer.
    fn replay_request(name: &str, wire: &str, keep: fn(&InputItem) -> bool) -> Value {
        let signatures = SignatureCache::new(16);
        let events = replay_events(
            &fixtures::antigravity(&format!("{name}-tool-turn.sse")),
            wire,
            &signatures,
        );
        let mut input = weather_turn();
        let items: Vec<_> = items_from(&events).into_iter().filter(keep).collect();
        let call_id = items
            .iter()
            .find_map(|item| match item {
                InputItem::FunctionCall { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .unwrap();
        input.extend(items);
        input.push(InputItem::FunctionCallOutput {
            call_id,
            output: FunctionCallOutput::Text(TOOL_OUTPUT.to_owned()),
        });
        translate(&request(wire, input), wire, &signatures)
            .unwrap()
            .request
    }

    /// The other half of the golden pair: what alc sends after a client
    /// replays turn one is what the live server accepted on the replay.
    fn check_replay(name: &str, wire: &str) {
        assert_eq!(
            replay_request(name, wire, |_| true),
            fixtures::antigravity_json(&format!("{name}-tool-replay.request.json"))
        );
    }

    #[test]
    fn a_replayed_claude_round_is_the_body_the_live_endpoint_accepted() {
        check_replay("claude", "claude-sonnet-5-5-low");
    }

    #[test]
    fn a_replayed_flash_round_is_the_body_the_live_endpoint_accepted() {
        check_replay("flash", "gemini-3.8-flash-low");
    }

    /// A client that drops reasoning items still replays the Flash round
    /// signed: the call id finds the signature in the cache.
    #[test]
    fn a_flash_round_replayed_without_its_reasoning_item_is_signed_from_the_cache() {
        let replayed = replay_request("flash", "gemini-3.8-flash-low", |item| {
            !matches!(item, InputItem::Reasoning { .. })
        });
        assert_eq!(
            replayed,
            fixtures::antigravity_json("flash-tool-replay.request.json")
        );
    }

    fn event(parts: Value, finish: Option<&str>, usage: Option<Value>) -> Value {
        let mut candidate = json!({"content": {"role": "model", "parts": parts}});
        if let Some(finish) = finish {
            candidate["finishReason"] = json!(finish);
        }
        let mut event = json!({"candidates": [candidate]});
        if let Some(usage) = usage {
            event["usageMetadata"] = usage;
        }
        event
    }

    fn run(events: &[Value]) -> Result<Vec<Value>, BridgeError> {
        let signatures = SignatureCache::new(8);
        let mut turn = GeminiTurn::new("claude-sonnet-5-5-low");
        let mut out = Vec::new();
        for event in events {
            out.extend(turn.accept(event, &signatures)?);
        }
        out.extend(turn.end_of_stream()?);
        Ok(out)
    }

    fn kinds(events: &[Value]) -> Vec<String> {
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap().to_owned())
            .collect()
    }

    fn done_items(events: &[Value]) -> Vec<Value> {
        events
            .iter()
            .filter(|e| e["type"] == "response.output_item.done")
            .map(|e| e["item"].clone())
            .collect()
    }

    #[test]
    fn split_thoughts_close_on_their_signature_and_usage_maps_to_responses() {
        let usage = json!({"promptTokenCount": 100, "candidatesTokenCount": 7, "thoughtsTokenCount": 5, "cachedContentTokenCount": 60, "totalTokenCount": 112});
        let events = run(&[
            event(json!([{"text": "Think", "thought": true}]), None, None),
            event(
                json!([{"text": "ing.", "thought": true, "thoughtSignature": "S1"}, {"text": "Hello"}]),
                None,
                None,
            ),
            event(json!([{"text": " there."}]), Some("STOP"), Some(usage)),
        ])
        .unwrap();
        let deltas: Vec<_> = events
            .iter()
            .filter(|e| e["type"] == "response.reasoning_summary_text.delta")
            .map(|e| e["delta"].clone())
            .collect();
        assert_eq!(deltas, [json!("Think"), json!("ing.")]);
        let reasoning = events
            .iter()
            .find(|e| e["type"] == "response.output_item.done" && e["item"]["type"] == "reasoning")
            .unwrap();
        assert_eq!(
            reasoning["item"]["encrypted_content"],
            "agy1:claude-sonnet-5-5-low:S1"
        );
        assert_eq!(
            reasoning["item"]["summary"],
            json!([{"type": "summary_text", "text": "Thinking."}])
        );
        let text: String = events
            .iter()
            .filter(|e| e["type"] == "response.output_text.delta")
            .map(|e| e["delta"].as_str().unwrap())
            .collect();
        assert_eq!(text, "Hello there.");
        let completed = events.last().unwrap();
        assert_eq!(completed["type"], "response.completed");
        assert_eq!(
            completed["response"]["usage"],
            json!({"input_tokens": 100, "input_tokens_details": {"cached_tokens": 60}, "output_tokens": 12,
                   "output_tokens_details": {"reasoning_tokens": 5}, "total_tokens": 112})
        );
    }

    /// What the spike saw: three counts, no thoughts and no cache. Both read
    /// as zero through the bridge's own `Usage`.
    #[test]
    fn usage_without_thoughts_or_cache_counts_them_as_none() {
        let mapped = responses_usage(
            &json!({"promptTokenCount": 403, "candidatesTokenCount": 50, "totalTokenCount": 453}),
        );
        assert_eq!(
            mapped,
            json!({"input_tokens": 403, "output_tokens": 50, "total_tokens": 453})
        );
        let usage: crate::bridge::upstream::Usage = serde_json::from_value(mapped).unwrap();
        let cached = usage
            .input_tokens_details
            .and_then(|details| details.cached_tokens);
        assert_eq!(cached.unwrap_or(0), 0);
    }

    #[test]
    fn the_last_usage_wins() {
        let first = json!({"promptTokenCount": 5, "candidatesTokenCount": 1});
        let last = json!({"promptTokenCount": 5, "candidatesTokenCount": 9});
        let events = run(&[
            event(json!([{"text": "a"}]), None, Some(first)),
            event(json!([{"text": "b"}]), Some("STOP"), Some(last)),
        ])
        .unwrap();
        assert_eq!(
            events.last().unwrap()["response"]["usage"]["output_tokens"],
            9
        );
    }

    #[test]
    fn an_empty_text_part_says_nothing() {
        let events = run(&[
            event(json!([{"text": ""}]), None, None),
            event(json!([{"text": ""}]), Some("STOP"), Some(json!({}))),
        ])
        .unwrap();
        assert_eq!(kinds(&events), ["response.created", "response.completed"]);
    }

    #[test]
    fn max_tokens_ends_incomplete() {
        let events = run(&[event(
            json!([{"text": "cut"}]),
            Some("MAX_TOKENS"),
            Some(json!({"promptTokenCount": 1})),
        )])
        .unwrap();
        let last = events.last().unwrap();
        assert_eq!(last["type"], "response.incomplete");
        assert_eq!(
            last["response"]["incomplete_details"]["reason"],
            "max_output_tokens"
        );
    }

    #[test]
    fn safety_ends_as_a_visible_failure() {
        let events = run(&[event(
            json!([]),
            Some("SAFETY"),
            Some(json!({"promptTokenCount": 1})),
        )])
        .unwrap();
        let last = events.last().unwrap();
        assert_eq!(last["type"], "response.failed");
        let message = last["response"]["error"]["message"].as_str().unwrap();
        assert!(message.contains("SAFETY"), "{message}");
        assert!(message.contains("safety filter"), "{message}");
        let blocked =
            run(&[json!({"promptFeedback": {"blockReason": "PROHIBITED_CONTENT"}})]).unwrap();
        assert_eq!(blocked.last().unwrap()["type"], "response.failed");
    }

    /// `decode_event` refuses an `error` on the envelope; one that reaches
    /// the translator (inside `response`) still ends the turn, visibly.
    #[test]
    fn an_error_inside_the_event_fails_the_turn() {
        let events = run(&[
            event(json!([{"text": "half"}]), None, None),
            json!({"error": {"code": 500, "message": "backend fell over"}}),
        ])
        .unwrap();
        let last = events.last().unwrap();
        assert_eq!(last["type"], "response.failed");
        let message = last["response"]["error"]["message"].as_str().unwrap();
        assert!(message.contains("backend fell over"), "{message}");
        assert_eq!(done_items(&events)[0]["type"], "message", "{events:#?}");
    }

    #[test]
    fn a_stream_without_a_finish_reason_is_an_error_at_its_end() {
        let error = run(&[event(json!([{"text": "half"}]), None, None)]).unwrap_err();
        assert!(error.message.contains("without finishing"), "{error}");
    }

    #[test]
    fn a_finish_without_usage_completes_at_the_end_of_the_stream() {
        let events = run(&[event(json!([{"text": "ok"}]), Some("STOP"), None)]).unwrap();
        assert_eq!(kinds(&events).last().unwrap(), "response.completed");
    }

    #[test]
    fn a_missing_call_id_gets_a_local_one_and_a_repeated_one_is_refused() {
        let events = run(&[event(
            json!([{"functionCall": {"name": "get_weather", "args": {"city": "Paris"}}}]),
            Some("STOP"),
            Some(json!({})),
        )])
        .unwrap();
        let call_id = the_call(&events)["call_id"].as_str().unwrap().to_owned();
        assert!(call_id.starts_with("ag_"), "{call_id}");
        let twice = json!([{"functionCall": {"name": "a", "args": {}, "id": "c1"}}, {"functionCall": {"name": "a", "args": {}, "id": "c1"}}]);
        assert!(run(&[event(twice, Some("STOP"), None)]).is_err());
    }

    /// The cache is keyed by call id alone, so two turns' local ids must
    /// never meet.
    #[test]
    fn local_call_ids_differ_between_turns() {
        let call = || {
            let events = run(&[event(
                json!([{"functionCall": {"name": "a", "args": {}}}]),
                Some("STOP"),
                Some(json!({})),
            )])
            .unwrap();
            the_call(&events)["call_id"].as_str().unwrap().to_owned()
        };
        let (first, second) = (call(), call());
        assert_ne!(first, second);
        for id in [&first, &second] {
            let (n, tail) = id.strip_prefix("ag_").unwrap().split_once('_').unwrap();
            assert!(n.parse::<u64>().is_ok(), "{id}");
            assert_eq!(tail.len(), 16, "{id}");
        }
    }

    #[test]
    fn a_signature_only_thought_signs_the_tool_call_after_it() {
        let signatures = SignatureCache::new(8);
        let mut turn = GeminiTurn::new("gemini-3.8-flash-low");
        let parts = json!([{"thought": true, "thoughtSignature": "S7"}, {"functionCall": {"name": "a", "args": {}, "id": "c7"}}]);
        let events = turn
            .accept(&event(parts, Some("STOP"), Some(json!({}))), &signatures)
            .unwrap();
        assert_eq!(
            signatures.recall("c7").as_deref(),
            Some("agy1:gemini-3.8-flash-low:S7")
        );
        assert!(kinds(&events).contains(&"response.completed".to_owned()));
        assert!(turn.is_finished());
    }

    #[test]
    fn a_signed_call_is_preceded_by_its_signature_and_remembered() {
        let signatures = SignatureCache::new(8);
        let mut turn = GeminiTurn::new("gemini-3.8-flash-low");
        let parts = json!([{"functionCall": {"name": "a", "args": {}, "id": "c8"}, "thoughtSignature": "S8"}]);
        let events = turn
            .accept(&event(parts, Some("STOP"), Some(json!({}))), &signatures)
            .unwrap();
        let kinds: Vec<_> = done_items(&events)
            .iter()
            .map(|item| item["type"].clone())
            .collect();
        assert_eq!(kinds, [json!("reasoning"), json!("function_call")]);
        assert_eq!(
            signatures.recall("c8").as_deref(),
            Some("agy1:gemini-3.8-flash-low:S8")
        );
    }

    /// Flash ends a text-only turn on a signed empty part; the signature
    /// still reaches the client, after the text, and nothing else is said.
    #[test]
    fn a_signed_empty_closing_part_carries_its_signature_after_the_text() {
        let events = run(&[
            event(json!([{"text": "Sunny."}]), None, None),
            event(
                json!([{"text": "", "thoughtSignature": "S9"}]),
                Some("STOP"),
                Some(json!({})),
            ),
        ])
        .unwrap();
        let dones = done_items(&events);
        assert_eq!(dones.len(), 2, "{dones:#?}");
        assert_eq!(dones[0]["type"], "message");
        assert_eq!(dones[1]["type"], "reasoning");
        assert_eq!(
            dones[1]["encrypted_content"],
            "agy1:claude-sonnet-5-5-low:S9"
        );
    }

    /// An unsigned, empty thought between a signature and its call does not
    /// lose the signature.
    #[test]
    fn an_empty_unsigned_thought_keeps_a_waiting_signature() {
        let signatures = SignatureCache::new(8);
        let mut turn = GeminiTurn::new("gemini-3.8-flash-low");
        let parts = json!([{"text": "", "thoughtSignature": "S3"}, {"thought": true, "text": ""}, {"functionCall": {"name": "a", "args": {}, "id": "c3"}}]);
        turn.accept(&event(parts, Some("STOP"), Some(json!({}))), &signatures)
            .unwrap();
        assert_eq!(
            signatures.recall("c3").as_deref(),
            Some("agy1:gemini-3.8-flash-low:S3")
        );
    }

    /// Gemini above LOW streams thought text unsigned (the signature rides
    /// the call after it). The client replays that reasoning item: it must
    /// parse, and alc then replays the turn with no signature invented.
    #[test]
    fn unsigned_thought_text_round_trips_and_is_replayed_unsigned() {
        let events = run(&[event(
            json!([{"text": "Pondering.", "thought": true}, {"text": "Sunny."}]),
            Some("STOP"),
            Some(json!({})),
        )])
        .unwrap();
        let reasoning = done_items(&events)
            .into_iter()
            .find(|item| item["type"] == "reasoning")
            .unwrap();
        assert_eq!(reasoning["encrypted_content"], "");
        let item: InputItem = serde_json::from_value(reasoning).unwrap();
        assert!(
            matches!(&item, InputItem::Reasoning { encrypted_content, .. } if encrypted_content.is_empty()),
            "{item:?}"
        );
        // A client that leaves the field out entirely is read the same way.
        let bare: InputItem =
            serde_json::from_value(json!({"type": "reasoning", "id": "rs_0", "summary": []}))
                .unwrap();
        assert!(
            matches!(&bare, InputItem::Reasoning { encrypted_content, .. } if encrypted_content.is_empty()),
            "{bare:?}"
        );
        let wire = "claude-sonnet-5-5-low";
        let mut input = weather_turn();
        input.extend(items_from(&events));
        input.push(InputItem::Message {
            role: "user".to_owned(),
            content: vec![crate::bridge::upstream::ContentPart::InputText {
                text: "And tomorrow?".to_owned(),
            }],
        });
        let replayed = translate(&request(wire, input), wire, &SignatureCache::new(8))
            .unwrap()
            .request;
        let text = replayed.to_string();
        assert!(!text.contains("thoughtSignature"), "{text}");
        // Task 6 drops an unsigned reasoning item rather than forging one;
        // the turn's own text is what replays.
        assert!(!text.contains("Pondering."), "{text}");
        assert_eq!(
            replayed["contents"][1],
            json!({"role": "model", "parts": [{"text": "Sunny."}]})
        );
    }

    #[test]
    fn counts_sent_after_the_finish_are_the_ones_reported() {
        let signatures = SignatureCache::new(8);
        let mut turn = GeminiTurn::new("claude-sonnet-5-5-low");
        let early = json!({"promptTokenCount": 5, "candidatesTokenCount": 1});
        let first = turn
            .accept(
                &event(json!([{"text": "a"}]), None, Some(early.clone())),
                &signatures,
            )
            .unwrap();
        let finish = turn
            .accept(
                &event(json!([{"text": "b"}]), Some("STOP"), None),
                &signatures,
            )
            .unwrap();
        assert!(!kinds(&first).contains(&"response.completed".to_owned()));
        assert!(
            !kinds(&finish).contains(&"response.completed".to_owned()),
            "{finish:#?}"
        );
        assert!(!turn.is_finished());
        let counts = json!({"usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 9}});
        let last = turn.accept(&counts, &signatures).unwrap();
        let completed = last.last().unwrap();
        assert_eq!(completed["type"], "response.completed");
        assert_eq!(completed["response"]["usage"]["output_tokens"], 9);
        assert!(turn.is_finished());
        // With no later counts, the end of the stream settles on the earlier.
        let events = run(&[
            event(json!([{"text": "a"}]), None, Some(early)),
            event(json!([{"text": "b"}]), Some("STOP"), None),
        ])
        .unwrap();
        let completed = events.last().unwrap();
        assert_eq!(completed["type"], "response.completed");
        assert_eq!(completed["response"]["usage"]["output_tokens"], 1);
    }

    /// The captured text-only Flash turn: its closing signed empty part
    /// becomes the last item, and only the terminal event follows it.
    #[test]
    fn the_captured_flash_text_turn_ends_on_its_sealed_signature() {
        let signatures = SignatureCache::new(8);
        let events = replay_events(
            &fixtures::antigravity("flash-tool-replay.sse"),
            "gemini-3.8-flash-low",
            &signatures,
        );
        let dones = done_items(&events);
        assert_eq!(dones.len(), 2, "{dones:#?}");
        assert_eq!(dones[0]["type"], "message");
        let said = dones[0]["content"][0]["text"].as_str().unwrap();
        assert!(said.starts_with("Sunny,"), "{said}");
        assert_eq!(dones[1]["type"], "reasoning");
        assert_eq!(dones[1]["summary"], json!([]));
        assert_eq!(
            dones[1]["encrypted_content"],
            "agy1:gemini-3.8-flash-low:sig-flash-2"
        );
        let position = events
            .iter()
            .position(|e| {
                e["type"] == "response.output_item.done" && e["item"]["type"] == "reasoning"
            })
            .unwrap();
        assert_eq!(kinds(&events[position + 1..]), ["response.completed"]);
    }

    #[test]
    fn more_than_one_candidate_is_refused() {
        let error = run(&[json!({"candidates": [{}, {}]})]).unwrap_err();
        assert!(error.message.contains("more than one candidate"), "{error}");
    }

    #[test]
    fn nothing_follows_the_terminal_event() {
        let signatures = SignatureCache::new(8);
        let mut turn = GeminiTurn::new("claude-sonnet-5-5-low");
        let finished = event(json!([{"text": "a"}]), Some("STOP"), Some(json!({})));
        turn.accept(&finished, &signatures).unwrap();
        assert!(turn.is_finished());
        let late = event(json!([{"text": "b"}]), None, None);
        assert!(turn.accept(&late, &signatures).unwrap().is_empty());
        assert!(turn.end_of_stream().unwrap().is_empty());
    }
}
