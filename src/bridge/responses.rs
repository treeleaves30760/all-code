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

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::Response;
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};

use super::upstream::{self, Lane};
use super::{BridgeError, BridgeState};

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
    normalise(&mut request);

    for attempt in 0..=REFUSAL_RETRIES {
        let payload = serialise(&request, &model)?;
        let response = upstream::post(state, payload, true, Lane::Plain).await?;
        if response.status() != StatusCode::BAD_REQUEST {
            return Ok(if streaming {
                relay_response(response)
            } else {
                fold_response(response).await?
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
            return Ok(buffered_response(status, headers, refusal));
        }
    }
    unreachable!("the loop returns on the last attempt")
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
fn relay_response(upstream: reqwest::Response) -> Response {
    let status = upstream.status();
    let headers = passthrough_headers(upstream.headers());
    let mut response = Response::new(Body::from_stream(guard_idle(upstream.bytes_stream())));
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
async fn fold_response(upstream: reqwest::Response) -> Result<Response, BridgeError> {
    let headers = passthrough_headers(upstream.headers());
    let mut body = Box::pin(upstream.bytes_stream());
    let mut decoder = upstream::SseDecoder::default();
    let mut output = Vec::new();
    let mut completed = None;

    loop {
        let chunk = match tokio::time::timeout(upstream::BODY_IDLE_TIMEOUT, body.next()).await {
            Ok(Some(Ok(chunk))) => Some(chunk),
            Ok(Some(Err(error))) => {
                return Err(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    format!("the Codex response body failed mid-stream: {error}"),
                ));
            }
            Ok(None) => None,
            Err(_) => {
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
}
