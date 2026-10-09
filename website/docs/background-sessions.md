---
id: background-sessions
title: Background sessions
sidebar_label: Background sessions
sidebar_position: 4
description: Claude Code's agent view, claude --bg and ← work in every session alc starts, on the provider alc gave it - and under alc --codex claude every Claude model becomes a Codex model.
keywords:
  - claude code agent view
  - claude --bg
  - background agents
  - codex claude code
  - apiKeyHelper
---

# Background sessions

Claude Code runs sessions in the background with [agent view](https://code.claude.com/docs/en/agent-view):
`claude agents` to dispatch and watch them, `claude --bg` to start one from the
shell, and `←` on an empty prompt to send the one you are in there. A supervisor
of Claude Code's own runs them, so they keep working after the terminal closes.

Every Claude Code session alc starts works there, on the provider alc gave it:

```sh
alc --codex claude agents
alc --codex claude --bg "fix the flaky test"
alc --openrouter claude agents
```

What a dispatched session answers on is the session's model, not a model fixed
at launch: `←` carries the conversation you are in, so one you send to the
background after `/model opus` keeps the model you just picked. Under
`alc --codex claude` every one of them is a Codex model either way.

## How alc hands a session its provider

Claude Code keeps one thing for a background session: the flags it was launched
with, which it reads again every time it restarts the session. So alc passes the
provider as a settings file, passed with `--settings`, instead of the environment
variables the supervisor drops. New generation documents live under
`<config>/run/g/<shortid>/claude/settings-<hash>.json`; legacy documents remain
under `<config>/claude/`.

| In the file | What it does |
| --- | --- |
| `ANTHROPIC_BASE_URL` | the provider's Anthropic endpoint, alc's Codex background bridge, or an opt-in API observation route |
| model variables and `modelPicker` | the models Claude Code starts on and offers |
| `apiKeyHelper` | A hash-pinned alc executable running `--runtime <id> claude-credential …`, which Claude Code runs for the credential |

The generated provider settings contain no key. For ordinary direct API-key
sessions, the helper prints the key from the profile's environment variable or
`alc config key` store; for Codex translation it prints the bridge token. With
Claude API-key `--metrics`, it instead returns a sealed local observer credential
as described below, never the raw vendor key over loopback HTTP. A key available
only in the original shell may be missing on a later background restart; save it
with `alc config key <profile>`. The helper never falls back to your Claude login.

A background session keeps the settings alc merged at launch. If you pass your
own `--settings`, alc merges it into the document it writes, yours winning,
because Claude Code reads only one - so later edits to your file reach the
sessions you launch afterwards, not the ones already running. New alc
compatibility defaults apply only to sessions started after the upgrade.
The helper executable and explicit runtime identity stay with the session's
generation even when the stable entry point activates a newer version. Older
unscoped `claude-credential` calls still select legacy, not the latest host.

## The background bridge

For Codex, the adapter now runs as a process of its own, because a background
session outlives the `alc` that started it:

- one per runtime generation in an alc configuration, on `127.0.0.1` only,
  on a port it picks and keeps;
- every model request must carry its token;
- a session that needs it starts it through its generation-pinned helper;
- it stops after an hour with nothing to do, or an explicit owner stop.

```sh
alc bridge                         # running or not, pid, port, runtime owner
alc --runtime legacy bridge stop   # explicitly stop the legacy owner
```

`alc doctor` shows the same under **Background sessions**. New generations use
`<config>/run/g/<shortid>`; legacy keeps `<config>/run`, its HTTP origin, token,
and existing routes. Real configuration, credentials, and `usage.jsonl` stay
shared. Updating does not restart an old host or move its sessions. New launches
use their own generation and require `request-metrics-v3` for measured requests;
a reused v1/v2 bridge cannot retroactively supply missing timing.

Use global `--runtime <id|legacy>` for host management. With several running
owners, `bridge stop` requires a target. It can interrupt in-flight requests;
it is not a zero-downtime operation or an update prerequisite.

## API-key metrics in background sessions

```sh
alc --openrouter --metrics claude agents
alc --openrouter --metrics claude --bg "fix the flaky test"
alc tps --filter-agent claude
```

Ordinary API-key sessions stay direct. With `--metrics`, supported Claude Code
API-key launches use a **forwarding observation route** on the durable background
host, not a short-lived listener owned by the launching terminal. The route
forwards the provider's native Anthropic-compatible protocol; it is not the
Codex translation adapter. New requests record metadata for [TTFT/TPS and
usage estimates](./usage.md), including after a supervisor restart.

The durable settings file contains the loopback endpoint and alc's helper, not
the key. On each restart the helper resolves the same profile key, starts the
host if needed, and authenticates it with a fresh challenge using the independent,
owner-only runtime `bridge.observer-key`. It registers only a key digest in memory and
returns an **AEAD-sealed surrogate** bound to the frozen route and current host
instance. The host restores the original upstream key/header only when dispatching
to that fixed endpoint. Observation artifacts never write the plaintext vendor
key; target registration retains digests, not the key. After a host restart, an
old surrogate gets HTTP 401; rerunning the helper creates one for the new instance.
The authenticated handshake/control challenges do not publish the observer secret.

Routes are content-addressed: Codex translation identity includes its full
effective model-tier mapping; forwarding identity freezes profile/kind/upstream
metadata, not keys. A later launch cannot overwrite an existing session's route.
Forwarding key allowlists are unioned for the same frozen identity so another
launch does not remove a key still used by an existing session. A key available
only in the original shell still needs `alc config key <profile>` for later restarts.
Changing or disabling that profile's endpoint requires a fresh launch; the
helper refuses to send a new key to the old route.

This preserves the native SDK, protocol, model, and **upstream authentication**,
not the plaintext credential on the local hop. The local request data plane is
still loopback HTTP, not TLS or full local confidentiality. Trust local processes:
the surrogate prevents raw vendor-key disclosure, but a hijacked local port can
still intercept plaintext request bodies.

Claude's native login, keyless endpoints, and overridden endpoint/authentication
or `apiKeyHelper` settings are not observed; explicit `--metrics` is refused
when alc cannot safely use its own API-key seam. It also refuses a profile whose
selected `api_key_env` is `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` and exported
nonempty, because that bypasses the sealed helper: save with
`alc config key <profile>`, unset that exported variable, and relaunch; ordinary
no-metrics auth behavior is unchanged. Metrics requires the authenticated
`forward-observer-v2` capability, not merely a matching alc version string.
A new launch uses its own capable generation host while an older host keeps
serving old sessions; no blanket `bridge stop` is required.
A metrics dry-run starts no host/listener and writes no settings, route, or
observer-key file. Codex translation requests are observed automatically and need
no `--metrics`; native/history-only records cannot supply past TTFT.

## Every Claude model becomes a Codex model

Under `alc --codex claude` no request reaches a Claude model:

| Where Claude Code picks a model | Under alc --codex claude |
| --- | --- |
| the model a session starts on, `/model`, the Default row | Codex models only |
| `opus`, `fable`, `best` | the first model in alc's catalog (currently GPT-6.1 Sol, the default workhorse) |
| `sonnet`, `opusplan` outside plan mode | the model the session started on |
| `haiku`, and Claude Code's background work (titles, summaries, agent view's rows) | the cheapest Codex model |
| a Claude model named in full: `/model claude-opus-5`, a subagent's `model:`, a fallback chain | the Codex model of the same tier |
| `[1m]` variants | sized like the plain model, at Codex's real window |
| fast mode, the advisor | off: they exist only on Claude models |

