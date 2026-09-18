//! The places where Windows' process and console model differs enough from
//! unix that the shared remote-control code cannot paper over it.
//!
//! Each item here exists because of something measured on a real Windows
//! machine rather than read in documentation, and says so.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Once;
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    SetHandleInformation,
};
use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
use windows_sys::Win32::System::Console::{
    CONSOLE_MODE, CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT,
    CTRL_SHUTDOWN_EVENT, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_VIRTUAL_TERMINAL_INPUT,
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, ReadConsoleW,
    STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
    SetConsoleMode,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
};

/// Starts a process that has to outlive this one and the console it runs in.
///
/// Two things differ from a plain spawn, and both were found by measuring:
///
/// * `Command::spawn` hands the child every inheritable handle this process
///   holds, whatever its `Stdio` says - Windows has no per-spawn list without
///   an API std does not use. This process's own stdout and stderr are
///   inheritable whenever something captured them, so the hub went on
///   holding its caller's output pipe for as long as it ran: a test harness,
///   a CI step or a script reading `alc hub start` waited for an end of file
///   that never came. That was the whole of the "hang" that kept remote
///   control off Windows - the hub itself came up and went away normally.
///   The std handles are made non-inheritable for the length of the spawn,
///   so the hub gets the NUL handles std opened for it and nothing else.
/// * `DETACHED_PROCESS` leaves the hub with no console at all, and then every
///   console program it runs - a tmux probe, twice a second - opens a new
///   visible console window of its own. `CREATE_NO_WINDOW` gives the hub a
///   console nobody can see, which those children share instead.
///
/// It also asks to leave the caller's job object, so a terminal or an SSH
/// session that kills its job when it closes does not take the hub with it.
/// A job that does not allow that refuses with access denied, and the spawn
/// is retried inside it rather than failed.
pub(crate) fn spawn_detached(command: &mut Command) -> io::Result<Child> {
    let flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    without_inheritable_std_handles(|| {
        command.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB);
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                command.creation_flags(flags);
                command.spawn()
            }
            other => other,
        }
    })
}

/// Runs `spawn` with this process's std handles marked non-inheritable, and
/// puts back whichever it changed.
///
/// Only the flag is touched, never the handles: this process keeps writing
/// to them as before. A child given `Stdio::inherit` elsewhere is unaffected
/// too, because std duplicates a handle into an inheritable copy for that
/// case rather than relying on the original's flag.
fn without_inheritable_std_handles<T>(spawn: impl FnOnce() -> T) -> T {
    let mut changed: Vec<HANDLE> = Vec::new();
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: plain Win32 calls on this process's own standard handles,
        // which it neither closes nor replaces here. A missing handle comes
        // back null or invalid and is skipped.
        unsafe {
            let handle = GetStdHandle(id);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                continue;
            }
            let mut flags = 0;
            if GetHandleInformation(handle, &mut flags) == 0 || flags & HANDLE_FLAG_INHERIT == 0 {
                continue;
            }
            if SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0 {
                changed.push(handle);
            }
        }
    }
    let result = spawn();
    for handle in changed {
        // SAFETY: the same handles, still open, having their flag put back.
        unsafe {
            SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        }
    }
    result
}

/// The console's input and output modes as they were before a shared session
/// took the terminal over, put back when this is dropped.
///
/// The relay writes the agent's escape sequences to the console verbatim and
/// reads keystrokes as the bytes a terminal would send, which is what a unix
/// terminal does in raw mode and what a Windows console only does when asked:
/// without virtual-terminal processing the output shows up as literal
/// `←[31m` text, and without virtual-terminal input the arrow keys produce
/// nothing at all, so the agent could not be driven from the keyboard that
/// launched it. `DISABLE_NEWLINE_AUTO_RETURN` makes a line feed only a line
/// feed, as raw mode makes it on unix, because the agent already sends the
/// carriage return it wants.
pub(crate) struct VtConsole {
    input: Option<CONSOLE_MODE>,
    output: Option<CONSOLE_MODE>,
}

impl VtConsole {
    /// Switches both modes on, saving what they were.
    ///
    /// Called before crossterm's raw mode, so the saved input mode is the
    /// user's own rather than raw mode's; restoring it undoes both.
    pub(crate) fn enable() -> Self {
        let console = Self {
            input: add_mode(STD_INPUT_HANDLE, ENABLE_VIRTUAL_TERMINAL_INPUT),
            output: add_mode(
                STD_OUTPUT_HANDLE,
                ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN,
            ),
        };
        SAVED_INPUT.store(console.input.unwrap_or(NOTHING_SAVED), Ordering::Release);
        SAVED_OUTPUT.store(console.output.unwrap_or(NOTHING_SAVED), Ordering::Release);
        RESTORE_ON_EXIT.call_once(|| {
            // SAFETY: registers a handler with the signature the API
            // requires; it only swaps two atomics and sets console modes.
            unsafe {
                SetConsoleCtrlHandler(Some(restore_on_exit), 1);
            }
        });
        console
    }

