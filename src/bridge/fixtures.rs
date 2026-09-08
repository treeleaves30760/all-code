//! Loader for the captured Codex corpus in `tests/fixtures/codex/`.
//!
//! The corpus is the specification. It was taken off the working bridge with
//! `CCP_TRAFFIC_LOG=1` and holds, for each request, what a client sent, what
//! went upstream, what came back, and what the client was answered. A
//! translator that reproduces those from those is correct on everything that
//! was exercised, which is why every surface's tests replay them rather than
//! asserting against shapes someone typed out.
//!
//! It lives in `tests/` because that is where fixture data belongs, but it is
//! loaded from here: alc has no library target, so `tests/*.rs` cannot see
//! `crate::bridge`, and every golden test has to be a `#[cfg(test)] mod` inside
//! `src/bridge/`.
//!
//! Five cases, one directory each, named for what they exercise:
//!
//! | Case | Exercises |
//! | --- | --- |
//! | `messages-tool-call-streaming` | `/v1/messages`, streaming, one tool, WebSocket lane, and the golden Anthropic event stream in `events/` |
//! | `messages-count-tokens` | `/v1/messages/count_tokens`, which never reaches upstream |
//! | `chat-completions-nonstreaming` | `/v1/chat/completions`, HTTP lane, `stream: true` upstream aggregated into one completion |
//! | `responses-passthrough-input-list` | `/v1/responses` forwarded verbatim; upstream 400 `Store must be set to false` |
//! | `responses-passthrough-input-string` | the same, refused with `Input must be a list` |
//!
//! Files inside a case keep the capture's own names; the accessors below find
//! them by suffix. `-010-` is the client's request, `-020-` the upstream
//! request, `-021-`/`-022-` its headers and transport, `-030-` the upstream
//! response headers and status, `-032-` its body (`.sse` when it streamed,
//! `.json` when it failed), `-050-` the answer sent back to the client, and
//! `events/` the interleaved `-040-` upstream and `-050-` downstream frames.
//!
//! Accounts and session state are redacted (`safety_identifier`,
//! `x-codex-turn-state`, request ids, `prompt_cache_key`); the capture itself
//! had already masked the bearer token and account id header.

use std::path::PathBuf;

use serde_json::Value;

/// One captured request/response exchange.
pub(crate) struct Case {
    name: String,
    dir: PathBuf,
}

impl Case {
    /// Loads a case by directory name. Panics with the available names when it
    /// is not one of them — a typo in a test should not read as a missing file.
    pub(crate) fn load(name: &str) -> Self {
        let dir = fixtures_root().join(name);
        assert!(
            dir.is_dir(),
            "no captured case named {name:?}; the corpus holds {:?}",
            case_names()
        );
        Self {
            name: name.to_owned(),
            dir,
        }
    }

    /// `*-000-metadata.json`: the surface (`kind`), path, model and the
    /// client's own request headers.
    pub(crate) fn metadata(&self) -> Value {
        self.json("-000-metadata.json")
    }

    /// The client's request, whichever of the three surfaces it arrived on.
    pub(crate) fn client_request(&self) -> Value {
        for suffix in [
            "-010-anthropic-request.json",
            "-010-openai-responses-request.json",
            "-010-openai-chat-completions-request.json",
        ] {
            if let Some(path) = self.find(suffix) {
                return read_json(&path);
            }
        }
        panic!("{}: no client request captured", self.name);
    }

    /// The body the bridge posted to chatgpt.com.
    ///
    /// Compare against this as a [`Value`], not as bytes: the capture is
    /// re-serialised with sorted keys and two-space indent, so it is the shape
    /// that is authoritative and not the byte order.
    pub(crate) fn upstream_request(&self) -> Value {
        self.json("-020-upstream-request.json")
    }

    /// `*-021-upstream-request-metadata.json` (HTTP lane) or
    /// `*-022-upstream-websocket-metadata.json` (WebSocket lane): the headers
    /// actually sent, and which transport carried them.
    pub(crate) fn upstream_metadata(&self) -> Value {
        self.find("-021-upstream-request-metadata.json")
            .or_else(|| self.find("-022-upstream-websocket-metadata.json"))
            .map(|path| read_json(&path))
            .unwrap_or_else(|| panic!("{}: no upstream metadata captured", self.name))
    }

    /// The raw upstream SSE. Panics on the cases that failed before streaming
    /// — use [`Case::upstream_body`] for those.
    pub(crate) fn upstream_sse(&self) -> String {
        let path = self
            .find("-032-upstream-response-body.sse")
            .unwrap_or_else(|| panic!("{}: this case has no SSE body", self.name));
        std::fs::read_to_string(&path).expect("the fixture is readable")
    }

