//! Remote control: running a coding agent under a pseudo-terminal and
//! mirroring it to both the user's own terminal and a small embedded web
//! server, so the session can be watched and driven from a browser.
//!
//! # Why a terminal, and not each agent's own protocol
//!
//! alc launches eight coding agents across thirteen provider kinds. Only
//! some of them expose a machine-readable control channel, and the ones that
//! do disagree about everything: transport, session identity, and what an
//! approval even is. Half of them offer nothing usable at all. The terminal
//! is the one substrate all eight share, so that is the floor this is built
//! on, and a session works the same whichever agent and whichever provider
//! it runs. Per-agent structured channels layer over it later; nothing here
//! depends on them.
//!
//! # Security posture
//!
//! Typing into this page is remote code execution on the user's machine, so
//! the default is deliberately narrow: loopback only, a mandatory token, an
//! exact `Host` match including the port - the DNS-rebinding defence a
//! localhost server needs - and a required `Origin` on the WebSocket
//! upgrade. Reaching a session from a phone is a step the user takes on
//! purpose. A LAN bind needs the setting turned on *and* the flag passed;
//! anything beyond the LAN is a tunnel the user runs themselves, so alc
//! never has to be the thing exposed to the internet.
//!
//! alc also reads no configuration from the working repository. That gives
//! "a checked-in file can never enable sharing" as a property of the code
//! rather than a rule someone has to remember.

mod assets;
mod caps;
mod ctl;
mod fanout;
mod hub;
mod id;
mod local;
mod permission;
mod pty;
mod request;
mod ring;
mod screen;
mod scrub;
mod server;
mod session;
mod settings;
mod utf8;
mod wire;

use std::env;
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

use crate::config::Store;
use crate::launch::LaunchSpec;
use crate::remote::permission::EscalationGate;
use crate::remote::settings::{Bind, RemoteSettings, Secrets};
use crate::remote::wire::ExitInfo;

/// What `alc remote` was asked to do.
#[derive(Debug, Clone)]
pub enum RemoteCommand {
    Status,
    Enable,
    Disable,
    RotateTokens,
    /// Answer to another name, for a tunnel.
    AllowHost {
        host: String,
    },
    /// Report the resolved posture without binding anything.
    DryRun,
}

/// Runs the agent under a pty, mirrors it to this terminal and to a loopback
/// web server, and returns the agent's exit code.
///
/// The session lives exactly as long as this process. Detaching it into a
/// hub that outlives the terminal is the next milestone; until then, closing
/// the terminal closes the session, which is at least the behaviour a user
/// already expects from a foreground command.
pub fn share(
    store: &Store,
    mut spec: LaunchSpec,
    lan_requested: bool,
    name: Option<String>,
    permission: Option<String>,
) -> Result<u8> {
    let settings = RemoteSettings::load(&store.dir)?;
    if !settings.enabled {
        bail!("remote control is turned off; run `alc remote on` to enable it");
    }
    // Parsed before the environment checks below, so a typo in a value the
    // user typed is named whether or not they are at a terminal.
    let requested = permission
        .as_deref()
        .map(str::parse::<caps::SafetyRung>)
        .transpose()?;
    // Checked before any work: a scripted `alc claude -p … > out.txt` must
    // fail loudly here rather than fill that file with escape sequences.
    require_terminal()?;

    let permission = permission::arm_at_launch(&mut spec, requested);
    let secrets = Secrets::load_or_create(&store.dir)?;
    let cwd = env::current_dir().context("failed to read the current directory")?;
    let agent = spec.agent;
    let name = match name {
        Some(name) => {
            id::validate_name(&name)?;
            name
        }
        None => id::default_name(agent, &cwd),
    };
    let (cols, rows) = local::size();

    // The hub owns the session, so it survives this terminal closing and
    // shares one page with every other session on the machine.
    let hub = hub::spawn_or_join(&store.dir, &secrets, lan_requested)?;
    let create = ctl::CreateRequest {
        spec: hub::to_wire(&spec)?,
        cwd: cwd.display().to_string(),
        // The agent must run in the environment of the shell that asked for
        // it, not the one that happened to start the hub.
        environ: env::vars().collect(),
        name: name.clone(),
        cols,
        rows,
        scrollback_bytes: settings.scrollback_bytes,
        permission,
    };
    let session_id = match ctl::request(
        &store.dir,
        &secrets.ctl,
        &ctl::CtlRequest::Create(Box::new(create)),
    )? {
        ctl::CtlReply::Created { id } => id,
        other => bail!("the hub answered a create with {other:?}"),
    };

    let url = server::page_url(hub.port, &secrets.operator, lan_requested);
    println!("alc session {session_id} ({name})");
    println!("  open  {url}");
    println!(
        "  hub   {} (pid {}) · this link grants input; keep it to yourself",
        if lan_requested || settings.bind == Bind::Lan {
            format!("0.0.0.0:{} · reachable from this network", hub.port)
        } else {
            format!("127.0.0.1:{} · loopback only", hub.port)
        },
        hub.pid
    );
    println!("  keys  ctrl-\\ then d detaches; the session keeps running");
    std::io::stdout().flush().ok();

    attach(store, &secrets, &session_id, cols, rows)
}

