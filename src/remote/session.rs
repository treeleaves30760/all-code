//! One mirrored coding-agent session.
//!
//! A session owns the agent process, the terminal emulator that tracks what
//! its screen currently looks like, the replay buffer that lets a viewer
//! reconnect without losing its place, and the fan-out to everyone watching.
//!
//! The shape is one pump thread reading the pty and pushing to everything
//! else. That thread is never allowed to block: it is what the agent's own
//! output backpressures against, so every downstream is either lock-free,
//! bounded-and-lossy (`fanout`), or held for a few microseconds at a time.
//!
//! LOCK ORDER, top to bottom: `card` -> `vt` -> `ring` -> `modes` -> `scrub`.
//! `fanout` takes its own lock internally and must never be held while any
//! of the above is. Nothing here may take two of these in the other order.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config::Agent;
use crate::launch::{LaunchSpec, SessionGuards};
use crate::remote::caps::{Confidence, caps};
use crate::remote::fanout::{Fanout, Frame, Subscription};
use crate::remote::permission::{PermState, probe};
use crate::remote::pty::{PtyCommand, PtyHost};
use crate::remote::ring::SeqRing;
use crate::remote::screen::ModeScanner;
use crate::remote::scrub::SecretScrubber;
use crate::remote::tmux::Tmux;
use crate::remote::utf8::Utf8Chunker;
use crate::remote::wire::{
    ExitInfo, NoticeLevel, OP_OUTPUT, OP_SNAPSHOT, ServerFrame, SessionCard, SessionState,
    encode_binary,
};

/// How much scrollback the emulator keeps for a snapshot. Enough that a
/// phone joining mid-task sees context, small enough that a session that has
/// been running for hours is not carrying a transcript around.
const SCROLLBACK_LINES: usize = 1_000;

/// A pty read is at most this big. Matches the usual pty buffer, so a busy
/// agent is drained in one syscall rather than several.
const READ_CHUNK: usize = 8 * 1024;

/// How often a tmux-hosted session is asked whether its agent is still
/// alive. A poll rather than a hook: it is one short-lived tmux client
/// twice a second against a socket on the same machine, and a hook would
/// mean a shell command tmux runs on alc's behalf with no way to report a
/// failure back.
const TMUX_POLL: Duration = Duration::from_millis(500);

/// How many unanswered probes in a row mean the tmux server is really gone.
const UNANSWERED_PROBES: u32 = 3;

/// The tmux session an agent runs in, and the binary to talk to it with.
///
/// Once this is present, four things stop coming from the pty and start
/// coming from tmux, because the pty's child is now a `tmux attach-session`
/// client rather than the agent: how the agent exited (a client exits 0
/// whatever the agent did), how to stop it (signalling the client only
/// detaches the mirror and leaves the agent running unwatched), which pid to
/// show on the card, and how a viewer's keystrokes are delivered (bytes
/// written to a tmux client are parsed as keys first - see `Session::input`).
pub(crate) struct TmuxHost {
    pub tmux: Tmux,
    pub binary: PathBuf,
}

/// Everything about a session that is decided before it starts.
pub(crate) struct SessionSpec {
    pub id: String,
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    pub scrollback_bytes: usize,
    /// What alc believes the agent started in. `Launched` confidence only
    /// when alc passed the flag itself; see `caps.rs`.
    pub permission: PermState,
}

pub(crate) struct Session {
    id: String,
    name: Mutex<String>,
    cwd: String,
    started_at: u64,
    agent: Agent,
    provider: String,
    provider_kind: String,
    model: Option<String>,
    effort: Option<String>,

    pty: PtyHost,
    vt: Mutex<avt::Vt>,
    ring: Mutex<SeqRing>,
    modes: Mutex<ModeScanner>,
    scrubber: Mutex<SecretScrubber>,
    fanout: Fanout,

