//! The consumer Cloud Code endpoint agy itself talks to.
//!
//! Requests present themselves as agy: the server chooses which models an
//! account may drive by the client's User-Agent, and any other shape gets a
//! smaller catalog and a 404 for models outside it (KohakuTerrarium's
//! finding). Everything else here is policy: one project lookup per token,
//! a guard against the login changing mid-request, and retries that stop
//! the moment the first byte of an answer has been read.

#![allow(dead_code, reason = "wired in by Plan C Tasks 7-12")]

use std::collections::VecDeque;
use std::time::Duration;

use axum::http::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};

use super::BridgeError;
use super::agy_auth::{AgyAuthError, AgyCredentials, AgyToken, Fingerprint};
use super::upstream::{BODY_IDLE_TIMEOUT, SseDecoder, SseFrame};

/// agy's own client identity, captured from agy 1.3.4 on 2026-10-14. Bumped
/// by hand, together, when agy moves.
pub(crate) const AGY_VERSION: &str = "1.3.4";
pub(crate) const AGY_CHANGELIST: &str = "998275202";

pub(crate) const CODE_ASSIST_BASE: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal:";
pub(crate) const LOAD_CODE_ASSIST_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
pub(crate) const QUOTA_SUMMARY_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
pub(crate) const X_GOOG_API_CLIENT: &str = "google-api-go-client/0.5";
/// Sent with Claude models only, as agy does; Gemini models never see it.
pub(crate) const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Until the status line and headers, and for a whole JSON answer.
const HEADER_TIMEOUT: Duration = Duration::from_secs(90);
/// JSON answers (project, quota, refusals) are a few KiB.
pub(crate) const BODY_LIMIT: usize = 4 * 1024 * 1024;
/// One SSE event. A model part larger than this is not one.
pub(crate) const EVENT_LIMIT: usize = 1024 * 1024;
const MAX_RETRIES: u32 = 3;
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(30);
/// How many times the project lookup is repeated when the login changes
/// under it, before the request is refused.
const SWITCH_RETRIES: u32 = 3;
/// The longest refusal reason relayed, in characters.
const REASON_LIMIT: usize = 160;

/// The only methods alc calls on the endpoint. An enum rather than a string,
/// so nothing can be appended to the pinned base but these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    LoadCodeAssist,
    StreamGenerateContent,
}

impl Method {
    fn path(self) -> &'static str {
        match self {
            Self::LoadCodeAssist => "loadCodeAssist",
            Self::StreamGenerateContent => "streamGenerateContent?alt=sse",
        }
    }
}

/// The running platform, in agy's spelling. The spike found the Linux
/// User-Agent accepted byte-for-byte like the Windows one, so alc tells the
/// truth rather than pretending to be one platform everywhere.
pub(crate) fn os_type() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

pub(crate) fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

pub(crate) fn user_agent() -> String {
    format!(
        "antigravity/cli/{AGY_VERSION} (aidev_client; os_type={}; arch={}; cl={AGY_CHANGELIST}; auth_method=consumer)",
        os_type(),
        arch()
    )
}

/// `WINDOWS_AMD64`, `DARWIN_ARM64`, `LINUX_AMD64`, …
pub(crate) fn platform() -> String {
    format!(
        "{}_{}",
        os_type().to_ascii_uppercase(),
        arch().to_ascii_uppercase()
    )
}

pub(crate) fn load_code_assist_body() -> Value {
    json!({"metadata": {"ideType": "ANTIGRAVITY", "platform": platform(), "pluginType": "GEMINI"}})
}

/// `cloudaicompanionProject`, which arrives as a string or as `{id}`.
pub(crate) fn project_from(answer: &Value) -> Option<String> {
    let project = answer.get("cloudaicompanionProject")?;
    let id = match project {
        Value::String(id) => id.as_str(),
        other => other.get("id")?.as_str()?,
    };
    (!id.trim().is_empty()).then(|| id.to_owned())
}

/// The host requests go to: Google, or a loopback stand-in in a debug build
/// (`ALC_AGY_API_BASE`, for `tests/cli.rs`). Compiled out of release builds.
fn base_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("ALC_AGY_API_BASE")
        && !base.is_empty()
    {
        return format!("{}/v1internal:", base.trim_end_matches('/'));
    }
    CODE_ASSIST_BASE.to_owned()
}

/// One line of an upstream refusal: `STATUS: message`, first line only,
/// control characters blanked, at most 160 characters. Never the body
/// itself: a body that is not Google's error shape gives no reason at all.
pub(crate) fn reason(body: &[u8]) -> String {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let text = |pointer: &str| {
        parsed
            .as_ref()
            .and_then(|value| value.pointer(pointer))
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
    };
    let joined = match (text("/error/status"), text("/error/message")) {
        (Some(status), Some(message)) => format!("{status}: {message}"),
        (None, Some(message)) => message.to_owned(),
        (Some(status), None) => status.to_owned(),
        (None, None) => "no reason given".to_owned(),
    };
    let line: String = joined
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let line = line.trim();
    if line.chars().count() <= REASON_LIMIT {
        line.to_owned()
    } else {
        let kept: String = line.chars().take(REASON_LIMIT - 1).collect();
        format!("{kept}…")
    }
}

