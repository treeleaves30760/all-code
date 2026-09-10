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
mod tmux;
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

// Re-exported for the configuration TUI, which edits these directly.
pub(crate) use crate::remote::caps::SafetyRung;
pub(crate) use crate::remote::settings::{Bind as RemoteBind, RemoteSettings as Settings};
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
    /// Print the link to the page.
    Url,
    /// Turn sharing every session on or off.
    AutoShare {
        on: bool,
    },
    /// Report the resolved posture without binding anything.
    DryRun,
}

/// The pid and alc version of a hub currently listening for `config_dir`.
///
/// Read from the record the hub writes at startup rather than by asking it,
/// so a caller that only wants to mention the hub in passing - `alc update`,
/// say - does not open a socket to do it.
///
/// Unix only, matching the platforms that can have a hub at all.
#[cfg(unix)]
pub fn running_hub(config_dir: &std::path::Path) -> Option<(u32, String)> {
    ctl::read_hub_record(config_dir)
        .ok()
        .flatten()
        .map(|record| (record.pid, record.alc))
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
    wants_tmux: bool,
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
    require_supported_platform()?;
    // Checked before any work: a scripted `alc claude -p … > out.txt` must
    // fail loudly here rather than fill that file with escape sequences.
    require_terminal()?;
    // Resolved here rather than in the hub, even though the hub is what
    // starts the session: a missing or too-old tmux is the user's own
    // machine answering, and they should hear it from the command they
    // typed rather than as a failure relayed back over a control socket
    // from a daemon they did not know they had started.
    let tmux = wants_tmux.then(tmux::find).transpose()?;

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
    if let Some(refusal) = hub::hub_cannot_carry(&hub.alc, hub.pid, &spec) {
        bail!("{refusal}");
    }
    let create = ctl::CreateRequest {
        alc: env!("CARGO_PKG_VERSION").to_owned(),
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
        tmux: wants_tmux,
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
    match &tmux {
        // tmux owns the keyboard now, so the key alc would otherwise name is
        // not the one that works: `ctrl-\ then d` here would send the user's
        // first detach attempt straight into the agent. `ctrl-b` and not
        // "your prefix" because alc's server reads no configuration file, so
        // the prefix is tmux's own default whatever the user has bound in
        // their own.
        Some(found) => {
            println!(
                "  keys  ctrl-b then d detaches ({}); the session keeps running",
                found.label()
            );
            // The one thing about `--tmux` a user has to be told, because
            // it is the opposite of what a terminal usually promises: the
            // page owns the size. A terminal smaller than it is not broken,
            // it is showing a corner.
            println!(
                "  size  the page sets it; a smaller terminal shows the top-left corner \
                 (ctrl-b :refresh-client -L/-R/-U/-D to pan)"
            );
        }
        None => println!("  keys  ctrl-\\ then d detaches; the session keeps running"),
    }
    std::io::stdout().flush().ok();

    attach(store, &secrets, &session_id, cols, rows)
}

/// Puts this terminal on a session the hub owns.
///
/// Returns the agent's exit code when the session ends, and 0 when the user
/// detaches - detaching is a success, and reporting the agent's last status
/// for it would be a lie about a session that is still running.
///
/// For a plain session this terminal owns the size: the pty is one grid, it
/// was opened at this terminal's size, and `watch_resize` below keeps it
/// there. The page draws that grid scaled to fit its window rather than
/// resizing it, so the two no longer take turns.
pub(crate) fn attach(
    store: &Store,
    secrets: &Secrets,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> Result<u8> {
    // A tmux session is not relayed. The whole point of `--tmux` is that
    // this terminal becomes a second, independent tmux client holding its
    // own size, so it runs one rather than reading the mirror's bytes.
    //
    // This branch is load-bearing beyond that: the relay path starts
    // `watch_resize`, which reports this terminal's size to the session.
    // `Session::resize_from_terminal` refuses it for a tmux session, so a
    // card that arrived without its `tmux` field could not put this
    // terminal in charge of the page's window - but it would put this
    // terminal on the mirror's byte stream, which is not what `--tmux`
    // promised either.
    if let Some(card) = card_for(store, secrets, session_id)
        && let Some(host) = card.tmux
    {
        return attach_tmux(store, secrets, session_id, &host);
    }

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

/// Puts this terminal on a tmux session the hub owns, as a client in its own
/// right.
///
/// Nothing about the mirror changes: the hub still holds its own client, and
/// the page still shows what the agent draws. What changes is that the two
/// clients hold their own sizes, which is the whole reason `--tmux` exists -
/// and that this terminal holds its own without setting the window's. The
/// page drives the window (`tmux::attach_argv`), so this terminal renders
/// whatever of it fits: padded out when it is larger, and clipped to the
/// top-left corner when it is smaller, which is what the line below is for.
///
/// The exit code still comes from the card afterwards, for the same reason
/// the relay path reads it there - tmux's own client exits 0 whether the
/// agent finished or failed, so asking the hub is the only honest answer.
fn attach_tmux(
    store: &Store,
    secrets: &Secrets,
    session_id: &str,
    host: &tmux::Tmux,
) -> Result<u8> {
    // Resolved again rather than carried from `share`: `alc attach` reaches
    // here without having been through it, and a terminal that cannot run
    // tmux should hear that here rather than fail obscurely one line later.
    let found = tmux::find()?;
    let mut command = std::process::Command::new(&found.binary);
    // No vote in the window's size: the page owns it, and this terminal
    // renders whatever of it fits. See `tmux::attach_argv`, and keep this in
    // step with the `Sizing::MIRROR` on the hub's mirror.
    command.args(host.attach_argv(tmux::Sizing::TERMINAL));
    // A shell already inside tmux exports the address of ITS server, and a
    // client that inherits it refuses to attach - "sessions should be nested
    // with care". alc's server is a different one, so nesting is safe; the
    // note below is so the user is not surprised by needing their outer
    // prefix first.
    if env::var_os("TMUX").is_some() {
        println!(
            "this terminal is already inside tmux; alc's session runs on its own server, so \
             reach it with your outer prefix first"
        );
    }
    // Printed before the terminal is handed to tmux, which is the last
    // moment anything alc writes will be read.
    println!(
        "the page sets this session's size; a smaller terminal shows the top-left corner of it \
         (ctrl-b :refresh-client -L/-R/-U/-D to pan, -c to follow the cursor)"
    );
    command.env_remove("TMUX");
    command.env_remove("TMUX_PANE");

    let status = command
        .status()
        .with_context(|| format!("failed to run {}", found.binary.display()))?;

    // The client's own status cannot answer this on its own. A clean detach
    // exits 0, but so does nothing else: when the agent ends, alc stops the
    // tmux server to release every client, and a client whose server went
    // away exits 1 - which would report a session that finished perfectly
    // well as a failure to attach. So the hub is asked what happened, and
    // the client's status is only consulted for the case the hub cannot
    // describe: the session is still there and this terminal never got on
    // it.
    match card_for(store, secrets, session_id) {
        Some(card) => match card.exit.as_ref() {
            Some(exit) => Ok(exit_code(exit)),
            None if status.success() => {
                println!("detached; the session is still running.");
                println!("reattach with `alc attach {session_id}`");
                Ok(0)
            }
            None => bail!(
                "tmux could not put this terminal on session {session_id}; the session is still \
                 running, so try `alc attach {session_id}` again"
            ),
        },
        None => Ok(0),
    }
}

/// One session's card, by id, or `None` when no hub can say.
///
/// Errors are folded into `None` on purpose: both callers are asking a
/// question they have a sensible answer for either way - "is this a tmux
/// session" and "did the agent exit" - and a hub that has gone away between
/// the attach and this call should not turn a clean detach into a failure.
fn card_for(store: &Store, secrets: &Secrets, session_id: &str) -> Option<wire::SessionCard> {
    list(store, secrets)
        .ok()?
        .into_iter()
        .find(|card| card.id == session_id)
}

/// A signalled agent is reported as failing, matching what a shell reports
/// for the same death and what `launch::execute` already returns.
fn exit_code(exit: &ExitInfo) -> u8 {
    exit.code
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

/// What `--tmux` would do on this machine: the tmux it found, or why it
/// could not use one.
///
/// Exists for `--dry-run`, which has to be able to admit that the launch it
/// is describing would be refused - the same promise the adapter check makes
/// a few lines above it.
pub fn tmux_status() -> Result<String> {
    tmux::find().map(|found| found.label())
}

/// Whether the user has asked for every session to be shared.
///
/// Unreadable settings answer `false`: a standing preference is not worth
/// failing a launch over, and `alc remote status` reports the real error.
pub fn shares_by_default(store: &Store) -> bool {
    RemoteSettings::load(&store.dir)
        .map(|settings| settings.enabled && settings.auto_share)
        .unwrap_or(false)
}

/// Whether this invocation is one that CAN be shared.
///
/// Sharing needs a terminal on both ends. `--share` says so loudly when
/// there is not one; the standing preference just stays out of the way.
pub fn can_share() -> bool {
    require_supported_platform().is_ok()
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
}

/// Refuses on a platform where remote control is not known to work.
///
/// Windows is not verified. The hub starts a detached process and talks to
/// it over a loopback control socket, and on Windows CI that process does
/// not come up and does not go away - it stalls the job rather than failing.
/// Shipping that would mean a Windows user's `alc claude --share` hangs and
/// leaves something running, which is worse than not having the feature.
///
/// The code is compiled on Windows and its unit tests run there, so this is
/// a gate to lift rather than a body of work to redo. Everything else alc
/// does is unaffected.
fn require_supported_platform() -> Result<()> {
    #[cfg(not(unix))]
    {
        bail!(
            "remote control is not available on Windows yet - the session hub is unverified \
             there. Everything else alc does works normally; follow \
             https://github.com/treeleaves30760/all-code for when this lands."
        );
    }
    #[cfg(unix)]
    Ok(())
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
            print!("share by default: {}", on_off(settings.auto_share));
            // Sharing off makes the preference a no-op; saying so here keeps
            // this block, `alc doctor` and the config screen telling one story.
            if settings.auto_share && !settings.enabled {
                print!(" (inactive; remote control is off)");
            }
            println!();
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
        RemoteCommand::Url => {
            let secrets = Secrets::load_or_create(&store.dir)?;
            for url in page_urls(store, &secrets)? {
                println!("{url}");
            }
            Ok(0)
        }
        RemoteCommand::AutoShare { on } => {
            let mut settings = RemoteSettings::load(&store.dir)?;
            settings.auto_share = on;
            settings.save(&store.dir)?;
            println!("share every session: {}", on_off(on));
            // `shares_by_default` is `enabled && auto_share`, so promising a
            // shared session here while remote control is off would be a
            // straight untruth - and the user would go looking for the
            // reason in the wrong place.
            if on && !settings.enabled {
                println!(
                    "remote control is off, so nothing shares yet; turn it on with `alc remote on`."
                );
            } else if on {
                println!("`alc <agent>` now shares without --share; `--no-share` opts one out.");
            }
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
    require_supported_platform()?;
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
            // Printed first and always, because the link `alc <agent>
            // --share` shows scrolls away the moment the agent draws its
            // own interface, and this is where a user comes looking for it.
            for (index, url) in page_urls(store, &secrets)?.iter().enumerate() {
                println!("{:<6}{url}", if index == 0 { "page" } else { "" });
            }
            println!();
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
                    "{:<22} {:<8} {:<9} {:<10} {:<6} {}",
                    card.id,
                    card.agent,
                    state,
                    card.permission
                        .rung
                        .map(|rung| rung.to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    // Said here because it changes what `alc attach` does
                    // and which key detaches, and a user should learn that
                    // before they are inside the session rather than after.
                    if card.tmux.is_some() { "tmux" } else { "-" },
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

/// Every address the page can be opened at, most useful first.
///
/// This exists because the link `alc <agent> --share` prints scrolls away
/// the instant the agent draws its own interface, and it is the one thing
/// the user needs. `alc sessions` and `alc remote url` both print it.
///
/// The token is included, because a link without it is not a link. That
/// does mean it lands in shell scrollback - which is the same place it was
/// printed the first time, and a token that cannot be recovered is a
/// feature nobody can use.
pub(crate) fn page_urls(store: &Store, secrets: &Secrets) -> Result<Vec<String>> {
    let record = ctl::read_hub_record(&store.dir)?
        .context("no hub is running; start one with `alc <agent> --share`")?;
    let settings = RemoteSettings::load(&store.dir)?;

    let mut urls = vec![server::page_url(record.port, &secrets.operator, record.lan)];
    // A tunnel's own name is the useful one when there is a tunnel, and it
    // is always https: both `tailscale serve` and `cloudflared` terminate
    // TLS themselves. A wildcard entry is skipped - alc knows the pattern,
    // not the name the tunnel actually minted.
    for host in &settings.allowed_hosts {
        if host.starts_with("*.") {
            continue;
        }
        urls.push(format!("https://{host}/#k={}", secrets.operator));
    }
    Ok(urls)
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
    // Reported as what a launch would actually do rather than as the raw
    // field: with sharing off, `auto_share = true` shares nothing, and a
    // bare "on" here would send the reader hunting for a different cause.
    rows.push((
        "share by default",
        match (settings.enabled, settings.auto_share) {
            (false, true) => "on (inactive; sharing is off)".to_owned(),
            (_, auto_share) => on_off(auto_share).to_owned(),
        },
    ));
    rows.push(("bind", settings.bind.to_string()));
    if !settings.allowed_hosts.is_empty() {
        rows.push(("also answers to", settings.allowed_hosts.join(", ")));
    }
    rows.push(("ceiling", settings.max_permission.clone()));
    // A row, never an issue. `--tmux` is opt-in, so a user who never types
    // it must not start seeing a failing `alc doctor` because the feature
    // exists - the same rule the agent binaries and the local models follow.
    rows.push((
        "tmux",
        match tmux::find() {
            Ok(found) => format!("{} · `--tmux` available", found.label()),
            // A row says what is there, not what a launch would be told: the
            // refusal names the install command and what the user loses,
            // which is the right length for the moment they typed the flag
            // and the wrong length for a status line they did not ask for.
            Err(_) => "not found · `--tmux` needs tmux 3.2 or newer".to_owned(),
        },
    ));

    match ctl::read_hub_record(&store.dir) {
        Ok(Some(record)) => {
            rows.push((
                "hub",
                format!(
                    "pid {} · alc {} · http://127.0.0.1:{}/",
                    record.pid, record.alc, record.port
                ),
            ));
            // A hub outlives the binary that started it, and `alc update`
            // replaces that binary without stopping it. The hub is what
            // actually launches a shared session, from a request this build
            // serialised, so a version apart is a launch neither half fully
            // understands - and the half that loses is the newer one, whose
            // added fields the older hub drops in silence.
            if record.alc != env!("CARGO_PKG_VERSION") {
                issues.push((
                    "hub",
                    format!(
                        "it is alc {}, but this is alc {}; it would launch a shared session from the older build",
                        record.alc,
                        env!("CARGO_PKG_VERSION")
                    ),
                    "alc hub stop".to_owned(),
                ));
            }
        }
        _ => rows.push(("hub", "not running".to_owned())),
    }

    // Where to change any of the above. Without it `alc doctor` reports the
    // sharing posture and leaves the reader with no next step.
    rows.push((
        "settings",
        format!(
            "{} (or `alc config`)",
            RemoteSettings::path(&store.dir).display()
        ),
    ));

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

pub(crate) fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}
