---
id: configuration
title: Configuration
sidebar_position: 7
description: Where alc stores provider profiles and API keys, how to edit them in the TUI, and the scripting commands that change configuration without it.
keywords:
  - alc config
  - provider profile
  - api key storage
---

# Configuration

## File locations

| Platform | Config directory |
| --- | --- |
| Windows | `%APPDATA%\alc` |
| macOS/Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/alc` |

Files:

- `config.toml`: provider metadata, models, defaults, URLs, and env-var names.
- `credentials.toml`: locally saved API keys, mode `0600` on Unix.
- `remote.toml`: the [remote-control](./remote-control.md) settings.
- `usage.jsonl`: launch entries and metadata-only request records from the Codex
  translation bridge and opt-in `--metrics` observation. [`alc usage` and
  `alc tps`](./usage.md) read it. Deleting it resets alc's records, not the
  separate native Claude/Codex histories.
- `pricing.toml`: optional exact USD token-rate overrides for usage estimates.
  This is a [separate sidecar](#pricing-sidecar), not a table in `config.toml`.
- `claude/settings-*.json` (legacy) or
  `run/g/<shortid>/claude/settings-*.json` (new generations): the settings
  documents alc hands Claude Code with `--settings` — the endpoint, the model
  variables, the picker, and the pinned `apiKeyHelper` line, with no key of any
  kind. Each is named after a hash of its
  own contents, so every launch that resolves to the same document reuses the
  same file; each is written mode `0600` on Unix. alc never deletes them,
  because a [background session](./background-sessions.md) reads its file again
  every time Claude Code restarts it. Removing them is safe while no background
  session is running; delete one a live session uses and that session breaks
  until its next launch. One thing to know before you leave them there: a
  `--settings` of your own is merged into the document, so a credential you put
  in your file is in alc's copy too.
- `run/` (legacy) and `run/g/<shortid>/` (new generations): host namespaces.
  The runtime artifacts below are relative to the selected namespace, not a
  different config directory. Provider config, credentials, `usage.jsonl`, and
  `remote.toml` sharing policy stay shared; legacy files are not migrated away.
- Runtime `bridge.port`, `bridge.token`: where the [background
  bridge](./background-sessions.md#the-background-bridge) is listening, and its
  control/translation token; the token is not sent to the API vendor. Durable
  Claude API observation uses a sealed surrogate on the local request hop.
- Runtime `bridge.observer-key`: an independent owner-only local observer secret
  (`0600` on Unix), not a vendor API key. It authenticates fresh host/control
  challenges and seals Claude metrics credentials to a frozen route/current host
  instance. It is never published by the handshake or sent upstream. The data
  plane remains loopback HTTP; this secret does not provide TLS or hide plaintext
  request bodies from a hijacked local port.
- Runtime `bridge/routes/`: content-addressed immutable Codex translation routes,
  including the provider profile, Codex auth path, and full effective
  model-tier mapping. A later launch cannot overwrite an old session's route.
- Runtime `bridge/forward/`: durable Claude API-key observation routes used by
  `--metrics`; frozen profile/kind/upstream metadata only, no API key.
  Key-digest allowlists are unioned for the same frozen identity, preserving keys
  still used by other sessions. The
  [helper](./background-sessions.md#api-key-metrics-in-background-sessions)
  resolves the key, authenticates the host, registers only its digest in memory,
  and returns an AEAD-sealed local surrogate. The host restores upstream
  authentication only on dispatch; observation artifacts never store the plaintext
  vendor key. Host restarts invalidate old surrogates; rerun the helper.

Override the real configuration directory with `ALC_CONFIG_DIR`. New helpers
pin their executable and pass explicit `--runtime <id>`; old unscoped
`claude-credential` calls still use legacy. `--runtime <id|legacy>` is also the
global owner selector for host management, not an account/config override.

The install directory is separate: its full-binary `alc` entry point reads
adjacent `.alc/active.json` and dispatches to `.alc/generations/<digest>/alc`
(`alc.exe` on Windows). Generations are retained; do not remove payloads or
settings a background session still needs. [Updating](./getting-started.md#update)
explains local bundles, rollback, and one-time migration.

## The configuration TUI

```sh
alc config
```

The keys are shown at the bottom of every screen. The primary controls are:

- `a`, `e`/Enter, `d`: add, edit, or delete a provider.
- `Tab`/`Shift+Tab`, or `1`/`2`/`3`: move between the three screens named in
  the header — Providers, Agent defaults, and Sharing & remote. The last one
  holds share-by-default, the bind address and the permission ceiling.
- Arrow keys: navigate fields and cycle choices, including reasoning effort.
- On a Codex profile, `←`/`→` on the Model field opens the guided GPT model and
  effort chooser, which writes the launch defaults for `alc --codex claude`.
- `s`: save; `q`: save and quit; `Ctrl+C`: quit without saving.

## Scripting commands

```sh
alc config init
alc config show
alc config path
alc config upsert codex --kind codex --model gpt-6.1-sol --effort low
alc config upsert work --kind openrouter --model anthropic/claude-sonnet-4.6
printf '%s' "$OPENROUTER_API_KEY" | alc config key work --stdin
alc config set-default claude work
alc config remove work
```

`alc config upsert` accepts `--kind`, `--model`, `--effort`, `--clear-effort`,
`--small-model`, `--base-url`, `--anthropic-base-url`, `--protocol`, `--auth`,
`--api-key-env`, `--codex-profile`, `--codex-home`, `--claude-config-dir`,
`--disable`, and `--enable`.

## Several logins of one kind

A second ChatGPT or Claude login is a second profile pointing at its own
credential directory:

```toml
[providers.codex-work]
kind = "codex"
codex_home = "/Users/you/.codex-work"

