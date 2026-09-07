//! The hub: a process that owns sessions so they outlive the terminal that
//! started them, and so every one of them appears on a single page.
//!
//! `alc <agent> --share` becomes a thin client. It resolves the launch as it
//! always did, hands the resolved spec to the hub over the control socket,
//! and then attaches its own terminal to the session the hub created. Close
//! that terminal and the session keeps running; open the page and every
//! session is on it, whichever directory or agent it came from.
//!
//! # What travels with a create, and why
//!
//! The hub is long-lived and was started from whichever shell happened to
//! run the first `alc --share`. Spawning an agent into ITS directory with
//! ITS environment would mean a second session, started from a different
//! repository, quietly editing the first one's - so the client's working
//! directory and full environment travel with every request and are applied
//! to the child.
//!
//! # What a hub crash leaves behind
//!
//! On unix a pty child is its own session leader, so `kill -9` on the hub
//! does not take the agents with it: they reparent to init and keep running,
//! still holding temporary files the hub would have cleaned up - including
//! the plaintext key file the Kimi builder writes. A record per session is
//! therefore written to disk at spawn and removed on a clean exit, and the
//! next hub to start reaps whatever the last one left.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::{Agent, ProviderKind, ReasoningEffort};
use crate::launch::{self, LaunchSpec};
use crate::remote::ctl::{
    self, CreateRequest, CtlReply, CtlRequest, CtlStream, HubRecord, WireSpec,
};
use crate::remote::server::{Registry, Server};
use crate::remote::session::{Session, SessionSpec};
use crate::remote::settings::{RemoteSettings, Secrets};
use crate::remote::{id, wire};

/// How long a client waits for a hub it just started to publish itself.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an exited session's card stays on the page before it is reaped,
/// so a session that failed at launch can still be read.
const LINGER: Duration = Duration::from_secs(900);

pub(crate) struct Hub {
    config_dir: PathBuf,
    registry: Arc<Registry>,
    secrets: Secrets,
    instance: String,
    port: u16,
    /// Serialises spawning.
    ///
    /// `Bridge::start` reserves a loopback port and then drops the listener
    /// before the helper binds it - harmless for one launch at a time, a
    /// real collision when a hub starts several Codex-backed sessions at
    /// once, and those helpers would also be refreshing the same
    /// `~/.codex/auth.json` concurrently.
    spawning: Mutex<()>,
    stop: AtomicBool,
}

impl Hub {
    /// Runs the hub in this process until it is told to stop.
    pub(crate) fn run(config_dir: &Path, bind_lan: bool) -> Result<u8> {
        let settings = RemoteSettings::load(config_dir)?;
        let secrets = Secrets::load_or_create(config_dir)?;
        let instance = id::generate(Agent::Claude)?
            .split('-')
            .next_back()
            .unwrap_or("00000")
            .to_owned();

        reap_orphans(config_dir);

        let server = Server::bind(config_dir, &settings, &secrets, bind_lan)?;
        let server_address = server.address();
        let port = server_address.port();
        let registry = server.registry();
        server.serve();

        let listener = ctl::listen(config_dir)?;
        #[cfg(not(unix))]
        let ctl_port = listener.local_addr()?.port();

        let record = HubRecord {
            pid: std::process::id(),
            port,
            instance: instance.clone(),
            alc: env!("CARGO_PKG_VERSION").to_owned(),
            lan: server_address.ip().is_unspecified(),
            #[cfg(not(unix))]
            ctl_port,
        };
        crate::config::atomic_write(
            &ctl::hub_record_path(config_dir),
            serde_json::to_string(&record)?.as_bytes(),
            false,
        )?;

        let hub = Arc::new(Self {
            config_dir: config_dir.to_owned(),
            registry,
            secrets,
            instance,
            port,
            spawning: Mutex::new(()),
            stop: AtomicBool::new(false),
        });

        let reaper = Arc::clone(&hub);
        thread::Builder::new()
            .name("alc-hub-reaper".to_owned())
            .spawn(move || reaper.reap_loop())
            .ok();

        for stream in listener.incoming() {
            if hub.stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let hub = Arc::clone(&hub);
            thread::Builder::new()
                .name("alc-hub-ctl".to_owned())
                .spawn(move || hub.serve_control(stream))
                .ok();
        }

        // The guards ran as each session ended, so the records describe
        // files that are already gone. Leaving them would make the next hub
        // try to delete paths that may since belong to something else.
        let _ = std::fs::remove_dir_all(OrphanRecord::dir(config_dir));
        let _ = std::fs::remove_file(ctl::hub_record_path(config_dir));
        let _ = std::fs::remove_file(ctl::socket_path(config_dir));
        Ok(0)
    }

