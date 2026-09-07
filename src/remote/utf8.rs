//! Turning a pseudo-terminal's byte stream into `&str` chunks.
//!
//! A PTY read hands back whatever bytes happened to be in the kernel buffer,
//! which is routinely the middle of a multi-byte character: an agent drawing a
//! box-drawing frame or an emoji spinner straddles read boundaries constantly.
//! The terminal emulator on the other side of the mirror (`avt::Vt::feed_str`)
//! takes `&str`, so something has to decode, and decoding each read on its own
//! — with `String::from_utf8_lossy`, say — would burn a replacement character
//! into the emulator's grid for every character unlucky enough to land on a
//! boundary. That corruption is permanent: the grid is what the browser is
//! served later, by which point the original bytes are gone.
//!
//! `Utf8Chunker` holds the straddling bytes back until the rest of the
//! character arrives, while still making progress on genuinely malformed
//! input, so that one bad byte can never wedge the stream.

use std::str;

/// Reassembles complete characters out of arbitrarily split byte chunks.
///
/// Both buffers live in the struct rather than being returned by value:
/// `push` hands out a borrow of `decoded`, which therefore has to outlive the
/// call, and `carry` is the whole point of the type.
pub(crate) struct Utf8Chunker {
    /// What the most recent call decoded. Cleared at the top of every call,
    /// so a slice handed out earlier must not be held across the next one.
    decoded: String,
    /// Trailing bytes of an earlier chunk that begin, but do not finish, one
    /// character. Never more than 3 bytes — the longest proper prefix of a
    /// 4-byte sequence — and empty whenever the stream sits on a character
    /// boundary, which is the common case.
    carry: Vec<u8>,
}

impl Utf8Chunker {
    pub(crate) fn new() -> Self {
        Self {
            decoded: String::new(),
            carry: Vec::new(),
        }
    }