    perm: Mutex<PermState>,
    size: Mutex<(u16, u16)>,
    /// The tmux session hosting the agent, when the launch asked for one.
    tmux: Option<TmuxHost>,
    /// The window size and agent pid tmux last reported, refreshed by the
    /// watcher.
    ///
    /// Cached rather than asked for on demand because both are read on every
    /// card - which is every session list, every notice and every viewer
    /// joining - and `Registry::cards` builds those while holding the
    /// registry's own lock. Forking a tmux client per session in there would
    /// put a process spawn per session behind that lock on every page
    /// refresh.
    tmux_window: Mutex<Option<(u16, u16)>>,
    tmux_pid: Mutex<Option<u32>>,
    /// Set by `kill`, so the watcher escalates from asking the agent to stop
    /// to stopping the server it runs in.
    killing: AtomicBool,
    seq: AtomicU64,
    warned_clipboard: AtomicBool,

    /// Filled once, by the pump thread, when the agent is gone.
    ///
    /// The pump is the only caller of `PtyHost::wait`, which is only safe
    /// once the reader has reached EOF - see the note on that method.
    exit: Mutex<Option<ExitInfo>>,

    /// Held for its `Drop`: Claude Code's own default model, the Codex
    /// bridge, and any temporary config written for this launch.
    ///
    /// Released by the pump the moment the pty reaches EOF, not when the
    /// session is finally dropped. An exited session's card lingers on the
    /// page for fifteen minutes so a launch that failed can still be read,
    /// and holding these that long would mean a bridge listening on a
    /// loopback port with nothing to serve and - worse - the Kimi builder's
    /// plaintext key file sitting in the temp directory for a quarter of an
    /// hour after the agent that needed it exited. The unshared path
    /// (`launch::execute`) drops them the instant the child is reaped, and
    /// this is the same promise.
    guards: Mutex<Option<SessionGuards>>,
}

impl Session {
    /// Spawns the agent and starts the pump thread that mirrors it.
    ///
    /// There is no unscrubbed view. Every viewer, including the terminal the
    /// session was launched from, reads the same masked stream - the
    /// terminal is attached through the hub like any browser is, and one
    /// rule for every viewer is both simpler to reason about and the safer
    /// default. What it costs is seeing your own provider key echoed back,
    /// which is not something anyone wants.
    pub(crate) fn start(
        session: SessionSpec,
        command: PtyCommand,
        spec: LaunchSpec,
        cwd: &Path,
        guards: SessionGuards,
        tmux: Option<TmuxHost>,
    ) -> Result<Arc<Self>> {
        let SessionSpec {
            id,
            name,
            cols,
            rows,
            scrollback_bytes,
            permission: _,
        } = session;
        let (pty, reader) = PtyHost::spawn(&command, cwd, cols, rows)?;

        let session = Arc::new(Self {
            id,
            name: Mutex::new(name),
            cwd: cwd.display().to_string(),
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_secs())
                .unwrap_or_default(),
            agent: spec.agent,
            provider: spec.provider_name.clone(),
            provider_kind: spec.provider_kind.to_string(),
            model: spec.model.clone(),
            effort: spec.effort.map(|effort| effort.to_string()),
            pty,
            vt: Mutex::new(
                avt::Vt::builder()
                    .size(usize::from(cols), usize::from(rows))
                    .scrollback_limit(SCROLLBACK_LINES)
                    .build(),
            ),
            ring: Mutex::new(SeqRing::new(scrollback_bytes)),
            modes: Mutex::new(ModeScanner::new()),
            scrubber: Mutex::new(SecretScrubber::new(&spec.secret_values)),
            fanout: Fanout::new(),
            perm: Mutex::new(session.permission),
            size: Mutex::new((cols, rows)),
            // Seeded from creation rather than left for the watcher's first
            // poll: a card built in the half second before that would
            // otherwise report a session with no pid at all.
            tmux_pid: Mutex::new(
                tmux.as_ref()
                    .and_then(|host| host.tmux.probe(&host.binary))
                    .and_then(|snapshot| snapshot.pane_pid),
            ),
            tmux,
            tmux_window: Mutex::new(None),
            killing: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            warned_clipboard: AtomicBool::new(false),
            exit: Mutex::new(None),
            guards: Mutex::new(Some(guards)),
        });

        let pump = Arc::clone(&session);
        thread::Builder::new()
            .name(format!("alc-session-{}", session.id))
            .spawn(move || pump.pump(reader))
            .context("failed to start the session's output thread")?;

