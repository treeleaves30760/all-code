//! Running a shared session's agent inside tmux, so the browser and the
//! terminal can each watch it at their own size.
//!
//! # The problem this exists for
//!
//! Without it, a shared session is one pty with two viewers. A pty has
//! exactly one size, so the browser and the local terminal take turns
//! setting it: whichever resized last wins, and the other one is left
//! rendering a full-screen TUI at a width it does not have. Claude Code
//! draws a boxed prompt to the column it was told about, so the losing side
//! sees wrapped borders, doubled lines and a cursor in the wrong place.
//!
//! tmux is a terminal multiplexer, which is the tool built for exactly this:
//! one program, many attached clients, each with its own size. alc starts the
//! agent in a tmux session, the hub attaches to it as one client, and the
//! user's terminal attaches to it as a second, independent one.
//!
//! The two clients are not equals, and `attach_argv` is where that is
//! decided: the user's terminal sizes the window and the mirror abstains
//! (`ignore-size`), because a mirror that voted would make the window
//! narrower than itself and tmux would then re-emit the pane row by row into
//! it - past a secret scrubber that matches contiguous bytes, and past a
//! permission probe that reads the bottom rows of a grid whose bottom rows
//! would be tmux's padding.
//!
//! # Why a private server per session
//!
//! alc never touches the user's own tmux server. Every session gets its own
//! socket, named after the alc session id, for three reasons that are each
//! sufficient on their own:
//!
//! * **Credentials.** A tmux server inherits its environment from the client
//!   that starts it, and hands that environment to the panes it spawns. That
//!   is how the provider key reaches the agent without ever appearing in an
//!   argv, where `ps` - and on Linux, other accounts' `ps` - would read it.
//!   A server shared between launches would hand the second launch the
//!   *first* one's key, silently.
//! * **Identity.** A socket keyed by anything reusable - the directory, say -
//!   means a second `alc --tmux --share claude` in the same repository
//!   quietly attaches to the first agent, while alc reports the provider,
//!   model and permission rung the user asked for this time. A per-session
//!   socket cannot do that.
//! * **Teardown.** `alc kill` ends with one `kill-server` on a server
//!   nothing else is using.
//!
//! The server also starts with `-f /dev/null`, so it reads no configuration
//! file at all. That is not tidiness: tmux runs `run-shell` and `if-shell`
//! lines out of a config as the user, and the agent can write files in the
//! user's home directory. Sourcing `~/.tmux.conf` would hand an agent that
//! edited it code execution inside the *next* session's server - the one
//! holding that launch's credentials, before alc has taken them back out of
//! the server environment. It also makes alc's sessions behave identically
//! for everyone, which is what lets the docs name `ctrl-b` as the prefix.
//!
//! # What tmux is not
//!
//! It is not a security boundary between alc and the agent. The pane can
//! reach its own server, so an agent that can already run shell commands can
//! type into itself. alc unsets `TMUX` in the pane so the address is not
//! handed over for free, and removes the credential names from the server's
//! own environment so `show-environment` does not serve them - but an agent
//! running arbitrary commands was never something the permission gate could
//! contain, and `--tmux` does not change that either way. The docs say so.
//!
//! It *is* a boundary between the browser and the agent, and `send` is where
//! that lives. A viewer's keystrokes are delivered to the pane with
//! `send-keys -H`, never written to the mirror's own terminal, because the
//! mirror is a tmux client and bytes written to a tmux client go through
//! tmux's key parser first: a viewer who typed the prefix would get tmux's
//! command prompt, and `:run-shell` from there is a shell that alc's
//! permission ceiling and its escalation gate never see. `-H` hands tmux
//! bytes for the pane and no keys at all.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::launch::LaunchSpec;
use crate::remote::wire::ExitInfo;

/// The oldest tmux alc will use.
///
/// Set by `attach-session -f ignore-size`, which arrived in 3.2 and is what
/// keeps the mirror out of the window-sizing vote (see `attach_argv`).
/// `window-size` itself only needs 2.9.
///
/// Deliberately not 3.3. `allow-passthrough` would have raised the floor
/// past Ubuntu 22.04, RHEL 9 and Amazon Linux 2023, and it gates only the
/// tmux-specific `ESC P tmux;` wrapper, which no coding agent emits - so it
/// would have cost those users the feature and bought nothing. tmux's
/// default of `off` also keeps it swallowing OSC 52, which is alc's own
/// policy anyway.
#[cfg(unix)]
pub(crate) const MIN_VERSION: (u32, u32) = (3, 2);

/// How the pane's own process is started.
///
/// `exec` so the pane's process *is* the agent: `#{pane_pid}` is the pid a
/// session card shows and `#{pane_dead_status}` is the agent's own exit
/// status, not a shell's. The unset is the part that needs explaining -
/// tmux puts its socket address in `TMUX` for everything it spawns, and
/// leaving it there means any `tmux` command the agent runs silently targets
/// alc's server, including `send-keys` into its own pane.
const PANE_WRAPPER: &str = r#"unset TMUX TMUX_PANE; exec "$0" "$@""#;

