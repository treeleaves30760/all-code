//! alc's background bridge: the Codex adapter as a process of its own.
//!
//! Claude Code runs background sessions - agent view, `claude --bg`, `←` on an
//! empty prompt - under a supervisor that outlives the terminal and the `alc`
//! that started them. An adapter living inside that `alc` died with it, on a
//! port the next launch could not know. So for Claude Code the adapter is one
//! detached `alc bridge serve` per configuration directory: loopback only, a
//! port chosen once and kept, a token on every model request, started on
//! demand by a launch or by the `apiKeyHelper` inside any session, and gone
//! after an hour with nothing to do. The other seven agents have no background
//! mode and keep their in-process adapter.

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};

use crate::config::Store;

pub(crate) mod files;
mod serve;

pub(crate) use serve::run as serve;

const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(10);
const STALE_LOCK: Duration = Duration::from_secs(30);

/// What a running bridge says about itself.
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct Hello {
    /// Tells one bridge process from another on a port that was reused.
    /// Nothing in alc compares two hellos yet, so it is carried, not read.
    #[allow(
        dead_code,
        reason = "part of the hello a bridge publishes; no caller compares instances yet"
    )]
    pub instance: String,
    pub alc: String,
    pub pid: u32,
    pub port: u16,
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(HELLO_TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

/// Asks whatever listens on `port` whether it is this configuration's bridge.
/// Only a listener that knows the token can answer, so some other program on
/// the port is never mistaken for it.
pub(crate) fn hello(port: u16, token: &str) -> Result<Hello> {
    let mut response = agent()
        .get(&format!("http://127.0.0.1:{port}/alc/hello"))
        .header("authorization", &format!("Bearer {token}"))
        .call()
        .context("no alc bridge answered")?;
    let text = response
        .body_mut()
        .read_to_string()
        .context("the bridge's answer could not be read")?;
    serde_json::from_str(&text).context("the bridge's answer did not parse")
}

/// A bridge that answered.
#[derive(Debug, Clone)]
pub(crate) struct Running {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub alc: String,
}

impl Running {
    /// What Claude Code's `ANTHROPIC_BASE_URL` starts with.
    pub(crate) fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// The bridge for `config_dir`, if one is up and answering to its token.
pub(crate) fn probe(config_dir: &Path) -> Option<Running> {
    let token = files::read_token(config_dir)?;
    let port = files::remembered_port(config_dir)?;
    let hello = hello(port, &token).ok()?;
    (hello.port == port).then_some(Running {
        port,
        token,
        pid: hello.pid,
        alc: hello.alc,
    })
}

/// The bridge for `config_dir`, starting one if none is up.
///
/// The probe and the spawn are separated by a lock file taken with
/// `create_new`, as the hub's are: two sessions asking at once must not both
/// start one. A lock older than half a minute belongs to a starter that died.
pub(crate) fn ensure(config_dir: &Path) -> Result<Running> {
    if let Some(running) = probe(config_dir) {
        return Ok(running);
    }
    files::load_or_create_token(config_dir)?;
    let lock = files::lock_path(config_dir);
    if lock_is_stale(&lock) {
        let _ = fs::remove_file(&lock);
    }
    let taken = OpenOptions::new().write(true).create_new(true).open(&lock);
    if taken.is_err() {
        return wait_for(config_dir).context(
            "timed out waiting for another alc to start the background bridge; run `alc bridge status`",
        );
    }
    let started = start_detached(config_dir);
    let running = wait_for(config_dir);
    let _ = fs::remove_file(&lock);
    started?;
    running.context(
        "alc's background bridge did not start; run `alc bridge serve` in a terminal to see why",
    )
}

fn wait_for(config_dir: &Path) -> Option<Running> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(running) = probe(config_dir) {
            return Some(running);
        }
        thread::sleep(Duration::from_millis(100));
    }
    None
}

fn lock_is_stale(lock: &Path) -> bool {
    fs::metadata(lock)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > STALE_LOCK)
}

