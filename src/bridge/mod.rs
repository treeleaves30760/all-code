//! alc's own Codex bridge.
//!
//! The bridge alc ships today is a vendored crate serving five providers; alc
//! reaches one of them over three of its wire surfaces. When `gpt-6-astra`
//! could not be routed, the fix was two entries in two hard-coded allowlists
//! inside code alc could not edit. This module is the answer to that: the same
//! contract, in the tree, small enough to read in an afternoon.
//!
//! It is deliberately not the default. `ALC_BRIDGE=native` selects it;
//! everything else keeps the vendored crate, because that one currently works.
//!
//! ## Shape of the thing
//!
//! Three request surfaces ([`messages`], [`responses`], [`chat`]) fan into one
//! upstream ([`upstream`]): `POST https://chatgpt.com/backend-api/codex/responses`,
//! answering Server-Sent Events. [`auth`] supplies the bearer token,
//! [`server`] does the routing. Nothing else is in here, and nothing else
//! should be.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use tokio::net::TcpListener;

use crate::config::ReasoningEffort;

pub(crate) mod auth;
pub(crate) mod chat;
pub(crate) mod messages;
pub(crate) mod responses;
pub(crate) mod server;
pub(crate) mod upstream;

#[cfg(test)]
pub(crate) mod fixtures;

/// What a bridge session needs to serve one agent.
///
/// The bridge this replaced took these through process environment
/// variables, which forced `launch.rs` into an `unsafe { set_var }` on the
/// launch path. Running in-process and owning the code, alc just hands them
/// over.
#[derive(Debug, Clone)]
pub(crate) struct BridgeConfig {
    /// The Codex CLI's `auth.json`, already resolved (`CODEX_HOME`, then
    /// `~/.codex`). The bridge reads and rotates it; it never runs a login.
    pub auth_file: PathBuf,
    /// Pinned effort for the agents that pick one at launch. Always `None` for
    /// Claude Code, which sends its own effort on every request.
    ///
    /// Wider than the ladder chatgpt.com accepts:
    /// [`ReasoningEffort::Ultra`] has no upstream spelling and must be clamped
    /// to `max` on the way into [`upstream::Effort`], not forwarded.
    pub effort: Option<ReasoningEffort>,
    /// Whether `/v1/responses` and `/v1/chat/completions` are routed at all.
    /// Mirrors the vendored bridge's `CCP_CODEX_RESPONSES_API`: Claude Code's
    /// plan does not set it, and those routes then 404 rather than existing
    /// unused.
    pub responses_api: bool,
}

/// Everything a handler shares: config, credentials, and one HTTP client.
///
/// One client for the process, so connection reuse survives across a session's
/// requests — Codex turns arrive in bursts and a fresh TLS handshake per turn
/// is visible latency.
pub(crate) struct BridgeState {
    pub config: BridgeConfig,
    pub auth: auth::AuthManager,
    pub http: reqwest::Client,
}

impl BridgeState {
    pub(crate) fn new(config: BridgeConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(upstream::USER_AGENT)
            // No total-request timeout: a Codex turn legitimately streams for
            // minutes. Idle detection belongs to the stream reader, which can
            // tell "thinking" from "hung".
            .build()
            .context("failed to build the Codex bridge's HTTP client")?;
        let auth = auth::AuthManager::new(config.auth_file.clone());
        Ok(Self { config, auth, http })
    }
}

/// Serves the bridge on an already-bound listener until `shutdown` resolves.
///
/// Mirrors the vendored `claude_codex::server::serve_listener` so
/// `launch::Bridge::start` can pick between them without restructuring: the
/// port is reserved by the caller with the standard library, so the agent's
/// base URL is known before any runtime exists.
///
/// The state is built by the caller rather than here, so that a failure to
/// build it is reported before the launch announces the bridge as ready — a
/// broken client is a message worth reading, not a ten-second timeout.
pub(crate) async fn serve(
    listener: TcpListener,
    state: Arc<BridgeState>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    axum::serve(listener, server::router(state))
        .with_graceful_shutdown(shutdown)
        .await
        .context("the Codex bridge stopped serving")
}

/// A refusal, in the one shape every surface has to be able to render.
///
/// The three surfaces disagree about the JSON envelope but not about the
/// facts, so the facts live here and each surface renders them
/// ([`BridgeError::anthropic`], [`BridgeError::openai`]). Acceptance criterion
/// 5 — "degrades honestly" — is this type's whole job: `message` names the real
/// cause, never a generic upstream failure.
#[derive(Debug, Clone)]
pub(crate) struct BridgeError {
    pub status: StatusCode,
    /// The Anthropic/OpenAI error taxonomy value: `invalid_request_error`,
    /// `authentication_error`, `rate_limit_error`, `api_error`,
    /// `overloaded_error`.
    pub kind: &'static str,
    pub message: String,
}

impl BridgeError {
    pub(crate) fn new(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
        }
    }

    /// The client sent something this bridge cannot translate.
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request_error", message)
    }

    /// `auth.json` is missing, unparseable, or its refresh was refused. The
    /// message must say which, and say `codex login`.
    pub(crate) fn auth(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "authentication_error", message)
    }

    /// chatgpt.com answered, and what it answered was a failure. The upstream
    /// text is carried through verbatim — it is the only party that knows.
    pub(crate) fn upstream(status: StatusCode, message: impl Into<String>) -> Self {
        Self::new(status, "api_error", message)
    }

    /// `{"type":"error","error":{"type":…,"message":…}}` — Messages surface.
    pub(crate) fn anthropic(&self) -> Response {
        let body = json!({
            "type": "error",
            "error": { "type": self.kind, "message": self.message },
        });
        (self.status, axum::Json(body)).into_response()
    }

    /// `{"error":{"message":…,"type":…,"param":null,"code":null}}` — Responses
    /// and Chat Completions surfaces.
    pub(crate) fn openai(&self) -> Response {
        let body = json!({
            "error": {
                "message": self.message,
                "type": self.kind,
                "param": serde_json::Value::Null,
                "code": serde_json::Value::Null,
            },
        });
        (self.status, axum::Json(body)).into_response()
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for BridgeError {}

/// Serialises `value` into a `data:` frame, terminated by the blank line SSE
/// requires. Every surface streams, and every one of them got this wrong at
/// least once in the crate being replaced.
pub(crate) fn sse_frame(event: &str, value: &impl Serialize) -> String {
    let data = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned());
    format!("event: {event}\ndata: {data}\n\n")
}
