//! Read-only, metadata-only readers for explicitly selected native JSONL roots.
//!
//! Supported upstream schemas and synthetic examples live in
//! `tests/fixtures/usage-native/README.md`. Neither reader resolves login/config
//! directories, writes an import cache, or retains prompt/tool/message content.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::Agent;

use super::records::{
    Billing, CorrelationId, Granularity, InputBasis, Outcome, ReadResult, Source,
    SourceDiagnostics, TokenCounts, UsageRecord, read_lines,
};

/// `roots` are transcript directories (normally Claude's `projects`), or explicit
/// JSONL files. Descendant and root symlinks are deliberately not followed.
pub(crate) fn read_claude(roots: &[PathBuf]) -> ReadResult {
    let mut diagnostics = SourceDiagnostics::new(Source::Claude);
    let files = history_files(roots, &mut diagnostics);
    let mut candidates = Vec::new();
    for path in files {
        diagnostics.files += 1;
        read_file(&path, &mut diagnostics, |line, diagnostics| {
            let header = match serde_json::from_slice::<NativeHeader>(line) {
                Ok(header) => header,
                Err(_) => {
                    diagnostics.skipped_lines += 1;
                    warn(diagnostics, "malformed native JSONL rows were skipped");
                    return;
                }
            };
            if header.kind != "assistant" {
                if !matches!(
                    header.kind.as_str(),
                    "user"
                        | "system"
                        | "progress"
                        | "summary"
                        | "file-history-snapshot"
                        | "queue-operation"
                        | "attachment"
                ) {
                    diagnostics.unsupported_records += 1;
                }
                return;
            }
            let row = match serde_json::from_slice::<ClaudeRow>(line) {
                Ok(row) => row,
                Err(_) => {
                    diagnostics.skipped_lines += 1;
                    warn(diagnostics, "malformed native JSONL rows were skipped");
                    return;
                }
            };
            let ClaudeRow {
                timestamp,
                session_id,
                request_id,
                is_api_error_message,
                message,
            } = row;

            let Some(timestamp_ms) = timestamp.as_deref().and_then(timestamp_ms) else {
                diagnostics.skipped_lines += 1;
                warn(
                    diagnostics,
                    "native usage has a missing or invalid RFC3339 timestamp",
                );
                return;
            };
            let Some(message) = message else {
                diagnostics.unsupported_records += 1;
                return;
            };
            let Some(usage) = message.usage else {
                diagnostics.unsupported_records += 1;
                return;
            };
            let mut record = UsageRecord::new(Source::Claude, Agent::Claude, timestamp_ms);
            record.session_id = metadata(session_id);
            record.model = metadata(message.model);
            record.billing = Billing::ApiEquivalent;
            record.tokens = TokenCounts {
                input_tokens: usage.input_tokens,
                input_basis: InputBasis::Separate,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_input_tokens,
                cache_write_tokens: usage.cache_creation_input_tokens,
                cache_write_5m_tokens: usage
                    .cache_creation
                    .as_ref()
                    .and_then(|c| c.ephemeral_5m_input_tokens),
                cache_write_1h_tokens: usage
                    .cache_creation
                    .as_ref()
                    .and_then(|c| c.ephemeral_1h_input_tokens),
                ..TokenCounts::default()
            };
            record.service_tier = metadata(usage.service_tier);
            validate_tokens(&mut record);
            record.outcome = if is_api_error_message {
                Outcome::Failed
            } else {
                match message.stop_reason.as_deref() {
                    Some("end_turn" | "tool_use" | "stop_sequence" | "pause_turn" | "refusal") => {
                        Outcome::Completed
                    }
                    Some("max_tokens") => Outcome::Truncated,
                    Some("model_context_window_exceeded") => Outcome::Incomplete,
                    _ => Outcome::Unknown,
                }
            };
            if is_api_error_message {
                // Claude creates these transcript placeholders locally, with a
                // fabricated message ID and zero usage. Neither is API evidence.
                record.tokens = TokenCounts {
                    input_basis: InputBasis::Separate,
                    ..TokenCounts::default()
                };
                record.model = None;
                record.service_tier = None;
                checkpoint(
                    &mut record,
                    "Claude API-error transcript placeholder has no measured usage",
                );
            }
            candidates.push(ClaudeCandidate {
                message_id: if is_api_error_message {
                    None
                } else {
                    metadata(message.id)
                },
                request_id: if is_api_error_message {
                    None
                } else {
                    metadata(request_id)
                },
                authoritative: message.stop_reason.is_some(),
                record,
            });
        });
    }

    // uuid identifies a transcript content block, NOT an API request. Messages
    // IDs survive forks/copies and are therefore deliberately not session-scoped.
    let mut request_messages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for candidate in &candidates {
        if let (Some(request), Some(message)) = (&candidate.request_id, &candidate.message_id) {
            request_messages
                .entry(request.clone())
                .or_default()
                .insert(message.clone());
        }
    }
    let mut identified: BTreeMap<(String, String), ClaudeCandidate> = BTreeMap::new();
    let mut unidentified = Vec::new();
    for mut candidate in candidates {
        if candidate.record.granularity == Granularity::Checkpoint {
            unidentified.push(candidate.record);
            continue;
        }
        let messages = candidate
            .request_id
            .as_ref()
            .and_then(|id| request_messages.get(id));
        let message_id = candidate.message_id.clone().or_else(|| {
            messages
                .filter(|ids| ids.len() == 1)
                .and_then(|ids| ids.first().cloned())
        });
        if let Some(id) = message_id {
            candidate.record.ids.push(correlation("messages", &id));
            if let Some(request) = &candidate.request_id {
                if messages.is_none_or(|ids| ids.len() <= 1) {
                    candidate
                        .record
                        .ids
                        .push(correlation("claude-request", request));
                } else {
                    record_warn(
                        &mut candidate.record,
                        "a request ID names multiple messages; only message IDs are used",
                    );
                }
            }
            insert_claude(&mut identified, ("messages".into(), id), candidate);
        } else if let Some(request) = candidate.request_id.clone() {
            if messages.is_some_and(|ids| ids.len() > 1) {
                checkpoint(
                    &mut candidate.record,
                    "Claude usage without a message ID has an ambiguous request ID",
                );
                unidentified.push(candidate.record);
            } else {
                candidate
                    .record
                    .ids
                    .push(correlation("claude-request", &request));
                insert_claude(
                    &mut identified,
                    ("claude-request".into(), request),
                    candidate,
                );
            }
        } else {
            checkpoint(
                &mut candidate.record,
                "Claude usage has no stable request or message ID",
            );
            unidentified.push(candidate.record);
        }
    }
    let mut records: Vec<_> = identified
        .into_values()
        .map(|candidate| candidate.record)
        .collect();
    records.extend(unidentified);
    finish(records, diagnostics)
}

