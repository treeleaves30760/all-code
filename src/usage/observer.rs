//! Request clocks and usage inspection. Payloads never enter the ledger.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ledger::Ledger;
use super::records::{
    CorrelationId, InputBasis, MAX_LINE_BYTES, Outcome, OutputBasis, Timing, TokenCounts,
    UsageRecord,
};
use crate::bridge::upstream::SseDecoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WireProtocol {
    Messages,
    Responses,
    Chat,
}

#[derive(Clone)]
pub(crate) struct RequestObservation(Arc<Inner>);

struct Inner {
    ledger: Arc<Ledger>,
    start: Instant,
    state: Mutex<State>,
}

struct State {
    record: UsageRecord,
    protocol: WireProtocol,
    finalized: bool,
    saw_visible: bool,
    output_basis: OutputBasis,
}

impl RequestObservation {
    pub(crate) fn new(
        ledger: Arc<Ledger>,
        model: &str,
        streaming: bool,
        protocol: WireProtocol,
        output_basis: OutputBasis,
    ) -> Self {
        let mut record = ledger.request_record(model);
        record.timing = Some(Timing {
            client_streaming: streaming,
            output_basis,
            ..Timing::default()
        });
        Self(Arc::new(Inner {
            ledger,
            start: Instant::now(),
            state: Mutex::new(State {
                record,
                protocol,
                finalized: false,
                saw_visible: false,
                output_basis,
            }),
        }))
    }

