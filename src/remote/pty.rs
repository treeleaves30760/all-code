//! Running a coding agent under a pseudo-terminal.
//!
//! Everything alc launches is an interactive TUI that decides what to render
//! by asking whether it is talking to a terminal. Handing it a pipe would
//! turn off colour, spinners and the whole full-screen interface, so a
//! mirrored session gives it a real pty and mirrors the bytes instead.
//!
//! Two `portable-pty` behaviours are easy to get wrong and are handled here
//! rather than at each call site:
//!
//! * `CommandBuilder` resolves its working directory to the user's HOME when
//!   none is set, so a session started in a repo would silently run the agent
//!   somewhere else. `spawn` always sets one.
//! * `take_writer` may be called exactly once, and the slave handle must be
//!   dropped after the spawn or the reader never sees EOF when the agent
//!   exits - the session would hang instead of closing.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::launch::LaunchSpec;
use crate::remote::wire::ExitInfo;

/// What to run under the pty.
///
/// Separate from `LaunchSpec` because the two stopped being the same thing
/// when `--tmux` landed: an ordinary session runs the agent here, but a tmux
/// session runs a `tmux attach-session` client, and the agent - with the
/// launch's environment, credentials included - lives in a pane on the other
/// side of the tmux server. Handing the attach client the agent's
/// environment would put the provider key in a process that has no use for
/// it, so the two are built separately and this is what they have in common.
#[derive(Debug, Clone)]
pub(crate) struct PtyCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub env_remove: Vec<OsString>,
}

impl PtyCommand {
    /// The agent itself, run directly - what every session did before
    /// `--tmux`, and what one still does without it.
    pub(crate) fn agent(program: &Path, spec: &LaunchSpec) -> Self {
        Self {
            program: program.to_path_buf(),
            args: spec.args.clone(),
            env: spec.env.clone(),
            env_remove: spec.env_remove.clone(),
        }
    }
}

/// A live agent process and its pty master.
pub(crate) struct PtyHost {
    /// `MasterPty` is `Send` but not `Sync`, and a session is shared across
    /// the pump, the local terminal and every viewer's thread, so the
    /// handle is owned by a mutex rather than by whoever got there first.
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    process_id: Option<u32>,
}

impl PtyHost {
    /// Opens a pty, spawns `program` into it, and hands back the reader for
    /// the caller's own pump thread.
    ///
    /// `cwd` is required rather than optional: see the module note.
    pub(crate) fn spawn(
        command: &PtyCommand,
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<(Self, Box<dyn Read + Send>)> {
        let program = command.program.as_path();
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open a pseudo-terminal for the session")?;

        let mut builder = CommandBuilder::new(program);
        builder.args(&command.args);
        for (name, value) in &command.env {
            builder.env(name, value);
        }
        for name in &command.env_remove {
            builder.env_remove(name);
        }
        builder.cwd(cwd);
        // Agents ask the terminal what it can do before they draw. Without
        // these they fall back to a monochrome, seven-bit rendering that
        // looks broken next to the same agent run directly.
        if !command.env.contains_key(OsStr::new("TERM")) {
            builder.env("TERM", "xterm-256color");
        }
        if !command.env.contains_key(OsStr::new("COLORTERM")) {
            builder.env("COLORTERM", "truecolor");
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .with_context(|| format!("failed to launch {} under a pty", program.display()))?;
        // The parent's copy of the slave must go before the reader can ever
        // see EOF; holding it would keep the pty open past the agent's exit.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to read from the session's pseudo-terminal")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to write to the session's pseudo-terminal")?;

        let process_id = child.process_id();
        let killer = child.clone_killer();
        Ok((
            Self {
                master: Mutex::new(pair.master),
                writer: Mutex::new(writer),
                killer,
                child: Mutex::new(child),
                process_id,
            },
            reader,
        ))
    }

    /// Forwards bytes to the agent. Takes `&self` because input arrives from
    /// several threads at once - the local terminal and every operator with
    /// the page open.
    pub(crate) fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("the session's input channel was poisoned"))?;
        writer
            .write_all(bytes)
            .context("failed to send input to the agent")?;
        writer.flush().context("failed to flush input to the agent")
    }

