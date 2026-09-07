//! Length-preserving masking of credential values in mirrored terminal output.
//!
//! A shared session is screen sharing. Whatever the agent prints reaches every
//! browser watching it, so an agent that dumps its own environment — `env`, a
//! verbose HTTP trace, a crash report — would fan the key alc just handed it
//! out to every viewer at once. `LaunchSpec::secret_values` records the
//! credential strings alc itself put in front of the agent, and this masks
//! those on the way to the mirror.
//!
//! It closes the one hole alc opened and no other. A key the user types into
//! the agent, one the agent reads from a file alc never wrote, or a token the
//! provider mints mid-session is not in the list and is not masked; the user
//! docs say that plainly rather than implying a guarantee this cannot make.
//!
//! Replacement is byte-for-byte because both ends of the stream are column
//! sensitive: the user's own terminal and the emulator that renders the
//! browser snapshot each mis-lay-out the screen if a mask is a different width
//! from the value it replaced. A shorter or longer mask would corrupt every
//! line after it.

/// Shorter values are ignored: an eight-byte floor keeps a placeholder like a
/// short model name or `ollama` from turning ordinary output into asterisks,
/// and no real credential is that short. Mirrors the same floor in
/// `LaunchSpec::mark_secret_value`, which is what fills the list.
const MIN_SECRET_BYTES: usize = 8;

/// One asterisk per masked byte.
const MASK: u8 = b'*';

/// A streaming masker over one direction of a PTY stream.
///
/// Bytes are matched, never characters. Credential material is ASCII-ish and
/// the chunker upstream owns UTF-8 boundaries, so a multi-byte character
/// straddling a read is not this type's problem — and a byte match cannot
/// split a code point it did not already contain, because the secret it
/// replaces was itself a `String`.
pub(crate) struct SecretScrubber {
    /// Deduplicated, each at least `MIN_SECRET_BYTES` long. Order is
    /// irrelevant: matches are collected against the unmodified buffer and
    /// applied afterwards, so two secrets that overlap both still land.
    secrets: Vec<Vec<u8>>,
    /// One less than the longest secret: the ceiling on how much `push` can
    /// hold back, not how much it does. What is actually held is computed
    /// per call and is almost always nothing.
    longest: usize,
    /// Carry plus the chunk being scanned. Drained down to `hold` bytes by
    /// every `push`, so it is bounded even under a flood of output.
    buffer: Vec<u8>,
}

impl SecretScrubber {
    /// Values shorter than 8 bytes are ignored - masking those would corrupt
    /// unrelated output.
    pub(crate) fn new(secrets: &[String]) -> Self {
        let mut values: Vec<Vec<u8>> = Vec::new();
        for secret in secrets {
            let bytes = secret.as_bytes();
            if bytes.len() < MIN_SECRET_BYTES {
                continue;
            }
            // The same key commonly reaches the agent through more than one
            // variable; scanning for it twice would only cost.
            if values.iter().any(|known| known.as_slice() == bytes) {
                continue;
            }
            values.push(bytes.to_vec());
        }
        let longest = values.iter().map(Vec::len).max().unwrap_or(0);
        Self {
            secrets: values,
            longest,
            buffer: Vec::new(),
        }
    }