    pub(crate) fn elapsed_us(&self) -> u64 {
        u64::try_from(self.0.start.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    pub(crate) fn update(&self, update: impl FnOnce(&mut UsageRecord)) {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.finalized {
            update(&mut state.record);
        }
    }

    pub(crate) fn set_streaming(&self, streaming: bool) {
        self.update(|record| {
            if let Some(timing) = &mut record.timing {
                timing.client_streaming = streaming;
            }
        });
    }

    pub(crate) fn add_id(&self, protocol: &str, id: &str) {
        if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
            return;
        }
        self.update(|record| {
            let id = CorrelationId {
                protocol: protocol.to_owned(),
                id: id.to_owned(),
            };
            if !record.ids.contains(&id) && record.ids.len() < 8 {
                record.ids.push(id);
            }
        });
    }

    pub(crate) fn warn(&self, warning: &str) {
        self.update(|record| {
            if record.warnings.len() < 8
                && !record.warnings.iter().any(|existing| existing == warning)
            {
                record.warnings.push(warning.to_owned());
            }
        });
    }

    pub(crate) fn finish(&self, outcome: Outcome) {
        self.finish_at(outcome, self.elapsed_us());
    }

    pub(crate) fn finish_at(&self, outcome: Outcome, elapsed_us: u64) {
        let record = {
            let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.finalized {
                return;
            }
            state.finalized = true;
            state.record.outcome = outcome;
            if let Some(timing) = &mut state.record.timing {
                timing.elapsed_us = elapsed_us;
                timing.terminal_us = outcome.has_terminal().then_some(elapsed_us);
            }
            state.record.tokens.validate();
            state.record.clone()
        };
        self.0.ledger.record_request(record);
    }

    /// Inspect a complete native frame at its receive time, not its dequeue time.
    pub(crate) fn frame_at(&self, data: &str, at_us: u64) {
        if data.trim() == "[DONE]" {
            let protocol = self
                .0
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .protocol;
            if protocol == WireProtocol::Chat {
                self.finish_at(Outcome::Completed, at_us);
            }
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            self.warn("unrecognized usage frame");
            return;
        };
        self.value_at(&value, at_us, true);
    }

    pub(crate) fn json_response_at(&self, value: &Value, at_us: u64) {
        self.value_at(value, at_us, false);
    }

    fn value_at(&self, value: &Value, at_us: u64, frame: bool) {
        let mut terminal = None;
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.finalized {
            return;
        }
        let protocol = state.protocol;
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        let mut generated = false;
        let mut reasoning = false;
        match protocol {
            WireProtocol::Responses => {
                match kind {
                    "response.output_text.delta" | "response.function_call_arguments.delta" => {
                        generated = nonempty(value.get("delta"));
                    }
                    // Summary and encrypted/signature frames are not actual
                    // reasoning tokens. Translated bridges suppress reasoning.
                    "response.reasoning_text.delta" => reasoning = nonempty(value.get("delta")),
                    "response.output_item.added" | "response.output_item.done" => {
                        generated = value.get("item").is_some_and(response_item_content);
                    }
                    "response.content_part.added" | "response.content_part.done" => {
                        generated = value
                            .pointer("/part/text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| !text.is_empty());
                    }
                    _ => {}
                }
                let response = value.get("response").unwrap_or(value);
                metadata(&mut state.record, response, "responses");
                if let Some(usage) = response.get("usage").filter(|usage| usage.is_object()) {
                    state.record.tokens = openai_tokens(usage);
                }
                terminal = match kind {
                    "response.completed" => Some(Outcome::Completed),
                    "response.incomplete" => Some(Outcome::Incomplete),
                    "response.failed" | "error" => Some(Outcome::Failed),
                    _ if !frame => Some(response_outcome(response)),
                    _ => None,
                };
            }
            WireProtocol::Messages => {
                if kind == "message_start" {
                    if let Some(message) = value.get("message") {
                        metadata(&mut state.record, message, "messages");
                        if let Some(usage) = message.get("usage").filter(|usage| usage.is_object())
                        {
                            merge_tokens(&mut state.record.tokens, anthropic_tokens(usage));
                            service_tier(&mut state.record, usage);
                        }
                    }
                } else if kind == "content_block_start" {
                    if let Some(block) = value.get("content_block") {
                        if block.get("type").and_then(Value::as_str) == Some("redacted_thinking") {
                            state.output_basis = OutputBasis::Unknown;
                        }
                        generated = nonempty(block.get("text"))
                            || block
                                .get("input")
                                .and_then(Value::as_object)
                                .is_some_and(|input| !input.is_empty());
                        reasoning = nonempty(block.get("thinking"));
                    }
                } else if kind == "content_block_delta" {
                    if let Some(delta) = value.get("delta") {
                        match delta.get("type").and_then(Value::as_str) {
                            Some("text_delta") => generated = nonempty(delta.get("text")),
                            Some("thinking_delta") => reasoning = nonempty(delta.get("thinking")),
                            Some("input_json_delta") => {
                                generated = nonempty(delta.get("partial_json"))
                            }
                            _ => {}
                        }
                    }
                } else if kind == "message_delta" {
                    if let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) {
                        merge_tokens(&mut state.record.tokens, anthropic_tokens(usage));
                        service_tier(&mut state.record, usage);
                    }
                } else if kind == "message_stop" {
                    terminal = Some(Outcome::Completed);
                } else if kind == "error" {
                    terminal = Some(Outcome::Failed);
                }
                if !frame {
                    metadata(&mut state.record, value, "messages");
                    if let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) {
                        state.record.tokens = anthropic_tokens(usage);
                        service_tier(&mut state.record, usage);
                    }
                    terminal = Some(if value.get("error").is_some() {
                        Outcome::Failed
                    } else {
                        Outcome::Completed
                    });
                }
            }
            WireProtocol::Chat => {
                metadata(&mut state.record, value, "chat");
                if let Some(choices) = value.get("choices").and_then(Value::as_array) {
                    for choice in choices {
                        if let Some(delta) = choice.get("delta") {
                            generated |= nonempty(delta.get("content"));
                            reasoning |= nonempty(delta.get("reasoning_content"))
                                || nonempty(delta.get("reasoning"));
                            generated |= delta
                                .get("tool_calls")
                                .and_then(Value::as_array)
                                .is_some_and(|calls| {
                                    calls
                                        .iter()
                                        .any(|call| nonempty(call.pointer("/function/arguments")))
                                });
                        }
                    }
                }
                if let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) {
                    state.record.tokens = openai_tokens(usage);
                }
                if !frame {
                    terminal = Some(if value.get("error").is_some() {
                        Outcome::Failed
                    } else {
                        Outcome::Completed
                    });
                } else if value.get("error").is_some() {
                    terminal = Some(Outcome::Failed);
                }
            }
        }
        let qualifies = generated || (reasoning && state.output_basis != OutputBasis::NonReasoning);
        if generated {
            state.saw_visible = true;
        }
        if let Some(timing) = &mut state.record.timing {
            if generated {
                timing.first_visible_us.get_or_insert(at_us);
            }
            if qualifies {
                timing.first_content_us.get_or_insert(at_us);
                timing.last_content_us = Some(at_us);
            }
        }
        let basis = match (state.output_basis, state.record.tokens.reasoning_tokens) {
            (OutputBasis::Unknown, Some(0)) => OutputBasis::Gross,
            (OutputBasis::Unknown, Some(_)) if state.saw_visible => OutputBasis::NonReasoning,
            (basis, _) => basis,
        };
        if let Some(timing) = &mut state.record.timing {
            timing.output_basis = basis;
        }
        drop(state);
        if let Some(outcome) = terminal {
            self.finish_at(outcome, at_us);
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        if !state.finalized {
            state.record.outcome = Outcome::Cancelled;
            if let Some(timing) = &mut state.record.timing {
                timing.elapsed_us =
                    u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX);
            }
            self.ledger.record_request(state.record.clone());
        }
    }
}

