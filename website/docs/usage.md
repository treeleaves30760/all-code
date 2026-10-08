---
id: usage
title: Usage
sidebar_label: Usage
sidebar_position: 6
description: Check Claude and Codex quota, inspect client-observed TTFT and tokens per second, and estimate token costs from local histories with offline pricing.
keywords:
  - alc usage
  - alc tps
  - TTFT
  - tokens per second
  - estimated token cost
  - claude code usage
  - codex quota
  - chatgpt plan limit
  - openrouter credits
  - multiple accounts
---

# Usage

What is left on each login, which agent used tokens, and how new requests performed.

```sh
alc usage                   # Accounts, compatibility ledger, token/cost statistics
alc usage --offline         # local statistics only; no credentials or network
alc tps                     # latest 20 matching alc request records
```

The normal usage report keeps **Accounts** and **Usage by provider and agent**, then
adds **Token usage and estimated cost (USD)**. Accounts and compatibility-ledger
excerpt:

```text
Accounts
     PROFILE     ACCOUNT                  PLAN  REMAINING
  ✓  anthropic   ~/.claude                max   5h 97% left, resets in 2h 53m — week 79% left, resets in 6d 4h — Fable week 62% left, resets in 6d 4h
  ✓  codex       you@example.com          pro   week 66% left, resets in 5d 9h — no credits
  ·  ollama      —                        —     no quota API
  ·  openrouter  —                        —     no API key; run `alc config key openrouter`

Usage by provider and agent
  PROVIDER  AGENT     LAUNCHES  TURNS  INPUT  CACHED  CACHE %  OUTPUT  LAST
  codex     claude    1         1      20.8K  15.4K   74%      35      7m ago
  ollama    opencode  1         —      —      —        —       —       12m ago
  source: ~/.config/alc/usage.jsonl — tokens are counted only where alc carries the traffic; an unobserved direct launch counts as a launch alone (opt in with --metrics)

✓ ready
```

One Accounts row per enabled provider profile. `REMAINING` counts down: `63% left`
is what you have, not what you have spent. The exit code is 1 when a login has
expired or been refused, or when a vendor could not be reached or answered an
error; 0 otherwise, so a plan that is simply used up does not fail a script.
Invalid query options, unreadable pricing files, and similar command errors still
fail; an unknown cost is not a quota failure.