/// How many bytes of input go into one `send-keys`.
///
/// Each byte is a separate argument, so a pasted prompt sent in one call
/// would be an argument list the kernel refuses. 512 keeps a call well
/// inside every platform's limit and still sends an ordinary keystroke in
/// one.
const SEND_CHUNK: usize = 512;

/// A tmux server alc owns, and the single session living on it.
///
/// Travels to the browser on the session card, which is what lets
/// `alc attach` put a terminal on the same tmux session rather than on the
/// hub's byte relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Tmux {
    /// The `-L` label the server was created under. Not a path: tmux builds
    /// `$TMUX_TMPDIR/tmux-<uid>/<label>` from it, and a label carrying a
    /// path separator would place the socket outside that 0700 directory,
    /// at a guessable path in world-writable `/tmp` - so `for_session` is
    /// the only constructor and it validates the shape.
    pub label: String,
    /// The socket's resolved path, read back from tmux once the server
    /// exists.
    ///
    /// Every command after creation addresses the server by this path
    /// (`-S`) rather than by re-deriving the label (`-L`). The hub inherited
    /// its environment from whichever shell shared first, possibly days ago,
    /// and `TMUX_TMPDIR` is part of that environment: a terminal attaching
    /// from a different shell would re-derive a different path and find
    /// nothing there.
    pub path: String,
    pub session: String,
    /// The pane the agent runs in, as tmux's own `%0`-style id.
    ///
    /// Every command after creation targets this rather than the session,
    /// and the difference is not cosmetic: `-t <session>` means "whatever
    /// window is current", so a user who opens a second window with the
    /// prefix key moves what the watcher is watching. It would then read
    /// that window's pane for the agent's exit status - and on seeing a
    /// shell the user closed, tear down a session whose agent is still
    /// working.
    pub pane: String,
}

/// One reading of a tmux session: whether the agent is still alive, how it
/// ended if not, the size tmux settled the window on, and the agent's pid.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Snapshot {
    /// How the agent ended, or `None` while it is still running.
    ///
    /// This is why `remain-on-exit` is on. Without it the pane's death closes
    /// the window, ends the session and stops the server, and every attached
    /// client exits 0 whatever the agent did - so a failing agent would be
    /// reported as a clean one, and a session stopped from the page as a
    /// session that finished. With it the pane stays long enough to be
    /// asked, and the watcher tears the session down straight afterwards.
    pub exit: Option<ExitInfo>,
    /// The size tmux settled on for the window, which under
    /// `window-size smallest` is the smaller of the two attached clients.
    ///
    /// Not the same as the hub's pty size, and the difference is load
    /// bearing: the hub's client is whatever the browser asked for, and tmux
    /// fills the rows past the window with padding. Anything reading the
    /// *agent's* output out of the grid - `permission::probe` reads the
    /// bottom rows for a mode line - has to look inside the window rather
    /// than at the bottom of the client, or it reads padding forever.
    pub window: Option<(u16, u16)>,
    /// The agent's own pid. `exec` in the pane wrapper is what makes this
    /// the agent rather than a shell holding it.
    pub pane_pid: Option<u32>,
}

/// tmux's name for a signal, from whatever it gave us.
///
/// It reports a name where the platform can supply one and the number where
/// it cannot, so the same stopped agent reads as `hup` on one machine and
/// `1` on another - measured, macOS against Ubuntu's tmux on CI. A session
/// card is read by a person, so the number is turned back into the name for
/// the signals a coding agent actually dies of, and anything else is passed
/// through rather than guessed at.
fn signal_name(signal: &str) -> String {
    let named = match signal.trim() {
        "1" => "hup",
        "2" => "int",
        "3" => "quit",
        "6" => "abrt",
        "9" => "kill",
        "13" => "pipe",
        "15" => "term",
        other => return other.to_ascii_lowercase(),
    };
    named.to_owned()
}

impl Snapshot {
    fn parse(line: &str) -> Option<Self> {
        let fields: Vec<&str> = line.trim().split('|').collect();
        let [dead, status, signal, cols, rows, pid] = fields.as_slice() else {
            return None;
        };
        // `pane_dead` is the field the whole reading turns on, so a reply
        // that does not answer it is not an answer. Anything else read out
        // of a blank line would say "alive, no size, no pid" - which is the
        // safe shape, and exactly the shape that would keep a watcher
        // polling a session that is no longer there.
        if !matches!(dead.trim(), "0" | "1") {
            return None;
        }
        let exit = (dead.trim() == "1").then(|| {
            // A signalled death and a plain `exit 1` are the same byte in a
            // process exit code, so the signal is checked first: a card that
            // could not tell them apart would call an agent the user stopped
            // a failing one.
            if signal.is_empty() {
                ExitInfo {
                    code: status.parse().ok(),
                    signal: None,
                }
            } else {
                ExitInfo {
                    code: None,
                    signal: Some(signal_name(signal)),
                }
            }
        });
        let window = cols
            .parse::<u16>()
            .ok()
            .zip(rows.parse::<u16>().ok())
            .filter(|(cols, rows)| *cols > 0 && *rows > 0);
        Some(Self {
            exit,
            window,
            pane_pid: pid.parse().ok(),
        })
    }
}

/// Whether an attaching client gets a say in how large the window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sizing {
    /// The user's own terminal: the window is this size.
    Drive,
    /// The mirror the hub holds open for the browser. See `attach_argv`.
    Abstain,
}