    /// The upstream's non-streaming body, which in this corpus is always an
    /// error (`{"detail": …}`).
    pub(crate) fn upstream_body(&self) -> Value {
        self.json("-032-upstream-response-body.json")
    }

    /// `*-030-upstream-response-headers.json`, including the `status` the
    /// upstream answered with.
    pub(crate) fn upstream_response_headers(&self) -> Value {
        self.json("-030-upstream-response-headers.json")
    }

    /// Upstream events in arrival order, unwrapped from their SSE framing.
    pub(crate) fn upstream_events(&self) -> Vec<Value> {
        self.events("-040-upstream-event.json")
    }

    /// The events the working bridge sent the client, in order. This is the
    /// golden output for a streaming surface.
    pub(crate) fn downstream_events(&self) -> Vec<Value> {
        self.events("-050-downstream-event.json")
    }

    /// The non-streaming answer the working bridge sent the client.
    pub(crate) fn client_response(&self) -> Value {
        self.find("-050-openai-chat-completion-response.json")
            .map(|path| read_json(&path))
            .unwrap_or_else(|| panic!("{}: no non-streaming client response captured", self.name))
    }

    fn events(&self, suffix: &str) -> Vec<Value> {
        let mut paths: Vec<PathBuf> = match std::fs::read_dir(self.dir.join("events")) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(suffix))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        // The capture numbers files with a shared, zero-padded sequence, so
        // lexical order is arrival order across both directions.
        paths.sort();
        paths.iter().map(|path| read_json(path)).collect()
    }

    fn json(&self, suffix: &str) -> Value {
        let path = self
            .find(suffix)
            .unwrap_or_else(|| panic!("{}: no file ending {suffix}", self.name));
        read_json(&path)
    }

    fn find(&self, suffix: &str) -> Option<PathBuf> {
        let mut matches: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(suffix))
            })
            .collect();
        matches.sort();
        // A retried request logs its upstream body twice under different
        // sequence numbers; the bodies are identical, so the first will do.
        matches.into_iter().next()
    }
}

/// Every captured case, by name.
pub(crate) fn case_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixtures_root())
        .expect("the corpus is checked in")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex")
}

fn read_json(path: &std::path::Path) -> Value {
    let raw =
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_corpus_covers_all_three_surfaces() {
        assert_eq!(
            case_names(),
            [
                "chat-completions-nonstreaming",
                "messages-count-tokens",
                "messages-tool-call-streaming",
                "responses-passthrough-input-list",
                "responses-passthrough-input-string",
            ]
        );
    }

    #[test]
    fn a_case_loads_its_client_request_upstream_request_and_events() {
        let case = Case::load("messages-tool-call-streaming");
        assert_eq!(case.metadata()["kind"], "messages");
        assert_eq!(case.client_request()["model"], "gpt-5.6-terra");
        assert_eq!(case.upstream_request()["store"], false);
        assert_eq!(case.upstream_metadata()["transport"], "websocket");
        assert_eq!(case.upstream_events().len(), 17);
        assert_eq!(case.downstream_events().len(), 16);
        assert_eq!(case.downstream_events()[0]["event"], "message_start");
        assert_eq!(
            case.downstream_events().last().unwrap()["event"],
            "message_stop"
        );
    }

    #[test]
    fn the_passthrough_cases_send_the_client_body_upstream_unchanged() {
        for name in [
            "responses-passthrough-input-list",
            "responses-passthrough-input-string",
        ] {
            let case = Case::load(name);
            assert_eq!(case.client_request(), case.upstream_request(), "{name}");
            assert_eq!(case.upstream_response_headers()["status"], 400);
        }
        assert_eq!(
            Case::load("responses-passthrough-input-list").upstream_body()["detail"],
            "Store must be set to false"
        );
        assert_eq!(
            Case::load("responses-passthrough-input-string").upstream_body()["detail"],
            "Input must be a list"
        );
    }

    #[test]
    fn the_chat_completion_id_is_the_upstream_response_id_reprefixed() {
        let case = Case::load("chat-completions-nonstreaming");
        let completion = case.client_response();
        let sse = case.upstream_sse();
        let response_id = sse
            .split("\"id\":\"resp_")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("the stream names its response");
        assert_eq!(completion["id"], format!("chatcmpl-{response_id}"));
        assert_eq!(completion["object"], "chat.completion");
        assert_eq!(completion["choices"][0]["message"]["content"], "chat-ok");
    }
}