    /// True when there is nothing to look for, so the caller can skip the copy
    /// entirely on the common path.
    pub(crate) fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Bytes safe to emit now.
    ///
    /// What is held back is computed from the buffer, not from the longest
    /// secret. That distinction is the whole point: a provider key is 100 to
    /// 170 bytes, and holding that many trailing bytes unconditionally would
    /// leave the browser permanently a screenful behind an agent sitting at
    /// a prompt - the mirror would look frozen every time the user stopped
    /// typing. Only two things are ever held:
    ///
    /// * a suffix that is a proper prefix of some secret, which the next
    ///   chunk might complete, and
    /// * a complete match the emit boundary would cut in half, which is held
    ///   whole so its tail cannot be emitted unmasked on a later call.
    ///
    /// In the overwhelmingly common case - output containing nothing that
    /// even starts to look like a credential - both are empty and nothing is
    /// held at all.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.secrets.is_empty() {
            return chunk.to_vec();
        }
        self.buffer.extend_from_slice(chunk);

        let spans = self.matches();
        let mut hold = self.partial_match_suffix();
        let boundary = self.buffer.len() - hold;
        if let Some(start) = spans
            .iter()
            .filter(|(_, end)| *end > boundary)
            .map(|(start, _)| *start)
            .min()
        {
            hold = self.buffer.len() - start;
        }

        let emit = self.buffer.len() - hold;
        let mut out = self.buffer[..emit].to_vec();
        // Masked into the copy, never into `self.buffer`: overwriting the
        // retained tail would destroy the only match of a longer secret that
        // contains this one, and its outer bytes would then be emitted in
        // the clear on the next call.
        for (start, end) in spans {
            if start < emit {
                out[start..end.min(emit)].fill(MASK);
            }
        }
        self.buffer.drain(..emit);
        out
    }

    /// Emits the held-back tail when the stream ends, masked.
    ///
    /// A masking pass is needed here even though `push` masks what it emits:
    /// `push` deliberately retains a complete match rather than cutting it,
    /// so the last thing an agent printed may be a whole credential sitting
    /// unmasked in the buffer.
    pub(crate) fn flush(&mut self) -> Vec<u8> {
        let spans = self.matches();
        let mut out = std::mem::take(&mut self.buffer);
        for (start, end) in spans {
            out[start..end].fill(MASK);
        }
        out
    }

    /// Every complete occurrence of every secret in the buffer.
    ///
    /// Collected before anything is masked, because masking as we go would
    /// blind the search for the next secret: given "abcdefghij" and
    /// "ghijklmnop" overlapping in the output, masking the first would erase
    /// the second's only match. The list is empty on the hot path, where
    /// `Vec::new` has not allocated.
    fn matches(&self) -> Vec<(usize, usize)> {
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for secret in &self.secrets {
            let mut from = 0;
            // Advance one byte past the match start, not past its end: a
            // secret can overlap its own next occurrence ("aaaaaaaa" inside
            // "aaaaaaaaa"), and skipping the whole match would leave the tail
            // of the second one in the clear.
            while let Some(at) = find_from(&self.buffer, secret, from) {
                spans.push((at, at + secret.len()));
                from = at + 1;
            }
        }
        spans
    }

    /// The longest suffix of the buffer that is a proper prefix of some
    /// secret - the only bytes a following chunk could turn into a match
    /// that has already begun.
    fn partial_match_suffix(&self) -> usize {
        let ceiling = self.longest.saturating_sub(1).min(self.buffer.len());
        (1..=ceiling)
            .rev()
            .find(|&length| {
                let tail = &self.buffer[self.buffer.len() - length..];
                self.secrets
                    .iter()
                    .any(|secret| secret.len() > length && secret.starts_with(tail))
            })
            .unwrap_or(0)
    }
}