/// Where tmux is and what version it is, resolved once at launch.
pub(crate) struct Found {
    pub binary: PathBuf,
    pub version: (u32, u32),
}

impl Found {
    pub(crate) fn label(&self) -> String {
        format!("tmux {}.{}", self.version.0, self.version.1)
    }
}

/// Resolves tmux, or explains what is missing.
///
/// Every refusal names the thing, where alc looked, and what the user loses
/// by dropping the flag - the session still shares either way; only the two
/// sizes stop being independent.
pub(crate) fn find() -> Result<Found> {
    #[cfg(not(unix))]
    {
        bail!(
            "`--tmux` is not available on Windows; tmux has no Windows port, and the session hub \
             it would run under is unverified there. Everything else alc does works normally."
        );
    }
    #[cfg(unix)]
    {
        find_unix()
    }
}

#[cfg(unix)]
fn find_unix() -> Result<Found> {
    let binary = which::which("tmux").map_err(|_| {
        anyhow::anyhow!(
            "`--tmux` needs tmux on PATH and there is none. Install it \
             (`brew install tmux`, `apt install tmux`, `dnf install tmux`) or drop the flag - \
             sharing works without it, but this terminal and the browser then share one size."
        )
    })?;
    let version = read_version(&binary)?;
    if version < MIN_VERSION {
        bail!(
            "tmux {}.{} is on your PATH but `--tmux` needs {}.{} or newer, for the client flags \
             that let this terminal and the browser hold different sizes. Upgrade tmux, or drop \
             the flag and share at one size.",
            version.0,
            version.1,
            MIN_VERSION.0,
            MIN_VERSION.1
        );
    }
    Ok(Found { binary, version })
}

/// Parses `tmux -V`, which prints `tmux 3.4` or `tmux 3.7b`.
///
///
/// The trailing letter is a patch release and is dropped: 3.1c is 3.1 for
/// every purpose alc has. An unparseable line is an error rather than an
/// optimistic pass, because the alternative is failing later with tmux's own
/// message about an option this build has never heard of.
#[cfg(unix)]
fn read_version(binary: &Path) -> Result<(u32, u32)> {
    let output = Command::new(binary)
        .arg("-V")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to run {} -V", binary.display()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    parse_version(&text).with_context(|| {
        format!(
            "could not read a version out of `{} -V` ({})",
            binary.display(),
            text.trim()
        )
    })
}

#[cfg(unix)]
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let digits = text
        .trim()
        .trim_start_matches(|character: char| !character.is_ascii_digit());
    let (major, rest) = digits.split_once('.')?;
    let minor: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((major.parse().ok()?, minor.parse().ok()?))
}