    pub(crate) fn restore(&mut self) {
        SAVED_INPUT.store(NOTHING_SAVED, Ordering::Release);
        SAVED_OUTPUT.store(NOTHING_SAVED, Ordering::Release);
        if let Some(mode) = self.input.take() {
            set_mode(STD_INPUT_HANDLE, mode);
        }
        if let Some(mode) = self.output.take() {
            set_mode(STD_OUTPUT_HANDLE, mode);
        }
    }
}

/// The modes `VtConsole` saved, for a console control handler to put back.
///
/// Ctrl+Break, closing the window, logging off and shutting down each end the
/// process through the console's default handler, which runs no destructor -
/// so a terminal left in raw mode with virtual-terminal input would stay that
/// way for whatever the shell started next. This is the Windows half of what
/// `local::unix_signals` does for SIGTERM and SIGHUP. Atomics because the
/// handler runs on a thread of its own with no way to reach the guard.
static SAVED_INPUT: AtomicU32 = AtomicU32::new(NOTHING_SAVED);
static SAVED_OUTPUT: AtomicU32 = AtomicU32::new(NOTHING_SAVED);
static RESTORE_ON_EXIT: Once = Once::new();

/// Not a mode any console reports: every flag at once.
const NOTHING_SAVED: CONSOLE_MODE = u32::MAX;

unsafe extern "system" fn restore_on_exit(event: u32) -> windows_sys::core::BOOL {
    if matches!(
        event,
        CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
    ) {
        let input = SAVED_INPUT.swap(NOTHING_SAVED, Ordering::AcqRel);
        if input != NOTHING_SAVED {
            set_mode(STD_INPUT_HANDLE, input);
        }
        let output = SAVED_OUTPUT.swap(NOTHING_SAVED, Ordering::AcqRel);
        if output != NOTHING_SAVED {
            set_mode(STD_OUTPUT_HANDLE, output);
        }
    }
    // Not handled: the process still ends the way it was asked to.
    0
}

/// This console's keyboard, read without std's end-of-file convention.
///
/// `io::stdin` on a console drops a Ctrl+Z at the end of a read - DOS's end
/// of file - so a lone Ctrl+Z came back as a zero-length read, which is end
/// of input everywhere else: the shared session stopped taking keys, the
/// detach keys included, the first time anybody pressed it. Measured. An
/// agent has its own use for Ctrl+Z and a console has no end of input, so
/// this reads the console directly and hands on every character as UTF-8.
pub(crate) struct ConsoleInput {
    /// Encoded but not yet read.
    pending: Vec<u8>,
    /// The first half of a character whose second half is still to come.
    high_surrogate: Option<u16>,
}

impl ConsoleInput {
    /// `None` when stdin is not a console, which std then reads as usual.
    pub(crate) fn open() -> Option<Self> {
        let mut mode = 0;
        // SAFETY: a plain query on this process's own standard handle.
        let console = unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            !handle.is_null() && GetConsoleMode(handle, &mut mode) != 0
        };
        console.then(|| Self {
            pending: Vec::new(),
            high_surrogate: None,
        })
    }

    fn decode(&mut self, units: &[u16]) {
        let mut units: Vec<u16> = self
            .high_surrogate
            .take()
            .into_iter()
            .chain(units.iter().copied())
            .collect();
        if units
            .last()
            .is_some_and(|last| (0xD800..0xDC00).contains(last))
        {
            self.high_surrogate = units.pop();
        }
        let mut encoded = [0_u8; 4];
        for character in char::decode_utf16(units) {
            let character = character.unwrap_or(char::REPLACEMENT_CHARACTER);
            self.pending
                .extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
        }
    }
}

