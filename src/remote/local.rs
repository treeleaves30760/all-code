//! The user's own terminal, while a session is being mirrored.
//!
//! Sharing a session must not take it away from the person who started it.
//! The local terminal stays fully interactive: it goes into raw mode so
//! keystrokes reach the agent unbuffered, its output is the agent's bytes
//! verbatim, and its size drives the pty's.
//!
//! Raw mode is the reason this module carries signal handling that the rest
//! of alc does not need. A terminal left in raw mode outlives the process
//! that set it: the user's shell keeps echoing nothing until they run
//! `reset`. `Drop` covers a normal exit and a panic, but not SIGTERM or
//! SIGHUP, so those are caught and the saved termios restored from the
//! handler - using only `tcsetattr`, which is async-signal-safe - before the
//! signal is re-raised with its default disposition.

use std::io::{self, IsTerminal, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::terminal;

/// How often the local terminal is asked whether it changed size. A poll
/// rather than SIGWINCH: it is a handful of syscalls a second, it needs no
/// signal handler, and it behaves the same on Windows.
const RESIZE_POLL: Duration = Duration::from_millis(250);

/// Puts the terminal in raw mode for as long as it is held.
pub(crate) struct TerminalGuard {
    restored: bool,
}

impl TerminalGuard {
    /// Fails rather than degrading when there is no terminal: an agent run
    /// with its output piped to a file is a scripted invocation, and turning
    /// it into a mirrored TUI session would replace that file's contents
    /// with escape sequences.
    pub(crate) fn acquire() -> Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            bail!(
                "sharing a session needs an interactive terminal; it cannot be used with redirected input or output"
            );
        }
        #[cfg(unix)]
        unix_signals::save_and_install()?;
        terminal::enable_raw_mode().context("failed to put the terminal into raw mode")?;
        Ok(Self { restored: false })
    }

    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = terminal::disable_raw_mode();
        let mut stdout = io::stdout();
        // The agent may have left the alternate screen active or the cursor
        // hidden; the shell the user returns to should have neither.
        let _ = stdout.write_all(b"\x1b[?1049l\x1b[?25h\x1b[?2004l\x1b[?1000l\x1b[?1006l");
        let _ = stdout.flush();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// The current terminal size, falling back to a conventional default when
/// the platform will not say.
pub(crate) fn size() -> (u16, u16) {
    terminal::size().map_or((80, 24), |(cols, rows)| (cols.max(20), rows.max(4)))
}

/// The detach sequence: Ctrl-\\ then `d`.
///
/// Two keys, not one, because a terminal agent has a use for nearly every
/// single keystroke - and Ctrl-\\ alone is SIGQUIT, which raw mode has
/// already stopped delivering, so borrowing it costs nothing that was
/// working.
const DETACH_LEAD: u8 = 0x1c;
const DETACH_KEY: u8 = b'd';

/// Forwards this terminal's keystrokes into `sink` until stdin ends or the
/// user detaches.
///
/// Reads raw bytes rather than decoding key events: the agent is a terminal
/// program expecting exactly what a terminal sends, and re-encoding
/// crossterm's parsed events would lose bracketed paste, mouse reports and
/// any sequence crossterm does not model.
///
/// Sets `detached` and returns when the detach sequence is typed, so the
/// caller can leave the session running rather than ending it.
pub(crate) fn pump_stdin<W: Write + Send + 'static>(
    mut sink: W,
    detached: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
) {
    thread::Builder::new()
        .name("alc-local-stdin".to_owned())
        .spawn(move || {
            let mut stdin = io::stdin();
            let mut buffer = [0_u8; 1024];
            let mut armed = false;
            while !finished.load(Ordering::Relaxed) {
                let read = match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };

                let (forward, detach) = split_detach(&buffer[..read], &mut armed);
                if !forward.is_empty()
                    && (sink.write_all(&forward).is_err() || sink.flush().is_err())
                {
                    break;
                }
                if detach {
                    detached.store(true, Ordering::Release);
                    return;
                }
            }
        })
        .ok();
}

/// Splits a read into the bytes that belong to the agent and whether the
/// detach sequence completed.
///
/// `armed` carries the half-typed sequence across reads: a keystroke pair
/// can land in two different `read` calls, and a state machine that reset
/// between them would make detaching work only by luck.
fn split_detach(chunk: &[u8], armed: &mut bool) -> (Vec<u8>, bool) {
    let mut forward = Vec::with_capacity(chunk.len());
    for &byte in chunk {
        if *armed {
            *armed = false;
            if byte == DETACH_KEY {
                return (forward, true);
            }
            // Not the detach key, so the lead was a real keystroke after all
            // and both bytes belong to the agent.
            forward.push(DETACH_LEAD);
            forward.push(byte);
            continue;
        }
        if byte == DETACH_LEAD {
            *armed = true;
            continue;
        }
        forward.push(byte);
    }
    (forward, false)
}