impl Tmux {
    /// Names the server and session for one alc session.
    ///
    /// The id is already unguessable and unique per launch (`id::generate`),
    /// so nothing else needs to go into the name - and nothing else may,
    /// because a name derived from anything reusable is what makes a second
    /// launch attach to the first one's agent.
    pub(crate) fn for_session(id: &str) -> Result<Self> {
        let label = format!("alc-{}", id.to_ascii_lowercase());
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            bail!("'{id}' cannot name a tmux socket; this is an alc bug - please report it");
        }
        Ok(Self {
            label,
            // Both filled by `create`. Until the server exists there is no
            // path to read, no pane to name, and no command to address.
            path: String::new(),
            session: "alc".to_owned(),
            pane: String::new(),
        })
    }

    /// Starts the server and the detached session the agent runs in.
    ///
    /// The order inside matters and is not obvious:
    ///
    /// 1. `new-session` runs first and starts the server, which inherits
    ///    `spec.env` from this command and gives it to the pane. That is the
    ///    only step that may carry a credential.
    /// 2. The options are set afterwards, on a server that already exists.
    ///    `window-size` is required - it is the feature - and the rest are
    ///    best-effort, so a tmux that has never heard of one of them still
    ///    gets a working session rather than a refusal over a nicety.
    /// 3. The credential names are then dropped from the server's *global*
    ///    environment. The pane already holds the real values, so the agent
    ///    is unaffected; what goes away is `tmux show-environment` serving
    ///    them to anything else that can reach the socket - the pane
    ///    included.
    pub(crate) fn create(
        &mut self,
        binary: &Path,
        program: &Path,
        spec: &LaunchSpec,
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        let mut command = Command::new(binary);
        // No configuration file, at all. See the module note: a config is a
        // list of commands tmux runs as the user, and the agent can write
        // one.
        command.args(["-f", "/dev/null"]);
        command.args(["-L", &self.label]);
        // Every option that has to be in force BEFORE the agent starts, in
        // the same invocation as `new-session` and ahead of it.
        //
        // Not a style choice. tmux applies a command sequence inside the
        // server it is starting, in order, so this is the only way to have
        // the options set before the pane exists. Setting them afterwards
        // looks like it works right up until the agent dies immediately -
        // `remain-on-exit` is what keeps a dead pane around, so without it
        // already on, a launch that fails on its first line takes the whole
        // server down before alc can set anything or read what happened.
        // Measured: that turns a failing launch into `no server running`.
        //
        // A sequence stops at the first command tmux refuses, so every
        // option here must exist at MIN_VERSION - they all predate it - and
        // anything newer or merely nice belongs in the best-effort pass
        // below.
        for option in [
            // The feature. Without it tmux sizes the window to whichever
            // client was most recently active, which is the pty's single
            // size all over again, wearing a multiplexer.
            &["-gw", "window-size", "smallest"][..],
            // How the agent's own exit status is read back, and how the
            // output of an agent that dies on its first line survives long
            // enough to be seen. See `Snapshot::exit`.
            &["-gw", "remain-on-exit", "on"][..],
            // One agent, no window list: the bar would only steal a row.
            &["-g", "status", "off"][..],
            // Stops a client that attaches later from copying its own
            // shell's environment into a session alc has already given the
            // environment the launch resolved.
            &["-g", "update-environment", ""][..],
        ] {
            command.arg("set-option").args(option).arg(";");
        }
        command.args(["new-session", "-d", "-s", &self.session]);
        command.arg("-c").arg(cwd);
        command.args(["-x", &cols.max(20).to_string()]);
        command.args(["-y", &rows.max(4).to_string()]);
        command.arg("--");
        // The pane command, as a real argv rather than a shell string: tmux
        // treats a single element as something to hand to `sh -c` and word
        // split, so an agent path with a space in it would silently never
        // run. Several elements are passed through verbatim, which is what
        // the agent's own quoted arguments need.
        command.arg("/bin/sh").arg("-c").arg(PANE_WRAPPER);
        command.arg(program);
        command.args(&spec.args);

        // The launch's environment, which the server inherits from this
        // command and hands to the pane. This is the whole reason alc starts
        // a private server per session: it is how the provider key reaches
        // the agent without ever appearing in an argv, where `ps` - and on
        // Linux, another account's `ps` - would read it.
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        for name in &spec.env_remove {
            command.env_remove(name);
        }
        // Same reasoning as `PtyHost::spawn`: an agent asks the terminal
        // what it can do before it draws, and one that was not told falls
        // back to a rendering that looks broken next to the same agent run
        // directly.
        if !spec.env.contains_key(OsStr::new("TERM")) {
            command.env("TERM", "xterm-256color");
        }
        if !spec.env.contains_key(OsStr::new("COLORTERM")) {
            command.env("COLORTERM", "truecolor");
        }
        // alc's own server, not the one this shell may already be inside.
        command.env_remove("TMUX");
        command.env_remove("TMUX_PANE");

        let created = command
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("failed to run {}", binary.display()))?;
        if !created.status.success() {
            bail!(
                "tmux could not start the session: {}",
                String::from_utf8_lossy(&created.stderr).trim()
            );
        }

        // Read back before anything else, so every command from here on -
        // and every client that reads this off the session card - addresses
        // the server by a path rather than by re-deriving the label, and the
        // agent's own pane rather than whichever window is current.
        let resolved = self.run(
            binary,
            &[
                "display-message",
                "-p",
                "-t",
                &self.session,
                "#{socket_path}|#{pane_id}",
            ],
        )?;
        let answer = String::from_utf8_lossy(&resolved.stdout).trim().to_owned();
        let Some((path, pane)) = answer.split_once('|').filter(|(path, pane)| {
            resolved.status.success() && !path.is_empty() && !pane.is_empty()
        }) else {
            bail!("tmux started a server but would not say where its socket and pane are");
        };
        self.path = path.to_owned();
        self.pane = pane.to_owned();

        // Best-effort, and deliberately after the session exists: these
        // change how a client renders rather than how the agent runs, and a
        // tmux that has never heard of one of them should still get a
        // working session rather than a refusal over a nicety.
        for option in [
            &["-ga", "terminal-features", ",*:RGB"][..],
            &["-sg", "escape-time", "10"][..],
        ] {
            self.set_option(binary, option);
        }

        // The pane already holds the real values, so the agent is
        // unaffected. What goes away is `tmux show-environment` serving them
        // to anything else that can reach this socket - the pane included.
        for name in &spec.secret_env {
            let Some(name) = name.to_str() else { continue };
            let _ = self.run(binary, &["set-environment", "-gu", name]);
        }
        // Then checked, rather than assumed. `set-environment -gu` is given
        // a name a provider profile chose (`api_key_env`), and tmux refuses
        // a name it does not consider an environment name - one containing
        // `=`, for instance - which leaves the key sitting on the socket
        // while every command above reported success. A name that cannot be
        // rendered as UTF-8 is skipped above for the same reason. So the
        // property itself is tested, once, against the values rather than
        // the names.
        self.refuse_if_still_serving(binary, &spec.secret_values)?;
        Ok(())
    }

    /// Fails the launch if the server would still hand a credential to
    /// anything that can reach its socket.
    ///
    /// Loud rather than best-effort: the whole reason alc runs a private
    /// server and takes the names back out of it is that `show-environment`
    /// is readable by the pane, and a session that quietly did not manage it
    /// is one where the agent can read the key alc was trying not to give
    /// it. The caller stops the server.
    fn refuse_if_still_serving(&self, binary: &Path, secrets: &[String]) -> Result<()> {
        if secrets.is_empty() {
            return Ok(());
        }
        let Ok(output) = self.run(binary, &["show-environment", "-g"]) else {
            return Ok(());
        };
        let environment = String::from_utf8_lossy(&output.stdout);
        for secret in secrets {
            if environment.contains(secret.as_str()) {
                bail!(
                    "tmux would keep serving this launch's credentials to anything on its \
                     socket, so the session was not started; this usually means a provider \
                     profile's `api_key_env` is not a name tmux accepts"
                );
            }
        }
        Ok(())
    }

    /// Applies one option, best-effort.
    ///
    /// Every option that a session actually depends on is set in the same
    /// invocation as `new-session` and would have failed the launch there;
    /// what is left is the rendering niceties, where a tmux that has never
    /// heard of one should still get a working session.
    fn set_option(&self, binary: &Path, option: &[&str]) {
        let mut args = vec!["set-option"];
        args.extend_from_slice(option);
        let _ = self.run(binary, &args);
    }

    fn run(&self, binary: &Path, args: &[&str]) -> Result<std::process::Output> {
        Command::new(binary)
            .args(self.address())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("failed to run {}", binary.display()))
    }

    /// How to name this server on a tmux command line: the resolved socket
    /// path once there is one, and the label it was created under until
    /// then.
    fn address(&self) -> [&str; 2] {
        if self.path.is_empty() {
            ["-L", &self.label]
        } else {
            ["-S", &self.path]
        }
    }

    /// Everything alc asks tmux about a session, read in one call.
    ///
    /// One `display-message` rather than one per field because this is
    /// polled: a watcher asking five questions separately would run five
    /// tmux clients twice a second, for a session that is doing nothing.
    pub(crate) fn probe(&self, binary: &Path) -> Option<Snapshot> {
        let output = self
            .run(
                binary,
                &[
                    "display-message",
                    "-p",
                    "-t",
                    &self.pane,
                    "#{pane_dead}|#{pane_dead_status}|#{pane_dead_signal}|#{window_width}|#{window_height}|#{pane_pid}",
                ],
            )
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Snapshot::parse(&String::from_utf8_lossy(&output.stdout))
    }

    /// Delivers a viewer's keystrokes to the agent.
    ///
    /// `send-keys -H` and not a write to the mirror's terminal, because the
    /// mirror is a tmux client: bytes written there are parsed as keys, and
    /// a viewer who typed the prefix would land in tmux's command prompt,
    /// where `:run-shell` is a shell alc's permission ceiling never sees.
    /// `-H` takes byte values and hands them to the pane, so the prefix is
    /// just a byte again - measured, sending `02 3a` (ctrl-b, colon) opens
    /// no command prompt and creates no window.
    ///
    /// Bytes go in chunks because they become one argument each, and a
    /// pasted prompt would otherwise be an argument list long enough to
    /// refuse.
    pub(crate) fn send(&self, binary: &Path, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(SEND_CHUNK) {
            let hex: Vec<String> = chunk.iter().map(|byte| format!("{byte:02x}")).collect();
            let mut command = Command::new(binary);
            command
                .args(self.address())
                .args(["send-keys", "-t", &self.pane, "-H"])
                .args(&hex)
                .stdin(Stdio::null());
            let sent = command
                .output()
                .with_context(|| format!("failed to run {}", binary.display()))?;
            if !sent.status.success() {
                bail!(
                    "tmux would not deliver input to the agent: {}",
                    String::from_utf8_lossy(&sent.stderr).trim()
                );
            }
        }
        Ok(())
    }

    /// The argv a terminal runs to become a client of this session.
    ///
    /// `ignore-size` is only for the mirror, and it is the decision the
    /// whole feature turns on. A tmux client that votes on the window size
    /// makes the window the smaller of the two viewers, which leaves the
    /// larger one - the browser, usually - rendering a window narrower than
    /// itself, and tmux then re-emits the pane row by row into that client's
    /// stream. Two things break at once when it does: the secret scrubber
    /// matches contiguous bytes, so a credential re-emitted across a row
    /// boundary reaches the page unmasked; and `permission::probe` reads the
    /// bottom rows of a grid whose bottom rows are now tmux's padding.
    ///
    /// With the mirror abstaining, the window is exactly the local
    /// terminal's size, the hub follows it (`Session::follow_tmux_window`),
    /// and the page draws that size. The local terminal drives; the browser
    /// fits to it.
    pub(crate) fn attach_argv(&self, sizing: Sizing) -> Vec<OsString> {
        let mut argv: Vec<OsString> = self.address().iter().map(OsString::from).collect();
        argv.push(OsString::from("attach-session"));
        if sizing == Sizing::Abstain {
            argv.push(OsString::from("-f"));
            argv.push(OsString::from("ignore-size"));
        }
        argv.push(OsString::from("-t"));
        argv.push(OsString::from(&self.session));
        argv
    }

    /// Asks the agent to stop, the way a hangup on its terminal would.
    ///
    /// A signal to the agent's own process, not a tmux command, and that is
    /// the only thing that works here. `kill-pane` destroys the pane, which
    /// takes `remain-on-exit`'s record of how the agent died with it - so
    /// the card would shrug at a session the user deliberately stopped,
    /// where before tmux it said "signal HUP". Signalling leaves the pane to
    /// die on its own terms and the watcher to read the answer.
    ///
    /// `Session::kill` escalates to `stop` for an agent that ignores it.
    #[cfg(unix)]
    pub(crate) fn hangup(&self, pid: u32) -> Result<()> {
        let pid = i32::try_from(pid).context("the agent's process id does not fit a pid")?;
        // SAFETY: a plain `kill(2)`. The pid is one tmux reported for this
        // session's own pane, and SIGHUP is what a terminal closing sends -
        // the same signal `PtyHost::kill` delivers to an unwrapped agent.
        if unsafe { libc::kill(pid, libc::SIGHUP) } != 0 {
            return Err(std::io::Error::last_os_error()).context("failed to stop the agent");
        }
        Ok(())
    }

    /// Stops everything alc started to host this session.
    ///
    /// `kill-server`, not `kill-session`: alc owns this socket alone, so
    /// there is nothing else on it to spare, and a server left running would
    /// be a daemon holding the session's environment with nothing to serve.
    pub(crate) fn stop(&self, binary: &Path) -> Result<()> {
        self.run(binary, &["kill-server"]).map(|_| ())
    }
}