/// agy's login problems, in the bridge's error shape.
pub(crate) fn auth_error(error: AgyAuthError) -> BridgeError {
    match error {
        AgyAuthError::RefreshBusy => BridgeError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "overloaded_error",
            error.message(),
        ),
        _ => BridgeError::auth(error.message()),
    }
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes];
    let _ = getrandom::fill(&mut buffer);
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn uuid4() -> String {
    let mut bytes = [0_u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Why an attempt ended without an answer worth relaying.
enum Attempt {
    /// A 401 for the token with this fingerprint: renew past it, once.
    Unauthorized(Fingerprint),
    /// A 403: the account itself, which neither a retry nor a renewal fixes.
    Forbidden(String),
    Retryable {
        error: BridgeError,
        retry_after: Option<Duration>,
    },
    Final(BridgeError),
}

pub(crate) struct AgyUpstream {
    credentials: AgyCredentials,
    http: reqwest::Client,
    base: String,
    backoff_base: Duration,
    /// The Cloud Code project of the last token seen. KohakuTerrarium asks on
    /// every turn; one ask per token is enough, and a token change - a
    /// renewal or another account - asks again.
    project: std::sync::Mutex<Option<(Fingerprint, String)>>,
}

impl AgyUpstream {
    pub(crate) fn new(credentials: AgyCredentials) -> anyhow::Result<Self> {
        Self::with_base(credentials, base_url())
    }

    pub(crate) fn with_base(credentials: AgyCredentials, base: String) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent())
            // A redirect off the pinned host would carry the bearer token
            // somewhere alc never chose.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|error| {
                anyhow::anyhow!("failed to build the Antigravity HTTP client: {error}")
            })?;
        Ok(Self {
            credentials,
            http,
            base,
            backoff_base: BACKOFF_BASE,
            project: std::sync::Mutex::new(None),
        })
    }

    #[cfg(test)]
    pub(crate) fn without_backoff(mut self) -> Self {
        self.backoff_base = Duration::ZERO;
        self
    }

    pub(crate) fn credentials(&self) -> &AgyCredentials {
        &self.credentials
    }

    /// Opens a generation stream: the project lookup, then the request, each
    /// retried on 429, 5xx and transport failures, a 401 renewed past once.
    ///
    /// It returns as soon as a 200's headers are in, before a byte of the
    /// answer has been read, so this is the only place a retry can happen:
    /// past it a failure is the client's to see, because replaying a turn
    /// it has half received would duplicate it.
    pub(crate) async fn stream_generate(
        &self,
        wire_model: &str,
        request: Value,
    ) -> Result<reqwest::Response, BridgeError> {
        let prompt_id = uuid4();
        let mut renewed = false;
        let mut attempt = 0_u32;
        loop {
            let token = self.credentials.token().await.map_err(auth_error)?;
            let outcome = match self.project_for(token).await {
                Err(outcome) => outcome,
                Ok((token, project)) => {
                    let envelope = json!({
                        "project": project,
                        "model": wire_model,
                        "userPromptId": prompt_id,
                        "request": request,
                    });
                    match self
                        .post(
                            Method::StreamGenerateContent,
                            &token,
                            Some(wire_model),
                            &envelope,
                        )
                        .await
                    {
                        Ok(response) if response.status() == StatusCode::OK => {
                            return Ok(response);
                        }
                        Ok(response) => refusal(response, &token, Some(wire_model)).await,
                        Err(outcome) => outcome,
                    }
                }
            };
            match outcome {
                Attempt::Unauthorized(rejected) if !renewed => {
                    renewed = true;
                    self.forget_project();
                    self.credentials
                        .renew_after_rejection(rejected)
                        .await
                        .map_err(auth_error)?;
                }
                Attempt::Unauthorized(_) => {
                    self.forget_project();
                    return Err(BridgeError::auth(
                        "Antigravity refused agy's login even after agy renewed it; \
                         run `agy` and sign in again",
                    ));
                }
                Attempt::Forbidden(reason) => {
                    self.forget_project();
                    return Err(BridgeError::new(
                        StatusCode::FORBIDDEN,
                        "permission_error",
                        format!(
                            "Google refused this Antigravity account ({reason}); the account may \
                             be restricted, so alc does not retry - check it in agy"
                        ),
                    ));
                }
                Attempt::Retryable { error, retry_after } if attempt < MAX_RETRIES => {
                    match self.delay(attempt, retry_after) {
                        Some(delay) => {
                            tokio::time::sleep(delay).await;
                            attempt += 1;
                        }
                        None => return Err(error),
                    }
                }
                Attempt::Retryable { error, .. } | Attempt::Final(error) => return Err(error),
            }
        }
    }

    fn forget_project(&self) {
        *self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// The project for `token`, asked once per token. The login is read again
    /// after the answer: if it changed while the request was out, the answer
    /// may belong to another account, so it is asked again (three times at
    /// most) rather than sent under the wrong token.
    async fn project_for(&self, mut token: AgyToken) -> Result<(AgyToken, String), Attempt> {
        let cached = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some((fingerprint, project)) = cached
            && fingerprint == token.fingerprint()
        {
            return Ok((token, project));
        }
        for _ in 0..=SWITCH_RETRIES {
            let response = self
                .post(
                    Method::LoadCodeAssist,
                    &token,
                    None,
                    &load_code_assist_body(),
                )
                .await?;
            if response.status() != StatusCode::OK {
                return Err(refusal(response, &token, None).await);
            }
            let body = read_capped(response).await?;
            let Ok(answer) = serde_json::from_slice::<Value>(&body) else {
                return Err(Attempt::Final(BridgeError::upstream(
                    StatusCode::BAD_GATEWAY,
                    "Antigravity's project lookup answered with something that is not JSON \
                     (answer not understood)",
                )));
            };
            let Some(project) = project_from(&answer) else {
                return Err(Attempt::Final(BridgeError::new(
                    StatusCode::FORBIDDEN,
                    "permission_error",
                    "this Google account is not onboarded to Antigravity yet (no Cloud Code \
                     project); open agy once and finish its setup",
                )));
            };
            let after = self
                .credentials
                .read()
                .await
                .map_err(|error| Attempt::Final(auth_error(error)))?;
            if after.fingerprint() == token.fingerprint() {
                *self
                    .project
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some((token.fingerprint(), project.clone()));
                return Ok((token, project));
            }
            token = after;
        }
        Err(Attempt::Final(BridgeError::auth(
            "agy's login changed while alc was using it (credentials_changed_during_request); \
             retry the request",
        )))
    }

    async fn post(
        &self,
        method: Method,
        token: &AgyToken,
        wire_model: Option<&str>,
        body: &Value,
    ) -> Result<reqwest::Response, Attempt> {
        let headers = headers(token, wire_model).map_err(Attempt::Final)?;
        let request = self
            .http
            .post(format!("{}{}", self.base, method.path()))
            .headers(headers)
            .body(body.to_string());
        match tokio::time::timeout(HEADER_TIMEOUT, request.send()).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(Attempt::Retryable {
                error: BridgeError::new(
                    StatusCode::BAD_GATEWAY,
                    "api_error",
                    format!("could not reach Antigravity: {}", describe(&error)),
                ),
                retry_after: None,
            }),
            Err(_) => Err(Attempt::Retryable {
                error: BridgeError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "api_error",
                    format!(
                        "Antigravity did not answer within {}s",
                        HEADER_TIMEOUT.as_secs()
                    ),
                ),
                retry_after: None,
            }),
        }
    }

    /// Exponential from the base, capped, with jitter in [½, 1] of it; a
    /// `Retry-After` longer than the cap is a refusal to wait, not a wait.
    fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
        let exponential = self
            .backoff_base
            .saturating_mul(1_u32 << attempt.min(16))
            .min(BACKOFF_CAP);
        let mut bytes = [0_u8; 4];
        let _ = getrandom::fill(&mut bytes);
        let fraction = f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX);
        let jittered = exponential.mul_f64(0.5 + fraction / 2.0);
        match retry_after {
            Some(wait) if wait > BACKOFF_CAP => None,
            Some(wait) => Some(wait.max(jittered)),
            None => Some(jittered),
        }
    }
}

