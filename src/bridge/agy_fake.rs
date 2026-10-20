//! A loopback stand-in for the Code Assist endpoint, so the transport, the
//! translators and the surfaces can be driven through a real socket without
//! a network or a login.
//!
//! It binds `127.0.0.1` only, answers one connection at a time, and its
//! thread is stopped and joined when it is dropped: a server thread that
//! outlived its test could hold the test runner's output open.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;

/// One canned answer.
#[derive(Clone)]
pub(crate) struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    content_type: &'static str,
    body: String,
    /// Bytes promised in `content-length` but never sent: a body that breaks off.
    missing: usize,
}

impl Reply {
    pub(crate) fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            headers: Vec::new(),
            content_type: "application/json",
            body: body.to_string(),
            missing: 0,
        }
    }

    pub(crate) fn sse(body: &str) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            content_type: "text/event-stream",
            body: body.to_owned(),
            missing: 0,
        }
    }

    /// Promises 64 bytes more than it sends, then closes.
    pub(crate) fn truncated(mut self) -> Self {
        self.missing = 64;
        self
    }

    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// One request as it arrived.
#[derive(Clone, Debug)]
pub(crate) struct Recorded {
    pub path: String,
    /// The request line and headers, lowercased.
    pub headers: String,
    pub body: Value,
}

pub(crate) struct FakeCodeAssist {
    pub base: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeCodeAssist {
    /// Answers `loadCodeAssist` with the next of `load` and each
    /// `streamGenerateContent` with the next of `generate`; the last of each
    /// repeats. Anything else is a 404.
    pub(crate) fn start(load: Vec<Reply>, generate: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let base = format!("http://{}/v1internal:", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (log, halt) = (Arc::clone(&requests), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            let (mut loads, mut generates) = (0_usize, 0_usize);
            while !halt.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let Some(recorded) = read(&stream) else {
                            continue;
                        };
                        let reply = if recorded.path.contains("loadCodeAssist") {
                            loads += 1;
                            load.get(loads - 1).or(load.last()).cloned()
                        } else if recorded.path.contains("streamGenerateContent") {
                            generates += 1;
                            generate.get(generates - 1).or(generate.last()).cloned()
                        } else {
                            None
                        };
                        log.lock().unwrap().push(recorded);
                        answer(
                            stream,
                            reply.unwrap_or_else(|| Reply::json(404, Value::Null)),
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    pub(crate) fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    /// How many requests named `method` (`loadCodeAssist`, `streamGenerateContent`).
    pub(crate) fn count(&self, method: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.path.contains(method))
            .count()
    }
}

impl Drop for FakeCodeAssist {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Reads the head and exactly `content-length` bytes of body: on Windows a
/// close with unread bytes is a reset the client sees as a failure.
fn read(mut stream: &TcpStream) -> Option<Recorded> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut wanted = None;
    loop {
        if wanted.is_none()
            && let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            wanted = Some((end, end + 4 + length));
        }
        if let Some((_, total)) = wanted
            && bytes.len() >= total
        {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
        }
    }
    let (end, total) = wanted?;
    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
    let path = String::from_utf8_lossy(&bytes[..end])
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_owned();
    let body =
        serde_json::from_slice(&bytes[end + 4..total.min(bytes.len())]).unwrap_or(Value::Null);
    Some(Recorded {
        path,
        headers,
        body,
    })
}

fn answer(mut stream: TcpStream, reply: Reply) {
    let mut head = format!(
        "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n",
        reply.status,
        reply.content_type,
        reply.body.len() + reply.missing
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(reply.body.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// A whole bridge on the Antigravity upstream, pointed at `fake`, signed with
/// a synthetic `tok-a`.
pub(crate) fn agy_bridge(
    fake: &FakeCodeAssist,
    ledger: Option<std::path::PathBuf>,
    effort: Option<crate::config::ReasoningEffort>,
) -> Arc<crate::bridge::BridgeState> {
    use crate::bridge::agy_auth::test_support::{MemoryStore, credential};
    use crate::bridge::agy_auth::{AgyCredentials, AgyLocator};
    use crate::bridge::agy_upstream::{AgyState, AgyUpstream};
    let credentials = AgyCredentials::with_parts(
        Arc::new(MemoryStore::with_file(credential("tok-a", 3600))),
        std::env::temp_dir().join("alc-agy-bridge-test.lock"),
        None,
    );
    let agy = AgyState::from_upstream(
        AgyUpstream::with_base(credentials, fake.base.clone())
            .unwrap()
            .without_backoff(),
    );
    let config = crate::bridge::BridgeConfig {
        auth_file: std::path::PathBuf::new(),
        effort,
        responses_api: true,
        agent: crate::config::Agent::Claude,
        provider: "antigravity".to_owned(),
        ledger,
        code_assist: None,
        claude_tiers: Some(crate::antigravity::tiers("claude-sonnet-5-5")),
        upstream: crate::bridge::UpstreamKind::Antigravity {
            credential: AgyLocator {
                file: std::path::PathBuf::from("unused"),
            },
            config_dir: std::env::temp_dir(),
        },
    };
    Arc::new(
        crate::bridge::BridgeState::with_upstream(
            config,
            crate::bridge::Upstream::Antigravity(Arc::new(agy)),
        )
        .unwrap(),
    )
}

/// POSTs `body` through the bridge's router and reads the whole answer.
pub(crate) async fn post_json(
    state: Arc<crate::bridge::BridgeState>,
    path: &str,
    body: Value,
) -> (axum::http::StatusCode, String) {
    use tower::ServiceExt as _;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let response = crate::bridge::server::router(state)
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}
