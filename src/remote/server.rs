//! The session server: static page, JSON routes, and the WebSocket that
//! carries a session's terminal.
//!
//! Thread per connection, blocking sockets, no runtime. The concurrency here
//! is a handful of viewers on one machine, and an async runtime would add
//! seventy crates to buy scheduling this does not need - on Windows a ConPTY
//! needs a thread per pipe regardless, so the hard half would stay
//! thread-shaped anyway.
//!
//! Routing is deliberately dull: a fixed table of static assets, three JSON
//! routes, and one upgrade path. There is no filesystem behind any of it, so
//! path traversal has nothing to reach. Every request - including the ones
//! that need no token - goes through `Guard::check`, so a route added later
//! cannot accidentally be added past the `Host` and `Origin` checks.

use std::collections::BTreeMap;
use std::io::{BufReader, Cursor, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tungstenite::Message;

use crate::remote::assets;
use crate::remote::caps::{CAPS, SafetyRung};
use crate::remote::fanout::Frame;
use crate::remote::permission::{self, EscalationGate};
use crate::remote::request::{Denied, Grade, Method, Request, read_request};
use crate::remote::session::Session;
use crate::remote::settings::{Bind, RemoteSettings, Secrets};
use crate::remote::wire::{ClientFrame, Hello, NoticeLevel, ServerFrame};

/// How long a peer may take to send a complete request head before the
/// connection is dropped. Bounds a trickle that would otherwise hold a
/// thread open indefinitely.
const HEAD_TIMEOUT: Duration = Duration::from_secs(15);

/// The largest request head accepted, matching `request.rs`'s own bound.
const MAX_HEAD: usize = 16 * 1024;

/// How long a WebSocket thread waits for a client frame before turning back
/// to drain its output queue. Short enough that output is not delayed
/// perceptibly, long enough not to spin.
const WS_TICK: Duration = Duration::from_millis(40);

/// How often a viewer is pinged, and how long it may go without answering.
///
/// A phone that walks into a tunnel does not close its socket - the
/// connection just stops, and without this the server would hold the slot
/// until the operating system's own keepalive gave up, which can be minutes.
const PING_EVERY: Duration = Duration::from_secs(20);
const PONG_DEADLINE: Duration = Duration::from_secs(60);

/// Every session this server can show. One entry today, a map because the
/// hub that outlives a terminal is the next milestone and this is the shape
/// it needs.
#[derive(Default)]
pub(crate) struct Registry {
    sessions: Mutex<BTreeMap<String, Arc<Session>>>,
}

impl Registry {
    pub(crate) fn insert(&self, session: Arc<Session>) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(session.id().to_owned(), session);
        }
    }

    pub(crate) fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(id).cloned())
    }

    pub(crate) fn remove(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(id);
        }
    }

    pub(crate) fn cards(&self) -> Vec<crate::remote::wire::SessionCard> {
        self.sessions
            .lock()
            .map(|sessions| sessions.values().map(|session| session.card()).collect())
            .unwrap_or_default()
    }
}

/// Everything a connection thread needs that outlives the connection.
/// Named for what it does rather than `Context`, which `anyhow` already
/// owns in this file.
struct Serving {
    guard: crate::remote::request::Guard,
    registry: Arc<Registry>,
    gate: EscalationGate,
    /// The loosest rung a browser may reach without a confirmation typed at
    /// a terminal on this machine.
    ceiling: SafetyRung,
}

pub(crate) struct Server {
    listener: TcpListener,
    context: Arc<Serving>,
    connections: Arc<AtomicUsize>,
    max_connections: usize,
    address: SocketAddr,
}

