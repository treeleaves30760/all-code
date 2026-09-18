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
use std::sync::{Arc, Mutex};

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
    ///
    /// `None` once a Windows session's agent has exited: dropping it is what
    /// closes the pseudo-console (see `close_when_the_child_exits`).
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    /// Shared with the reader on Windows, which answers conhost's opening
    /// question through it; see `CursorHandshake`.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
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
        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .context("failed to write to the session's pseudo-terminal")?,
        ));
        #[cfg(windows)]
        let reader: Box<dyn Read + Send> =
            Box::new(CursorHandshake::new(reader, Arc::clone(&writer)));

        let process_id = child.process_id();
        let killer = child.clone_killer();
        let master = Arc::new(Mutex::new(Some(pair.master)));
        let child = Arc::new(Mutex::new(child));
        #[cfg(windows)]
        close_when_the_child_exits(Arc::clone(&child), Arc::clone(&master));
        Ok((
            Self {
                master,
                writer,
                killer,
                child,
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
        let Some(master) = master.as_ref() else {
            anyhow::bail!("the session's terminal has closed");
        };
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

/// How often a Windows session checks whether its agent has exited.
#[cfg(windows)]
const EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// Closes a Windows pseudo-console once the process in it has exited, so the
/// session's reader sees the end of the output.
///
/// A unix pty reports end of file when the last process holding it goes. A
/// ConPTY does not: its output pipe stays open until the pseudo-console
/// itself is closed, and portable-pty closes it only when the master is
/// dropped - which the session holds for as long as it runs. So an agent
/// that exited left its session reading for ever - measured: `Running` on
/// the card, the terminal still attached, a `--tmux` mirror's pump stuck
/// after its tmux client had gone. Dropping the master here closes it;
/// conhost then sends whatever it had not yet drawn and closes the pipe, and
/// `Session::pump` finishes the session the same way it does everywhere
/// else.
///
/// A poll rather than a blocking wait, so the child's lock is never held for
/// longer than a check.
#[cfg(windows)]
fn close_when_the_child_exits(
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
) {
    let watch = move || {
        loop {
            std::thread::sleep(EXIT_POLL);
            let exited = match child.lock() {
                Ok(mut child) => child.try_wait(),
                Err(_) => return,
            };
            match exited {
                Ok(None) => continue,
                Ok(Some(_)) => {
                    if let Ok(mut master) = master.lock() {
                        master.take();
                    }
                    return;
                }
                // Nothing more can be learned about this process; leave the
                // pseudo-console as it is rather than close it on a guess.
                Err(_) => return,
            }
        }
    };
    let _ = std::thread::Builder::new()
        .name("alc-conpty-exit".to_owned())
        .spawn(watch);
}

/// Answers the question a Windows pseudo-console asks before it will draw
/// anything, and keeps its opening negotiation away from every viewer.
///
/// portable-pty opens its ConPTY with `PSEUDOCONSOLE_INHERIT_CURSOR`, which
/// makes conhost start by asking the terminal on the other end where the
/// cursor is (`ESC [ 6 n`) and hold back all output until it hears. The hub
/// is not a terminal and nothing in it answered, so a session was stuck until
/// some viewer's own terminal happened to reply - measured: a `--tmux`
/// mirror's pty printed those four bytes and nothing else, for ever, and its
/// tmux client never attached. A plain session was freed by whichever
/// terminal answered first, with that terminal's cursor rather than the
/// session's, and a second viewer's answer then arrived at the agent as
/// typed text.
///
/// So the hub answers, once, at the start of the stream: the top-left
/// corner, which is where the session's own emulator starts.
///
/// conhost then asks its terminal for two things of its own - win32 input
/// mode (`?9001h`) and focus reports (`?1004h`) - and those are declined by
/// being kept from the viewers too. Both are requests to the terminal
/// hosting conhost, which is the hub, and a viewer that honoured them would
/// answer the wrong party: under `--tmux` a viewer's keystrokes go to the
/// agent's pane rather than back through this conhost, so the page's focus
/// reports would reach the agent as typed text, and the mode scanner would
/// take `?1004h` for the agent asking. conhost works without either.
///
/// Only the opening is taken. conhost answers the agent's own queries itself,
/// so anything after it is not addressed to alc and passes through untouched,
/// and a stream that does not open with the cursor query - a future Windows,
/// a sideloaded conpty.dll - is left entirely alone.
#[cfg(any(windows, test))]
struct CursorHandshake<R, W> {
    inner: R,
    writer: Arc<Mutex<W>>,
    /// Bytes read while deciding what the stream opens with.
    held: Vec<u8>,
    stage: Handshake,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handshake {
    /// Waiting to see whether the stream opens with the cursor query.
    Query,
    /// Answered; conhost's own requests to its terminal may follow.
    Negotiation,
    /// The opening is over, one way or the other.
    Settled,
}

#[cfg(any(windows, test))]
impl<R: Read, W: Write> CursorHandshake<R, W> {
    const QUERY: &'static [u8] = b"\x1b[6n";
    const ANSWER: &'static [u8] = b"\x1b[1;1R";
    const NEGOTIATION: [&'static [u8]; 2] = [b"\x1b[?9001h", b"\x1b[?1004h"];

    fn new(inner: R, writer: Arc<Mutex<W>>) -> Self {
        Self {
            inner,
            writer,
            held: Vec::new(),
            stage: Handshake::Query,
        }
    }

    /// Reads until the opening is decided.
    ///
    /// Blocks only while what has arrived could still be the start of the
    /// opening, which conhost writes in one burst before anything else.
    fn settle(&mut self) -> std::io::Result<()> {
        let mut chunk = [0_u8; 4096];
        loop {
            let undecided = match self.stage {
                Handshake::Settled => return Ok(()),
                Handshake::Query => {
                    if self.held.starts_with(Self::QUERY) {
                        self.held.drain(..Self::QUERY.len());
                        self.answer()?;
                        self.stage = Handshake::Negotiation;
                        continue;
                    }
                    Self::QUERY.starts_with(&self.held)
                }
                Handshake::Negotiation => {
                    if let Some(request) = Self::NEGOTIATION
                        .iter()
                        .find(|request| self.held.starts_with(request))
                    {
                        self.held.drain(..request.len());
                        continue;
                    }
                    Self::NEGOTIATION
                        .iter()
                        .any(|request| request.starts_with(&self.held))
                }
            };
            if !undecided {
                self.stage = Handshake::Settled;
                return Ok(());
            }
            let read = self.inner.read(&mut chunk)?;
            if read == 0 {
                self.stage = Handshake::Settled;
                return Ok(());
            }
            self.held.extend_from_slice(&chunk[..read]);
        }
    }

    fn answer(&self) -> std::io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| std::io::Error::other("the session's input channel was poisoned"))?;
        writer.write_all(Self::ANSWER)?;
        writer.flush()
    }
}

#[cfg(any(windows, test))]
impl<R: Read, W: Write> Read for CursorHandshake<R, W> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.settle()?;
        if self.held.is_empty() {
            // Nothing held back - including when the stream opened with the
            // handshake alone, which must not read as the end of it.
            return self.inner.read(buffer);
        }
        let count = buffer.len().min(self.held.len());
        buffer[..count].copy_from_slice(&self.held[..count]);
        self.held.drain(..count);
        Ok(count)
    }
}

#[cfg(test)]
mod handshake_tests {
    use super::*;

    /// A reader that hands out its chunks one read at a time, the way a pty
    /// delivers whatever has arrived.
    struct Chunks(std::collections::VecDeque<Vec<u8>>);

    impl Read for Chunks {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let Some(mut chunk) = self.0.pop_front() else {
                return Ok(0);
            };
            let count = buffer.len().min(chunk.len());
            buffer[..count].copy_from_slice(&chunk[..count]);
            if count < chunk.len() {
                self.0.push_front(chunk.split_off(count));
            }
            Ok(count)
        }
    }

    fn drain(chunks: &[&[u8]]) -> (Vec<u8>, Vec<u8>) {
        let writer = Arc::new(Mutex::new(Vec::new()));
        let mut reader = CursorHandshake::new(
            Chunks(chunks.iter().map(|chunk| chunk.to_vec()).collect()),
            Arc::clone(&writer),
        );
        let mut read = Vec::new();
        reader.read_to_end(&mut read).unwrap();
        let answered = writer.lock().unwrap().clone();
        (read, answered)
    }

    #[test]
    fn the_opening_cursor_query_is_answered_and_kept_from_viewers() {
        let (read, answered) = drain(&[b"\x1b[6n", b"hello"]);
        assert_eq!(read, b"hello");
        assert_eq!(answered, b"\x1b[1;1R");
    }

    #[test]
    fn a_query_split_across_reads_is_still_recognised() {
        let (read, answered) = drain(&[b"\x1b[", b"6", b"nhello"]);
        assert_eq!(read, b"hello");
        assert_eq!(answered, b"\x1b[1;1R");
    }

    /// What conhost was measured sending, in the order it sent it.
    #[test]
    fn conhosts_own_requests_to_its_terminal_are_declined() {
        let (read, answered) = drain(&[b"\x1b[6n", b"\x1b[?9001h\x1b[?1004h\x1b[mhello"]);
        assert_eq!(read, b"\x1b[mhello");
        assert_eq!(answered, b"\x1b[1;1R");
        // However the burst happens to be split.
        let (read, _) = drain(&[b"\x1b[6n\x1b[?90", b"01h\x1b[?1004", b"hhello"]);
        assert_eq!(read, b"hello");
    }

    /// A pseudo-console that asks nothing - a future Windows, a sideloaded
    /// conpty.dll - must not have its first bytes eaten or answered, and the
    /// agent's own requests for focus reports are the agent's.
    #[test]
    fn a_stream_that_opens_with_anything_else_is_left_alone() {
        let (read, answered) = drain(&[b"\x1b[?25l", b"hello"]);
        assert_eq!(read, b"\x1b[?25lhello");
        assert!(answered.is_empty());
        let (read, answered) = drain(&[b"\x1b[6", b"x"]);
        assert_eq!(read, b"\x1b[6x");
        assert!(answered.is_empty());
        let (read, answered) = drain(&[b"\x1b[?1004hhello"]);
        assert_eq!(read, b"\x1b[?1004hhello");
        assert!(answered.is_empty());
    }

    /// conhost answers the agent's own queries itself, so a later one is not
    /// alc's to answer and reaches the viewers as the agent wrote it - and
    /// so does a later request for focus reports, which is the agent's.
    #[test]
    fn only_the_opening_is_taken() {
        let (read, answered) = drain(&[b"\x1b[6n", b"a\x1b[6nb\x1b[?1004h"]);
        assert_eq!(read, b"a\x1b[6nb\x1b[?1004h");
        assert_eq!(answered, b"\x1b[1;1R");
    }

    #[test]
    fn a_stream_that_is_only_the_handshake_ends_cleanly() {
        let (read, answered) = drain(&[b"\x1b[6n"]);
        assert!(read.is_empty());
        assert_eq!(answered, b"\x1b[1;1R");
        let (read, _) = drain(&[b"\x1b[6n\x1b[?9001h"]);
        assert!(read.is_empty());
        let (read, answered) = drain(&[]);
        assert!(read.is_empty());
        assert!(answered.is_empty());
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
