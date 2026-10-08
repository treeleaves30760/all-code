//! The launch-and-turn ledger `alc usage` aggregates.
//!
//! # Why a file rather than a counter
//!
//! alc is not a daemon. Every launch is its own process, and a shared session's
//! turns are served by a bridge inside a hub that outlives all of them, so
//! there is no single process whose memory could hold "how much have I spent
//! on this provider". An append-only file is the smallest thing several
//! unrelated processes can write at once and any of them can read later.
//!
//! # Why append-only, one line per row
//!
//! The hub runs several bridges in one process and a user runs several shells;
//! a format that rewrites the file would need a lock, and a lock left behind by
//! a killed hub is worse than a slightly larger file. One `write_all` of one
//! short line on an `O_APPEND` handle interleaves with nobody.
//!
//! # Why failures are dropped
//!
//! This is a diagnostic. A launch must not fail, and a turn must not be lost,
//! because a log line could not be written - so every write here ends in
//! `let _ =` and the reader counts what it could not parse instead of refusing
//! the whole file.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Value;

use super::records::{
    Billing, Granularity, InputBasis, Outcome, ReadResult, Source, SourceDiagnostics, TokenCounts,
    UsageRecord, read_lines,
};
use crate::config::{Agent, ProviderKind};
use crate::launch::LaunchSpec;

/// The ledger's file name inside alc's config directory.
pub(crate) const LEDGER_FILE: &str = "usage.jsonl";

/// One recorded event. `t` discriminates, `v` versions the row so a future
/// shape can be added without making today's rows unreadable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub(crate) enum Entry {
    /// A session started. Written once per launch, by every launch.
    Launch {
        v: u32,
        ts: u64,
        agent: Agent,
        provider: String,
        kind: ProviderKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        bridged: bool,
    },
    /// A completed model turn. Written only where alc carries the traffic,
    /// which today means a Codex-bridged session.
    Turn {
        v: u32,
        ts: u64,
        agent: Agent,
        provider: String,
        kind: ProviderKind,
        model: String,
        /// The ChatGPT account id the turn was billed to. Never a token: this
        /// is the same id the bridge already sends as a request header.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        /// The current writer always emits this field, including a real zero.
        /// Missing means the turn predates cache reporting or lacked a breakdown.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cached_tokens: Option<u64>,
        #[serde(default)]
        reasoning_tokens: u64,
        total_tokens: u64,
    },
    /// A measured request. Nullable counters preserve missing usage; IDs let a
    /// native history be reconciled without matching prompts or timestamps.
    Request { v: u32, record: Box<UsageRecord> },
}

const ROW_VERSION: u32 = 2;
const CACHE_REPORTING_ROW_VERSION: u32 = 2;

/// Everything a bridge needs to attribute the turns it sees.
///
/// Deliberately holds no credential: `account_id` is an identifier the bridge
/// already puts on the wire as a header, and nothing else about the login is
/// recorded.
#[derive(Debug)]
pub(crate) struct Ledger {
    path: PathBuf,
    agent: Agent,
    provider: String,
    kind: ProviderKind,
    account_id: Option<String>,
}

impl Ledger {
    pub(crate) fn new(
        path: PathBuf,
        agent: Agent,
        provider: String,
        kind: ProviderKind,
        account_id: Option<String>,
    ) -> Self {
        Self {
            path,
            agent,
            provider,
            kind,
            account_id,
        }
    }

    pub(crate) fn request_record(&self, model: &str) -> UsageRecord {
        let mut record = UsageRecord::new(Source::Alc, self.agent, now_unix().saturating_mul(1000));
        record.profile = Some(self.provider.clone());
        record.provider = Some(self.kind.as_str().to_owned());
        record.model = (!model.is_empty()).then(|| model.to_owned());
        record.account_id = self.account_id.clone();
        record.billing = if self.kind == ProviderKind::Codex {
            Billing::ApiEquivalent
        } else {
            Billing::Api
        };
        record
    }

    pub(crate) fn record_request(&self, record: UsageRecord) {
        append(
            &self.path,
            &Entry::Request {
                v: 3,
                record: Box::new(record),
            },
        );
    }