/// `roots` normally contain Codex's `sessions` and `archived_sessions` directories.
/// Cumulative histories are folded in full, before a caller applies date filters.
pub(crate) fn read_codex(roots: &[PathBuf]) -> ReadResult {
    let mut diagnostics = SourceDiagnostics::new(Source::Codex);
    let paths = history_files(roots, &mut diagnostics);
    let mut sessions: BTreeMap<String, Vec<CodexFile>> = BTreeMap::new();
    let mut unidentified = Vec::new();
    for path in paths {
        diagnostics.files += 1;
        let file = parse_codex_file(&path, &mut diagnostics);
        if let Some(id) = file
            .meta
            .as_ref()
            .and_then(|meta| metadata(meta.id.clone()))
        {
            sessions.entry(id).or_default().push(file);
        } else {
            unidentified.push(file);
        }
    }

    let mut requests: BTreeMap<String, UsageRecord> = BTreeMap::new();
    let mut records = Vec::new();
    for files in sessions.into_values() {
        // An archive/truncated copy is reconciled only by a complete identical
        // prefix of actual source events, not by timestamps and token values.
        let longest = files
            .iter()
            .enumerate()
            .max_by_key(|(index, file)| (file.hashes.len(), std::cmp::Reverse(*index)))
            .map(|(index, _)| index)
            .unwrap_or(0);
        let compatible = files
            .iter()
            .all(|file| files[longest].hashes.starts_with(&file.hashes));
        if compatible {
            if let Some(file) = files.into_iter().nth(longest) {
                fold_codex(file, false, &mut requests, &mut records);
            }
        } else {
            warn(
                &mut diagnostics,
                "Codex files for one session have unreconciled overlap; cumulative usage is checkpoint-only",
            );
            for file in files {
                fold_codex(file, true, &mut requests, &mut records);
            }
        }
    }
    for file in unidentified {
        fold_codex(file, true, &mut requests, &mut records);
    }
    records.extend(requests.into_values());
    finish(records, diagnostics)
}

fn finish(mut records: Vec<UsageRecord>, mut diagnostics: SourceDiagnostics) -> ReadResult {
    records.sort_by(|a, b| {
        a.timestamp_ms
            .cmp(&b.timestamp_ms)
            .then_with(|| a.session_id.cmp(&b.session_id))
            .then_with(|| a.ids.cmp(&b.ids))
            .then_with(|| a.model.cmp(&b.model))
    });
    diagnostics.records = records.len() as u64;
    diagnostics.ambiguous_records = records
        .iter()
        .filter(|record| {
            record.granularity == Granularity::Checkpoint || !record.warnings.is_empty()
        })
        .count() as u64;
    for record in &records {
        for warning in &record.warnings {
            warn(&mut diagnostics, warning);
        }
    }
    ReadResult {
        records,
        diagnostics: vec![diagnostics],
    }
}

fn timestamp_ms(value: &str) -> Option<u64> {
    u64::try_from(DateTime::parse_from_rfc3339(value).ok()?.timestamp_millis()).ok()
}

fn metadata(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
    })
}

fn correlation(protocol: &str, id: &str) -> CorrelationId {
    CorrelationId {
        protocol: protocol.into(),
        id: id.into(),
    }
}

fn warn(diagnostics: &mut SourceDiagnostics, message: &str) {
    if !diagnostics
        .warnings
        .iter()
        .any(|warning| warning == message)
    {
        diagnostics.warnings.push(message.into());
    }
}

fn record_warn(record: &mut UsageRecord, message: &str) {
    if !record.warnings.iter().any(|warning| warning == message) {
        record.warnings.push(message.into());
    }
}

fn checkpoint(record: &mut UsageRecord, message: &str) {
    record.granularity = Granularity::Checkpoint;
    record.billing = Billing::Unknown;
    record_warn(record, message);
}

fn validate_tokens(record: &mut UsageRecord) {
    let before = record.tokens.clone();
    record.tokens.validate();
    if before != record.tokens {
        record_warn(record, "invalid native token subsets were left unknown");
    }
}

fn history_files(roots: &[PathBuf], diagnostics: &mut SourceDiagnostics) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut pending: Vec<_> = roots
        .iter()
        .cloned()
        .map(|path| (path, None::<PathBuf>))
        .collect();
    pending.sort();
    while let Some((path, boundary)) = pending.pop() {
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                warn(diagnostics, "a native history root or entry is missing");
                continue;
            }
            Err(_) => {
                warn(
                    diagnostics,
                    "a native history root or entry could not be read",
                );
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            warn(diagnostics, "native history symlinks were not followed");
            continue;
        }
        let canonical = match fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(_) => {
                warn(
                    diagnostics,
                    "a native history root or entry could not be resolved",
                );
                continue;
            }
        };
        if boundary
            .as_ref()
            .is_some_and(|boundary| !canonical.starts_with(boundary))
        {
            warn(
                diagnostics,
                "a native history entry resolved outside its selected root",
            );
            continue;
        }
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if meta.is_file() {
            if canonical
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                files.insert(canonical);
            }
        } else if meta.is_dir() {
            let boundary = boundary.unwrap_or_else(|| canonical.clone());
            match fs::read_dir(canonical) {
                Ok(entries) => {
                    let mut children = Vec::new();
                    for entry in entries {
                        match entry {
                            Ok(entry) => children.push(entry.path()),
                            Err(_) => warn(
                                diagnostics,
                                "a native history directory entry could not be read",
                            ),
                        }
                    }
                    children.sort();
                    pending.extend(
                        children
                            .into_iter()
                            .rev()
                            .map(|path| (path, Some(boundary.clone()))),
                    );
                }
                Err(_) => warn(diagnostics, "a native history directory could not be read"),
            }
        }
    }
    files.into_iter().collect()
}

fn read_file(
    path: &Path,
    diagnostics: &mut SourceDiagnostics,
    mut consumer: impl FnMut(&[u8], &mut SourceDiagnostics),
) -> bool {
    // Recheck after discovery. No symlink is intentionally traversed, including
    // files renamed to symlinks between the directory scan and this read.
    if !fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
    {
        warn(
            diagnostics,
            "a native history file disappeared or became a symlink",
        );
        return false;
    }
    match read_lines(path, |line| consumer(line, diagnostics)) {
        Ok(counts) => {
            diagnostics.skipped_lines += counts.oversized;
            if counts.oversized > 0 {
                warn(diagnostics, "oversized native JSONL rows were skipped");
            }
            counts.oversized == 0
        }
        Err(_) => {
            warn(
                diagnostics,
                "a native history file could not be completely read",
            );
            false
        }
    }
}