impl Server {
    /// Binds the session server.
    ///
    /// Loopback unless the user has asked for a LAN bind twice over - once
    /// in the settings file and once on the command line. One switch is too
    /// easy to leave on by accident, and what is on the other side of this
    /// socket is a shell.
    pub(crate) fn bind(
        config_dir: &std::path::Path,
        settings: &RemoteSettings,
        secrets: &Secrets,
        lan_requested: bool,
    ) -> Result<Self> {
        let lan = lan_requested && settings.allow_lan && settings.bind == Bind::Lan;
        let host = if lan {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        };

        let listener = TcpListener::bind(SocketAddr::new(host, settings.port))
            .or_else(|_| TcpListener::bind(SocketAddr::new(host, 0)))
            .context("failed to bind the remote-control server")?;
        let address = listener
            .local_addr()
            .context("failed to read the remote-control server's address")?;

        let guard = crate::remote::request::Guard {
            hosts: allowed_hosts(address, settings, lan),
            origins: allowed_origins(address, settings, lan),
            operator: secrets.operator.clone(),
            viewer: secrets.viewer.clone(),
        };
        let ceiling = settings
            .max_permission
            .parse()
            .unwrap_or(SafetyRung::AutoEdit);

        Ok(Self {
            listener,
            context: Arc::new(Serving {
                guard,
                registry: Arc::new(Registry::default()),
                gate: EscalationGate::new(config_dir)?,
                ceiling,
            }),
            connections: Arc::new(AtomicUsize::new(0)),
            max_connections: settings.max_connections,
            address,
        })
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) fn registry(&self) -> Arc<Registry> {
        Arc::clone(&self.context.registry)
    }

    /// Accepts connections until the process ends. Runs on its own thread;
    /// a session's lifetime, not this loop, is what ends the program.
    pub(crate) fn serve(self) {
        thread::Builder::new()
            .name("alc-remote-accept".to_owned())
            .spawn(move || {
                for stream in self.listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    if self.connections.load(Ordering::Relaxed) >= self.max_connections {
                        // Refusing is better than queueing: a client that
                        // waits behind a full server looks like a hang.
                        let _ = respond(
                            &stream,
                            503,
                            "text/plain; charset=utf-8",
                            None,
                            b"too many connections",
                        );
                        continue;
                    }
                    self.connections.fetch_add(1, Ordering::Relaxed);

                    let context = Arc::clone(&self.context);
                    let connections = Arc::clone(&self.connections);
                    thread::Builder::new()
                        .name("alc-remote-conn".to_owned())
                        .spawn(move || {
                            handle(stream, &context);
                            connections.fetch_sub(1, Ordering::Relaxed);
                        })
                        .ok();
                }
            })
            .ok();
    }
}

/// The `Host` values this server answers to. Exact matches including the
/// port: that is what makes the check a DNS-rebinding defence rather than a
/// formality.
fn allowed_hosts(address: SocketAddr, settings: &RemoteSettings, lan: bool) -> Vec<String> {
    let port = address.port();
    let mut hosts = vec![
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ];
    if lan && let Some(ip) = local_lan_address() {
        hosts.push(format!("{ip}:{port}"));
    }
    hosts.extend(settings.extra_hosts.iter().cloned());
    hosts
}

fn allowed_origins(address: SocketAddr, settings: &RemoteSettings, lan: bool) -> Vec<String> {
    let mut origins: Vec<String> = allowed_hosts(address, settings, lan)
        .into_iter()
        .map(|host| format!("http://{host}"))
        .collect();
    origins.extend(settings.allowed_origins.iter().cloned());
    origins
}

/// The link a viewer opens.
///
/// The token rides in the URL fragment, which no browser sends to a server
/// and which therefore lands in no proxy and no access log.
pub(crate) fn page_url(port: u16, token: &str, lan: bool) -> String {
    let host = if lan {
        local_lan_address().unwrap_or_else(|| "127.0.0.1".to_owned())
    } else {
        "127.0.0.1".to_owned()
    };
    format!("http://{host}:{port}/#k={token}")
}

/// This machine's first non-loopback IPv4 address, for printing a link a
/// phone on the same network can open. Discovered by asking the routing
/// table where a packet to a public address would leave from - a UDP
/// "connect" sends nothing, it only resolves the source address.
fn local_lan_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then(|| address.to_string())
}