    /// Tells the kernel the window changed, which is what raises SIGWINCH in
    /// the agent and makes it redraw at the new size.
    pub(crate) fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let master = self
            .master
            .lock()
            .map_err(|_| anyhow::anyhow!("the session's terminal handle was poisoned"))?;
        master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize the session's pseudo-terminal")
    }

    pub(crate) fn kill(&self) -> Result<()> {
        self.killer
            .clone_killer()
            .kill()
            .context("failed to stop the agent")
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    /// Blocks until the agent exits.
    ///
    /// Only safe to call once the reader has reached EOF. A pty has no flow
    /// control beyond its buffer: if anything in the agent's process group
    /// is still writing and nobody is draining the master, that writer
    /// blocks, the agent waits on it, and this waits on the agent. Measured,
    /// not theorised - `sh -c 'echo hi; tput cols'` reproduces it when the
    /// reader stops after `hi`. `Session::pump` is the only caller, and it
    /// calls this after its read loop ends.
    pub(crate) fn wait(&self) -> Result<ExitInfo> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| anyhow::anyhow!("the session's process handle was poisoned"))?;
        let status = child.wait().context("failed to wait for the agent")?;
        Ok(exit_info(&status))
    }
}

/// Splits a pty exit status into the two things a session card shows
/// separately. `portable_pty::ExitStatus` reports a signalled death as exit
/// code 1 plus a signal name, and a card that only read the code would call
/// an agent the user stopped a failing one.
fn exit_info(status: &portable_pty::ExitStatus) -> ExitInfo {
    match status.signal() {
        Some(signal) => ExitInfo {
            code: None,
            signal: Some(signal.to_owned()),
        },
        None => ExitInfo {
            code: i32::try_from(status.exit_code()).ok(),
            signal: None,
        },
    }
}