    fn serve_control(&self, stream: CtlStream) {
        let Ok(peer) = stream.try_clone() else { return };
        let mut reader = BufReader::new(stream);
        let request = match ctl::read_request(&mut reader, &self.secrets.ctl) {
            Ok(request) => request,
            Err(error) => {
                let _ = reply(
                    &peer,
                    &CtlReply::Error {
                        message: error.to_string(),
                    },
                );
                return;
            }
        };

        match request {
            CtlRequest::Hello => {
                let _ = reply(
                    &peer,
                    &CtlReply::Hello {
                        alc: env!("CARGO_PKG_VERSION").to_owned(),
                        instance: self.instance.clone(),
                        port: self.port,
                        pid: std::process::id(),
                    },
                );
            }
            CtlRequest::Create(create) => match self.create(*create) {
                Ok(id) => {
                    let _ = reply(&peer, &CtlReply::Created { id });
                }
                Err(error) => {
                    let _ = reply(
                        &peer,
                        &CtlReply::Error {
                            message: format!("{error:#}"),
                        },
                    );
                }
            },
            CtlRequest::List => {
                let _ = reply(
                    &peer,
                    &CtlReply::Sessions {
                        sessions: self.registry.cards(),
                    },
                );
            }
            CtlRequest::Kill { id } => match self.registry.get(&id) {
                Some(session) => {
                    let _ = session.kill();
                    let _ = reply(&peer, &CtlReply::Ok);
                }
                None => {
                    let _ = reply(
                        &peer,
                        &CtlReply::Error {
                            message: format!("no session matches '{id}'"),
                        },
                    );
                }
            },
            CtlRequest::Rename { id, name } => {
                let outcome = id::validate_name(&name).and_then(|()| {
                    self.registry
                        .get(&id)
                        .map(|session| session.rename(name.clone()))
                        .context("no session matches that id")
                });
                let _ = match outcome {
                    Ok(()) => reply(&peer, &CtlReply::Ok),
                    Err(error) => reply(
                        &peer,
                        &CtlReply::Error {
                            message: error.to_string(),
                        },
                    ),
                };
            }
            CtlRequest::Attach { id, cols, rows } => {
                let Some(session) = self.registry.get(&id) else {
                    let _ = reply(
                        &peer,
                        &CtlReply::Error {
                            message: format!("no session matches '{id}'"),
                        },
                    );
                    return;
                };
                if cols > 0 && rows > 0 {
                    let _ = session.resize(cols, rows);
                }
                if reply(&peer, &CtlReply::Ok).is_err() {
                    return;
                }
                self.relay_terminal(&session, peer, reader);
            }
            CtlRequest::Resize { id, cols, rows } => {
                if let Some(session) = self.registry.get(&id) {
                    let _ = session.resize(cols, rows);
                }
                let _ = reply(&peer, &CtlReply::Ok);
            }
            CtlRequest::Shutdown { drain } => {
                let live = self
                    .registry
                    .cards()
                    .iter()
                    .filter(|card| card.state == wire::SessionState::Running)
                    .count();
                if live > 0 && !drain {
                    let _ = reply(
                        &peer,
                        &CtlReply::Error {
                            message: format!(
                                "{live} session(s) are still running; pass --drain to stop them too"
                            ),
                        },
                    );
                    return;
                }
                for card in self.registry.cards() {
                    if let Some(session) = self.registry.get(&card.id) {
                        let _ = session.kill();
                    }
                }
                let _ = reply(&peer, &CtlReply::Ok);
                self.stop.store(true, Ordering::Relaxed);
                // Unblocks the accept loop, which is otherwise parked.
                let _ = ctl::connect(&self.config_dir);
            }
        }
    }

