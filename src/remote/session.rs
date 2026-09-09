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
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config::Agent;
use crate::launch::{LaunchSpec, SessionGuards};
use crate::remote::caps::{Confidence, caps};
use crate::remote::fanout::{Fanout, Frame, Subscription};
use crate::remote::permission::{PermState, probe};
use crate::remote::pty::PtyHost;
use crate::remote::ring::SeqRing;
use crate::remote::screen::ModeScanner;
use crate::remote::scrub::SecretScrubber;
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
    seq: AtomicU64,
    warned_clipboard: AtomicBool,

    /// Filled once, by the pump thread, when the agent is gone.
    ///
    /// The pump is the only caller of `PtyHost::wait`, which is only safe
    /// once the reader has reached EOF - see the note on that method.
    exit: Mutex<Option<ExitInfo>>,

    /// Held for its `Drop`: the Codex bridge and any temporary config written
    /// for this launch.
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
        program: &Path,
        spec: LaunchSpec,
        cwd: &Path,
        guards: SessionGuards,
    ) -> Result<Arc<Self>> {
        let SessionSpec {
            id,
            name,
            cols,
            rows,
            scrollback_bytes,
            permission: _,
        } = session;
        let (pty, reader) = PtyHost::spawn(program, &spec, cwd, cols, rows)?;

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

        Ok(session)
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

        let exit = self.pty.wait().unwrap_or(ExitInfo {
            code: None,
            signal: None,
        });
        if let Ok(mut slot) = self.exit.lock() {
            *slot = Some(exit.clone());
        }
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
        let screen = self.vt.lock().map(|vt| vt.text()).unwrap_or_default();
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
            pid: self.pty.process_id(),
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

    pub(crate) fn input(&self, data: &[u8]) -> Result<()> {
        self.pty.write(data)
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
            self.pty.write(b"\x1b[200~")?;
            self.pty.write(data.as_bytes())?;
            self.pty.write(b"\x1b[201~")
        } else {
            self.pty.write(data.as_bytes())
        }
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16) -> Result<()> {
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

    pub(crate) fn kill(&self) -> Result<()> {
        self.pty.kill()
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