fn headers(token: &AgyToken, wire_model: Option<&str>) -> Result<HeaderMap, BridgeError> {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert(
        "x-goog-api-client",
        HeaderValue::from_static(X_GOOG_API_CLIENT),
    );
    let mut bearer =
        HeaderValue::from_str(&format!("Bearer {}", token.access_token())).map_err(|_| {
            BridgeError::auth(
                "agy's stored token cannot be sent as a header; run `agy` and sign in again",
            )
        })?;
    bearer.set_sensitive(true);
    headers.insert("authorization", bearer);
    if wire_model.is_some_and(|model| model.starts_with("claude-")) {
        headers.insert(
            "anthropic-beta",
            HeaderValue::from_static(INTERLEAVED_THINKING_BETA),
        );
    }
    Ok(headers)
}

/// A non-200 answer, classified. Reads at most [`BODY_LIMIT`] of it, and
/// keeps one line of it, with the token that was sent blanked out in case
/// the server echoed it back.
async fn refusal(
    response: reqwest::Response,
    token: &AgyToken,
    wire_model: Option<&str>,
) -> Attempt {
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| retry_after_from(value, chrono::Utc::now()));
    let body = read_capped(response).await.ok().unwrap_or_default();
    let secret = token.access_token();
    let body = if secret.is_empty() {
        body
    } else {
        String::from_utf8_lossy(&body)
            .replace(secret, "[redacted]")
            .into_bytes()
    };
    let reason = reason(&body);
    match status.as_u16() {
        401 => Attempt::Unauthorized(token.fingerprint()),
        403 => Attempt::Forbidden(reason),
        429 => Attempt::Retryable {
            error: BridgeError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                format!(
                    "Antigravity is rate limiting this account or its quota is used up \
                     ({reason}); `alc usage` shows when it resets"
                ),
            ),
            retry_after,
        },
        500..=599 => Attempt::Retryable {
            error: BridgeError::new(
                StatusCode::BAD_GATEWAY,
                if status == StatusCode::SERVICE_UNAVAILABLE {
                    "overloaded_error"
                } else {
                    "api_error"
                },
                format!("Antigravity failed the request with {status} ({reason})"),
            ),
            retry_after,
        },
        200..=299 => Attempt::Final(BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("Antigravity gave an unexpected answer ({status}) where alc expects 200"),
        )),
        300..=399 => Attempt::Final(BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            "Antigravity answered with a redirect, which alc never follows",
        )),
        404 => Attempt::Final(BridgeError::upstream(
            StatusCode::NOT_FOUND,
            match wire_model {
                Some(model) => format!(
                    "Antigravity does not serve {model} on this account ({reason}); \
                     `alc models --agy` lists what it does"
                ),
                None => format!("Antigravity has no project lookup at this address ({reason})"),
            },
        )),
        _ => Attempt::Final(BridgeError::new(
            status,
            "invalid_request_error",
            format!(
                "Antigravity refused the request ({}): {reason}",
                status.as_u16()
            ),
        )),
    }
}

/// `Retry-After` in either of its forms: delay-seconds, or an HTTP-date
/// (RFC 2822 parsing accepts the `GMT` zone it is written in), which becomes
/// the wait from `now`, zero when it has already passed.
fn retry_after_from(value: &str, now: chrono::DateTime<chrono::Utc>) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(
        date.with_timezone(&chrono::Utc)
            .signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO),
    )
}