/// The first offset at or after `from` where `needle` occurs in `haystack`.
fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    (from..=last).find(|&at| &haystack[at..at + needle.len()] == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "sk-secret-value";
    const MASKED: &str = "***************";

    fn scrubber() -> SecretScrubber {
        SecretScrubber::new(&[KEY.to_owned()])
    }

    /// Feeds every chunk then the tail, the way the PTY mirror does at EOF.
    fn drain(scrubber: &mut SecretScrubber, chunks: &[&[u8]]) -> String {
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend_from_slice(&scrubber.push(chunk));
        }
        out.extend_from_slice(&scrubber.flush());
        String::from_utf8(out).expect("ascii test data")
    }

    #[test]
    fn a_secret_whole_in_one_chunk_is_masked() {
        let line = format!("ANTHROPIC_API_KEY={KEY}\n");
        assert_eq!(
            drain(&mut scrubber(), &[line.as_bytes()]),
            format!("ANTHROPIC_API_KEY={MASKED}\n")
        );
    }

    #[test]
    fn a_secret_split_across_two_reads_is_still_masked() {
        assert_eq!(
            drain(&mut scrubber(), &[b"key=sk-secret", b"-value\n"]),
            format!("key={MASKED}\n")
        );
    }

    #[test]
    fn a_secret_split_across_three_reads_is_still_masked() {
        assert_eq!(
            drain(&mut scrubber(), &[b"key=sk-", b"secret", b"-value\n"]),
            format!("key={MASKED}\n")
        );
    }

    #[test]
    fn a_secret_split_across_four_reads_is_still_masked() {
        assert_eq!(
            drain(&mut scrubber(), &[b"key=sk", b"-sec", b"ret-va", b"lue\n"]),
            format!("key={MASKED}\n")
        );
    }

    #[test]
    fn a_secret_delivered_one_byte_at_a_time_is_still_masked() {
        let line = format!("key={KEY}\n");
        let chunks: Vec<&[u8]> = line.as_bytes().chunks(1).collect();
        assert_eq!(
            drain(&mut scrubber(), &chunks),
            format!("key={MASKED}\n"),
            "a one-byte read never completes a match on its own"
        );
    }

    #[test]
    fn two_different_secrets_are_both_masked() {
        let mut scrubber =
            SecretScrubber::new(&["first-secret".to_owned(), "second-secret-x".to_owned()]);
        assert_eq!(
            drain(&mut scrubber, &[b"a=first-secret b=second-secret-x c=3"]),
            "a=************ b=*************** c=3"
        );
    }

    #[test]
    fn a_secret_at_the_very_start_of_a_chunk_is_masked() {
        let line = format!("{KEY} trailing\n");
        assert_eq!(
            drain(&mut scrubber(), &[line.as_bytes()]),
            format!("{MASKED} trailing\n")
        );
    }

    #[test]
    fn a_secret_at_the_very_end_of_a_chunk_is_masked() {
        let line = format!("token={KEY}");
        assert_eq!(
            drain(&mut scrubber(), &[line.as_bytes()]),
            format!("token={MASKED}")
        );
    }

    #[test]
    fn the_same_secret_twice_in_one_chunk_is_masked_both_times() {
        let line = format!("{KEY} and {KEY}\n");
        assert_eq!(
            drain(&mut scrubber(), &[line.as_bytes()]),
            format!("{MASKED} and {MASKED}\n")
        );
    }

    // Masking the first match in place would erase the second's only match, so
    // the spans are collected against the unmodified buffer.
    #[test]
    fn two_overlapping_secrets_are_both_masked() {
        let mut scrubber = SecretScrubber::new(&["abcdefghij".to_owned(), "ghijklmnop".to_owned()]);
        assert_eq!(
            drain(&mut scrubber, &[b"[abcdefghijklmnop]"]),
            "[****************]"
        );
    }

    // Skipping past a whole match would leave the tail of the next, shifted-by-one
    // occurrence in the clear.
    #[test]
    fn a_secret_overlapping_its_own_next_occurrence_is_fully_masked() {
        let mut scrubber = SecretScrubber::new(&["aaaaaaaa".to_owned()]);
        assert_eq!(drain(&mut scrubber, &[b"-aaaaaaaaa-"]), "-*********-");
    }

    // Output that could not be the start of a credential is passed straight
    // through. Holding a fixed tail instead would leave a browser roughly a
    // key's length behind an agent waiting at a prompt.
    #[test]
    fn output_that_cannot_begin_a_secret_is_emitted_immediately() {
        let mut scrubber = scrubber();
        let emitted = scrubber.push(b"no secret here at all");
        assert_eq!(emitted, b"no secret here at all".to_vec());
        assert!(scrubber.flush().is_empty());
    }

    #[test]
    fn a_chunk_with_no_secret_is_returned_unchanged() {
        let mut scrubber = scrubber();
        let text = "\x1b[2J\x1b[Hordinary terminal output, nothing to hide\r\n";
        assert_eq!(drain(&mut scrubber, &[text.as_bytes()]), text);
    }

    #[test]
    fn masked_output_is_the_same_length_as_the_input() {
        let input = format!("a={KEY} b={KEY} c=plain\n");
        let masked = drain(&mut scrubber(), &[input.as_bytes()]);
        assert_eq!(masked.len(), input.len());
    }

    #[test]
    fn a_secret_shorter_than_eight_bytes_is_ignored() {
        let mut scrubber = SecretScrubber::new(&["ollama".to_owned()]);
        assert!(scrubber.is_empty());
        assert_eq!(drain(&mut scrubber, &[b"model=ollama"]), "model=ollama");
    }

    #[test]
    fn a_scrubber_with_nothing_to_look_for_is_empty() {
        assert!(SecretScrubber::new(&[]).is_empty());
        assert!(!scrubber().is_empty());
    }

    // The hold exists so a split value is still caught; it must not scale
    // with how much the agent printed, or a busy session would buffer
    // without bound.
    #[test]
    fn the_held_back_tail_never_scales_with_how_much_the_agent_printed() {
        let mut scrubber = scrubber();
        let chunk = vec![b'x'; 64 * 1024];
        let emitted = scrubber.push(&chunk);
        // Nothing in a wall of 'x' can begin this key, so nothing is held.
        assert_eq!(emitted.len(), chunk.len());
        assert!(scrubber.flush().is_empty());

        // And a chunk that does end mid-key holds only that much.
        let opening = &KEY.as_bytes()[..KEY.len() - 1];
        let emitted = scrubber.push(opening);
        assert!(emitted.is_empty());
        assert_eq!(scrubber.flush().len(), opening.len());
    }

    #[test]
    fn a_duplicate_secret_value_is_only_stored_once() {
        let scrubber = SecretScrubber::new(&[KEY.to_owned(), KEY.to_owned(), "short".to_owned()]);
        assert_eq!(scrubber.secrets.len(), 1);
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    const KEY: &str = "sk-ant-api03-0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn ordinary_output_is_not_held_back_at_all() {
        // The failure this guards against: holding `longest - 1` trailing
        // bytes unconditionally left the browser ~110 bytes behind whenever
        // an agent stopped at a prompt, so the mirror looked frozen every
        // time the user stopped typing.
        let mut scrubber = SecretScrubber::new(&[KEY.to_owned()]);
        let prompt = b"\x1b[2J\x1b[H  Claude Code\r\n\r\n  > ";
        assert_eq!(scrubber.push(prompt), prompt.to_vec());
    }

    #[test]
    fn only_a_real_partial_match_is_held() {
        let mut scrubber = SecretScrubber::new(&[KEY.to_owned()]);
        // Ends with the opening of the key, so that much is held.
        let out = scrubber.push(b"ready: sk-ant-");
        assert_eq!(out, b"ready: ".to_vec());

        let rest = format!("{}\r\n", &KEY["sk-ant-".len()..]);
        let out = scrubber.push(rest.as_bytes());
        assert!(!String::from_utf8_lossy(&out).contains("api03"), "{out:?}");
        assert!(out.ends_with(b"\r\n"));
    }

    #[test]
    fn a_secret_inside_a_longer_secret_survives_a_chunk_boundary() {
        // Masking into the retained tail destroyed the longer secret's only
        // match, and its outer bytes then reached every viewer in the clear.
        let short = "12345678".to_owned();
        let long = "ABC12345678DEF".to_owned();
        let mut scrubber = SecretScrubber::new(&[short, long]);

        let mut seen = scrubber.push(b"xxABC12345");
        seen.extend(scrubber.push(b"678DEF\r\n"));
        seen.extend(scrubber.flush());

        let rendered = String::from_utf8_lossy(&seen).into_owned();
        assert!(!rendered.contains("ABC"), "{rendered}");
        assert!(!rendered.contains("DEF"), "{rendered}");
        assert!(!rendered.contains("12345678"), "{rendered}");
        assert_eq!(seen.len(), "xxABC12345678DEF\r\n".len());
    }

    #[test]
    fn a_secret_at_the_very_end_of_a_stream_is_masked_by_flush() {
        // `push` holds a complete match rather than cutting it, so the last
        // thing an agent printed can be a whole credential still in the
        // buffer when the stream ends.
        let mut scrubber = SecretScrubber::new(&[KEY.to_owned(), format!("{KEY}EXTRA")]);
        let mut seen = scrubber.push(format!("key={KEY}").as_bytes());
        seen.extend(scrubber.flush());

        let rendered = String::from_utf8_lossy(&seen).into_owned();
        assert!(!rendered.contains("sk-ant"), "{rendered}");
        assert_eq!(seen.len(), format!("key={KEY}").len());
    }

    #[test]
    fn a_secret_split_one_byte_at_a_time_is_still_masked() {
        let mut scrubber = SecretScrubber::new(&[KEY.to_owned()]);
        let stream = format!("before {KEY} after\r\n");
        let mut seen = Vec::new();
        for byte in stream.as_bytes() {
            seen.extend(scrubber.push(&[*byte]));
        }
        seen.extend(scrubber.flush());

        let rendered = String::from_utf8_lossy(&seen).into_owned();
        assert!(!rendered.contains("sk-ant"), "{rendered}");
        assert!(rendered.starts_with("before "), "{rendered}");
        assert!(rendered.ends_with(" after\r\n"), "{rendered}");
        assert_eq!(seen.len(), stream.len(), "masking must preserve length");
    }

    #[test]
    fn the_hold_stays_bounded_under_a_flood_of_output() {
        let mut scrubber = SecretScrubber::new(&[KEY.to_owned()]);
        for _ in 0..200 {
            scrubber.push(&vec![b'x'; 8192]);
        }
        assert!(
            scrubber.buffer.len() < 2 * KEY.len(),
            "{}",
            scrubber.buffer.len()
        );
    }
}