/// Puts this terminal on a session the hub owns.
///
/// Returns the agent's exit code when the session ends, and 0 when the user
/// detaches - detaching is a success, and reporting the agent's last status
/// for it would be a lie about a session that is still running.
pub(crate) fn attach(
    store: &Store,
    secrets: &Secrets,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> Result<u8> {
    let stream = hub::attach_stream(&store.dir, &secrets.ctl, session_id, cols, rows)?;
    let terminal = local::TerminalGuard::acquire()?;

    let detached = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));

    let input = stream
        .try_clone()
        .context("failed to open the session's input channel")?;
    local::pump_stdin(input, Arc::clone(&detached), Arc::clone(&finished));

    let config_dir = store.dir.clone();
    let ctl_secret = secrets.ctl.clone();
    let id = session_id.to_owned();
    local::watch_resize(Arc::clone(&finished), move |cols, rows| {
        // Out of band: the relay itself carries only terminal bytes.
        let _ = ctl::request(
            &config_dir,
            &ctl_secret,
            &ctl::CtlRequest::Resize {
                id: id.clone(),
                cols,
                rows,
            },
        );
    });

    // This thread owns the terminal until the session ends or the user
    // detaches, which is what keeps the process alive.
    hub::relay(stream, std::io::stdout());
    finished.store(true, Ordering::Release);
    drop(terminal);

    if detached.load(Ordering::Acquire) {
        println!("detached; the session is still running.");
        println!("reattach with `alc attach {session_id}`");
        return Ok(0);
    }

    // The relay ended because the agent did. Ask what it exited with.
    let code = match ctl::request(&store.dir, &secrets.ctl, &ctl::CtlRequest::List) {
        Ok(ctl::CtlReply::Sessions { sessions }) => sessions
            .iter()
            .find(|card| card.id == session_id)
            .and_then(|card| card.exit.as_ref())
            .map(exit_code)
            .unwrap_or(0),
        _ => 0,
    };
    Ok(code)
}

/// A signalled agent is reported as failing, matching what a shell reports
/// for the same death and what `launch::execute` already returns.
fn exit_code(exit: &ExitInfo) -> u8 {
    exit.code
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

fn require_terminal() -> Result<()> {
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return Ok(());
    }
    bail!(
        "sharing a session needs an interactive terminal; \
         `--share` cannot be combined with redirected input or output"
    )
}

pub fn run_command(store: &Store, command: RemoteCommand) -> Result<u8> {
    match command {
        RemoteCommand::Status | RemoteCommand::DryRun => {
            let settings = RemoteSettings::load(&store.dir)?;
            let dir = Secrets::run_dir(&store.dir);
            println!("remote control: {}", on_off(settings.enabled));
            println!("bind:           {}", settings.bind);
            let hosts = if settings.allowed_hosts.is_empty() {
                "(loopback only)".to_owned()
            } else {
                settings.allowed_hosts.join(", ")
            };
            println!("also answers to: {hosts}");
            println!(
                "port:           {}",
                if settings.port == 0 {
                    "ephemeral".to_owned()
                } else {
                    settings.port.to_string()
                }
            );
            println!(
                "ceiling:        {} (looser needs `alc confirm` at this terminal)",
                settings.max_permission
            );
            println!(
                "settings:       {}",
                RemoteSettings::path(&store.dir).display()
            );
            println!(
                "credentials:    {} (values are never printed)",
                dir.display()
            );
            if matches!(command, RemoteCommand::DryRun) {
                println!("\nno socket was bound and no session was started.");
            }
            Ok(0)
        }
        RemoteCommand::Enable | RemoteCommand::Disable => {
            let mut settings = RemoteSettings::load(&store.dir)?;
            settings.enabled = matches!(command, RemoteCommand::Enable);
            settings.save(&store.dir)?;
            println!("remote control: {}", on_off(settings.enabled));
            Ok(0)
        }
        RemoteCommand::AllowHost { host } => {
            let settings = RemoteSettings::allow_host(&store.dir, &host)?;
            println!("now answering to: {}", settings.allowed_hosts.join(", "));
            println!("restart any running hub for this to take effect: alc hub stop --drain");
            Ok(0)
        }
        RemoteCommand::RotateTokens => {
            Secrets::rotate(&store.dir)?;
            println!("rotated the remote-control tokens; existing links no longer work");
            Ok(0)
        }
    }
}

