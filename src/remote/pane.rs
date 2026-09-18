//! The pane launcher: how a `--tmux` session's agent is started on Windows.
//!
//! On unix the tmux pane runs the agent directly, under a one-line shell that
//! takes tmux's socket address back out of its environment first
//! (`tmux::PANE_WRAPPER`). None of that carries over to tmux for Windows,
//! whose behaviour was measured rather than assumed:
//!
//! * There is no `/bin/sh`, so there is nothing to run that line in, and the
//!   pane is handed `TMUX` and `TMUX_PANE` - the address of alc's own server,
//!   which any `tmux` command the agent ran would then target.
//! * A pane's command line, environment and working directory pass through
//!   the ANSI code page: a non-ASCII argument arrives mangled, a non-ASCII
//!   environment value arrives mangled, a non-ASCII working directory is
//!   silently replaced by the home directory, and a program under a non-ASCII
//!   path does not start at all. Every Windows user whose name is not plain
//!   ASCII has a profile path that is not either.
//! * The arguments are joined with spaces and split again, and a bare `;`
//!   among them is taken as tmux's own command separator.
//!
//! So the pane runs alc instead, with nothing on its command line but a
//! loopback port and a one-time token - all ASCII. That process, the
//! launcher, asks the hub for the launch the token stands for, receives it as
//! UTF-8 JSON, and starts the agent itself: the exact arguments, the exact
//! environment minus tmux's address, the exact working directory. It then
//! waits, and exits with the agent's own status, which is what tmux records
//! for the pane and what the session card reports.
//!
//! A side effect worth having: the provider key never enters tmux at all on
//! Windows. The server is started without the launch's environment, so there
//! is nothing for `show-environment` to serve and nothing to take back out.

use std::io::{BufRead, BufReader, Write};
#[cfg(any(windows, test))]
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

// Only the hub on Windows builds a launch; on unix the pane runs the agent
// itself and this module only answers - and refuses - a stray token.
#[cfg(any(windows, test))]
use crate::launch::LaunchSpec;

/// The hidden subcommand a pane runs. Named here so the command line tmux is
/// given and the one alc's parser accepts cannot drift apart.
pub(crate) const SUBCOMMAND: &str = "__tmux-pane";

/// How long a launch waits to be collected before it is dropped.
///
/// tmux starts the pane within a second of being asked; this only has to
/// outlast a slow machine, and a launch nobody collects - because tmux
/// failed to start the pane - should not sit in the hub holding a provider
/// key for longer than that.
const UNCLAIMED: Duration = Duration::from_secs(60);

/// How long the launcher waits for the hub to answer.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Everything the launcher needs to start the agent, exactly as the hub
/// resolved it.
///
/// Strings rather than `OsString`s because it crosses a socket as JSON, and
/// everything in it arrived at the hub as UTF-8 over the control channel in
/// the first place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaneLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    /// In the order it is applied. Later entries win, and on Windows names
    /// are compared without regard to case - `Command` does both, so the
    /// order is the only thing that has to be right here.
    pub env: Vec<(String, String)>,
    /// Applied after `env`.
    pub env_remove: Vec<String>,
}

