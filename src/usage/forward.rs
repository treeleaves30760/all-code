//! Opt-in native API forwarding. Wire payloads are never rewritten or persisted.
//!
//! A durable host must mount `relay` behind its own registered capability and
//! loopback Host check. The ephemeral server below supplies both itself. Neither
//! transport accepts an upstream URL from a request.

use std::future::IntoFuture;
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::body::{Body, BodyDataStream};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use reqwest::{Client, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use super::ledger::Ledger;
use super::observer::{RequestObservation, StreamObservation, WireProtocol};
use super::records::{MAX_LINE_BYTES, Outcome, OutputBasis};

/// A fixed target and pooled client. Deliberately has no Debug implementation:
/// the configured URL can contain a private query parameter.
pub(crate) struct ForwardTarget {
    upstream: Url,
    endpoint: String,
    ledger: Arc<Ledger>,
    expected_key_digests: Vec<String>,
    client: Client,
}

impl ForwardTarget {
    pub(crate) fn new(
        upstream: &str,
        ledger: Arc<Ledger>,
        expected_key_digests: Vec<String>,
    ) -> Result<Self> {
        let mut url = parse_upstream(upstream)?;
        if !url.username().is_empty() || url.password().is_some() {
            bail!("direct API forwarding does not accept URL credentials");
        }
        url.set_fragment(None);
        let endpoint = sanitized_endpoint(upstream)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            // Also stay byte-preserving if another dependency enables these
            // reqwest features in the future.
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(30))
            // No total/read deadline: native long-running requests remain native.
            .build()
            .map_err(|_| anyhow::anyhow!("could not create the direct API forwarding client"))?;
        Ok(Self {
            upstream: url,
            endpoint,
            ledger,
            expected_key_digests,
            client,
        })
    }

    fn url(&self, suffix: &str, query: Option<&str>) -> Result<Url> {
        validate_suffix(suffix)?;
        let mut url = self.upstream.clone();
        if !suffix.is_empty() {
            let path = format!("{}{}", url.path().trim_end_matches('/'), suffix);
            url.set_path(&path);
        }
        let combined = match (
            url.query().filter(|s| !s.is_empty()),
            query.filter(|s| !s.is_empty()),
        ) {
            (Some(base), Some(request)) => Some(format!("{base}&{request}")),
            (Some(base), None) => Some(base.to_owned()),
            (None, Some(request)) => Some(request.to_owned()),
            (None, None) => None,
        };
        url.set_query(combined.as_deref());
        Ok(url)
    }
}

/// Only a digest is retained by a target/registration, never the vendor key.
pub(crate) fn key_digest(key: &str) -> String {
    Sha256::digest(key.trim().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Metadata identity, not a usable forwarding URL: remove all credentials,
/// query parameters and fragments before the endpoint can reach the ledger.
pub(crate) fn sanitized_endpoint(upstream: &str) -> Result<String> {
    let mut url = parse_upstream(upstream)?;
    url.set_username("")
        .map_err(|_| anyhow::anyhow!("invalid direct API endpoint"))?;
    url.set_password(None)
        .map_err(|_| anyhow::anyhow!("invalid direct API endpoint"))?;
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn parse_upstream(upstream: &str) -> Result<Url> {
    // WHATWG URL parsing can remove controls or treat a backslash as a path
    // separator. Reject ambiguous raw input before inspecting its authority.
    if upstream
        .bytes()
        .any(|byte| byte == b'\\' || byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        bail!("invalid direct API endpoint");
    }
    let Some((scheme, rest)) = upstream.split_once("://") else {
        bail!("direct API forwarding requires an absolute HTTP(S) endpoint");
    };
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        bail!("direct API forwarding requires an HTTP(S) endpoint");
    }
    let url = Url::parse(upstream).map_err(|_| anyhow::anyhow!("invalid direct API endpoint"))?;
    if url.host_str().is_none() || url.cannot_be_a_base() {
        bail!("invalid direct API endpoint");
    }
    if scheme == "http" {
        // Also validate the parsed destination used by reqwest, not only the
        // authority spelling: the two must independently be explicit loopback.
        if !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
            bail!(
                "unencrypted direct API forwarding is allowed only for explicit loopback endpoints"
            );
        }
        // Inspect the spelling supplied by the user, not Url's canonicalized
        // host: 127.1, integer IPs, localhost.evil and 0.0.0.0 are not opt-ins.
        let raw_authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let raw_host = raw_authority.rsplit('@').next().unwrap_or("");
        let authority = raw_host
            .parse::<axum::http::uri::Authority>()
            .map_err(|_| anyhow::anyhow!("invalid direct API endpoint"))?;
        let host = authority.host();
        if !host.eq_ignore_ascii_case("localhost") && host != "127.0.0.1" && host != "[::1]" {
            bail!(
                "unencrypted direct API forwarding is allowed only for explicit loopback endpoints"
            );
        }
    }
    Ok(url)
}

/// Reject URL references and traversal before URL's dot-segment normalization.
/// Encoded percent signs are disallowed too, so a second decoding pass cannot
/// turn a seemingly safe suffix into an escaped separator or dot segment.
fn validate_suffix(suffix: &str) -> Result<()> {
    if suffix.is_empty() {
        return Ok(());
    }
    if !suffix.starts_with('/') || suffix.starts_with("//") || suffix.contains("//") {
        bail!("forwarding suffix must be an absolute path without an authority");
    }
    let mut decoded = Vec::with_capacity(suffix.len());
    let bytes = suffix.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let hex = |byte: u8| (byte as char).to_digit(16).map(|value| value as u8);
            let Some(value) = bytes
                .get(index + 1)
                .copied()
                .and_then(hex)
                .zip(bytes.get(index + 2).copied().and_then(hex))
                .map(|(hi, lo)| hi * 16 + lo)
            else {
                bail!("invalid encoded forwarding path");
            };
            if matches!(value, b'/' | b'\\' | b'%' | b'?' | b'#' | b';')
                || value < b' '
                || value == 127
            {
                bail!("unsafe encoded forwarding path");
            }
            decoded.push(value);
            index += 3;
        } else {
            if matches!(byte, b'\\' | b'?' | b'#' | b';') || byte <= b' ' || byte == 127 {
                bail!("unsafe forwarding path");
            }
            decoded.push(byte);
            index += 1;
        }
    }
    if decoded
        .split(|byte| *byte == b'/')
        .any(|segment| segment == b"." || segment == b"..")
    {
        bail!("forwarding path cannot contain traversal segments");
    }
    Ok(())
}

fn protocol(suffix: &str) -> Option<WireProtocol> {
    match suffix {
        "/messages" | "/v1/messages" => Some(WireProtocol::Messages),
        "/responses" | "/v1/responses" => Some(WireProtocol::Responses),
        "/chat/completions" | "/v1/chat/completions" => Some(WireProtocol::Chat),
        _ => None,
    }
}

