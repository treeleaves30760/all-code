//! The background bridge process. See the module above for why it exists.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt;
use serde_json::json;

use crate::bridge::auth::AuthManager;
use crate::bridge::{BridgeConfig, BridgeError, BridgeState};
use crate::config::Agent;

use super::files;

/// How long a bridge with nothing to do stays up. The helper Claude Code runs
/// before a request starts it again, so this only decides how long an idle
/// process lingers.
const IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);
const IDLE_CHECK: Duration = Duration::from_secs(60);
/// How long an explicit stop waits for streaming turns before it ends them.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How many times a start asks a port it could not bind whether an alc bridge
/// is there, and how long it waits between asking. Four pauses, a little over
/// a second, is all the delay this adds by itself; each ask is separately
/// bounded by the hello timeout.
const PROBE_TRIES: u32 = 5;
const PROBE_PAUSE: Duration = Duration::from_millis(300);

pub(crate) struct Host {
    config_dir: PathBuf,
    token: String,
    port: u16,
    instance: String,
    routes: Mutex<HashMap<String, Arc<BridgeState>>>,
    logins: Mutex<HashMap<PathBuf, Arc<AuthManager>>>,
    activity: Activity,
    stop: Arc<tokio::sync::Notify>,
}

/// Whether anything is using the bridge: requests still in flight, and when
/// the last one started or finished, in Unix seconds.
struct Activity {
    in_flight: AtomicUsize,
    last: AtomicU64,
}

impl Activity {
    fn new(now: u64) -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            last: AtomicU64::new(now),
        }
    }

    /// How long the bridge has had nothing to do at `now`. Zero while any
    /// request - a turn streaming for ten minutes included - is in flight.
    fn idle_for(&self, now: u64) -> Duration {
        if self.in_flight.load(Ordering::SeqCst) > 0 {
            return Duration::ZERO;
        }
        Duration::from_secs(now.saturating_sub(self.last.load(Ordering::SeqCst)))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl Host {
    fn new(config_dir: &Path, token: String, port: u16) -> Result<Self> {
        Ok(Self {
            config_dir: config_dir.to_owned(),
            token,
            port,
            instance: instance_id()?,
            routes: Mutex::default(),
            logins: Mutex::default(),
            activity: Activity::new(now_secs()),
            stop: Arc::new(tokio::sync::Notify::new()),
        })
    }

    /// The bridge state for route `id`, built from its record the first time.
    fn route(&self, id: &str) -> Result<Arc<BridgeState>, BridgeError> {
        if let Some(state) = self
            .routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            return Ok(Arc::clone(state));
        }
        let record = files::read_route(&self.config_dir, id)
            .ok()
            .flatten()
            .ok_or_else(|| {
                BridgeError::new(
                    StatusCode::NOT_FOUND,
                    "not_found_error",
                    format!(
                        "alc's background bridge has no route {id}; start `alc claude` once with \
                         that Codex profile to recreate it"
                    ),
                )
            })?;
        let login = Arc::clone(
            self.logins
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(record.auth_file.clone())
                .or_insert_with(|| Arc::new(AuthManager::new(record.auth_file.clone()))),
        );
        let config = BridgeConfig {
            auth_file: record.auth_file.clone(),
            effort: None,
            responses_api: false,
            agent: Agent::Claude,
            provider: record.profile.clone(),
            ledger: Some(self.config_dir.join(crate::usage::ledger::LEDGER_FILE)),
            claude_tiers: Some(record.tiers),
        };
        let state = Arc::new(BridgeState::with_auth(config, login).map_err(|error| {
            BridgeError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                format!("{error:#}"),
            )
        })?);
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), Arc::clone(&state));
        Ok(state)
    }
}

fn instance_id() -> Result<String> {
    let mut bytes = [0_u8; 6];
    getrandom::fill(&mut bytes).context("failed to read operating-system randomness")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn router(host: Arc<Host>) -> Router {
    let guarded = Router::new()
        .route("/alc/hello", get(hello))
        .route("/alc/stop", post(stop))
        .route("/r/{route}/v1/messages", post(messages))
        .route("/r/{route}/v1/messages/count_tokens", post(count_tokens))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&host),
            require_token,
        ));
    Router::new()
        .route("/healthz", get(|| async { (StatusCode::OK, "ok") }))
        .merge(guarded)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&host),
            track_activity,
        ))
        // A long session's turn carries megabytes; the upstream decides what
        // is too large, as it does for the in-process adapter.
        .layer(DefaultBodyLimit::disable())
        .with_state(host)
}