/// A whole JSON answer, at most [`BODY_LIMIT`] of it, within
/// [`HEADER_TIMEOUT`]: a body that stalls is as good as none. A body that
/// breaks off or stalls is a transport failure, retryable on these
/// idempotent calls; one over the limit is final, since asking again would
/// get the same answer.
async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, Attempt> {
    let read = async {
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > BODY_LIMIT {
                        return Err(Attempt::Final(BridgeError::upstream(
                            StatusCode::BAD_GATEWAY,
                            "Antigravity sent an answer larger than 4 MiB, which is not one alc \
                             asked for",
                        )));
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => return Ok(body),
                Err(error) => {
                    return Err(Attempt::Retryable {
                        error: BridgeError::upstream(
                            StatusCode::BAD_GATEWAY,
                            format!("the Antigravity answer broke off: {}", describe(&error)),
                        ),
                        retry_after: None,
                    });
                }
            }
        }
    };
    tokio::time::timeout(HEADER_TIMEOUT, read)
        .await
        .unwrap_or_else(|_| {
            Err(Attempt::Retryable {
                error: BridgeError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "api_error",
                    format!(
                        "Antigravity did not finish its answer within {}s",
                        HEADER_TIMEOUT.as_secs()
                    ),
                ),
                retry_after: None,
            })
        })
}

fn describe(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(inner) = source {
        parts.push(inner.to_string());
        source = inner.source();
    }
    parts.join(": ")
}

/// What one SSE frame turned out to be.
#[derive(Debug)]
pub(crate) enum Decoded {
    Event(Value),
    Done,
    Empty,
}

/// One `data:` payload: unwrapped from its `response` envelope, refused when
/// it carries an `error` (never retried: the turn has started).
pub(crate) fn decode_event(data: &str) -> Result<Decoded, BridgeError> {
    let data = data.trim();
    if data.is_empty() {
        return Ok(Decoded::Empty);
    }
    if data == "[DONE]" {
        return Ok(Decoded::Done);
    }
    if data.len() > EVENT_LIMIT {
        return Err(BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            "Antigravity sent a stream event larger than 1 MiB",
        ));
    }
    let value: Value = serde_json::from_str(data).map_err(|error| {
        BridgeError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("Antigravity sent a stream event that is not JSON: {error}"),
        )
    })?;
    if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
        return Err(stream_error(error));
    }
    Ok(Decoded::Event(match value.get("response") {
        Some(inner) if inner.is_object() => inner.clone(),
        _ => value,
    }))
}

fn stream_error(error: &Value) -> BridgeError {
    let reason = reason(json!({ "error": error }).to_string().as_bytes());
    let code = error
        .get("code")
        .and_then(Value::as_u64)
        .and_then(|code| u16::try_from(code).ok())
        .unwrap_or(0);
    let message = format!("Antigravity reported an error mid-turn ({code}): {reason}");
    match code {
        429 => BridgeError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limit_error", message),
        401 => BridgeError::new(StatusCode::UNAUTHORIZED, "authentication_error", message),
        403 => BridgeError::new(StatusCode::FORBIDDEN, "permission_error", message),
        _ => BridgeError::upstream(StatusCode::BAD_GATEWAY, message),
    }
}

/// A generation stream, decoded into Gemini events as the bytes land.
pub(crate) struct GeminiEvents {
    response: reqwest::Response,
    decoder: SseDecoder,
    ready: VecDeque<SseFrame>,
    finished: bool,
}