fn authorized(headers: &HeaderMap, expected: &[String]) -> bool {
    if expected.is_empty() {
        return true;
    }
    // Duplicates are ambiguous between clients, HTTP stacks and vendors.
    if headers.get_all("x-api-key").iter().count() > 1
        || headers.get_all(header::AUTHORIZATION).iter().count() > 1
    {
        return false;
    }
    let api_key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, key) = value.trim().split_once(char::is_whitespace)?;
            scheme.eq_ignore_ascii_case("bearer").then(|| key.trim())
        });
    [api_key, bearer]
        .into_iter()
        .flatten()
        .filter(|key| !key.is_empty())
        .any(|key| {
            let actual = key_digest(key);
            expected
                .iter()
                .any(|digest| constant_time_eq(actual.as_bytes(), digest.as_bytes()))
        })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn strip_hop_headers(headers: &mut HeaderMap) {
    let nominated: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in nominated {
        headers.remove(name);
    }
    for name in [
        "host",
        "connection",
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

fn opaque_encoding(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .any(|value| {
            value
                .to_str()
                .map_or(true, |value| !value.trim().eq_ignore_ascii_case("identity"))
        })
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn rejected(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

/// Relay native bytes. The caller owns capability registration and Host checks;
/// an empty expected-key set is safe only inside such an authenticated mount.
pub(crate) async fn relay(target: Arc<ForwardTarget>, suffix: &str, request: Request) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return rejected(
            StatusCode::FORBIDDEN,
            "browser-origin API forwarding is not allowed",
        );
    }
    if !authorized(request.headers(), &target.expected_key_digests) {
        return rejected(
            StatusCode::UNAUTHORIZED,
            "direct API forwarding credentials did not match",
        );
    }
    let url = match target.url(suffix, request.uri().query()) {
        Ok(url) => url,
        Err(_) => {
            return rejected(
                StatusCode::BAD_REQUEST,
                "invalid direct API forwarding path",
            );
        }
    };
    let (parts, body) = request.into_parts();
    let observed = protocol(suffix).map(|protocol| {
        let basis = if protocol == WireProtocol::Messages {
            OutputBasis::Gross
        } else {
            OutputBasis::Unknown
        };
        // The clock starts before any upload inspection, including a cold/slow
        // upload. Client streaming is set only from a complete typed body.
        let request = RequestObservation::new(target.ledger.clone(), "", false, protocol, basis);
        request.update(|record| record.endpoint = Some(target.endpoint.clone()));
        request
    });
    let mut dispatch_guard = DispatchGuard {
        request: observed.clone(),
        armed: true,
    };
    let encoded_upload = opaque_encoding(&parts.headers);
    let mut headers = parts.headers;
    strip_hop_headers(&mut headers);
    let upload = Upload::new(
        body.into_data_stream(),
        observed.clone(),
        content_length(&headers),
        encoded_upload,
    );
    let response = match target
        .client
        .request(parts.method.clone(), url)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(upload.stream()))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let timed_out = error.is_timeout();
            if let Some(request) = &observed {
                request.finish(if timed_out {
                    Outcome::TimedOut
                } else {
                    Outcome::Failed
                });
            }
            // reqwest errors can contain the entire secret-bearing URL.
            return rejected(
                if timed_out {
                    StatusCode::GATEWAY_TIMEOUT
                } else {
                    StatusCode::BAD_GATEWAY
                },
                "direct API upstream request failed",
            );
        }
    };
    let status = response.status();
    let mut headers = response.headers().clone();
    let expected_length = content_length(&headers);
    let encoded_response = opaque_encoding(&headers);
    let sse = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("text/event-stream")
        });
    let headers_received_us = observed.as_ref().map_or(0, RequestObservation::elapsed_us);
    let inspection = observed.map(|request| {
        if let Some(id) = headers.get("x-request-id").and_then(|id| id.to_str().ok()) {
            request.add_id("http", id);
        }
        if !status.is_success() {
            request.finish(Outcome::Failed);
            ResponseInspection::Unavailable(request)
        } else if encoded_response {
            request.warn("encoded response has unavailable usage metadata");
            ResponseInspection::Unavailable(request)
        } else if sse {
            ResponseInspection::Sse(StreamObservation::new(request))
        } else {
            ResponseInspection::Json {
                request,
                capture: Capture::default(),
            }
        }
    });
    strip_hop_headers(&mut headers);
    let bodyless = parts.method == Method::HEAD
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED;
    let mut pump = Download {
        body: response.bytes_stream().boxed(),
        inspection,
        expected_length: if bodyless { Some(0) } else { expected_length },
        received: 0,
        last_received_us: headers_received_us,
        ended: false,
    };
    if pump.expected_length == Some(0) {
        pump.finish();
    }
    let mut response = Response::new(Body::from_stream(pump.stream()));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    dispatch_guard.armed = false;
    response
}

/// Dispatch cancellation must not wait for a retained hyper upload body to be
/// dropped. Completed/error paths are idempotent with the observation's RAII.
struct DispatchGuard {
    request: Option<RequestObservation>,
    armed: bool,
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(request) = &self.request
        {
            request.finish(Outcome::Cancelled);
        }
    }
}

/// At most MAX_LINE_BYTES of inspection, independent of forwarded body size.
#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    unavailable: bool,
}

impl Capture {
    fn push(&mut self, chunk: &[u8]) -> bool {
        if self.unavailable {
            return false;
        }
        if self.bytes.len().saturating_add(chunk.len()) > MAX_LINE_BYTES {
            self.bytes = Vec::new();
            self.unavailable = true;
            return true;
        }
        self.bytes.extend_from_slice(chunk);
        false
    }
}

#[derive(Deserialize)]
struct RequestMetadata {
    model: Option<String>,
    stream: Option<bool>,
    service_tier: Option<String>,
}

struct Upload {
    body: BodyDataStream,
    request: Option<RequestObservation>,
    capture: Capture,
    expected_length: Option<u64>,
    received: u64,
    inspected: bool,
}

impl Upload {
    fn new(
        body: BodyDataStream,
        request: Option<RequestObservation>,
        expected_length: Option<u64>,
        encoded: bool,
    ) -> Self {
        let mut upload = Self {
            body,
            request,
            capture: Capture::default(),
            expected_length,
            received: 0,
            inspected: false,
        };
        if encoded {
            upload.capture.unavailable = true;
            if let Some(request) = &upload.request {
                request.warn("encoded request has unavailable model and streaming metadata");
            }
        }
        if expected_length == Some(0) {
            upload.inspect();
        }
        upload
    }

    fn inspect(&mut self) {
        if self.inspected {
            return;
        }
        self.inspected = true;
        if let Some(request) = &self.request
            && !self.capture.unavailable
        {
            match serde_json::from_slice::<RequestMetadata>(&self.capture.bytes) {
                Ok(metadata) => {
                    if let Some(streaming) = metadata.stream {
                        request.set_streaming(streaming);
                    }
                    if let Some(model) = metadata.model.filter(|model| {
                        !model.is_empty()
                            && model.len() <= 512
                            && !model.chars().any(char::is_control)
                    }) {
                        request.update(|record| record.model = Some(model));
                    }
                    if let Some(tier) = metadata.service_tier.filter(|tier| {
                        !tier.is_empty() && tier.len() <= 128 && !tier.chars().any(char::is_control)
                    }) {
                        request.update(|record| record.service_tier = Some(tier));
                    }
                }
                Err(_) => request.warn("request model and streaming metadata unavailable"),
            }
        }
        self.capture.bytes = Vec::new();
    }

