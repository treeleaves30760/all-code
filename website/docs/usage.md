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
alc usage                   # Accounts, compatibility ledger, daily token/cost statistics
alc usage --offline         # local statistics only; no credentials or network
alc usage weekly --offline --timezone Asia/Taipei --chart
alc usage yearly --wrapped  # one shareable image across every agent and provider
alc tps                     # latest 20 matching requests with recorded timing
```

The normal usage report shows **Accounts**, **Usage by provider and agent**, then
**Token usage** and **By model**:

```text
Accounts
     PROFILE     ACCOUNT          PLAN  LEFT        REMAINING
  ✓  anthropic   ~/.claude        max   ▰▰▰▰▰▰▰▰▱▱  5h 97% left, resets in 2h 53m — week 79% left, resets in 6d 4h
  ✓  codex       you@example.com  pro   ▰▰▰▰▰▰▰▱▱▱  week 66% left, resets in 5d 9h — no credits
  ·  ollama      —                —     —           no quota API
  ·  openrouter  —                —     —           no API key; run `alc config key openrouter`

Usage by provider and agent
  ╭──────────┬──────────┬──────────┬───────┬────────┬────────┬─────────┬────────┬─────────╮
  │ Provider │ Agent    │ Launches │ Turns │  Input │ Cached │ Cache % │ Output │ Last    │
  ├──────────┼──────────┼──────────┼───────┼────────┼────────┼─────────┼────────┼─────────┤
  │ codex    │ claude   │        1 │     1 │ 20,800 │ 15,400 │     74% │     35 │ 7m ago  │
  │ ollama   │ opencode │        1 │     — │      — │      — │       — │      — │ 12m ago │
  ╰──────────┴──────────┴──────────┴───────┴────────┴────────┴─────────┴────────┴─────────╯
  ~/.config/alc/usage.jsonl — tokens count only where alc carries the traffic (opt in with --metrics)

✓ ready