        if session.tmux.is_some() {
            let watcher = Arc::clone(&session);
            thread::Builder::new()
                .name(format!("alc-tmux-{}", session.id))
                .spawn(move || watcher.watch_tmux())
                .context("failed to start the session's tmux watcher")?;
        }

        Ok(session)
    }

    /// Watches the tmux session, because the pty can no longer be the thing
    /// that says when the agent is gone.
    ///
    /// With `remain-on-exit` on, a dead agent leaves its pane in place, so
    /// nothing closes and no client sees EOF. This is what notices, records
    /// the agent's real exit status, and then stops the server - which is
    /// what finally gives every client its EOF and lets `pump` finish the
    /// session the same way it always has.
    ///
    /// It also records the window size, which the permission probe reads,
    /// and the agent's pid, which the card shows.
    fn watch_tmux(self: Arc<Self>) {
        let Some(host) = self.tmux.as_ref() else {
            return;
        };
        // One unanswered probe is not proof of anything: tmux can refuse a
        // client under fd pressure, and a watcher that gave up on the first
        // one would leave the session reported as Running for ever, with
        // nothing left to notice the agent had gone.
        let mut unanswered = 0;
        loop {
            match host.tmux.probe(&host.binary) {
                Some(snapshot) => {
                    unanswered = 0;
                    if let Some(window) = snapshot.window {
                        self.note_tmux_window(window);
                    }
                    if let Ok(mut pid) = self.tmux_pid.lock() {
                        *pid = snapshot.pane_pid.or(*pid);
                    }
                    if let Some(exit) = snapshot.exit {
                        self.record_exit(exit);
                        // Ends the session, which detaches every client and
                        // stops the server. The user's own terminal, attached
                        // as its own tmux client, returns to their shell here.
                        let _ = host.tmux.stop(&host.binary);
                        return;
                    }
                    // `kill` asked the agent to stop and it is still here.
                    // Waiting longer would mean `alc kill`, `alc hub stop
                    // --drain` and the page's stop button each reporting they
                    // stopped something that carried on running.
                    if self.killing.load(Ordering::Acquire) {
                        let _ = host.tmux.stop(&host.binary);
                        return;
                    }
                }
                None => {
                    // A stop that was asked for and cannot be confirmed is
                    // still a stop: the pane may be gone with the server
                    // behind it, or tmux may simply not be answering, and
                    // either way leaving the agent running is the one
                    // outcome `kill` must not produce.
                    if self.killing.load(Ordering::Acquire) {
                        let _ = host.tmux.stop(&host.binary);
                        return;
                    }
                    unanswered += 1;
                    if unanswered >= UNANSWERED_PROBES {
                        // The server is gone - `alc kill`, or someone
                        // reaching for tmux directly. `pump` has its EOF
                        // already; there is nothing left to watch.
                        return;
                    }
                }
            }
            if self.has_exited() {
                return;
            }
            thread::sleep(TMUX_POLL);
        }
    }

    /// Records the size tmux settled the window on. Bookkeeping only - it
    /// moves nothing.
    ///
    /// The mirror is the client that votes (`tmux::attach_argv`), so the
    /// window *is* the mirror's pty size and there is nothing to follow.
    /// Writing the pty from here would be worse than redundant: this poll
    /// runs every 500ms, and one landing between `resize_from_viewer` and
    /// tmux's own window update would read the old window and drag the
    /// mirror back to it - whereupon the window, which follows the mirror,
    /// would come back down with it and the browser's resize would be
    /// silently undone. A slow flap, in a loop the old arrangement could not
    /// have, because back then the mirror had no vote.
    ///
    /// What the value is still for is `Session::permission`, which cuts the
    /// emulator's text back to the window's rows before reading the bottom
    /// of it. The two agree except for the few milliseconds tmux takes to
    /// catch up with a resize, and that is exactly the window this keeps
    /// honest.
    fn note_tmux_window(&self, (cols, rows): (u16, u16)) {
        if let Ok(mut window) = self.tmux_window.lock() {
            *window = Some((cols, rows));
        }
    }

    /// Records how the agent ended, unless something already has.
    ///
    /// Two threads can reach this: the tmux watcher, which has the agent's
    /// own status, and `pump`, which only has the tmux client's. First write
    /// wins, and the watcher always gets there first because it is what
    /// stops the server that gives `pump` its EOF.
    fn record_exit(&self, exit: ExitInfo) {
        if let Ok(mut slot) = self.exit.lock()
            && slot.is_none()
        {
            *slot = Some(exit);
        }
    }

    /// Reads the pty until the agent closes it, feeding every byte to the
    /// emulator, the replay buffer and every viewer.
    fn pump(self: Arc<Self>, mut reader: Box<dyn Read + Send>) {
        let mut chunker = Utf8Chunker::new();
        let mut buffer = vec![0_u8; READ_CHUNK];

        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                // A closed pty surfaces as an error on some platforms and as
                // a zero-length read on others; both mean the agent is gone.
                Err(_) => break,
            };
            let raw = &buffer[..read];

            let masked = {
                let mut scrubber = match self.scrubber.lock() {
                    Ok(scrubber) => scrubber,
                    Err(_) => break,
                };
                if scrubber.is_empty() {
                    raw.to_vec()
                } else {
                    scrubber.push(raw)
                }
            };
            if masked.is_empty() {
                continue;
            }

            let clipboard_read = if let Ok(mut modes) = self.modes.lock() {
                modes.feed(&masked);
                modes.saw_clipboard_read()
            } else {
                false
            };
            // OSC 52's read form asks the terminal to type the viewer's
            // clipboard back into the agent's stdin. alc never answers one -
            // a hostile file the agent prints would otherwise exfiltrate
            // whatever the viewer last copied into the model's context - but
            // the attempt itself is worth saying out loud once.
            if clipboard_read && !self.warned_clipboard.swap(true, Ordering::AcqRel) {
                self.notify(
                    NoticeLevel::Warn,
                    "this session asked to read the clipboard; alc refused",
                );
            }
            if let Ok(mut vt) = self.vt.lock() {
                let text = chunker.push(&masked);
                if !text.is_empty() {
                    vt.feed_str(text);
                }
            }
            let seq = {
                let Ok(mut ring) = self.ring.lock() else {
                    break;
                };
                ring.push(&masked);
                ring.seq()
            };
            self.seq.store(seq, Ordering::Release);
            self.fanout
                .broadcast(Frame::Binary(encode_binary(OP_OUTPUT, seq, &masked)));
        }

        // Both stream buffers hold bytes back by design - the scrubber a
        // partial credential match, the chunker an incomplete character -
        // so both are drained here or a session's last output is lost.
        let tail = self
            .scrubber
            .lock()
            .map(|mut scrubber| scrubber.flush())
            .unwrap_or_default();
        if !tail.is_empty() {
            let seq = {
                match self.ring.lock() {
                    Ok(mut ring) => {
                        ring.push(&tail);
                        ring.seq()
                    }
                    Err(_) => self.seq(),
                }
            };
            self.seq.store(seq, Ordering::Release);
            if let Ok(mut vt) = self.vt.lock() {
                let text = chunker.push(&tail);
                if !text.is_empty() {
                    vt.feed_str(text);
                }
            }
            self.fanout
                .broadcast(Frame::Binary(encode_binary(OP_OUTPUT, seq, &tail)));
        }
        if let Ok(mut vt) = self.vt.lock() {
            let rest = chunker.flush();
            if !rest.is_empty() {
                vt.feed_str(rest);
            }
        }

        // A tmux session's pty child is a `tmux attach-session` client, and a
        // client's exit status says nothing about the agent: it is 0 whether
        // the agent finished, failed, or was stopped from the page. The
        // watcher has already recorded the real one - and if EOF arrived for
        // any other reason, this is where alc finds out the agent is still
        // running with nobody left watching it.
        if let Some(host) = self.tmux.as_ref() {
            if let Some(snapshot) = host.tmux.probe(&host.binary) {
                match snapshot.exit {
                    Some(exit) => self.record_exit(exit),
                    // Something detached the mirror without ending the
                    // session - `tmux detach-client`, or a stray kill of that
                    // one process. Leaving it would strand the agent: the
                    // guards below tear down the Codex adapter and delete the
                    // temporary config it is still using, and the card is
                    // about to say the session ended. Stopping it is the only
                    // answer that keeps those two stories the same.
                    None => {
                        let _ = host.tmux.stop(&host.binary);
                    }
                }
            }
            self.record_exit(ExitInfo {
                code: None,
                signal: None,
            });
        } else {
            let exit = self.pty.wait().unwrap_or(ExitInfo {
                code: None,
                signal: None,
            });
            self.record_exit(exit);
        }
        let exit = self
            .exit
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or(ExitInfo {
                code: None,
                signal: None,
            });
        let frame = ServerFrame::Exit {
            code: exit.code,
            signal: exit.signal.clone(),
        };
        if let Ok(rendered) = serde_json::to_string(&frame) {
            self.fanout.broadcast(Frame::Text(rendered));
        }
        self.fanout.broadcast(Frame::Close);

        // Torn down only after every viewer has been told the session ended.
        // `Bridge::drop` waits up to two seconds for its runtime thread, and
        // doing that first would make each viewer sit through it before
        // learning what they were waiting for. Taken under the lock and
        // dropped outside it, so that wait is not held over a lock a viewer
        // may want.
        let guards = self.guards.lock().ok().and_then(|mut held| held.take());
        drop(guards);
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn agent(&self) -> Agent {
        self.agent
    }

    /// The mode alc believes this session is in, refreshed from the screen
    /// when the agent renders one and alc did not set it itself.
    ///
    /// A `Launched` state is never overwritten by the probe: alc passed that
    /// flag and nothing has been sent since, which is a stronger claim than
    /// matching a phrase against a status line that any release can restyle.
    pub(crate) fn permission(&self) -> PermState {
        let Ok(mut current) = self.perm.lock() else {
            return PermState::unknown();
        };
        if current.confidence == Confidence::Launched {
            return current.clone();
        }
        let caps = caps(self.agent);
        let mut screen = self.vt.lock().map(|vt| vt.text()).unwrap_or_default();
        // `probe` reads the bottom rows, where an agent renders its mode
        // line. Under `--tmux` the bottom of this grid is not always the
        // bottom of the agent's screen: the window follows the mirror, but
        // for the few milliseconds tmux takes to catch up with a resize the
        // two disagree, so the grid is cut back to the window tmux last
        // reported. The transient runs the other way from the one this used
        // to guard - a browser growing the view makes the emulator taller
        // than the last-seen window, so a card built in that instant reads
        // four rows from the middle of the screen rather than the bottom.
        // Bounded, corrected by the next poll, and the worst it can produce
        // is a stale rung at `Confidence::Reported`, which is never allowed
        // to decide anything on its own (`permission.rs`).
        if let Some((_, rows)) = self.tmux_window.lock().ok().and_then(|window| *window) {
            screen.truncate(usize::from(rows).min(screen.len()));
        }
        if let Some(rung) = probe(caps, &screen) {
            *current = PermState {
                rung: Some(rung),
                native: caps.mode(rung).map(|mode| mode.label.to_owned()),
                confidence: Confidence::Reported,
            };
        }
        current.clone()
    }

    /// Renames the card. The id never changes - a link already open stays
    /// pointed at the same session.
    pub(crate) fn rename(&self, name: String) {
        if let Ok(mut current) = self.name.lock() {
            *current = name;
        }
    }

    pub(crate) fn set_permission(&self, state: PermState) {
        if let Ok(mut current) = self.perm.lock() {
            *current = state;
        }
    }

    pub(crate) fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    pub(crate) fn card(&self) -> SessionCard {
        let (cols, rows) = self.size.lock().map(|size| *size).unwrap_or((80, 24));
        let exit = self.exit.lock().ok().and_then(|slot| slot.clone());
        SessionCard {
            id: self.id.clone(),
            name: self
                .name
                .lock()
                .map(|name| name.clone())
                .unwrap_or_default(),
            agent: self.agent.to_string(),
            provider: self.provider.clone(),
            provider_kind: self.provider_kind.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            cwd: self.cwd.clone(),
            // The agent's pid, which under `--tmux` is the pane's rather
            // than the pty child's - the pty child is only the mirror.
            pid: self.agent_pid(),
            state: if exit.is_some() {
                SessionState::Exited
            } else {
                SessionState::Running
            },
            exit,
            cols,
            rows,
            viewers: self.fanout.len(),
            started_at: self.started_at,
            unsandboxed: !caps(self.agent).sandboxed,
            permission: self.permission(),
            tmux: self.tmux.as_ref().map(|host| host.tmux.clone()),
        }
    }

    /// The agent's own process id.
    ///
    /// Read from what the watcher last saw rather than asked for here: this
    /// runs inside `Registry::cards`, which holds the registry's lock while
    /// it builds every card, and a tmux client forked per session in there
    /// would put a process spawn per session behind that lock on every page
    /// refresh.
    fn agent_pid(&self) -> Option<u32> {
        match self.tmux.is_some() {
            true => self.tmux_pid.lock().ok().and_then(|pid| *pid),
            false => self.pty.process_id(),
        }
    }

    /// What a viewer joining now should draw: the private modes the agent
    /// turned on, then the screen contents.
    ///
    /// The modes come first and separately because `avt::Vt::dump` emits
    /// cells only. Without the prelude a late joiner loses bracketed paste -
    /// and would then paste a multi-line prompt into Claude Code line by
    /// line, submitting the first line on its own.
    pub(crate) fn snapshot(&self) -> String {
        let prelude = self
            .modes
            .lock()
            .map(|modes| modes.prelude())
            .unwrap_or_default();
        let screen = self.vt.lock().map(|vt| vt.dump()).unwrap_or_default();
        format!("{prelude}{screen}")
    }

    /// The binary frame that brings a viewer up to date: an exact delta when
    /// its cursor is still buffered, a full snapshot when it is not.
    pub(crate) fn catch_up(&self, since: Option<u64>) -> Vec<u8> {
        let seq = self.seq();
        if let Some(since) = since
            && let Ok(ring) = self.ring.lock()
            && let Some(delta) = ring.since(since)
        {
            return encode_binary(OP_OUTPUT, seq, &delta);
        }
        encode_binary(OP_SNAPSHOT, seq, self.snapshot().as_bytes())
    }

    /// Delivers a viewer's keystrokes to the agent.
    ///
    /// A tmux session's pty belongs to the mirror's tmux *client*, and bytes
    /// written to a tmux client are parsed as keys before they are anything
    /// else. A viewer who sent the prefix would get tmux's command prompt,
    /// and `:run-shell` from there is a shell that alc's permission ceiling
    /// and its escalation gate never see - a browser holding the operator
    /// link is deliberately less trusted than that. `Tmux::send` hands the
    /// bytes to the pane instead, where the prefix is only a byte again.
    ///
    /// Every write that carries a viewer's bytes goes through here,
    /// `permission::apply`'s mode changes included.
    pub(crate) fn input(&self, data: &[u8]) -> Result<()> {
        match self.tmux.as_ref() {
            Some(host) => host.tmux.send(&host.binary, data),
            None => self.pty.write(data),
        }
    }

    /// Sends composed text as one unit. When the agent has bracketed paste
    /// on, the markers tell it this is pasted rather than typed, so a
    /// multi-line prompt arrives whole instead of submitting each line.
    pub(crate) fn paste(&self, data: &str) -> Result<()> {
        let bracketed = self
            .modes
            .lock()
            .map(|modes| modes.modes().bracketed_paste)
            .unwrap_or(false);
        if bracketed {
            self.input(b"\x1b[200~")?;
            self.input(data.as_bytes())?;
            self.input(b"\x1b[201~")
        } else {
            self.input(data.as_bytes())
        }
    }

    /// Puts the session at a browser viewer's size.
    ///
    /// Honoured only for a tmux session, and that is the trade `--tmux`
    /// makes: the page owns the size, because the page is what somebody
    /// asking for `--tmux` is about to go and use. Resizing this pty is the
    /// whole mechanism - the pty's child is a `tmux attach-session` client,
    /// so the kernel raises SIGWINCH in it, the client reports its new size,
    /// and `window-size smallest` with the mirror as the only voter makes
    /// the window exactly this. One event moves the mirror and the window
    /// together, so they cannot drift apart into the mismatch that has tmux
    /// re-emitting the pane row by row.
    ///
    /// Ignored for a plain session, where there is one pty and its size
    /// belongs to the terminal that launched it (`resize_from_terminal`).
    /// Honouring a browser here is what put a shared session at a phone's
    /// width and left the user's own terminal drawing for a size it no
    /// longer had; the page draws that grid scaled to fit instead.
    ///
    /// The clamps match `tmux::create`'s `-x`/`-y`, so a phone cannot ask
    /// for a window narrower than the pane can hold.
    pub(crate) fn resize_from_viewer(&self, cols: u16, rows: u16) -> Result<()> {
        if self.tmux.is_none() {
            return Ok(());
        }
        self.set_size(cols, rows)
    }

    /// Puts the session at the local terminal's size.
    ///
    /// The mirror image of `resize_from_viewer`, and the reason there are
    /// two methods rather than one: which viewer is asking decides whether
    /// the answer is honoured, and a single `resize` could not tell them
    /// apart. The transports already can - a `ClientFrame` is a browser and
    /// a `CtlRequest` is the local `alc` process - so the distinction is
    /// made at the call sites and named here.
    ///
    /// Ignored for a tmux session, where this terminal is a tmux client in
    /// its own right and gets no vote (`tmux::attach_argv`). In practice it
    /// is never called for one either: `remote::attach` diverts to
    /// `attach_tmux` before the relay and its resize watcher ever start.
    pub(crate) fn resize_from_terminal(&self, cols: u16, rows: u16) -> Result<()> {
        if self.tmux.is_some() {
            return Ok(());
        }
        self.set_size(cols, rows)
    }

    fn set_size(&self, cols: u16, rows: u16) -> Result<()> {
        let cols = cols.max(20);
        let rows = rows.max(4);
        self.pty.resize(cols, rows)?;
        if let Ok(mut vt) = self.vt.lock() {
            vt.resize(usize::from(cols), usize::from(rows));
        }
        if let Ok(mut size) = self.size.lock() {
            *size = (cols, rows);
        }
        Ok(())
    }

    /// Stops the agent.
    ///
    /// For a tmux session this must go through tmux. `PtyHost::kill` signals
    /// the pty's direct child, which under `--tmux` is the mirror's own
    /// `tmux attach-session` client: signalling it detaches the mirror,
    /// reports success, and leaves the agent running unattended - which
    /// would turn `alc kill`, `alc hub stop --drain` and the page's stop
    /// button into three controls that say they stopped something and did
    /// not.
    ///
    /// Two steps, and the order is the point. Ending the agent's pane leaves
    /// `remain-on-exit` holding how it died, so the card can say the session
    /// was stopped rather than shrugging at it. Stopping the server outright
    /// would destroy that answer along with the session. The watcher takes it
    /// from here: it reads the status and stops the server - either because
    /// the pane is now dead, or because `killing` says an agent that ignored
    /// the first request has had its turn.
    pub(crate) fn kill(&self) -> Result<()> {
        let Some(host) = self.tmux.as_ref() else {
            return self.pty.kill();
        };
        self.killing.store(true, Ordering::Release);
        #[cfg(unix)]
        if let Some(pid) = self.tmux_pid.lock().ok().and_then(|pid| *pid)
            && host.tmux.hangup(pid).is_ok()
        {
            return Ok(());
        }
        host.tmux.stop(&host.binary)
    }

    pub(crate) fn has_exited(&self) -> bool {
        self.exit.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    pub(crate) fn subscribe(&self) -> Subscription {
        self.fanout.subscribe()
    }

    pub(crate) fn unsubscribe(&self, id: u64) {
        self.fanout.unsubscribe(id);
    }

    pub(crate) fn notify(&self, level: NoticeLevel, message: impl Into<String>) {
        let frame = ServerFrame::Notice {
            level,
            message: message.into(),
        };
        if let Ok(rendered) = serde_json::to_string(&frame) {
            self.fanout.broadcast(Frame::Text(rendered));
        }
    }
}