fn response_item_content(item: &Value) -> bool {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => nonempty(item.get("arguments")),
        Some("message") => item
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("type").and_then(Value::as_str) == Some("output_text")
                        && nonempty(part.get("text"))
                })
            }),
        _ => false,
    }
}

fn nonempty(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

fn service_tier(record: &mut UsageRecord, value: &Value) {
    if let Some(tier) = value
        .get("service_tier")
        .and_then(Value::as_str)
        .filter(|tier| !tier.is_empty() && tier.len() <= 128 && !tier.chars().any(char::is_control))
    {
        record.service_tier = Some(tier.to_owned());
    }
}

fn metadata(record: &mut UsageRecord, response: &Value, protocol: &str) {
    if let Some(model) = response
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| model.len() <= 512)
    {
        record.model = Some(model.to_owned());
    }
    service_tier(record, response);
    if let Some(id) = response
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 512)
    {
        let id = CorrelationId {
            protocol: protocol.to_owned(),
            id: id.to_owned(),
        };
        if !record.ids.contains(&id) && record.ids.len() < 8 {
            record.ids.push(id);
        }
    }
}

fn response_outcome(response: &Value) -> Outcome {
    match response.get("status").and_then(Value::as_str) {
        Some("failed") => Outcome::Failed,
        Some("incomplete") => Outcome::Incomplete,
        _ if response.get("error").is_some_and(|error| !error.is_null()) => Outcome::Failed,
        _ => Outcome::Completed,
    }
}

pub(crate) fn openai_tokens(usage: &Value) -> TokenCounts {
    let read = |key: &str| usage.get(key).and_then(Value::as_u64);
    let input_details = usage
        .get("input_tokens_details")
        .or_else(|| usage.get("prompt_tokens_details"));
    let output_details = usage
        .get("output_tokens_details")
        .or_else(|| usage.get("completion_tokens_details"));
    let mut tokens = TokenCounts {
        input_tokens: read("input_tokens").or_else(|| read("prompt_tokens")),
        output_tokens: read("output_tokens").or_else(|| read("completion_tokens")),
        cache_read_tokens: input_details
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64),
        cache_write_tokens: input_details
            .and_then(|details| details.get("cache_write_tokens"))
            .and_then(Value::as_u64),
        reasoning_tokens: output_details
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64),
        total_tokens: read("total_tokens"),
        ..TokenCounts::default()
    };
    tokens.validate();
    tokens
}

pub(crate) fn anthropic_tokens(usage: &Value) -> TokenCounts {
    let mut tokens = TokenCounts {
        input_basis: InputBasis::Separate,
        input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
        output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
        cache_read_tokens: usage.get("cache_read_input_tokens").and_then(Value::as_u64),
        cache_write_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64),
        cache_write_5m_tokens: usage
            .pointer("/cache_creation/ephemeral_5m_input_tokens")
            .and_then(Value::as_u64),
        cache_write_1h_tokens: usage
            .pointer("/cache_creation/ephemeral_1h_input_tokens")
            .and_then(Value::as_u64),
        ..TokenCounts::default()
    };
    tokens.validate();
    tokens
}