[providers.anthropic-work]
kind = "anthropic"
claude_config_dir = "/Users/you/.claude-work"
```

Both paths must be absolute, `codex_home` belongs to a `codex` profile and
`claude_config_dir` to an `anthropic` one, and each beats the matching
environment variable so a shell setting cannot move which account a named
profile spends. [Usage](./usage.md) has the whole flow.

## Pricing sidecar

[`alc usage`](./usage.md#what-an-estimated-dollar-means) estimates with a bundled,
offline curated LiteLLM subset and optional local overrides, and fills models
both lack from LiteLLM's public price map, cached for a day as
`litellm-prices.json` in the config directory (the source `npx ccusage` uses;
`--offline` reads only the cache). The bundled snapshot is dated 2026-10-08 and pins LiteLLM
commit `33d908e0ae2c0a257eeb5d546df08527d348a670`, upstream SHA-256, and MIT license
provenance. The report's `pricing_snapshot` identifies the bundled data and any
selected override by hash; historical usage is repriced with that snapshot,
not reconstructed into historical invoices.

Put overrides in **`<alc-config-dir>/pricing.toml`**, or choose another file with
`alc usage --pricing-file /absolute/path/pricing.toml`. Do not add `[pricing]` to
`config.toml`. Example for an explicitly free local model — use this only if you
intend a zero API-token reference, not to account for electricity or hardware:

```toml
version = 1
currency = "USD"
units = "USD-per-million-tokens"

[[models]]
provider = "custom"
model = "local-model"
profile = "local-work"
endpoint = "http://127.0.0.1:8080/v1"
input = "0"
output = "0"
cache_read = "0"
cache_write = "0"
```

| Field | Rule |
| --- | --- |
| `version`, `currency`, `units` | Required exactly as above: `1`, `"USD"`, `"USD-per-million-tokens"`. |
| `[[models]].provider`, `model` | Required exact recorded provider kind/reference provider and model ID; the profile name belongs in `profile`, not `provider`. No fuzzy matching or automatic prefix stripping. |
| `aliases` | Optional list of additional exact model IDs. Each name must belong unambiguously to one model family in the same scope. |
| `profile`, `endpoint` | Optional exact selectors. Endpoint is an absolute HTTP(S) URL without credentials, query, or fragment, matching recorded upstream metadata rather than the loopback observation route. |
| `tier` | Optional service-tier selector; defaults to `"standard"`. Missing recorded tier assumes standard; OpenAI's `"default"` maps to standard. Other actual tiers need their own rates. |
| `context_min_tokens`, `context_max_tokens` | Optional inclusive gross-input bounds. The selected band's rates apply to the whole request, not marginal tokens; cumulative deltas cannot select per-request bands. |
| `input`, `output`, `cache_read`, `cache_write` | Optional nonnegative decimal **strings** in USD per million tokens, with at most six fractional digits. At least one explicit rate is required per entry. |
| `cache_write_5m`, `cache_write_1h` | TTL-specific write rates instead of `cache_write`; do not mix flat and TTL-specific rates in one entry. The counters replace aggregate cache writes, not add another charge. |

Unknown fields, malformed rates, overlapping context bands, duplicate tiers, and
ambiguous profile-only/endpoint-only scopes are rejected. A more specific exact
profile/endpoint scope can replace a broader scope. **The selected override
family replaces bundled prices as a whole**: omitted rates, tiers, and context
bands do not inherit fallback prices. Missing counters or positive usage without
a rate stay unknown/partial; explicit `"0"` is the only free rate. Local/custom
models need exact overrides unless a supported exact official endpoint supplies
a reference. An override never invents unknown native provider/profile metadata.

Cost excludes subscription fees, taxes, discounts, tools, and other non-token
charges. It is an API-token or API-equivalent reference estimate, not a bill.

## Credential precedence

For each provider profile, alc resolves the API key in this order:

1. The environment variable named by `api_key_env`, when it is set and not
   empty.
2. The key saved in `credentials.toml`.

Profiles whose authentication style is `native` or `none` need no key at all —
that covers the Codex login and local runtimes such as Ollama.

## Setting precedence for Codex-to-Claude

1. This run's `--model` / `--effort`
2. The alc provider profile
3. `<codex_home>/<profile>.config.toml`, then `<codex_home>/config.toml` —
   where `codex_home` is the profile's field, else `CODEX_HOME`, else
   `~/.codex`
4. The model catalog's documented default