// Two-pass typed parsing first chooses a schema from a tiny header, then reads
// only its metadata fields. No serde internally tagged/flattened value buffer is
// used: ignored prompt/tool/message payloads are skipped rather than retained.
#[derive(Deserialize)]
struct NativeHeader {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct ClaudeRow {
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(default, rename = "isApiErrorMessage")]
    is_api_error_message: bool,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<ClaudeCacheCreation>,
    service_tier: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeCacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

struct ClaudeCandidate {
    message_id: Option<String>,
    request_id: Option<String>,
    authoritative: bool,
    record: UsageRecord,
}

fn insert_claude(
    records: &mut BTreeMap<(String, String), ClaudeCandidate>,
    key: (String, String),
    mut candidate: ClaudeCandidate,
) {
    if let Some(previous) = records.get_mut(&key) {
        let conflicting_model = previous.record.model.is_some()
            && candidate.record.model.is_some()
            && previous.record.model != candidate.record.model;
        let replace = (candidate.authoritative, candidate.record.timestamp_ms)
            >= (previous.authoritative, previous.record.timestamp_ms);
        let stable_ids: BTreeSet<_> = previous
            .record
            .ids
            .iter()
            .chain(candidate.record.ids.iter())
            .cloned()
            .collect();
        if replace {
            candidate.record.ids = stable_ids.into_iter().collect();
            if conflicting_model {
                candidate.record.model = None;
                record_warn(
                    &mut candidate.record,
                    "copies of one Claude message disagree on model metadata",
                );
            }
            // Preserve an already-discovered conflict when an additional copy
            // arrives; a copied session must not repair ambiguous metadata.
            if previous
                .record
                .warnings
                .iter()
                .any(|warning| warning == "copies of one Claude message disagree on model metadata")
            {
                candidate.record.model = None;
                record_warn(
                    &mut candidate.record,
                    "copies of one Claude message disagree on model metadata",
                );
            }
            *previous = candidate;
        } else {
            previous.record.ids = stable_ids.into_iter().collect();
            if conflicting_model {
                previous.record.model = None;
                record_warn(
                    &mut previous.record,
                    "copies of one Claude message disagree on model metadata",
                );
            }
        }
    } else {
        records.insert(key, candidate);
    }
}

#[derive(Deserialize)]
struct CodexEnvelope<T> {
    timestamp: Option<String>,
    ordinal: Option<u64>,
    payload: T,
}

#[derive(Deserialize)]
struct CodexEventHeader {
    payload: NativeHeader,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
struct CodexMeta {
    id: Option<String>,
    model_provider: Option<String>,
    forked_from_id: Option<String>,
    parent_thread_id: Option<String>,
    subagent_history_start_ordinal: Option<u64>,
    history_base: Option<CodexHistoryBase>,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
struct CodexHistoryBase {
    end_ordinal_exclusive: Option<u64>,
}

#[derive(Deserialize)]
struct CodexTurnContext {
    turn_id: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct CodexRequest {
    thread_id: Option<String>,
    turn_id: Option<String>,
    response_id: Option<String>,
    usage: Option<CodexUsage>,
    thread_token_usage: Option<CodexUsage>,
}

#[derive(Deserialize)]
struct CodexTokenCount {
    info: Option<CodexUsageInfo>,
}

#[derive(Deserialize)]
struct CodexUsageInfo {
    total_token_usage: Option<CodexUsage>,
    last_token_usage: Option<CodexUsage>,
    model_context_window: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
struct CodexUsage {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

impl CodexUsage {
    fn tokens(&self) -> TokenCounts {
        TokenCounts {
            input_tokens: self.input_tokens,
            input_basis: InputBasis::Inclusive,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cached_input_tokens,
            // Even though upstream serde defaults historical cache writes to
            // zero, an unversioned transcript does not prove it measured zero.
            cache_write_tokens: self.cache_write_input_tokens,
            reasoning_tokens: self.reasoning_output_tokens,
            total_tokens: self.total_tokens,
            ..TokenCounts::default()
        }
    }

    fn is_zero(&self) -> bool {
        self.input_tokens == Some(0)
            && self.output_tokens == Some(0)
            && self.total_tokens.is_none_or(|total| total == 0)
            && [
                self.cached_input_tokens,
                self.cache_write_input_tokens,
                self.reasoning_output_tokens,
            ]
            .into_iter()
            .flatten()
            .all(|value| value == 0)
    }

    fn has_billable_components(&self) -> bool {
        [
            self.input_tokens,
            self.output_tokens,
            self.cached_input_tokens,
            self.cache_write_input_tokens,
        ]
        .into_iter()
        .flatten()
        .any(|value| value > 0)
    }

    fn artificial_total(&self, context_window: Option<u64>) -> bool {
        self.total_tokens.is_some_and(|total| total > 0)
            && self.input_tokens == Some(0)
            && self.output_tokens == Some(0)
            && (context_window == self.total_tokens || !self.has_billable_components())
    }

    fn delta(&self, previous: &Self) -> Option<Self> {
        fn subtract(current: Option<u64>, previous: Option<u64>) -> Option<Option<u64>> {
            match (current, previous) {
                (Some(current), Some(previous)) => Some(Some(current.checked_sub(previous)?)),
                _ => Some(None),
            }
        }
        Some(Self {
            input_tokens: subtract(self.input_tokens, previous.input_tokens)?,
            cached_input_tokens: subtract(self.cached_input_tokens, previous.cached_input_tokens)?,
            cache_write_input_tokens: subtract(
                self.cache_write_input_tokens,
                previous.cache_write_input_tokens,
            )?,
            output_tokens: subtract(self.output_tokens, previous.output_tokens)?,
            reasoning_output_tokens: subtract(
                self.reasoning_output_tokens,
                previous.reasoning_output_tokens,
            )?,
            total_tokens: subtract(self.total_tokens, previous.total_tokens)?,
        })
    }