    fn stream(
        self,
    ) -> impl futures_util::Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send {
        futures_util::stream::unfold(Some(self), |state| async move {
            let mut upload = state?;
            match upload.body.next().await {
                Some(Ok(chunk)) => {
                    upload.received = upload.received.saturating_add(chunk.len() as u64);
                    if !upload.inspected
                        && upload.request.is_some()
                        && upload.capture.push(&chunk)
                        && let Some(request) = &upload.request
                    {
                        request.warn("request metadata inspection exceeded its limit");
                    }
                    // Content-Length closes a body even if the HTTP stack never
                    // polls the wrapped stream once more after its last chunk.
                    if upload.expected_length == Some(upload.received) {
                        upload.inspect();
                    }
                    Some((Ok(chunk), Some(upload)))
                }
                Some(Err(_)) => {
                    if let Some(request) = &upload.request {
                        request.finish(Outcome::Truncated);
                    }
                    Some((
                        Err(std::io::Error::other("client request body was interrupted")),
                        None,
                    ))
                }
                None => {
                    upload.inspect();
                    None
                }
            }
        })
    }
}

enum ResponseInspection {
    Sse(StreamObservation),
    Json {
        request: RequestObservation,
        capture: Capture,
    },
    Unavailable(RequestObservation),
}

impl ResponseInspection {
    fn elapsed_us(&self) -> u64 {
        match self {
            Self::Sse(stream) => stream.request.elapsed_us(),
            Self::Json { request, .. } | Self::Unavailable(request) => request.elapsed_us(),
        }
    }

    fn push_at(&mut self, chunk: &[u8], at_us: u64) {
        match self {
            Self::Sse(stream) => stream.push_at(chunk, at_us),
            Self::Json { request, capture } => {
                if capture.push(chunk) {
                    request.warn("response metadata inspection exceeded its limit");
                }
            }
            Self::Unavailable(_) => {}
        }
    }

    fn finish_at(&mut self, at_us: u64) {
        match self {
            Self::Sse(stream) => stream.eof(),
            Self::Json { request, capture } => {
                if !capture.unavailable {
                    match serde_json::from_slice::<serde_json::Value>(&capture.bytes) {
                        Ok(value) => request.json_response_at(&value, at_us),
                        Err(_) => request.warn("response usage metadata unavailable"),
                    }
                }
                request.finish_at(Outcome::Completed, at_us);
                capture.bytes = Vec::new();
            }
            Self::Unavailable(request) => request.finish_at(Outcome::Completed, at_us),
        }
    }

    fn cancel(&self) {
        match self {
            Self::Sse(stream) => stream.request.finish(Outcome::Cancelled),
            Self::Json { request, .. } | Self::Unavailable(request) => {
                request.finish(Outcome::Cancelled)
            }
        }
    }

    fn error(&self) {
        match self {
            Self::Sse(stream) => stream.request.finish(Outcome::Truncated),
            Self::Json { request, .. } | Self::Unavailable(request) => {
                request.finish(Outcome::Truncated)
            }
        }
    }
}

struct Download {
    body: BoxStream<'static, std::result::Result<Bytes, reqwest::Error>>,
    inspection: Option<ResponseInspection>,
    expected_length: Option<u64>,
    received: u64,
    last_received_us: u64,
    ended: bool,
}

impl Download {
    fn finish(&mut self) {
        if !self.ended {
            self.ended = true;
            if let Some(inspection) = &mut self.inspection {
                inspection.finish_at(self.last_received_us);
            }
        }
    }

    fn stream(
        self,
    ) -> impl futures_util::Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send {
        futures_util::stream::unfold(Some(self), |state| async move {
            let mut download = state?;
            if download.ended {
                return None;
            }
            match download.body.next().await {
                Some(Ok(chunk)) => {
                    download.received = download.received.saturating_add(chunk.len() as u64);
                    // Observe arrival before yielding native bytes downstream;
                    // JSON parsing/ledger I/O must not become generation time.
                    if let Some(inspection) = &mut download.inspection {
                        download.last_received_us = inspection.elapsed_us();
                        inspection.push_at(&chunk, download.last_received_us);
                    }
                    if download.expected_length == Some(download.received) {
                        download.finish();
                    }
                    Some((Ok(chunk), Some(download)))
                }
                Some(Err(_)) => {
                    if let Some(inspection) = &download.inspection {
                        inspection.error();
                    }
                    Some((
                        Err(std::io::Error::other(
                            "upstream response body was interrupted",
                        )),
                        None,
                    ))
                }
                None => {
                    download.finish();
                    None
                }
            }
        })
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        if !self.ended
            && let Some(inspection) = &self.inspection
        {
            inspection.cancel();
        }
    }
}

struct EphemeralState {
    target: Arc<ForwardTarget>,
    host: String,
    mount: String,
}

fn router(state: Arc<EphemeralState>) -> Router {
    // A fallback intentionally does not path-decode a wildcard before the
    // capability boundary/path validation. Unknown vendor routes still relay.
    Router::new().fallback(ephemeral_relay).with_state(state)
}

async fn ephemeral_relay(State(state): State<Arc<EphemeralState>>, request: Request) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return rejected(
            StatusCode::FORBIDDEN,
            "browser-origin API forwarding is not allowed",
        );
    }
    let mut hosts = request.headers().get_all(header::HOST).iter();
    let host = hosts.next().and_then(|host| host.to_str().ok());
    if host != Some(state.host.as_str())
        || hosts.next().is_some()
        || request
            .uri()
            .authority()
            .is_some_and(|authority| authority.as_str() != state.host)
    {
        return rejected(StatusCode::FORBIDDEN, "invalid loopback forwarding host");
    }
    let path = request.uri().path();
    // Compare the raw capability before decoding any path segment. A token is
    // an authentication secret, not a route wildcard or a query parameter.
    if !path
        .get(..state.mount.len())
        .is_some_and(|prefix| constant_time_eq(prefix.as_bytes(), state.mount.as_bytes()))
    {
        return rejected(
            StatusCode::NOT_FOUND,
            "unknown direct API forwarding capability",
        );
    }
    let suffix = &path[state.mount.len()..];
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return rejected(
            StatusCode::NOT_FOUND,
            "unknown direct API forwarding capability",
        );
    }
    let suffix = suffix.to_owned();
    relay(state.target.clone(), &suffix, request).await
}