fn merge_tokens(existing: &mut TokenCounts, update: TokenCounts) {
    existing.input_basis = update.input_basis;
    macro_rules! merge {
        ($($field:ident),+) => { $(if update.$field.is_some() { existing.$field = update.$field; })+ };
    }
    merge!(
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cache_write_5m_tokens,
        cache_write_1h_tokens,
        reasoning_tokens,
        total_tokens
    );
    existing.validate();
}

pub(crate) struct StreamObservation {
    pub(crate) request: RequestObservation,
    decoder: SseDecoder,
    pending_bytes: usize,
    last_received_us: u64,
    disabled: bool,
}

impl StreamObservation {
    pub(crate) fn new(request: RequestObservation) -> Self {
        Self {
            request,
            decoder: SseDecoder::default(),
            pending_bytes: 0,
            last_received_us: 0,
            disabled: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        let at = self.request.elapsed_us();
        self.push_at(chunk, at);
    }

    pub(crate) fn push_at(&mut self, chunk: &[u8], at_us: u64) {
        self.last_received_us = at_us;
        if self.disabled {
            return;
        }
        if self.pending_bytes.saturating_add(chunk.len()) > MAX_LINE_BYTES {
            self.disabled = true;
            self.decoder = SseDecoder::default();
            self.request
                .warn("usage inspection exceeded its frame limit");
            return;
        }
        let frames = self.decoder.push(chunk);
        self.pending_bytes = if frames.is_empty() {
            self.pending_bytes + chunk.len()
        } else {
            self.decoder.pending_bytes()
        };
        for frame in frames {
            self.request.frame_at(&frame.data, at_us);
        }
    }

    pub(crate) fn eof(&mut self) {
        if !self.disabled {
            for frame in self.decoder.finish() {
                self.request.frame_at(&frame.data, self.last_received_us);
            }
        }
        self.request.finish(Outcome::Truncated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Agent, ProviderKind};
    use crate::usage::ledger::read_records;

    fn observation(dir: &std::path::Path, protocol: WireProtocol) -> RequestObservation {
        let ledger = Arc::new(Ledger::new(
            dir.join("usage.jsonl"),
            Agent::Claude,
            "test".to_owned(),
            ProviderKind::Codex,
            None,
        ));
        RequestObservation::new(
            ledger,
            "test-model",
            true,
            protocol,
            OutputBasis::NonReasoning,
        )
    }

    #[test]
    fn headers_metadata_empty_delta_and_usage_are_not_first_token() {
        let dir = tempfile::tempdir().unwrap();
        let request = observation(dir.path(), WireProtocol::Responses);
        request.frame_at(
            r#"{"type":"response.created","response":{"id":"resp_test"}}"#,
            10,
        );
        request.frame_at(r#"{"type":"response.output_text.delta","delta":""}"#, 20);
        request.frame_at(
            r#"{"type":"response.output_text.delta","delta":"hi"}"#,
            1000,
        );
        request.frame_at(r#"{"type":"response.completed","response":{"id":"resp_test","usage":{"input_tokens":100,"output_tokens":4,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}}"#, 3000);
        request.finish_at(Outcome::Completed, 4000);
        let records = read_records(dir.path()).records;
        assert_eq!(records.len(), 1);
        let timing = records[0].timing.as_ref().unwrap();
        assert_eq!(timing.first_content_us, Some(1000));
        assert_eq!(timing.terminal_us, Some(3000));
        assert_eq!(records[0].metrics().stream_tps, Some(1500.0));
        assert_eq!(records[0].ids[0].id, "resp_test");
    }

    #[test]
    fn translated_bridge_ignores_reasoning_before_visible_content() {
        let dir = tempfile::tempdir().unwrap();
        let request = observation(dir.path(), WireProtocol::Responses);
        request.frame_at(
            r#"{"type":"response.reasoning_text.delta","delta":"thinking"}"#,
            100,
        );
        request.frame_at(
            r#"{"type":"response.output_text.delta","delta":"hi"}"#,
            1000,
        );
        request.frame_at(r#"{"type":"response.completed","response":{"usage":{"input_tokens":20,"output_tokens":6,"output_tokens_details":{"reasoning_tokens":2}}}}"#, 3000);
        let records = read_records(dir.path()).records;
        let timing = records[0].timing.as_ref().unwrap();
        assert_eq!(timing.first_content_us, Some(1000));
        assert_eq!(timing.first_visible_us, Some(1000));
        assert_eq!(records[0].metrics().ttft_ms, Some(1.0));
        assert_eq!(records[0].metrics().stream_tps, Some(1500.0));
    }

    #[test]
    fn raw_reasoning_ttft_and_visible_token_rate_use_different_clocks() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::new(
            dir.path().join("usage.jsonl"),
            Agent::Qwen,
            "test".to_owned(),
            ProviderKind::Openai,
            None,
        ));
        let request = RequestObservation::new(
            ledger,
            "model",
            true,
            WireProtocol::Chat,
            OutputBasis::Unknown,
        );
        request.frame_at(
            r#"{"choices":[{"delta":{"reasoning_content":"thinking"}}]}"#,
            100,
        );
        request.frame_at(r#"{"choices":[{"delta":{"content":"hi"}}]}"#, 1000);
        request.frame_at(r#"{"usage":{"prompt_tokens":20,"completion_tokens":6,"completion_tokens_details":{"reasoning_tokens":2}},"choices":[]}"#, 2000);
        request.frame_at("[DONE]", 3000);
        let records = read_records(dir.path()).records;
        assert_eq!(
            records[0].timing.as_ref().unwrap().first_visible_us,
            Some(1000)
        );
        assert_eq!(records[0].metrics().ttft_ms, Some(0.1));
        assert_eq!(records[0].metrics().stream_tps, Some(1500.0));
    }

    #[test]
    fn unterminated_terminal_uses_receive_time_not_eof() {
        let dir = tempfile::tempdir().unwrap();
        let request = observation(dir.path(), WireProtocol::Responses);
        let mut stream = StreamObservation::new(request);
        stream.push_at(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n",
            100,
        );
        stream.push_at(
            b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":null}}",
            200,
        );
        stream.eof();
        let records = read_records(dir.path()).records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tokens.output_tokens, None);
        assert_eq!(records[0].timing.as_ref().unwrap().terminal_us, Some(200));
    }

    #[test]
    fn messages_usage_service_tier_is_authoritative_for_streams_and_json() {
        for streaming in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let ledger = Arc::new(Ledger::new(
                dir.path().join("usage.jsonl"),
                Agent::Claude,
                "test".to_owned(),
                ProviderKind::Anthropic,
                None,
            ));
            let request = RequestObservation::new(
                ledger,
                "claude-sonnet-4-6",
                streaming,
                WireProtocol::Messages,
                OutputBasis::Gross,
            );
            let usage = serde_json::json!({
                "input_tokens":100, "output_tokens":10,
                "cache_read_input_tokens":0, "cache_creation_input_tokens":0,
                "service_tier":"priority"
            });
            if streaming {
                request.frame_at(
                    &serde_json::json!({"type":"message_start", "message":{
                        "id":"msg-tier", "model":"claude-sonnet-4-6", "usage":{
                            "input_tokens":100, "cache_read_input_tokens":0,
                            "cache_creation_input_tokens":0, "service_tier":"standard"
                        }
                    }})
                    .to_string(),
                    10,
                );
                request.frame_at(
                    &serde_json::json!({"type":"message_delta", "usage":usage}).to_string(),
                    20,
                );
                request.frame_at(r#"{"type":"message_stop"}"#, 30);
            } else {
                request.json_response_at(
                    &serde_json::json!({
                        "id":"msg-tier", "model":"claude-sonnet-4-6", "usage":usage
                    }),
                    30,
                );
            }
            let record = read_records(dir.path()).records.remove(0);
            assert_eq!(record.service_tier.as_deref(), Some("priority"));
            let estimate = crate::usage::pricing::PriceBook::load(dir.path(), None)
                .unwrap()
                .estimate(&record);
            assert_eq!(estimate.total_usd, None);
            assert_ne!(estimate.status, crate::usage::pricing::CostStatus::Complete);
        }
    }

    #[test]
    fn drop_and_premature_eof_are_not_success_or_zero_usage() {
        let dir = tempfile::tempdir().unwrap();
        let request = observation(dir.path(), WireProtocol::Responses);
        drop(request);
        let records = read_records(dir.path()).records;
        assert_eq!(records[0].outcome, Outcome::Cancelled);
        assert_eq!(records[0].tokens.input_tokens, None);
        assert_eq!(records[0].timing.as_ref().unwrap().terminal_us, None);
    }
}