/// Starts a bridge that outlives this process - and, when this process is the
/// `apiKeyHelper`, the Claude Code session that ran it. Its standard streams
/// are closed and nothing is inherited: Claude Code reads the helper's output
/// until it ends, and a bridge still holding it would hang every session.
fn start_detached(config_dir: &Path) -> Result<()> {
    let alc = std::env::current_exe().context("failed to find alc's own path")?;
    let mut command = Command::new(alc);
    command
        .arg("--config-dir")
        .arg(config_dir)
        .args(["bridge", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so the Ctrl-C of the terminal that happened
        // to start it, and the shell's SIGHUP on exit, do not reach it.
        command.process_group(0);
    }
    // Not a plain `spawn` on Windows: `win::spawn_detached` also keeps the
    // child from inheriting this process's handles, which is what the helper's
    // stdout being read to end of file depends on.
    #[cfg(windows)]
    crate::remote::win::spawn_detached(&mut command).context("failed to start the bridge")?;
    #[cfg(not(windows))]
    command.spawn().context("failed to start the bridge")?;
    Ok(())
}

/// Stops the bridge, answering the pid of the one it stopped.
pub(crate) fn stop(config_dir: &Path) -> Result<Option<u32>> {
    let Some(running) = probe(config_dir) else {
        return Ok(None);
    };
    agent()
        .post(&format!("{}/alc/stop", running.origin()))
        .header("authorization", &format!("Bearer {}", running.token))
        .send_empty()
        .context("the bridge did not take the stop request")?;
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if probe(config_dir).is_none() {
            return Ok(Some(running.pid));
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "the bridge (pid {}) did not stop within ten seconds",
        running.pid
    )
}

/// `label  value` rows for `alc bridge status` and `alc doctor`.
pub(crate) fn status_rows(config_dir: &Path) -> Vec<(&'static str, String)> {
    let ours = env!("CARGO_PKG_VERSION");
    let mut rows = Vec::new();
    match probe(config_dir) {
        Some(running) => {
            rows.push((
                "bridge",
                format!(
                    "running · pid {} · 127.0.0.1:{} · alc {}",
                    running.pid, running.port, running.alc
                ),
            ));
            if running.alc != ours {
                rows.push((
                    "version",
                    format!(
                        "started by alc {}; `alc bridge stop` swaps it for alc {ours}, and \
                         sessions reconnect within a minute",
                        running.alc
                    ),
                ));
            }
        }
        None => rows.push((
            "bridge",
            "not running · a Claude Code session on a Codex profile starts it when it needs it"
                .to_owned(),
        )),
    }
    rows.push(("routes", files::route_count(config_dir).to_string()));
    rows
}

/// What `alc claude-credential <route>` prints: the bridge's token for a Codex
/// route, after making sure a bridge is up, or a profile's API key.
///
/// Trimmed however it was resolved. Claude Code reads the helper's whole
/// stdout as the credential, and `Credentials::key_for` hands back an
/// environment variable exactly as the shell exported it - so a key exported
/// with a trailing newline would print two lines and break the one-line
/// contract every background session depends on.
pub(crate) fn credential(store: &Store, route: &str) -> Result<String> {
    Ok(resolve_credential(store, route)?.trim().to_owned())
}

fn resolve_credential(store: &Store, route: &str) -> Result<String> {
    if let Some(profile) = route.strip_prefix("profile:") {
        let provider = store.config.providers.get(profile).with_context(|| {
            format!(
                "alc has no provider profile named '{profile}' any more; start the session again \
                 with one that exists"
            )
        })?;
        return store
            .credentials
            .key_for(profile, provider)
            .with_context(|| {
                format!(
                    "profile '{profile}' has no API key this session can read; save one with \
                 `alc config key {profile}`"
                )
            });
    }
    let record = files::read_route(&store.dir, route)?.with_context(|| {
        format!(
            "alc's bridge has no route '{route}'; start `alc claude` once with that Codex profile \
             to recreate it"
        )
    })?;
    if !record.auth_file.is_file() {
        bail!(
            "Codex credentials were not found at {}; run `codex login` and retry",
            record.auth_file.display()
        );
    }
    Ok(ensure(&store.dir)?.token)
}