pub(crate) struct ForwardServer {
    port: u16,
    mount: String,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ForwardServer {
    pub(crate) fn start(
        upstream: &str,
        ledger: Arc<Ledger>,
        expected_key_digests: Vec<String>,
    ) -> Result<Self> {
        let target = Arc::new(ForwardTarget::new(upstream, ledger, expected_key_digests)?);
        let mount = format!("/{}", crate::remote::generate_token()?);
        let listener = TcpListener::bind("127.0.0.1:0")
            .context("could not reserve a loopback API forwarding port")?;
        let port = listener.local_addr()?.port();
        listener
            .set_nonblocking(true)
            .context("could not prepare the loopback API forwarding listener")?;
        let state = Arc::new(EphemeralState {
            target,
            host: format!("127.0.0.1:{port}"),
            mount: mount.clone(),
        });
        let (shutdown, stopped) = oneshot::channel();
        let (ready, started) = std::sync::mpsc::channel::<std::result::Result<(), &'static str>>();
        let thread = thread::Builder::new()
            .name("api-forward".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = ready.send(Err("could not start the API forwarding runtime"));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::from_std(listener) {
                        Ok(listener) => listener,
                        Err(_) => {
                            let _ = ready.send(Err("could not start the API forwarding listener"));
                            return;
                        }
                    };
                    let app = router(state);
                    let (draining, drain) = oneshot::channel();
                    let server = axum::serve(listener, app)
                        .with_graceful_shutdown(async move {
                            let _ = stopped.await;
                            let _ = draining.send(());
                        })
                        .into_future();
                    let bounded_shutdown = async move {
                        let _ = drain.await;
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    };
                    let _ = ready.send(Ok(()));
                    futures_util::pin_mut!(server, bounded_shutdown);
                    let _ = futures_util::future::select(server, bounded_shutdown).await;
                });
                runtime.shutdown_timeout(Duration::from_secs(1));
            })
            .context("could not start the API forwarding thread")?;
        let server = Self {
            port,
            mount,
            shutdown: Some(shutdown),
            thread: Some(thread),
        };
        match started.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(server),
            Ok(Err(message)) => bail!("{message}"),
            Err(_) => bail!("API forwarding did not become ready within 10 seconds"),
        }
    }

    pub(crate) fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}{}", self.port, self.mount)
    }
}

impl Drop for ForwardServer {
    fn drop(&mut self) {
        // Stop accepting immediately, allow a short drain, then drop the server
        // future/runtime so long-lived requests cannot strand a local listener.
        self.shutdown.take();
        if let Some(thread) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !thread.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Mutex, PoisonError};

    use axum::body::to_bytes;
    use axum::http::HeaderValue;
    use tower::ServiceExt;

    use super::*;
    use crate::config::{Agent, ProviderKind};
    use crate::usage::ledger::{LEDGER_FILE, read_records};
    use crate::usage::records::UsageRecord;