impl PaneLaunch {
    /// The launch a session's agent would get from `PtyHost::spawn`, in a
    /// form that survives the trip.
    ///
    /// The layering is the same as an unwrapped session's: the hub's own
    /// environment underneath, then the terminal defaults, then everything
    /// the launch resolved - the client's environment and the provider's
    /// credentials - and finally what the launch removes. tmux's address is
    /// removed last of all, whatever anything above said.
    #[cfg(any(windows, test))]
    pub(crate) fn resolve(program: &Path, spec: &LaunchSpec, cwd: &Path) -> Result<Self> {
        let text = |value: &std::ffi::OsStr, what: &str| -> Result<String> {
            value.to_str().map(str::to_owned).with_context(|| {
                format!("{what} is not valid Unicode, so it cannot reach the pane")
            })
        };
        let mut env: Vec<(String, String)> = std::env::vars_os()
            // A variable of the hub's own that is not Unicode cannot have
            // been one the launch asked for; it is dropped rather than
            // failing a launch over the hub's environment.
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        for (name, value) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")] {
            if !spec.env.contains_key(std::ffi::OsStr::new(name)) {
                env.push((name.to_owned(), value.to_owned()));
            }
        }
        for (name, value) in &spec.env {
            env.push((
                text(name, "an environment variable name")?,
                text(value, "an environment variable")?,
            ));
        }
        let mut env_remove = spec
            .env_remove
            .iter()
            .map(|name| text(name, "an environment variable name"))
            .collect::<Result<Vec<_>>>()?;
        env_remove.extend(["TMUX".to_owned(), "TMUX_PANE".to_owned()]);
        Ok(Self {
            program: text(program.as_os_str(), "the agent's path")?,
            args: spec
                .args
                .iter()
                .map(|argument| text(argument, "an argument"))
                .collect::<Result<_>>()?,
            cwd: text(cwd.as_os_str(), "the working directory")?,
            env,
            env_remove,
        })
    }
}

/// Launches the hub is holding for panes that have not collected them yet.
///
/// A token is the only credential the launcher presents, so it is treated
/// like one: 32 random bytes, compared in constant time, good for a single
/// collection, and dropped unclaimed after [`UNCLAIMED`].
#[derive(Default)]
pub(crate) struct PendingLaunches {
    held: Mutex<Vec<(String, PaneLaunch, Instant)>>,
}

impl PendingLaunches {
    /// Holds `launch` and returns the token that collects it.
    #[cfg(any(windows, test))]
    pub(crate) fn hold(&self, launch: PaneLaunch) -> Result<String> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .context("failed to read operating-system randomness for a pane token")?;
        // Hex rather than the base64url the other tokens use: it goes on a
        // command line tmux re-splits and alc's own argument parser reads,
        // and base64url may start with `-`.
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut held = self
            .held
            .lock()
            .map_err(|_| anyhow::anyhow!("the hub's pane launches were poisoned"))?;
        held.retain(|(_, _, since)| since.elapsed() < UNCLAIMED);
        held.push((token.clone(), launch, Instant::now()));
        Ok(token)
    }

    /// Hands over the launch `token` stands for, once.
    pub(crate) fn take(&self, token: &str) -> Option<PaneLaunch> {
        let mut held = self.held.lock().ok()?;
        held.retain(|(_, _, since)| since.elapsed() < UNCLAIMED);
        // Every entry is compared, so how long this takes says nothing about
        // how close a guess came.
        let mut found = None;
        for (index, (candidate, _, _)) in held.iter().enumerate() {
            if crate::remote::request::constant_time_eq(candidate.as_bytes(), token.as_bytes()) {
                found = Some(index);
            }
        }
        found.map(|index| held.remove(index).1)
    }

    /// What the hub says to a launcher presenting `token`.
    pub(crate) fn answer(&self, token: &str) -> crate::remote::ctl::CtlReply {
        match self.take(token) {
            Some(launch) => crate::remote::ctl::CtlReply::PaneLaunch {
                launch: Box::new(launch),
            },
            // Said the same way whether the token never existed, was already
            // collected, or expired: the launcher cannot use the difference,
            // and a guesser should not get one.
            None => crate::remote::ctl::CtlReply::Error {
                message: "this pane's launch is not waiting at the hub".to_owned(),
            },
        }
    }

    /// Drops every launch that has waited longer than [`UNCLAIMED`].
    ///
    /// `hold` and `take` do this too, but only when they run; the hub also
    /// calls this on its own clock, so a launch nobody came for does not sit
    /// in memory holding a provider key until the next `--tmux` session.
    pub(crate) fn expire(&self) {
        if let Ok(mut held) = self.held.lock() {
            held.retain(|(_, _, since)| since.elapsed() < UNCLAIMED);
        }
    }

    /// Drops a launch whose pane will never come for it.
    pub(crate) fn forget(&self, token: &str) {
        let _ = self.take(token);
    }
}

