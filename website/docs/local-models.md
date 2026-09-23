---
id: local-models
title: Local models
sidebar_label: Local models
sidebar_position: 4
description: Run Claude Code on a local Ollama, llama.cpp or vLLM model — what alc sets for it, and how to keep the first turn short.
keywords:
  - ollama claude code
  - llama.cpp claude code
  - vllm claude code
  - local llm coding agent
  - gemma
  - qwen3-coder
---

# Local models

`alc --ollama claude`, `alc --llamacpp claude` and `alc --vllm claude` point
Claude Code at the local server's Anthropic Messages endpoint — its root,
beside the OpenAI routes under `/v1` — and set the session up for a server
that serves one model and answers one request at a time, or a few.

```sh
alc --ollama claude
alc --llamacpp claude
alc doctor          # server, model, context, and for llama.cpp and vLLM /v1/messages
```

## What alc sets

- Every model alias — `ANTHROPIC_DEFAULT_MODEL`, the sonnet/opus/haiku tiers,
  `ANTHROPIC_SMALL_FAST_MODEL` — pinned to the profile's model (haiku to
  `small_model` when set), so Claude Code never asks the server for a Claude
  model ID it does not have.
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`, so side requests do not hold
  a slot before the real one starts.
- `CLAUDE_CODE_MAX_CONTEXT_TOKENS` from the window the server reports, so
  compaction follows the real window: Ollama's `/api/ps` while loaded and
  `/api/show` otherwise, llama.cpp's per-slot `n_ctx` from `/props`, vLLM's
  `max_model_len`. Skipped when the server is down.
- `API_FORCE_IDLE_TIMEOUT=0`, `API_TIMEOUT_MS=1800000` and
  `CLAUDE_STREAM_IDLE_TIMEOUT_MS=1800000` unless you set them, so Claude Code
  waits up to thirty minutes for the first token instead of abandoning the
  request after five or six.
- For llama.cpp and vLLM, `CLAUDE_CODE_MODEL_CAPABILITIES=-mid_conv_system,-mid_conv_tool_change`;
  see below.

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

## llama.cpp and vLLM

```sh
llama-server -m Qwen3.8-27B-UD-Q4_K_XL.gguf --alias qwen3.8-27b --jinja -c 131072 --api-key "$LLAMA_API_KEY"

alc config upsert box --kind llamacpp --base-url http://127.0.0.1:8080/v1 --model qwen3.8-27b
printf '%s' "$LLAMA_API_KEY" | alc config key box --stdin
alc -p box claude
```

The profile keeps the OpenAI-style URL ending in `/v1`, which every other agent
uses; Claude Code is given the root. `--model` is the name the server lists at
`/v1/models` — llama-server's `--alias`, vLLM's `--served-model-name`. A key
saved for the profile, or `LLAMA_API_KEY` for a llama.cpp profile, reaches every
agent, and Claude Code asks for it through `apiKeyHelper` rather than reading
it from a file. On the same profile OpenCode uses llama-server's Chat
Completions route, and Codex its Responses route.

A `vllm` profile works the same way. So does a `vllm` profile pointed at a
llama-server before alc had a kind for it: Claude Code runs on it whichever
OpenAI protocol it names for the other agents.

### Why two more settings

**The chat template.** Both servers render the model's own Jinja chat
template, and many templates — Qwen's among them — refuse a system message
anywhere but first. Claude Code sends exactly that to a model it does not
recognise: its environment block as a `role: "system"` message after the first
user turn. llama.cpp answers `500 System message must be at the beginning`,
which Claude Code does not treat as a capability it can drop, so it retries
the same request until it gives up. alc sets
`CLAUDE_CODE_MODEL_CAPABILITIES=-mid_conv_system,-mid_conv_tool_change`, and
the block rides in the first user turn as a `<system-reminder>` instead.

**The silent prompt read.** llama-server sends its response headers at once,
then nothing until it has read the whole prompt: half a minute for 11k tokens
on a 27B model, a quarter of an hour for 200k. Claude Code's stream watchdogs
end that silence after five minutes on any `ANTHROPIC_BASE_URL`, however
`API_FORCE_IDLE_TIMEOUT` is set, and the retry reads the prompt again.
`CLAUDE_STREAM_IDLE_TIMEOUT_MS` goes to thirty minutes, the most the watchdog
accepts.

### Checking the server

`alc doctor` prints a **llama.cpp and vLLM** section for every enabled profile
of either kind:

```text
llama.cpp and vLLM
  Claude Code needs the server's /v1/messages; the context shown is what one request gets
  ✓  box          http://127.0.0.1:8080  llama.cpp b11100-7ab4ee7ba
  ✓  qwen3.8-27b  served  131072 tokens of context  Anthropic Messages
```

It says when nothing answers, when the server wants a key or refuses the saved
one, when the model is not one the server lists, when one request gets less
than 64k of context, and when there is no `/v1/messages` — which Claude Code
needs and the other agents do not. The route is asked with an empty body, which
the server refuses before any model reads a token, so the check costs a shared
server nothing.
