---
id: intro
slug: /
title: all-code (alc)
sidebar_label: Introduction
sidebar_position: 1
description: One CLI to configure LLM providers once and launch Claude Code, Codex CLI, or OpenCode with any of them, including Claude Code on a Codex/ChatGPT login.
keywords:
  - claude code
  - codex cli
  - opencode
  - llm provider
  - coding agent
---

# all-code (`alc`)

**One CLI for Claude Code, Codex CLI, and OpenCode.** Configure your LLM
providers once, then launch any of the three coding agents with any provider —
including running Claude Code on your Codex/ChatGPT subscription.

```sh
alc config
alc claude
alc codex
alc opencode
alc --codex claude
alc --openrouter codex
alc --provider work opencode
```

## What alc does

- **Switch LLM provider per agent.** Point Claude Code, Codex CLI, or OpenCode
  at Anthropic, the OpenAI API, OpenRouter, Ollama, vLLM, or any custom
  endpoint, and change it for a single run without editing config files.
- **Run Claude Code on GPT models.** [`alc --codex claude`](./codex-to-claude.md)
  bridges your Codex / ChatGPT login to Claude Code, and lists every GPT model
  in Claude Code's own `/model` picker so you switch model and reasoning effort
  mid-session.
- **Validate before launching.** alc checks that the agent and provider speak a
  [compatible model protocol](./providers.md) instead of sending a request that
  cannot work.
- **Keep credentials out of the way.** API keys live in a separate file or come
  from environment variables; nothing is copied between agents.

## Why it exists

Each coding agent has its own idea of how a provider is configured: Claude Code
reads Anthropic-style environment variables, Codex CLI takes TOML overrides on
the command line, and OpenCode expects an inline JSON config. Keeping the same
set of providers usable across all three means repeating that work three times,
in three formats, every time a key or an endpoint changes.

`alc` holds one provider list and translates it into whatever the agent you are
launching expects.

## Requirements

`alc` launches coding agents that are already installed; install the ones you
plan to use separately:

- [Claude Code](https://code.claude.com/docs/en/setup)
- [Codex CLI](https://learn.chatgpt.com/docs/codex/cli)
- [OpenCode](https://opencode.ai/docs)

## Next steps

- [Install alc](./installation.md)
- [Quick start](./quick-start.md)
- [Run Claude Code on GPT models](./codex-to-claude.md)
