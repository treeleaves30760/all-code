---
id: providers
title: Provider compatibility
sidebar_position: 5
description: Which LLM providers work with Claude Code, Codex CLI, and OpenCode, and why an Anthropic Messages or OpenAI Responses endpoint is required.
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - ollama claude code
---

# Provider compatibility

The coding agents do not all speak the same model protocol. `alc` validates the
combination before launch instead of silently sending an incompatible request.

| Provider profile | Claude Code | Codex CLI | OpenCode |
| --- | --- | --- | --- |
| Anthropic | Yes | No | Yes |
| OpenAI API | Gateway required | Yes (Responses API) | Yes |
| OpenRouter | Yes (Anthropic skin) | Yes (Responses API) | Yes |
| Codex login | Yes (bundled adapter) | Yes (native) | No direct credential reuse |
| Ollama | Yes (Anthropic compatibility) | Yes (`--oss`) | Yes |
| vLLM | If it exposes Anthropic Messages | If it exposes Responses | Yes |
| Custom | According to configured protocol | According to configured protocol | Yes |

## Why the differences exist

- Claude Code gateways must expose Anthropic Messages, Bedrock, or Vertex API
  formats. `ANTHROPIC_BASE_URL` selects the gateway.
- Codex custom providers use the OpenAI Responses wire API.
- OpenRouter and Ollama expose Anthropic-compatible endpoints that Claude Code
  can use directly.

If an OpenAI-compatible service only implements Chat Completions, use it with
OpenCode. Claude Code needs an Anthropic-compatible gateway, and current Codex
requires Responses rather than Chat Completions.

## Protocols alc understands

Each provider profile declares a protocol, which decides what alc will allow:

| Protocol | Meaning |
| --- | --- |
| `anthropic-messages` | Anthropic Messages API |
| `openai-responses` | OpenAI Responses API |
| `openai-chat` | Chat Completions only; OpenCode-compatible |
| `codex-native` | Codex CLI login, used through the bundled adapter |
| `dual` | Serves both Anthropic Messages and OpenAI Responses |

Run `alc doctor` to print the resolved matrix for your own configuration.
