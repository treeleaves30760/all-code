//! Tracks the DEC private modes an agent's TUI has turned on.
//!
//! alc snapshots a running session for a browser that connects late by dumping
//! the terminal emulator's grid (`avt::Vt::dump()`). That dump is cells: text,
//! colours, cursor position. It is not the terminal's *mode* state, and the
//! modes are most of what makes a TUI usable. A viewer handed only the cells
//! gets a screen that looks right and behaves wrongly:
//!
//! - Without bracketed paste (`?2004`), a paste into Claude Code arrives as
//!   bare keystrokes and the first newline inside it submits the prompt — half
//!   of what the viewer meant to send goes to the model, and the rest lands in
//!   whatever the agent does next.
//! - Without mouse reporting (`?1000`/`?1002`/`?1003`, and the `?1006`
//!   encoding that carries columns past 223), clicking anything in OpenCode
//!   does nothing at all.
//! - Without the alternate screen (`?1049`), the dumped cells are painted over
//!   the viewer's scrollback instead of into the full-screen buffer the agent
//!   believes it owns.
//!
//! So the mirror runs this scanner over the same bytes it feeds the emulator,
//! and `prelude()` replays the modes ahead of the cells.
//!
//! It is deliberately not a VT parser — avt already is one, and a second full
//! implementation is a second thing to get wrong. It recognises the *shape* of
//! a CSI and of an OSC, well enough to read `CSI ? … h`/`l` and to know when a
//! string has ended, and steps over everything else without looking at it. The
//! one payload it does read is OSC 52, to notice a clipboard read request; see
//! `saw_clipboard_read`.
//!
//! Every sequence can arrive split across `feed` calls. A PTY read ends
//! wherever the kernel buffer did, so the boundary lands between the `?` and
//! the `h` often enough to matter, and the parser state therefore lives in the
//! struct rather than in a loop over one chunk.

use std::str;

/// The DEC private modes the scanner follows, in the order `prelude` replays
/// them.
///
/// The alternate screen leads: `?1049h` clears the screen it switches to, so a
/// prelude that entered it after the snapshot's cells had been written would
/// blank the very content it exists to deliver. The rest are independent of
/// each other and of order.
const TRACKED: [u16; 10] = [1049, 47, 1047, 2004, 1000, 1002, 1003, 1006, 1004, 1];

/// The three DEC modes that reach the one alternate screen buffer. They are
/// tracked separately so that `prelude` replays the one the agent actually
/// used, but they are not three screens; see `finish_csi`.
const ALT_SCREEN: [u16; 3] = [1049, 47, 1047];

/// The longest CSI worth reading: `?` plus a dozen `;`-separated mode numbers
/// is already far past anything a real program emits. Something longer is a
/// stream that has lost sync, not a sequence.
const CSI_MAX: usize = 64;

/// How much of an OSC string is kept while it is being read. The only OSC this
/// inspects is `52;<targets>;?`, complete in a handful of bytes; past this the
/// payload is a clipboard write's base64 or a window title, and is consumed
/// and dropped rather than stored.
const OSC_KEEP: usize = 64;

/// Where an OSC string stops being believable and is abandoned mid-flight.
/// Without it, a program that writes `ESC ]` and never terminates it would
/// park the scanner in the OSC state for the rest of the session, blind to
/// every mode change after that point. Abandoning is safe in the other
/// direction too: the payloads that legitimately run long are base64 or plain
/// text, neither of which contains the ESC this resynchronises on.
const OSC_ABANDON: usize = 8 * 1024;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// The modes a late-joining viewer has to be told about, summarised.
///
/// Deliberately coarser than what the scanner tracks: a viewer needs to know
/// *that* mouse input is wanted, while the prelude replays exactly which
/// reporting mode was asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ScreenModes {
    /// `?1049`, `?47` or `?1047`: the agent is drawing on the alternate
    /// screen, so the snapshot's cells belong there and not in scrollback.
    pub alt: bool,
    /// `?2004`: pasted text arrives wrapped in `ESC [ 200 ~` … `ESC [ 201 ~`,
    /// which is what stops a newline inside a paste from submitting it.
    pub bracketed_paste: bool,
    /// Any of `?1000` (clicks), `?1002` (clicks and drags) or `?1003` (all
    /// motion).
    pub mouse: bool,
    /// `?1006`: SGR mouse encoding. Set independently of the modes above, and
    /// meaningless without one of them.
    pub mouse_sgr: bool,
    /// `?1004`: the agent asked to be told when the terminal gains or loses
    /// focus.
    pub focus: bool,
    /// `?1` (DECCKM): cursor keys send `ESC O A` rather than `ESC [ A`.
    pub app_cursor: bool,
}

