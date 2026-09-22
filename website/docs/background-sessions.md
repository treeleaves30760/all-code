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
provider as a settings file, `--settings ~/.config/alc/claude/settings-<hash>.json`,
instead of the environment variables it used before, which the supervisor
drops.

| In the file | What it does |
| --- | --- |
| `ANTHROPIC_BASE_URL` | the provider's Anthropic endpoint, or alc's background bridge for Codex |
| model variables and `modelPicker` | the models Claude Code starts on and offers |
| `apiKeyHelper` | `alc claude-credential …`, which Claude Code runs for the credential |

The file never contains a key. The helper prints the key alc already keeps - the
profile's environment variable, or the key saved with `alc config key` - or, for
Codex, the background bridge's token. A key that lives only in one shell's
environment works in the sessions started from that shell; a background session
started later may not see it, and then the helper says to save it with
`alc config key <profile>`. It never falls back to your Claude login.

A background session keeps the settings alc merged at launch. If you pass your
own `--settings`, alc merges it into the document it writes, yours winning,
because Claude Code reads only one - so later edits to your file reach the
sessions you launch afterwards, not the ones already running.

## The background bridge

For Codex, the adapter now runs as a process of its own, because a background
session outlives the `alc` that started it:

- one per alc configuration, on `127.0.0.1` only, on a port it picks once and keeps;
- every model request must carry its token;
- a session that needs it starts it, through the helper;
- it stops after an hour with nothing to do, or with `alc bridge stop`.

```sh
alc bridge          # running or not, pid, port, which alc started it
alc bridge stop     # the next session that needs it starts it again
```

`alc doctor` shows the same under **Background sessions**.

## Every Claude model becomes a Codex model

Under `alc --codex claude` no request reaches a Claude model:

| Where Claude Code picks a model | Under alc --codex claude |
| --- | --- |
| the model a session starts on, `/model`, the Default row | Codex models only |
| `opus`, `fable`, `best` | the most capable Codex model |
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
- If the bridge ever has to move to another port - another program took its
  port while it was down - sessions still running on the old one reconnect when
  they restart: `claude respawn <id>`, or `claude respawn --all`.