    /// Runs one attached terminal: the session's output out, the terminal's
    /// keystrokes in, until either end goes away.
    ///
    /// The attached terminal is a viewer like any browser, so it goes
    /// through the same fan-out - which is what keeps a lagging phone from
    /// ever slowing the local terminal down, and vice versa.
    fn relay_terminal(
        &self,
        session: &Arc<Session>,
        peer: CtlStream,
        mut reader: BufReader<CtlStream>,
    ) {
        let subscription = session.subscribe();
        let viewer = subscription.handle.id();

        // A terminal joining an existing session needs the screen, not the
        // backlog - the same snapshot a browser gets.
        let mut out = peer;
        let snapshot = session.snapshot();
        if out.write_all(snapshot.as_bytes()).is_err() {
            session.unsubscribe(viewer);
            return;
        }
        let _ = out.flush();

        let input = Arc::clone(session);
        let pump = thread::Builder::new()
            .name("alc-hub-attach-in".to_owned())
            .spawn(move || {
                let mut buffer = [0_u8; 4096];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if input.input(&buffer[..read]).is_err() {
                                break;
                            }
                        }
                    }
                }
            });

        while let Ok(frame) = subscription.rx.recv() {
            let written = match &*frame {
                crate::remote::fanout::Frame::Binary(bytes) if bytes.len() > 9 => {
                    // Strip the wire header: a terminal wants the bytes, not
                    // the sequence number a browser uses to resynchronise.
                    out.write_all(&bytes[9..])
                }
                crate::remote::fanout::Frame::Binary(_) => Ok(()),
                crate::remote::fanout::Frame::Text(_) => Ok(()),
                crate::remote::fanout::Frame::Close => break,
            };
            if written.is_err() || out.flush().is_err() {
                break;
            }
        }

        session.unsubscribe(viewer);
        drop(out);
        if let Ok(handle) = pump {
            let _ = handle.join();
        }
    }

    /// Spawns an agent on a client's behalf.
    fn create(&self, request: CreateRequest) -> Result<String> {
        let CreateRequest {
            spec,
            cwd,
            environ,
            name,
            cols,
            rows,
            scrollback_bytes,
            permission,
        } = request;
        let cwd = PathBuf::from(cwd);
        if !cwd.is_dir() {
            bail!("{} is not a directory on this machine", cwd.display());
        }

        let agent = spec.agent.parse::<Agent>()?;
        // The hub creates processes, so it refuses to create anything that
        // is not one of the eight agents alc knows - a compromised browser
        // token cannot reach this socket, but defence in depth is cheap here
        // and the alternative is an arbitrary-execution primitive.
        //
        // The escape hatch is read from the CLIENT's environment, not the
        // hub's: `ALC_<AGENT>_BIN` is something a person sets in the shell
        // they are launching from, and the hub was started by whichever
        // shell happened to share first.
        let program = PathBuf::from(&spec.program);
        let stem = program
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let known = Agent::ALL
            .iter()
            .any(|known| stem.starts_with(known.as_str()));
        let overridden = environ
            .iter()
            .any(|(name, value)| name == binary_override(agent) && !value.is_empty());
        if !known && !overridden {
            bail!(
                "'{stem}' is not one of the agents alc launches; set {} if that is deliberate",
                binary_override(agent)
            );
        }

        let spec = from_wire(spec, agent, &environ)?;
        let id = id::generate(agent)?;

        // Held across `prepare`, not only across the spawn: the bridge it
        // may start reserves a port and drops the listener before its helper
        // binds it.
        let prepared = {
            let _serialised = self
                .spawning
                .lock()
                .map_err(|_| anyhow::anyhow!("the hub's spawn lock was poisoned"))?;
            launch::prepare(spec)?
        };

        let record = OrphanRecord {
            id: id.clone(),
            cleanup: prepared.cleanup_paths(),
        };
        record.write(&self.config_dir).ok();

        let session = Session::start(
            SessionSpec {
                id: id.clone(),
                name,
                cols,
                rows,
                scrollback_bytes,
                permission,
            },
            &prepared.program,
            prepared.spec,
            &cwd,
            prepared.guards,
        )?;
        self.registry.insert(session);
        Ok(id)
    }

    /// Removes exited sessions once their cards have had time to be read,
    /// and clears their on-disk records.
    fn reap_loop(self: Arc<Self>) {
        let mut exited: BTreeMap<String, Instant> = BTreeMap::new();
        while !self.stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(1));
            for card in self.registry.cards() {
                if card.state != wire::SessionState::Exited {
                    continue;
                }
                let since = exited.entry(card.id.clone()).or_insert_with(Instant::now);
                if since.elapsed() > LINGER {
                    self.registry.remove(&card.id);
                    OrphanRecord::clear(&self.config_dir, &card.id);
                    exited.remove(&card.id);
                }
            }
        }
    }
}

