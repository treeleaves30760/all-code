---
id: providers
title: Provider compatibility
sidebar_position: 5
description: Which LLM providers work with each of the nine coding agents, the sixteen built-in provider-kind presets and their default URLs, key envs and models.
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - provider presets
  - coding agent compatibility
---

# Provider compatibility

The nine agents do not all speak the same model protocol, so alc checks the
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
| Antigravity CLI | Its own Google login | `antigravity-native` |

Whatever an agent accepts natively, it also runs on one `codex login` through
the [Codex bridge](./codex-to-claude.md) — every agent but the Antigravity CLI,
which runs only on its own Google login ([Antigravity
CLI](./agents.md#antigravity-cli-agy)).

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
| `antigravity` | — (native `agy` login) | — | `claude-opus-5-5-high` |
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

`antigravity` launches the Antigravity CLI and nothing else, so it is not
Claude-ready — not yet: routing other agents through that login is coming as an
explicit opt-in. A `config.toml` written before it existed lacks the profile;
`alc config upsert antigravity --kind antigravity` adds it.

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

Four cases differ:

- The `anthropic` kind — your own Claude login or API key — keeps Anthropic's
  server-side check and its alias table.
- A profile whose model is itself a Claude Code alias, such as `sonnet` or
  `opus`, is not pinned: Claude Code would send that alias verbatim as the
  model's name. The server attempt is still skipped.
- A profile whose model is a Claude id in full, such as `claude-opus-5`, is not
  pinned either: its endpoint is a gateway in front of Anthropic (LiteLLM, a
  corporate proxy) that already serves `claude-sonnet-*` by name, so `/model
  sonnet` and every subagent declared `model: sonnet` keep Claude Sonnet. The
  server attempt is still skipped. A provider's own spelling, such as
  OpenRouter's `anthropic/claude-sonnet-5`, is not a Claude id in full and is
  pinned.
- A `--settings` of your own that names `ANTHROPIC_DEFAULT_SONNET_MODEL` still
  wins over alc's. One set in `~/.claude/settings.json` does not: alc's
  settings take precedence over it.

The [Codex bridge](./codex-to-claude.md#auto-mode-and-prompt-caching) and local
servers already worked this way.