/// Against a real tmux, when there is one.
///
/// Skipped rather than failed where tmux is missing: CI's runner images do
/// not ship it, and a suite that goes red on a machine without an optional
/// dependency teaches people to ignore it. What these cover cannot be
/// covered any other way - each one is a claim about tmux's behaviour that
/// was measured rather than read, and that the parsing tests above assume.
#[cfg(all(test, unix))]
mod live {
    use super::*;
    use crate::launch::LaunchSpec;
    use std::ffi::OsString;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// A tmux new enough for what alc asks of it, or `None`.
    fn tmux() -> Option<Found> {
        find().ok()
    }

    fn spec(args: &[&str]) -> LaunchSpec {
        let mut spec = LaunchSpec::for_test();
        spec.args = args.iter().map(OsString::from).collect();
        spec
    }

    /// A `Tmux` whose server is running, plus a guard that stops it.
    struct Live {
        tmux: Tmux,
        binary: PathBuf,
    }

    impl Drop for Live {
        fn drop(&mut self) {
            let _ = self.tmux.stop(&self.binary);
        }
    }

    fn start(found: &Found, id: &str, args: &[&str], cols: u16, rows: u16) -> Live {
        let mut tmux = Tmux::for_session(id).unwrap();
        let temp = tempfile::tempdir().unwrap();
        tmux.create(
            &found.binary,
            Path::new("/bin/sh"),
            &spec(args),
            temp.path(),
            cols,
            rows,
        )
        .unwrap();
        Live {
            tmux,
            binary: found.binary.clone(),
        }
    }