fn binary_override(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "ALC_CLAUDE_BIN",
        Agent::Codex => "ALC_CODEX_BIN",
        Agent::Opencode => "ALC_OPENCODE_BIN",
        Agent::Pi => "ALC_PI_BIN",
        Agent::Copilot => "ALC_COPILOT_BIN",
        Agent::Goose => "ALC_GOOSE_BIN",
        Agent::Qwen => "ALC_QWEN_BIN",
        Agent::Kimi => "ALC_KIMI_BIN",
    }
}

fn reply(stream: &CtlStream, body: &CtlReply) -> Result<()> {
    let mut stream = stream;
    let mut line = serde_json::to_string(body)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    Ok(())
}

/// What the hub keeps on disk so the next one can clean up after a crash.
#[derive(serde::Serialize, serde::Deserialize)]
struct OrphanRecord {
    id: String,
    cleanup: Vec<String>,
}

impl OrphanRecord {
    fn dir(config_dir: &Path) -> PathBuf {
        Secrets::run_dir(config_dir).join("sessions")
    }

    fn write(&self, config_dir: &Path) -> Result<()> {
        let dir = Self::dir(config_dir);
        crate::remote::settings::restricted_dir(&dir)?;
        crate::config::atomic_write(
            &dir.join(format!("{}.json", self.id)),
            serde_json::to_string(self)?.as_bytes(),
            true,
        )
    }

    fn clear(config_dir: &Path, id: &str) {
        let _ = std::fs::remove_file(Self::dir(config_dir).join(format!("{id}.json")));
    }
}

/// Removes files a previous hub was killed before it could clean up.
///
/// The Kimi builder writes the provider's key into a temporary config file
/// so the agent can read it; `SessionGuards` deletes it when the session
/// ends. A hub that is SIGKILLed never runs that, and the key would sit on
/// disk indefinitely.
fn reap_orphans(config_dir: &Path) {
    let dir = OrphanRecord::dir(config_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Ok(record) = serde_json::from_str::<OrphanRecord>(&text) {
            for path in record.cleanup {
                let _ = std::fs::remove_file(path);
            }
        }
        let _ = std::fs::remove_file(entry.path());
    }
}

/// Rebuilds a `LaunchSpec` on the hub side.
fn from_wire(spec: WireSpec, agent: Agent, environ: &[(String, String)]) -> Result<LaunchSpec> {
    let mut env: BTreeMap<OsString, OsString> = environ
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect();
    // The launch's own environment wins over the client's ambient one: the
    // builder put provider credentials there deliberately.
    for (name, value) in &spec.env {
        env.insert(OsString::from(name), OsString::from(value));
    }

    Ok(LaunchSpec {
        program: OsString::from(spec.program),
        args: spec.args.into_iter().map(OsString::from).collect(),
        env,
        env_remove: spec.env_remove.into_iter().map(OsString::from).collect(),
        provider_name: spec.provider_name,
        provider_kind: spec.provider_kind.parse().unwrap_or(ProviderKind::Custom),
        agent,
        bridge: None,
        file_setup: Vec::new(),
        model: spec.model,
        effort: spec
            .effort
            .as_deref()
            .and_then(|effort| effort.parse::<ReasoningEffort>().ok()),
        secret_env: spec.secret_env.into_iter().map(OsString::from).collect(),
        secret_values: spec.secret_values,
    })
}

/// Turns a resolved launch into something that survives a socket.
pub(crate) fn to_wire(spec: &LaunchSpec) -> Result<WireSpec> {
    fn text(value: &OsString, what: &str) -> Result<String> {
        value.to_str().map(str::to_owned).with_context(|| {
            format!(
                "this launch has a {what} that is not valid UTF-8, which alc cannot hand to the hub"
            )
        })
    }

    Ok(WireSpec {
        program: text(&spec.program, "program path")?,
        args: spec
            .args
            .iter()
            .map(|arg| text(arg, "argument"))
            .collect::<Result<_>>()?,
        env: spec
            .env
            .iter()
            .map(|(name, value)| Ok((text(name, "variable name")?, text(value, "variable")?)))
            .collect::<Result<_>>()?,
        env_remove: spec
            .env_remove
            .iter()
            .map(|name| text(name, "variable name"))
            .collect::<Result<_>>()?,
        provider_name: spec.provider_name.clone(),
        provider_kind: spec.provider_kind.to_string(),
        agent: spec.agent.as_str().to_owned(),
        model: spec.model.clone(),
        effort: spec.effort.map(|effort| effort.to_string()),
        secret_values: spec.secret_values.clone(),
        secret_env: spec
            .secret_env
            .iter()
            .map(|name| text(name, "variable name"))
            .collect::<Result<_>>()?,
    })
}

