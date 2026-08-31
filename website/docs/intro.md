---
id: intro
slug: /
title: all-code (alc)
sidebar_label: Introduction
sidebar_position: 1
description: One CLI to configure LLM providers once and launch eight coding agents with any of them, including running any of them on a Codex/ChatGPT login.
keywords:
  - claude code
  - codex cli
  - opencode
  - pi coding agent
  - copilot cli
  - goose
  - qwen code
  - kimi code cli
  - llm provider
  - coding agent
---

# all-code (`alc`)

**One CLI for eight coding agents.** Configure your LLM providers once, then
launch [Claude Code](https://code.claude.com/docs/en/setup),
[Codex CLI](https://learn.chatgpt.com/docs/codex/cli),
[OpenCode](https://opencode.ai/docs),
[Pi](https://github.com/earendil-works/pi),
[Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli),
[Goose](https://block.github.io/goose/),
[Qwen Code](https://github.com/QwenLM/qwen-code), or
[Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli) with any of them —
including running any of them on your Codex/ChatGPT subscription.

```sh
alc config
alc claude
alc codex
alc opencode
alc pi
alc copilot
alc goose
alc qwen
alc kimi
alc --codex opencode
alc --deepseek claude
alc --provider work opencode
```

## What alc does

- **Switch LLM provider per agent.** Point any of the eight agents at
  Anthropic, the OpenAI API, OpenRouter, Ollama, vLLM, DeepSeek, Moonshot,
  Z.ai, MiniMax, Groq, xAI, Google, or any custom endpoint, and change it for
  a single run without editing config files.
- **Run every agent on GPT models.** [`alc --codex <agent>`](./codex-to-claude.md)
  bridges your Codex / ChatGPT login to whichever agent you launch. Claude Code
  lists every GPT model in its own `/model` picker so you switch model and
  reasoning effort mid-session; every other agent picks one model for the
  session.
- **Validate before launching.** alc checks that the agent and provider speak a
  [compatible model protocol](./providers.md) instead of sending a request that
  cannot work.
- **Keep credentials out of the way.** API keys live in a separate file or come
  from environment variables; nothing is copied between agents.

## Why it exists

Each coding agent has its own idea of how a provider is configured: Claude
Code, Copilot CLI, and Goose read environment variables; Codex CLI and Qwen
Code take flags on the command line; OpenCode expects an inline JSON config;
Pi merges an entry into its own `models.json`; and Kimi Code CLI merges one
into a TOML config file. Keeping the same set of providers usable across all
eight means repeating that work eight times, in eight formats, every time a
key or an endpoint changes.

`alc` holds one provider list and translates it into whatever the agent you are
launching expects — see [Supported agents](./agents.md) for exactly what it
sets for each one.

## Requirements

`alc` launches coding agents that are already installed; install the ones you
plan to use separately:

- [Claude Code](https://code.claude.com/docs/en/setup)
- [Codex CLI](https://learn.chatgpt.com/docs/codex/cli)
- [OpenCode](https://opencode.ai/docs)
- [Pi](https://github.com/earendil-works/pi) (`npm install -g @earendil-works/pi-coding-agent`)
- [Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli)
- [Goose](https://block.github.io/goose/)
- [Qwen Code](https://github.com/QwenLM/qwen-code)
- [Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli)

## Next steps

- [Install alc](./installation.md)
- [Quick start](./quick-start.md)
- [Supported agents](./agents.md)
- [Codex bridge](./codex-to-claude.md)
