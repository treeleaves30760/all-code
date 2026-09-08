//! Credentials, read from the Codex CLI's own `auth.json`.
//!
//! The bridge never runs a login. It borrows the tokens `codex login` already
//! stored, and when it rotates an expired one it writes the new pair back so
//! the Codex CLI keeps working from the same file. Everything alc does not own
//! in that file — `auth_mode`, `OPENAI_API_KEY`, `tokens.id_token`, anything
//! added later — survives the write untouched, which is why [`AuthFile`]
//! carries an `extra` bag instead of a closed struct.
//!
//! Nothing in this module prints. The bridge reports failures over HTTP rather
//! than to stderr, and every type that holds a secret redacts itself in
//! `Debug`, so a token cannot reach a log, a panic message or a failing
//! assertion by way of a `{:?}` somebody added later.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use super::BridgeError;

/// OAuth client alc authenticates as. This is the Codex CLI's public client
/// id, because the tokens being refreshed were issued to it.
pub(crate) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Where a refresh is exchanged.
pub(crate) const TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";

/// Refresh this far before the JWT's own `exp`.
///
/// A token that expires mid-stream cannot be retried without replaying the
/// turn, so the window is wide enough that a long Codex turn cannot straddle
/// it unnoticed.
pub(crate) const REFRESH_MARGIN_MS: u64 = 5 * 60 * 1000;

/// A refresh is a handshake, not a turn: it either answers promptly or it is
/// not going to. The shared client carries no total timeout because a Codex
/// stream legitimately runs for minutes, so the bound is applied per request.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

/// Assumed lifetime when a refresh response omits `expires_in` *and* the new
/// access token's own `exp` cannot be read. Matches what the issuer actually
/// grants, so the next rotation lands at roughly the right time.
const ASSUMED_LIFETIME_SECS: u64 = 3600;

/// `~/.codex/auth.json`, as it is on disk.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct AuthFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<AuthTokens>,
    /// Everything alc does not own, preserved verbatim across a rotation.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct AuthTokens {
    /// Defaulted rather than required so a file whose `tokens` object is empty
    /// is refused as "no access token" instead of as "not valid JSON", which
    /// would send the user looking for a syntax error that is not there.
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Present when `codex login` recorded it. When absent the account id is
    /// recovered from the `chatgpt_account_id` claim inside the JWT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    /// Unknown token fields, preserved the same way.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// What a request actually needs: a bearer token and the account it belongs to.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
    /// Sent as the `chatgpt-account-id` header. Codex refuses a request
    /// without it even when the bearer token is valid.
    pub account_id: Option<String>,
    /// Epoch milliseconds, from the access token's `exp` claim.
    pub expires_at_ms: u64,
}

impl Credentials {
    /// Whether this token can still be sent, with [`REFRESH_MARGIN_MS`] of
    /// headroom for the turn it is about to sign.
    fn is_fresh(&self, now_ms: u64) -> bool {
        self.expires_at_ms > now_ms.saturating_add(REFRESH_MARGIN_MS)
    }
}

/// The refresh response from [`TOKEN_ENDPOINT`].
#[derive(Clone, Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    /// Optional by OAuth: an issuer that returns nothing here means "keep
    /// using the one you have", and treating that as a failure would break a
    /// perfectly valid rotation.
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: Option<String>,
    /// Seconds. Absent on some responses; assume an hour.
    #[serde(default)]
    pub expires_in: Option<u64>,
}

/// Holds the current credentials and serialises refreshes.
///
/// The mutex is the point, not the storage: several of alc's agents fire
/// concurrent requests, and without single-flighting they each rotate the
/// refresh token, and all but one of the resulting tokens is immediately
/// invalid. Reload from disk *after* taking the lock — another alc process may
/// have rotated while this one waited.
pub(crate) struct AuthManager {
    path: PathBuf,
    /// Always [`TOKEN_ENDPOINT`] in a real session. Tests point it at a
    /// loopback listener so the rotation logic can be exercised without a
    /// network, and without an environment variable the launch path would then
    /// have to defend against.
    endpoint: String,
    state: Mutex<Option<Credentials>>,
}

impl AuthManager {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            endpoint: TOKEN_ENDPOINT.to_owned(),
            state: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_endpoint(path: PathBuf, endpoint: String) -> Self {
        Self {
            path,
            endpoint,
            state: Mutex::new(None),
        }
    }