`alc --provider codex-work usage` or `alc --codex usage` filters **Accounts only**.
It does not filter the compatibility ledger or statistics. Use
`--filter-profile codex-work` for recorded statistics. `alc usage --json` keeps
the existing `schema_version: 1`, `accounts`, and `ledger` fields and adds a
`statistics` object; see [JSON reports](#json-reports).

## Codex logins

alc reads the `auth.json` that `codex login` wrote and asks chatgpt.com what is
left on it. Nothing is written back: OpenAI's refresh tokens are single-use, so
a status command that rotated one would invalidate the token a running session
is holding. An expired token says `codex login`, and the next real session
renews it anyway.

## Claude logins

alc reads Claude Code's own login — the macOS Keychain first, then
`.credentials.json` in its config directory — and asks api.anthropic.com for
the five-hour and weekly windows, plus a per-model window where your plan has
one. Read-only, never refreshed. macOS may ask once for permission to read the
Keychain item.

An Anthropic profile that uses an API key instead shows `no quota API`: that
key bills per token and has no remaining balance to report.

## Several logins of one kind

Two ChatGPT logins are two profiles. Point each at its own directory and every
launch through that profile uses that account, so the row you read is the
account you spend:

```sh
CODEX_HOME=~/.codex-work codex login
alc config upsert codex-work --kind codex --codex-home ~/.codex-work
alc --provider codex-work claude
```

The same for Claude Code, with the directory it keeps its login in:

```sh
CLAUDE_CONFIG_DIR=~/.claude-work claude          # sign in once
alc config upsert anthropic-work --kind anthropic --claude-config-dir ~/.claude-work
```

Both paths must be absolute. A profile's directory beats `CODEX_HOME` or
`CLAUDE_CONFIG_DIR` in your shell, so a variable left in a shell profile cannot
quietly move which account a named profile spends.

`--provider` and the kind shortcuts match an exact profile name first, then a
kind. So with profiles named `codex` and `codex-work`, `alc --codex claude`
resolves to the one actually named `codex` rather than asking which you meant —
name the other explicitly, as above. The shortcut only refuses to choose when
no profile carries the kind's own name and several share that kind.

## Providers with a balance API

| Kind | What the row shows |
| --- | --- |
| `openrouter` | Credit limit, used and remaining |
| `deepseek` | Balance, in the currency the account is billed in |
| `moonshot` | Available balance |
| `minimax` | Remaining requests per window |
| `zai` | Token windows and credit balance |

Everything else — OpenAI, Groq, xAI, Google, Ollama, vLLM, llama.cpp, custom endpoints —
reports `no quota API`, because none publishes one for an API key. These
requests go to the vendor's own endpoint, so a profile pointed at a proxy is
reported as having no quota API rather than having its key sent to a host that
did not issue it.

## Usage by provider and agent

This compatibility table reads only `usage.jsonl` in the [config
directory](./configuration.md). Launches append launch entries; requests carried
by the [Codex translation bridge](./codex-to-claude.md) or the opt-in direct API
observer append metadata entries. Ordinary direct launches still count as
launches alone. Native histories are separate statistics sources, not imports
into this ledger.

`INPUT` is total upstream input. `CACHED` is the part read from the prompt
cache. `CACHE %` is `CACHED / INPUT`, rounded to the nearest whole percent; it
is a token share, not a request hit rate. `0` is a measured zero. `—` means the
value is unknown or cannot be computed, including cache data from an older hub.

The JSON `ledger` rows keep the raw nullable `cached_tokens` total. The percentage
is calculated only for display and is not a JSON field. [Codex through Claude
Code](./codex-to-claude.md#auto-mode-and-prompt-caching) explains when prompt
reuse can miss.

For traffic alc did not carry, token columns show `—`, not zero. For example,
ordinary `alc claude` on Anthropic is direct; adding `--metrics` opts into
observation where supported. Deleting `usage.jsonl` resets alc's recorded
history, not the separate Claude or Codex histories.

## TTFT and tokens per second

```sh
alc tps
alc tps --limit 50 --filter-profile work --filter-agent claude
alc tps --since 2026-10-01 --until 2026-10-08 --json
```

`alc tps` reads local records without credentials, quota queries, or network
access. It defaults to `--source alc --limit 20`, newest matching records first;
`--limit` accepts 1–10000. A launch is not a performance sample: measurements
start with new requests carried by the translation bridge or a supported
`alc --metrics <agent>` launch. Old ledger entries and native histories have no
observed request timing and show `N/A`.

These are **client-observed measurements**, not a model-server benchmark:

| Column | Meaning |
| --- | --- |
| `TTFT ms` | Request start to the first nonempty generated text, exposed thinking, or tool-argument content seen by alc on a streaming request. Headers, role-only or empty deltas, usage, pings, reasoning summaries, and signatures do not start the clock. A bridge that suppresses reasoning waits for visible content. |
| `TPS est.` | Streaming estimate: `(N - 1) / (terminal - first matching content)`, with the span in seconds. `N` is output in the known measured token domain; subtract reported reasoning when only non-reasoning output is represented. Unknown output/reasoning basis, no terminal, `N <= 1`, or a nonpositive span means `N/A`. It does not count SSE chunks as tokens. |
| `BASIS` | Streaming numerator domain: `gross` (reported output), `non-reasoning` (output minus explicitly reported reasoning), or `unknown` (streaming TPS unavailable). This does not change gross E2E TPS. |
| `TPS E2E` | Gross reported output tokens divided by request-start-to-terminal seconds. Includes queueing, network, prompt processing, and reasoning; **not server decode speed**. |

Nonstreaming requests can have E2E TPS when output and a terminal time are known,
but TTFT and streaming TPS are `N/A`. Failures, cancellations, timeouts, and
truncated streams retain their outcomes; missing counters or a missing terminal
are not fabricated as zero or successful timing.

The summary shows valid sample counts separately for TTFT, streaming TPS, and
E2E TPS. TTFT has a mean, p50, and p95. Weighted TPS divides summed valid token
numerators by summed corresponding durations; it is neither the arithmetic mean
of request speeds nor the sum of concurrent speeds.

### Opt-in direct API observation

```sh
alc --metrics --provider openai codex --config model_providers.alc_openai.supports_websockets=false
alc --openrouter --metrics claude
alc --openrouter --metrics --dry-run claude
```

Put `--metrics` **before the agent name**. It adds a protected loopback HTTP
forwarding route at an alc-managed endpoint seam, keeping the agent's SDK,
protocol, models, arguments, and **upstream authentication**. Durable Claude
observation replaces the local helper credential with a sealed surrogate as
explained below. It is not protocol translation. Without it, ordinary direct
launch behavior is unchanged; the Codex translation bridge is already observed
and needs no opt-in.

| Direct launch | Observation boundary |
| --- | --- |
| Claude Code with an alc-managed API-key helper | Anthropic-compatible HTTP endpoint. The durable host/helper uses a route/instance-bound sealed local credential while preserving upstream API-key authentication; [background sessions](./background-sessions.md#api-key-metrics-in-background-sessions) can restart it. |
| Codex CLI with an alc-generated API endpoint | HTTP Responses requests only, and only when you explicitly select HTTP-only with `--config model_providers.alc_<profile-normalized>.supports_websockets=false`. Missing/true is refused. WebSocket traffic is not observed; alc does not force-disable it. Native Codex login and native Ollama integration are not observed. |
| OpenCode / Copilot CLI | Supported alc-managed Anthropic- or OpenAI-compatible endpoint configuration. |
| Qwen Code / Goose | Supported alc-managed Anthropic or OpenAI branches. Qwen's Google/Gemini branch and Goose's native OpenRouter/Ollama integrations are not observed. Goose's OpenAI split endpoint with a query or fragment is refused for metrics; ordinary launches are unchanged. |
| Kimi Code CLI | The provider endpoint in alc's generated temporary config, not a user-supplied config. |
| Pi | Direct observation is unavailable: alc will not put an ephemeral listener into its persistent shared `models.json`. |

Codex's generated provider ID is `alc_` plus the alc profile name with hyphens
changed to underscores: profile `openai-work` needs
`model_providers.alc_openai_work.supports_websockets=false`. This must be an
explicit passthrough argument for the metrics launch; alc does not infer it by
reading Codex config files. Ordinary no-metrics WebSocket behavior is unchanged.

Native OAuth/login authentication and explicit endpoint, helper, config, or
provider overrides outside the supported seam are left untouched. An explicit
`--metrics` request that cannot be observed safely is refused with a reason,
rather than silently switching provider, auth, or transport. `--dry-run` checks
and describes the plan but starts no listener and writes nothing. This matrix
describes supported configuration seams, not universal coverage of every SDK,
transport, or provider-specific response extension. Durable Claude metrics also
refuses a profile whose selected `api_key_env` is `ANTHROPIC_API_KEY` or
`ANTHROPIC_AUTH_TOKEN` and exported nonempty, because the client would bypass the
sealed helper: save the key with `alc config key <profile>`, unset that exported
variable, and relaunch; ordinary no-metrics auth behavior is unchanged.

For **durable Claude API-key observation**, the helper authenticates the host
with the independent owner-only `run/bridge.observer-key` and a fresh challenge,
then returns an AEAD-sealed surrogate, not the vendor key. It is bound to the
frozen route and current host instance; the host restores the original upstream
key/header only when dispatching to the fixed endpoint. Registrations retain
key digests only, and observation artifacts never write the plaintext vendor
key. Old surrogates receive HTTP 401 after a host restart; rerun the helper to
create a new one. The handshake/control challenges do not publish the secret;
`forward-observer-v2` capability checks refuse older daemons even with the same
version string. Stop the old host with `alc bridge stop`, then relaunch.

This is not TLS or full local data-plane confidentiality. Requests still cross
loopback HTTP, and local processes remain trusted: the Claude surrogate avoids
raw vendor-key disclosure, but a hijacked local port can intercept plaintext
request bodies. Do not assume other agents' ephemeral routes also seal their
credentials.

## Local history and token/cost statistics

```sh
alc usage --offline --daily --since 2026-10-01 --until 2026-10-08
alc usage --offline --monthly --source claude,codex --json
alc usage --offline --source alc --filter-profile work --filter-model example-model
alc usage --offline --claude-dir "$HOME/.claude-work" --codex-dir "$HOME/.codex-work"
```

`--offline` reads local configuration, histories, and pricing only. It does not
read API keys, login files, or the Keychain, refresh credentials, query quota,
fetch prices, or otherwise access the network. Text output says `Accounts: not
fetched (--offline)`; JSON has an empty `accounts` array and still includes the
compatibility `ledger`. `--daily` and `--monthly` are mutually exclusive and
bucket statistics in **UTC**; without either, the period is `all-time`.

### Shared query options

These options apply to the statistics in `alc usage` and to `alc tps`, not to
Accounts or the compatibility ledger:

| Option | Meaning |
| --- | --- |
| `--source all\|alc\|claude\|codex` | Comma-separated sources are accepted, for example `--source alc,claude`. Usage defaults to `all`; TPS defaults to `alc`. |
| `--since DATE` | Inclusive start. `YYYY-MM-DD` means midnight UTC; RFC3339 offsets are accepted. |
| `--until DATE` | Exclusive end, in the same formats. To include all of October 7 UTC, use `--until 2026-10-08`. |
| `--filter-profile PROFILE` | Exact recorded alc profile. Native records without a profile do not match. |
| `--filter-agent AGENT` | Recorded coding agent: `claude`, `codex`, `opencode`, `pi`, `copilot`, `goose`, `qwen`, or `kimi`. |
| `--filter-model MODEL` | Exact reported model ID, not a fuzzy alias search. |
| `--claude-dir PATH` | Repeatable absolute Claude **config root**; reads `projects/` beneath it. |
| `--codex-dir PATH` | Repeatable absolute Codex **home**; reads `sessions/` and `archived_sessions/` beneath it. |

An explicit root list replaces automatic roots for that native source. Otherwise
alc includes directories pinned in provider profiles, plus `CLAUDE_CONFIG_DIR`
or `CODEX_HOME` when set, or the corresponding `~/.claude` / `~/.codex` fallback.
Roots are deduplicated. They identify where to read, not which current profile
or account to attribute past native usage to.

### Coverage and reconciliation

Native readers are read-only and retain usage/identity metadata only. They do not
copy prompts, generated output, tool content, or keys into alc's ledger or an
import cache. JSONL inspection is bounded to 4 MiB per line; oversized,
malformed, unsupported, and ambiguous records are reported through source
coverage diagnostics instead of being treated as zero usage.

Claude assistant snapshots are reconciled by stable message/request identity,
not transcript-block UUIDs. Codex records with stable response IDs can identify
requests. Older cumulative token counters are folded into defensible deltas;
first nonzero baselines, counter resets, artificial context-window checkpoints,
or unallocatable model changes remain checkpoints. **Cumulative deltas and
checkpoints are not request counts**, and checkpoints are not summed as token
usage. Neither has invented TTFT/TPS. If selected with `alc tps --source codex`,
these usage records can appear as rows with `N/A` timing.

Across sources, only exact, protocol-qualified request/message/response IDs for
the same agent prove a duplicate; alc records take precedence for a proven
match. Similar timestamps, token totals, or session names are not evidence.
Unverified overlaps are retained, flagged `possible_overlap`, and shown as
source subtotals **without an additive grand total**. A source warning or skipped
record likewise makes overall coverage incomplete. For legacy v1/v2 alc turn
entries, each zero input/output counter is independently unknown in the new
statistics because those rows lack field-presence evidence; positive counters
are retained. The compatibility ledger's existing totals are unchanged.

### What an estimated dollar means

Costs are **USD token-rate estimates** from the named pricing snapshot, not an
invoice, subscription bill, quota deduction, or proof of actual spend. Native
and subscription traffic uses API-equivalent reference pricing when an exact
reference is available. Native metadata is not relabeled with today's alc
profile; reference pricing does not change unknown provider attribution.

Input, cache reads, cache writes, and output remain separate cost components.
OpenAI-style input includes cache subsets; Anthropic-style input is the uncached
remainder, so gross input adds reads and writes. Reasoning is a subset of output,
not an extra billable token count. Cache-write TTL buckets replace the aggregate,
not add to it. Unknown TTL splits are not guessed when 5-minute and 1-hour rates
differ.

Missing counters or applicable exact rates, an unpriced recorded service tier,
or unavailable per-request context for banded rates produce unknown/partial costs
with reasons. Absent service-tier metadata assumes standard; OpenAI's `default`
maps to standard. Text shows `N/A` or a known subtotal
plus `?`; JSON leaves `total_usd` as `null`. A known zero count is not an absent
count, and a missing model price is **not free**. Local/custom endpoints need an
exact override unless an exact official endpoint supplies a supported reference;
free reference rates must be explicit `"0"` strings.

Pricing is an offline curated LiteLLM subset, not the full upstream catalog or
a live price feed. The bundled snapshot is dated **2026-10-08**, pinned to
LiteLLM commit `33d908e0ae2c0a257eeb5d546df08527d348a670`, with upstream SHA-256
and MIT license provenance. Historical usage is repriced with this snapshot;
taxes, discounts, subscriptions, unrecorded tools, and non-token charges are
excluded. Add exact rates in the config directory's **`pricing.toml`** or select
one with **`alc usage --pricing-file PATH`**. The [pricing sidecar
reference](./configuration.md#pricing-sidecar) gives the schema; it is not a table
in the main `config.toml`.

## JSON reports

- `alc usage --json`: existing top-level `schema_version: 1`, `generated_at`,
  `resolved_by`, `accounts`, and `ledger`, plus `statistics` (also schema version
  1). Statistics expose UTC periods, source/profile/provider/agent/model rows,
  `granularity`, nullable token totals, `records`, nullable `requests`,
  `known_requests`, `priced_records`, `unpriced_records`,
  `deduplicated_records`, `known_subtotal_usd`, nullable `total_usd`,
  `possible_overlap`, `pricing_snapshot`, and source diagnostics. Rows include
  `cost_status` (`complete`, `partial`, or `unknown`), `partial_records`,
  `reference_providers`, `reference_models`, `price_sources`, `provenance`,
  assumptions, and reasons. USD amounts are decimal **strings**, not
  floating-point JSON numbers. Overall `known_subtotal_usd` is also `null` when
  sources may overlap.
- `alc tps --json`: `schema_version: 1`, `measurement`, `rows`, `summary`, and
  `sources`. Rows contain metadata records, `provenance`, raw `timing` offsets in
  microseconds, token counters, outcome, and `metrics` (`ttft_ms`, `stream_tps`,
  `e2e_tps`, `stream_output_basis`). The summary includes `records`,
  `known_requests`, nullable `requests`, valid sample counts, `ttft_mean_ms`,
  `ttft_p50_ms`, `ttft_p95_ms`, `weighted_stream_tps`, and `weighted_e2e_tps`;
  unavailable metrics are `null`. `requests` is `null` when any selected row is
  a cumulative delta or checkpoint, not a verified API request.

## On the remote-control page

The [remote-control page](./remote-control.md) retains Accounts and the
compatibility ledger behind the usage button in its header: one meter per
window, then the ledger. It refreshes once a minute while that pane is open.
Native-history statistics, cost estimates, and the per-request TPS report are
CLI-only; the hub does not scan native histories for this page.

A link that can watch but not type sees the numbers without the email, the
account id or the credential path. The page is served by the hub, which reads
no shell variable and never opens the Keychain — so on macOS the Claude row
there points you back at `alc usage` in a terminal.
