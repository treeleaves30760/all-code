---
id: providers
title: Provider compatibility
sidebar_position: 5
description: Which LLM providers work with each of the eight coding agents, the fourteen built-in provider-kind presets and their default URLs/models/key envs, and why an Anthropic Messages or OpenAI Responses endpoint is required.
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - ollama claude code
  - provider presets
---

# Provider compatibility

The eight coding agents do not all speak the same model protocol. `alc`
validates the combination before launch instead of silently sending an
incompatible request.

| Agent | Accepts |
| --- | --- |
| Claude Code | An Anthropic-compatible endpoint |
| Codex CLI | The OpenAI Responses API |
| OpenCode | Any API-compatible provider |
| Pi | An Anthropic-, OpenAI-, or OpenAI-compatible endpoint |
| Copilot CLI | An OpenAI- or Anthropic-compatible endpoint |
| Goose | An OpenAI- or Anthropic-compatible endpoint |
| Qwen Code | An OpenAI-, Anthropic-, or Gemini-compatible endpoint |
| Kimi Code CLI | An OpenAI- or Anthropic-compatible endpoint |

Every agent, regardless of what it accepts natively, also works through the
[Codex bridge](./codex-to-claude.md) with a single `codex login` — the `codex`
provider kind supports all eight.

## Why the differences exist

- Claude Code gateways must expose Anthropic Messages, Bedrock, or Vertex API
  formats. `ANTHROPIC_BASE_URL` selects the gateway.
- Codex CLI's own custom providers use the OpenAI Responses wire API.
- OpenRouter, Ollama, and four of the newer presets (DeepSeek, Moonshot,
  Z.ai, MiniMax) expose an Anthropic-compatible endpoint that Claude Code can
  use directly, alongside their OpenAI-shaped one.
- OpenCode, Pi, Copilot CLI, Goose, Qwen Code, and Kimi Code CLI all accept a
  Chat-Completions-only service; only Claude Code and Codex CLI need more than
  that.

## Provider-kind presets

`alc config` ships fourteen provider kinds. Choosing a `--kind` fills in a
default endpoint, key environment variable, and starting model; every value
is a plain field in `config.toml` that `alc config upsert` can override.

| Kind | Default endpoint | Key env | Starting model |
| --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `sonnet` |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-5.6-terra` |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4.6` |
| `codex` | — (native `codex login`) | — | — (see [Codex bridge](./codex-to-claude.md)) |
| `ollama` | `http://localhost:11434` | — | `qwen3-coder` |
| `vllm` | `http://localhost:8000/v1` | — | — (deployment-specific; ships disabled) |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-v4-pro` |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3` |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | `MiniMax-M3` |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-build-0.1` |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-3.7-flash` |
| `custom` | — (you provide it) | — (you name it with `--api-key-env`) | — (you provide it) |

`deepseek`, `moonshot`, `zai`, and `minimax` each ship a *second*,
Anthropic-compatible base URL alongside their primary OpenAI-chat one — that
is what makes them Claude-ready without any extra configuration:

| Kind | Anthropic-compatible URL |
| --- | --- |
| `deepseek` | `https://api.deepseek.com/anthropic` |
| `moonshot` | `https://api.moonshot.ai/anthropic` |
| `zai` | `https://api.z.ai/api/anthropic` |
| `minimax` | `https://api.minimax.io/anthropic` |

These presets are starting values, not permanent ones: upstream model IDs
drift faster than alc releases, so treat every `Starting model` above as a
default to edit in `alc config`, not a guarantee of what a provider currently
serves.

## Protocols alc understands

Each provider profile declares a protocol, which decides what alc will allow:

| Protocol | Meaning |
| --- | --- |
| `anthropic-messages` | Anthropic Messages API |
| `openai-responses` | OpenAI Responses API |
| `openai-chat` | Chat Completions only |
| `codex-native` | Codex CLI login, used through the bundled bridge |
| `dual` | Serves both Anthropic Messages and OpenAI Responses |

Run `alc doctor` to print the resolved compatibility matrix — every provider
profile against all eight agents — for your own configuration.