    /// Records that a session started.
    ///
    /// Called from `launch::prepare`, which both spawn paths go through and
    /// `--dry-run` returns before reaching - so a dry run still writes nothing.
    pub(crate) fn record_launch(config_dir: &Path, spec: &LaunchSpec) {
        append(
            &config_dir.join(LEDGER_FILE),
            &Entry::Launch {
                v: ROW_VERSION,
                ts: now_unix(),
                agent: spec.agent,
                provider: spec.provider_name.clone(),
                kind: spec.provider_kind,
                model: spec.model.clone(),
                bridged: spec.is_bridged(),
            },
        );
    }

    /// Records a turn if `data` is the frame that ends one.
    ///
    /// `data` is one SSE `data:` payload as it came off the wire. Anything
    /// that is not a terminal frame carrying a `usage` object is ignored, and
    /// the cheap `contains` check in front keeps the hundreds of delta frames
    /// in a turn from being parsed as JSON twice.
    #[cfg(test)]
    pub(crate) fn observe_frame(&self, data: &str, fallback_model: &str) {
        if !data.contains("response.completed") && !data.contains("response.incomplete") {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind != "response.completed" && kind != "response.incomplete" {
            return;
        }
        let Some(response) = value.get("response") else {
            return;
        };
        let Some(usage) = response.get("usage") else {
            return;
        };
        if !usage.is_object()
            || usage.get("input_tokens").and_then(Value::as_u64).is_none()
            || usage.get("output_tokens").and_then(Value::as_u64).is_none()
        {
            return;
        }
        let count =
            |parent: &Value, key: &str| parent.get(key).and_then(Value::as_u64).unwrap_or(0);
        let nested = |key: &str, inner: &str| {
            usage
                .get(key)
                .map(|details| count(details, inner))
                .unwrap_or(0)
        };
        let model = response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(fallback_model);
        let input_tokens = count(usage, "input_tokens");
        let reported_cached_tokens = usage
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64);
        // A missing or broken detail counter must not become a reported zero or
        // make a derived cache share exceed 100%. Preserve the gross input and
        // mark cache reads unknown instead.
        let cached_tokens = reported_cached_tokens.filter(|cached| *cached <= input_tokens);

        append(
            &self.path,
            &Entry::Turn {
                v: ROW_VERSION,
                ts: now_unix(),
                agent: self.agent,
                provider: self.provider.clone(),
                kind: self.kind,
                model: model.to_owned(),
                account_id: self.account_id.clone(),
                input_tokens,
                output_tokens: count(usage, "output_tokens"),
                cached_tokens,
                reasoning_tokens: nested("output_tokens_details", "reasoning_tokens"),
                total_tokens: count(usage, "total_tokens"),
            },
        );
    }
}

/// One line, one `write_all`, every error dropped. See the module doc.
fn append(path: &Path, entry: &Entry) {
    let Ok(mut line) = serde_json::to_string(entry) else {
        return;
    };
    line.push('\n');
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// What `alc usage` and the remote page show for one provider and agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LedgerRow {
    pub provider: String,
    pub kind: ProviderKind,
    pub agent: Agent,
    pub launches: u64,
    /// Zero means alc never carried this pair's traffic, so the token columns
    /// are unknown rather than zero. The renderers show a dash for it.
    pub turns: u64,
    pub input_tokens: u64,
    /// Input tokens served from Codex's prompt cache. `None` means at least one
    /// recorded turn omitted or did not have a trustworthy cache breakdown.
    #[serde(default)]
    pub cached_tokens: Option<u64>,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub last_at: u64,
}

/// The aggregate the report carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LedgerSummary {
    pub path: String,
    pub rows: Vec<LedgerRow>,
    pub first_at: Option<u64>,
    /// Lines that did not parse. Reported rather than hidden: a number here
    /// means something wrote to the file that alc does not understand.
    pub skipped_lines: u64,
    /// Set only when the file exists and could not be read. An absent file is
    /// not an error - it is a machine that has not launched anything yet.
    pub error: Option<String>,
}