impl io::Read for ConsoleInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        while self.pending.is_empty() {
            let mut wide = [0_u16; 512];
            let mut read = 0;
            // SAFETY: `wide` is as long as the count passed, the count
            // written goes to a local, and no read-control block is given -
            // which is the point: that block is what makes Ctrl+Z special.
            let ok = unsafe {
                ReadConsoleW(
                    GetStdHandle(STD_INPUT_HANDLE),
                    wide.as_mut_ptr().cast(),
                    wide.len() as u32,
                    &mut read,
                    std::ptr::null(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            self.decode(&wide[..read as usize]);
        }
        let count = buffer.len().min(self.pending.len());
        buffer[..count].copy_from_slice(&self.pending[..count]);
        self.pending.drain(..count);
        Ok(count)
    }
}

impl Drop for VtConsole {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Adds `extra` to a console handle's mode and returns what it was, or `None`
/// when the handle is not a console.
///
/// A console too old for the flag refuses the whole call; the new mode is
/// then retried without `DISABLE_NEWLINE_AUTO_RETURN`, which is the one of
/// the three a legacy console is most likely not to know.
fn add_mode(id: STD_HANDLE, extra: CONSOLE_MODE) -> Option<CONSOLE_MODE> {
    // SAFETY: plain Win32 calls on this process's own standard handle.
    unsafe {
        let handle = GetStdHandle(id);
        let mut mode = 0;
        if handle.is_null() || GetConsoleMode(handle, &mut mode) == 0 {
            return None;
        }
        if SetConsoleMode(handle, mode | extra) == 0 {
            SetConsoleMode(handle, mode | (extra & !DISABLE_NEWLINE_AUTO_RETURN));
        }
        Some(mode)
    }
}

fn set_mode(id: STD_HANDLE, mode: CONSOLE_MODE) {
    // SAFETY: plain Win32 calls on this process's own standard handle.
    unsafe {
        let handle = GetStdHandle(id);
        if !handle.is_null() {
            SetConsoleMode(handle, mode);
        }
    }
}

/// The 8.3 form of `path`, which Windows keeps in plain ASCII.
///
/// Fails where the volume has short names turned off, which is a setting
/// rather than an error, so the caller says what to do instead.
pub(crate) fn short_path(path: &Path) -> io::Result<PathBuf> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buffer = vec![0_u16; 260];
    loop {
        // SAFETY: `wide` is NUL-terminated and `buffer` is as long as the
        // length passed; the call writes at most that many units.
        let written =
            unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) }
                as usize;
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        // Too small: the return value is the size needed, terminator
        // included.
        if written >= buffer.len() {
            buffer.resize(written, 0);
            continue;
        }
        return Ok(PathBuf::from(OsString::from_wide(&buffer[..written])));
    }
}

/// Ties `child` to this process, so that if this process ends first -
/// however it ends - the child ends with it.
///
/// For the tmux pane launcher, whose pid is the one tmux reports for the
/// pane. Something that stops that process must not leave the agent running
/// in a pane that tmux already reports as dead. `SILENT_BREAKAWAY_OK` keeps
/// the agent's own children out of the job, so they live and die by the
/// ordinary console rules exactly as they would without `--tmux`; only the
/// agent is bound to the launcher.
///
/// Best-effort: a process already in a job that forbids nesting refuses the
/// assignment, and the agent then simply runs unbound.
pub(crate) fn bind_to_this_process(child: &Child) -> Option<OwnedHandle> {
    // SAFETY: `CreateJobObjectW` with no name and no security attributes
    // returns a fresh handle or null; ownership passes to `OwnedHandle`,
    // which closes it exactly once.
    let job = unsafe {
        let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if raw.is_null() {
            return None;
        }
        OwnedHandle::from_raw_handle(raw)
    };
    // SAFETY: a zeroed limit block is the documented "no limits" value, and
    // only the flags field is then set.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
    // SAFETY: both handles are open for the length of these calls, and the
    // limit block is the type and size the information class names.
    unsafe {
        if SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            return None;
        }
        if AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) == 0 {
            return None;
        }
    }
    Some(job)
}

/// Keeps Ctrl+C and Ctrl+Break from ending this process.
///
/// Every process attached to a console receives its control events - the
/// pane launcher and the agent it started alike. The agent decides what
/// Ctrl+C means (most of them interrupt a turn, not exit), and the launcher
/// dying of it instead would mark the pane dead with the agent still in it.
///
/// A handler routine rather than `SetConsoleCtrlHandler(NULL, TRUE)`, because
/// the NULL form is inherited by child processes and would switch Ctrl+C off
/// for the agent too; a routine is this process's alone.
pub(crate) fn ignore_console_interrupts() {
    unsafe extern "system" fn handler(event: u32) -> windows_sys::core::BOOL {
        windows_sys::core::BOOL::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
    }
    // SAFETY: registers a handler with the signature the API requires; the
    // function is a plain `extern "system" fn` that touches no state.
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(reads: &[&[u16]]) -> String {
        let mut input = ConsoleInput {
            pending: Vec::new(),
            high_surrogate: None,
        };
        for units in reads {
            input.decode(units);
        }
        String::from_utf8(input.pending).unwrap()
    }

    /// Ctrl+Z is a key like any other here, which is the reason this reader
    /// exists: std drops it, and a lone one read as the end of input.
    #[test]
    fn ctrl_z_is_passed_on_as_a_byte() {
        assert_eq!(decoded(&[&[0x1a]]), "\u{1a}");
        assert_eq!(decoded(&[&[0x61, 0x1a]]), "a\u{1a}");
    }

    /// A character outside the basic plane arrives as two UTF-16 units, and
    /// a console read can end between them.
    #[test]
    fn a_character_split_across_two_reads_is_put_back_together() {
        // U+1F600 is d83d de00.
        assert_eq!(decoded(&[&[0x61, 0xd83d], &[0xde00, 0x62]]), "a\u{1f600}b");
        // A lone low surrogate can never be completed.
        assert_eq!(decoded(&[&[0xde00]]), "\u{fffd}");
    }
}
