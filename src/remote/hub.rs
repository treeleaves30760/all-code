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
//! The client's environment is layered ON the hub's rather than replacing it:
//! the hub is spawned with `Command::new` and no `env_clear`, and the pty's
//! base environment is this process's. That is fine for the child, whose
//! every meaningful variable is overridden by name - but it means anything
//! the hub itself resolves out of `std::env` while preparing a launch reads
//! the wrong shell's answer. What a launch needs from the user's environment
//! is therefore resolved client-side and carried: the agent's binary
//! override, the Codex `auth.json`, Claude Code's own `settings.json`, the
//! model and effort. Anything added later has to travel the same way.
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
use std::ffi::{OsStr, OsString};
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
use crate::remote::pty::PtyCommand;
use crate::remote::server::{Registry, Server};
use crate::remote::session::{Session, SessionSpec, TmuxHost};
use crate::remote::settings::{RemoteSettings, Secrets};
use crate::remote::{id, tmux, wire};

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
    /// Serialises the part of a launch that touches shared state outside the
    /// hub's own memory.
    ///
    /// The hub serves each control connection on its own thread, and
    /// `launch::prepare` is where a session's bridge is started and its
    /// temporary files are written. Two Codex-backed creates arriving
    /// together would otherwise be reading and rotating the same
    /// `~/.codex/auth.json` from two threads with nothing between them.
    ///
    /// (The bridge no longer configures itself through this process's
    /// environment - it takes a `BridgeConfig` by value - so the lock is
    /// narrower than it once was, and the environment it used to guard is
    /// the reason the comment said `set_var`.)
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

        // The per-session records are deliberately NOT swept here. Some
        // sessions are still live at this point, and their records are the
        // only note of the temporary files they hold - including the Kimi
        // builder's plaintext key file. Each record is removed when its own
        // session is reaped, and whatever is left after a hub dies badly is
        // reaped by the next hub's `reap_orphans`, which is what the records
        // exist for.
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
                    let _ = session.resize_from_terminal(cols, rows);
                }
                if reply(&peer, &CtlReply::Ok).is_err() {
                    return;
                }
                self.relay_terminal(&session, peer, reader);
            }
            CtlRequest::Resize { id, cols, rows } => {
                if let Some(session) = self.registry.get(&id) {
                    let _ = session.resize_from_terminal(cols, rows);
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
            alc,
            spec,
            cwd,
            environ,
            name,
            cols,
            rows,
            scrollback_bytes,
            permission,
            tmux: wants_tmux,
        } = request;
        // Refused rather than served on a best-effort basis: a spec this hub
        // and that client do not describe identically is one where a field
        // either side has never heard of is dropped in silence, and a
        // launch missing a field it needed is how a Codex session came to
        // run with no adapter in front of it.
        if alc != env!("CARGO_PKG_VERSION") {
            let named = if alc.is_empty() {
                "an alc too old to say which".to_owned()
            } else {
                format!("alc {alc}")
            };
            bail!(
                "this hub is alc {}, and the request came from {named}; run `alc hub stop` and \
                 retry so both halves are the same build",
                env!("CARGO_PKG_VERSION")
            );
        }

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

        let spec = from_wire(spec, &environ)?;
        let id = id::generate(agent)?;

        // Held across `prepare`, not only across the spawn: the bridge it
        // may start configures itself through this process's environment,
        // which every other session the hub is starting shares.
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

        // The agent is started here, not by `Session::start`, when it runs
        // in tmux: the pty then hosts a `tmux attach-session` client, and the
        // agent lives in a pane on the other side of the server.
        //
        // `prepared.spec` is what reaches the pane, untouched - the same
        // argv, the same environment, the permission flag
        // `permission::arm_at_launch` put at the front of it. Only the socket
        // and session names reach the attach client. Getting that backwards
        // would arm the gate on tmux and leave the agent running in whatever
        // mode it defaults to, while the card reported the rung the user
        // asked for at the highest confidence alc has.
        // Kept alongside `tmux` because `Session::start` takes ownership of
        // it, and the failure path below still has to reach the server.
        let mut orphan: Option<(tmux::Tmux, PathBuf)> = None;
        let (command, tmux) = match wants_tmux {
            true => {
                let found = tmux::find()?;
                let mut session = tmux::Tmux::for_session(&id)?;
                // A failed `create` can still have left a server running -
                // it refuses a launch whose credentials it could not take
                // back out of that server's environment, which is a check
                // made after the agent has started.
                session
                    .create(
                        &found.binary,
                        &prepared.program,
                        &prepared.spec,
                        &cwd,
                        cols,
                        rows,
                    )
                    .inspect_err(|_| {
                        let _ = session.stop(&found.binary);
                    })?;
                let command = PtyCommand {
                    program: found.binary.clone(),
                    // The mirror votes on the window size, and it is the
                    // only client that does: the page owns a `--tmux`
                    // session's geometry, and this pty is how a resize
                    // frame reaches tmux. Must stay in step with the
                    // `Sizing::TERMINAL` on the local terminal in
                    // `remote::attach_tmux` - with every client flagged,
                    // tmux counts them again and the size becomes a race.
                    args: session.attach_argv(tmux::Sizing::MIRROR),
                    // Deliberately not the launch's environment. The attach
                    // client only needs to talk to a socket, and giving it
                    // the provider key would put that key in a second
                    // process for no reason at all.
                    env: BTreeMap::new(),
                    env_remove: vec![OsString::from("TMUX"), OsString::from("TMUX_PANE")],
                };
                orphan = Some((session.clone(), found.binary.clone()));
                let host = TmuxHost {
                    tmux: session,
                    binary: found.binary,
                };
                (command, Some(host))
            }
            false => (PtyCommand::agent(&prepared.program, &prepared.spec), None),
        };

        // A failure from here on has to take the tmux server with it. The
        // agent is already running in a pane at this point, with the
        // launch's credentials in its environment, and nothing else knows
        // the socket: the session that would have carried it never existed,
        // so `Session::kill` cannot reach it, `alc sessions` cannot list it,
        // and `reap_orphans` would delete the temporary config it is still
        // reading while the agent itself carried on.
        let session = Session::start(
            SessionSpec {
                id: id.clone(),
                name,
                cols,
                rows,
                scrollback_bytes,
                permission,
            },
            command,
            prepared.spec,
            &cwd,
            prepared.guards,
            tmux,
        )
        .inspect_err(|_| {
            if let Some(host) = &orphan {
                let _ = host.0.stop(&host.1);
            }
        })?;
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
///
/// Destructured for the same reason [`to_wire`] is, and in the other
/// direction: a field added to `WireSpec` that nobody remembers to read here
/// would arrive on the socket and go nowhere. Between the two, a field can
/// only be lost by writing code that says so.
fn from_wire(spec: WireSpec, environ: &[(String, String)]) -> Result<LaunchSpec> {
    let WireSpec {
        program,
        args,
        env: launch_env,
        env_remove,
        provider_name,
        provider_kind,
        agent,
        bridge,
        codex_auth_file,
        claude_settings_file,
        file_setup,
        model,
        effort,
        secret_values,
        secret_env,
    } = spec;

    // Parsed here rather than taken from the caller, so the agent this spec
    // launches and the agent named on the wire cannot be two values that
    // merely happen to agree - and so no field of `WireSpec` is bound to `_`
    // in a destructure whose whole purpose is that none can go unread.
    let agent: Agent = agent.parse()?;

    let mut env: BTreeMap<OsString, OsString> = environ
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect();
    // The launch's own environment wins over the client's ambient one: the
    // builder put provider credentials there deliberately.
    for (name, value) in &launch_env {
        env.insert(OsString::from(name), OsString::from(value));
    }

    Ok(LaunchSpec {
        program: OsString::from(program),
        args: args.into_iter().map(OsString::from).collect(),
        env,
        env_remove: env_remove.into_iter().map(OsString::from).collect(),
        provider_name,
        provider_kind: provider_kind.parse().unwrap_or(ProviderKind::Custom),
        agent,
        // Carried, not dropped. `launch::prepare` starts the bridge from
        // this and hands the session a `SessionGuards` that stops it again,
        // so a shared Codex session gets the same adapter an unshared one
        // does instead of talking straight to the model vendor.
        bridge,
        codex_auth_file: codex_auth_file.map(PathBuf::from),
        claude_settings_file: claude_settings_file.map(PathBuf::from),
        file_setup,
        model,
        effort: effort
            .as_deref()
            .and_then(|effort| effort.parse::<ReasoningEffort>().ok()),
        secret_env: secret_env.into_iter().map(OsString::from).collect(),
        secret_values,
    })
}

/// Turns a resolved launch into something that survives a socket.
pub(crate) fn to_wire(spec: &LaunchSpec) -> Result<WireSpec> {
    fn text(value: &OsStr, what: &str) -> Result<String> {
        value.to_str().map(str::to_owned).with_context(|| {
            format!(
                "this launch has a {what} that is not valid UTF-8, which alc cannot hand to the hub"
            )
        })
    }

    // Destructured rather than read field by field, so adding a field to
    // `LaunchSpec` fails to compile here instead of quietly not crossing the
    // wire. `bridge` and `file_setup` were dropped exactly that way, and the
    // session that reached the user was an agent pointed at a model only the
    // missing bridge could serve.
    let LaunchSpec {
        program,
        args,
        env,
        env_remove,
        provider_name,
        provider_kind,
        agent,
        bridge,
        codex_auth_file,
        claude_settings_file,
        file_setup,
        model,
        effort,
        secret_env,
        secret_values,
    } = spec;

    Ok(WireSpec {
        program: text(program, "program path")?,
        args: args
            .iter()
            .map(|arg| text(arg, "argument"))
            .collect::<Result<_>>()?,
        env: env
            .iter()
            .map(|(name, value)| Ok((text(name, "variable name")?, text(value, "variable")?)))
            .collect::<Result<_>>()?,
        env_remove: env_remove
            .iter()
            .map(|name| text(name, "variable name"))
            .collect::<Result<_>>()?,
        provider_name: provider_name.clone(),
        provider_kind: provider_kind.to_string(),
        agent: agent.as_str().to_owned(),
        bridge: bridge.clone(),
        codex_auth_file: codex_auth_file
            .as_ref()
            .map(|path| text(path.as_os_str(), "Codex credential path"))
            .transpose()?,
        claude_settings_file: claude_settings_file
            .as_ref()
            .map(|path| text(path.as_os_str(), "Claude Code settings path"))
            .transpose()?,
        file_setup: file_setup.clone(),
        model: model.clone(),
        effort: effort.map(|effort| effort.to_string()),
        secret_values: secret_values.clone(),
        secret_env: secret_env
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
///
/// The version comes from the live reply rather than the record, because the
/// record was written when that hub started and `alc update` replaces the
/// binary underneath it without stopping it.
fn probe(config_dir: &Path, secrets: &Secrets) -> Option<HubRecord> {
    let record = ctl::read_hub_record(config_dir).ok().flatten()?;
    match ctl::request(config_dir, &secrets.ctl, &CtlRequest::Hello) {
        Ok(CtlReply::Hello {
            alc,
            instance,
            port,
            ..
        }) if instance == record.instance => Some(HubRecord {
            port,
            alc,
            ..record
        }),
        _ => None,
    }
}

/// Why a hub already running some other version of alc cannot carry this
/// launch, if it cannot.
///
/// The session is launched by the hub's binary, from a request this one
/// serialised, and the two only agree about that request while they are the
/// same build. The cost of getting it wrong is not a parse error: a
/// `WireSpec` field the older hub has never heard of is dropped in silence by
/// serde, which is exactly how a shared `alc --codex claude` came to run with
/// no bridge - so an upgrade that left yesterday's hub listening would have
/// gone on reproducing the bug it fixed.
///
/// Scoped to launches that actually carry something a version apart can lose.
/// A plain `alc --openrouter opencode --share` is fully described by fields
/// every version has had, and refusing it would turn a patch release into an
/// outage for sessions it serves perfectly well.
///
/// Pure and parameterised so the four cases are asserted rather than assumed.
pub(crate) fn hub_cannot_carry(hub_alc: &str, hub_pid: u32, spec: &LaunchSpec) -> Option<String> {
    let ours = env!("CARGO_PKG_VERSION");
    if hub_alc == ours {
        return None;
    }
    if spec.bridge.is_none() && spec.file_setup.is_empty() {
        return None;
    }
    Some(format!(
        "this session needs the Codex adapter, and the hub that would run it is alc {hub_alc} \
         (pid {hub_pid}) while this is alc {ours}; an older hub drops what it does not recognise \
         and would launch the agent with no adapter behind it. Run `alc hub stop` and retry, or \
         `--no-share` to run this one outside the hub"
    ))
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
        let back = from_wire(wire, &[]).unwrap();

        assert_eq!(back.args, spec.args);
        assert_eq!(back.model, spec.model);
        assert_eq!(back.secret_values, spec.secret_values);
        assert_eq!(
            back.env.get(&OsString::from("ALC_PROVIDER_API_KEY")),
            Some(&OsString::from("never-print-this-value"))
        );
    }

    /// The property the two named tests below are instances of: a launch
    /// that crosses the socket is the same launch on the other side.
    ///
    /// Whole-struct equality against a fixture that is empty nowhere, rather
    /// than a list of fields somebody remembered to assert - the previous
    /// round trip checked four of thirteen fields, and the two it did not
    /// check are the two that were being dropped.
    #[test]
    fn every_field_of_a_launch_survives_the_wire() {
        let spec = LaunchSpec::saturated();
        let back = from_wire(to_wire(&spec).unwrap(), &[]).unwrap();
        assert_eq!(back, spec);
    }

    /// The other half: a field that crosses as a placeholder passes the
    /// equality test above only if the fixture had that placeholder too, so
    /// the fixture's own non-emptiness is checked rather than assumed.
    #[test]
    fn nothing_in_the_saturated_fixture_is_empty() {
        let wire = serde_json::to_value(to_wire(&LaunchSpec::saturated()).unwrap()).unwrap();
        let object = wire.as_object().expect("a JSON object");
        assert!(!object.is_empty());
        for (name, value) in object {
            let empty = value.is_null()
                || value.as_str() == Some("")
                || value.as_array().is_some_and(Vec::is_empty)
                || value.as_object().is_some_and(serde_json::Map::is_empty);
            assert!(!empty, "`{name}` is empty, so it proves nothing: {value}");
        }
    }

    /// The regression this file exists to not repeat.
    ///
    /// A shared `alc --codex claude` reached Claude Code with the Codex model
    /// picker in its arguments and `bridge: None` behind it, so the agent
    /// asked api.anthropic.com for `gpt-6-astra` and was told - correctly -
    /// that no such model exists. The plan and the file setup have to survive
    /// the socket, because the hub is the process that acts on them.
    #[test]
    fn a_bridged_launch_keeps_its_bridge_and_its_file_setup() {
        let mut spec = LaunchSpec::for_test();
        spec.bridge = Some(crate::launch::BridgePlan {
            model: "gpt-6-astra".to_owned(),
            effort: None,
            context_window: Some(272_000),
            options: crate::model_catalog::ModelCatalog::built_in().models,
            api: crate::launch::BridgeApi::Messages,
        });
        spec.codex_auth_file = Some(PathBuf::from("/work/codex/auth.json"));
        spec.file_setup = vec![crate::launch::FileSetup::WriteTemp {
            path: PathBuf::from("/tmp/alc-kimi.json"),
            contents: "{\"apiKey\":\"never-print-this-value\"}".to_owned(),
            secret: true,
            cleanup: true,
        }];

        let back = from_wire(to_wire(&spec).unwrap(), &[]).unwrap();

        // The client's CODEX_HOME, not the hub's: the hub's belongs to
        // whichever shell started it, which may have been another project
        // days ago.
        assert_eq!(
            back.codex_auth_file,
            Some(PathBuf::from("/work/codex/auth.json"))
        );
        let plan = back.bridge.expect("the bridge plan crossed the wire");
        assert_eq!(plan.model, "gpt-6-astra");
        assert_eq!(plan.api, crate::launch::BridgeApi::Messages);
        assert_eq!(plan.context_window, Some(272_000));
        assert_eq!(plan.options.len(), 4);
        match back.file_setup.as_slice() {
            [crate::launch::FileSetup::WriteTemp { path, secret, .. }] => {
                assert_eq!(path, &PathBuf::from("/tmp/alc-kimi.json"));
                assert!(secret, "a secret file stays marked secret on the hub side");
            }
            other => panic!("the file setup did not survive the wire: {other:?}"),
        }
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
        let back = from_wire(wire, &environ).unwrap();

        assert_eq!(
            back.env.get(&OsString::from("PATH")),
            Some(&OsString::from("/client/bin"))
        );
        assert_eq!(
            back.env.get(&OsString::from("ANTHROPIC_API_KEY")),
            Some(&OsString::from("the-resolved-provider-key"))
        );
    }

    /// The upgrade path this bug's fix would otherwise not survive: a hub
    /// left running from the build that had the bug drops the new wire
    /// fields in silence, so joining it would go on shipping the same
    /// broken session from a binary that believes it is fixed.
    #[test]
    fn a_hub_running_another_version_of_alc_is_refused_only_for_what_it_could_lose() {
        let plain = LaunchSpec::for_test();
        let mut bridged = LaunchSpec::for_test();
        bridged.bridge = Some(crate::launch::BridgePlan {
            model: "gpt-6-astra".to_owned(),
            effort: None,
            context_window: None,
            options: Vec::new(),
            api: crate::launch::BridgeApi::Messages,
        });

        // A version apart plus something it can drop: refused, by name.
        let refusal = hub_cannot_carry("1.4.1", 4321, &bridged).expect("refused");
        assert!(refusal.contains("1.4.1"), "{refusal}");
        assert!(refusal.contains("4321"), "{refusal}");
        assert!(refusal.contains("alc hub stop"), "{refusal}");

        // A version apart, but nothing an older hub could lose: a patch
        // release must not take every plain shared session down with it.
        assert_eq!(hub_cannot_carry("1.4.1", 4321, &plain), None);

        // The same build: never in the way.
        let ours = env!("CARGO_PKG_VERSION");
        assert_eq!(hub_cannot_carry(ours, 4321, &bridged), None);
        assert_eq!(hub_cannot_carry(ours, 4321, &plain), None);
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