    /// Reads `auth.json`, preserving unknown fields for a later write-back.
    ///
    /// Distinguishes "no file" from "bad file" because the two need different
    /// advice: the first means `codex login`, the second means the file is
    /// corrupt and should not be silently overwritten.
    pub(crate) fn read_file(&self) -> Result<AuthFile, BridgeError> {
        let raw = std::fs::read(&self.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                BridgeError::auth(format!(
                    "no Codex credentials at {}; run `codex login`",
                    self.path.display()
                ))
            } else {
                BridgeError::auth(format!("could not read {}: {error}", self.path.display()))
            }
        })?;
        serde_json::from_slice(&raw).map_err(|error| {
            BridgeError::auth(format!(
                "{} is not valid JSON: {error}",
                self.path.display()
            ))
        })
    }

    /// The credentials to sign the next upstream request with, refreshing
    /// first if the stored access token is within [`REFRESH_MARGIN_MS`] of
    /// expiry.
    ///
    /// The read happens under the lock and against the file, never against the
    /// cached copy: a sibling request or a second alc process may have rotated
    /// since this one last looked, and signing with the token it remembers
    /// would send a value the issuer has already retired.
    pub(crate) async fn credentials(
        &self,
        http: &reqwest::Client,
    ) -> Result<Credentials, BridgeError> {
        let mut cached = self.state.lock().await;
        let stored = match self.stored() {
            Ok(stored) => stored,
            // The file can go out from under a live session — a `codex logout`
            // in another terminal. A token already in hand stays valid until
            // its own expiry, so the turn in flight does not have to die for
            // it. Once that token ages out the real error surfaces.
            Err(error) => {
                return match cached.as_ref().filter(|held| held.is_fresh(now_ms())) {
                    Some(held) => Ok(held.clone()),
                    None => Err(error),
                };
            }
        };

        if stored.is_fresh(now_ms()) {
            *cached = Some(stored.clone());
            return Ok(stored);
        }

        // An access token whose `exp` cannot be read reads as expired, which is
        // the safe default for a value that arrived from disk. Without this
        // check it would also be the *permanent* state: every turn would rotate
        // a token that never looks fresh. The cache remembers what the issuer
        // said the lifetime was, so an unreadable expiry costs one rotation
        // rather than one per turn.
        if let Some(held) = cached.as_ref()
            && held.access_token == stored.access_token
            && held.is_fresh(now_ms())
        {
            return Ok(held.clone());
        }

        self.rotate(http, &stored, &mut cached).await
    }

    /// Rotates unconditionally, after upstream answered 401 on a token that
    /// still looked fresh.
    ///
    /// `rejected` is the token that was refused. If the file no longer holds
    /// it, someone else already rotated while this request was in flight and
    /// their token is the live one — rotating again would retire it and turn
    /// one 401 into two.
    pub(crate) async fn force_refresh(
        &self,
        http: &reqwest::Client,
        rejected: Option<&str>,
    ) -> Result<Credentials, BridgeError> {
        let mut cached = self.state.lock().await;
        let stored = self.stored()?;
        if rejected.is_some_and(|token| token != stored.access_token) {
            *cached = Some(stored.clone());
            return Ok(stored);
        }
        self.rotate(http, &stored, &mut cached).await
    }

    /// Writes rotated tokens back, leaving every other field of the document
    /// as it was found.
    ///
    /// Re-reads first so a field written by the Codex CLI since this process
    /// started is carried across, and lands the result through a temporary
    /// file renamed into place: the CLI may read `auth.json` at any moment and
    /// must never catch a half-written document. A read that fails here aborts
    /// the write rather than starting from a blank document — the alternative
    /// is overwriting a file whose contents were merely unparseable to us.
    pub(crate) fn write_tokens(
        &self,
        credentials: &Credentials,
        id_token: Option<&str>,
    ) -> Result<(), BridgeError> {
        let mut file = self.read_file()?;
        let tokens = file.tokens.get_or_insert_with(AuthTokens::default);
        tokens.access_token = credentials.access_token.clone();
        tokens.refresh_token = credentials.refresh_token.clone();
        if let Some(account_id) = &credentials.account_id {
            tokens.account_id = Some(account_id.clone());
        }
        // Only when the issuer sent a new one. The Codex CLI reads `id_token`
        // for the account it displays, and blanking it would degrade that.
        if let Some(id_token) = id_token.filter(|value| !value.is_empty()) {
            tokens.id_token = Some(id_token.to_owned());
        }

        let document = serde_json::to_vec_pretty(&file).map_err(|error| {
            BridgeError::auth(format!(
                "could not re-encode {}: {error}",
                self.path.display()
            ))
        })?;
        write_atomic(&self.path, &document)
    }

    /// The credentials as the file currently states them, with the account id
    /// recovered from the tokens themselves when `codex login` did not record
    /// one.
    fn stored(&self) -> Result<Credentials, BridgeError> {
        let tokens = self
            .read_file()?
            .tokens
            .filter(|tokens| !tokens.access_token.trim().is_empty())
            .ok_or_else(|| {
                BridgeError::auth(format!(
                    "{} holds no Codex access token; run `codex login`",
                    self.path.display()
                ))
            })?;

        let account_id = tokens
            .account_id
            .clone()
            .filter(|id| !id.is_empty())
            // The id token carries the account the user actually chose; the
            // access token only carries whatever the issuer stamped on it.
            .or_else(|| tokens.id_token.as_deref().and_then(account_id_from_token))
            .or_else(|| account_id_from_token(&tokens.access_token));

        Ok(Credentials {
            // An unreadable expiry counts as expired: one wasted rotation is
            // cheaper than a turn signed with a token that died an hour ago.
            expires_at_ms: token_expiry_ms(&tokens.access_token).unwrap_or(0),
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            account_id,
        })
    }

    /// The single-flighted half of a refresh. The caller holds the lock and
    /// has already re-read the file, so what arrives here is what is really on
    /// disk right now.
    async fn rotate(
        &self,
        http: &reqwest::Client,
        current: &Credentials,
        cached: &mut Option<Credentials>,
    ) -> Result<Credentials, BridgeError> {
        if current.refresh_token.trim().is_empty() {
            return Err(BridgeError::auth(format!(
                "the Codex access token in {} has expired and the file holds no \
                 refresh token; run `codex login`",
                self.path.display()
            )));
        }

        let form = [
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", current.refresh_token.as_str()),
        ];
        let response = http
            .post(&self.endpoint)
            .timeout(REFRESH_TIMEOUT)
            .form(&form)
            .send()
            .await
            .map_err(|error| {
                BridgeError::new(
                    StatusCode::BAD_GATEWAY,
                    "api_error",
                    format!(
                        "could not reach {} to refresh the Codex token: {error}",
                        self.endpoint
                    ),
                )
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await;
            // A refused refresh normally means the refresh token itself is
            // gone. It can also mean the file moved on underneath us while
            // this request was in flight — another alc process rotated, the
            // issuer retired the token this one presented, and the live token
            // is already on disk. That is a success, not a failure.
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                && let Ok(latest) = self.stored()
                && latest.access_token != current.access_token
            {
                *cached = Some(latest.clone());
                return Ok(latest);
            }
            return Err(self.refusal(status, body));
        }

        let tokens: TokenResponse = response.json().await.map_err(|error| {
            BridgeError::auth(format!(
                "could not read the refresh response from {}: {error}",
                self.endpoint
            ))
        })?;
        if tokens.access_token.trim().is_empty() {
            return Err(BridgeError::auth(format!(
                "{} returned no access token; run `codex login`",
                self.endpoint
            )));
        }

        let account_id = tokens
            .id_token
            .as_deref()
            .and_then(account_id_from_token)
            .or_else(|| account_id_from_token(&tokens.access_token))
            .or_else(|| current.account_id.clone());
        let rotated = Credentials {
            expires_at_ms: token_expiry_ms(&tokens.access_token).unwrap_or_else(|| {
                now_ms().saturating_add(
                    tokens
                        .expires_in
                        .unwrap_or(ASSUMED_LIFETIME_SECS)
                        .saturating_mul(1000),
                )
            }),
            access_token: tokens.access_token.clone(),
            // Empty means "the one you sent is still yours" — see
            // [`TokenResponse::refresh_token`].
            refresh_token: if tokens.refresh_token.trim().is_empty() {
                current.refresh_token.clone()
            } else {
                tokens.refresh_token.clone()
            },
            account_id,
        };

        // A rotation that cannot be persisted has already happened upstream:
        // the refresh token on disk is dead from here on. Failing loudly names
        // the file and the reason; carrying on would leave the next launch
        // broken with nothing to point at.
        self.write_tokens(&rotated, tokens.id_token.as_deref())?;
        *cached = Some(rotated.clone());
        Ok(rotated)
    }

    /// Turns a refused rotation into the message the user needs.
    ///
    /// A 401 or 403 means the *refresh* token is gone, not the access token,
    /// so the only cure is a new login and the message has to say so — the
    /// user is otherwise looking at "unauthorized" with a file full of tokens
    /// in front of them. `auth.json` is never deleted for it: the file belongs
    /// to the Codex CLI, and a broken refresh does not entitle alc to throw
    /// away the API key sitting next to it.
    fn refusal(&self, status: StatusCode, body: Result<String, reqwest::Error>) -> BridgeError {
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return BridgeError::auth(format!(
                "the refresh token in {} was refused ({status}{}); run `codex login`",
                self.path.display(),
                detail(body)
            ));
        }
        BridgeError::new(
            StatusCode::BAD_GATEWAY,
            "api_error",
            format!(
                "{} could not refresh the Codex token ({status}{})",
                self.endpoint,
                detail(body)
            ),
        )
    }
}