    fn sum(values: &[Self]) -> Option<Self> {
        fn add(values: &[CodexUsage], field: impl Fn(&CodexUsage) -> Option<u64>) -> Option<u64> {
            values
                .iter()
                .try_fold(0_u64, |sum, value| sum.checked_add(field(value)?))
        }
        (!values.is_empty()).then(|| Self {
            input_tokens: add(values, |usage| usage.input_tokens),
            cached_input_tokens: add(values, |usage| usage.cached_input_tokens),
            cache_write_input_tokens: add(values, |usage| usage.cache_write_input_tokens),
            output_tokens: add(values, |usage| usage.output_tokens),
            reasoning_output_tokens: add(values, |usage| usage.reasoning_output_tokens),
            total_tokens: add(values, |usage| usage.total_tokens),
        })
    }
}

enum CodexEvent {
    Context(CodexTurnContext),
    Request(CodexRequest),
    Total(CodexUsageInfo),
    Gap,
}

struct StampedCodexEvent {
    timestamp_ms: u64,
    ordinal: Option<u64>,
    index: usize,
    event: CodexEvent,
}

#[derive(Default)]
struct CodexFile {
    meta: Option<CodexMeta>,
    meta_conflict: bool,
    hashes: Vec<[u8; 32]>,
    events: Vec<StampedCodexEvent>,
    incomplete_read: bool,
}

fn parse_codex_file(path: &Path, diagnostics: &mut SourceDiagnostics) -> CodexFile {
    let mut file = CodexFile::default();
    let mut last_timestamp = 0;
    let complete = read_file(path, diagnostics, |line, diagnostics| {
        // Only opaque hashes of complete source events are kept for exact copy
        // detection. The raw row and any content fields die with this callback.
        let row_bytes = line.strip_suffix(b"\n").unwrap_or(line);
        let row_bytes = row_bytes.strip_suffix(b"\r").unwrap_or(row_bytes);
        file.hashes.push(Sha256::digest(row_bytes).into());
        let index = file.hashes.len() - 1;
        let parsed = (|| -> Result<_, serde_json::Error> {
            let header: NativeHeader = serde_json::from_slice(line)?;
            let envelope = match header.kind.as_str() {
                "session_meta" => {
                    let envelope: CodexEnvelope<CodexMeta> = serde_json::from_slice(line)?;
                    if let Some(previous) = &file.meta {
                        if previous != &envelope.payload {
                            file.meta_conflict = true;
                        }
                    } else {
                        file.meta = Some(envelope.payload);
                    }
                    return Ok(None);
                }
                "turn_context" => {
                    let envelope: CodexEnvelope<CodexTurnContext> = serde_json::from_slice(line)?;
                    CodexEnvelope {
                        timestamp: envelope.timestamp,
                        ordinal: envelope.ordinal,
                        payload: CodexEvent::Context(envelope.payload),
                    }
                }
                "token_usage_record" => {
                    let envelope: CodexEnvelope<CodexRequest> = serde_json::from_slice(line)?;
                    if envelope.payload.usage.is_none() {
                        diagnostics.unsupported_records += 1;
                    }
                    CodexEnvelope {
                        timestamp: envelope.timestamp,
                        ordinal: envelope.ordinal,
                        payload: CodexEvent::Request(envelope.payload),
                    }
                }
                "event_msg" => {
                    let header: CodexEventHeader = serde_json::from_slice(line)?;
                    if header.payload.kind != "token_count" {
                        return Ok(None);
                    }
                    let envelope: CodexEnvelope<CodexTokenCount> = serde_json::from_slice(line)?;
                    let Some(info) = envelope.payload.info else {
                        return Ok(None);
                    };
                    if info.total_token_usage.is_none() {
                        diagnostics.unsupported_records += 1;
                    }
                    CodexEnvelope {
                        timestamp: envelope.timestamp,
                        ordinal: envelope.ordinal,
                        payload: CodexEvent::Total(info),
                    }
                }
                "response_item"
                | "compacted"
                | "world_state"
                | "retained_context"
                | "security_risk_score"
                | "inter_agent_communication"
                | "inter_agent_communication_metadata"
                | "realtime_item" => return Ok(None),
                _ => {
                    diagnostics.unsupported_records += 1;
                    return Ok(None);
                }
            };
            Ok(Some(envelope))
        })();
        let envelope = match parsed {
            Ok(Some(envelope)) => envelope,
            Ok(None) => return,
            Err(_) => {
                diagnostics.skipped_lines += 1;
                warn(diagnostics, "malformed native JSONL rows were skipped");
                file.events.push(StampedCodexEvent {
                    timestamp_ms: last_timestamp,
                    ordinal: None,
                    index,
                    event: CodexEvent::Gap,
                });
                return;
            }
        };
        let Some(timestamp) = envelope.timestamp.as_deref().and_then(timestamp_ms) else {
            diagnostics.skipped_lines += 1;
            warn(
                diagnostics,
                "native usage has a missing or invalid RFC3339 timestamp",
            );
            file.events.push(StampedCodexEvent {
                timestamp_ms: last_timestamp,
                ordinal: envelope.ordinal,
                index,
                event: CodexEvent::Gap,
            });
            return;
        };
        last_timestamp = timestamp;
        file.events.push(StampedCodexEvent {
            timestamp_ms: timestamp,
            ordinal: envelope.ordinal,
            index,
            event: envelope.payload,
        });
    });
    // Oversized or unreadable rows have no position in the bounded callback.
    // Treat that file conservatively rather than allocate their usage to a date.
    file.incomplete_read = !complete;
    file.events
        .sort_by_key(|event| (event.timestamp_ms, event.index));
    file
}

fn fold_codex(
    file: CodexFile,
    overlapping: bool,
    requests: &mut BTreeMap<String, UsageRecord>,
    records: &mut Vec<UsageRecord>,
) {
    let session_id = file
        .meta
        .as_ref()
        .and_then(|meta| metadata(meta.id.clone()));
    let provider = file
        .meta
        .as_ref()
        .and_then(|meta| metadata(meta.model_provider.clone()));
    let inherited = file.meta.as_ref().is_some_and(|meta| {
        meta.forked_from_id.is_some()
            || meta.parent_thread_id.is_some()
            || meta.history_base.is_some()
    });
    let local_start = file
        .meta
        .as_ref()
        .and_then(|meta| meta.subagent_history_start_ordinal);
    let mixed = file
        .events
        .iter()
        .any(|event| matches!(event.event, CodexEvent::Request(_)));
    let mut turn_models: BTreeMap<String, Option<String>> = BTreeMap::new();
    for stamped in &file.events {
        if let CodexEvent::Context(context) = &stamped.event
            && let Some(turn) = metadata(context.turn_id.clone())
        {
            let context_model = metadata(context.model.clone());
            match turn_models.entry(turn) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(context_model);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get() != &context_model {
                        entry.insert(None);
                    }
                }
            }
        }
    }
    let mut model = None;
    let mut previous: Option<CodexUsage> = None;
    let mut baseline_model = None;
    let mut changed_model = false;
    let mut gap = false;
    let mut pending = Vec::new();
    let mut pending_endpoint = None;
    let mut previous_artificial = false;
    for stamped in file.events {
        let owned = !inherited
            || local_start
                .zip(stamped.ordinal)
                .is_some_and(|(start, ordinal)| ordinal >= start);
        match stamped.event {
            CodexEvent::Gap => gap = true,
            CodexEvent::Context(context) => {
                let next_model = metadata(context.model);
                if previous.is_some() && model != next_model {
                    changed_model = true;
                }
                model = next_model;
            }
            CodexEvent::Request(request) => {
                let Some(usage) = request.usage else {
                    gap = true;
                    continue;
                };
                let mut record =
                    UsageRecord::new(Source::Codex, Agent::Codex, stamped.timestamp_ms);
                let request_thread = metadata(request.thread_id);
                let own_thread = request_thread.is_some() && request_thread == session_id;
                record.session_id = request_thread.or_else(|| session_id.clone());
                record.provider = if own_thread && !file.meta_conflict {
                    provider.clone()
                } else {
                    None
                };
                record.model = metadata(request.turn_id)
                    .and_then(|turn| turn_models.get(&turn).cloned().flatten());
                if file.meta_conflict {
                    record.model = None;
                    record_warn(&mut record, "Codex session metadata is inconsistent");
                }
                record.tokens = usage.tokens();
                record.billing = Billing::ApiEquivalent;
                record.outcome = Outcome::Completed;
                validate_tokens(&mut record);
                if let Some(id) = metadata(request.response_id) {
                    record.ids.push(correlation("responses", &id));
                    insert_codex_request(requests, id, record);
                } else {
                    checkpoint(
                        &mut record,
                        "Codex response usage has no stable response ID",
                    );
                    records.push(record);
                }
                pending.push(usage);
                pending_endpoint = request.thread_token_usage;
            }
            CodexEvent::Total(info) => {
                // last_token_usage is not a separately billable request. It can
                // repeat after quota updates and represents multiple schemas.
                let _last = info.last_token_usage;
                let Some(total) = info.total_token_usage else {
                    gap = true;
                    continue;
                };
                if previous.as_ref() == Some(&total) {
                    baseline_model = model.clone();
                    changed_model = false;
                    continue;
                }
                if previous.is_none() && total.is_zero() {
                    // A recorded zero is a baseline, not a billable sample or an
                    // overlap checkpoint, even when later response rows exist.
                    previous = Some(total);
                    baseline_model = model.clone();
                    changed_model = false;
                    gap = false;
                    pending.clear();
                    pending_endpoint = None;
                    continue;
                }
                let mut record =
                    UsageRecord::new(Source::Codex, Agent::Codex, stamped.timestamp_ms);
                record.session_id = session_id.clone();
                record.provider = provider.clone();
                record.model = model.clone();
                record.granularity = Granularity::CumulativeDelta;
                record.billing = Billing::ApiEquivalent;
                record.tokens = total.tokens();
                let delta = previous.as_ref().and_then(|previous| total.delta(previous));
                let artificial = total.artificial_total(info.model_context_window);
                let reconciled = mixed
                    && !artificial
                    && !previous_artificial
                    && !gap
                    && !file.incomplete_read
                    && pending_endpoint.as_ref() == Some(&total)
                    && delta.as_ref().is_some_and(|delta| {
                        delta.input_tokens.is_some()
                            && delta.output_tokens.is_some()
                            && CodexUsage::sum(&pending).as_ref() == Some(delta)
                    });
                if reconciled {
                    previous = Some(total);
                    baseline_model = model.clone();
                    changed_model = false;
                    gap = false;
                    pending.clear();
                    pending_endpoint = None;
                    continue;
                }
                let reason = if file.meta_conflict {
                    Some("Codex session metadata is inconsistent")
                } else if overlapping || session_id.is_none() {
                    Some("Codex cumulative usage lacks a reconciled session history")
                } else if file.incomplete_read || gap {
                    Some("Codex cumulative usage crosses skipped or unreadable history")
                } else if !owned {
                    Some("forked or inherited Codex cumulative usage has no proven local boundary")
                } else if artificial {
                    Some(
                        "Codex context-window fill is an artificial checkpoint, not billable usage",
                    )
                } else if previous_artificial {
                    Some("Codex cumulative usage follows an artificial context-window baseline")
                } else if mixed {
                    Some("Codex response and cumulative usage overlap could not be reconciled")
                } else if previous.is_none() && !total.is_zero() {
                    Some("first nonzero Codex cumulative usage has no zero baseline")
                } else if previous.is_some() && delta.is_none() {
                    Some("Codex cumulative counters reset; the reset is checkpoint-only")
                } else if previous.is_some() && (changed_model || baseline_model != model) {
                    Some(
                        "Codex cumulative usage crosses model metadata changes and cannot be allocated",
                    )
                } else if delta
                    .as_ref()
                    .is_some_and(|delta| !delta.has_billable_components() && !delta.is_zero())
                {
                    Some("Codex cumulative usage has no comparable billable component baseline")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    checkpoint(&mut record, reason);
                    // Cumulative/mixed/foreign usage must never inherit a latest
                    // model and accidentally become priceable as a request.
                    record.model = None;
                    validate_tokens(&mut record);
                    records.push(record);
                } else if let Some(delta) = delta
                    && delta.has_billable_components()
                {
                    record.tokens = delta.tokens();
                    validate_tokens(&mut record);
                    records.push(record);
                }
                previous = Some(total);
                previous_artificial = artificial;
                baseline_model = model.clone();
                changed_model = false;
                gap = false;
                pending.clear();
                pending_endpoint = None;
            }
        }
    }
}

fn insert_codex_request(
    records: &mut BTreeMap<String, UsageRecord>,
    id: String,
    mut record: UsageRecord,
) {
    if let Some(previous) = records.get_mut(&id) {
        let token_conflict = previous.tokens != record.tokens;
        let model_conflict =
            previous.model.is_some() && record.model.is_some() && previous.model != record.model;
        let provider_conflict = previous.provider.is_some()
            && record.provider.is_some()
            && previous.provider != record.provider;
        if record.timestamp_ms < previous.timestamp_ms {
            std::mem::swap(previous, &mut record);
        }
        if previous.model.is_none() && !model_conflict {
            previous.model = record.model.clone();
        }
        if previous.provider.is_none() && !provider_conflict {
            previous.provider = record.provider.clone();
        }
        if model_conflict {
            previous.model = None;
            record_warn(
                previous,
                "copies of one Codex response disagree on model metadata",
            );
        }
        if provider_conflict {
            previous.provider = None;
            record_warn(
                previous,
                "copies of one Codex response disagree on provider metadata",
            );
        }
        if token_conflict {
            checkpoint(
                previous,
                "copies of one Codex response disagree on usage; counted only as a checkpoint",
            );
        }
        for warning in &record.warnings {
            record_warn(previous, warning);
        }
        if previous
            .warnings
            .iter()
            .any(|warning| warning == "copies of one Codex response disagree on model metadata")
        {
            previous.model = None;
        }
        if previous
            .warnings
            .iter()
            .any(|warning| warning == "copies of one Codex response disagree on provider metadata")
        {
            previous.provider = None;
        }
        if previous.warnings.iter().any(|warning| {
            warning
                == "copies of one Codex response disagree on usage; counted only as a checkpoint"
        }) {
            previous.granularity = Granularity::Checkpoint;
            previous.billing = Billing::Unknown;
        }
    } else {
        records.insert(id, record);
    }
}

#[cfg(test)]
mod tests {
    use super::super::records::MAX_LINE_BYTES;
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/usage-native")
            .join(name)
    }

