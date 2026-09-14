---
id: providers
title: Provider compatibility
sidebar_position: 5
description: Which LLM providers work with each of the eight coding agents, the fourteen built-in provider-kind presets and their default URLs, key envs and models.
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - provider presets
  - coding agent compatibility
---

# Provider compatibility

The eight agents do not all speak the same model protocol, so alc checks the
pair before launch instead of sending a request that cannot work.

| Agent | Accepts | Protocol |
| --- | --- | --- |
| Claude Code | An Anthropic-compatible endpoint | `anthropic-messages` |
| Codex CLI | The OpenAI Responses API | `openai-responses` |
| OpenCode | Any API-compatible provider | any |
| Pi | Anthropic, OpenAI, or OpenAI-compatible | any |
| Copilot CLI | OpenAI- or Anthropic-compatible | `openai-chat`, `anthropic-messages` |
| Goose | OpenAI- or Anthropic-compatible | `openai-chat`, `anthropic-messages` |
| Qwen Code | OpenAI-, Anthropic-, or Gemini-compatible | `openai-chat`, `anthropic-messages` |
| Kimi Code CLI | OpenAI- or Anthropic-compatible | `openai-chat`, `anthropic-messages` |

Whatever an agent accepts natively, it also runs on one `codex login` through
the [Codex bridge](./codex-to-claude.md).

`alc doctor` prints this matrix resolved against your own profiles.

## Presets

Choosing a `--kind` fills in an endpoint, a key variable and a starting model.
Every value is a plain field in `config.toml` that `alc config upsert`
overrides.

| Kind | Default endpoint | Key env | Starting model |
| --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `sonnet` |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-5.6-terra` |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4.6` |
| `codex` | — (native `codex login`) | — | — |
| `ollama` | `http://localhost:11434` | — | `qwen3-coder` |
| `vllm` | `http://localhost:8000/v1` | — | — (ships disabled) |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-v4-pro` |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3` |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | `MiniMax-M3` |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-build-0.1` |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-3.7-flash` |
| `custom` | — (you provide it) | — (name it with `--api-key-env`) | — |

Model IDs drift faster than alc releases, so treat every starting model as a
value to edit rather than a promise about what the provider serves today.

## Claude-ready without extra configuration

DeepSeek, Moonshot, Z.ai and MiniMax each publish a second,
Anthropic-compatible base URL beside their OpenAI-shaped one, so Claude Code
runs on them directly:

| Kind | Anthropic-compatible URL |
| --- | --- |
| `deepseek` | `https://api.deepseek.com/anthropic` |
| `moonshot` | `https://api.moonshot.ai/anthropic` |
| `zai` | `https://api.z.ai/api/anthropic` |
| `minimax` | `https://api.minimax.io/anthropic` |

## Claude Code on a local Ollama model

Moved to [Local models](./local-models.md), which covers what `alc --ollama
claude` sets and how to keep the first turn short on a laptop.
