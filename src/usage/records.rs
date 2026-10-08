//! Metadata-only usage shared by the observed ledger and native histories.

use std::io::{self, BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::Agent;

pub(crate) const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Source {
    Alc,
    Claude,
    Codex,
}

impl Source {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Alc => "alc",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum InputBasis {
    /// OpenAI counts cache reads/writes inside input_tokens.
    #[default]
    Inclusive,
    /// Anthropic's input_tokens is the uncached remainder.
    Separate,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TokenCounts {
    pub input_tokens: Option<u64>,
    pub input_basis: InputBasis,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cache_write_5m_tokens: Option<u64>,
    pub cache_write_1h_tokens: Option<u64>,
    /// A subset of output_tokens, never an additional billed quantity.
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl TokenCounts {
    pub(crate) fn gross_input(&self) -> Option<u64> {
        let input = self.input_tokens?;
        match self.input_basis {
            InputBasis::Inclusive => Some(input),
            InputBasis::Separate => input
                .checked_add(self.cache_read_tokens?)?
                .checked_add(self.cache_write_tokens?),
        }
    }

    pub(crate) fn uncached_input(&self) -> Option<u64> {
        let input = self.input_tokens?;
        match self.input_basis {
            InputBasis::Separate => Some(input),
            InputBasis::Inclusive => input
                .checked_sub(self.cache_read_tokens?)?
                .checked_sub(self.cache_write_tokens?),
        }
    }

    pub(crate) fn validate(&mut self) {
        if self.input_basis == InputBasis::Inclusive
            && let Some(input) = self.input_tokens
        {
            if self.cache_read_tokens.is_some_and(|read| read > input) {
                self.cache_read_tokens = None;
            }
            if self.cache_write_tokens.is_some_and(|write| write > input) {
                self.cache_write_tokens = None;
            }
            if let (Some(read), Some(write)) = (self.cache_read_tokens, self.cache_write_tokens)
                && read.checked_add(write).is_none_or(|cached| cached > input)
            {
                self.cache_read_tokens = None;
                self.cache_write_tokens = None;
            }
        }
        if let (Some(reasoning), Some(output)) = (self.reasoning_tokens, self.output_tokens)
            && reasoning > output
        {
            self.reasoning_tokens = None;
        }
        if let (Some(short), Some(long), Some(write)) = (
            self.cache_write_5m_tokens,
            self.cache_write_1h_tokens,
            self.cache_write_tokens,
        ) && short.checked_add(long) != Some(write)
        {
            self.cache_write_5m_tokens = None;
            self.cache_write_1h_tokens = None;
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Outcome {
    Completed,
    Incomplete,
    Failed,
    Cancelled,
    TimedOut,
    Truncated,
    #[default]
    Unknown,
}

impl Outcome {
    pub(crate) fn has_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Incomplete | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Granularity {
    #[default]
    Request,
    CumulativeDelta,
    Checkpoint,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Billing {
    Api,
    ApiEquivalent,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum OutputBasis {
    /// All reported output is represented by observable content, including thinking.
    Gross,
    /// The bridge suppresses reasoning; subtract its explicitly reported subset.
    NonReasoning,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Timing {
    pub client_streaming: bool,
    pub first_content_us: Option<u64>,
    /// Needed when a stream exposes reasoning but the rate counts only visible output.
    pub first_visible_us: Option<u64>,
    pub last_content_us: Option<u64>,
    pub terminal_us: Option<u64>,
    pub elapsed_us: u64,
    pub output_basis: OutputBasis,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct CorrelationId {
    /// messages, responses, chat, or a source-local request namespace.
    pub protocol: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageRecord {
    pub source: Source,
    pub timestamp_ms: u64,
    pub agent: Agent,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub ids: Vec<CorrelationId>,
    #[serde(default)]
    pub tokens: TokenCounts,
    #[serde(default)]
    pub timing: Option<Timing>,
    #[serde(default)]
    pub outcome: Outcome,
    #[serde(default)]
    pub granularity: Granularity,
    #[serde(default)]
    pub billing: Billing,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl UsageRecord {
    pub(crate) fn new(source: Source, agent: Agent, timestamp_ms: u64) -> Self {
        Self {
            source,
            timestamp_ms,
            agent,
            profile: None,
            provider: None,
            model: None,
            endpoint: None,
            account_id: None,
            session_id: None,
            ids: Vec::new(),
            tokens: TokenCounts::default(),
            timing: None,
            outcome: Outcome::Unknown,
            granularity: Granularity::Request,
            billing: Billing::Unknown,
            service_tier: None,
            warnings: Vec::new(),
        }
    }

    pub(crate) fn metrics(&self) -> Metrics {
        let Some(timing) = &self.timing else {
            return Metrics::default();
        };
        let ttft_ms = timing
            .client_streaming
            .then_some(timing.first_content_us)
            .flatten()
            .map(|us| us as f64 / 1000.0);
        let terminal = timing.terminal_us.filter(|us| *us > 0);
        let e2e_tps = terminal.and_then(|us| {
            self.tokens
                .output_tokens
                .map(|tokens| tokens as f64 * 1_000_000.0 / us as f64)
        });
        let stream_tokens = match timing.output_basis {
            OutputBasis::Gross => self.tokens.output_tokens,
            OutputBasis::NonReasoning => self
                .tokens
                .output_tokens
                .and_then(|output| output.checked_sub(self.tokens.reasoning_tokens?)),
            OutputBasis::Unknown => None,
        };
        let stream_tps = if timing.client_streaming {
            terminal.and_then(|terminal| {
                let first = if timing.output_basis == OutputBasis::NonReasoning {
                    timing.first_visible_us?
                } else {
                    timing.first_content_us?
                };
                let span = terminal.checked_sub(first)?;
                let tokens = stream_tokens?.checked_sub(1)?;
                (span > 0 && tokens > 0).then(|| tokens as f64 * 1_000_000.0 / span as f64)
            })
        } else {
            None
        };
        Metrics {
            ttft_ms,
            stream_tps,
            e2e_tps,
            stream_output_basis: timing.output_basis,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Metrics {
    pub ttft_ms: Option<f64>,
    pub stream_tps: Option<f64>,
    pub e2e_tps: Option<f64>,
    pub stream_output_basis: OutputBasis,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceDiagnostics {
    pub source: Source,
    pub files: u64,
    pub records: u64,
    pub skipped_lines: u64,
    pub unsupported_records: u64,
    pub ambiguous_records: u64,
    pub warnings: Vec<String>,
}

impl SourceDiagnostics {
    pub(crate) fn new(source: Source) -> Self {
        Self {
            source,
            files: 0,
            records: 0,
            skipped_lines: 0,
            unsupported_records: 0,
            ambiguous_records: 0,
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ReadResult {
    pub records: Vec<UsageRecord>,
    pub diagnostics: Vec<SourceDiagnostics>,
}

#[derive(Debug, Default)]
pub(crate) struct LineCounts {
    pub oversized: u64,
}

/// Bounds a single JSONL row even when a native log contains a huge prompt.
/// The consumer never receives an oversized row or its raw contents.
pub(crate) fn read_lines(path: &Path, mut consumer: impl FnMut(&[u8])) -> io::Result<LineCounts> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut counts = LineCounts::default();
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            if oversized {
                counts.oversized += 1;
            } else if !line.iter().all(u8::is_ascii_whitespace) {
                consumer(&line);
            }
            return Ok(counts);
        }
        let end = buffer.iter().position(|byte| *byte == b'\n');
        let take = end.map_or(buffer.len(), |index| index + 1);
        if !oversized {
            if line.len().saturating_add(take) > MAX_LINE_BYTES {
                line.clear();
                oversized = true;
            } else {
                line.extend_from_slice(&buffer[..take]);
            }
        }
        reader.consume(take);
        if end.is_some() {
            if oversized {
                counts.oversized += 1;
            } else if !line.iter().all(u8::is_ascii_whitespace) {
                consumer(&line);
            }
            line.clear();
            oversized = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_semantics_preserve_unknown_and_check_subsets() {
        let mut tokens = TokenCounts {
            input_tokens: Some(100),
            cache_read_tokens: Some(60),
            cache_write_tokens: Some(10),
            ..TokenCounts::default()
        };
        assert_eq!(tokens.gross_input(), Some(100));
        assert_eq!(tokens.uncached_input(), Some(30));
        tokens.input_basis = InputBasis::Separate;
        assert_eq!(tokens.gross_input(), Some(170));
        assert_eq!(tokens.uncached_input(), Some(100));
        tokens.cache_read_tokens = None;
        assert_eq!(tokens.gross_input(), None);
        tokens.input_basis = InputBasis::Inclusive;
        tokens.cache_read_tokens = Some(110);
        tokens.validate();
        assert_eq!(tokens.cache_read_tokens, None);
    }

    #[test]
    fn tps_does_not_count_chunks_or_unknown_reasoning() {
        let mut record = UsageRecord::new(Source::Alc, Agent::Claude, 0);
        record.tokens.output_tokens = Some(11);
        record.timing = Some(Timing {
            client_streaming: true,
            first_content_us: Some(1_000_000),
            first_visible_us: Some(1_000_000),
            last_content_us: Some(2_000_000),
            terminal_us: Some(3_000_000),
            elapsed_us: 3_000_000,
            output_basis: OutputBasis::NonReasoning,
        });
        assert_eq!(record.metrics().ttft_ms, Some(1000.0));
        assert_eq!(record.metrics().stream_tps, None);
        record.tokens.reasoning_tokens = Some(2);
        assert_eq!(record.metrics().stream_tps, Some(4.0));
        assert_eq!(record.metrics().e2e_tps, Some(11.0 / 3.0));
        record.timing.as_mut().unwrap().client_streaming = false;
        assert_eq!(record.metrics().ttft_ms, None);
        assert_eq!(record.metrics().stream_tps, None);
    }

    #[test]
    fn oversized_lines_are_skipped_without_losing_next_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut body = vec![b'x'; MAX_LINE_BYTES + 1];
        body.extend_from_slice(b"\n{}\n");
        std::fs::write(&path, body).unwrap();
        let mut rows = Vec::new();
        let counts = read_lines(&path, |line| rows.push(line.to_vec())).unwrap();
        assert_eq!(counts.oversized, 1);
        assert_eq!(rows, vec![b"{}\n".to_vec()]);
    }
}
