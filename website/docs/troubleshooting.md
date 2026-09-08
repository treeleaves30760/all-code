---
id: troubleshooting
title: Troubleshooting
sidebar_position: 8
description: Diagnose alc problems with alc doctor, including missing agents, incompatible providers, missing API keys, and Codex login errors.
keywords:
  - alc doctor
  - claude code needs anthropic messages
  - codex login expired
---

# Troubleshooting

Start with:

```sh
alc doctor
```

It reports all eight agent binaries, credential status, provider profiles
with a per-agent compatibility column each, the resolved defaults, and, when
a Codex provider is configured, the Codex bridge's login state.

## `'claude' is not installed or not on PATH`

alc launches agents that already exist on your machine. Install the agent, or
point alc at a specific binary with `ALC_CLAUDE_BIN`, `ALC_CODEX_BIN`,
`ALC_OPENCODE_BIN`, `ALC_PI_BIN`, `ALC_COPILOT_BIN`, `ALC_GOOSE_BIN`,
`ALC_QWEN_BIN`, or `ALC_KIMI_BIN`.

## `provider '…' cannot be used with claude; Claude Code needs Anthropic Messages`

The selected profile speaks a protocol Claude Code cannot use. Choose an
Anthropic-compatible endpoint, OpenRouter, or Ollama, or use
[`alc --codex claude`](./codex-to-claude.md). See
[provider compatibility](./providers.md).

## `provider '…' has no API key`

Save one with `alc config key <profile>`, or set the environment variable named
in the profile's `api_key_env` field.

## `Codex credentials were not found`

Run `codex login`, then retry. `alc doctor` reports the login state under
**Codex bridge**.

## `the bundled claude-codex … helper is missing`

It cannot be, from 1.4.0 on: the bridge is part of `alc` rather than shipped
beside it, and from 1.5.0 it is alc's own code. If you are seeing this from an
older `alc`, upgrade with the one-line installer.

## `API Error: Request timed out` (or `500`) with an Ollama profile

Claude Code abandons a request that has not started answering after six minutes
and retries it; Ollama logs the abandoned request as a `500`. The model simply
did not get through Claude Code's first request — 25k to 40k tokens — in time.
Current alc versions set `API_FORCE_IDLE_TIMEOUT=0` and
`API_TIMEOUT_MS=1800000` for Ollama profiles so Claude Code waits instead
(update alc if you still see the cutoff); without them, retries resume from
Ollama's prompt cache and the session usually starts on the second or third
attempt. To make the first turn quick instead:

- Check the **Ollama** section of `alc doctor`: the model must be pulled and
  able to call tools, and its context must be at least 64k.
- Launch with fewer MCP servers, plugins, and skills; every one of them adds
  tool schemas to the first request, and reading time grows with its length.
- Keep Ollama's context length at 64k–128k rather than the model's maximum on
  a small machine, and keep the model loaded with `OLLAMA_KEEP_ALIVE=4h` so
  the prompt cache survives between turns.
- Let any running `ollama pull` finish first, and close other memory-hungry
  programs.

See [Claude Code on a local Ollama model](./providers.md#claude-code-on-a-local-ollama-model).

## `404 model 'claude-…' not found` from Ollama

Claude Code asked the server for one of its own model IDs — usually through the
`haiku` alias it uses for background work or a `/model` row. alc now pins every
alias to the profile's model for Ollama profiles; update alc, or set the
profile's `small_model` to a model you have pulled.

## The model list looks out of date

The catalog syncs from the installed Codex CLI at most once every 24 hours:

```sh
alc models --refresh
```

## Secrets in output

`alc --dry-run` redacts API keys and auth tokens, and `alc config show` never
prints credential values — only whether each profile has one.

## I cannot find the sharing setting in `alc config`

It is on the third screen. `alc config` names all three across its header —
`1 Providers`, `2 Agent defaults`, `3 Sharing & remote` — and `Tab`,
`Shift+Tab` or the number key moves between them. Share-by-default, the bind
address and the permission ceiling all live on the third one.

Outside the TUI, `alc remote auto-share on` sets the same thing, `alc remote
status` and `alc doctor` report it, and `alc config show` prints it under
`# Remote control`.

If the row reads `on (inactive)`, sharing itself is off: a session shares by
default only when both are on. Turn on the `sharing` row above it, or run
`alc remote on`.

## `the model may not exist or you may not have access to it`

From a `--codex` session, this usually means the model is real but the bundled
bridge could not route it — before 1.5.0 the bridge alc depended on kept a
hard-coded model list that could be a release behind Codex. From 1.5.0 the
bridge keeps no list at all, so this should no longer happen; if it does, the
message names the models that do work.
before the bridge learns to serve it. alc now refuses that launch up front and
lists the models the bridge does route; pick one of those:

```sh
alc config upsert codex --model gpt-5.6-terra
```

A codex profile whose model is empty follows the Codex CLI's own `model`
setting instead, which is where an unroutable one usually comes from. Native
`alc codex` is unaffected.
