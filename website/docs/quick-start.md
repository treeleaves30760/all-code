---
id: quick-start
title: Quick start
sidebar_position: 3
description: Configure providers in the alc TUI, launch Claude Code, Codex CLI, or OpenCode, override the provider for one run, and preview the resolved command.
keywords:
  - switch llm provider
  - launch claude code
  - openrouter codex
---

# Quick start

## 1. Configure providers

Open the full-screen configuration UI:

```sh
alc config
```

The starter configuration includes Anthropic, OpenAI, OpenRouter, Codex,
Ollama, and a disabled vLLM template. Add API keys in the TUI or point a
profile at an environment variable. Environment variables take precedence over
locally saved keys.

## 2. Launch an agent

Each agent launches with its configured default provider:

```sh
alc claude
alc codex
alc opencode
```

## 3. Override the provider for one run

```sh
alc --codex claude
alc --openrouter codex
alc -p local-vllm opencode
```

`--provider` (or `-p`) takes a profile name, or a provider kind when only one
profile of that kind exists. The shortcut flags `--anthropic`, `--openai`,
`--openrouter`, `--codex`, `--ollama`, and `--vllm` are equivalent.

## Forwarding arguments

Apart from Claude's alc-specific `--model`, `--effort`, and `--save` options,
arguments after the agent name are forwarded unchanged:

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc --ollama opencode run "fix the failing test"
```

To pass an option with one of those same names to Claude Code itself, place it
after `--`:

```sh
alc claude -- --model sonnet
```

## Preview without launching

Print the exact command and environment that would be used. Secrets are
redacted:

```sh
alc --openrouter --dry-run claude
```

## Check the setup

```sh
alc doctor
```

`alc doctor` reports agent binaries, credential status, the Codex login, the
bundled adapter, the resolved Codex-to-Claude defaults, and the compatibility
matrix.