/// Reports this terminal's size to `on_change` whenever it changes.
pub(crate) fn watch_resize<F>(finished: Arc<AtomicBool>, on_change: F)
where
    F: Fn(u16, u16) + Send + 'static,
{
    thread::Builder::new()
        .name("alc-local-resize".to_owned())
        .spawn(move || {
            let mut last = size();
            while !finished.load(Ordering::Relaxed) {
                thread::sleep(RESIZE_POLL);
                let current = size();
                if current != last {
                    last = current;
                    on_change(current.0, current.1);
                }
            }
        })
        .ok();
}

#[cfg(unix)]
mod unix_signals {
    //! Restoring the terminal when alc is killed rather than exiting.
    //!
    //! The handler runs in a signal context, so it may only call
    //! async-signal-safe functions. `tcsetattr` is one; allocating,
    //! formatting or locking are not. It therefore restores the termios
    //! saved before raw mode was entered, writes one fixed escape string,
    //! and re-raises the signal with the default disposition so the exit
    //! status still says the process was signalled.

    use std::os::fd::AsRawFd;
    use std::sync::{Once, OnceLock};

    use anyhow::{Context, Result};

    /// `libc::termios` is a plain C struct with no interior mutability, and
    /// this one is written once before any handler can run and only read
    /// afterwards.
    struct Saved(libc::termios);
    // SAFETY: written once through `OnceLock`, never mutated after, and
    // read only as a value to hand back to `tcsetattr`.
    unsafe impl Sync for Saved {}

    static INSTALL: Once = Once::new();
    static SAVED: OnceLock<Saved> = OnceLock::new();

    pub(super) fn save_and_install() -> Result<()> {
        let fd = std::io::stdin().as_raw_fd();
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `termios` is a valid, owned, correctly sized buffer and
        // `fd` is this process's stdin, which `TerminalGuard::acquire` has
        // already confirmed is a terminal.
        let read = unsafe { libc::tcgetattr(fd, &raw mut termios) };
        if read != 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to read the terminal's settings");
        }
        let _ = SAVED.set(Saved(termios));

        INSTALL.call_once(|| {
            for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                // SAFETY: `handler` is async-signal-safe; see the module note.
                unsafe {
                    libc::signal(
                        signal,
                        handler as extern "C" fn(libc::c_int) as libc::sighandler_t,
                    )
                };
            }
        });
        Ok(())
    }

    extern "C" fn handler(signal: libc::c_int) {
        // SAFETY: async-signal-safe calls only. `OnceLock::get` is a
        // relaxed load and a read; it neither allocates nor locks.
        unsafe {
            if let Some(saved) = SAVED.get() {
                libc::tcsetattr(0, libc::TCSANOW, &saved.0);
            }
            const RESET: &[u8] = b"\x1b[?1049l\x1b[?25h\x1b[?2004l";
            libc::write(1, RESET.as_ptr().cast(), RESET.len());
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquiring_a_terminal_fails_when_stdio_is_not_one() {
        // Under `cargo test` stdout is captured, so this is the redirected
        // case: `--share` must refuse rather than write escape sequences
        // into whatever the user redirected to.
        let Err(error) = TerminalGuard::acquire() else {
            panic!("acquiring a terminal must fail when stdio is redirected");
        };
        assert!(
            error.to_string().contains("interactive terminal"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn the_detach_sequence_is_recognised_and_not_forwarded() {
        let mut armed = false;
        let (forward, detach) = split_detach(b"abc\x1cd", &mut armed);
        assert_eq!(forward, b"abc".to_vec());
        assert!(detach);
    }

    #[test]
    fn a_detach_sequence_split_across_two_reads_still_works() {
        // The two keystrokes routinely land in different reads; a machine
        // that reset between them would detach only by luck.
        let mut armed = false;
        let (first, detach) = split_detach(b"hi\x1c", &mut armed);
        assert_eq!(first, b"hi".to_vec());
        assert!(!detach);
        assert!(armed);

        let (second, detach) = split_detach(b"d", &mut armed);
        assert!(second.is_empty());
        assert!(detach);
    }

    #[test]
    fn the_lead_key_alone_still_reaches_the_agent() {
        // Ctrl-\\ followed by anything else was a real keystroke, and a
        // terminal program has a use for nearly every one of them.
        let mut armed = false;
        let (forward, detach) = split_detach(b"\x1cx", &mut armed);
        assert_eq!(forward, b"\x1cx".to_vec());
        assert!(!detach);
    }

    #[test]
    fn ordinary_input_passes_through_untouched() {
        let mut armed = false;
        let typed = b"git commit -m 'fix'\r";
        let (forward, detach) = split_detach(typed, &mut armed);
        assert_eq!(forward, typed.to_vec());
        assert!(!detach);
        assert!(!armed);
    }

    #[test]
    fn bytes_before_the_detach_still_reach_the_agent() {
        let mut armed = false;
        let (forward, detach) = split_detach(b"done\r\x1cd", &mut armed);
        assert_eq!(forward, b"done\r".to_vec());
        assert!(detach);
    }

    #[test]
    fn size_is_never_degenerate() {
        let (cols, rows) = size();
        assert!(cols >= 20 && rows >= 4, "{cols}x{rows}");
    }
}