// Gated as a whole rather than per test: every one of these drives a real
// pty through `/bin/sh`, so on Windows the module's helpers would be dead
// code and `-D warnings` fails the build on them.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::io::BufRead;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    fn spec() -> LaunchSpec {
        let mut spec = crate::launch::LaunchSpec::for_test();
        spec.args.clear();
        spec
    }

    /// The tests drive `/bin/sh` rather than a real agent, so they build the
    /// pty command the way `Session::start` does for an unwrapped launch.
    fn shell(spec: &LaunchSpec) -> PtyCommand {
        PtyCommand::agent(Path::new("/bin/sh"), spec)
    }

    /// Reads until `needle` appears, or the reader ends.
    ///
    /// A read on a pty blocks until the agent writes something, and a deadline
    /// checked between reads never fires while one is blocked - so the
    /// caller arms `watchdog`, which kills the agent and turns what would
    /// have been a hung test into a failing one.
    fn read_until(reader: &mut dyn BufRead, needle: &str, label: &str) -> String {
        let mut seen = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    seen.push_str(&line);
                    if seen.contains(needle) {
                        return seen;
                    }
                }
                Err(_) => break,
            }
        }
        panic!("never saw {label} in:\n{seen}");
    }

    /// Stops the agent after `seconds`, so no test in this module can hang
    /// the suite waiting on a pty that will never produce another byte.
    fn watchdog(host: Arc<PtyHost>, seconds: u64) {
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(seconds));
            let _ = host.kill();
        });
    }

    #[test]
    fn the_agent_runs_in_the_requested_directory_and_sees_a_terminal() {
        // Both halves matter: portable-pty defaults cwd to HOME, and every
        // agent decides how to render by asking whether stdout is a tty.
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().canonicalize().unwrap();
        let mut spec = spec();
        spec.args = vec![
            OsString::from("-c"),
            OsString::from("pwd; test -t 1 && echo IS_TTY"),
        ];

        let (host, reader) = PtyHost::spawn(&shell(&spec), &dir, 120, 30).unwrap();
        let host = Arc::new(host);
        watchdog(Arc::clone(&host), 20);
        let mut reader = std::io::BufReader::new(reader);
        let seen = read_until(&mut reader, "IS_TTY", "the tty marker");

        assert!(seen.contains(dir.to_str().unwrap()), "{seen}");
        assert!(host.process_id().is_some());
        // Killed rather than waited for: this stopped reading mid-stream,
        // and `wait` is only safe once the reader has drained to EOF.
        let _ = host.kill();
    }

    #[test]
    fn the_agent_is_told_the_window_size_and_notices_it_change() {
        // Driven by input rather than by a WINCH trap: what matters is that
        // the kernel's idea of the window changed, and asking the shell to
        // report it on demand tests exactly that without depending on how a
        // particular /bin/sh handles a signal arriving during `wait`.
        let temp = tempfile::tempdir().unwrap();
        let mut spec = spec();
        spec.args = vec![
            OsString::from("-c"),
            OsString::from("while read line; do stty size; done"),
        ];

        let (host, reader) = PtyHost::spawn(&shell(&spec), temp.path(), 120, 30).unwrap();
        let host = Arc::new(host);
        watchdog(Arc::clone(&host), 20);
        let mut reader = std::io::BufReader::new(reader);

        host.write(b"\r").unwrap();
        read_until(&mut reader, "30 120", "the initial size");

        host.resize(90, 24).unwrap();
        host.write(b"\r").unwrap();
        read_until(&mut reader, "24 90", "the resized size");

        let _ = host.kill();
    }

    #[test]
    fn input_written_to_the_pty_reaches_the_agent() {
        let temp = tempfile::tempdir().unwrap();
        let mut spec = spec();
        spec.args = vec![
            OsString::from("-c"),
            OsString::from("read line; echo \"GOT:$line\""),
        ];

        let (host, reader) = PtyHost::spawn(&shell(&spec), temp.path(), 80, 24).unwrap();
        let host = Arc::new(host);
        watchdog(Arc::clone(&host), 20);
        let mut reader = std::io::BufReader::new(reader);
        host.write(b"ping\r").unwrap();

        let seen = read_until(&mut reader, "GOT:ping", "the echoed input");
        assert!(seen.contains("GOT:ping"), "{seen}");
        let _ = host.kill();
    }

    #[test]
    fn a_signalled_agent_is_reported_as_signalled_not_as_exit_one() {
        let temp = tempfile::tempdir().unwrap();
        let mut spec = spec();
        spec.args = vec![OsString::from("-c"), OsString::from("kill -TERM $$")];

        let (host, _reader) = PtyHost::spawn(&shell(&spec), temp.path(), 80, 24).unwrap();
        let host = Arc::new(host);
        watchdog(Arc::clone(&host), 20);
        let exit = host.wait().unwrap();

        assert_eq!(exit.code, None);
        assert!(exit.signal.is_some(), "{exit:?}");
    }

    #[test]
    fn the_environment_the_builder_asked_for_reaches_the_agent() {
        let temp = tempfile::tempdir().unwrap();
        let mut spec = spec();
        spec.env
            .insert(OsString::from("ALC_MARKER"), OsString::from("present"));
        spec.args = vec![
            OsString::from("-c"),
            OsString::from("echo \"MARKER=$ALC_MARKER TERM=$TERM\""),
        ];

        let (host, reader) = PtyHost::spawn(&shell(&spec), temp.path(), 80, 24).unwrap();
        let host = Arc::new(host);
        watchdog(Arc::clone(&host), 20);
        let mut reader = std::io::BufReader::new(reader);
        let seen = read_until(&mut reader, "MARKER=", "the marker");

        assert!(seen.contains("MARKER=present"), "{seen}");
        assert!(seen.contains("TERM=xterm-256color"), "{seen}");
        let _ = host.kill();
    }
}