fn handle(stream: TcpStream, context: &Serving) {
    let _ = stream.set_read_timeout(Some(HEAD_TIMEOUT));
    let _ = stream.set_nodelay(true);

    // The head is PEEKED rather than read, so a WebSocket upgrade can be
    // handed to tungstenite with the handshake still sitting in the socket
    // where it expects to find it.
    let Some(head) = peek_head(&stream) else {
        let _ = respond(
            &stream,
            400,
            "text/plain; charset=utf-8",
            None,
            b"bad request",
        );
        return;
    };
    // A cheap look, only to choose a branch. Both branches then parse the
    // request properly and run the same guard, so this decides nothing on
    // its own - which is why it is allowed to be approximate.
    if looks_like_upgrade(&head) {
        // An upgrade carries no body, so the peeked head IS the whole
        // request and can be parsed and guarded as it stands.
        let Ok(peeked) = read_request(&mut BufReader::new(Cursor::new(&head))) else {
            let _ = respond(
                &stream,
                400,
                "text/plain; charset=utf-8",
                None,
                b"bad request",
            );
            return;
        };
        if let Err(denied) = context.guard.check(&peeked, true) {
            refuse(&stream, denied);
            return;
        }
        serve_websocket(stream, &peeked, context);
        return;
    }

    // Everything else is read properly off the socket, body included. The
    // peek above only decided which of these two paths to take: a POST's
    // body is not in the head, and parsing the head alone would answer a
    // perfectly good request with "bad request".
    let mut reader = BufReader::new(&stream);
    let request = match read_request(&mut reader) {
        Ok(request) => request,
        Err(_) => {
            let _ = respond(
                &stream,
                400,
                "text/plain; charset=utf-8",
                None,
                b"bad request",
            );
            return;
        }
    };

    let grade = match context.guard.check(&request, is_public(&request.path)) {
        Ok(grade) => grade,
        Err(denied) => {
            refuse(&stream, denied);
            return;
        }
    };

    let _ = serve_http(&stream, &request, grade, context);
}

/// Whether the peeked head is a WebSocket handshake.
///
/// Deliberately not a parse: a request with a body would fail the real
/// parser here, because the body is not in the peeked head - which is how
/// every POST used to be answered with "bad request".
fn looks_like_upgrade(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    text.contains("upgrade: websocket")
        || text
            .lines()
            .any(|line| line.starts_with("upgrade:") && line.contains("websocket"))
}

fn refuse(stream: &TcpStream, denied: Denied) {
    let (status, message) = match denied {
        Denied::Host => (403, "host not allowed"),
        Denied::Origin => (403, "origin not allowed"),
        Denied::Token => (401, "unauthorized"),
    };
    let _ = respond(
        stream,
        status,
        "text/plain; charset=utf-8",
        None,
        message.as_bytes(),
    );
}

fn is_public(path: &str) -> bool {
    path == "/" || path == "/healthz" || path.starts_with("/assets/")
}

/// Reads the request head without consuming it, so an upgrade can be handed
/// to tungstenite with the handshake still in the socket where it expects
/// it - while the same parser and the same guard decide every route.
///
/// Bounded twice over: by `MAX_HEAD`, matching what `read_request` itself
/// enforces, and by a deadline, so a peer that sends one byte at a time
/// cannot hold the thread open. The buffer grows only when a peek filled
/// it, because growing on a short read would exhaust the bound in a few
/// milliseconds and cut off a merely slow client.
fn peek_head(stream: &TcpStream) -> Option<Vec<u8>> {
    let deadline = Instant::now() + HEAD_TIMEOUT;
    let mut buffer = vec![0_u8; 2048];
    loop {
        let read = stream.peek(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        if let Some(end) = find_head_end(&buffer[..read]) {
            return Some(buffer[..end].to_vec());
        }
        if read == buffer.len() {
            if buffer.len() >= MAX_HEAD {
                return None;
            }
            buffer.resize((buffer.len() * 2).min(MAX_HEAD), 0);
            continue;
        }
        if Instant::now() >= deadline {
            return None;
        }
        // The peer has sent everything it has so far and it is not yet a
        // complete head; give it a moment rather than spinning on peek.
        thread::sleep(Duration::from_millis(5));
    }
}

fn find_head_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|start| start + 4)
}