    fn write_fixture(root: &Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    fn serialized(result: &ReadResult) -> String {
        format!(
            "{}{}",
            serde_json::to_string(&result.records).unwrap(),
            serde_json::to_string(&result.diagnostics).unwrap()
        )
    }

    fn codex_total(timestamp: &str, input: u64, output: u64, total: u64) -> String {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": input,
                        "output_tokens": output,
                        "total_tokens": total,
                    }
                }
            }
        })
        .to_string()
            + "\n"
    }

    #[test]
    fn claude_recursive_content_blocks_use_one_latest_authoritative_snapshot() {
        let result = read_claude(&[fixture("claude")]);
        assert_eq!(result.records.len(), 3);
        let record = result
            .records
            .iter()
            .find(|record| record.ids.iter().any(|id| id.id == "msg-shared"))
            .unwrap();
        assert_eq!(record.tokens.input_basis, InputBasis::Separate);
        assert_eq!(record.tokens.input_tokens, Some(10));
        assert_eq!(record.tokens.output_tokens, Some(12));
        assert_eq!(record.tokens.gross_input(), Some(130));
        assert_eq!(record.tokens.uncached_input(), Some(10));
        assert_eq!(record.tokens.cache_write_5m_tokens, Some(5));
        assert_eq!(record.tokens.cache_write_1h_tokens, Some(15));
        assert_eq!(record.billing, Billing::ApiEquivalent);
        assert_eq!(record.agent, Agent::Claude);
        assert_eq!(record.granularity, Granularity::Request);
        assert_eq!(record.outcome, Outcome::Completed);
        assert!(
            record
                .ids
                .iter()
                .any(|id| id.protocol == "claude-request" && id.id == "req-shared")
        );
        assert!(record.provider.is_none());
        assert!(record.account_id.is_none());
        assert!(record.endpoint.is_none());
        let missing = result
            .records
            .iter()
            .find(|record| record.ids.iter().any(|id| id.id == "msg-missing"))
            .unwrap();
        assert_eq!(missing.tokens.output_tokens, None);
        assert_eq!(missing.tokens.cache_read_tokens, None);
        assert_eq!(missing.tokens.cache_write_tokens, None);
        assert_eq!(missing.tokens.gross_input(), None);
        assert_eq!(missing.model, None);
        assert_eq!(result.diagnostics[0].files, 2);
        assert_eq!(result.diagnostics[0].unsupported_records, 1);
        assert!(!serialized(&result).contains("SYNTHETIC_"));
    }

    #[test]
    fn claude_malformed_missing_and_invalid_counters_are_bounded_and_private() {
        let result = read_claude(&[fixture("claude-malformed.jsonl")]);
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.diagnostics[0].skipped_lines, 6);
        assert_eq!(result.diagnostics[0].unsupported_records, 1);
        let ttl = result
            .records
            .iter()
            .find(|record| record.ids.iter().any(|id| id.id == "msg-invalid-ttl"))
            .unwrap();
        assert_eq!(ttl.tokens.cache_write_tokens, Some(9));
        assert_eq!(ttl.tokens.cache_write_5m_tokens, None);
        assert_eq!(ttl.tokens.cache_write_1h_tokens, None);
        let unknown = result
            .records
            .iter()
            .find(|record| record.ids.is_empty())
            .unwrap();
        assert_eq!(unknown.granularity, Granularity::Checkpoint);
        assert_eq!(unknown.billing, Billing::Unknown);
        assert!(!serialized(&result).contains("SYNTHETIC_"));
        assert!(!serialized(&result).contains("secret-token-counter"));
    }

    #[test]
    fn claude_request_only_rows_reconcile_with_later_message_id() {
        let temp = tempfile::tempdir().unwrap();
        write_fixture(
            temp.path(),
            "request.jsonl",
            concat!(
                "{\"type\":\"assistant\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"requestId\":\"request-only\",\"message\":{\"usage\":{\"output_tokens\":1}}}\n",
                "{\"type\":\"assistant\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"requestId\":\"request-only\",\"message\":{\"id\":\"message-later\",\"usage\":{\"output_tokens\":9}}}\n"
            ),
        );
        let result = read_claude(&[temp.path().into()]);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].tokens.output_tokens, Some(9));
        assert!(
            result.records[0]
                .ids
                .iter()
                .any(|id| id.protocol == "messages")
        );
        assert!(
            result.records[0]
                .ids
                .iter()
                .any(|id| id.protocol == "claude-request")
        );
    }

    #[test]
    fn codex_legacy_folds_cumulative_before_any_date_filter_without_requests() {
        let result = read_codex(&[fixture("codex-legacy.jsonl")]);
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.diagnostics[0].skipped_lines, 0);
        for record in &result.records {
            assert_eq!(record.granularity, Granularity::CumulativeDelta);
            assert_eq!(record.tokens.input_basis, InputBasis::Inclusive);
            assert_eq!(record.tokens.input_tokens, Some(100));
            assert_eq!(record.tokens.output_tokens, Some(10));
            assert_eq!(record.tokens.cache_read_tokens, Some(30));
            assert_eq!(record.tokens.cache_write_tokens, None);
            assert_eq!(record.tokens.reasoning_tokens, Some(4));
            assert_eq!(record.tokens.total_tokens, Some(110));
            assert_eq!(record.tokens.uncached_input(), None);
            assert_eq!(record.billing, Billing::ApiEquivalent);
            assert!(record.ids.is_empty());
            assert_eq!(record.model.as_deref(), Some("gpt-fixture"));
        }
        let after_midnight: Vec<_> = result
            .records
            .iter()
            .filter(|record| record.timestamp_ms >= timestamp_ms("2026-01-02T00:00:00Z").unwrap())
            .collect();
        assert_eq!(after_midnight.len(), 1);
        assert_eq!(after_midnight[0].tokens.input_tokens, Some(100));
        assert!(!serialized(&result).contains("SYNTHETIC_"));
    }

    #[test]
    fn codex_response_usage_replaces_only_precisely_reconciled_cumulative_rows() {
        let result = read_codex(&[fixture("codex-requests.jsonl")]);
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.diagnostics[0].skipped_lines, 0);
        assert_eq!(result.records[0].tokens.input_tokens, Some(100));
        assert_eq!(result.records[1].tokens.input_tokens, Some(50));
        assert_eq!(result.records[1].tokens.cache_write_tokens, Some(5));
        for record in &result.records {
            assert_eq!(record.granularity, Granularity::Request);
            assert_eq!(record.billing, Billing::ApiEquivalent);
            assert_eq!(record.outcome, Outcome::Completed);
            assert_eq!(record.ids[0].protocol, "responses");
            assert_eq!(record.model.as_deref(), Some("gpt-fixture"));
        }
    }

    #[test]
    fn codex_first_nonzero_reset_model_switch_and_artificial_fill_are_checkpoints() {
        let result = read_codex(&[fixture("codex-checkpoints.jsonl")]);
        assert_eq!(result.records.len(), 7);
        let checkpoints: Vec<_> = result
            .records
            .iter()
            .filter(|record| record.granularity == Granularity::Checkpoint)
            .collect();
        assert_eq!(checkpoints.len(), 4);
        for record in checkpoints {
            assert_eq!(record.billing, Billing::Unknown);
            assert!(record.model.is_none());
            assert!(!record.warnings.is_empty());
        }
        let deltas: Vec<_> = result
            .records
            .iter()
            .filter(|record| record.granularity == Granularity::CumulativeDelta)
            .collect();
        assert_eq!(
            deltas
                .iter()
                .map(|record| record.tokens.input_tokens.unwrap())
                .collect::<Vec<_>>(),
            vec![20, 10, 5]
        );
        assert_eq!(deltas[2].model.as_deref(), Some("gpt-fixture-b"));
        assert!(
            result.diagnostics[0]
                .warnings
                .iter()
                .any(|warning| warning.contains("artificial checkpoint"))
        );
    }

    #[test]
    fn codex_fork_history_without_local_boundary_never_bills_copied_totals() {
        let result = read_codex(&[fixture("codex-fork.jsonl")]);
        assert_eq!(result.records.len(), 2);
        assert!(
            result
                .records
                .iter()
                .all(|record| record.granularity == Granularity::Checkpoint
                    && record.billing == Billing::Unknown
                    && record.model.is_none())
        );
    }

    #[test]
    fn codex_archives_and_exact_prefix_copies_are_reconciled_by_source_events() {
        let temp = tempfile::tempdir().unwrap();
        let body = fs::read_to_string(fixture("codex-legacy.jsonl")).unwrap();
        write_fixture(temp.path(), "sessions/active.jsonl", &body);
        write_fixture(temp.path(), "archived_sessions/copied.jsonl", &body);
        let prefix = body.lines().take(4).collect::<Vec<_>>().join("\n") + "\n";
        write_fixture(temp.path(), "archived_sessions/partial-copy.jsonl", &prefix);
        let result = read_codex(&[
            temp.path().join("sessions"),
            temp.path().join("archived_sessions"),
            temp.path().join("sessions"),
        ]);
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.diagnostics[0].files, 3);
        assert!(
            result
                .records
                .iter()
                .all(|record| record.granularity == Granularity::CumulativeDelta)
        );
    }

    #[test]
    fn codex_unreconciled_file_overlap_is_warned_not_timestamp_token_deduped() {
        let temp = tempfile::tempdir().unwrap();
        let body = fs::read_to_string(fixture("codex-legacy.jsonl")).unwrap();
        write_fixture(temp.path(), "a.jsonl", &body);
        write_fixture(
            temp.path(),
            "b.jsonl",
            &body.replace(
                "SYNTHETIC_INSTRUCTIONS_NEVER_EXPORTED",
                "OTHER_SYNTHETIC_EVENT",
            ),
        );
        let result = read_codex(&[temp.path().into()]);
        assert!(!result.records.is_empty());
        assert!(
            result
                .records
                .iter()
                .all(|record| record.granularity == Granularity::Checkpoint
                    && record.billing == Billing::Unknown)
        );
        assert!(
            result.diagnostics[0]
                .warnings
                .iter()
                .any(|warning| warning.contains("unreconciled overlap"))
        );
    }

    #[test]
    fn codex_response_copies_dedup_globally_and_conflicts_remain_nonbillable() {
        let temp = tempfile::tempdir().unwrap();
        let body = fs::read_to_string(fixture("codex-requests.jsonl")).unwrap();
        write_fixture(temp.path(), "a.jsonl", &body);
        write_fixture(
            temp.path(),
            "b.jsonl",
            &body.replace(
                "\"id\":\"codex-new\"",
                "\"id\":\"codex-fork\",\"forked_from_id\":\"codex-new\"",
            ),
        );
        let result = read_codex(&[temp.path().into()]);
        assert_eq!(
            result
                .records
                .iter()
                .filter(|record| record.granularity == Granularity::Request)
                .count(),
            2
        );
        assert_eq!(
            result
                .records
                .iter()
                .filter(|record| record.ids.iter().any(|id| id.id == "resp-fixture-1"))
                .count(),
            1
        );
        write_fixture(
            temp.path(),
            "c.jsonl",
            &body.replace("\"output_tokens\":10", "\"output_tokens\":11"),
        );
        let conflict = read_codex(&[temp.path().into()]);
        let record = conflict
            .records
            .iter()
            .find(|record| record.ids.iter().any(|id| id.id == "resp-fixture-1"))
            .unwrap();
        assert_eq!(record.granularity, Granularity::Checkpoint);
        assert_eq!(record.billing, Billing::Unknown);
    }

    #[test]
    fn codex_missing_cache_counters_and_malformed_gaps_stay_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let mut body = String::from(
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"gap-fixture\"}}\n",
        );
        body += &codex_total("2026-01-01T00:00:01Z", 0, 0, 0);
        body += &codex_total("2026-01-01T00:00:02Z", 10, 2, 12);
        body += "MALFORMED_SYNTHETIC_SECRET\n";
        body += &codex_total("2026-01-01T00:00:03Z", 30, 5, 35);
        body += &codex_total("2026-01-01T00:00:04Z", 40, 6, 46);
        body += "{\"timestamp\":\"2026-01-01T00:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":-1}}}}\n";
        body += "{\"type\":\"unknown_rollout_format\",\"payload\":{\"secret\":\"SYNTHETIC_UNKNOWN_SECRET\"}}\n";
        write_fixture(temp.path(), "gap.jsonl", &body);
        let result = read_codex(&[temp.path().into()]);
        assert_eq!(result.diagnostics[0].skipped_lines, 2);
        assert_eq!(result.diagnostics[0].unsupported_records, 1);
        assert_eq!(result.records.len(), 3);
        assert_eq!(result.records[1].granularity, Granularity::Checkpoint);
        for record in &result.records {
            assert_eq!(record.tokens.cache_read_tokens, None);
            assert_eq!(record.tokens.cache_write_tokens, None);
        }
        assert!(!serialized(&result).contains("SYNTHETIC"));
    }

    #[test]
    fn bounded_native_read_continues_after_oversized_prompt_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.jsonl");
        let mut body = vec![b'x'; MAX_LINE_BYTES + 1];
        body.extend_from_slice(b"\n");
        body.extend_from_slice(
            fs::read(fixture("claude/primary.jsonl"))
                .unwrap()
                .as_slice(),
        );
        fs::write(&path, &body).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let result = read_claude(std::slice::from_ref(&path));
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.diagnostics[0].skipped_lines, 1);
        assert_eq!(fs::read(&path).unwrap(), body);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn importers_are_read_only_missing_roots_nonfatal_and_order_deterministic() {
        let temp = tempfile::tempdir().unwrap();
        let claude = write_fixture(
            temp.path(),
            "claude.jsonl",
            &fs::read_to_string(fixture("claude/primary.jsonl")).unwrap(),
        );
        let codex = write_fixture(
            temp.path(),
            "codex.jsonl",
            &fs::read_to_string(fixture("codex-legacy.jsonl")).unwrap(),
        );
        let snapshots: Vec<_> = [&claude, &codex]
            .iter()
            .map(|path| {
                (
                    fs::read(path).unwrap(),
                    fs::metadata(path).unwrap().modified().unwrap(),
                )
            })
            .collect();
        let missing = temp.path().join("absent");
        let first = read_claude(&[missing.clone(), claude.clone()]);
        let second = read_claude(&[claude.clone(), missing.clone()]);
        assert_eq!(serialized(&first), serialized(&second));
        let _ = read_codex(&[missing.clone(), codex.clone()]);
        assert_eq!(read_claude(std::slice::from_ref(&missing)).records.len(), 0);
        assert_eq!(read_codex(&[missing]).records.len(), 0);
        for (path, (bytes, modified)) in [&claude, &codex].iter().zip(snapshots) {
            assert_eq!(fs::read(path).unwrap(), bytes);
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn claude_api_error_placeholder_is_failed_unmeasured_and_not_a_request() {
        let result = read_claude(&[fixture("claude-api-error.jsonl")]);
        assert_eq!(result.records.len(), 1);
        let record = &result.records[0];
        assert_eq!(record.outcome, Outcome::Failed);
        assert_eq!(record.granularity, Granularity::Checkpoint);
        assert_eq!(record.billing, Billing::Unknown);
        assert!(record.ids.is_empty());
        assert!(record.model.is_none());
        assert_eq!(record.tokens.input_tokens, None);
        assert_eq!(record.tokens.output_tokens, None);
        assert_eq!(record.tokens.cache_read_tokens, None);
        assert_eq!(record.tokens.cache_write_tokens, None);
        assert!(
            record
                .warnings
                .iter()
                .any(|warning| warning.contains("placeholder"))
        );
        let output = serialized(&result);
        assert!(!output.contains("SYNTHETIC_"));
        assert!(!output.contains("locally-created-not-api-message"));
        assert!(!output.contains("synthetic-error-request"));
    }

    #[test]
    fn claude_partial_final_snapshot_does_not_add_or_fill_missing_counters() {
        let temp = tempfile::tempdir().unwrap();
        write_fixture(
            temp.path(),
            "partial.jsonl",
            concat!(
                "{\"type\":\"assistant\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"requestId\":\"r\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":3,\"cache_read_input_tokens\":10,\"cache_creation_input_tokens\":0,\"output_tokens\":1}}}\n",
                "{\"type\":\"assistant\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"requestId\":\"r\",\"message\":{\"id\":\"m\",\"usage\":{\"output_tokens\":9}}}\n"
            ),
        );
        let result = read_claude(&[temp.path().into()]);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].tokens.output_tokens, Some(9));
        assert_eq!(result.records[0].tokens.input_tokens, None);
        assert_eq!(result.records[0].tokens.cache_read_tokens, None);
        assert_eq!(result.records[0].tokens.cache_write_tokens, None);
    }

    #[test]
    fn codex_context_window_fill_cannot_be_used_as_next_billable_baseline() {
        let temp = tempfile::tempdir().unwrap();
        let mut body = String::from(
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"artificial-fixture\"}}\n",
        );
        body += &codex_total("2026-01-01T00:00:01Z", 0, 0, 0);
        body += &codex_total("2026-01-01T00:00:02Z", 0, 0, 1000);
        body += &codex_total("2026-01-01T00:00:03Z", 5, 2, 1007);
        write_fixture(temp.path(), "artificial.jsonl", &body);
        let result = read_codex(&[temp.path().into()]);
        assert_eq!(result.records.len(), 2);
        assert!(
            result
                .records
                .iter()
                .all(|record| record.granularity == Granularity::Checkpoint
                    && record.billing == Billing::Unknown)
        );
        assert!(
            result.records[1]
                .warnings
                .iter()
                .any(|warning| warning.contains("artificial context-window baseline"))
        );
    }

    #[test]
    fn codex_out_of_order_rows_fold_chronologically_and_component_resets_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let mut body = String::from(
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"chronology-fixture\"}}\n",
        );
        body += &codex_total("2026-01-01T00:00:01Z", 0, 0, 0);
        body += &codex_total("2026-01-01T00:00:03Z", 30, 5, 35);
        body += &codex_total("2026-01-01T00:00:02Z", 10, 2, 12);
        body += &codex_total("2026-01-01T00:00:04Z", 29, 10, 39);
        write_fixture(temp.path(), "chronology.jsonl", &body);
        let result = read_codex(&[temp.path().into()]);
        assert_eq!(result.records.len(), 3);
        assert_eq!(result.records[0].tokens.input_tokens, Some(10));
        assert_eq!(result.records[1].tokens.input_tokens, Some(20));
        assert_eq!(result.records[2].granularity, Granularity::Checkpoint);
        assert!(
            result.records[2]
                .warnings
                .iter()
                .any(|warning| warning.contains("reset"))
        );
    }

    #[test]
    fn codex_response_model_switch_within_a_turn_is_not_priced_as_latest_model() {
        let temp = tempfile::tempdir().unwrap();
        let mut body = fs::read_to_string(fixture("codex-requests.jsonl")).unwrap();
        body += "{\"timestamp\":\"2026-01-02T00:01:02Z\",\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"turn-new\",\"model\":\"gpt-another-fixture\"}}\n";
        write_fixture(temp.path(), "model-conflict.jsonl", &body);
        let result = read_codex(&[temp.path().into()]);
        assert_eq!(result.records.len(), 2);
        assert!(result.records.iter().all(|record| record.model.is_none()));
    }

    #[test]
    fn codex_oversized_row_prevents_cumulative_allocation_without_losing_records() {
        let temp = tempfile::tempdir().unwrap();
        let mut body = vec![b'x'; MAX_LINE_BYTES + 1];
        body.extend_from_slice(b"\n");
        body.extend_from_slice(fs::read(fixture("codex-legacy.jsonl")).unwrap().as_slice());
        let path = temp.path().join("large-codex.jsonl");
        fs::write(&path, &body).unwrap();
        let result = read_codex(std::slice::from_ref(&path));
        assert_eq!(result.diagnostics[0].skipped_lines, 1);
        assert_eq!(result.records.len(), 2);
        assert!(
            result
                .records
                .iter()
                .all(|record| record.granularity == Granularity::Checkpoint
                    && record.billing == Billing::Unknown)
        );
        assert_eq!(fs::read(path).unwrap(), body);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn neither_root_nor_descendant_symlinks_are_followed() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_file = write_fixture(
            &outside,
            "private.jsonl",
            &fs::read_to_string(fixture("claude/primary.jsonl")).unwrap(),
        );
        write_fixture(
            &outside,
            "codex-private.jsonl",
            &fs::read_to_string(fixture("codex-legacy.jsonl")).unwrap(),
        );
        symlink(&outside, root.join("linked-directory")).unwrap();
        symlink(&outside_file, root.join("linked-file.jsonl")).unwrap();
        symlink(&root, root.join("loop")).unwrap();
        symlink(&outside, temp.path().join("root-link")).unwrap();
        symlink(&outside_file, temp.path().join("file-link.jsonl")).unwrap();
        for result in [
            read_claude(&[
                root.clone(),
                temp.path().join("root-link"),
                temp.path().join("file-link.jsonl"),
            ]),
            read_codex(&[root]),
        ] {
            assert!(result.records.is_empty());
            assert_eq!(result.diagnostics[0].files, 0);
            assert!(
                result.diagnostics[0]
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("symlinks"))
            );
        }
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 2);
    }
}