    /// Polls until the agent is reported dead, or gives up.
    fn wait_for_exit(live: &Live) -> Option<ExitInfo> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(exit) = live.tmux.probe(&live.binary).and_then(|snap| snap.exit) {
                return Some(exit);
            }
            sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn a_session_starts_at_the_size_it_was_asked_for_and_names_its_socket() {
        let Some(found) = tmux() else { return };
        let live = start(&found, "claude-LIVESIZE01", &["-c", "sleep 30"], 96, 28);
        assert!(
            live.tmux.path.ends_with("alc-claude-livesize01"),
            "{}",
            live.tmux.path
        );
        let snapshot = live.tmux.probe(&found.binary).unwrap();
        assert_eq!(snapshot.window, Some((96, 28)));
        assert!(snapshot.exit.is_none());
        assert!(snapshot.pane_pid.is_some());
    }

    /// The reason `remain-on-exit` is on at all: a `tmux attach-session`
    /// client exits 0 whatever the agent did, so without this a failing
    /// launch would be reported on the session card as a clean finish.
    #[test]
    fn a_failing_agent_is_reported_with_its_own_status() {
        let Some(found) = tmux() else { return };
        let live = start(&found, "claude-LIVEEXIT01", &["-c", "exit 42"], 80, 24);
        assert_eq!(wait_for_exit(&live).and_then(|exit| exit.code), Some(42));
    }

    #[test]
    fn a_signalled_agent_is_not_reported_as_a_failing_one() {
        let Some(found) = tmux() else { return };
        let live = start(
            &found,
            "claude-LIVESIGNAL",
            &["-c", "kill -TERM $$"],
            80,
            24,
        );
        let exit = wait_for_exit(&live).unwrap();
        assert_eq!(exit.code, None);
        assert!(exit.signal.is_some(), "{exit:?}");
    }

    /// `alc kill` has to leave an answer behind. Destroying the pane would
    /// stop the agent just as well and take `remain-on-exit`'s record of how
    /// it died with it, so the card would shrug at a session the user
    /// deliberately stopped.
    #[test]
    fn asking_the_agent_to_stop_leaves_the_reason_it_stopped() {
        let Some(found) = tmux() else { return };
        let live = start(&found, "claude-LIVEHANGUP", &["-c", "sleep 60"], 80, 24);
        let pid = live.tmux.probe(&found.binary).unwrap().pane_pid.unwrap();

        live.tmux.hangup(pid).unwrap();
        let exit = wait_for_exit(&live).expect("the agent never stopped");
        assert_eq!(exit.code, None);
        // Named whichever way this tmux reports it; see `signal_name`.
        assert_eq!(exit.signal.as_deref(), Some("hup"), "{exit:?}");
    }