/// Where the scanner sits inside an escape sequence, carried across `feed`
/// calls because a chunk boundary can fall on any byte.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// Saw ESC; the next byte decides what kind of sequence this is.
    Escape,
    /// Inside `ESC [ …`, collecting parameter and intermediate bytes.
    Csi,
    /// Inside `ESC ] …`, collecting the string until BEL or ST.
    Osc,
    /// Saw ESC inside an OSC string: a `\` makes it ST and ends the string,
    /// anything else means the emitter dropped the string mid-flight.
    OscEscape,
    /// Inside a DCS, SOS, PM or APC string (`ESC P`, `ESC X`, `ESC ^`,
    /// `ESC _`). Its payload is not inspected, only consumed: a real
    /// terminal treats everything up to ST as opaque, so a `CSI ? 1049 h`
    /// appearing inside a sixel image or a kitty graphics command must NOT
    /// be applied. Scanning it as ordinary output would put a late-joining
    /// viewer on an alternate screen the agent never entered.
    Str,
    /// Saw ESC inside such a string; `\` ends it.
    StrEscape,
}

/// An incremental scanner for the private modes, run alongside the emulator.
#[derive(Debug, Default)]
pub(crate) struct ModeScanner {
    state: State,
    /// Whether each `TRACKED` mode is currently set, by the same index. Kept
    /// per mode rather than as a `ScreenModes` so that a TUI which asked for
    /// `?1003` gets `?1003` back from `prelude` and not a weaker stand-in.
    on: [bool; TRACKED.len()],
    /// Everything between `ESC [` and the final byte of the CSI being read,
    /// the leading `?` included. Never longer than `CSI_MAX`.
    csi: Vec<u8>,
    /// The first `OSC_KEEP` bytes of the OSC string being read.
    osc: Vec<u8>,
    /// How long that string has actually run, counting what `osc` dropped.
    osc_len: usize,
    clipboard_read: bool,
}