fn serve_http(
    stream: &TcpStream,
    request: &Request,
    grade: Option<Grade>,
    context: &Serving,
) -> Result<()> {
    let registry = &*context.registry;
    // Stopping a session is the one destructive thing the page can do, so
    // it is a method of its own rather than a POST that could be reached by
    // a form submission, and it needs operator rights.
    if request.method == Method::Delete {
        let Some(id) = request.path.strip_prefix("/api/sessions/") else {
            return respond(stream, 404, "text/plain; charset=utf-8", None, b"not found");
        };
        if grade < Some(Grade::Operator) {
            return respond(
                stream,
                403,
                "text/plain; charset=utf-8",
                None,
                b"this link can watch but not stop a session",
            );
        }
        return match registry.get(id) {
            Some(session) => {
                session.notify(NoticeLevel::Warn, "a viewer stopped this session");
                let _ = session.kill();
                respond(
                    stream,
                    200,
                    "application/json; charset=utf-8",
                    None,
                    br#"{"stopped":true}"#,
                )
            }
            None => respond(
                stream,
                404,
                "text/plain; charset=utf-8",
                None,
                b"no session",
            ),
        };
    }

    // Changing a session's permission mode. A POST rather than a GET
    // because it acts, and operator-only because tightening is the only
    // half a viewer could be trusted with and splitting the route by
    // direction would be a second place to get the check wrong.
    if request.method == Method::Post
        && let Some(id) = request
            .path
            .strip_prefix("/api/sessions/")
            .and_then(|rest| rest.strip_suffix("/permission"))
    {
        if grade < Some(Grade::Operator) {
            return respond(
                stream,
                403,
                "text/plain; charset=utf-8",
                None,
                b"this link can watch but not change permissions",
            );
        }
        let Some(session) = registry.get(id) else {
            return respond(
                stream,
                404,
                "text/plain; charset=utf-8",
                None,
                b"no session",
            );
        };
        let Ok(body) = serde_json::from_slice::<PermissionRequest>(&request.body) else {
            return respond(
                stream,
                400,
                "text/plain; charset=utf-8",
                None,
                b"expected {\"rung\":\"plan|ask|auto-edit|auto|full\"}",
            );
        };
        let Ok(rung) = body.rung.parse::<SafetyRung>() else {
            return respond(
                stream,
                400,
                "text/plain; charset=utf-8",
                None,
                b"unknown permission rung",
            );
        };

        let applied = match permission::apply(
            &session,
            rung,
            context.ceiling,
            &context.gate,
            body.ticket.as_deref(),
        ) {
            Ok(applied) => applied,
            Err(error) => {
                let body = serde_json::json!({ "error": error.to_string() });
                return respond(
                    stream,
                    500,
                    "application/json; charset=utf-8",
                    None,
                    &serde_json::to_vec(&body)?,
                );
            }
        };
        // Everyone watching is told, including the local terminal's own
        // viewer: a mode change is exactly the kind of thing another person
        // holding the link should not be able to do invisibly.
        session.notify(
            NoticeLevel::Warn,
            format!("a viewer asked for permission mode: {rung}"),
        );
        let status = if matches!(
            applied,
            crate::remote::caps::Applied::NeedsConfirmation { .. }
        ) {
            409
        } else {
            200
        };
        return respond(
            stream,
            status,
            "application/json; charset=utf-8",
            None,
            &serde_json::to_vec(&applied)?,
        );
    }

    if request.method != Method::Get {
        return respond(
            stream,
            405,
            "text/plain; charset=utf-8",
            None,
            b"method not allowed",
        );
    }

    if let Some(asset) = assets::find(&request.path) {
        return respond(stream, 200, asset.content_type, Some("gzip"), asset.gzipped);
    }

    match request.path.as_str() {
        "/healthz" => respond(
            stream,
            200,
            "application/json; charset=utf-8",
            None,
            br#"{"ok":true}"#,
        ),
        // The page renders its permission control as a pure function of
        // this, so an agent alc cannot drive shows a disabled control with
        // the reason on it rather than a button that does nothing.
        "/api/caps" => {
            let body = serde_json::to_vec(&serde_json::json!({
                "agents": CAPS,
                "ceiling": context.ceiling,
            }))?;
            respond(stream, 200, "application/json; charset=utf-8", None, &body)
        }
        "/api/sessions" => {
            // Reached only with a token: `is_public` does not list it.
            debug_assert!(grade.is_some());
            let body = serde_json::to_vec(&registry.cards())?;
            respond(stream, 200, "application/json; charset=utf-8", None, &body)
        }
        path if path.starts_with("/api/sessions/") => {
            let id = path.trim_start_matches("/api/sessions/");
            match registry.get(id) {
                Some(session) => {
                    let body = serde_json::to_vec(&session.card())?;
                    respond(stream, 200, "application/json; charset=utf-8", None, &body)
                }
                None => respond(
                    stream,
                    404,
                    "text/plain; charset=utf-8",
                    None,
                    b"no session",
                ),
            }
        }
        _ => respond(stream, 404, "text/plain; charset=utf-8", None, b"not found"),
    }
}