/// Decodes a JWT's payload. Signature is not checked: this token is not being
/// trusted, it is being read for the claims its own issuer put there, and the
/// only consumer of a wrong answer is a request that upstream will refuse.
pub(crate) fn decode_jwt_payload(token: &str) -> Option<Value> {
    let mut segments = token.split('.');
    let (_header, payload, _signature) = (segments.next()?, segments.next()?, segments.next()?);
    if segments.next().is_some() {
        return None;
    }
    // Accept both alphabets. JWTs are base64url, but a hand-edited file or a
    // future issuer may hand over the standard one, and the difference is two
    // characters.
    let normalised: String = payload
        .chars()
        .map(|character| match character {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(normalised.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// The `exp` claim, in epoch milliseconds.
///
/// `None` for anything that cannot be read, so the caller treats the token as
/// due for refresh rather than trusting it forever.
pub(crate) fn token_expiry_ms(token: &str) -> Option<u64> {
    let claims = decode_jwt_payload(token)?;
    claims
        .get("exp")?
        .as_u64()
        .map(|seconds| seconds.saturating_mul(1000))
}

/// The ChatGPT account id, for the `chatgpt-account-id` header.
///
/// Four spellings, all of them observed in tokens the Codex CLI has stored.
/// They are tried in descending specificity and the first non-empty string
/// wins; a claim that is present but null falls through rather than ending the
/// search, because a null is what a token carries when that field was not the
/// one populated.
pub(crate) fn account_id_from_claims(claims: &Value) -> Option<String> {
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    };
    text(claims.get("chatgpt_account_id"))
        .or_else(|| {
            text(
                claims
                    .get("https://api.openai.com/auth")
                    .and_then(|auth| auth.get("chatgpt_account_id")),
            )
        })
        .or_else(|| text(claims.get("https://api.openai.com/auth.chatgpt_account_id")))
        .or_else(|| {
            text(
                claims
                    .get("organizations")
                    .and_then(Value::as_array)
                    .and_then(|organizations| organizations.first())
                    .and_then(|organization| organization.get("id")),
            )
        })
}

/// [`account_id_from_claims`] over a token rather than its decoded claims.
fn account_id_from_token(token: &str) -> Option<String> {
    account_id_from_claims(&decode_jwt_payload(token)?)
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

/// The issuer's own words about a refusal, bounded. The body is the only party
/// that knows why, but it can also be an error page, and an error message is
/// no place for a kilobyte of HTML.
fn detail(body: Result<String, reqwest::Error>) -> String {
    let Ok(body) = body else {
        return String::new();
    };
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let mut summary: String = body.chars().take(200).collect();
    if summary.len() < body.len() {
        summary.push('…');
    }
    format!(": {summary}")
}

/// Replaces `path` without ever letting a reader see a partial document.
///
/// The temporary file is created in the destination's own directory so the
/// rename cannot cross a filesystem boundary, and `tempfile` creates it 0600 —
/// which is what the result must end up as, since it is about to hold a
/// bearer token.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BridgeError> {
    use std::io::Write as _;

    let failed = |error: &dyn std::fmt::Display| {
        BridgeError::auth(format!("could not persist {}: {error}", path.display()))
    };
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::Builder::new()
        .prefix(".auth.json.")
        .tempfile_in(directory)
        .map_err(|error| failed(&error))?;
    file.write_all(bytes).map_err(|error| failed(&error))?;
    file.flush().map_err(|error| failed(&error))?;
    file.persist(path).map_err(|error| failed(&error))?;
    Ok(())
}

/// Redaction, so that a secret cannot escape through a `{:?}` that seemed
/// harmless where it was written.
fn redacted(value: &str) -> &'static str {
    if value.is_empty() {
        "<empty>"
    } else {
        "<redacted>"
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("access_token", &redacted(&self.access_token))
            .field("refresh_token", &redacted(&self.refresh_token))
            .field("account_id", &self.account_id)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

impl fmt::Debug for AuthTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthTokens")
            .field("access_token", &redacted(&self.access_token))
            .field("refresh_token", &redacted(&self.refresh_token))
            .field("account_id", &self.account_id)
            .field("id_token", &self.id_token.as_deref().map(redacted))
            // Keys only: `OPENAI_API_KEY` can live in here.
            .field("extra", &self.extra.keys())
            .finish()
    }
}

impl fmt::Debug for AuthFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthFile")
            .field("tokens", &self.tokens)
            .field("extra", &self.extra.keys())
            .finish()
    }
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenResponse")
            .field("access_token", &redacted(&self.access_token))
            .field("refresh_token", &redacted(&self.refresh_token))
            .field("id_token", &self.id_token.as_deref().map(redacted))
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;

    use serde_json::json;

    use super::*;

    /// A loopback stand-in for `auth.openai.com`.
    ///
    /// Refresh behaviour is the part of this module most likely to be wrong
    /// and the part hardest to reach from a unit test, so it gets a real
    /// socket rather than a trait. It answers every connection with the same
    /// canned response and records what it was asked, which is enough to prove
    /// single-flighting, the form the request takes, and what lands on disk
    /// afterwards. No test here talks to a network.
    struct FakeIssuer {
        endpoint: String,
        seen: Arc<std::sync::Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl FakeIssuer {
        fn start(status_line: &str, body: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            let endpoint = format!(
                "http://{}/oauth/token",
                listener.local_addr().expect("the bound address")
            );
            listener
                .set_nonblocking(true)
                .expect("a pollable listener, so the thread can be stopped");

            let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let response = format!(
                "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let recorder = Arc::clone(&seen);
            let halt = Arc::clone(&stop);
            let thread = std::thread::spawn(move || {
                while !halt.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).expect("a blocking stream");
                            // The request arrives in one write, but nothing
                            // promises it arrives in one read; read until the
                            // client stops talking rather than assuming.
                            stream
                                .set_read_timeout(Some(Duration::from_millis(200)))
                                .expect("a bounded read");
                            let mut request = Vec::new();
                            let mut chunk = [0_u8; 4096];
                            while let Ok(read) = stream.read(&mut chunk) {
                                if read == 0 {
                                    break;
                                }
                                request.extend_from_slice(&chunk[..read]);
                            }
                            recorder
                                .lock()
                                .expect("an unpoisoned recorder")
                                .push(String::from_utf8_lossy(&request).into_owned());
                            let _ = stream.write_all(response.as_bytes());
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                endpoint,
                seen,
                stop,
                thread: Some(thread),
            }
        }

        fn requests(&self) -> Vec<String> {
            self.seen.lock().expect("an unpoisoned recorder").clone()
        }
    }

    impl Drop for FakeIssuer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn base64url(bytes: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    /// An unsigned JWT carrying `claims`. The signature is never checked, so
    /// there is nothing to sign it with and nothing that would notice.
    fn jwt(claims: Value) -> String {
        format!(
            "{}.{}.signature",
            base64url(br#"{"alg":"none"}"#),
            base64url(&serde_json::to_vec(&claims).expect("serialisable claims"))
        )
    }

    /// An access token that expires `seconds` from now, negative for a token
    /// that has already lapsed.
    fn access_token(seconds: i64) -> String {
        let now = i64::try_from(now_ms() / 1000).expect("a representable clock");
        jwt(json!({ "exp": now + seconds, "chatgpt_account_id": "acct_jwt" }))
    }

    fn write_auth_json(directory: &Path, tokens: Value) -> PathBuf {
        let path = directory.join("auth.json");
        let document = json!({
            "OPENAI_API_KEY": Value::Null,
            "auth_mode": "chatgpt",
            "last_refresh": "2026-09-01T00:00:00Z",
            "tokens": tokens,
        });
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&document).expect("serialisable document"),
        )
        .expect("a writable temporary directory");
        path
    }

    fn read_back(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).expect("the written file"))
            .expect("valid JSON on disk")
    }

    #[test]
    fn an_auth_file_keeps_the_fields_the_bridge_does_not_own() {
        let raw = r#"{
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "id",
                "access_token": "access",
                "refresh_token": "refresh",
                "account_id": "acct"
            },
            "last_refresh": "2026-09-08T21:09:00Z"
        }"#;
        let parsed: AuthFile = serde_json::from_str(raw).unwrap();
        let tokens = parsed.tokens.as_ref().unwrap();
        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.account_id.as_deref(), Some("acct"));
        let round_tripped = serde_json::to_value(&parsed).unwrap();
        assert_eq!(round_tripped["auth_mode"], "chatgpt");
        assert_eq!(round_tripped["last_refresh"], "2026-09-08T21:09:00Z");
        assert!(round_tripped.get("OPENAI_API_KEY").is_some());
    }

    #[test]
    fn a_jwt_payload_is_read_without_its_signature_being_trusted() {
        let claims = decode_jwt_payload(&jwt(json!({ "sub": "user_1", "exp": 42 })))
            .expect("a decodable payload");
        assert_eq!(claims["sub"], "user_1");
        assert_eq!(claims["exp"], 42);
    }

    #[test]
    fn a_token_that_is_not_three_segments_of_base64_json_yields_no_claims() {
        for token in [
            "",
            "opaque-token",
            "only.two",
            "four.segments.are.wrong",
            "header..signature",
        ] {
            assert!(
                decode_jwt_payload(token).is_none(),
                "{token:?} must not decode"
            );
        }
        let not_json = format!("{}.{}.sig", base64url(b"{}"), base64url(b"not json"));
        assert!(decode_jwt_payload(&not_json).is_none());
    }

    #[test]
    fn the_account_id_is_taken_from_the_first_claim_spelling_that_carries_one() {
        let all_four = json!({
            "chatgpt_account_id": "acct_direct",
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct_nested" },
            "https://api.openai.com/auth.chatgpt_account_id": "acct_flat",
            "organizations": [{ "id": "org_first" }],
        });
        assert_eq!(
            account_id_from_claims(&all_four).as_deref(),
            Some("acct_direct")
        );

        let nested = json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct_nested" },
            "https://api.openai.com/auth.chatgpt_account_id": "acct_flat",
        });
        assert_eq!(
            account_id_from_claims(&nested).as_deref(),
            Some("acct_nested")
        );

        let flat = json!({ "https://api.openai.com/auth.chatgpt_account_id": "acct_flat" });
        assert_eq!(account_id_from_claims(&flat).as_deref(), Some("acct_flat"));

        let organizations = json!({ "organizations": [{ "id": "org_first" }, { "id": "org_2" }] });
        assert_eq!(
            account_id_from_claims(&organizations).as_deref(),
            Some("org_first")
        );
    }

    #[test]
    fn a_claim_that_is_present_but_empty_falls_through_to_the_next_spelling() {
        let claims = json!({
            "chatgpt_account_id": Value::Null,
            "https://api.openai.com/auth": { "chatgpt_account_id": "" },
            "organizations": [{ "id": "org_last_resort" }],
        });
        assert_eq!(
            account_id_from_claims(&claims).as_deref(),
            Some("org_last_resort")
        );
        assert_eq!(account_id_from_claims(&json!({ "sub": "user_1" })), None);
    }

    #[test]
    fn an_expiry_claim_is_returned_in_milliseconds() {
        assert_eq!(
            token_expiry_ms(&jwt(json!({ "exp": 4_102_444_800_u64 }))),
            Some(4_102_444_800_000)
        );
    }

    #[test]
    fn a_token_whose_expiry_cannot_be_read_counts_as_due_for_refresh() {
        for token in ["opaque", &jwt(json!({ "sub": "user_1" }))] {
            assert_eq!(token_expiry_ms(token), None, "{token:?} has no usable exp");
        }
        let unreadable = Credentials {
            access_token: "opaque".to_owned(),
            refresh_token: "refresh".to_owned(),
            account_id: None,
            expires_at_ms: token_expiry_ms("opaque").unwrap_or(0),
        };
        assert!(!unreadable.is_fresh(now_ms()));
    }

    #[test]
    fn a_token_inside_the_refresh_margin_is_not_fresh_enough_to_send() {
        let now = now_ms();
        let at = |expires_at_ms| Credentials {
            access_token: "access".to_owned(),
            refresh_token: "refresh".to_owned(),
            account_id: None,
            expires_at_ms,
        };
        assert!(at(now + REFRESH_MARGIN_MS + 60_000).is_fresh(now));
        assert!(!at(now + REFRESH_MARGIN_MS).is_fresh(now));
        assert!(!at(now + REFRESH_MARGIN_MS - 1).is_fresh(now));
        assert!(!at(now.saturating_sub(1)).is_fresh(now));
    }

    #[tokio::test]
    async fn a_fresh_token_is_served_from_the_file_without_contacting_the_issuer() {
        let directory = tempfile::TempDir::new().unwrap();
        let token = access_token(3600);
        let path = write_auth_json(
            directory.path(),
            json!({
                "access_token": token,
                "refresh_token": "stored-refresh",
                "account_id": "acct_file",
            }),
        );
        // Any request at all would fail against a port nothing is listening on.
        let manager = AuthManager::with_endpoint(path, "http://127.0.0.1:1/oauth/token".to_owned());

        let credentials = manager.credentials(&reqwest::Client::new()).await.unwrap();
        assert_eq!(credentials.access_token, token);
        assert_eq!(credentials.refresh_token, "stored-refresh");
        assert_eq!(credentials.account_id.as_deref(), Some("acct_file"));
    }

    #[tokio::test]
    async fn the_account_id_falls_back_to_the_tokens_when_the_file_records_none() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = write_auth_json(
            directory.path(),
            json!({
                "access_token": access_token(3600),
                "refresh_token": "stored-refresh",
                "id_token": jwt(json!({ "chatgpt_account_id": "acct_id_token" })),
            }),
        );
        let manager = AuthManager::new(path);
        assert_eq!(
            manager.stored().unwrap().account_id.as_deref(),
            Some("acct_id_token"),
            "the id token is the account the user chose, so it wins"
        );

        let directory = tempfile::TempDir::new().unwrap();
        let path = write_auth_json(
            directory.path(),
            json!({ "access_token": access_token(3600), "refresh_token": "r" }),
        );
        assert_eq!(
            AuthManager::new(path)
                .stored()
                .unwrap()
                .account_id
                .as_deref(),
            Some("acct_jwt"),
            "and the access token is the last place left to look"
        );
    }

    #[tokio::test]
    async fn a_missing_auth_file_says_where_it_looked_and_to_run_codex_login() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("auth.json");
        let manager = AuthManager::new(path.clone());

        let error = manager
            .credentials(&reqwest::Client::new())
            .await
            .expect_err("no file, no credentials");
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert!(error.message.contains(&path.display().to_string()));
        assert!(error.message.contains("codex login"));
    }

    #[tokio::test]
    async fn an_auth_file_without_an_access_token_blames_the_token_not_the_json() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = write_auth_json(directory.path(), json!({ "refresh_token": "r" }));

        let error = AuthManager::new(path)
            .credentials(&reqwest::Client::new())
            .await
            .expect_err("tokens without an access token are no credentials");
        assert!(
            error.message.contains("no Codex access token"),
            "{}",
            error.message
        );
        assert!(!error.message.contains("valid JSON"), "{}", error.message);
    }

    #[tokio::test]
    async fn an_expired_token_with_nothing_to_refresh_it_with_says_exactly_that() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = write_auth_json(
            directory.path(),
            json!({ "access_token": access_token(-60), "refresh_token": "" }),
        );

        let error = AuthManager::new(path)
            .credentials(&reqwest::Client::new())
            .await
            .expect_err("an expired token cannot be rotated without a refresh token");
        assert!(
            error.message.contains("no refresh token"),
            "{}",
            error.message
        );
        assert!(error.message.contains("codex login"));
    }

    #[tokio::test]
    async fn a_rotation_carries_the_codex_client_id_and_preserves_the_rest_of_the_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let rotated = access_token(3600);
        let issuer = FakeIssuer::start(
            "200 OK",
            &json!({
                "access_token": rotated,
                "refresh_token": "rotated-refresh",
                "id_token": jwt(json!({ "chatgpt_account_id": "acct_rotated" })),
                "expires_in": 3600,
            })
            .to_string(),
        );
        let path = write_auth_json(
            directory.path(),
            json!({
                "access_token": access_token(-60),
                "refresh_token": "stored-refresh",
                "id_token": "stale.id.token",
                "account_id": "acct_file",
            }),
        );
        let manager = AuthManager::with_endpoint(path.clone(), issuer.endpoint.clone());

        let credentials = manager.credentials(&reqwest::Client::new()).await.unwrap();
        assert_eq!(credentials.access_token, rotated);
        assert_eq!(credentials.refresh_token, "rotated-refresh");
        assert_eq!(credentials.account_id.as_deref(), Some("acct_rotated"));

        let request = issuer.requests().pop().expect("one refresh");
        assert!(request.contains("grant_type=refresh_token"), "{request}");
        assert!(
            request.contains(&format!("client_id={CLIENT_ID}")),
            "{request}"
        );
        assert!(
            request.contains("refresh_token=stored-refresh"),
            "{request}"
        );

        let document = read_back(&path);
        assert_eq!(document["tokens"]["access_token"], rotated);
        assert_eq!(document["tokens"]["refresh_token"], "rotated-refresh");
        assert_eq!(document["tokens"]["account_id"], "acct_rotated");
        assert_ne!(document["tokens"]["id_token"], "stale.id.token");
        // Everything the Codex CLI owns is still there.
        assert_eq!(document["auth_mode"], "chatgpt");
        assert_eq!(document["last_refresh"], "2026-09-01T00:00:00Z");
        assert!(document.as_object().unwrap().contains_key("OPENAI_API_KEY"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_rotated_file_is_left_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::TempDir::new().unwrap();
        let issuer = FakeIssuer::start(
            "200 OK",
            &json!({ "access_token": access_token(3600), "refresh_token": "rotated-refresh" })
                .to_string(),
        );
        let path = write_auth_json(
            directory.path(),
            json!({ "access_token": access_token(-60), "refresh_token": "stored-refresh" }),
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        AuthManager::with_endpoint(path.clone(), issuer.endpoint.clone())
            .credentials(&reqwest::Client::new())
            .await
            .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a file holding a bearer token is not world-readable"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_callers_rotate_an_expired_token_exactly_once() {
        let directory = tempfile::TempDir::new().unwrap();
        let rotated = access_token(3600);
        let issuer = FakeIssuer::start(
            "200 OK",
            &json!({ "access_token": rotated, "refresh_token": "rotated-refresh" }).to_string(),
        );
        let path = write_auth_json(
            directory.path(),
            json!({ "access_token": access_token(-60), "refresh_token": "stored-refresh" }),
        );
        let manager = AuthManager::with_endpoint(path, issuer.endpoint.clone());
        let http = reqwest::Client::new();

        let (first, second) = tokio::join!(manager.credentials(&http), manager.credentials(&http));

        assert_eq!(first.unwrap().access_token, rotated);
        assert_eq!(second.unwrap().access_token, rotated);
        assert_eq!(
            issuer.requests().len(),
            1,
            "a second rotation would retire the token the first one just won"
        );
    }

    #[tokio::test]
    async fn a_rotation_the_issuer_refuses_names_the_file_and_asks_for_a_new_login() {
        let directory = tempfile::TempDir::new().unwrap();
        let issuer = FakeIssuer::start("401 Unauthorized", r#"{"error":"invalid_grant"}"#);
        let path = write_auth_json(
            directory.path(),
            json!({ "access_token": access_token(-60), "refresh_token": "stored-refresh" }),
        );

        let error = AuthManager::with_endpoint(path.clone(), issuer.endpoint.clone())
            .credentials(&reqwest::Client::new())
            .await
            .expect_err("a refused refresh is not a credential");
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert!(error.message.contains(&path.display().to_string()));
        assert!(error.message.contains("invalid_grant"), "{}", error.message);
        assert!(error.message.contains("codex login"));
        assert!(path.exists(), "auth.json belongs to the Codex CLI");
    }

    #[tokio::test]
    async fn a_forced_refresh_defers_to_the_token_a_sibling_already_rotated_to() {
        let directory = tempfile::TempDir::new().unwrap();
        let issuer = FakeIssuer::start("200 OK", "{}");
        let path = write_auth_json(
            directory.path(),
            json!({ "access_token": access_token(3600), "refresh_token": "stored-refresh" }),
        );
        let manager = AuthManager::with_endpoint(path, issuer.endpoint.clone());

        let credentials = manager
            .force_refresh(&reqwest::Client::new(), Some("the-token-upstream-refused"))
            .await
            .unwrap();

        assert_ne!(credentials.access_token, "the-token-upstream-refused");
        assert!(
            issuer.requests().is_empty(),
            "the file already moved on; rotating again would retire the live token"
        );
    }

    #[tokio::test]
    async fn a_rotated_token_without_a_readable_expiry_is_not_rotated_again_next_turn() {
        let directory = tempfile::TempDir::new().unwrap();
        let issuer = FakeIssuer::start(
            "200 OK",
            r#"{"access_token":"opaque-access","refresh_token":"rotated-refresh","expires_in":3600}"#,
        );
        let path = write_auth_json(
            directory.path(),
            json!({ "access_token": access_token(-60), "refresh_token": "stored-refresh" }),
        );
        let manager = AuthManager::with_endpoint(path, issuer.endpoint.clone());
        let http = reqwest::Client::new();

        assert_eq!(
            manager.credentials(&http).await.unwrap().access_token,
            "opaque-access"
        );
        assert_eq!(
            manager.credentials(&http).await.unwrap().access_token,
            "opaque-access"
        );
        assert_eq!(
            issuer.requests().len(),
            1,
            "the issuer's own expires_in is remembered, so an opaque token is \
             rotated once rather than once per turn"
        );
    }

    #[test]
    fn a_debug_print_of_a_credential_carries_no_secret() {
        let credentials = Credentials {
            access_token: "super-secret-access".to_owned(),
            refresh_token: "super-secret-refresh".to_owned(),
            account_id: Some("acct_1".to_owned()),
            expires_at_ms: 1,
        };
        let printed = format!("{credentials:?}");
        assert!(!printed.contains("super-secret"), "{printed}");
        assert!(printed.contains("acct_1"), "{printed}");

        let file: AuthFile = serde_json::from_value(json!({
            "OPENAI_API_KEY": "sk-super-secret",
            "tokens": {
                "access_token": "super-secret-access",
                "refresh_token": "super-secret-refresh",
                "id_token": "super-secret-id",
            },
        }))
        .unwrap();
        let printed = format!("{file:?}");
        assert!(!printed.contains("super-secret"), "{printed}");
        assert!(
            printed.contains("OPENAI_API_KEY"),
            "the key's name is not the key: {printed}"
        );
    }
}