Token usage
  since 2026-10-05 · UTC · 3,412 records · prices alc-curated-2026-10-08-litellm-33d908e0
  ╭────────────┬───────────────┬──────────────────────┬───────────┬─────────┬─────────────┬────────────┬──────────────┬─────────╮
  │ Date       │ Agents        │ Models               │     Input │  Output │ Cache write │ Cache read │ Total tokens │    Cost │
  ├────────────┼───────────────┼──────────────────────┼───────────┼─────────┼─────────────┼────────────┼──────────────┼─────────┤
  │ 2026-10-05 │ claude, codex │ gpt-5.2-codex,       │ 1,204,330 │  88,410 │     310,201 │ 21,733,090 │   23,336,031 │  $19.42 │
  │            │               │ sonnet-4-6           │           │         │             │            │              │         │
  │ 2026-10-06 │ claude        │ sonnet-4-6           │   402,118 │  51,902 │     120,554 │  9,108,422 │   9,682,996+ │  $6.71+ │
  ├────────────┼───────────────┼──────────────────────┼───────────┼─────────┼─────────────┼────────────┼──────────────┼─────────┤
  │ Total      │               │                      │ 1,606,448 │ 140,312 │     430,755 │ 30,841,512 │  33,019,027+ │ $26.13+ │
  ╰────────────┴───────────────┴──────────────────────┴───────────┴─────────┴─────────────┴────────────┴──────────────┴─────────╯
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
alc tps --since 2026-10-01 --until 2026-10-08 --timezone Asia/Taipei --json
alc tps --include-unmeasured --source all --json
```

`alc tps` reads local records without credentials, quota queries, network access,
or probing active daemons. It defaults to `--source alc --limit 20`. After the
query filters, it selects **actual `Request` records with a timing object before
sorting newest-first and applying the limit**. Newer historical turns cannot hide
older measured requests. `--limit` accepts 1–10000.

A launch is not a performance sample. Measurements start with requests carried
by a capable translation bridge or a supported `alc --metrics <agent>` launch.
Coverage counts identify excluded legacy, untimed, and non-request records.
`--include-unmeasured` restores the historical view, including native deltas and
checkpoints; unavailable metrics remain `N/A` rather than becoming zero.

An old all-`N/A` report can contain v1/v2 turns from a reused older bridge that
never recorded request timing. There is no historical TTFT/TPS to reconstruct.
New launches use their generation's host and require `request-metrics-v3` for
request measurement; older hosts keep serving older sessions. Start a new
session after upgrading for future measurements, without stopping old work.
A version string alone does not prove measurement capability.

These are **client-observed measurements**, not a model-server benchmark:

| Column | Meaning |
| --- | --- |
| `TTFT ms` | Request start to the first nonempty generated text, exposed thinking, or tool-argument content seen by alc on a streaming request. Headers, role-only or empty deltas, usage, pings, reasoning summaries, and signatures do not start the clock. A bridge that suppresses reasoning waits for visible content. |
| `TPS est.` | Streaming estimate: `(N - 1) / (terminal - first matching content)`, with the span in seconds. `N` is output in the known measured token domain; subtract reported reasoning when only non-reasoning output is represented. Unknown output/reasoning basis, no terminal, `N <= 1`, or a nonpositive span means `N/A`. It does not count SSE chunks as tokens. |
| `BASIS` | Streaming numerator domain: `gross` (reported output), `non-reasoning` (output minus explicitly reported reasoning), or `unknown` (streaming TPS unavailable). This does not change gross E2E TPS. |
| `TPS E2E` | Gross reported output tokens divided by request-start-to-terminal seconds. Includes queueing, network, prompt processing, and reasoning; **not server decode speed**. |

Nonstreaming requests can have E2E TPS when output and a terminal time are known,
but TTFT and streaming TPS are `N/A`. Observed failures, cancellations, timeouts,
truncated streams, and requests without usage remain rows in the default report.
Their outcomes are retained; missing counters or a missing terminal are not
fabricated as zero or successful timing.

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
with the independent owner-only runtime `bridge.observer-key` and a fresh challenge,
then returns an AEAD-sealed surrogate, not the vendor key. It is bound to the
frozen route and current host instance; the host restores the original upstream
key/header only when dispatching to the fixed endpoint. Registrations retain
key digests only, and observation artifacts never write the plaintext vendor
key. Old surrogates receive HTTP 401 after a host restart; rerun the helper to
create a new one. The handshake/control challenges do not publish the secret.
`forward-observer-v2` checks require actual capability, not a matching version
string. New launches use their own generation host; they do not require stopping
an old host and interrupting its sessions. Runtime identity and helper binaries
are pinned as described in [background sessions](./background-sessions.md).

This is not TLS or full local data-plane confidentiality. Requests still cross
loopback HTTP, and local processes remain trusted: the Claude surrogate avoids
raw vendor-key disclosure, but a hijacked local port can intercept plaintext
request bodies. Do not assume other agents' ephemeral routes also seal their
credentials.

## Local history and token/cost statistics

```sh
alc usage weekly --offline --timezone Asia/Taipei
alc usage monthly --offline --chart
alc usage yearly --offline --json
alc usage --offline --daily --since 2026-10-01 --until 2026-10-08
alc usage --offline --monthly --source claude,codex --json
alc usage --offline --source alc --filter-profile work --filter-model example-model
alc usage --offline --claude-dir "$HOME/.claude-work" --codex-dir "$HOME/.codex-work"
```

`--offline` reads local configuration, histories, and pricing only. It does not
read API keys, login files, or the Keychain, refresh credentials, query quota,
fetch prices, or otherwise access the network. Text output says `Accounts: not
fetched (--offline)`; JSON has an empty `accounts` array and still includes the
compatibility `ledger`.

### Calendar windows and daily totals

Positional `weekly`, `monthly`, and `yearly` mean the **current calendar week,
month, or year**, not the last 7/30/365 days. Weeks start on Monday. Each window
runs from its first midnight inclusive to the next window's first midnight
exclusive and retains daily detail. It cannot be combined with `--since`,
`--until`, `--daily`, or `--monthly`.

Without a positional window or date bounds, alc still reads all history. The
existing `--daily` and `--monthly` flags remain mutually exclusive grouping
options for all selected history, with any explicit date/source/model filters;
`--monthly` does **not** mean this month.

The terminal view draws boxed tables sized to the terminal: long model lists
wrap, then numbers switch to `1.23M` form, then optional columns drop. Piped
output keeps full integers. Where part of a sum is unknown (a checkpoint without
token counts, an unreadable counter, an unpriced model) the cell shows what is
known and marks it `+`, meaning at least this much, instead of turning the whole
day into `N/A`. `~` marks days where alc's ledger and a native history both saw an
agent and may count a request twice; `--source` picks one. `--details` appends
the strict per-source table, which keeps provider and granularity distinctions,
exact sums or `N/A`, fee components, assumptions and coverage gaps.

`--timezone UTC|local|<IANA>` defaults to UTC. Date-only bounds, calendar windows,
and day/month buckets use that same zone. For example,
`--since 2026-10-01 --until 2026-10-08 --timezone Asia/Taipei` includes October 1–7
in Taipei. RFC3339 bounds are exact instants defined by their offsets; choosing
a timezone does not reinterpret them.

### Wrapped image

```sh
alc usage --wrapped                 # all history, ~/alc-wrapped.png
alc usage yearly --wrapped="$HOME/2026.png"
alc usage --source claude,codex --since 2026-01-01 --wrapped
```

`--wrapped[=PATH]` writes one shareable PNG of the selected history across every
agent and provider: total tokens, first and busiest days, weekday rhythm, a
GitHub-style activity heatmap, agent shares, top models and providers, requests,
sessions, active days, longest streak, cache hit rate, peak hour and estimated
cost. It replaces the text report and shows the image inline in iTerm2, WezTerm,
kitty and Ghostty (not inside tmux). Native histories record a model but not
the route, so their provider is labelled by the model's maker. Figures are the
known sums described above; the footer says when they are lower bounds.

### Offline PNG export

```sh
alc usage weekly --offline --chart
alc usage monthly --offline --timezone local --chart="$HOME/ai-usage-month.png"
alc usage --offline --source alc --json --chart="$HOME/ai-usage.png" > usage.json
```

`--chart[=PATH]` is opt-in; normal reports write no image. With no path it writes
`ai-usage.png` in your actual home directory. An explicit path uses the `=` form;
its parent directory must exist, and write errors fail explicitly. Rust renders
the PNG offline with a bundled licensed font, without Python, fontconfig, or
system-font setup. It does not modify ledgers, native histories, or credentials.
With `--json`, stdout remains JSON-only and the artifact path goes to stderr.

The three panels use the report's selection, timezone, and sources:

- **Date token bars:** uncached input, cache, and output are disjoint categories.
  Gross input already contains cache and must not be stacked with it again.
- **Date USD bars:** token-rate estimates on their own scale, not a token/USD
  dual axis. Partial costs are labeled known subtotals; missing prices are not
  plotted as free spending.
- **Token composition pie:** uncached input, cache (reads plus writes), and
  output. Read/write counters and fees remain separate in tables and JSON.

Unsafe pooled buckets/totals and the pie remain unavailable when sources overlap;
safe daily/source detail is retained. Missing dates/counters remain gaps, not zero.
Empty, all-zero, or unmeasurable composition uses a no-data message rather than
meaningless slices. Longer spans use labeled weekly, monthly, or yearly chart
buckets to stay legible; the CLI/JSON daily detail remains exact. If any selected
sources may overlap, those coarser pooled bars are conservatively unavailable;
individually safe daily values do not prove cross-date sources are disjoint.

### Shared query options

These options apply to the statistics in `alc usage` and to `alc tps`, not to
Accounts or the compatibility ledger:

| Option | Meaning |
| --- | --- |
| `--source all\|alc\|claude\|codex` | Comma-separated sources are accepted, for example `--source alc,claude`. Usage defaults to `all`; TPS defaults to `alc`. |
| `--since DATE` | Inclusive start. `YYYY-MM-DD` means midnight in `--timezone`; RFC3339 preserves the instant its offset specifies. |
| `--until DATE` | Exclusive end, in the same formats. To include October 7 in the selected zone, use `--until 2026-10-08`. |
| `--timezone ZONE` | `UTC` (default), `local`, or an IANA name such as `Asia/Taipei`; shared by date-only bounds, windows, and buckets. |
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
usage. Neither has invented TTFT/TPS. To inspect these rows with `N/A` timing,
use `alc tps --source codex --include-unmeasured`. A cumulative delta spanning
several dates is attributed to the **later checkpoint's date** in the selected
timezone; it cannot reconstruct the original daily traffic.

Across sources, only exact, protocol-qualified request/message/response IDs for
the same agent prove a duplicate; alc records take precedence for a proven
match. Similar timestamps, token totals, or session names are not evidence.
Unverified overlaps are retained, flagged `possible_overlap`, and shown as
source subtotals **without an additive grand total**. Native cumulative parsing
and exact-ID reconciliation run before filters; overlap is then reassessed for
the selected range and independently for each daily rollup. A source warning or
skipped record likewise makes overall coverage incomplete. For legacy v1/v2 alc turn
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
The existing JSON `input_tokens` remains **gross input**; the additive
`uncached_input_tokens` field is the disjoint remainder used in charts.
OpenAI-style input includes cache subsets; Anthropic-style input is the uncached
remainder, so gross input adds reads and writes. Reasoning is a subset of output,
not an extra billable token count. Cache-write TTL buckets replace the aggregate,
not add to it. Unknown TTL splits are not guessed when 5-minute and 1-hour rates
differ. Each record is priced with its own tier, context, and TTL before checked
addition; summed tokens are never multiplied by one arbitrary rate. Arithmetic
uses exact integer pico-dollars (10^-12 USD), not floating-point money.

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
  1). Existing gross `input_tokens` semantics and decimal USD strings remain.
  Additions include `timezone`, resolved `range`, selected `window`,
  `daily_rollups`, `uncached_input_tokens`, and `cost_components` at total,
  daily, and model/source levels. Components are `uncached_input`, `cache_read`,
  `cache_write`, and `output`, each with nullable `known_subtotal_usd` and
  `total_usd`. `known_tokens` (`uncached_input`, `cache_read`, `cache_write`,
  `output`, `incomplete`) sits beside the strict totals at every level: the
  known sums the terminal view shows, a lower bound when `incomplete` is true.
  Each daily rollup also lists its `agents` and `models`. Statistics retain
  source/profile/provider/agent/model rows,
  `granularity`, nullable token totals, `records`, nullable `requests`,
  `known_requests`, `priced_records`, `unpriced_records`,
  `deduplicated_records`, `known_subtotal_usd`, nullable `total_usd`,
  `possible_overlap`, `pricing_snapshot`, and source diagnostics. Rows include
  `cost_status` (`complete`, `partial`, or `unknown`), `partial_records`,
  `reference_providers`, `reference_models`, `price_sources`, `provenance`,
  assumptions, and reasons. USD amounts are decimal **strings**, not
  floating-point JSON numbers. Unsafe pooled totals/subtotals and their
  components are `null` when sources may overlap; safe source rows remain.
- `alc tps --json`: `schema_version: 1`, `measurement`, `timezone`, `range`,
  `rows`, `summary`, `coverage`, and `sources`. Coverage includes
  `matching_records`, `measured_requests`, `excluded_legacy_records`,
  `excluded_unmeasured_records`, `excluded_nonrequest_records`, `eligible_records`,
  `returned_records`, `limited_records`, and `include_unmeasured`. Exclusion
  categories are disjoint and counted before sorting/limiting. Rows contain
  metadata records, `provenance`, raw `timing` offsets in microseconds, token
  counters, outcome, and `metrics` (`ttft_ms`, `stream_tps`, `e2e_tps`,
  `stream_output_basis`). The summary includes `records`, `known_requests`,
  nullable `requests`, valid sample counts, `ttft_mean_ms`, `ttft_p50_ms`,
  `ttft_p95_ms`, `weighted_stream_tps`, and `weighted_e2e_tps`; unavailable
  metrics are `null`. With `--include-unmeasured`, `requests` is `null` when a
  selected row is a cumulative delta or checkpoint, not a verified API request.

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