impl LedgerSummary {
    fn empty(path: &Path) -> Self {
        Self {
            path: path.display().to_string(),
            rows: Vec::new(),
            first_at: None,
            skipped_lines: 0,
            error: None,
        }
    }
}

/// Reads the ledger and folds it into one row per provider and agent.
pub(crate) fn summarise(config_dir: &Path) -> LedgerSummary {
    let path = config_dir.join(LEDGER_FILE);
    let mut summary = LedgerSummary::empty(&path);
    if !path.exists() {
        return summary;
    }
    let mut rows: BTreeMap<(String, Agent), LedgerRow> = BTreeMap::new();
    let read = read_lines(&path, |line| {
        let Ok(mut entry) = serde_json::from_slice::<Entry>(line) else {
            summary.skipped_lines += 1;
            return;
        };
        if let Entry::Request { v, record } = &mut entry {
            if *v != 3 || record.source != Source::Alc {
                summary.skipped_lines += 1;
                return;
            }
            record.tokens.validate();
        }
        let (ts, provider, kind, agent) = match &entry {
            Entry::Launch {
                ts,
                provider,
                kind,
                agent,
                ..
            }
            | Entry::Turn {
                ts,
                provider,
                kind,
                agent,
                ..
            } => (*ts, provider.clone(), *kind, *agent),
            Entry::Request { record, .. } => {
                let (Some(provider), Some(kind)) = (
                    record.profile.as_ref(),
                    record
                        .provider
                        .as_deref()
                        .and_then(|kind| kind.parse().ok()),
                ) else {
                    return;
                };
                (
                    record.timestamp_ms / 1000,
                    provider.clone(),
                    kind,
                    record.agent,
                )
            }
        };
        summary.first_at = Some(summary.first_at.map_or(ts, |first| first.min(ts)));
        let row = rows
            .entry((provider.clone(), agent))
            .or_insert_with(|| LedgerRow {
                provider,
                kind,
                agent,
                launches: 0,
                turns: 0,
                input_tokens: 0,
                cached_tokens: Some(0),
                output_tokens: 0,
                total_tokens: 0,
                last_at: ts,
            });
        row.last_at = row.last_at.max(ts);
        match entry {
            Entry::Launch { .. } => row.launches += 1,
            Entry::Turn {
                v,
                input_tokens,
                cached_tokens,
                output_tokens,
                total_tokens,
                ..
            } => {
                // Before v2, a missing upstream cache breakdown was serialized as
                // zero, so no cache value from those rows is trustworthy.
                let cached_tokens = if v >= CACHE_REPORTING_ROW_VERSION {
                    cached_tokens
                } else {
                    None
                };
                row.turns += 1;
                row.input_tokens += input_tokens;
                row.cached_tokens = match (row.cached_tokens, cached_tokens) {
                    (Some(total), Some(cached)) => Some(total + cached),
                    _ => None,
                };
                row.output_tokens += output_tokens;
                row.total_tokens += total_tokens;
            }
            Entry::Request { record, .. } => {
                if !matches!(record.outcome, Outcome::Completed | Outcome::Incomplete) {
                    return;
                }
                let (Some(input), Some(output)) =
                    (record.tokens.gross_input(), record.tokens.output_tokens)
                else {
                    return;
                };
                row.turns = row.turns.saturating_add(1);
                row.input_tokens = row.input_tokens.saturating_add(input);
                row.output_tokens = row.output_tokens.saturating_add(output);
                row.total_tokens = row.total_tokens.saturating_add(
                    record
                        .tokens
                        .total_tokens
                        .unwrap_or_else(|| input.saturating_add(output)),
                );
                row.cached_tokens = match (row.cached_tokens, record.tokens.cache_read_tokens) {
                    (Some(total), Some(cached)) => total.checked_add(cached),
                    _ => None,
                };
            }
        }
    });
    match read {
        Ok(counts) => summary.skipped_lines += counts.oversized,
        Err(error) => summary.error = Some(error.to_string()),
    }
    summary.rows = rows.into_values().collect();
    summary
}