/// The pane launcher's whole life: collect the launch, run the agent, exit
/// with its status.
///
/// Anything that goes wrong before the agent starts is printed to the pane,
/// which is the one place the person watching will see it, and ends the pane
/// with a failing status so the session card says the launch failed.
pub fn run(port: u16, token: &str) -> ! {
    let code = match launch(port, token) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("alc: could not start the agent in this pane: {error:#}");
            // Long enough to be read on the page before the watcher notices
            // the pane has died and ends the session.
            std::thread::sleep(Duration::from_secs(3));
            1
        }
    };
    std::process::exit(code)
}

fn launch(port: u16, token: &str) -> Result<i32> {
    let launch = fetch(port, token)?;

    #[cfg(windows)]
    crate::remote::win::ignore_console_interrupts();

    let mut command = std::process::Command::new(&launch.program);
    command
        .args(&launch.args)
        .current_dir(&launch.cwd)
        .env_clear();
    for (name, value) in &launch.env {
        command.env(name, value);
    }
    for name in &launch.env_remove {
        command.env_remove(name);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start {}", launch.program))?;

    // Held until this process exits: dropping it would end the agent.
    #[cfg(windows)]
    let _bound = crate::remote::win::bind_to_this_process(&child);

    let status = child.wait().context("lost track of the agent")?;
    // A process that ended without an exit code was signalled; report it the
    // way a shell would, as a failure.
    Ok(status.code().unwrap_or(1))
}

/// Collects a launch from the hub over its loopback control port.
fn fetch(port: u16, token: &str) -> Result<PaneLaunch> {
    let stream = std::net::TcpStream::connect(("127.0.0.1", port))
        .with_context(|| format!("no hub is listening on 127.0.0.1:{port}"))?;
    stream.set_read_timeout(Some(FETCH_TIMEOUT)).ok();
    let mut writer = stream
        .try_clone()
        .context("failed to open the hub connection")?;
    let mut line = serde_json::to_string(&serde_json::json!({ "pane": token }))?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;

    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .context("the hub closed the connection without answering")?;
    match serde_json::from_str(reply.trim()) {
        Ok(crate::remote::ctl::CtlReply::PaneLaunch { launch }) => Ok(*launch),
        Ok(crate::remote::ctl::CtlReply::Error { message }) => bail!("{message}"),
        _ => bail!("the hub answered with something alc cannot read"),
    }
}

/// Quotes one argument the way `CommandLineToArgvW` splits it.
///
/// tmux for Windows joins a pane's arguments with spaces and hands the result
/// to `CreateProcess`, so an argument containing a space would otherwise
/// arrive as several. Only the launcher's own command line goes through
/// this - its path may contain a space - and that is all ASCII; the agent's
/// arguments never reach tmux.
#[cfg(any(windows, test))]
pub(crate) fn quote_windows_arg(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return argument.to_owned();
    }
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                // Backslashes before a quote are escapes, so each one is
                // doubled, and the quote itself needs one more.
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            other => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(other);
                backslashes = 0;
            }
        }
    }
    // Backslashes before the closing quote would escape it.
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// Converts the launch's environment to the map `LaunchSpec` keeps, for
/// tests that compare the two.
#[cfg(test)]
fn env_map(
    launch: &PaneLaunch,
) -> std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> {
    launch
        .env
        .iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn a_token_collects_its_launch_once() {
        let pending = PendingLaunches::default();
        let launch = PaneLaunch {
            program: "agent".to_owned(),
            args: vec!["--flag".to_owned()],
            cwd: ".".to_owned(),
            env: Vec::new(),
            env_remove: Vec::new(),
        };
        let token = pending.hold(launch.clone()).unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(pending.take("0".repeat(64).as_str()).is_none());
        assert_eq!(pending.take(&token), Some(launch));
        // Single use: a second collection, or a replay of the command line
        // tmux still shows for the pane, gets nothing.
        assert!(pending.take(&token).is_none());
    }

    #[test]
    fn a_forgotten_launch_cannot_be_collected() {
        let pending = PendingLaunches::default();
        let token = pending
            .hold(PaneLaunch {
                program: "agent".to_owned(),
                args: Vec::new(),
                cwd: ".".to_owned(),
                env: Vec::new(),
                env_remove: Vec::new(),
            })
            .unwrap();
        pending.forget(&token);
        assert!(pending.take(&token).is_none());
    }

    #[test]
    fn the_launch_keeps_what_tmux_for_windows_would_have_mangled() {
        // Each of these was measured arriving wrong, or not at all, when
        // passed to a pane through tmux for Windows directly.
        let mut spec = LaunchSpec::for_test();
        let arguments = [
            "arg with space",
            "uni\u{e9}\u{4e2d}",
            ";",
            "in\"side",
            "",
            "100%PATH%",
        ];
        spec.args = arguments.iter().map(OsString::from).collect();
        spec.env.insert(
            OsString::from("ALC_TEST_VALUE"),
            OsString::from("val\u{e9}\u{4e2d}\u{6587}"),
        );
        let cwd = Path::new("C:\\Users\\\u{738b}\u{5c0f}\u{660e}\\repo");
        let launch = PaneLaunch::resolve(Path::new("C:\\bin\\claude.exe"), &spec, cwd).unwrap();
        assert_eq!(launch.args, arguments);
        assert_eq!(launch.cwd, cwd.to_str().unwrap());
        assert_eq!(
            env_map(&launch).get(std::ffi::OsStr::new("ALC_TEST_VALUE")),
            Some(&OsString::from("val\u{e9}\u{4e2d}\u{6587}"))
        );
    }

    #[test]
    fn the_launch_overrides_the_hubs_environment_and_strips_tmuxs_address() {
        let mut spec = LaunchSpec::for_test();
        spec.env
            .insert(OsString::from("TERM"), OsString::from("launch-term"));
        spec.env_remove.push(OsString::from("ALC_REMOVED"));
        let launch = PaneLaunch::resolve(Path::new("agent"), &spec, Path::new(".")).unwrap();
        // The launch's own value comes after anything the hub had, so it is
        // the one `Command` keeps.
        let last_term = launch
            .env
            .iter()
            .rev()
            .find(|(name, _)| name == "TERM")
            .map(|(_, value)| value.as_str());
        assert_eq!(last_term, Some("launch-term"));
        // COLORTERM was not set by the launch, so the terminal default is.
        assert!(
            launch
                .env
                .iter()
                .any(|(name, value)| name == "COLORTERM" && value == "truecolor")
        );
        for name in ["ALC_REMOVED", "TMUX", "TMUX_PANE"] {
            assert!(
                launch.env_remove.iter().any(|removed| removed == name),
                "{name}"
            );
        }
    }

    #[test]
    fn an_argument_is_quoted_the_way_windows_splits_it() {
        assert_eq!(quote_windows_arg("plain"), "plain");
        assert_eq!(quote_windows_arg(""), "\"\"");
        assert_eq!(
            quote_windows_arg("C:\\Program Files\\alc\\alc.exe"),
            "\"C:\\Program Files\\alc\\alc.exe\""
        );
        assert_eq!(quote_windows_arg("in\"side"), "\"in\\\"side\"");
        // A trailing backslash would escape the closing quote.
        assert_eq!(
            quote_windows_arg("dir with space\\"),
            "\"dir with space\\\\\""
        );
        // Backslashes not followed by a quote are literal.
        assert_eq!(quote_windows_arg("a\\b c"), "\"a\\b c\"");
        assert_eq!(quote_windows_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    }
}
