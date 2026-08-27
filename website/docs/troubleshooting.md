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

It reports agent binaries, credential status, the Codex login, the bundled
adapter, the resolved Codex-to-Claude defaults, and the compatibility matrix.

## `'claude' is not installed or not on PATH`

alc launches agents that already exist on your machine. Install the agent, or
point alc at a specific binary with `ALC_CLAUDE_BIN`, `ALC_CODEX_BIN`, or
`ALC_OPENCODE_BIN`.

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
**Codex login**.

## `the bundled claude-codex … helper is missing`

Source builds do not include the adapter. Reinstall with the one-line
installer, put a compatible `claude-codex` on PATH, or set
`ALC_CLAUDE_CODEX_BIN`.

## The model list looks out of date

The catalog syncs from the installed Codex CLI at most once every 24 hours:

```sh
alc models --refresh
```

## Secrets in output

`alc --dry-run` redacts API keys and auth tokens, and `alc config show` never
prints credential values — only whether each profile has one.