/// Metadata-only historical records, separate from the compatibility projection.
pub(crate) fn read_records(config_dir: &Path) -> ReadResult {
    let mut result = ReadResult::default();
    let mut diagnostics = SourceDiagnostics::new(Source::Alc);
    let path = config_dir.join(LEDGER_FILE);
    if !path.exists() {
        result.diagnostics.push(diagnostics);
        return result;
    }
    diagnostics.files = 1;
    let read = read_lines(&path, |line| {
        let Ok(entry) = serde_json::from_slice::<Entry>(line) else {
            diagnostics.skipped_lines += 1;
            return;
        };
        match entry {
            Entry::Launch { .. } => {}
            Entry::Request { v, mut record } => {
                if v != 3 || record.source != Source::Alc {
                    diagnostics.unsupported_records += 1;
                    return;
                }
                record.tokens.validate();
                result.records.push(*record);
            }
            Entry::Turn {
                v,
                ts,
                agent,
                provider,
                kind,
                model,
                account_id,
                input_tokens,
                output_tokens,
                cached_tokens,
                reasoning_tokens,
                total_tokens,
            } => {
                if v > 2 {
                    diagnostics.unsupported_records += 1;
                    return;
                }
                let mut record = UsageRecord::new(Source::Alc, agent, ts.saturating_mul(1000));
                record.profile = Some(provider);
                record.provider = Some(kind.as_str().to_owned());
                record.model = Some(model);
                record.account_id = account_id;
                record.outcome = Outcome::Unknown;
                record.granularity = Granularity::Request;
                record.billing = if kind == ProviderKind::Codex {
                    Billing::ApiEquivalent
                } else {
                    Billing::Unknown
                };
                record.tokens = TokenCounts {
                    input_tokens: (input_tokens > 0).then_some(input_tokens),
                    input_basis: InputBasis::Inclusive,
                    output_tokens: (output_tokens > 0).then_some(output_tokens),
                    cache_read_tokens: (v >= CACHE_REPORTING_ROW_VERSION)
                        .then_some(cached_tokens)
                        .flatten(),
                    cache_write_tokens: None,
                    reasoning_tokens: (reasoning_tokens > 0).then_some(reasoning_tokens),
                    total_tokens: (total_tokens > 0).then_some(total_tokens),
                    ..TokenCounts::default()
                };
                if input_tokens == 0 || output_tokens == 0 || total_tokens == 0 {
                    record
                        .warnings
                        .push("legacy zero usage has no field-presence evidence".to_owned());
                }
                record
                    .warnings
                    .push("legacy request has no timing or correlation IDs".to_owned());
                record.tokens.validate();
                result.records.push(record);
            }
        }
    });
    match read {
        Ok(counts) => diagnostics.skipped_lines += counts.oversized,
        Err(_) => diagnostics
            .warnings
            .push("could not read alc usage ledger".to_owned()),
    }
    diagnostics.records = result.records.len() as u64;
    result.diagnostics.push(diagnostics);
    result
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn_frame(input: u64, output: u64) -> String {
        format!(
            r#"{{"type":"response.completed","response":{{"model":"gpt-6-astra","usage":{{"input_tokens":{input},"output_tokens":{output},"total_tokens":{},"input_tokens_details":{{"cached_tokens":3}},"output_tokens_details":{{"reasoning_tokens":4}}}}}}}}"#,
            input + output
        )
    }

    fn ledger(dir: &Path) -> Ledger {
        Ledger::new(
            dir.join(LEDGER_FILE),
            Agent::Claude,
            "codex".to_owned(),
            ProviderKind::Codex,
            Some("acct_1".to_owned()),
        )
    }

    #[test]
    fn a_launch_row_and_a_turn_row_fold_into_one_line_per_provider_and_agent() {
        let dir = tempfile::tempdir().unwrap();
        let spec = LaunchSpec::saturated();
        Ledger::record_launch(dir.path(), &spec);
        let ledger = ledger(dir.path());
        ledger.observe_frame(&turn_frame(138, 21), "fallback");
        ledger.observe_frame(&turn_frame(12, 6), "fallback");

        let summary = summarise(dir.path());
        assert_eq!(summary.skipped_lines, 0);
        assert_eq!(summary.rows.len(), 1, "{summary:?}");
        let row = &summary.rows[0];
        assert_eq!(row.turns, 2);
        assert_eq!(row.input_tokens, 150);
        assert_eq!(row.cached_tokens, Some(6));
        assert_eq!(row.output_tokens, 27);
        assert_eq!(row.total_tokens, 177);
    }

    #[test]
    fn a_legacy_turn_without_cache_details_marks_the_counter_unknown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(LEDGER_FILE),
            r#"{"t":"turn","v":1,"ts":1,"agent":"claude","provider":"codex","kind":"codex","model":"m","input_tokens":100,"output_tokens":10,"reasoning_tokens":0,"total_tokens":110}
"#,
        )
        .unwrap();

        let summary = summarise(dir.path());
        assert_eq!(summary.rows[0].cached_tokens, None);
    }

    #[test]
    fn a_v1_turn_with_an_explicit_zero_marks_the_cache_counter_unknown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(LEDGER_FILE),
            r#"{"t":"turn","v":1,"ts":1,"agent":"claude","provider":"codex","kind":"codex","model":"m","input_tokens":100,"output_tokens":10,"cached_tokens":0,"reasoning_tokens":0,"total_tokens":110}
