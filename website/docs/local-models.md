---
id: local-models
title: Local models
sidebar_label: Local models
sidebar_position: 4
description: Run Claude Code on a local Ollama model — what alc sets for it, and how to keep the first turn short on a laptop.
keywords:
  - ollama claude code
  - local llm coding agent
  - gemma
  - qwen3-coder
---

# Local models

`alc --ollama claude` points Claude Code at the Ollama server's Anthropic
Messages endpoint and sets the session up for a server that answers one
request at a time.

```sh
alc --ollama claude
alc doctor          # the Ollama section: server, model pulled, tool calling, context
```

## What alc sets

- Every model alias — `ANTHROPIC_DEFAULT_MODEL`, the sonnet/opus/haiku tiers,
  `ANTHROPIC_SMALL_FAST_MODEL` — pinned to the profile's model (haiku to
  `small_model` when set), so Claude Code never asks Ollama for a Claude model
  ID it does not have.
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`, so side requests do not hold
  the server's single slot before the real one starts.
- `CLAUDE_CODE_MAX_CONTEXT_TOKENS` from the window Ollama reports (`/api/ps`
  while loaded, `/api/show` otherwise), so compaction follows the real window.
  Skipped when the server is down.
- `API_FORCE_IDLE_TIMEOUT=0` and `API_TIMEOUT_MS=1800000` unless you set them,
  so Claude Code waits up to thirty minutes for the first token instead of
  abandoning the request after six.

## Keeping the first turn short

Claude Code opens a session with a 25k–40k-token request, and a laptop-sized
model reads that at a few dozen tokens per second — minutes before the first
token. What helps:

- A model whose `ollama show <model>` lists `tools`.
- Fewer MCP servers, plugins, and skills; each adds tool schemas to the first
  request.
- A 64k–128k context (`OLLAMA_CONTEXT_LENGTH`): Claude Code needs at least
  64k, and a 256k window on a 24 GB Mac reserves KV cache for nothing.
  `OLLAMA_KV_CACHE_TYPE=q8_0` halves what is left.
- `OLLAMA_KEEP_ALIVE=4h` (or `-1`), so the prompt cache survives idle minutes.
- No `ollama pull` or second model during a session.