`claude ultrareview` and cloud sessions run on Anthropic's servers; they stay
Anthropic features.

## Managing sessions

`alc claude attach <id>`, `logs`, `stop`, `respawn` and `rm` go straight to
Claude Code. Plain `claude attach <id>` works just as well: the session already
carries its settings file, and waking it starts the bridge if it needs one.

So do the Claude Code commands that never reach a model - `mcp`, `doctor`,
`plugin`, `update`, `auth` and the like. They start no bridge, write no settings
file and count no session in `alc usage`.

alc's own flags go before the agent's name and Claude Code's go after it. Where
both want the same spelling - `-p`, `--name` - put Claude Code's after `--`:

```sh
alc --codex claude -- -p "fix the flaky test"
alc --codex claude -- --bg --name nightly "run the slow suite"
```

`--` goes straight after the agent's name, before all of its flags. Later in
the line it is passed to Claude Code as an argument of its own.

## Limits

- A model you pick with `/model` in a background session you attached to with
  plain `claude attach` becomes Claude Code's default for new sessions, with no
  alc process around to put yours back. `alc doctor` reports it, and the next
  `alc --codex claude` clears it.
- Retained generations, helper executables, and settings must remain on disk
  while sessions use them. Self-update neither garbage-collects them nor
  restarts Claude Code or its supervisor.
- New alc-managed runtimes coordinate Codex refreshes using a cross-process
  lock on the canonical auth path, then reread. Old hosts and external Codex
  do not cooperate; generation namespaces do not guarantee zero impact from
  shared-auth rotation or external agent/package updates.
- If another program occupies a generation bridge's remembered port while it
  is down, alc fails without rotating its token or rewriting settings/origins.
  Resolve that conflict to keep the frozen session contract. Initial allocation
  can choose an ephemeral port. Legacy can still move ports; a legacy session
  then picks up the new port on `claude respawn <id>`.
