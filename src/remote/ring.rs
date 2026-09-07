//! A bounded byte ring that remembers how many bytes have ever passed through
//! it.
//!
//! A browser watching a session drops its connection and comes back saying "I
//! last saw byte N". When the bytes after N are still buffered, replaying
//! exactly that delta restores the view for the cost of the delta itself. When
//! the session has since produced more output than the ring holds, those bytes
//! are gone and the client has to be handed a full screen snapshot instead.
//!
//! Telling those two cases apart is the whole job of this module: `since`
//! answers `Some` for the cheap path and `None` for the expensive one, and the
//! caller never has to reason about the buffer's geometry to decide which it
//! is looking at.

use std::collections::VecDeque;

/// The most recent bytes of a session, tagged with the running total of bytes
/// ever pushed.
///
/// Two invariants hold after every operation, and `since` is written against
/// them rather than re-deriving them:
/// - `buffer.len() <= capacity`, so the ring never outgrows its budget;
/// - `seq == first_seq() + buffer.len()`, so the buffer is exactly the
///   half-open byte range `first_seq()..seq` of the stream.
///
/// `seq` counts bytes - not writes, not messages - and is never reset. A
/// client hands the number back verbatim on reconnect, so a reset would make
/// one cursor name two different positions in the stream and replay the wrong
/// bytes. At u64 even a gigabyte a second takes longer than the hardware to
/// wrap, so there is deliberately no wrap handling here.
pub(crate) struct SeqRing {
    buffer: VecDeque<u8>,
    capacity: usize,
    seq: u64,
}

impl SeqRing {
    /// `capacity` is a byte budget, not a message count: the ring keeps the
    /// last `capacity` bytes of the stream however many writes they arrived
    /// in. The budget is reserved up front, because a ring that grows into its
    /// limit is a ring whose real memory cost is not the number it was
    /// configured with.
    ///
    /// A capacity of 0 is legal and degrades to "nothing is ever replayable":
    /// every client but an already up-to-date one is told to take a snapshot.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            seq: 0,
        }
    }

    /// Appends `bytes`, evicting from the front to stay inside the budget.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.seq += bytes.len() as u64;

        // A single write at least as large as the whole budget is copied
        // tail-first instead of being appended and then trimmed: appending
        // would first grow the buffer to the size of the write, and a write is
        // however much the pty handed over in one read - `cat` of a large file
        // arrives in chunks the ring's own budget has no say over.
        if bytes.len() >= self.capacity {
            self.buffer.clear();
            self.buffer.extend(&bytes[bytes.len() - self.capacity..]);
            return;
        }

        // Evicted BEFORE appending, not after. Appending first pushes the
        // length past the reserved capacity for as long as it takes to
        // drain, and a `VecDeque` that overflows its capacity doubles its
        // allocation and never gives it back - so the ring would silently
        // settle at twice its configured byte budget.
        let overflow = (self.buffer.len() + bytes.len()).saturating_sub(self.capacity);
        self.buffer.drain(..overflow);
        self.buffer.extend(bytes);
    }

    /// Total bytes ever written, and so the cursor a client reports back.
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// The earliest sequence still replayable. Equals `seq` when the buffer is
    /// empty, which is why `since(first_seq())` is always `Some`.
    pub(crate) fn first_seq(&self) -> u64 {
        self.seq - self.buffer.len() as u64
    }

    /// Bytes from `seq` to now, or `None` when they cannot be served.
    ///
    /// `None` is the caller's signal to resynchronise the client from a full
    /// screen snapshot, and covers two cases that need the same remedy: a
    /// cursor old enough to have been evicted, and a cursor from the future -
    /// a client holding a cursor from a previous session id reports a
    /// sequence this ring has never reached, and handing it the empty replay
    /// its arithmetic would produce leaves it silently stuck forever.
    ///
    /// A client that is exactly up to date gets `Some(vec![])`: having missed
    /// nothing is not a cache miss, and forcing a snapshot for it would make
    /// every idle poll expensive.
    pub(crate) fn since(&self, seq: u64) -> Option<Vec<u8>> {
        let first = self.first_seq();
        if seq > self.seq || seq < first {
            return None;
        }
        let offset = (seq - first) as usize;
        Some(self.buffer.range(offset..).copied().collect())
    }
}