/// The local half of the escalation gate.
///
/// Requires a controlling terminal, so a ticket cannot be redeemed by the
/// agent itself piping a command into a shell - which is the whole point:
/// the confirmation has to come from a person at this machine, not from
/// whoever holds the link, and not from the model.
pub fn confirm(store: &Store, ticket: &str) -> Result<u8> {
    if !std::io::stdin().is_terminal() {
        bail!("`alc confirm` has to be run at a terminal on this machine; that is what it is for");
    }
    let gate = EscalationGate::new(&store.dir)?;
    let rung = gate.grant(ticket)?;
    println!("granted: {rung}");
    println!("the page can apply it once, within the next minute.");
    Ok(0)
}

/// What `alc hub` and the session commands were asked to do.
#[derive(Debug, Clone)]
pub enum HubCommand {
    /// Start a hub, detached unless `foreground`.
    Start {
        foreground: bool,
        bind_lan: bool,
    },
    Stop {
        drain: bool,
    },
    Status,
    List {
        json: bool,
    },
    Attach {
        id: String,
    },
    Kill {
        id: String,
    },
    Rename {
        id: String,
        name: String,
    },
}

pub fn run_hub(store: &Store, command: HubCommand) -> Result<u8> {
    let secrets = Secrets::load_or_create(&store.dir)?;
    match command {
        HubCommand::Start {
            foreground: true,
            bind_lan,
        } => hub::Hub::run(&store.dir, bind_lan),
        HubCommand::Start {
            foreground: false,
            bind_lan,
        } => {
            let record = hub::spawn_or_join(&store.dir, &secrets, bind_lan)?;
            println!(
                "hub running on 127.0.0.1:{} (pid {})",
                record.port, record.pid
            );
            Ok(0)
        }
        HubCommand::Stop { drain } => {
            ctl::request(
                &store.dir,
                &secrets.ctl,
                &ctl::CtlRequest::Shutdown { drain },
            )?;
            println!("hub stopped");
            Ok(0)
        }
        HubCommand::Status => match ctl::read_hub_record(&store.dir)? {
            Some(record) => {
                match ctl::request(&store.dir, &secrets.ctl, &ctl::CtlRequest::Hello) {
                    Ok(ctl::CtlReply::Hello { alc, port, pid, .. }) => {
                        println!("hub:      running (pid {pid}, alc {alc})");
                        println!("page:     http://127.0.0.1:{port}/");
                        println!("sessions: {}", session_count(store, &secrets));
                        Ok(0)
                    }
                    // A record with nothing behind it: the hub was killed
                    // outright. Say so rather than reporting it as running.
                    _ => {
                        println!(
                            "hub:      not running (a stale record names pid {})",
                            record.pid
                        );
                        Ok(1)
                    }
                }
            }
            None => {
                println!("hub:      not running");
                Ok(1)
            }
        },
        HubCommand::List { json } => {
            let sessions = list(store, &secrets)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
                return Ok(0);
            }
            if sessions.is_empty() {
                println!("no shared sessions; start one with `alc <agent> --share`");
                return Ok(0);
            }
            for card in &sessions {
                let state = match card.state {
                    wire::SessionState::Running => "running",
                    wire::SessionState::Exited => "exited",
                };
                println!(
                    "{:<22} {:<8} {:<9} {:<10} {}",
                    card.id,
                    card.agent,
                    state,
                    card.permission
                        .rung
                        .map(|rung| rung.to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    card.cwd
                );
            }
            Ok(0)
        }
        HubCommand::Attach { id } => {
            require_terminal()?;
            let id = resolve(store, &secrets, &id)?;
            let (cols, rows) = local::size();
            attach(store, &secrets, &id, cols, rows)
        }
        HubCommand::Kill { id } => {
            let id = resolve(store, &secrets, &id)?;
            ctl::request(
                &store.dir,
                &secrets.ctl,
                &ctl::CtlRequest::Kill { id: id.clone() },
            )?;
            println!("stopped {id}");
            Ok(0)
        }
        HubCommand::Rename { id, name } => {
            let id = resolve(store, &secrets, &id)?;
            ctl::request(
                &store.dir,
                &secrets.ctl,
                &ctl::CtlRequest::Rename {
                    id: id.clone(),
                    name: name.clone(),
                },
            )?;
            println!("{id} is now '{name}'");
            Ok(0)
        }
    }
}