    /// Signalling the pty's child only detaches the mirror, so `alc kill`,
    /// `alc hub stop --drain` and the page's stop button all have to reach
    /// the agent through tmux instead. This is that path.
    #[test]
    fn stopping_the_server_stops_the_agent_rather_than_a_client() {
        let Some(found) = tmux() else { return };
        let live = start(&found, "claude-LIVEKILL01", &["-c", "sleep 60"], 80, 24);
        let pid = live.tmux.probe(&found.binary).unwrap().pane_pid.unwrap();
        // SAFETY: signal 0 tests for the process's existence and delivers
        // nothing. The pid is one tmux just reported for its own pane.
        let alive = || unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        assert!(alive(), "the agent never started");

        live.tmux.stop(&found.binary).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive() && Instant::now() < deadline {
            sleep(Duration::from_millis(50));
        }
        assert!(
            !alive(),
            "the agent outlived the tmux server that hosted it"
        );
    }

    /// The pane must not be handed the address of alc's own tmux server: a
    /// `tmux` command the agent runs would otherwise target the session it
    /// is itself running in, `send-keys` included.
    #[test]
    fn the_agent_is_not_given_the_address_of_its_own_tmux_server() {
        let Some(found) = tmux() else { return };
        let temp = tempfile::tempdir().unwrap();
        let probe = temp.path().join("tmux-env");
        let live = start(
            &found,
            "claude-LIVEENV001",
            &[
                "-c",
                &format!(
                    "printf '[%s][%s]' \"$TMUX\" \"$TMUX_PANE\" > {}",
                    probe.display()
                ),
            ],
            80,
            24,
        );
        wait_for_exit(&live);
        assert_eq!(std::fs::read_to_string(&probe).unwrap(), "[][]");
    }

    /// A provider profile names the environment variable its key travels
    /// in, and tmux refuses to unset a name it does not consider one - which
    /// left the key readable through `show-environment` while every command
    /// alc ran reported success. The launch is refused instead.
    #[test]
    fn a_credential_tmux_will_not_take_back_refuses_the_launch() {
        let Some(found) = tmux() else { return };
        let temp = tempfile::tempdir().unwrap();
        let secret = "sk-test-only-Zz9Qm2Rt9Bv4Nz1Cw8Ky5Hf3Jd6Ps0Ga";
        let mut spec = spec(&["-c", "sleep 5"]);
        spec.set_secret_env("MY=KEY", secret);

        let mut tmux = Tmux::for_session("claude-LIVEBADNAM").unwrap();
        let refused = tmux.create(
            &found.binary,
            Path::new("/bin/sh"),
            &spec,
            temp.path(),
            80,
            24,
        );
        // Whatever happened, the server does not outlive the attempt.
        let live = Live {
            tmux,
            binary: found.binary.clone(),
        };
        let error = refused.expect_err("a key tmux will not unset must refuse the launch");
        assert!(
            format!("{error:#}").contains("credentials"),
            "unexpected error: {error:#}"
        );
        drop(live);
    }