impl GeminiEvents {
    pub(crate) fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            decoder: SseDecoder::default(),
            ready: VecDeque::new(),
            finished: false,
        }
    }

    /// The next Gemini event, `None` once the stream has ended. An error ends
    /// it too: nothing after it is delivered.
    pub(crate) async fn next(&mut self) -> Option<Result<Value, BridgeError>> {
        loop {
            // Frames already read are delivered even after the end of the
            // body, so a last frame without its blank line is not lost.
            while let Some(frame) = self.ready.pop_front() {
                match decode_event(&frame.data) {
                    Ok(Decoded::Event(event)) => return Some(Ok(event)),
                    Ok(Decoded::Done) => {
                        self.finished = true;
                        self.ready.clear();
                    }
                    Ok(Decoded::Empty) => {}
                    Err(error) => {
                        self.finished = true;
                        self.ready.clear();
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
                    if self.decoder.pending_bytes() > EVENT_LIMIT {
                        self.finished = true;
                        self.ready.clear();
                        return Some(Err(BridgeError::upstream(
                            StatusCode::BAD_GATEWAY,
                            "Antigravity sent a stream event larger than 1 MiB",
                        )));
                    }
                }
                Ok(Ok(None)) => {
                    self.finished = true;
                    let frames = self.decoder.finish();
                    self.ready.extend(frames);
                }
                Ok(Err(error)) => {
                    self.finished = true;
                    self.ready.clear();
                    return Some(Err(BridgeError::upstream(
                        StatusCode::BAD_GATEWAY,
                        format!("the Antigravity stream broke: {}", describe(&error)),
                    )));
                }
                Err(_) => {
                    self.finished = true;
                    self.ready.clear();
                    return Some(Err(BridgeError::new(
                        StatusCode::GATEWAY_TIMEOUT,
                        "api_error",
                        format!(
                            "Antigravity sent nothing for {}s; the stream is stalled",
                            BODY_IDLE_TIMEOUT.as_secs()
                        ),
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::http::StatusCode;
    use serde_json::json;

    use super::*;
    use crate::bridge::agy_auth::test_support::{MemoryStore, agy_calls, credential, fake_agy};
    use crate::bridge::agy_auth::{AgyAuthError, CredentialStore, RawCredential, SystemStore};
    use crate::bridge::agy_fake::{FakeCodeAssist, Reply};

    const DONE: &str = "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"OK\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":1,\"totalTokenCount\":4}}}\n\n";

    fn project() -> Reply {
        Reply::json(200, json!({"cloudaicompanionProject": "test-project"}))
    }

    fn memory(token: &str) -> AgyCredentials {
        AgyCredentials::with_parts(
            Arc::new(MemoryStore::with_file(credential(token, 3600))),
            std::env::temp_dir().join("alc-agy-test-unused.lock"),
            None,
        )
    }

    fn upstream(fake: &FakeCodeAssist, credentials: AgyCredentials) -> AgyUpstream {
        AgyUpstream::with_base(credentials, fake.base.clone())
            .unwrap()
            .without_backoff()
    }

    fn request() -> serde_json::Value {
        json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]})
    }

    #[tokio::test]
    async fn requests_carry_agys_identity_and_the_envelope() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(DONE)]);
        let upstream = upstream(&fake, memory("tok-a"));
        let response = upstream
            .stream_generate("claude-sonnet-5-5-low", request())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let requests = fake.requests();
        let (load, generate) = (&requests[0], &requests[1]);
        assert_eq!(load.path, "/v1internal:loadCodeAssist");
        assert_eq!(
            load.body,
            json!({"metadata": {"ideType": "ANTIGRAVITY", "platform": platform(), "pluginType": "GEMINI"}})
        );
        assert_eq!(generate.path, "/v1internal:streamGenerateContent?alt=sse");
        for recorded in [load, generate] {
            assert!(
                recorded.headers.contains("authorization: bearer tok-a"),
                "{}",
                recorded.headers
            );
            assert!(recorded.headers.contains(&format!(
                "user-agent: {}",
                user_agent().to_ascii_lowercase()
            )));
            assert!(
                recorded
                    .headers
                    .contains("x-goog-api-client: google-api-go-client/0.5")
            );
        }
        assert!(
            generate
                .headers
                .contains("anthropic-beta: interleaved-thinking-2025-05-14")
        );
        assert!(!load.headers.contains("anthropic-beta"));
        assert_eq!(generate.body["project"], "test-project");
        assert_eq!(generate.body["model"], "claude-sonnet-5-5-low");
        assert_eq!(generate.body["request"], request());
        assert_eq!(generate.body["userPromptId"].as_str().unwrap().len(), 36);
    }

    #[test]
    fn the_user_agent_is_agys_for_this_platform() {
        assert_eq!(
            user_agent(),
            format!(
                "antigravity/cli/1.3.4 (aidev_client; os_type={}; arch={}; cl=998275202; auth_method=consumer)",
                os_type(),
                arch()
            )
        );
        assert!(["windows", "darwin", "linux"].contains(&os_type()));
        assert!(["amd64", "arm64"].contains(&arch()));
        assert_eq!(
            platform(),
            format!("{}_{}", os_type().to_uppercase(), arch().to_uppercase())
        );
    }

    #[tokio::test]
    async fn a_gemini_model_carries_no_anthropic_beta() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(DONE)]);
        upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        assert!(!fake.requests()[1].headers.contains("anthropic-beta"));
    }

    #[tokio::test]
    async fn the_project_is_asked_once_per_token_and_again_after_a_change() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(DONE)]);
        let store = Arc::new(MemoryStore::with_file(credential("tok-a", 3600)));
        let credentials = AgyCredentials::with_parts(
            store.clone(),
            std::env::temp_dir().join("unused.lock"),
            None,
        );
        let upstream = upstream(&fake, credentials);
        upstream
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        upstream
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        assert_eq!(fake.count("loadCodeAssist"), 1);
        store.set_file(credential("tok-b", 3600));
        upstream
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        assert_eq!(fake.count("loadCodeAssist"), 2);
        assert!(
            fake.requests()[3]
                .headers
                .contains("authorization: bearer tok-b")
        );
    }

    /// A login that changes on every read: another agy signing in and out.
    struct Flapping(AtomicUsize);
    impl CredentialStore for Flapping {
        fn keyring(&self) -> Result<Option<RawCredential>, AgyAuthError> {
            Ok(None)
        }
        fn file(&self) -> Result<Option<RawCredential>, AgyAuthError> {
            let read = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Some(RawCredential::new(credential(
                &format!("tok-{read}"),
                3600,
            ))))
        }
    }

    #[tokio::test]
    async fn a_login_that_keeps_changing_during_the_project_lookup_is_refused() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(DONE)]);
        let credentials = AgyCredentials::with_parts(
            Arc::new(Flapping(AtomicUsize::new(0))),
            std::env::temp_dir().join("unused.lock"),
            None,
        );
        let error = upstream(&fake, credentials)
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert!(
            error.message.contains("credentials_changed_during_request"),
            "{error}"
        );
        assert_eq!(
            fake.count("loadCodeAssist"),
            4,
            "the first ask and three retries"
        );
        assert_eq!(fake.count("streamGenerateContent"), 0);
    }

    #[tokio::test]
    async fn a_401_renews_once_and_retries_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("login");
        std::fs::write(&file, credential("tok-a", 3600)).unwrap();
        let agy = fake_agy(
            dir.path(),
            Some(credential("tok-b", 3600).as_slice()),
            &file,
            0,
        );
        let credentials = AgyCredentials::with_parts(
            Arc::new(SystemStore::file_only(file)),
            dir.path().join("lock"),
            Some(agy),
        );
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::json(401, json!({})), Reply::sse(DONE)],
        );
        upstream(&fake, credentials)
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        let generates: Vec<_> = fake
            .requests()
            .into_iter()
            .filter(|r| r.path.contains("streamGenerate"))
            .collect();
        assert_eq!(generates.len(), 2);
        assert!(generates[1].headers.contains("authorization: bearer tok-b"));
        assert_eq!(
            fake.count("loadCodeAssist"),
            2,
            "a 401 drops the cached project"
        );
        assert_eq!(agy_calls(dir.path()), 1);
    }

    #[tokio::test]
    async fn a_second_401_is_not_retried_and_names_no_token() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("login");
        std::fs::write(&file, credential("tok-a", 3600)).unwrap();
        let agy = fake_agy(
            dir.path(),
            Some(credential("tok-b", 3600).as_slice()),
            &file,
            0,
        );
        let credentials = AgyCredentials::with_parts(
            Arc::new(SystemStore::file_only(file)),
            dir.path().join("lock"),
            Some(agy),
        );
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::json(401, json!({}))]);
        let upstream = upstream(&fake, credentials);
        let error = upstream
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(fake.count("streamGenerateContent"), 2);
        assert!(!error.message.contains("tok-"), "{error}");
        assert_eq!(agy_calls(dir.path()), 1);
        assert!(
            upstream.project.lock().unwrap().is_none(),
            "any 401 drops the cached project"
        );
    }

    #[tokio::test]
    async fn a_401_on_the_project_lookup_renews_too() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("login");
        std::fs::write(&file, credential("tok-a", 3600)).unwrap();
        let agy = fake_agy(
            dir.path(),
            Some(credential("tok-b", 3600).as_slice()),
            &file,
            0,
        );
        let credentials = AgyCredentials::with_parts(
            Arc::new(SystemStore::file_only(file)),
            dir.path().join("lock"),
            Some(agy),
        );
        let fake = FakeCodeAssist::start(
            vec![Reply::json(401, json!({})), project()],
            vec![Reply::sse(DONE)],
        );
        upstream(&fake, credentials)
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        let requests = fake.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests[2].headers.contains("authorization: bearer tok-b"));
        assert_eq!(agy_calls(dir.path()), 1);
    }

    #[tokio::test]
    async fn an_expired_login_with_no_agy_never_reaches_google() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::sse(DONE)]);
        let credentials = AgyCredentials::with_parts(
            Arc::new(MemoryStore::with_file(credential("tok-a", -100))),
            std::env::temp_dir().join("unused.lock"),
            None,
        );
        let error = upstream(&fake, credentials)
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert!(error.message.contains("sign in"), "{error}");
        assert!(fake.requests().is_empty());
    }

    #[tokio::test]
    async fn a_403_is_neither_retried_nor_renewed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("login");
        std::fs::write(&file, credential("tok-a", 3600)).unwrap();
        let agy = fake_agy(
            dir.path(),
            Some(credential("tok-b", 3600).as_slice()),
            &file,
            0,
        );
        let credentials = AgyCredentials::with_parts(
            Arc::new(SystemStore::file_only(file)),
            dir.path().join("lock"),
            Some(agy),
        );
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::json(
                403,
                json!({"error": {"code": 403, "status": "PERMISSION_DENIED", "message": "The caller does not have permission"}}),
            )],
        );
        let error = upstream(&fake, credentials)
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.kind, "permission_error");
        assert!(error.message.contains("may be restricted"), "{error}");
        assert!(error.message.contains("PERMISSION_DENIED"), "{error}");
        assert_eq!(fake.count("loadCodeAssist"), 1);
        assert_eq!(fake.count("streamGenerateContent"), 1);
        assert_eq!(agy_calls(dir.path()), 0, "a 403 never runs agy");
    }

    #[tokio::test]
    async fn a_403_on_the_project_lookup_is_neither_retried_nor_renewed() {
        let fake = FakeCodeAssist::start(
            vec![Reply::json(
                403,
                json!({"error": {"code": 403, "status": "PERMISSION_DENIED", "message": "no"}}),
            )],
            vec![Reply::sse(DONE)],
        );
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.kind, "permission_error");
        assert!(error.message.contains("PERMISSION_DENIED: no"), "{error}");
        assert_eq!(fake.count("loadCodeAssist"), 1);
        assert_eq!(fake.count("streamGenerateContent"), 0);
    }

    #[tokio::test]
    async fn rate_limits_and_server_errors_are_retried_honouring_retry_after() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![
                Reply::json(429, json!({})).with_header("retry-after", "0"),
                Reply::json(503, json!({})),
                Reply::sse(DONE),
            ],
        );
        upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        assert_eq!(fake.count("streamGenerateContent"), 3);
    }

    /// The first live `loadCodeAssist` of the spike answered 503.
    #[tokio::test]
    async fn a_server_error_on_the_project_lookup_is_retried() {
        let fake = FakeCodeAssist::start(
            vec![
                Reply::json(503, json!({"error": {"status": "UNAVAILABLE"}})),
                project(),
            ],
            vec![Reply::sse(DONE)],
        );
        upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        assert_eq!(fake.count("loadCodeAssist"), 2);
        assert_eq!(fake.count("streamGenerateContent"), 1);
    }

    #[tokio::test]
    async fn project_lookup_retries_stop_after_three() {
        let fake = FakeCodeAssist::start(vec![Reply::json(500, json!({}))], vec![Reply::sse(DONE)]);
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert_eq!(fake.count("loadCodeAssist"), 4);
        assert_eq!(fake.count("streamGenerateContent"), 0);
    }

    #[tokio::test]
    async fn a_retry_after_past_the_cap_is_not_waited_for() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::json(429, json!({})).with_header("retry-after", "120")],
        );
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error.kind, "rate_limit_error");
        assert_eq!(fake.count("streamGenerateContent"), 1);
    }

    #[tokio::test]
    async fn retries_stop_after_three() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::json(500, json!({}))]);
        upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(fake.count("streamGenerateContent"), 4);
    }

    #[tokio::test]
    async fn an_unreachable_host_is_retried_then_reported() {
        // A port that was just free: nothing answers it.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let upstream = AgyUpstream::with_base(
            memory("tok-a"),
            format!("http://127.0.0.1:{port}/v1internal:"),
        )
        .unwrap()
        .without_backoff();
        let error = upstream
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(
            error.message.contains("could not reach Antigravity"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn other_4xx_is_not_retried_and_only_one_line_of_the_body_is_relayed() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::json(
                400,
                json!({"error": {"code": 400, "status": "INVALID_ARGUMENT", "message": "bad thing\nsecond line"}}),
            )],
        );
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(
            error.message.contains("INVALID_ARGUMENT: bad thing"),
            "{error}"
        );
        assert!(!error.message.contains("second line"), "{error}");
        assert_eq!(fake.count("streamGenerateContent"), 1);
    }

    #[tokio::test]
    async fn a_refusal_that_echoes_the_token_does_not_relay_it() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::json(
                400,
                json!({"error": {"status": "INVALID_ARGUMENT", "message": "bad header Bearer tok-secret-a"}}),
            )],
        );
        let error = upstream(&fake, memory("tok-secret-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert!(!error.message.contains("tok-secret-a"), "{error}");
        assert!(error.message.contains("bad header"), "{error}");
    }

    #[tokio::test]
    async fn a_redirect_is_refused_not_followed() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![
                Reply::json(302, json!({})).with_header("location", "http://127.0.0.1:9/elsewhere"),
            ],
        );
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert!(error.message.contains("redirect"), "{error}");
        assert_eq!(fake.requests().len(), 2);
    }

    #[tokio::test]
    async fn an_account_without_a_project_is_told_to_open_agy() {
        let fake = FakeCodeAssist::start(vec![Reply::json(200, json!({}))], vec![Reply::sse(DONE)]);
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert!(error.message.contains("open agy once"), "{error}");
        assert_eq!(fake.count("streamGenerateContent"), 0);
    }

    #[test]
    fn a_project_is_a_string_or_an_object_with_an_id() {
        assert_eq!(
            project_from(&json!({"cloudaicompanionProject": "p-1"})),
            Some("p-1".to_owned())
        );
        assert_eq!(
            project_from(&json!({"cloudaicompanionProject": {"id": "p-2"}})),
            Some("p-2".to_owned())
        );
        assert_eq!(project_from(&json!({"cloudaicompanionProject": ""})), None);
        assert_eq!(project_from(&json!({})), None);
    }

    #[test]
    fn the_captured_project_answer_names_its_project() {
        let answer: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/antigravity/load-code-assist.json"
        ))
        .unwrap();
        assert_eq!(project_from(&answer), Some("test-project".to_owned()));
    }

    #[test]
    fn a_reason_is_one_short_clean_line() {
        assert_eq!(
            reason(br#"{"error":{"status":"UNAVAILABLE","message":"try later"}}"#),
            "UNAVAILABLE: try later"
        );
        assert_eq!(reason(b"<html>nope</html>"), "no reason given");
        assert_eq!(reason(b""), "no reason given");
        let long = json!({"error": {"message": "x".repeat(500)}}).to_string();
        assert!(reason(long.as_bytes()).chars().count() <= 160);
        let escapes = json!({"error": {"message": "a\u{1b}[31mred\tb"}}).to_string();
        assert!(
            !reason(escapes.as_bytes()).chars().any(char::is_control),
            "{}",
            reason(escapes.as_bytes())
        );
    }

    #[test]
    fn prompt_ids_are_version_4_uuids() {
        let id = uuid4();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(["8", "9", "a", "b"].contains(&&id[19..20]));
        assert_ne!(id, uuid4());
        assert_eq!(random_hex(8).len(), 16);
    }

    #[test]
    fn stream_events_are_unwrapped_and_errors_and_oversize_refused() {
        let Decoded::Event(event) =
            decode_event(r#"{"response":{"candidates":[]},"traceId":"t"}"#).unwrap()
        else {
            panic!()
        };
        assert_eq!(event, json!({"candidates": []}));
        assert!(matches!(decode_event("[DONE]").unwrap(), Decoded::Done));
        assert!(matches!(decode_event("  ").unwrap(), Decoded::Empty));
        let error = decode_event(
            r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"quota"}}"#,
        )
        .unwrap_err();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(
            error.message.contains("RESOURCE_EXHAUSTED: quota"),
            "{error}"
        );
        assert!(decode_event(&"x".repeat(EVENT_LIMIT + 1)).is_err());
        assert!(decode_event("not json").is_err());
        let forbidden =
            decode_event(r#"{"error":{"code":403,"status":"PERMISSION_DENIED","message":"no"}}"#)
                .unwrap_err();
        assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
        assert_eq!(forbidden.kind, "permission_error");
        let unauthorized = decode_event(r#"{"error":{"code":401,"message":"no"}}"#).unwrap_err();
        assert_eq!(unauthorized.kind, "authentication_error");
    }

    #[tokio::test]
    async fn the_event_reader_yields_each_gemini_event_then_ends() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::sse(&format!(
                "data: {{\"response\":{{\"a\":1}}}}\n\n{DONE}data: [DONE]\n\n"
            ))],
        );
        let response = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        let mut events = GeminiEvents::new(response);
        assert_eq!(events.next().await.unwrap().unwrap(), json!({"a": 1}));
        assert!(
            events
                .next()
                .await
                .unwrap()
                .unwrap()
                .get("candidates")
                .is_some()
        );
        assert!(events.next().await.is_none());
    }

    /// The live endpoint frames with CRLF; a final frame may lack its blank line.
    #[tokio::test]
    async fn the_event_reader_takes_crlf_framing_and_an_unterminated_last_frame() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::sse(
                "data: {\"response\":{\"a\":1}}\r\n\r\ndata: {\"response\":{\"b\":2}}\r\n",
            )],
        );
        let response = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        let mut events = GeminiEvents::new(response);
        assert_eq!(events.next().await.unwrap().unwrap(), json!({"a": 1}));
        assert_eq!(events.next().await.unwrap().unwrap(), json!({"b": 2}));
        assert!(events.next().await.is_none());
    }

    #[test]
    fn the_captured_streams_decode_to_gemini_events() {
        for sse in [
            include_str!("../../tests/fixtures/antigravity/claude-tool-turn.sse"),
            include_str!("../../tests/fixtures/antigravity/flash-tool-turn.sse"),
        ] {
            let crlf = sse.replace("\r\n", "\n").replace('\n', "\r\n");
            for body in [sse.to_owned(), crlf] {
                let mut decoder = SseDecoder::default();
                let mut frames = decoder.push(body.as_bytes());
                frames.extend(decoder.finish());
                assert!(!frames.is_empty());
                for frame in frames {
                    let Decoded::Event(event) = decode_event(&frame.data).unwrap() else {
                        continue;
                    };
                    assert!(event.get("candidates").is_some(), "{event}");
                    assert!(event.get("traceId").is_none());
                }
            }
        }
    }

    #[tokio::test]
    async fn a_stream_error_mid_turn_ends_the_reader() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::sse(
                "data: {\"response\":{\"a\":1}}\n\ndata: {\"error\":{\"code\":500,\"status\":\"INTERNAL\",\"message\":\"boom\"}}\n\ndata: {\"response\":{\"b\":2}}\n\n",
            )],
        );
        let response = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        let mut events = GeminiEvents::new(response);
        assert!(events.next().await.unwrap().is_ok());
        let error = events.next().await.unwrap().unwrap_err();
        assert!(error.message.contains("INTERNAL: boom"), "{error}");
        assert!(events.next().await.is_none());
    }

    fn agy_upstream_with_real_backoff() -> AgyUpstream {
        // Never contacted: `delay` is arithmetic.
        AgyUpstream::with_base(memory("tok-a"), "http://127.0.0.1:9/v1internal:".to_owned())
            .unwrap()
    }

    #[test]
    fn the_delay_honours_retry_after_up_to_the_cap() {
        let upstream = agy_upstream_with_real_backoff();
        let secs = Duration::from_secs;
        assert!(upstream.delay(0, Some(secs(5))).unwrap() >= secs(5));
        assert!(upstream.delay(2, Some(secs(5))).unwrap() >= secs(5));
        assert_eq!(upstream.delay(0, Some(secs(31))), None);
        assert!(upstream.delay(0, Some(BACKOFF_CAP)).is_some());
        for attempt in 0..40 {
            let delay = upstream.delay(attempt, None).unwrap();
            assert!(delay <= BACKOFF_CAP, "attempt {attempt}: {delay:?}");
            assert!(delay >= BACKOFF_BASE / 2, "attempt {attempt}: {delay:?}");
        }
    }

    #[test]
    fn retry_after_is_read_as_seconds_or_as_an_http_date() {
        let now = chrono::Utc::now();
        let http_date = |offset: i64| {
            (now + chrono::TimeDelta::seconds(offset))
                .format("%a, %d %b %Y %H:%M:%S GMT")
                .to_string()
        };
        assert_eq!(retry_after_from("7", now), Some(Duration::from_secs(7)));
        assert_eq!(retry_after_from(" 0 ", now), Some(Duration::ZERO));
        let ahead = retry_after_from(&http_date(10), now).unwrap();
        assert!(
            ahead >= Duration::from_secs(9) && ahead <= Duration::from_secs(10),
            "{ahead:?}"
        );
        assert!(
            agy_upstream_with_real_backoff()
                .delay(0, Some(ahead))
                .unwrap()
                >= Duration::from_secs(9)
        );
        assert_eq!(retry_after_from(&http_date(-30), now), Some(Duration::ZERO));
        assert_eq!(retry_after_from("soon", now), None);
        assert_eq!(retry_after_from("-5", now), None);
    }

    #[tokio::test]
    async fn an_http_date_retry_after_past_the_cap_is_not_waited_for() {
        let date = (chrono::Utc::now() + chrono::TimeDelta::seconds(120))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::json(429, json!({})).with_header("retry-after", &date)],
        );
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(fake.count("streamGenerateContent"), 1);
    }

    #[tokio::test]
    async fn a_project_answer_that_breaks_off_is_retried() {
        let fake = FakeCodeAssist::start(
            vec![project().truncated(), project()],
            vec![Reply::sse(DONE)],
        );
        upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        assert_eq!(fake.count("loadCodeAssist"), 2);
        assert_eq!(fake.count("streamGenerateContent"), 1);
    }

    #[tokio::test]
    async fn a_project_answer_that_is_not_json_is_not_called_unonboarded() {
        let fake =
            FakeCodeAssist::start(vec![Reply::sse("<html>hi</html>")], vec![Reply::sse(DONE)]);
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(!error.message.contains("open agy once"), "{error}");
        assert!(error.message.contains("not understood"), "{error}");
        assert_eq!(fake.count("streamGenerateContent"), 0);
    }

    #[tokio::test]
    async fn a_project_answer_over_the_body_limit_is_refused() {
        let fake = FakeCodeAssist::start(
            vec![Reply::json(
                200,
                json!({"cloudaicompanionProject": "p", "pad": "x".repeat(BODY_LIMIT)}),
            )],
            vec![Reply::sse(DONE)],
        );
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("4 MiB"), "{error}");
        assert_eq!(
            fake.count("loadCodeAssist"),
            1,
            "an oversize answer is not retried"
        );
        assert_eq!(fake.count("streamGenerateContent"), 0);
    }

    #[tokio::test]
    async fn a_success_status_other_than_200_is_an_unexpected_answer() {
        let fake = FakeCodeAssist::start(vec![project()], vec![Reply::json(202, json!({}))]);
        let error = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("unexpected"), "{error}");
        assert_eq!(fake.count("streamGenerateContent"), 1);
    }

    #[tokio::test]
    async fn an_unterminated_stream_event_over_the_limit_ends_the_reader() {
        let fake = FakeCodeAssist::start(
            vec![project()],
            vec![Reply::sse(&format!(
                "data: {}",
                "x".repeat(EVENT_LIMIT + 16)
            ))],
        );
        let response = upstream(&fake, memory("tok-a"))
            .stream_generate("gemini-3.8-flash-low", request())
            .await
            .unwrap();
        let mut events = GeminiEvents::new(response);
        let error = events.next().await.unwrap().unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("1 MiB"), "{error}");
        assert!(events.next().await.is_none());
    }
}
