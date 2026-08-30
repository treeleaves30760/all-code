---
id: quick-start
title: Quick start
sidebar_position: 3
description: Configure providers in the alc TUI, launch any of the eight coding agents, override the provider for one run, and preview the resolved command.
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
alc pi
alc copilot
alc goose
alc qwen
alc kimi
```

See [Supported agents](./agents.md) for what alc sets for each one.

## 3. Override the provider for one run

```sh
alc --codex claude
alc --openrouter codex
alc --deepseek pi
alc --codex opencode
alc -p local-vllm opencode
```

`--provider` (or `-p`) takes a profile name, or a provider kind when only one
profile of that kind exists. The shortcut flags `--anthropic`, `--openai`,
`--openrouter`, `--codex`, `--ollama`, `--vllm`, `--deepseek`, `--moonshot`,
`--zai`, `--minimax`, `--groq`, `--xai`, and `--google` are equivalent.

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

`alc doctor` reports all eight agent binaries, credential status, provider
profiles with a per-agent compatibility column each, the resolved defaults,
and, when a Codex provider is configured, the Codex bridge's login state.