fn respond(
    stream: &TcpStream,
    status: u16,
    content_type: &str,
    encoding: Option<&str>,
    body: &[u8],
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         content-type: {content_type}\r\n\
         content-length: {}\r\n\
         cache-control: no-store\r\n\
         referrer-policy: no-referrer\r\n\
         x-content-type-options: nosniff\r\n\
         x-frame-options: DENY\r\n\
         content-security-policy: default-src 'self'; script-src 'self'; \
         style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; \
         frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n\
         connection: close\r\n",
        body.len()
    );
    if let Some(encoding) = encoding {
        head.push_str(&format!("content-encoding: {encoding}\r\n"));
    }
    head.push_str("\r\n");

    let mut stream = stream;
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

/// Runs one viewer's WebSocket for the life of the connection.
///
/// A single thread owns the socket in both directions. Splitting it would be
/// natural and is wrong here: tungstenite answers pings and close frames
/// from inside `read`, so a second writer on the same descriptor interleaves
/// with those replies and corrupts the stream at teardown.
fn serve_websocket(stream: TcpStream, request: &Request, context: &Serving) {
    let registry = &*context.registry;
    let guard = &context.guard;
    let Some(id) = request.query_param("session") else {
        return;
    };
    // Deliberately NOT looked up yet. Refusing a bad id before the handshake
    // would answer "does this session exist" to an unauthenticated peer,
    // because a real id completes the upgrade and a made-up one drops the
    // connection outright.
    let Ok(mut socket) = tungstenite::accept(stream) else {
        return;
    };
    let _ = socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(20)));

    // The token is the first frame or the connection ends. Nothing about
    // the session is revealed before it arrives - not even that the id
    // names a real session.
    let authenticated = match socket.read() {
        Ok(Message::Text(text)) => match serde_json::from_str::<ClientFrame>(&text) {
            Ok(ClientFrame::Auth {
                token,
                cols,
                rows,
                since,
            }) => guard
                .grade_token(&token)
                .map(|grade| (grade, since, cols, rows)),
            _ => None,
        },
        _ => None,
    };
    let Some((grade, since, cols, rows)) = authenticated else {
        let _ = socket.close(None);
        let _ = socket.flush();
        return;
    };

    // Only now, once the peer has proved it holds a token, does an unknown
    // id look any different from a known one.
    let Some(session) = registry.get(&id) else {
        let _ = socket.close(None);
        let _ = socket.flush();
        return;
    };

    if grade >= Grade::Operator && cols > 0 && rows > 0 {
        let _ = session.resize(cols, rows);
    }

    let subscription = session.subscribe();
    let viewer = subscription.handle.id();

    let hello = ServerFrame::Hello(Box::new(Hello {
        session: session.card(),
        grade: if grade >= Grade::Operator {
            "operator"
        } else {
            "viewer"
        },
        seq: session.seq(),
    }));
    if let Ok(rendered) = serde_json::to_string(&hello) {
        let _ = socket.send(Message::Text(rendered.into()));
    }
    let _ = socket.send(Message::Binary(session.catch_up(since).into()));
    if session.has_exited() {
        let card = session.card();
        if let Some(exit) = card.exit
            && let Ok(rendered) = serde_json::to_string(&ServerFrame::Exit {
                code: exit.code,
                signal: exit.signal,
            })
        {
            let _ = socket.send(Message::Text(rendered.into()));
        }
    }
    let _ = socket.flush();

    let _ = socket.get_ref().set_read_timeout(Some(WS_TICK));

    let mut last_ping = Instant::now();
    let mut last_seen = Instant::now();

    loop {
        if last_seen.elapsed() > PONG_DEADLINE {
            break;
        }
        if last_ping.elapsed() >= PING_EVERY {
            last_ping = Instant::now();
            if let Ok(rendered) = serde_json::to_string(&ServerFrame::Ping)
                && socket.send(Message::Text(rendered.into())).is_err()
            {
                break;
            }
        }

        // Drain everything queued for this viewer first, so output is never
        // held behind a read that is only waiting for a keystroke.
        let mut closed = false;
        while let Ok(frame) = subscription.rx.try_recv() {
            let sent = match &*frame {
                Frame::Binary(bytes) => socket.send(Message::Binary(bytes.clone().into())),
                Frame::Text(text) => socket.send(Message::Text(text.clone().into())),
                Frame::Close => {
                    closed = true;
                    break;
                }
            };
            if sent.is_err() {
                closed = true;
                break;
            }
        }
        if socket.flush().is_err() {
            break;
        }
        if closed {
            let _ = socket.close(None);
            let _ = socket.flush();
            break;
        }

        // Dropped output is repaired with the current screen rather than a
        // replay: the screen is both smaller and more use than a backlog.
        if subscription.handle.take_lagging() {
            let frame = session.catch_up(None);
            if socket.send(Message::Binary(frame.into())).is_err() {
                break;
            }
            let notice = ServerFrame::Notice {
                level: NoticeLevel::Info,
                message: "this view fell behind and was refreshed".to_owned(),
            };
            if let Ok(rendered) = serde_json::to_string(&notice) {
                let _ = socket.send(Message::Text(rendered.into()));
            }
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                last_seen = Instant::now();
                if let Ok(frame) = serde_json::from_str::<ClientFrame>(&text)
                    && !apply(&session, frame, grade)
                {
                    let notice = ServerFrame::Notice {
                        level: NoticeLevel::Warn,
                        message: "this link can watch but not type".to_owned(),
                    };
                    if let Ok(rendered) = serde_json::to_string(&notice) {
                        let _ = socket.send(Message::Text(rendered.into()));
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => last_seen = Instant::now(),
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }

    session.unsubscribe(viewer);
}

/// The body of a permission change.
#[derive(serde::Deserialize)]
struct PermissionRequest {
    rung: String,
    /// A ticket from `alc confirm`, when the change needs one.
    #[serde(default)]
    ticket: Option<String>,
}

/// Applies one client frame. Returns false when the frame needed operator
/// rights the connection does not have, so the caller can say so once
/// rather than dropping input silently.
fn apply(session: &Session, frame: ClientFrame, grade: Grade) -> bool {
    let operator = grade >= Grade::Operator;
    match frame {
        ClientFrame::Input { data } => {
            if !operator {
                return false;
            }
            if session.input(data.as_bytes()).is_err() {
                session.notify(NoticeLevel::Error, "the agent is no longer accepting input");
            }
        }
        ClientFrame::Paste { data } => {
            if !operator {
                return false;
            }
            if session.paste(&data).is_err() {
                session.notify(NoticeLevel::Error, "the agent is no longer accepting input");
            }
        }
        ClientFrame::Resize { cols, rows } => {
            if !operator {
                return false;
            }
            let _ = session.resize(cols, rows);
        }
        ClientFrame::Auth { .. } | ClientFrame::Pong => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::io::{BufRead, BufReader};
    use std::path::Path;

    use crate::launch::{LaunchSpec, SessionGuards};
    use crate::remote::session::SessionSpec;

    struct Harness {
        /// Held so the temp config directory outlives the server.
        _config: tempfile::TempDir,
        address: SocketAddr,
        operator: String,
        viewer: String,
        session: Arc<Session>,
    }

    /// A server in front of a real pty running `sh`, so every test exercises
    /// the same path a session does rather than a stand-in.
    fn harness() -> Harness {
        let secrets = Secrets {
            ctl: "ctl-token-for-tests-only-0000".to_owned(),
            operator: "operator-token-for-tests-0000".to_owned(),
            viewer: "viewer-token-for-tests-000000".to_owned(),
        };
        let settings = RemoteSettings {
            port: 0,
            ..RemoteSettings::default()
        };
        let config = tempfile::tempdir().unwrap();
        let server = Server::bind(config.path(), &settings, &secrets, false).unwrap();
        let address = server.address();

        let mut spec = LaunchSpec::for_test();
        spec.args = vec![OsString::from("-c"), OsString::from("sleep 30")];
        let session = Session::start(
            SessionSpec {
                id: "codex-TESTTESTTE".to_owned(),
                name: "codex@test".to_owned(),
                cols: 80,
                rows: 24,
                scrollback_bytes: 64 * 1024,
                permission: crate::remote::permission::PermState::unknown(),
            },
            Path::new("/bin/sh"),
            spec,
            Path::new("/"),
            SessionGuards::none(),
        )
        .unwrap();

        server.registry().insert(Arc::clone(&session));
        server.serve();
        Harness {
            _config: config,
            address,
            operator: secrets.operator,
            viewer: secrets.viewer,
            session,
        }
    }

    /// Sends a raw request and returns the whole response.
    fn request(address: SocketAddr, head: &str) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(head.as_bytes()).unwrap();
        let mut response = String::new();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            response.push_str(&line);
            line.clear();
        }
        response
    }

    fn status(response: &str) -> &str {
        response.lines().next().unwrap_or_default()
    }

    #[test]
    fn a_json_route_without_a_token_is_refused() {
        let harness = harness();
        let response = request(
            harness.address,
            &format!(
                "GET /api/sessions HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                harness.address.port()
            ),
        );
        assert!(status(&response).contains("401"), "{response}");
        let _ = harness.session.kill();
    }

    #[test]
    fn a_foreign_host_header_is_refused_even_with_a_valid_token() {
        // The DNS-rebinding case: evil.com can be made to resolve to
        // 127.0.0.1, so the address the request arrived on proves nothing.
        let harness = harness();
        let response = request(
            harness.address,
            &format!(
                "GET /api/sessions HTTP/1.1\r\nHost: evil.com:{}\r\n\
                 Authorization: Bearer {}\r\nConnection: close\r\n\r\n",
                harness.address.port(),
                harness.operator
            ),
        );
        assert!(status(&response).contains("403"), "{response}");
        let _ = harness.session.kill();
    }

    #[test]
    fn a_host_header_without_the_port_is_refused() {
        let harness = harness();
        let response = request(
            harness.address,
            &format!(
                "GET /api/sessions HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                 Authorization: Bearer {}\r\nConnection: close\r\n\r\n",
                harness.operator
            ),
        );
        assert!(status(&response).contains("403"), "{response}");
        let _ = harness.session.kill();
    }

    #[test]
    fn the_page_and_its_assets_load_without_a_token() {
        // The page has to run before it can read the token out of the URL
        // fragment, so the static files cannot require one.
        let harness = harness();
        for path in ["/", "/assets/app.js", "/assets/xterm.js"] {
            let response = request(
                harness.address,
                &format!(
                    "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                    harness.address.port()
                ),
            );
            assert!(status(&response).contains("200"), "{path}: {response}");
            assert!(
                response.contains("content-encoding: gzip"),
                "{path} was not compressed"
            );
        }
        let _ = harness.session.kill();
    }

    #[test]
    fn a_session_listing_names_the_session_and_never_the_token() {
        let harness = harness();
        let response = request(
            harness.address,
            &format!(
                "GET /api/sessions HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
                 Authorization: Bearer {}\r\nConnection: close\r\n\r\n",
                harness.address.port(),
                harness.operator
            ),
        );
        assert!(status(&response).contains("200"), "{response}");
        assert!(response.contains("codex-TESTTESTTE"), "{response}");
        assert!(
            !response.contains(&harness.operator) && !response.contains(&harness.viewer),
            "a response echoed a token"
        );
        let _ = harness.session.kill();
    }

    #[test]
    fn a_viewer_token_cannot_stop_a_session() {
        let harness = harness();
        let response = request(
            harness.address,
            &format!(
                "DELETE /api/sessions/codex-TESTTESTTE HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
                 Authorization: Bearer {}\r\nConnection: close\r\n\r\n",
                harness.address.port(),
                harness.viewer
            ),
        );
        assert!(status(&response).contains("403"), "{response}");
        assert!(!harness.session.has_exited());
        let _ = harness.session.kill();
    }

    #[test]
    fn a_websocket_upgrade_without_an_origin_is_refused() {
        // Every browser sends an Origin on a handshake, so one without it is
        // not a page - and a page is the only client whose origin the
        // browser would have policed on our behalf.
        let harness = harness();
        let response = request(
            harness.address,
            &format!(
                "GET /ws?session=codex-TESTTESTTE HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
                 Upgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
                harness.address.port()
            ),
        );
        assert!(status(&response).contains("403"), "{response}");
        let _ = harness.session.kill();
    }

    #[test]
    fn a_websocket_upgrade_from_a_foreign_origin_is_refused() {
        let harness = harness();
        let response = request(
            harness.address,
            &format!(
                "GET /ws?session=codex-TESTTESTTE HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
                 Origin: https://evil.example\r\n\
                 Upgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
                harness.address.port()
            ),
        );
        assert!(status(&response).contains("403"), "{response}");
        let _ = harness.session.kill();
    }

    #[test]
    fn an_unknown_path_is_not_reachable_through_traversal() {
        let harness = harness();
        for path in [
            "/assets/../../../etc/passwd",
            "/etc/passwd",
            "/assets/%2e%2e/app.js",
        ] {
            let response = request(
                harness.address,
                &format!(
                    "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                    harness.address.port()
                ),
            );
            assert!(
                !status(&response).contains("200"),
                "{path} was served: {response}"
            );
        }
        let _ = harness.session.kill();
    }

    #[test]
    fn a_loopback_bind_does_not_answer_on_a_lan_address() {
        // Asking for a LAN bind without turning it on in the settings has to
        // stay on loopback: one switch is too easy to leave on by accident.
        let secrets = Secrets {
            ctl: "ctl-token-for-tests-only-0000".to_owned(),
            operator: "operator-token-for-tests-0000".to_owned(),
            viewer: "viewer-token-for-tests-000000".to_owned(),
        };
        let settings = RemoteSettings {
            port: 0,
            allow_lan: false,
            ..RemoteSettings::default()
        };
        let config = tempfile::tempdir().unwrap();
        let server = Server::bind(config.path(), &settings, &secrets, true).unwrap();
        assert!(
            server.address().ip().is_loopback(),
            "bound to {} despite allow_lan being false",
            server.address()
        );
    }

    #[test]
    fn the_url_carries_the_token_in_the_fragment() {
        // A fragment is never sent to a server and never lands in a proxy or
        // an access log.
        let secrets = Secrets {
            ctl: "ctl-token-for-tests-only-0000".to_owned(),
            operator: "operator-token-for-tests-0000".to_owned(),
            viewer: "viewer-token-for-tests-000000".to_owned(),
        };
        let settings = RemoteSettings {
            port: 0,
            ..RemoteSettings::default()
        };
        let config = tempfile::tempdir().unwrap();
        let server = Server::bind(config.path(), &settings, &secrets, false).unwrap();
        let url = page_url(server.address().port(), &secrets.operator, false);
        assert!(url.contains("/#k="), "{url}");
        let (before, _) = url.split_once('#').unwrap();
        assert!(!before.contains(&secrets.operator), "{url}");
    }
}
