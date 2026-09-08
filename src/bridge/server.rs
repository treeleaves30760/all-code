//! Routing.
//!
//! Four routes and a health check, which is the whole HTTP surface alc's eight
//! agents touch. `/healthz` exists because `launch::Bridge` polls it to decide
//! the bridge came up; it must answer 200 with no credentials and no upstream
//! contact, or a bad `auth.json` turns into "the adapter did not become ready"
//! instead of a message naming the file.

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};

use super::{BridgeState, chat, messages, responses};

pub(crate) fn router(state: Arc<BridgeState>) -> Router {
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/messages", post(messages::handle_messages))
        .route(
            "/v1/messages/count_tokens",
            post(messages::handle_count_tokens),
        );

    // Mirrors the vendored bridge's CCP_CODEX_RESPONSES_API: Claude Code's
    // plan does not set it, and these routes then 404 rather than sitting
    // there answering an agent that was never meant to reach them.
    if state.config.responses_api {
        router = router
            .route("/v1/responses", post(responses::handle))
            .route("/v1/chat/completions", post(chat::handle));
    }

    // A coding agent's turn carries whatever it has read: a long session with
    // several files in context passes a couple of megabytes without being
    // remarkable, and axum's 2 MiB default would refuse it outright. The
    // bridge this replaces has no such cap on these routes, so a limit here
    // would be a regression that only shows up once someone is deep into real
    // work - and it would arrive as bare text rather than an error envelope
    // the agent can parse. The upstream decides what is too large.
    router
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(state)
}

/// Liveness only. Deliberately says nothing about credentials: a 200 here
/// means the port is serving, and every other question is answered by the
/// request that asks it.
async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeConfig;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn state(responses_api: bool) -> Arc<BridgeState> {
        Arc::new(
            BridgeState::new(BridgeConfig {
                auth_file: std::path::PathBuf::from("/nonexistent/auth.json"),
                effort: None,
                responses_api,
            })
            .expect("the state builds without touching the auth file"),
        )
    }

    async fn status(responses_api: bool, method: &str, path: &str) -> StatusCode {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        router(state(responses_api))
            .oneshot(request)
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn healthz_answers_before_any_credential_is_read() {
        assert_eq!(status(false, "GET", "/healthz").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_openai_surfaces_are_absent_unless_the_plan_asked_for_them() {
        assert_eq!(
            status(false, "POST", "/v1/responses").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(false, "POST", "/v1/chat/completions").await,
            StatusCode::NOT_FOUND
        );
    }
}