    /// A credential reaches the agent through the server's environment
    /// rather than through an argv, and is then taken back out of the
    /// server's own environment so nothing else on the socket can read it.
    #[test]
    fn a_credential_reaches_the_agent_without_staying_readable_on_the_socket() {
        let Some(found) = tmux() else { return };
        let temp = tempfile::tempdir().unwrap();
        let probe = temp.path().join("key");
        let secret = "sk-test-only-Xq7Lm2Rt9Bv4Nz1Cw8Ky5Hf3Jd6Ps0Ga";
        let mut spec = spec(&[
            "-c",
            &format!("printf %s \"$ALC_TEST_KEY\" > {}", probe.display()),
        ]);
        spec.set_secret_env("ALC_TEST_KEY", secret);

        let mut tmux = Tmux::for_session("claude-LIVEKEY001").unwrap();
        tmux.create(
            &found.binary,
            Path::new("/bin/sh"),
            &spec,
            temp.path(),
            80,
            24,
        )
        .unwrap();
        let live = Live {
            tmux,
            binary: found.binary.clone(),
        };
        wait_for_exit(&live);

        assert_eq!(std::fs::read_to_string(&probe).unwrap(), secret);
        let environment = live
            .tmux
            .run(&found.binary, &["show-environment", "-g"])
            .unwrap();
        let environment = String::from_utf8_lossy(&environment.stdout);
        assert!(
            !environment.contains(secret),
            "the server is still serving the key to anything on its socket"
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_version_line_parses_with_or_without_a_patch_letter() {
        assert_eq!(parse_version("tmux 3.4\n"), Some((3, 4)));
        assert_eq!(parse_version("tmux 3.7b\n"), Some((3, 7)));
        assert_eq!(parse_version("tmux next-3.6"), Some((3, 6)));
        assert_eq!(parse_version("tmux 2.9a"), Some((2, 9)));
    }

    #[test]
    fn a_line_with_no_version_is_refused_rather_than_assumed_new_enough() {
        // Guessing here would trade a clear message at launch for tmux's own
        // complaint about an option it has never heard of, mid-session.
        assert_eq!(parse_version("tmux"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn the_floor_is_the_version_that_added_the_ignore_size_client_flag() {
        assert_eq!(MIN_VERSION, (3, 2));
    }

    #[test]
    fn a_socket_label_can_never_carry_a_path_separator() {
        // A label is pasted into `$TMUX_TMPDIR/tmux-<uid>/<label>`, so one
        // with a separator in it puts the socket somewhere else entirely -
        // at a guessable path in world-writable /tmp.
        let tmux = Tmux::for_session("claude-ABCDEFGHJK").unwrap();
        assert_eq!(tmux.label, "alc-claude-abcdefghjk");
        assert!(!tmux.label.contains('/'));
        assert!(Tmux::for_session("../../escaped").is_err());
        assert!(Tmux::for_session("a b").is_err());
    }

    fn argv(tmux: &Tmux, sizing: Sizing) -> Vec<String> {
        tmux.attach_argv(sizing)
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_client_addresses_the_socket_by_path_once_there_is_one() {
        // The hub and the shell attaching to it need not agree on
        // TMUX_TMPDIR, so a re-derived label would look in the wrong place.
        let mut tmux = Tmux::for_session("codex-0123456789").unwrap();
        assert_eq!(
            argv(&tmux, Sizing::Drive)[..2],
            ["-L", "alc-codex-0123456789"]
        );
        tmux.path = "/tmp/tmux-501/alc-codex-0123456789".to_owned();
        assert_eq!(
            argv(&tmux, Sizing::Drive)[..2],
            ["-S", "/tmp/tmux-501/alc-codex-0123456789"]
        );
    }

    #[test]
    fn only_the_mirror_abstains_from_sizing_the_window() {
        // The one flag the whole feature turns on: a mirror that voted would
        // shrink the window below its own width, and tmux would then re-emit
        // the pane row by row - past the secret scrubber, which matches
        // contiguous bytes.
        let tmux = Tmux::for_session("claude-ABCDEFGHJK").unwrap();
        assert!(argv(&tmux, Sizing::Abstain).contains(&"ignore-size".to_owned()));
        assert!(!argv(&tmux, Sizing::Drive).contains(&"ignore-size".to_owned()));
        assert_eq!(
            argv(&tmux, Sizing::Drive),
            vec!["-L", "alc-claude-abcdefghjk", "attach-session", "-t", "alc"]
        );
    }

    /// The pane wrapper is the only place alc's own tmux address is taken
    /// away from the agent, and it has to keep `exec` so the pane's process
    /// stays the agent itself.
    #[test]
    fn the_pane_wrapper_unsets_the_socket_address_and_execs() {
        assert!(PANE_WRAPPER.contains("unset TMUX TMUX_PANE"));
        assert!(PANE_WRAPPER.contains("exec \"$0\" \"$@\""));
    }

    #[test]
    fn a_live_session_reports_its_window_and_no_exit() {
        let snapshot = Snapshot::parse("0|||80|24|4242").unwrap();
        assert!(snapshot.exit.is_none());
        assert_eq!(snapshot.window, Some((80, 24)));
        assert_eq!(snapshot.pane_pid, Some(4242));
    }

    #[test]
    fn a_failing_agent_keeps_its_own_exit_code() {
        // The whole reason `remain-on-exit` is on: the tmux client exits 0
        // whatever the agent did, so without this the card would call a
        // failed launch a clean finish.
        let snapshot = Snapshot::parse("1|42||80|24|4242").unwrap();
        assert_eq!(snapshot.exit.unwrap().code, Some(42));
    }

    #[test]
    fn a_signalled_agent_is_reported_as_signalled_not_as_a_status() {
        let snapshot = Snapshot::parse("1|1|TERM|80|24|4242").unwrap();
        let exit = snapshot.exit.unwrap();
        assert_eq!(exit.code, None);
        assert_eq!(exit.signal.as_deref(), Some("term"));
    }

    #[test]
    fn a_numbered_signal_reads_the_same_as_a_named_one() {
        // tmux names a signal where the platform can and numbers it where it
        // cannot, so without this the same stopped agent reads as `hup` on
        // one machine and `1` on another.
        assert_eq!(signal_name("1"), "hup");
        assert_eq!(signal_name("HUP"), "hup");
        assert_eq!(signal_name("15"), "term");
        // Not in the table, and not worth guessing at.
        assert_eq!(signal_name("31"), "31");
        assert_eq!(signal_name("WINCH"), "winch");
    }

    #[test]
    fn a_reply_that_is_not_the_expected_shape_is_refused() {
        // tmux answers a dead session on stderr with an empty stdout, and a
        // partial line parsed optimistically would report a running agent as
        // exited.
        assert!(Snapshot::parse("").is_none());
        assert!(Snapshot::parse("0|||80|24").is_none());
        // Six empty fields is the shape a reply about a pane that is not
        // there would take, and reading it as "alive" would keep the watcher
        // polling something that has gone.
        assert!(Snapshot::parse("|||||").is_none());
        assert!(Snapshot::parse("x|||80|24|1").is_none());
    }
}