/// Returns a running hub, starting one if there is not already a matching
/// one listening.
///
/// The probe and the spawn are separated by a lock file taken with
/// `create_new`, because two `alc --share` invocations racing here would
/// otherwise both find nothing and both start a hub - and the loser's would
/// fail to bind, leaving the user with an error for no reason.
pub(crate) fn spawn_or_join(
    config_dir: &Path,
    secrets: &Secrets,
    bind_lan: bool,
) -> Result<HubRecord> {
    if let Some(record) = probe(config_dir, secrets) {
        return Ok(record);
    }

    let lock = Secrets::run_dir(config_dir).join("hub.lock");
    let taken = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock);
    if taken.is_err() {
        // Somebody else is starting one. Wait for it rather than racing.
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(record) = probe(config_dir, secrets) {
                return Ok(record);
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_file(&lock);
        bail!("timed out waiting for a hub to start; try `alc hub status`");
    }

    let started = start_detached(config_dir, bind_lan);
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut outcome = None;
    while Instant::now() < deadline {
        if let Some(record) = probe(config_dir, secrets) {
            outcome = Some(record);
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(&lock);
    started?;
    outcome.context("the hub did not come up; run `alc hub start --foreground` to see why")
}

/// Asks whatever is listening whether it is a hub, and whether it is the one
/// this record describes. A stale `hub.json` from a crashed hub answers
/// nothing, and a different program on the port answers wrongly.
fn probe(config_dir: &Path, secrets: &Secrets) -> Option<HubRecord> {
    let record = ctl::read_hub_record(config_dir).ok().flatten()?;
    match ctl::request(config_dir, &secrets.ctl, &CtlRequest::Hello) {
        Ok(CtlReply::Hello { instance, port, .. }) if instance == record.instance => {
            Some(HubRecord { port, ..record })
        }
        _ => None,
    }
}

/// Starts a hub that outlives this process.
fn start_detached(config_dir: &Path, bind_lan: bool) -> Result<()> {
    let alc = std::env::current_exe().context("failed to find alc's own path")?;
    let mut command = Command::new(alc);
    command
        .arg("--config-dir")
        .arg(config_dir)
        .args(["hub", "start", "--foreground"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if bind_lan {
        command.arg("--bind-lan");
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so the terminal's Ctrl-C and the shell's
        // SIGHUP on exit do not reach it - which is the whole point of a
        // session that outlives the terminal.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    command.spawn().context("failed to start the hub")?;
    Ok(())
}

/// Attaches this terminal to a session the hub owns.
///
/// After the request is accepted the connection stops being line-delimited
/// JSON and becomes a raw byte relay in both directions - the same bytes the
/// pty produces and consumes, so nothing has to be re-encoded.
pub(crate) fn attach_stream(
    config_dir: &Path,
    secret: &str,
    id: &str,
    cols: u16,
    rows: u16,
) -> Result<CtlStream> {
    let mut stream = ctl::connect(config_dir)?;
    let envelope = serde_json::json!({
        "secret": secret,
        "request": { "op": "attach", "id": id, "cols": cols, "rows": rows },
    });
    let mut line = serde_json::to_string(&envelope)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut answer = String::new();
    reader.read_line(&mut answer)?;
    match serde_json::from_str::<CtlReply>(answer.trim()) {
        Ok(CtlReply::Ok) => Ok(stream),
        Ok(CtlReply::Error { message }) => bail!("{message}"),
        _ => bail!("the hub answered an attach with something else"),
    }
}

/// Copies bytes from a reader to a writer until either end closes. Used for
/// both directions of an attached terminal.
pub(crate) fn relay<R: Read, W: Write>(mut from: R, mut to: W) {
    let mut buffer = [0_u8; 8192];
    loop {
        match from.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if to.write_all(&buffer[..read]).is_err() || to.flush().is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_round_trips_through_the_wire_form() {
        let mut spec = LaunchSpec::for_test();
        spec.args = vec![OsString::from("--model"), OsString::from("gpt-5.6-terra")];
        spec.set_secret_env("ALC_PROVIDER_API_KEY", "never-print-this-value");
        spec.model = Some("gpt-5.6-terra".to_owned());

        let wire = to_wire(&spec).unwrap();
        let back = from_wire(wire, Agent::Codex, &[]).unwrap();

        assert_eq!(back.args, spec.args);
        assert_eq!(back.model, spec.model);
        assert_eq!(back.secret_values, spec.secret_values);
        assert_eq!(
            back.env.get(&OsString::from("ALC_PROVIDER_API_KEY")),
            Some(&OsString::from("never-print-this-value"))
        );
    }

    #[test]
    fn the_clients_environment_reaches_the_child_but_never_overrides_the_launch() {
        // Both halves matter. Without the client's environ the agent runs
        // with the hub's, which belongs to whichever shell started it; if
        // the client's won, an ambient ANTHROPIC_API_KEY would override the
        // provider key the builder resolved.
        let mut spec = LaunchSpec::for_test();
        spec.set_secret_env("ANTHROPIC_API_KEY", "the-resolved-provider-key");
        let wire = to_wire(&spec).unwrap();

        let environ = vec![
            ("PATH".to_owned(), "/client/bin".to_owned()),
            ("ANTHROPIC_API_KEY".to_owned(), "an-ambient-key".to_owned()),
        ];
        let back = from_wire(wire, Agent::Claude, &environ).unwrap();

        assert_eq!(
            back.env.get(&OsString::from("PATH")),
            Some(&OsString::from("/client/bin"))
        );
        assert_eq!(
            back.env.get(&OsString::from("ANTHROPIC_API_KEY")),
            Some(&OsString::from("the-resolved-provider-key"))
        );
    }

    #[test]
    fn a_launch_alc_cannot_represent_is_refused_rather_than_mangled() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut spec = LaunchSpec::for_test();
            spec.args = vec![OsString::from_vec(vec![0xff, 0xfe])];
            let error = to_wire(&spec).unwrap_err();
            assert!(error.to_string().contains("not valid UTF-8"), "{error}");
        }
    }
}

#[cfg(test)]
mod allowlist_tests {
    use super::*;

    /// Mirrors the check in `create`, which cannot be called without a live
    /// hub.
    fn accepted(program: &str, agent: Agent, environ: &[(String, String)]) -> bool {
        let stem = PathBuf::from(program)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let known = Agent::ALL
            .iter()
            .any(|known| stem.starts_with(known.as_str()));
        let overridden = environ
            .iter()
            .any(|(name, value)| name == binary_override(agent) && !value.is_empty());
        known || overridden
    }

    #[test]
    fn the_hub_spawns_the_agents_it_knows() {
        assert!(accepted("/usr/local/bin/claude", Agent::Claude, &[]));
        assert!(accepted("codex", Agent::Codex, &[]));
        // A path with the platform's own separator; the hub and the client
        // are always on the same machine, so there is no cross-platform
        // case to model here.
        #[cfg(windows)]
        assert!(accepted(r"C:\tools\opencode.exe", Agent::Opencode, &[]));
        #[cfg(not(windows))]
        assert!(accepted("/opt/bin/opencode", Agent::Opencode, &[]));
    }

    #[test]
    fn the_hub_refuses_to_spawn_anything_else() {
        // The control socket is a process-creation primitive; the guard is
        // what keeps it from being an arbitrary one.
        for program in ["/bin/sh", "curl", "/usr/bin/python3"] {
            assert!(!accepted(program, Agent::Goose, &[]), "{program} accepted");
        }
    }

    #[test]
    fn the_override_is_read_from_the_clients_environment_not_the_hubs() {
        // The hub was started by whichever shell shared first, and
        // `ALC_GOOSE_BIN` is something a person sets in the shell they are
        // launching from - so reading it from the hub's own environment
        // rejected every deliberately overridden binary.
        let environ = vec![("ALC_GOOSE_BIN".to_owned(), "/tmp/stand-in".to_owned())];
        assert!(accepted("/tmp/stand-in", Agent::Goose, &environ));
        // And it is per agent: an override for one does not open another.
        assert!(!accepted("/tmp/stand-in", Agent::Qwen, &environ));
    }

    #[test]
    fn an_empty_override_does_not_open_the_gate() {
        let environ = vec![("ALC_GOOSE_BIN".to_owned(), String::new())];
        assert!(!accepted("/tmp/stand-in", Agent::Goose, &environ));
    }
}