impl ModeScanner {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feeds one read from the PTY. Sequences may be split across calls at any
    /// byte and are still recognised.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.step(byte);
        }
    }

    pub(crate) fn modes(&self) -> ScreenModes {
        ScreenModes {
            alt: ALT_SCREEN.iter().any(|&mode| self.is_on(mode)),
            bracketed_paste: self.is_on(2004),
            mouse: self.is_on(1000) || self.is_on(1002) || self.is_on(1003),
            mouse_sgr: self.is_on(1006),
            focus: self.is_on(1004),
            app_cursor: self.is_on(1),
        }
    }

    /// The escape sequences that put a freshly-opened terminal into the modes
    /// currently set, emitted before a snapshot's cell content.
    ///
    /// Only `h` (set) sequences appear. Every tracked mode is off in a terminal
    /// that has just opened, so a mode this scanner never saw set needs nothing
    /// said about it, and a mode that was set and then reset needs nothing
    /// either.
    pub(crate) fn prelude(&self) -> String {
        let mut out = String::new();
        for (mode, on) in TRACKED.iter().zip(self.on) {
            if on {
                out.push_str(&format!("\x1b[?{mode}h"));
            }
        }
        out
    }

    /// True once the stream has asked to READ the clipboard (OSC 52 with a `?`
    /// payload). alc never answers one: the reply is injected into the agent's
    /// stdin, so a hostile file the agent merely printed could exfiltrate the
    /// viewer's clipboard into the model's context. The session surfaces this
    /// as a warning.
    pub(crate) fn saw_clipboard_read(&self) -> bool {
        self.clipboard_read
    }

    fn is_on(&self, mode: u16) -> bool {
        index_of(mode).is_some_and(|index| self.on[index])
    }

    fn step(&mut self, byte: u8) {
        match self.state {
            State::Ground => {
                if byte == ESC {
                    self.state = State::Escape;
                }
            }
            State::Escape => self.step_escape(byte),
            State::Csi => self.step_csi(byte),
            State::Osc => self.step_osc(byte),
            State::OscEscape => {
                if byte == b'\\' {
                    self.finish_osc();
                } else {
                    // The emitter abandoned the string. The byte is re-read as
                    // the one after an ESC, which is what recovers a
                    // `CSI ? … h` written straight after an unterminated OSC.
                    self.discard_osc();
                    self.step_escape(byte);
                }
            }
            State::Str => self.step_str(byte),
            State::StrEscape => {
                if byte == b'\\' {
                    self.discard_osc();
                } else {
                    self.discard_osc();
                    self.step_escape(byte);
                }
            }
        }
    }

    fn step_escape(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.csi.clear();
                self.state = State::Csi;
            }
            b']' => {
                self.osc.clear();
                self.osc_len = 0;
                self.state = State::Osc;
            }
            // DCS, SOS, PM and APC all carry an opaque payload terminated
            // by ST. Consumed without inspection; see `State::Str`.
            b'P' | b'X' | b'^' | b'_' => {
                self.osc.clear();
                self.osc_len = 0;
                self.state = State::Str;
            }
            // A second ESC restarts: the first one introduced nothing.
            ESC => self.state = State::Escape,
            // Two-byte sequences (`ESC 7`, `ESC =`, a stray ST) and every
            // other introducer alc does not track.
            _ => self.state = State::Ground,
        }
    }

    fn step_csi(&mut self, byte: u8) {
        match byte {
            // Intermediate (0x20-0x2f) and parameter (0x30-0x3f) bytes.
            0x20..=0x3f => {
                if self.csi.len() < CSI_MAX {
                    self.csi.push(byte);
                } else {
                    self.discard_csi();
                }
            }
            // A final byte ends the sequence whether or not alc cares about it.
            0x40..=0x7e => {
                self.finish_csi(byte);
                self.discard_csi();
            }
            ESC => self.state = State::Escape,
            // A C0 control or an 8-bit byte in the middle of a CSI: a real
            // terminal executes the control and reads on, but whatever this
            // was, it was not a mode change.
            _ => self.discard_csi(),
        }
    }

    /// Applies `CSI ? <params> h` (set) and `CSI ? <params> l` (reset). One
    /// sequence carries any number of `;`-separated modes, which is how a TUI
    /// turns on mouse reporting and its encoding in a single write.
    fn finish_csi(&mut self, final_byte: u8) {
        let enable = match final_byte {
            b'h' => true,
            b'l' => false,
            _ => return,
        };
        // Without the `?` this is an ANSI mode — `CSI 4 h` is insert mode —
        // which shares the numbers with the private set and means something
        // else entirely.
        let Some(params) = self.csi.strip_prefix(b"?") else {
            return;
        };
        for mode in params.split(|&byte| byte == b';').filter_map(parse_mode) {
            // `params` borrows `self.csi` across the whole loop, so this
            // touches `self.on` directly rather than through a `&mut self`
            // method the borrow checker would reject.
            if let Some(index) = index_of(mode) {
                self.on[index] = enable;
            }
            // A reset through any one of ?47/?1047/?1049 returns to the normal
            // screen whichever of them the program entered through, so the
            // whole group clears together. Treating them as three independent
            // flags would strand a late viewer on the alternate screen for the
            // rest of the session whenever a TUI enters with ?1049h and leaves
            // with ?47l.
            if !enable && ALT_SCREEN.contains(&mode) {
                for alt in ALT_SCREEN {
                    if let Some(index) = index_of(alt) {
                        self.on[index] = false;
                    }
                }
            }
        }
    }

    fn step_osc(&mut self, byte: u8) {
        match byte {
            BEL => self.finish_osc(),
            ESC => self.state = State::OscEscape,
            _ => {
                self.osc_len += 1;
                if self.osc_len > OSC_ABANDON {
                    self.discard_osc();
                } else if self.osc.len() < OSC_KEEP {
                    self.osc.push(byte);
                }
            }
        }
    }

    /// Consumes a DCS/SOS/PM/APC payload, sharing the OSC buffer's bound so
    /// an unterminated one cannot grow without limit either.
    fn step_str(&mut self, byte: u8) {
        match byte {
            BEL => self.discard_osc(),
            ESC => self.state = State::StrEscape,
            _ => {
                self.osc_len += 1;
                if self.osc_len > OSC_ABANDON {
                    self.discard_osc();
                }
            }
        }
    }

    fn finish_osc(&mut self) {
        if is_clipboard_read(&self.osc) {
            self.clipboard_read = true;
        }
        self.discard_osc();
    }

    /// Drops the OSC string in progress and returns to ground. Also the way a
    /// completed string is retired, so the buffer is empty for the next one.
    fn discard_osc(&mut self) {
        self.osc.clear();
        self.osc_len = 0;
        self.state = State::Ground;
    }

    fn discard_csi(&mut self) {
        self.csi.clear();
        self.state = State::Ground;
    }
}