    /// Decodes as much of the held-back bytes plus `chunk` as forms complete
    /// characters, retaining an incomplete trailing sequence for the next
    /// call. The returned slice borrows internal storage and stays valid only
    /// until the next `push` or `flush`.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> &str {
        self.decoded.clear();
        if self.carry.is_empty() {
            // The overwhelmingly common case: nothing straddles the boundary,
            // so the chunk is decoded in place instead of being copied first.
            self.decode(chunk);
        } else {
            // The rest of a split character is in this chunk, and the two
            // halves only mean anything validated as one run of bytes. Taking
            // `carry` leaves it empty for `decode` to refill.
            let mut joined = std::mem::take(&mut self.carry);
            joined.extend_from_slice(chunk);
            self.decode(&joined);
        }
        &self.decoded
    }

    /// Decodes whatever is still held back. Anything there is a sequence the
    /// stream ended in the middle of, which can never be completed now, so it
    /// becomes a single replacement character. Called once when the stream
    /// ends; a second call yields an empty slice.
    pub(crate) fn flush(&mut self) -> &str {
        self.decoded.clear();
        if !self.carry.is_empty() {
            self.decoded.push(char::REPLACEMENT_CHARACTER);
            self.carry.clear();
        }
        &self.decoded
    }

    /// Appends every complete character in `bytes` to `decoded` and leaves the
    /// incomplete trailing sequence, if any, in `carry`.
    ///
    /// `Utf8Error::error_len` is the discriminator the whole type turns on:
    /// `None` means the input merely ran out mid-sequence and more bytes could
    /// still complete it, so those bytes are carried; `Some(len)` means the
    /// sequence is ill-formed and no continuation could ever rescue it, so it
    /// is replaced and decoding resumes just past it. Carrying an ill-formed
    /// prefix instead would stall the mirror on the next 0xff a program emits.
    ///
    /// `bytes` must not alias `carry`, which is why `push` takes the carry out
    /// before joining.
    fn decode(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        loop {
            let error = match str::from_utf8(rest) {
                Ok(text) => {
                    self.decoded.push_str(text);
                    self.carry.clear();
                    return;
                }
                Err(error) => error,
            };
            let (valid, invalid) = rest.split_at(error.valid_up_to());
            // `valid_up_to` is by definition a character boundary, so this
            // borrows the prefix and replaces nothing.
            self.decoded.push_str(&String::from_utf8_lossy(valid));
            match error.error_len() {
                Some(len) => {
                    self.decoded.push(char::REPLACEMENT_CHARACTER);
                    rest = &invalid[len..];
                }
                None => {
                    self.carry.clear();
                    self.carry.extend_from_slice(invalid);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pushes each chunk in turn and appends the flush, the way the mirror
    /// drives a chunker across the whole life of a session.
    fn decode_chunks(chunks: &[&[u8]]) -> String {
        let mut chunker = Utf8Chunker::new();
        let mut out = String::new();
        for chunk in chunks {
            out.push_str(chunker.push(chunk));
        }
        out.push_str(chunker.flush());
        out
    }

    #[test]
    fn ascii_passes_through_a_single_push_unchanged() {
        let mut chunker = Utf8Chunker::new();
        assert_eq!(chunker.push(b"\x1b[2J$ ls -la\r\n"), "\x1b[2J$ ls -la\r\n");
    }

    #[test]
    fn a_four_byte_emoji_split_at_any_interior_boundary_is_rejoined() {
        let bytes = "🌀".as_bytes();
        for split in 1..bytes.len() {
            let decoded = decode_chunks(&[&bytes[..split], &bytes[split..]]);
            assert_eq!(decoded, "🌀", "split after {split} byte(s)");
        }
    }

    #[test]
    fn a_three_byte_character_split_at_either_boundary_is_rejoined() {
        let bytes = "中".as_bytes();
        for split in 1..bytes.len() {
            let decoded = decode_chunks(&[&bytes[..split], &bytes[split..]]);
            assert_eq!(decoded, "中", "split after {split} byte(s)");
        }
    }

    #[test]
    fn a_lone_continuation_byte_becomes_one_replacement_character() {
        let mut chunker = Utf8Chunker::new();
        assert_eq!(chunker.push(b"a\x80b"), "a\u{fffd}b");
    }

    // 0xff can never begin a sequence, so holding it back in the hope that a
    // continuation byte rescues it would stop the stream dead.
    #[test]
    fn an_invalid_leading_byte_is_replaced_without_stalling_the_stream() {
        let mut chunker = Utf8Chunker::new();
        assert_eq!(chunker.push(b"\xff\xffok"), "\u{fffd}\u{fffd}ok");
    }

    // The carried bytes looked like the opening of an emoji until the next
    // read arrived and ruled it out; they must be replaced at that point
    // rather than carried any further.
    #[test]
    fn a_held_back_prefix_that_the_next_read_contradicts_is_replaced() {
        let mut chunker = Utf8Chunker::new();
        chunker.push(b"\xf0\x9f");
        assert_eq!(chunker.push(b"A"), "\u{fffd}A");
    }

    #[test]
    fn a_sequence_truncated_by_the_end_of_the_stream_is_replaced_at_flush() {
        let mut chunker = Utf8Chunker::new();
        // The first two bytes of 🌀; the agent exited before the rest came.
        chunker.push(b"hi\xf0\x9f");
        assert_eq!(chunker.flush(), "\u{fffd}");
    }

    #[test]
    fn the_bytes_before_a_truncated_sequence_are_decoded_immediately() {
        let mut chunker = Utf8Chunker::new();
        assert_eq!(chunker.push(b"hi\xf0\x9f"), "hi");
    }

    #[test]
    fn flush_is_empty_when_the_stream_ended_on_a_character_boundary() {
        let mut chunker = Utf8Chunker::new();
        chunker.push("中".as_bytes());
        assert!(chunker.flush().is_empty());
    }

    // The bound a caller relies on: a chunker fed forever holds a fixed
    // amount of state, whatever the split.
    #[test]
    fn no_more_than_three_bytes_are_ever_held_back() {
        for prefix in [
            b"\xc3".as_slice(),
            b"\xe4\xb8".as_slice(),
            b"\xf0".as_slice(),
            b"\xf0\x9f".as_slice(),
            b"\xf0\x9f\x8c".as_slice(),
        ] {
            let mut chunker = Utf8Chunker::new();
            chunker.push(prefix);
            assert!(chunker.carry.len() <= 3, "held back {:?}", chunker.carry);
        }
    }

    #[test]
    fn feeding_one_byte_at_a_time_reconstructs_the_original_text() {
        let original = "héllo 中文 🌀 ✓ ok\r\n$ ";
        let chunks: Vec<&[u8]> = original.as_bytes().chunks(1).collect();
        assert_eq!(decode_chunks(&chunks), original);
    }
}
