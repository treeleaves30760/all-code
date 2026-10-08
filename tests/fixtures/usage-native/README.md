# Synthetic native usage fixtures

Every ID, model, prompt, tool input, workspace, and counter in these files was
invented. No personal session history, credential, or downloaded execution result
was used. Importers read only caller-provided roots; these tests do not resolve a
user's Claude/Codex directories.

## Verified source contracts

- Claude Code **2.1.39**, official distributed source
  <https://registry.npmjs.org/@anthropic-ai/claude-code/-/claude-code-2.1.39.tgz>.
  `package/cli.js` emits assistant entries with `requestId`, `timestamp`, `uuid`,
  and a nested API `message`. Content-block completion emits separate entries
  sharing the API message ID; `message_delta` updates the last entry's usage.
  Content splitting preserves `message.id` and `requestId`, not the entry UUID.
  API-error synthetic messages are identified by `isApiErrorMessage`.
- Official Claude Agent SDK Python transcript parser at
  **f7b0b62c2a8d110d4da0eec0aa70cf795ec3afc4**
  <https://github.com/anthropics/claude-agent-sdk-python/blob/f7b0b62c2a8d110d4da0eec0aa70cf795ec3afc4/src/claude_agent_sdk/_internal/sessions.py>
  and official synthetic transcript tests
  <https://github.com/anthropics/claude-agent-sdk-python/blob/f7b0b62c2a8d110d4da0eec0aa70cf795ec3afc4/tests/test_sessions.py>
  establish `sessionId`, `type`, `uuid`, `timestamp`, and nested `message`.
- Claude documentation verified 2026-10-08
  <https://code.claude.com/docs/en/claude-directory>,
  <https://code.claude.com/docs/en/monitoring-usage>, and
  <https://code.claude.com/docs/en/agent-sdk/cost-tracking>
  describe recursive project/subagent JSONL transcripts, one API response entry
  per content block, and deduplication by API message ID. These are native
  transcripts, not `--output-format stream-json` result-message aggregates.
- Anthropic's official API types
  <https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/usage.py>
  and
  <https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/cache_creation.py>
  verified 2026-10-08 define uncached `input_tokens`, cache read/create counters,
  `output_tokens`, and optional `cache_creation.ephemeral_5m_input_tokens` and
  `ephemeral_1h_input_tokens`. Absent fields are unknown, not measured zero.
- Codex public source pinned at
  **e974aad3b1a8f144273e882c614aefe69eaef615** (2026-10-08):
  <https://github.com/openai/codex/blob/e974aad3b1a8f144273e882c614aefe69eaef615/codex-rs/protocol/src/protocol.rs>
  defines `SessionMeta`, `TurnContextItem`, `TokenUsage`, `TokenUsageInfo`,
  `TokenCountEvent`, and `TokenUsageRecord`.
  `TokenUsageInfo.append_last_usage` adds to totals but overwrites last usage;
  `fill_to_context_window` replaces component counters with zero and sets an
  artificial total. `cache_write_input_tokens` has an upstream serde legacy
  default, but unversioned imports deliberately retain absence as unknown.
- Current Codex **persisted** per-response usage is a top-level rollout item,
  NOT an invented `EventMsg` variant:
  <https://github.com/openai/codex/blob/e974aad3b1a8f144273e882c614aefe69eaef615/codex-rs/history/src/rollout_payload.rs>
  specifies `{type:"token_usage_record",payload:{...}}` and
  <https://github.com/openai/codex/blob/e974aad3b1a8f144273e882c614aefe69eaef615/codex-rs/rollout/src/policy.rs>
  explicitly persists both `RolloutItem::TokenUsageRecord` and legacy
  `EventMsg::TokenCount`. The upstream integration test
  <https://github.com/openai/codex/blob/e974aad3b1a8f144273e882c614aefe69eaef615/codex-rs/core/tests/suite/token_usage_rollout.rs>
  reads the written JSONL and asserts per-response IDs and totals. The recorder
  <https://github.com/openai/codex/blob/e974aad3b1a8f144273e882c614aefe69eaef615/codex-rs/rollout/src/recorder.rs>
  writes a timestamp plus flattened rollout type/payload and optional ordinal.

## Import semantics exercised

- Claude content-block UUIDs and copies in another session do not multiply
  requests. Prefer the latest authoritative snapshot, not a sum of partials.
  Stable correlations are `messages` plus `claude-request` when unambiguous.
  Missing tokens/model/provider stay unknown; no current-login inference.
  `claude-api-error.jsonl` exercises Claude's locally fabricated API-error
  placeholders: keep Failed outcome, but unknown counters/model, no protocol
  IDs, and Checkpoint/Unknown billing, never an invented zero-token request.
- Codex per-response rows use `usage`, never `turn_token_usage` or
  `thread_token_usage` as a new billable request. Response IDs correlate through
  `responses` globally across copied sessions. Conflicting copies are warned
  checkpoints.
- Legacy token-count totals are folded chronologically for the entire session
  before caller date filtering. Repeated checkpoints and repeated last usage
  do not add. Deltas have no inferred request IDs/count. First nonzero totals
  without zero baseline, resets, unallocatable model changes, artificial fills,
  malformed gaps, and unreconciled overlap are nonbillable checkpoints.
- Known forks/inherited legacy histories without a proven local ordinal boundary
  are checkpoint-only; their arbitrary copied prefix cannot be billed again.
- Exact archive/file copies and identical prefixes are reconciled using stable
  session metadata plus SHA-256 of complete source-event rows, not a heuristic of
  timestamp + token counts. Unreconciled copies expose an overlap warning.
  Hashes are in-memory only and are never emitted.
- A mixed current/legacy stream suppresses a cumulative increment only when its
  component delta exactly matches the intervening per-response usage sum and the
  persisted request's thread-total endpoint. Otherwise retain an explicitly
  warned checkpoint, not double-bill the increment.
- Typed deserialization ignores prompt/content/tool/rate-limit fields. Malformed
  counters and oversized rows get bounded, content-free diagnostics. Every
  symlink (including roots, files, directories, and loops) is skipped. Reading
  leaves source bytes, modification times, and directory entries unchanged.

`claude-malformed.jsonl` intentionally includes invalid JSON, negative/string/
overflow counters, an invalid timestamp, inconsistent cache TTL breakdown, a
missing ID, a missing usage object, and a truncated final row.