"#,
        )
        .unwrap();

        let summary = summarise(dir.path());
        assert_eq!(summary.rows[0].cached_tokens, None);
    }

    #[test]
    fn a_v2_turn_with_an_explicit_zero_preserves_the_measured_zero() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(LEDGER_FILE),
            r#"{"t":"turn","v":2,"ts":1,"agent":"claude","provider":"codex","kind":"codex","model":"m","input_tokens":100,"output_tokens":10,"cached_tokens":0,"reasoning_tokens":0,"total_tokens":110}
"#,
        )
        .unwrap();

        let summary = summarise(dir.path());
        assert_eq!(summary.rows[0].cached_tokens, Some(0));
    }

    #[test]
    fn an_old_aggregate_without_cached_tokens_marks_the_counter_unknown() {
        let row: LedgerRow = serde_json::from_value(serde_json::json!({
            "provider": "codex",
            "kind": "codex",
            "agent": "claude",
            "launches": 1,
            "turns": 1,
            "input_tokens": 100,
            "output_tokens": 10,
            "total_tokens": 110,
            "last_at": 1
        }))
        .unwrap();
        assert_eq!(row.cached_tokens, None);
    }

    #[test]
    fn a_missing_or_invalid_cache_counter_is_recorded_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        ledger(dir.path()).observe_frame(
            r#"{"type":"response.completed","response":{"model":"m","usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}}}"#,
            "m",
        );
        let summary = summarise(dir.path());
        assert_eq!(summary.rows[0].cached_tokens, None);

        let dir = tempfile::tempdir().unwrap();
        ledger(dir.path()).observe_frame(
            r#"{"type":"response.completed","response":{"model":"m","usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11,"input_tokens_details":{"cached_tokens":null}}}}"#,
            "m",
        );
        let summary = summarise(dir.path());
        assert_eq!(summary.rows[0].cached_tokens, None);
    }

    #[test]
    fn an_impossible_cache_counter_is_recorded_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        ledger(dir.path()).observe_frame(
            r#"{"type":"response.completed","response":{"model":"m","usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11,"input_tokens_details":{"cached_tokens":11}}}}"#,
            "m",
        );
        let summary = summarise(dir.path());
        assert_eq!(summary.rows[0].input_tokens, 10);
        assert_eq!(summary.rows[0].cached_tokens, None);
    }

    /// The distinction the table is built on: a direct launch is a launch with
    /// no tokens, not a launch that spent zero.
    #[test]
    fn a_provider_that_only_ever_launched_has_no_turns() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = LaunchSpec::saturated();
        spec.bridge = None;
        spec.provider_name = "ollama".to_owned();
        spec.provider_kind = ProviderKind::Ollama;
        Ledger::record_launch(dir.path(), &spec);

        let summary = summarise(dir.path());
        assert_eq!(summary.rows.len(), 1);
        assert_eq!(summary.rows[0].launches, 1);
        assert_eq!(summary.rows[0].turns, 0);
    }

    #[test]
    fn a_malformed_line_is_skipped_and_counted_rather_than_failing_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LEDGER_FILE);
        Ledger::record_launch(dir.path(), &LaunchSpec::saturated());
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"not json at all\n{\"t\":\"from-a-later-alc\",\"v\":9}\n")
            .unwrap();
        drop(file);
        Ledger::record_launch(dir.path(), &LaunchSpec::saturated());

        let summary = summarise(dir.path());
        assert_eq!(summary.skipped_lines, 2);
        assert_eq!(summary.rows[0].launches, 2);
        assert!(summary.error.is_none());
    }

    #[test]
    fn only_a_terminal_frame_carrying_usage_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = ledger(dir.path());
        ledger.observe_frame(r#"{"type":"response.output_text.delta","delta":"hi"}"#, "m");
        ledger.observe_frame("[DONE]", "m");
        ledger.observe_frame(r#"{"type":"response.completed","response":{}}"#, "m");
        ledger.observe_frame("not json", "m");

        assert!(summarise(dir.path()).rows.is_empty());
    }

    #[test]
    fn the_model_falls_back_to_the_requested_one_when_the_frame_omits_it() {
        let dir = tempfile::tempdir().unwrap();
        ledger(dir.path()).observe_frame(
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}"#,
            "gpt-5.6-terra",
        );
        let text = fs::read_to_string(dir.path().join(LEDGER_FILE)).unwrap();
        assert!(text.contains(r#""model":"gpt-5.6-terra""#), "{text}");
    }

    /// Several bridges append from one hub process and several shells append
    /// from their own; a row that arrived half-written would poison the file
    /// for every later read.
    #[test]
    fn concurrent_writers_never_interleave_a_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let ledger = ledger(&path);
                    for _ in 0..100 {
                        ledger.observe_frame(&turn_frame(1, 1), "m");
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }

        let summary = summarise(dir.path());
        assert_eq!(summary.skipped_lines, 0);
        assert_eq!(summary.rows[0].turns, 800);
    }

    #[test]
    fn each_legacy_zero_counter_is_unknown_without_changing_compatibility_totals() {
        for (input, output, total) in [(100, 0, 100), (0, 10, 10), (100, 10, 0)] {
            let dir = tempfile::tempdir().unwrap();
            let row = serde_json::json!({
                "t":"turn", "v":2, "ts":1, "agent":"claude",
                "provider":"codex", "kind":"codex", "model":"gpt-4.1",
                "input_tokens":input, "cached_tokens":0, "output_tokens":output,
                "reasoning_tokens":0, "total_tokens":total
            });
            fs::write(dir.path().join(LEDGER_FILE), format!("{row}\n")).unwrap();
            let record = read_records(dir.path()).records.remove(0);
            assert_eq!(record.tokens.input_tokens, (input > 0).then_some(input));
            assert_eq!(record.tokens.output_tokens, (output > 0).then_some(output));
            assert_eq!(record.tokens.total_tokens, (total > 0).then_some(total));
            assert_eq!(record.tokens.cache_read_tokens, Some(0));
            if input == 0 || output == 0 {
                let cost = crate::usage::pricing::PriceBook::load(dir.path(), None)
                    .unwrap()
                    .estimate(&record);
                assert_eq!(cost.total_usd, None);
            }
            let summary = summarise(dir.path());
            assert_eq!(summary.rows[0].input_tokens, input);
            assert_eq!(summary.rows[0].output_tokens, output);
            assert_eq!(summary.rows[0].total_tokens, total);
        }
    }

    #[test]
    fn an_absent_ledger_is_an_empty_summary_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let summary = summarise(dir.path());
        assert!(summary.rows.is_empty());
        assert!(summary.error.is_none());
        assert!(summary.path.ends_with(LEDGER_FILE));
    }
}