async fn require_token(State(host): State<Arc<Host>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let presented = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
        });
    if presented.is_some_and(|token| same(token.as_bytes(), host.token.as_bytes())) {
        return next.run(request).await;
    }
    BridgeError::auth(
        "this request did not carry the token of alc's background bridge; Claude Code gets it \
         from `alc claude-credential` through apiKeyHelper - run `alc bridge status`",
    )
    .anthropic()
}

/// Compares without an early exit, so the time taken says nothing about how
/// much of a guess was right.
fn same(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Counts a request as activity until the last byte of its response has gone,
/// so a turn that streams for ten minutes keeps the bridge up.
async fn track_activity(State(host): State<Arc<Host>>, request: Request, next: Next) -> Response {
    let guard = InFlight::begin(Arc::clone(&host));
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    // The guard rides the body's stream: it is dropped when the stream is,
    // which is the last byte of the response or the client walking away -
    // never when this function returns. Deleting this capture is deleting the
    // guarantee, which is why two tests drive a real streaming response
    // through here rather than moving the counter by hand.
    let body = Body::from_stream(body.into_data_stream().map(move |chunk| {
        let _held = &guard;
        chunk
    }));
    Response::from_parts(parts, body)
}

struct InFlight(Arc<Host>);

impl InFlight {
    fn begin(host: Arc<Host>) -> Self {
        host.activity.in_flight.fetch_add(1, Ordering::SeqCst);
        host.activity.last.store(now_secs(), Ordering::SeqCst);
        Self(host)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.activity.last.store(now_secs(), Ordering::SeqCst);
        self.0.activity.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn hello(State(host): State<Arc<Host>>) -> Response {
    axum::Json(json!({
        "instance": host.instance,
        "alc": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "port": host.port,
    }))
    .into_response()
}

async fn stop(State(host): State<Arc<Host>>) -> StatusCode {
    host.stop.notify_one();
    StatusCode::ACCEPTED
}

async fn messages(
    State(host): State<Arc<Host>>,
    UrlPath(route): UrlPath<String>,
    body: Bytes,
) -> Response {
    match host.route(&route) {
        Ok(state) => crate::bridge::messages::handle_messages(State(state), body).await,
        Err(error) => error.anthropic(),
    }
}

async fn count_tokens(
    State(host): State<Arc<Host>>,
    UrlPath(route): UrlPath<String>,
    body: Bytes,
) -> Response {
    match host.route(&route) {
        Ok(state) => crate::bridge::messages::handle_count_tokens(State(state), body).await,
        Err(error) => error.anthropic(),
    }
}

async fn watch_idle(host: Arc<Host>) {
    loop {
        tokio::time::sleep(IDLE_CHECK).await;
        if host.activity.idle_for(now_secs()) >= IDLE_LIMIT {
            host.stop.notify_one();
            return;
        }
    }
}

/// Runs the bridge until it is stopped or has been idle for [`IDLE_LIMIT`].
pub(crate) fn run(config_dir: &Path) -> Result<u8> {
    let token = files::load_or_create_token(config_dir)?;
    let (listener, port, token) = bind(config_dir, token)?;
    listener
        .set_nonblocking(true)
        .context("failed to prepare the bridge's listener")?;
    // Seen only when run in a terminal; a detached bridge has no stderr.
    eprintln!("alc bridge listening on 127.0.0.1:{port}");
    let host = Arc::new(Host::new(config_dir, token, port)?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("failed to start the bridge's runtime")?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)
            .context("failed to hand the bridge's listener to its runtime")?;
        tokio::spawn(watch_idle(Arc::clone(&host)));
        let stop = Arc::clone(&host.stop);
        axum::serve(listener, router(host))
            .with_graceful_shutdown(async move {
                stop.notified().await;
                // A turn still streaming gets a few seconds, then the process
                // ends it: `alc bridge stop` is a request, not a suggestion.
                tokio::spawn(async {
                    tokio::time::sleep(STOP_GRACE).await;
                    std::process::exit(0);
                });
            })
            .await
            .context("the bridge stopped serving")
    })?;
    Ok(0)
}

/// Binds the remembered port, or moves: a new port, a new token, and alc's
/// settings files rewritten to follow. A port held by this bridge already is
/// not a reason to move - that is a second start losing a race.
fn bind(config_dir: &Path, token: String) -> Result<(TcpListener, u16, String)> {
    let previous = files::remembered_port(config_dir);
    if let Some(port) = previous {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => return Ok((listener, port, token)),
            // Something already holds the port, which is the one case where
            // asking who is there is worth the wait. See `serving_there`.
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                if serving_there(port, &token) {
                    bail!("an alc bridge is already serving on 127.0.0.1:{port}")
                }
            }
            // `PermissionDenied` is the answer a range Windows reserves for
            // Hyper-V or WinNAT gives, and nothing is listening on one of
            // those; no other failure means a bridge is there either. Move.
            Err(_) => {}
        }
    }
    for _ in 0..50 {
        let port = files::choose_port()?;
        let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else {
            continue;
        };
        files::remember_port(config_dir, port)?;
        let token = match previous {
            Some(old) => {
                let rotated = files::rotate_token(config_dir)?;
                files::move_settings_origin(config_dir, old, port)?;
                rotated
            }
            None => token,
        };
        return Ok((listener, port, token));
    }
    bail!(
        "could not find a free loopback port for the bridge between {} and {}",
        files::PORT_RANGE.start,
        files::PORT_RANGE.end - 1
    )
}