fn index_of(mode: u16) -> Option<usize> {
    TRACKED.iter().position(|&tracked| tracked == mode)
}

/// A DEC private mode number, or `None` for an empty or non-numeric parameter.
/// Sub-parameters (`:`-separated) and colour arguments are not modes, so
/// failing to parse them is the outcome that is wanted.
fn parse_mode(param: &[u8]) -> Option<u16> {
    str::from_utf8(param).ok()?.parse().ok()
}

/// `OSC 52 ; <targets> ; <data>`: a `<data>` of `?` asks the terminal to send
/// its clipboard back, anything else is a write (base64, which cannot contain
/// a `?`). Truncation at `OSC_KEEP` can only lose a write's payload — a read
/// request is six bytes — so it never turns a write into a false positive.
fn is_clipboard_read(payload: &[u8]) -> bool {
    payload
        .strip_prefix(b"52;")
        .and_then(|rest| rest.split(|&byte| byte == b';').nth(1))
        == Some(b"?".as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(bytes: &[u8]) -> ModeScanner {
        let mut scanner = ModeScanner::new();
        scanner.feed(bytes);
        scanner
    }

    #[test]
    fn a_fresh_scanner_reports_every_mode_off() {
        assert_eq!(ModeScanner::new().modes(), ScreenModes::default());
    }

    #[test]
    fn every_tracked_mode_is_set_by_its_own_sequence_and_cleared_by_the_reset() {
        for mode in TRACKED {
            let mut scanner = scan(format!("\x1b[?{mode}h").as_bytes());
            assert!(scanner.is_on(mode), "?{mode}h did not set the mode");
            scanner.feed(format!("\x1b[?{mode}l").as_bytes());
            assert!(!scanner.is_on(mode), "?{mode}l did not reset the mode");
        }
    }

    #[test]
    fn each_summary_field_reports_the_modes_that_feed_it() {
        assert!(scan(b"\x1b[?1049h").modes().alt);
        assert!(scan(b"\x1b[?47h").modes().alt);
        assert!(scan(b"\x1b[?1047h").modes().alt);
        assert!(scan(b"\x1b[?2004h").modes().bracketed_paste);
        assert!(scan(b"\x1b[?1000h").modes().mouse);
        assert!(scan(b"\x1b[?1002h").modes().mouse);
        assert!(scan(b"\x1b[?1003h").modes().mouse);
        assert!(scan(b"\x1b[?1006h").modes().mouse_sgr);
        assert!(scan(b"\x1b[?1004h").modes().focus);
        assert!(scan(b"\x1b[?1h").modes().app_cursor);
    }

    // The three alternate-screen modes reach one buffer between them, so a TUI
    // that enters through one door and leaves through another is still out.
    #[test]
    fn a_reset_through_any_alternate_screen_mode_leaves_the_alternate_screen() {
        for enter in ALT_SCREEN {
            for leave in ALT_SCREEN {
                let mut scanner = scan(format!("\x1b[?{enter}h").as_bytes());
                scanner.feed(format!("\x1b[?{leave}l").as_bytes());
                assert!(!scanner.modes().alt, "entered ?{enter}h, left ?{leave}l");
            }
        }
    }

    // How a TUI actually turns mouse tracking on: one write, several modes.
    #[test]
    fn one_csi_carrying_several_parameters_sets_all_of_them() {
        assert_eq!(
            scan(b"\x1b[?1000;1006;2004h").modes(),
            ScreenModes {
                bracketed_paste: true,
                mouse: true,
                mouse_sgr: true,
                ..ScreenModes::default()
            }
        );
    }

    #[test]
    fn one_csi_carrying_several_parameters_resets_all_of_them() {
        let mut scanner = scan(b"\x1b[?1000;1006;2004h");
        scanner.feed(b"\x1b[?1000;1006;2004l");
        assert_eq!(scanner.modes(), ScreenModes::default());
    }

    // The bug this type exists to avoid: a PTY read ends wherever the kernel
    // buffer did, including between the `?` and the `h`.
    #[test]
    fn a_csi_split_across_two_feeds_at_any_byte_is_still_recognised() {
        let sequence = b"\x1b[?2004h";
        for split in 1..sequence.len() {
            let mut scanner = ModeScanner::new();
            scanner.feed(&sequence[..split]);
            scanner.feed(&sequence[split..]);
            assert!(
                scanner.modes().bracketed_paste,
                "split after {split} byte(s)"
            );
        }
    }

    #[test]
    fn a_multi_parameter_csi_split_across_two_feeds_sets_every_mode_in_it() {
        let sequence = b"\x1b[?1002;1006h";
        for split in 1..sequence.len() {
            let mut scanner = ModeScanner::new();
            scanner.feed(&sequence[..split]);
            scanner.feed(&sequence[split..]);
            let modes = scanner.modes();
            assert!(
                modes.mouse && modes.mouse_sgr,
                "split after {split} byte(s)"
            );
        }
    }

    #[test]
    fn a_startup_burst_fed_one_byte_at_a_time_reaches_the_modes_it_asked_for() {
        let startup = b"\x1b[?1049h\x1b[?1h\x1b[2J\x1b[H\x1b[?2004h\x1b[?1002;1006h";
        let mut scanner = ModeScanner::new();
        for byte in startup {
            scanner.feed(&[*byte]);
        }
        assert_eq!(
            scanner.modes(),
            ScreenModes {
                alt: true,
                bracketed_paste: true,
                mouse: true,
                mouse_sgr: true,
                focus: false,
                app_cursor: true,
            }
        );
    }

    // `CSI 1 h` is an ANSI mode; only `CSI ? 1 h` is DECCKM.
    #[test]
    fn a_mode_without_the_private_marker_is_not_a_dec_private_mode() {
        assert!(!scan(b"\x1b[1h").modes().app_cursor);
    }

    // DECRQM asks whether the mode is set. It must not set it.
    #[test]
    fn a_private_sequence_ending_in_another_final_byte_sets_nothing() {
        assert!(!scan(b"\x1b[?2004$p").modes().bracketed_paste);
    }

    #[test]
    fn ordinary_output_and_untracked_sequences_leave_every_mode_alone() {
        let scanner = scan(b"\x1b[0;1;38;5;213mbuilding\x1b[m\r\n\x1b[2J\x1b]0;alc\x07$ ");
        assert_eq!(scanner.modes(), ScreenModes::default());
    }

    #[test]
    fn an_empty_parameter_is_skipped_without_disturbing_its_neighbours() {
        assert!(scan(b"\x1b[?;2004h").modes().bracketed_paste);
    }

    #[test]
    fn an_osc_52_read_request_raises_the_clipboard_flag() {
        assert!(scan(b"\x1b]52;c;?\x07").saw_clipboard_read());
    }

    #[test]
    fn an_osc_52_write_does_not_raise_the_clipboard_flag() {
        assert!(!scan(b"\x1b]52;c;aGVsbG8sIHdvcmxk\x07").saw_clipboard_read());
    }

    // ST rather than BEL is the other legal terminator, and the one xterm's
    // own documentation uses.
    #[test]
    fn an_osc_52_read_terminated_by_st_raises_the_clipboard_flag() {
        assert!(scan(b"\x1b]52;c;?\x1b\\").saw_clipboard_read());
    }

    #[test]
    fn an_osc_split_across_two_feeds_at_any_byte_is_still_recognised() {
        let sequence = b"\x1b]52;c;?\x07";
        for split in 1..sequence.len() {
            let mut scanner = ModeScanner::new();
            scanner.feed(&sequence[..split]);
            scanner.feed(&sequence[split..]);
            assert!(scanner.saw_clipboard_read(), "split after {split} byte(s)");
        }
    }

    // A title that never terminates must not turn the scanner into a slow
    // memory leak.
    #[test]
    fn an_unterminated_osc_does_not_grow_the_buffer_without_bound() {
        let mut scanner = ModeScanner::new();
        scanner.feed(b"\x1b]0;");
        for _ in 0..64 {
            scanner.feed(&[b'x'; 4096]);
        }
        assert!(
            scanner.osc.len() <= OSC_KEEP,
            "kept {} bytes",
            scanner.osc.len()
        );
    }

    // ...nor leave it parked in the OSC state, deaf to every later mode.
    #[test]
    fn the_scanner_resynchronises_after_an_osc_that_is_never_terminated() {
        let mut scanner = ModeScanner::new();
        scanner.feed(b"\x1b]0;");
        for _ in 0..64 {
            scanner.feed(&[b'x'; 4096]);
        }
        scanner.feed(b"\x1b[?2004h");
        assert!(scanner.modes().bracketed_paste);
    }

    // An emitter that starts an OSC and then writes something else without
    // terminating it must not swallow what follows.
    #[test]
    fn a_csi_written_straight_after_an_abandoned_osc_is_still_read() {
        assert!(scan(b"\x1b]0;title\x1b[?2004h").modes().bracketed_paste);
    }

    #[test]
    fn a_csi_longer_than_the_cap_is_abandoned_without_growing_the_buffer() {
        let mut scanner = ModeScanner::new();
        scanner.feed(b"\x1b[?");
        scanner.feed(&[b'1'; CSI_MAX * 8]);
        assert!(
            scanner.csi.len() <= CSI_MAX,
            "kept {} bytes",
            scanner.csi.len()
        );
    }

    #[test]
    fn a_prelude_replayed_into_a_fresh_scanner_reaches_the_exact_same_modes() {
        let live = scan(b"\x1b[?1049h\x1b[?1h\x1b[?2004h\x1b[?1003;1006h\x1b[?1004h");
        let replayed = scan(live.prelude().as_bytes());
        assert_eq!(replayed.on, live.on);
    }

    // The summary flattens ?1003 to `mouse`; the prelude must not, or a viewer
    // joining an OpenCode session loses motion reporting.
    #[test]
    fn the_prelude_replays_the_mouse_mode_the_agent_actually_asked_for() {
        assert_eq!(
            scan(b"\x1b[?1003;1006h").prelude(),
            "\x1b[?1003h\x1b[?1006h"
        );
    }

    // ?1049h clears the screen it switches to, so it has to precede the
    // snapshot's cells, and therefore everything else in the prelude.
    #[test]
    fn the_prelude_enters_the_alternate_screen_before_anything_else() {
        let prelude = scan(b"\x1b[?2004h\x1b[?1049h").prelude();
        assert!(prelude.starts_with("\x1b[?1049h"), "{prelude:?}");
    }

    #[test]
    fn a_mode_that_was_set_and_reset_again_is_absent_from_the_prelude() {
        assert!(scan(b"\x1b[?2004h\x1b[?2004l").prelude().is_empty());
    }
}

#[cfg(test)]
mod string_payload_tests {
    use super::*;

    /// The scanner runs alongside `avt`, and what matters is that the two
    /// agree: a mode `avt` applied but the scanner missed would be dropped
    /// from a late joiner's snapshot, and one the scanner invented would be
    /// replayed onto a screen the agent never asked for. `avt`'s parser
    /// takes `(_, ESC) => Escape` from every state, so these tests pin the
    /// scanner to that reading rather than to a stricter one.
    #[test]
    fn a_string_payload_with_no_escape_cannot_set_a_mode() {
        // A kitty graphics command or a sixel image carries arbitrary bytes.
        // Without a string state these were scanned as ordinary output.
        for introducer in *b"PX^_" {
            let mut scanner = ModeScanner::new();
            let mut stream = vec![ESC, introducer];
            stream.extend_from_slice(b"Gf=100;[?1049h[?2004h");
            stream.extend_from_slice(&[ESC, b'\\']);
            scanner.feed(&stream);
            let modes = scanner.modes();
            assert!(
                !modes.alt && !modes.bracketed_paste,
                "ESC {} payload leaked",
                char::from(introducer)
            );
        }
    }

    /// Deliberately asserts the permissive reading, because it is `avt`'s.
    /// A terminal that consumed this would leave the scanner and the
    /// emulator holding different ideas of the screen.
    #[test]
    fn an_escape_inside_a_string_aborts_it_exactly_as_the_emulator_does() {
        let mut scanner = ModeScanner::new();
        scanner.feed(b"\x1bPq\x1b[?1049h\x1b\\");
        assert!(scanner.modes().alt);
    }

    #[test]
    fn a_mode_after_a_terminated_string_is_applied_normally() {
        let mut scanner = ModeScanner::new();
        scanner.feed(b"\x1bPq;;;\x1b\\\x1b[?2004h");
        assert!(scanner.modes().bracketed_paste);
    }

    #[test]
    fn a_string_split_across_feeds_is_still_consumed() {
        let mut scanner = ModeScanner::new();
        scanner.feed(b"\x1b_Gf=100;dat");
        scanner.feed(b"a[?1049h mor");
        scanner.feed(b"e\x1b\\\x1b[?1h");
        assert!(!scanner.modes().alt, "the payload leaked through a split");
        assert!(scanner.modes().app_cursor, "the sequence after ST was lost");
    }

    #[test]
    fn an_unterminated_string_does_not_grow_without_bound() {
        let mut scanner = ModeScanner::new();
        scanner.feed(&[ESC, b'P']);
        scanner.feed(&vec![b'x'; OSC_ABANDON * 2]);
        assert!(scanner.osc.len() <= OSC_KEEP);
        // Abandoned, so the scanner is back in ground and usable again.
        scanner.feed(b"\x1b[?2004h");
        assert!(scanner.modes().bracketed_paste);
    }

    #[test]
    fn a_bel_terminated_string_also_ends() {
        let mut scanner = ModeScanner::new();
        scanner.feed(b"\x1b_Gf=100;payload\x07\x1b[?2004h");
        assert!(scanner.modes().bracketed_paste);
    }
}
