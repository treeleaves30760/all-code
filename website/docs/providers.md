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

## Claude Code on a local Ollama model

`alc --ollama claude` points Claude Code at the Ollama server's Anthropic
Messages endpoint. A local server serves only the models it has pulled and
answers one request at a time, so alc sets the session up differently from a
hosted provider:

- `ANTHROPIC_DEFAULT_MODEL`, `ANTHROPIC_DEFAULT_SONNET_MODEL`,
  `ANTHROPIC_DEFAULT_OPUS_MODEL`, `ANTHROPIC_DEFAULT_HAIKU_MODEL`, and
  `ANTHROPIC_SMALL_FAST_MODEL` all point at the profile's model (the haiku
  tier at its `small_model` when one is set), so Claude Code's own aliases,
  background calls, and `/model` rows never ask Ollama for a Claude model ID
  it does not have (`404 model not found`).
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` skips the session-title and
  similar side requests, which would otherwise hold the server's single slot
  for minutes before the real request even starts.
- `CLAUDE_CODE_MAX_CONTEXT_TOKENS` carries the context window Ollama reports
  for the model (`/api/ps` while it is loaded, `/api/show` otherwise), so
  auto-compaction follows the real window instead of the 200k Claude Code
  assumes for an unknown model ID. Skipped silently when the server is down.
- `API_FORCE_IDLE_TIMEOUT=0` and `API_TIMEOUT_MS=1800000`, unless you set
  them yourself, let Claude Code wait up to thirty minutes for the first
  token. Against any host other than Anthropic's it would otherwise abandon
  the request after six minutes and start over.

Claude Code opens every session with a request of roughly 25k to 40k tokens
(system prompt, tool schemas, project context), and a laptop-sized model reads
that at a few dozen tokens per second: on an M3 MacBook Air, `gemma4:12b` needs
about six minutes for a 22k-token first request and fifteen for a 39k one
before a single token comes back. The two timeout variables above keep Claude
Code waiting through that (an older alc, or a bare `claude`, abandons each
attempt after six minutes; retries resume from Ollama's prompt cache, so the
session still starts eventually). The first turn is only pleasant when the
prompt is small and the model is quick to read it. On a laptop that means:

- Use a model whose `ollama show <model>` lists the `tools` capability;
  coding agents are useless without tool calling.
- Keep the first request small: every MCP server, plugin, and skill adds
  tool schemas to it, and reading time grows with its length — faster than
  linearly for models such as Gemma 4, whose full-attention layers slow
  down the deeper they get into the prompt.
- Give the model a 64k to 128k context (Ollama's settings, or
  `OLLAMA_CONTEXT_LENGTH`): Claude Code needs at least 64k, while a 256k
  window on a 24 GB Mac reserves gigabytes of KV cache for nothing. Flash
  attention is already on by default; `OLLAMA_KV_CACHE_TYPE=q8_0` halves
  what is left.
- Keep the model loaded (`OLLAMA_KEEP_ALIVE=4h`, or `-1`): when Ollama
  unloads it after five idle minutes the prompt cache goes too, and the next
  turn reads the whole conversation again.
- Do not pull or run other models during a session.

`alc doctor` prints an **Ollama** section with the server version, whether
the model is pulled, whether it can call tools, and the context it gets.

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
