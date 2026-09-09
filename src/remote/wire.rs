//! The types the browser and the session server exchange.
//!
//! Two encodings share one WebSocket. Terminal output is BINARY with a
//! one-byte opcode and an 8-byte sequence number, because it is the only
//! high-volume traffic and JSON-escaping raw escape sequences would roughly
//! double it. Everything else is a JSON text frame, which stays readable in
//! a browser's network panel and costs nothing at control-message rates.
//!
//! The sequence number counts bytes ever written to the session, so a client
//! that reconnects can name exactly where it stopped. See `ring::SeqRing`
//! for what happens when that point has already been evicted.

use serde::{Deserialize, Serialize};

/// Incremental terminal output: `[0x01][u64 big-endian seq][raw bytes]`.
/// `seq` is the sequence AFTER these bytes, so a client stores it directly.
pub(crate) const OP_OUTPUT: u8 = 0x01;

/// A full screen restore: `[0x02][u64 big-endian seq][utf8]`. Sent instead
/// of a delta when the client's cursor is no longer in the buffer, and on a
/// first connection.
pub(crate) const OP_SNAPSHOT: u8 = 0x02;

/// Frames the browser sends. Every one is a JSON text frame; `Auth` must be
/// the first, and a connection that sends anything else first is closed
/// without a reply.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(crate) enum ClientFrame {
    Auth {
        token: String,
        #[serde(default)]
        cols: u16,
        #[serde(default)]
        rows: u16,
        /// The last sequence this client saw, when reconnecting.
        #[serde(default)]
        since: Option<u64>,
    },
    /// Keystrokes, forwarded to the pty verbatim.
    Input {
        data: String,
    },
    /// A block of text the user composed outside the terminal. Wrapped in
    /// bracketed-paste markers when the agent has that mode on, so a
    /// multi-line prompt does not submit itself line by line.
    Paste {
        data: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Pong,
}

/// JSON text frames the server sends. Terminal bytes go out as binary
/// instead; see `OP_OUTPUT`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(crate) enum ServerFrame {
    /// Always the first frame after a successful auth. Boxed because the
    /// card dwarfs every other variant, and this enum is passed by value on
    /// every notice and every keepalive.
    Hello(Box<Hello>),
    Notice {
        level: NoticeLevel,
        message: String,
    },
    Exit {
        code: Option<i32>,
        signal: Option<String>,
    },
    Ping,
}

/// The opening frame's payload.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Hello {
    pub session: SessionCard,
    pub grade: &'static str,
    pub seq: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SessionState {
    Running,
    Exited,
}

/// How a session ended. A signalled death and a plain `exit 1` are the same
/// byte in a process exit code, and a card that cannot tell them apart
/// reports a killed agent as a failing one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

/// One row on the session list, and the header above an attached terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionCard {
    pub id: String,
    pub name: String,
    pub agent: String,
    pub provider: String,
    pub provider_kind: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub cwd: String,
    /// The agent's own process id, so a session can be found from another
    /// terminal without hunting for it.
    pub pid: Option<u32>,
    pub state: SessionState,
    pub exit: Option<ExitInfo>,
    pub cols: u16,
    pub rows: u16,
    pub viewers: usize,
    /// Seconds since the unix epoch. Rendered locally by the browser, which
    /// knows the viewer's timezone and alc does not.
    pub started_at: u64,
    /// True when the agent has no permission model at all, so a viewer can
    /// see that nothing is gating its tool calls. Pi is the case this
    /// exists for.
    pub unsandboxed: bool,
    pub permission: crate::remote::permission::PermState,
    /// The tmux session the agent runs in, when the launch asked for one.
    ///
    /// On the card rather than kept in the hub because it is what a terminal
    /// needs in order to attach as its own tmux client - which is the entire
    /// point of `--tmux`, and something only the client can do for itself.
    /// It also lets `alc sessions` mark which sessions detach with tmux's
    /// prefix rather than with alc's own key, before the user is inside one.
    ///
    /// The socket is a unix socket in the user's own 0700 directory, so the
    /// browser it is also sent to can do nothing with it.
    #[serde(default)]
    pub tmux: Option<crate::remote::tmux::Tmux>,
}

/// Encodes a binary output frame: opcode, sequence, then the payload.
pub(crate) fn encode_binary(opcode: u8, seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(9 + payload.len());
    frame.push(opcode);
    frame.extend_from_slice(&seq.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_auth_frame_parses_without_the_optional_fields() {
        let frame: ClientFrame = serde_json::from_str(r#"{"t":"auth","token":"x"}"#).unwrap();
        match frame {
            ClientFrame::Auth {
                token,
                cols,
                rows,
                since,
            } => {
                assert_eq!(token, "x");
                assert_eq!((cols, rows), (0, 0));
                assert_eq!(since, None);
            }
            other => panic!("expected auth, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_client_frame_is_rejected_rather_than_ignored() {
        // A typo in a control message must not silently do nothing.
        assert!(serde_json::from_str::<ClientFrame>(r#"{"t":"detonate"}"#).is_err());
    }

    #[test]
    fn a_server_frame_carries_its_tag() {
        let rendered = serde_json::to_string(&ServerFrame::Ping).unwrap();
        assert_eq!(rendered, r#"{"t":"ping"}"#);
    }

    #[test]
    fn a_binary_frame_puts_the_sequence_in_network_order() {
        let frame = encode_binary(OP_OUTPUT, 0x0102_0304_0506_0708, b"hi");
        assert_eq!(frame[0], OP_OUTPUT);
        assert_eq!(&frame[1..9], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&frame[9..], b"hi");
    }
}
