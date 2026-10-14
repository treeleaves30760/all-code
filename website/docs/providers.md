---
id: providers
title: Provider compatibility
sidebar_position: 5
description: Which LLM providers work with each of the eight coding agents, the fifteen built-in provider-kind presets and their default URLs, key envs and models.
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
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-6.1-sol` |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4.6` |
| `codex` | — (native `codex login`) | — | — |
| `ollama` | `http://localhost:11434` | — | `qwen3-coder` |
| `vllm` | `http://localhost:8000/v1` | — | — (ships disabled) |
| `llamacpp` | `http://localhost:8080/v1` | `LLAMA_API_KEY` | — (the name the server lists) |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-v4-pro` |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3` |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | `MiniMax-M3` |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-build-0.1` |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-3.7-flash` |
| `custom` | — (you provide it) | — (name it with `--api-key-env`) | — |

New `openai` profiles also start with `low` reasoning effort. Existing profiles
keep their saved model and effort.

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

## Claude Code on a local server

Ollama, llama.cpp and vLLM each answer Anthropic Messages at their root, beside
the OpenAI routes, so Claude Code runs on all three whichever OpenAI protocol a
profile names for the other agents. [Local models](./local-models.md) covers
what alc sets for each and how to keep the first turn short.

## Auto mode on every provider

Claude Code's auto mode checks each tool call with a classifier. Anthropic runs
that check on its own servers, which no other provider can, so on every other
provider alc has Claude Code run its local classifier on the profile's model:
it pins `ANTHROPIC_DEFAULT_SONNET_MODEL` — the `sonnet` alias the classifier
uses — to that model and sets `CLAUDE_CODE_AUTO_MODE_SERVER=0` to skip the
server attempt. Auto mode then works on OpenRouter, DeepSeek, a custom endpoint
or anything else Claude Code can reach, and its classifier calls spend that
provider's quota. `/model sonnet` names the profile's model too.

Three things are left alone:

- The `anthropic` kind — your own Claude login or API key — keeps Anthropic's
  server-side check and its alias table.
- A profile whose model is itself a Claude Code alias, such as `sonnet` or
  `opus`, is not pinned: Claude Code would send that alias verbatim as the
  model's name.
- A `--settings` of your own that names `ANTHROPIC_DEFAULT_SONNET_MODEL` still
  wins over alc's.

The [Codex bridge](./codex-to-claude.md#auto-mode-and-prompt-caching) and local
servers already worked this way.