/// Whether an alc bridge for this configuration is serving on `port`, asked
/// more than once.
///
/// A listening socket answers TCP before its server answers HTTP. From the
/// moment a sibling start binds the remembered port, the operating system
/// completes handshakes into its backlog while that sibling is still setting
/// the socket non-blocking, building its runtime and handing the listener
/// over - so a hello that connects and then times out means "not answering
/// yet" at least as often as it means "not an alc bridge". Believing one
/// timeout would move this start to a port of its own, rotating the token out
/// from under the sibling about to serve on the remembered one: every request
/// to it then fails the token check, the helper hands back the rotated token
/// and that fails too, and the port stays held by a bridge nothing can use
/// until it idles out an hour later.
fn serving_there(port: u16, token: &str) -> bool {
    says_yes(PROBE_TRIES, PROBE_PAUSE, || {
        super::hello(port, token).is_ok()
    })
}

/// Whether `ask` says yes within `tries`, waiting `pause` between attempts.
fn says_yes(tries: u32, pause: Duration, mut ask: impl FnMut() -> bool) -> bool {
    for attempt in 0..tries {
        if attempt > 0 {
            std::thread::sleep(pause);
        }
        if ask() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn host(config_dir: &Path) -> Arc<Host> {
        Arc::new(
            Host::new(
                config_dir,
                "t0ken-for-tests-only-xxxxxxxxxxxxxxxxxxxx".to_owned(),
                24_817,
            )
            .unwrap(),
        )
    }

    async fn status(
        host: Arc<Host>,
        method: &str,
        path: &str,
        auth: Option<(&str, &str)>,
    ) -> StatusCode {
        let mut request = axum::http::Request::builder().method(method).uri(path);
        if let Some((name, value)) = auth {
            request = request.header(name, value);
        }
        router(host)
            .oneshot(request.body(Body::from("{}")).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// One route answering a body in two chunks, behind the real middleware,
    /// so what the tests below assert is the middleware's doing and not the
    /// test's: the in-flight count has no other writer.
    fn streaming_router(host: &Arc<Host>) -> Router {
        Router::new()
            .route(
                "/stream",
                get(|| async {
                    Body::from_stream(futures_util::stream::iter([
                        Ok::<_, std::io::Error>(Bytes::from_static(b"one")),
                        Ok(Bytes::from_static(b"two")),
                    ]))
                }),
            )
            .layer(middleware::from_fn_with_state(
                Arc::clone(host),
                track_activity,
            ))
    }

    async fn streamed(host: &Arc<Host>) -> Response {
        streaming_router(host)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn only_the_health_check_answers_without_the_token() {
        let temp = tempfile::tempdir().unwrap();
        let token = "t0ken-for-tests-only-xxxxxxxxxxxxxxxxxxxx";
        assert_eq!(
            status(host(temp.path()), "GET", "/healthz", None).await,
            StatusCode::OK
        );
        assert_eq!(
            status(host(temp.path()), "GET", "/alc/hello", None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(
                host(temp.path()),
                "GET",
                "/alc/hello",
                Some(("x-api-key", "wrong"))
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(
                host(temp.path()),
                "GET",
                "/alc/hello",
                Some(("x-api-key", token))
            )
            .await,
            StatusCode::OK
        );
        let bearer = format!("Bearer {token}");
        assert_eq!(
            status(
                host(temp.path()),
                "GET",
                "/alc/hello",
                Some(("authorization", &bearer))
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            status(
                host(temp.path()),
                "POST",
                "/r/codex-0123456789ab/v1/messages",
                None
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn an_unknown_route_is_named_not_served() {
        let temp = tempfile::tempdir().unwrap();
        let auth = Some(("x-api-key", "t0ken-for-tests-only-xxxxxxxxxxxxxxxxxxxx"));
        assert_eq!(
            status(
                host(temp.path()),
                "POST",
                "/r/codex-0123456789ab/v1/messages",
                auth
            )
            .await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(
                host(temp.path()),
                "POST",
                "/r/..%2F..%2Fsecrets/v1/messages",
                auth
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn idle_means_nothing_in_flight_for_the_whole_limit() {
        let activity = Activity::new(1_000);
        assert_eq!(activity.idle_for(1_000 + 3_599), Duration::from_secs(3_599));
        activity.in_flight.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            activity.idle_for(1_000 + 99_999),
            Duration::ZERO,
            "a long turn is not idleness"
        );
        activity.in_flight.fetch_sub(1, Ordering::SeqCst);
        assert!(activity.idle_for(1_000 + 3_600) >= IDLE_LIMIT);
    }

    /// The guarantee section 6.5 rests on, driven through the middleware that
    /// makes it rather than by poking the counter: headers out is not the end
    /// of a turn, the last byte of its body is.
    #[tokio::test]
    async fn a_streaming_turn_is_in_flight_until_its_last_byte() {
        let temp = tempfile::tempdir().unwrap();
        let host = host(temp.path());
        assert_eq!(host.activity.in_flight.load(Ordering::SeqCst), 0);

        let response = streamed(&host).await;
        assert_eq!(
            host.activity.in_flight.load(Ordering::SeqCst),
            1,
            "the response has only begun"
        );
        assert_eq!(
            host.activity.idle_for(now_secs() + 99_999),
            Duration::ZERO,
            "a turn still streaming is not idleness, however long it takes"
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"onetwo");
        assert_eq!(host.activity.in_flight.load(Ordering::SeqCst), 0);
        assert!(host.activity.idle_for(now_secs() + 3_600) >= IDLE_LIMIT);
    }

    /// A client that disconnects mid-turn leaves a body nobody reads to the
    /// end. It must not hold the bridge awake for the rest of the hour.
    #[tokio::test]
    async fn a_client_that_walks_away_mid_turn_lets_the_bridge_go_idle() {
        let temp = tempfile::tempdir().unwrap();
        let host = host(temp.path());

        let response = streamed(&host).await;
        assert_eq!(host.activity.in_flight.load(Ordering::SeqCst), 1);
        drop(response);
        assert_eq!(
            host.activity.in_flight.load(Ordering::SeqCst),
            0,
            "an abandoned body is not a turn still running"
        );
        assert!(host.activity.idle_for(now_secs() + 3_600) >= IDLE_LIMIT);
    }

    /// A port that refuses to bind is asked more than once who holds it,
    /// because a sibling that has bound but is not serving yet answers TCP
    /// and not HTTP. One timed-out ask is not a free port.
    #[test]
    fn a_port_is_asked_until_it_answers_or_the_tries_run_out() {
        // Answering only on the third ask is the case a single ask gets
        // wrong: a sibling that bound the port a moment ago and is still
        // building its runtime.
        let mut asked = 0;
        assert!(says_yes(PROBE_TRIES, Duration::ZERO, || {
            asked += 1;
            asked == 3
        }));
        assert_eq!(asked, 3, "it stops asking once someone answers");

        let mut asked = 0;
        assert!(!says_yes(PROBE_TRIES, Duration::ZERO, || {
            asked += 1;
            false
        }));
        assert_eq!(
            asked, PROBE_TRIES,
            "and only gives up on the port after every try"
        );
    }

    #[test]
    fn tokens_are_compared_whole() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
        assert!(!same(b"", b"a"));
    }
}