fn list(store: &Store, secrets: &Secrets) -> Result<Vec<wire::SessionCard>> {
    match ctl::request(&store.dir, &secrets.ctl, &ctl::CtlRequest::List)? {
        ctl::CtlReply::Sessions { sessions } => Ok(sessions),
        other => bail!("the hub answered a list with {other:?}"),
    }
}

fn session_count(store: &Store, secrets: &Secrets) -> usize {
    list(store, secrets)
        .map(|sessions| sessions.len())
        .unwrap_or(0)
}

/// Resolves an id prefix, the way git resolves a short hash. Typing ten
/// random characters to stop a session is not a thing anyone should have to
/// do.
fn resolve(store: &Store, secrets: &Secrets, prefix: &str) -> Result<String> {
    let sessions = list(store, secrets)?;
    let ids: Vec<String> = sessions.into_iter().map(|card| card.id).collect();
    id::resolve_prefix(ids.iter(), prefix)
}

/// What `alc doctor` should say about remote control.
///
/// Rows, not verdicts, for everything except two genuine problems: a user
/// who never turns this on must not start seeing a failing doctor because
/// the feature exists.
pub struct RemoteReport {
    pub rows: Vec<(&'static str, String)>,
    /// `(subject, problem, fix)` for the two conditions that are actually
    /// wrong rather than merely worth knowing.
    pub issues: Vec<(&'static str, String, String)>,
}

pub fn report(store: &Store) -> RemoteReport {
    let mut rows = Vec::new();
    let mut issues = Vec::new();

    let settings = match RemoteSettings::load(&store.dir) {
        Ok(settings) => settings,
        Err(error) => {
            issues.push((
                "remote.toml",
                error.to_string(),
                "fix or delete the file; a malformed one is never silently defaulted,                  because these values are a security posture"
                    .to_owned(),
            ));
            return RemoteReport { rows, issues };
        }
    };

    rows.push(("sharing", on_off(settings.enabled).to_owned()));
    rows.push(("bind", settings.bind.to_string()));
    if !settings.allowed_hosts.is_empty() {
        rows.push(("also answers to", settings.allowed_hosts.join(", ")));
    }
    rows.push(("ceiling", settings.max_permission.clone()));

    match ctl::read_hub_record(&store.dir) {
        Ok(Some(record)) => {
            rows.push((
                "hub",
                format!("pid {} · http://127.0.0.1:{}/", record.pid, record.port),
            ));
        }
        _ => rows.push(("hub", "not running".to_owned())),
    }

    let dir = Secrets::run_dir(&store.dir);
    rows.push(("credentials", dir.display().to_string()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["ctl.token", "operator.token", "viewer.token"] {
            let path = dir.join(name);
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                issues.push((
                    "remote token",
                    format!(
                        "{} is mode {mode:o}; other accounts can read it",
                        path.display()
                    ),
                    format!("chmod 600 {}", path.display()),
                ));
            }
        }
        if let Ok(metadata) = std::fs::metadata(&dir) {
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                issues.push((
                    "remote dir",
                    format!("{} is mode {mode:o}", dir.display()),
                    format!("chmod 700 {}", dir.display()),
                ));
            }
        }
    }

    RemoteReport { rows, issues }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}