    #[derive(Clone)]
    enum ReplyBody {
        Bytes(Bytes),
        Timed(Vec<(Duration, std::result::Result<Bytes, &'static str>)>),
    }

    #[derive(Clone)]
    struct Reply {
        status: StatusCode,
        headers: HeaderMap,
        body: ReplyBody,
    }

    impl Reply {
        fn json(body: impl Into<Bytes>) -> Self {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            Self {
                status: StatusCode::OK,
                headers,
                body: ReplyBody::Bytes(body.into()),
            }
        }

        fn sse(chunks: Vec<(Duration, std::result::Result<Bytes, &'static str>)>) -> Self {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream; charset=utf-8"),
            );
            Self {
                status: StatusCode::OK,
                headers,
                body: ReplyBody::Timed(chunks),
            }
        }
    }

    struct Received {
        method: Method,
        uri: String,
        headers: HeaderMap,
        body: Bytes,
    }

    struct Script {
        reply: Reply,
        received: Arc<Mutex<Vec<Received>>>,
    }

    struct FakeServer {
        base_url: String,
        received: Arc<Mutex<Vec<Received>>>,
        shutdown: Option<oneshot::Sender<()>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl FakeServer {
        async fn start(reply: Reply) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let received = Arc::new(Mutex::new(Vec::new()));
            let state = Arc::new(Script {
                reply,
                received: received.clone(),
            });
            let app = Router::new().fallback(fake_reply).with_state(state);
            let (shutdown, stopped) = oneshot::channel();
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = stopped.await;
                    })
                    .await;
            });
            Self {
                base_url,
                received,
                shutdown: Some(shutdown),
                task,
            }
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.shutdown.take();
            self.task.abort();
        }
    }

    async fn fake_reply(State(state): State<Arc<Script>>, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, usize::MAX).await.unwrap();
        state
            .received
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Received {
                method: parts.method,
                uri: parts.uri.to_string(),
                headers: parts.headers,
                body,
            });
        let reply = state.reply.clone();
        let body = match reply.body {
            ReplyBody::Bytes(bytes) => Body::from(bytes),
            ReplyBody::Timed(chunks) => Body::from_stream(futures_util::stream::unfold(
                VecDeque::from(chunks),
                |mut chunks| async move {
                    let (delay, chunk) = chunks.pop_front()?;
                    tokio::time::sleep(delay).await;
                    Some((chunk.map_err(std::io::Error::other), chunks))
                },
            )),
        };
        let mut response = Response::new(body);
        *response.status_mut() = reply.status;
        *response.headers_mut() = reply.headers;
        response
    }

    fn ledger(dir: &std::path::Path) -> Arc<Ledger> {
        Arc::new(Ledger::new(
            dir.join(LEDGER_FILE),
            Agent::Claude,
            "test-profile".to_owned(),
            ProviderKind::Openai,
            None,
        ))
    }

    fn native_request(path: &str, body: impl Into<Bytes>) -> Request {
        let body = body.into();
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, body.len())
            .body(Body::from(body))
            .unwrap()
    }

    fn test_client() -> Client {
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .unwrap()
    }

    async fn records(dir: &std::path::Path, count: usize) -> Vec<UsageRecord> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let records = read_records(dir).records;
                if records.len() >= count {
                    return records;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("observation did not reach the ledger")
    }

    #[test]
    fn digests_and_endpoint_identity_never_contain_credentials_or_query() {
        assert_eq!(
            key_digest("  abc \n"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sanitized_endpoint(
                "https://user:secret@example.com:8443/prefix/v1?api_key=private#fragment"
            )
            .unwrap(),
            "https://example.com:8443/prefix/v1"
        );
        let dir = tempfile::tempdir().unwrap();
        assert!(
            ForwardTarget::new(
                "https://user:secret@example.com/v1",
                ledger(dir.path()),
                Vec::new()
            )
            .is_err()
        );
        assert!(parse_upstream("https://example.com/v1").is_ok());
        for endpoint in [
            "http://localhost:1234/v1",
            "http://127.0.0.1:1234/v1",
            "http://[::1]:1234/v1",
        ] {
            assert!(parse_upstream(endpoint).is_ok(), "{endpoint}");
        }
        for endpoint in [
            "http://example.com/v1",
            "http://localhost.evil/v1",
            "http://0.0.0.0:1234",
            "http://127.1",
            "http://2130706433",
            "http://[::ffff:127.0.0.1]",
            "ftp://localhost/v1",
            "localhost:1234/v1",
        ] {
            assert!(parse_upstream(endpoint).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn raw_endpoint_normalization_cannot_hide_a_nonloopback_http_destination() {
        let differential = "http://outside.example\\@127.0.0.1/v1";
        assert_eq!(
            Url::parse(differential).unwrap().host_str(),
            Some("outside.example")
        );
        let dir = tempfile::tempdir().unwrap();
        for endpoint in [
            differential,
            "http://outside.example\\@localhost/v1",
            "http://outside.example\\@127.0.0.1:1234/v1",
            "http://outside.example\\@127.0.0.1\\v1",
            "http://127.0.0.1\\v1",
            "http://127.0.0.1/with\\backslash",
            "http://127.0.0.1/v1?query=raw\\backslash",
            "http://127.0.\t0.1/v1",
            "http://localhost/with\nnewline",
            "http://localhost/v1?query=carriage\rreturn",
            "http://localhost/v1?query=raw space",
            "http://localhost/v1#fragment\x7f",
            "http://localhost/v1\0",
            " http://127.0.0.1/v1",
            "http://127.0.0.1/v1 ",
            "https://outside.example\\@127.0.0.1/v1",
            "https://example.com/v1?query=raw space",
            "https://example.com/with\nnewline",
            "http://127.1/v1",
            "http://2130706433/v1",
            "http://0x7f000001/v1",
            "http://0177.0.0.1/v1",
            "http://%31%32%37.0.0.1/v1",
        ] {
            assert!(parse_upstream(endpoint).is_err(), "{endpoint:?}");
            assert!(sanitized_endpoint(endpoint).is_err(), "{endpoint:?}");
            assert!(
                ForwardTarget::new(endpoint, ledger(dir.path()), Vec::new()).is_err(),
                "{endpoint:?}"
            );
        }
    }

    #[test]
    fn explicit_loopback_paths_queries_and_keyless_targets_remain_valid() {
        let dir = tempfile::tempdir().unwrap();
        for (origin, expected_host) in [
            ("http://localhost:1234", "localhost"),
            ("http://LOCALHOST:1234", "localhost"),
            ("http://127.0.0.1:1234", "127.0.0.1"),
            ("http://[::1]:1234", "[::1]"),
        ] {
            let endpoint = format!(
                "{origin}/gateway%20prefix/v1?api_key=private%2Bvalue&route=one%2Ftwo#fragment"
            );
            let target = ForwardTarget::new(&endpoint, ledger(dir.path()), Vec::new()).unwrap();
            assert_eq!(target.upstream.host_str(), Some(expected_host));
            assert!(target.expected_key_digests.is_empty());
            let joined = target
                .url("/responses", Some("beta=1&beta=2%20three"))
                .unwrap();
            assert_eq!(joined.host_str(), Some(expected_host));
            assert_eq!(joined.path(), "/gateway%20prefix/v1/responses");
            assert_eq!(
                joined.query(),
                Some("api_key=private%2Bvalue&route=one%2Ftwo&beta=1&beta=2%20three")
            );
            assert_eq!(joined.fragment(), None);
            let sanitized = Url::parse(&sanitized_endpoint(&endpoint).unwrap()).unwrap();
            assert_eq!(sanitized.path(), "/gateway%20prefix/v1");
            assert_eq!(sanitized.query(), None);
            assert_eq!(sanitized.fragment(), None);
        }
        assert!(parse_upstream("https://example.com/gateway%20prefix/v1?query=a%20b").is_ok());
    }

    #[test]
    fn suffixes_cannot_replace_authority_or_escape_the_original_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let target = ForwardTarget::new(
            "https://example.com/gateway/v1?key=private%2Bvalue#never",
            ledger(dir.path()),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            target
                .url("/responses", Some("beta=one&beta=two%2Fthree"))
                .unwrap()
                .as_str(),
            "https://example.com/gateway/v1/responses?key=private%2Bvalue&beta=one&beta=two%2Fthree"
        );
        assert_eq!(
            target.url("/v1/messages", None).unwrap().path(),
            "/gateway/v1/v1/messages"
        );
        assert_eq!(
            target.url("/models/model%20name", None).unwrap().path(),
            "/gateway/v1/models/model%20name"
        );
        for suffix in [
            "https://evil.example/",
            "//evil.example/",
            "/../models",
            "/a/./b",
            "/%2e%2e/models",
            "/a/%2F../models",
            "/%5c../models",
            "/%252f../models",
            "/a\\..\\models",
            "/a//models",
            "/models?url=evil",
            "/models#fragment",
            "/%00models",
            "/%",
            "/..;x/models",
        ] {
            assert!(target.url(suffix, None).is_err(), "{suffix}");
        }
        assert_eq!(protocol("/v1/messages"), Some(WireProtocol::Messages));
        assert_eq!(protocol("/responses"), Some(WireProtocol::Responses));
        assert_eq!(protocol("/chat/completions"), Some(WireProtocol::Chat));
        assert_eq!(protocol("/v1/messages/count_tokens"), None);
        assert_eq!(protocol("/models"), None);
    }

    #[tokio::test]
    async fn byte_exact_upload_response_headers_prefix_query_and_nonstream_usage() {
        let bytes = Bytes::from_static(br#" { "id":"resp_original", "model":"served", "service_tier":"default", "usage": {"input_tokens":9,"output_tokens":3}, "output":[] } "#);
        let mut reply = Reply::json(bytes.clone());
        reply
            .headers
            .insert("x-request-id", HeaderValue::from_static("req-vendor"));
        reply.headers.insert(
            "x-ratelimit-remaining-tokens",
            HeaderValue::from_static("123"),
        );
        reply.headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("x-vendor-hop, keep-alive"),
        );
        reply
            .headers
            .insert("x-vendor-hop", HeaderValue::from_static("remove-me"));
        let upstream = FakeServer::start(reply).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(
                &format!(
                    "{}/gateway/v1?private=fixed%2Bvalue#never",
                    upstream.base_url
                ),
                ledger(dir.path()),
                vec![key_digest("real-vendor-key")],
            )
            .unwrap(),
        );
        let body = Bytes::from_static(r#" { "model" : "requested", "stream":false, "service_tier":"priority", "messages" : [ { "content" : "你\n native" } ] } "#.as_bytes());
        let mut request = native_request("/responses?beta=one&beta=two%2Fthree", body.clone());
        for (name, value) in [
            ("authorization", "Bearer real-vendor-key"),
            ("x-api-key", "real-vendor-key"),
            ("anthropic-beta", "cache-beta"),
            ("openai-beta", "responses=experimental"),
            ("x-request-id", "req-client"),
            ("host", "do-not-forward.example"),
            ("connection", "x-client-hop, keep-alive"),
            ("x-client-hop", "remove-me"),
        ] {
            request.headers_mut().insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let response = relay(target, "/responses", request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-request-id"], "req-vendor");
        assert_eq!(response.headers()["x-ratelimit-remaining-tokens"], "123");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert!(!response.headers().contains_key("x-vendor-hop"));
        assert!(!response.headers().contains_key(header::CONNECTION));
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            bytes
        );
        {
            let received = upstream
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            assert_eq!(received.len(), 1);
            assert_eq!(received[0].method, Method::POST);
            assert_eq!(
                received[0].uri,
                "/gateway/v1/responses?private=fixed%2Bvalue&beta=one&beta=two%2Fthree"
            );
            assert_eq!(received[0].body, body);
            assert_eq!(
                received[0].headers[header::AUTHORIZATION],
                "Bearer real-vendor-key"
            );
            assert_eq!(received[0].headers["x-api-key"], "real-vendor-key");
            assert_eq!(received[0].headers["anthropic-beta"], "cache-beta");
            assert_eq!(received[0].headers["openai-beta"], "responses=experimental");
            assert_eq!(received[0].headers["x-request-id"], "req-client");
            assert!(!received[0].headers.contains_key("x-client-hop"));
            assert_ne!(received[0].headers[header::HOST], "do-not-forward.example");
        }
        let rows = records(dir.path(), 1).await;
        assert_eq!(rows.len(), 1);
        let record = &rows[0];
        assert_eq!(record.outcome, Outcome::Completed);
        assert_eq!(record.model.as_deref(), Some("served"));
        assert_eq!(record.service_tier.as_deref(), Some("default"));
        assert_eq!(
            record.endpoint.as_deref(),
            Some(format!("{}/gateway/v1", upstream.base_url).as_str())
        );
        assert_eq!(record.tokens.input_tokens, Some(9));
        assert_eq!(record.tokens.output_tokens, Some(3));
        assert_eq!(record.tokens.cache_read_tokens, None);
        assert_eq!(record.tokens.reasoning_tokens, None);
        assert_eq!(record.metrics().ttft_ms, None);
        assert_eq!(record.metrics().stream_tps, None);
        assert!(!record.timing.as_ref().unwrap().client_streaming);
        assert!(record.timing.as_ref().unwrap().terminal_us.is_some());
        let metadata = std::fs::read_to_string(dir.path().join(LEDGER_FILE)).unwrap();
        assert!(!metadata.contains("real-vendor-key"));
        assert!(!metadata.contains("private="));
        assert!(!metadata.contains("messages"));
        assert!(!metadata.contains("native"));
    }

    #[tokio::test]
    async fn router_rejects_origin_host_wrong_capability_key_and_unsafe_suffix() {
        let upstream = FakeServer::start(Reply::json(Bytes::from_static(b"{}"))).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(
                &upstream.base_url,
                ledger(dir.path()),
                vec![key_digest("vendor-key")],
            )
            .unwrap(),
        );
        let app = router(Arc::new(EphemeralState {
            target,
            host: "127.0.0.1:12345".to_owned(),
            mount: "/secret-cap".to_owned(),
        }));
        let valid = || {
            let mut request = native_request("/secret-cap/responses", Bytes::from_static(b"{}"));
            request
                .headers_mut()
                .insert(header::HOST, HeaderValue::from_static("127.0.0.1:12345"));
            request
                .headers_mut()
                .insert("x-api-key", HeaderValue::from_static("  vendor-key  "));
            request
        };
        let mut rejected_requests = Vec::new();
        let mut request = valid();
        request.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        rejected_requests.push((request, StatusCode::FORBIDDEN));
        let mut request = valid();
        request
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("localhost:12345"));
        rejected_requests.push((request, StatusCode::FORBIDDEN));
        let mut request = valid();
        request.headers_mut().remove(header::HOST);
        rejected_requests.push((request, StatusCode::FORBIDDEN));
        let mut request = valid();
        request
            .headers_mut()
            .append(header::HOST, HeaderValue::from_static("127.0.0.1:12345"));
        rejected_requests.push((request, StatusCode::FORBIDDEN));
        let mut request = valid();
        *request.uri_mut() = "http://evil.example/secret-cap/responses".parse().unwrap();
        rejected_requests.push((request, StatusCode::FORBIDDEN));
        for path in [
            "/wrong-cap/responses",
            "/secret-cap-extra/responses",
            "/%73ecret-cap/responses",
            "/responses?cap=secret-cap",
        ] {
            let mut request = valid();
            *request.uri_mut() = path.parse().unwrap();
            rejected_requests.push((request, StatusCode::NOT_FOUND));
        }
        let mut request = valid();
        request
            .headers_mut()
            .insert("x-api-key", HeaderValue::from_static("wrong-key"));
        rejected_requests.push((request, StatusCode::UNAUTHORIZED));
        let mut request = valid();
        request.headers_mut().remove("x-api-key");
        rejected_requests.push((request, StatusCode::UNAUTHORIZED));
        let mut request = valid();
        request
            .headers_mut()
            .append("x-api-key", HeaderValue::from_static("wrong-key"));
        rejected_requests.push((request, StatusCode::UNAUTHORIZED));
        for path in [
            "/secret-cap/../models",
            "/secret-cap/%2e%2e/models",
            "/secret-cap/%2fmodels",
            "/secret-cap/%252fmodels",
        ] {
            let mut request = valid();
            *request.uri_mut() = path.parse().unwrap();
            rejected_requests.push((request, StatusCode::BAD_REQUEST));
        }
        for (request, status) in rejected_requests {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), status);
        }
        assert!(
            upstream
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        );
        assert!(read_records(dir.path()).records.is_empty());
        let response = app.clone().oneshot(valid()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let mut request = valid();
        request.headers_mut().remove("x-api-key");
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("  bEaReR   vendor-key  "),
        );
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(records(dir.path(), 2).await.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ephemeral_server_keyless_access_exists_only_on_its_capability_mount() {
        let upstream =
            FakeServer::start(Reply::json(Bytes::from_static(br#"{"usage":null}"#))).await;
        let dir = tempfile::tempdir().unwrap();
        let server =
            ForwardServer::start(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap();
        let base = server.base_url();
        assert_eq!(Url::parse(&base).unwrap().host_str(), Some("127.0.0.1"));
        assert_eq!(Url::parse(&base).unwrap().path().len(), 44);
        let client = test_client();
        let response = client
            .post(format!("{base}/responses"))
            .body(r#"{"model":"keyless","stream":false}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.bytes().await.unwrap(),
            Bytes::from_static(br#"{"usage":null}"#)
        );
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.model.as_deref(), Some("keyless"));
        assert_eq!(record.tokens.input_tokens, None);
        assert_eq!(record.tokens.output_tokens, None);
        assert_eq!(record.outcome, Outcome::Completed);
        assert_eq!(record.metrics().ttft_ms, None);
        let root = format!("http://127.0.0.1:{}", server.port);
        assert_eq!(
            client
                .post(format!("{root}/responses"))
                .body("{}")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .post(format!("{root}/wrong-cap/responses"))
                .body("{}")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .post(format!("{base}/responses"))
                .header(header::ORIGIN, "null")
                .body("{}")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .post(format!("{base}/responses"))
                .header(header::HOST, "evil.example")
                .body("{}")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            upstream
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            1
        );
        let address = format!("127.0.0.1:{}", server.port);
        let at = Instant::now();
        drop(server);
        assert!(at.elapsed() < Duration::from_secs(4));
        assert!(std::net::TcpStream::connect(address).is_err());
    }

    #[tokio::test]
    async fn token_count_models_and_unknown_routes_never_create_usage_records() {
        let native = Bytes::from_static(br#"{"usage":{"input_tokens":777,"output_tokens":777}}"#);
        let upstream = FakeServer::start(Reply::json(native.clone())).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(
                &format!("{}/prefix", upstream.base_url),
                ledger(dir.path()),
                Vec::new(),
            )
            .unwrap(),
        );
        for suffix in [
            "/v1/messages/count_tokens",
            "/messages/count_tokens",
            "/models",
            "/v1/models",
            "/models/provider-model",
            "/unrecognized/vendor/operation",
        ] {
            let response = relay(
                target.clone(),
                suffix,
                native_request(
                    suffix,
                    Bytes::from_static(br#"{"model":"m","stream":true}"#),
                ),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                native
            );
        }
        assert_eq!(
            upstream
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            6
        );
        assert!(read_records(dir.path()).records.is_empty());
    }

    #[tokio::test]
    async fn null_usage_success_preserves_na_with_request_model_and_explicit_tier() {
        let native = Bytes::from_static(br#"{"id":"resp_null","usage":null}"#);
        let upstream = FakeServer::start(Reply::json(native.clone())).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
        );
        let response = relay(
            target,
            "/responses",
            native_request(
                "/responses",
                Bytes::from_static(
                    br#"{"model":"requested","stream":false,"service_tier":"priority"}"#,
                ),
            ),
        )
        .await;
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            native
        );
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.outcome, Outcome::Completed);
        assert_eq!(record.model.as_deref(), Some("requested"));
        assert_eq!(record.service_tier.as_deref(), Some("priority"));
        assert_eq!(record.tokens.input_tokens, None);
        assert_eq!(record.tokens.output_tokens, None);
        assert_eq!(record.tokens.cache_read_tokens, None);
        assert_eq!(record.tokens.cache_write_tokens, None);
        assert_eq!(record.tokens.reasoning_tokens, None);
        assert_eq!(record.metrics().ttft_ms, None);
        assert_eq!(record.metrics().e2e_tps, None);
        let json = serde_json::to_value(record).unwrap();
        assert!(json["tokens"]["input_tokens"].is_null());
        assert!(json["tokens"]["output_tokens"].is_null());
    }

    #[tokio::test]
    async fn malformed_and_oversized_metadata_does_not_reject_or_rewrite_native_bytes() {
        for body in [
            Bytes::from_static(b"not request json"),
            Bytes::from(vec![b'x'; MAX_LINE_BYTES + 100]),
        ] {
            let reply_bytes = Bytes::from_static(br#"{"model":"served-fallback","usage":null}"#);
            let upstream = FakeServer::start(Reply::json(reply_bytes.clone())).await;
            let dir = tempfile::tempdir().unwrap();
            let target = Arc::new(
                ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
            );
            let chunks: Vec<_> = body
                .chunks(32 * 1024)
                .map(Bytes::copy_from_slice)
                .map(Ok::<_, std::io::Error>)
                .collect();
            let request = Request::builder()
                .method(Method::POST)
                .uri("/responses")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_LENGTH, body.len())
                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                .unwrap();
            let response = relay(target, "/responses", request).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                reply_bytes
            );
            assert_eq!(
                upstream
                    .received
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)[0]
                    .body,
                body
            );
            let record = records(dir.path(), 1).await.remove(0);
            assert_eq!(record.outcome, Outcome::Completed);
            assert_eq!(record.model.as_deref(), Some("served-fallback"));
            assert!(!record.timing.as_ref().unwrap().client_streaming);
            assert!(!record.warnings.is_empty());
        }
        for reply_bytes in [
            Bytes::from_static(b"not response json"),
            Bytes::from(vec![b'x'; MAX_LINE_BYTES + 100]),
        ] {
            let upstream = FakeServer::start(Reply::json(reply_bytes.clone())).await;
            let dir = tempfile::tempdir().unwrap();
            let target = Arc::new(
                ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
            );
            let response = relay(
                target,
                "/responses",
                native_request("/responses", Bytes::from_static(b"{}")),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                reply_bytes
            );
            let record = records(dir.path(), 1).await.remove(0);
            assert_eq!(record.outcome, Outcome::Completed);
            assert_eq!(record.tokens.input_tokens, None);
            assert_eq!(record.tokens.output_tokens, None);
            assert!(!record.warnings.is_empty());
        }
    }

    #[tokio::test]
    async fn compressed_native_bytes_and_encoding_headers_are_not_decoded() {
        let bytes = Bytes::from_static(b"\x1f\x8bopaque-vendor-body\x00\xff");
        let mut reply = Reply::json(bytes.clone());
        reply
            .headers
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        reply.headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&bytes.len().to_string()).unwrap(),
        );
        let upstream = FakeServer::start(reply).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
        );
        let mut request = native_request("/responses", bytes.clone());
        request
            .headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        let response = relay(target, "/responses", request).await;
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            bytes.len().to_string()
        );
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            bytes
        );
        assert_eq!(
            upstream
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner)[0]
                .body,
            bytes
        );
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.outcome, Outcome::Completed);
        assert_eq!(record.tokens.output_tokens, None);
        assert_eq!(record.model, None);
        assert!(
            record
                .warnings
                .iter()
                .any(|warning| warning.contains("encoded response"))
        );
    }

    #[tokio::test]
    async fn redirects_and_failed_http_are_forwarded_once_without_resending_credentials() {
        let destination = FakeServer::start(Reply::json(Bytes::from_static(b"{}"))).await;
        for status in [
            StatusCode::TEMPORARY_REDIRECT,
            StatusCode::BAD_GATEWAY,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            let native = Bytes::from_static(br#"{"error":{"message":"native vendor response"},"usage":{"input_tokens":3,"output_tokens":4}}"#);
            let mut reply = Reply::json(native.clone());
            reply.status = status;
            reply.headers.insert(
                header::LOCATION,
                HeaderValue::from_str(&format!("{}/responses", destination.base_url)).unwrap(),
            );
            reply
                .headers
                .insert("retry-after", HeaderValue::from_static("0"));
            let upstream = FakeServer::start(reply).await;
            let dir = tempfile::tempdir().unwrap();
            let target = Arc::new(
                ForwardTarget::new(
                    &upstream.base_url,
                    ledger(dir.path()),
                    vec![key_digest("vendor-key")],
                )
                .unwrap(),
            );
            let mut request = native_request("/responses", Bytes::from_static(b"{}"));
            request.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_static("Bearer vendor-key"),
            );
            let response = relay(target, "/responses", request).await;
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["retry-after"], "0");
            assert_eq!(
                to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                native
            );
            assert_eq!(
                upstream
                    .received
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .len(),
                1
            );
            let rows = records(dir.path(), 1).await;
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].outcome, Outcome::Failed);
            assert_eq!(rows[0].tokens.input_tokens, None);
            assert_eq!(rows[0].tokens.output_tokens, None);
        }
        assert!(
            destination
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn native_messages_sse_metadata_is_not_ttft_and_thinking_is_gross_output() {
        let frames = [
            Bytes::from_static(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg-native\",\"model\":\"served\",\"usage\":{\"input_tokens\":8,\"cache_read_input_tokens\":2,\"cache_creation_input_tokens\":0,\"output_tokens\":0}}}\n\n"),
            Bytes::from_static(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"\"}}\n\n"),
            Bytes::from_static(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"native thinking\"}}\n\n"),
            Bytes::from_static(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n"),
            Bytes::from_static(b"event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":6}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
        ];
        let native: Vec<u8> = frames
            .iter()
            .flat_map(|chunk| chunk.iter().copied())
            .collect();
        let reply = Reply::sse(
            frames
                .into_iter()
                .enumerate()
                .map(|(index, chunk)| {
                    (
                        if index == 0 {
                            Duration::ZERO
                        } else {
                            Duration::from_millis(35)
                        },
                        Ok(chunk),
                    )
                })
                .collect(),
        );
        let upstream = FakeServer::start(reply).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
        );
        let response = relay(
            target,
            "/v1/messages",
            native_request(
                "/v1/messages",
                Bytes::from_static(br#"{"model":"requested","stream":true}"#),
            ),
        )
        .await;
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream; charset=utf-8"
        );
        let mut body = response.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        assert!(first.starts_with(b"event: message_start"));
        assert!(read_records(dir.path()).records.is_empty());
        let mut original = first.to_vec();
        while let Some(chunk) = body.next().await {
            original.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(original, native);
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.outcome, Outcome::Completed);
        assert_eq!(record.model.as_deref(), Some("served"));
        assert_eq!(record.tokens.input_tokens, Some(8));
        assert_eq!(record.tokens.output_tokens, Some(6));
        assert_eq!(record.tokens.cache_read_tokens, Some(2));
        assert_eq!(record.tokens.cache_write_tokens, Some(0));
        let timing = record.timing.as_ref().unwrap();
        assert!(timing.client_streaming);
        assert_eq!(timing.output_basis, OutputBasis::Gross);
        assert!(timing.first_content_us.unwrap() >= 50_000);
        assert!(timing.terminal_us.unwrap() > timing.first_content_us.unwrap());
        assert!(record.metrics().ttft_ms.is_some());
        assert!(record.metrics().stream_tps.is_some());
        assert!(
            !std::fs::read_to_string(dir.path().join(LEDGER_FILE))
                .unwrap()
                .contains("native thinking")
        );
    }

    #[tokio::test]
    async fn chat_and_responses_sse_do_not_assume_reasoning_zero_or_infer_streaming_from_content_type()
     {
        let streams = [
            ("/responses", Bytes::from_static(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n")),
            ("/chat/completions", Bytes::from_static(b"data: {\"id\":\"chat-native\",\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n")),
        ];
        for (suffix, native) in streams {
            let upstream =
                FakeServer::start(Reply::sse(vec![(Duration::ZERO, Ok(native.clone()))])).await;
            for request_body in [
                Bytes::from_static(br#"{"stream":true}"#),
                Bytes::from_static(b"{}"),
                Bytes::from_static(br#"{"stream":false}"#),
                Bytes::from_static(b"malformed"),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let target = Arc::new(
                    ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
                );
                let response =
                    relay(target, suffix, native_request(suffix, request_body.clone())).await;
                assert_eq!(
                    to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                    native
                );
                let record = records(dir.path(), 1).await.remove(0);
                assert_eq!(record.outcome, Outcome::Completed);
                assert_eq!(record.tokens.output_tokens, Some(2));
                assert_eq!(record.tokens.reasoning_tokens, None);
                assert_eq!(
                    record.timing.as_ref().unwrap().output_basis,
                    OutputBasis::Unknown
                );
                assert_eq!(
                    record.timing.as_ref().unwrap().client_streaming,
                    request_body.as_ref() == br#"{"stream":true}"#
                );
                assert_eq!(record.metrics().stream_tps, None);
                if request_body.as_ref() != br#"{"stream":true}"# {
                    assert_eq!(record.metrics().ttft_ms, None);
                }
            }
        }
    }

    #[tokio::test]
    async fn sse_eof_cancellation_and_body_errors_have_distinct_outcomes() {
        let native = Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n",
        );
        let upstream =
            FakeServer::start(Reply::sse(vec![(Duration::ZERO, Ok(native.clone()))])).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
        );
        let response = relay(
            target,
            "/responses",
            native_request("/responses", Bytes::from_static(br#"{"stream":true}"#)),
        )
        .await;
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            native
        );
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.outcome, Outcome::Truncated);
        assert_eq!(record.timing.as_ref().unwrap().terminal_us, None);
        assert_eq!(record.tokens.output_tokens, None);

        let upstream = FakeServer::start(Reply::sse(vec![
            (Duration::ZERO, Ok(native.clone())),
            (
                Duration::from_secs(30),
                Ok(Bytes::from_static(b"data: [DONE]\n\n")),
            ),
        ]))
        .await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
        );
        let response = relay(
            target,
            "/responses",
            native_request("/responses", Bytes::from_static(br#"{"stream":true}"#)),
        )
        .await;
        let mut body = response.into_body().into_data_stream();
        assert_eq!(body.next().await.unwrap().unwrap(), native);
        drop(body);
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.outcome, Outcome::Cancelled);
        assert_eq!(record.timing.as_ref().unwrap().terminal_us, None);
        assert_eq!(
            upstream
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            1
        );

        let upstream = FakeServer::start(Reply::sse(vec![
            (Duration::ZERO, Ok(native)),
            (Duration::from_millis(25), Err("upstream broke")),
        ]))
        .await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
        );
        let response = relay(
            target,
            "/responses",
            native_request("/responses", Bytes::from_static(br#"{"stream":true}"#)),
        )
        .await;
        assert!(to_bytes(response.into_body(), usize::MAX).await.is_err());
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.outcome, Outcome::Truncated);
        assert_eq!(record.tokens.output_tokens, None);
        assert_eq!(
            upstream
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn oversized_sse_inspection_keeps_the_complete_native_response() {
        let mut bytes = b"data: ".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', MAX_LINE_BYTES + 100));
        bytes.extend_from_slice(b"\n\ndata: [DONE]\n\n");
        let native = Bytes::from(bytes);
        let reply = Reply::sse(
            native
                .chunks(32 * 1024)
                .map(|chunk| (Duration::ZERO, Ok(Bytes::copy_from_slice(chunk))))
                .collect(),
        );
        let upstream = FakeServer::start(reply).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
        );
        let response = relay(
            target,
            "/responses",
            native_request("/responses", Bytes::from_static(br#"{"stream":true}"#)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            native
        );
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.tokens.output_tokens, None);
        assert!(
            record
                .warnings
                .iter()
                .any(|warning| warning.contains("limit"))
        );
    }

    #[tokio::test]
    async fn upload_cancellation_keeps_the_cold_upload_inside_the_request_clock() {
        let upstream = FakeServer::start(Reply::json(Bytes::from_static(b"{}"))).await;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(
            ForwardTarget::new(&upstream.base_url, ledger(dir.path()), Vec::new()).unwrap(),
        );
        let (first, inspected) = oneshot::channel();
        let stream = futures_util::stream::once(async move {
            let _ = first.send(());
            Ok::<_, std::io::Error>(Bytes::from_static(b"{"))
        })
        .chain(futures_util::stream::pending());
        let request = Request::builder()
            .method(Method::POST)
            .uri("/responses")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from_stream(stream))
            .unwrap();
        let task = tokio::spawn(async move { relay(target, "/responses", request).await });
        tokio::time::timeout(Duration::from_secs(2), inspected)
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        task.abort();
        let _ = task.await;
        let record = records(dir.path(), 1).await.remove(0);
        assert_eq!(record.outcome, Outcome::Cancelled);
        assert!(record.timing.as_ref().unwrap().elapsed_us >= 20_000);
        assert_eq!(record.timing.as_ref().unwrap().terminal_us, None);
        assert_eq!(record.tokens.input_tokens, None);
    }
}