/// Reports the ring's shape and never its contents: the buffer holds raw
/// terminal output, which is both far too large for a debug line and not
/// something to spill into a log.
impl std::fmt::Debug for SeqRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeqRing")
            .field("first_seq", &self.first_seq())
            .field("seq", &self.seq)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every fixture below is ASCII, so rendering a replay as text keeps the
    /// assertions readable instead of byte-string noise.
    fn replay(ring: &SeqRing, seq: u64) -> Option<String> {
        ring.since(seq)
            .map(|bytes| String::from_utf8(bytes).expect("fixtures are ascii"))
    }

    #[test]
    fn a_client_that_has_seen_nothing_replays_the_whole_stream() {
        let mut ring = SeqRing::new(64);
        ring.push(b"hello ");
        ring.push(b"world");
        assert_eq!(replay(&ring, 0).as_deref(), Some("hello world"));
    }

    #[test]
    fn seq_counts_bytes_rather_than_pushes() {
        let mut ring = SeqRing::new(64);
        ring.push(b"hello ");
        ring.push(b"world");
        assert_eq!(ring.seq(), 11);
    }

    #[test]
    fn nothing_is_evicted_while_the_stream_fits_the_budget() {
        let mut ring = SeqRing::new(11);
        ring.push(b"hello ");
        ring.push(b"world");
        assert_eq!(ring.first_seq(), 0);
    }

    #[test]
    fn pushing_past_the_budget_drops_the_oldest_bytes() {
        let mut ring = SeqRing::new(8);
        ring.push(b"abcde");
        ring.push(b"fghij");
        // Ten bytes through an eight-byte budget: "ab" is gone, so the ring
        // now begins at sequence 2.
        assert_eq!(ring.first_seq(), 2);
        assert_eq!(replay(&ring, 2).as_deref(), Some("cdefghij"));
    }

    #[test]
    fn a_cursor_older_than_the_budget_is_a_cache_miss() {
        let mut ring = SeqRing::new(8);
        ring.push(b"abcde");
        ring.push(b"fghij");
        assert_eq!(ring.since(0), None);
    }

    #[test]
    fn the_oldest_surviving_cursor_still_replays_every_buffered_byte() {
        let mut ring = SeqRing::new(8);
        ring.push(b"abcde");
        ring.push(b"fghij");
        assert_eq!(replay(&ring, ring.first_seq()).as_deref(), Some("cdefghij"));
    }

    // The boundary is inclusive on the surviving side: one byte earlier is the
    // first cursor that must fall back to a snapshot.
    #[test]
    fn the_byte_just_before_the_boundary_is_the_first_cache_miss() {
        let mut ring = SeqRing::new(8);
        ring.push(b"abcdefghij");
        assert_eq!(ring.first_seq(), 2);
        assert!(ring.since(2).is_some());
        assert_eq!(ring.since(1), None);
    }

    #[test]
    fn a_single_push_larger_than_the_budget_keeps_its_tail() {
        let mut ring = SeqRing::new(4);
        ring.push(b"0123456789");
        assert_eq!(replay(&ring, ring.first_seq()).as_deref(), Some("6789"));
    }

    #[test]
    fn a_single_push_larger_than_the_budget_still_counts_every_byte() {
        let mut ring = SeqRing::new(4);
        ring.push(b"0123456789");
        assert_eq!(ring.seq(), 10);
        assert_eq!(ring.first_seq(), 6);
    }

    #[test]
    fn a_cursor_from_the_future_is_a_cache_miss() {
        let mut ring = SeqRing::new(64);
        ring.push(b"abc");
        // A client resuming against a different session id reports a sequence
        // this ring has never produced.
        assert_eq!(ring.since(9_000), None);
    }

    #[test]
    fn a_client_that_is_up_to_date_replays_nothing_rather_than_missing() {
        let mut ring = SeqRing::new(64);
        ring.push(b"abc");
        assert_eq!(replay(&ring, ring.seq()).as_deref(), Some(""));
    }

    #[test]
    fn an_empty_ring_serves_an_empty_replay_from_zero() {
        let ring = SeqRing::new(64);
        assert_eq!(replay(&ring, 0).as_deref(), Some(""));
    }

    #[test]
    fn an_empty_push_moves_neither_cursor() {
        let mut ring = SeqRing::new(64);
        ring.push(b"abc");
        ring.push(b"");
        assert_eq!(ring.seq(), 3);
        assert_eq!(ring.first_seq(), 0);
    }

    // Many small writes exercise the wrap-around that a single large write
    // never reaches: the tail must still read back contiguously.
    #[test]
    fn a_long_run_of_small_pushes_keeps_exactly_the_last_budget_of_bytes() {
        let mut ring = SeqRing::new(5);
        for byte in b'a'..=b'z' {
            ring.push(&[byte]);
        }
        assert_eq!(ring.seq(), 26);
        assert_eq!(ring.first_seq(), 21);
        assert_eq!(replay(&ring, 21).as_deref(), Some("vwxyz"));
    }

    #[test]
    fn a_zero_capacity_ring_serves_only_a_client_that_is_up_to_date() {
        let mut ring = SeqRing::new(0);
        ring.push(b"abc");
        assert_eq!(ring.first_seq(), 3);
        assert_eq!(replay(&ring, 3).as_deref(), Some(""));
        assert_eq!(ring.since(0), None);
    }
}
