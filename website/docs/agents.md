---
id: agents
title: Supported agents
sidebar_label: Agents
sidebar_position: 6
description: What alc injects for each of the eight coding agents it launches — exact environment variables, flags, and config files, agent by agent.
keywords:
  - claude code
  - codex cli
  - opencode
  - pi coding agent
  - github copilot cli
  - goose
  - qwen code
  - kimi code cli
---

# Supported agents

alc launches eight coding agents. Each one has its own idea of how a provider
is configured — environment variables, CLI flags, or a config file — so this
page lists, agent by agent, exactly what alc sets to make your chosen provider
work. Every agent also runs through the [Codex bridge](./codex-to-claude.md)
with a single `codex login`; see that page for how the bridge itself works.

## Claude Code

- Binary: `claude` — [install](https://code.claude.com/docs/en/setup)
- Accepts: an Anthropic-compatible endpoint
- alc injects: `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`, `ANTHROPIC_API_KEY`
  (or `ANTHROPIC_AUTH_TOKEN` for bearer-style providers such as OpenRouter),
  and `ANTHROPIC_SMALL_FAST_MODEL` when the profile sets a small model
- Through the bridge, Claude Code is the one agent with in-session switching:
  every GPT model appears in its own `/model` picker, and `/model`/`/effort`
  change the running session — see [Codex bridge](./codex-to-claude.md) for
  the full walkthrough.

```sh
alc claude
alc --openrouter claude
alc --codex claude --model gpt-5.6-terra --effort medium
```

## Codex CLI

- Binary: `codex` — [install](https://learn.chatgpt.com/docs/codex/cli)
- Accepts: the OpenAI Responses API
- alc injects: `--model` and, when configured, `--config
  model_reasoning_effort=<level>`. A non-Codex provider also gets a full
  `model_providers.<id>.*` override (`base_url`, `wire_api=responses`,
  `requires_openai_auth=false`), plus — only for a profile that needs a key
  at all — `env_key` and an `ALC_PROVIDER_API_KEY` env carrying it; an Ollama
  profile instead gets `--oss --local-provider ollama --model <model>`.
- Codex CLI is the one agent that never goes through the bridge: a
  `codex`-kind profile runs `codex` directly on your native `codex login`.

```sh
alc codex
alc --openrouter codex
```

## OpenCode

- Binary: `opencode` — [install](https://opencode.ai/docs)
- Accepts: any API-compatible provider
- alc injects an inline `OPENCODE_CONFIG_CONTENT` JSON environment variable
  (no file is written) naming the model as `<provider-id>/<model>`. The
  provider id is the literal kind name for Anthropic, OpenAI, OpenRouter, and
  Ollama profiles (`anthropic`, `openai`, `openrouter`, `ollama`); every other
  kind — vLLM, Custom, and the seven new provider-kind presets — gets
  `alc-<profile>` instead.
- A full `provider.<id>` object (npm package, `name`, `options.baseURL`,
  `models`) is also written into that same JSON: always for Ollama, vLLM,
  Custom, and the seven new presets; for an Anthropic/OpenAI/OpenRouter
  profile only when its base URL has been pointed away from that kind's own
  default. `options.apiKey: "{env:ALC_PROVIDER_API_KEY}"` is added only when
  the profile needs a key at all — a default (keyless) Ollama profile gets no
  `apiKey` field.
- Through the bridge, the same mechanism defines an `alc-codex` provider
  pointed at the loopback adapter.

```sh
alc opencode
alc --zai opencode
alc --codex opencode
```

## Pi

- Binary: `pi` — [install](https://github.com/earendil-works/pi)
  (`npm install -g @earendil-works/pi-coding-agent`)
- Accepts: an Anthropic-, OpenAI-, or OpenAI-compatible endpoint
- alc injects: an `alc-<profile>` entry merged into
  `$PI_CODING_AGENT_DIR/models.json` (default `~/.pi/agent/models.json`), plus
  `--provider`, `--model`, and, when an effort is configured, `--thinking`
  flags.
- **The merge is additive.** alc only ever writes keys named `alc-*`, so any
  provider you added yourself is untouched. The write is atomic, and alc
  refuses outright — rather than overwriting it with a fresh file — if your
  existing `models.json` fails to parse.
- **Anthropic-subscription edge case:** an `anthropic`-kind profile with no
  stored API key skips the `models.json` write entirely and launches with
  `--provider anthropic --model <model>` instead, so Pi falls back to its own
  `/login` (subscription) credentials rather than a `models.json` entry
  nothing would use.

```sh
alc pi
alc --minimax pi
alc --codex pi
```

## Copilot CLI

- Binary: `copilot` — [install](https://docs.github.com/en/copilot/how-tos/copilot-cli)
- Accepts: an OpenAI- or Anthropic-compatible endpoint
- alc injects: `COPILOT_PROVIDER_TYPE` (`anthropic` or `openai`),
  `COPILOT_PROVIDER_BASE_URL`, `COPILOT_PROVIDER_API_KEY` (skipped for
  key-less providers such as Ollama), and `COPILOT_MODEL`.
- This is pure BYOK — no config file is written, and no GitHub Copilot login
  is required.

```sh
alc copilot
alc --deepseek copilot
alc --codex copilot
```

## Goose

- Binary: `goose` — [install](https://block.github.io/goose/)
- Accepts: an OpenAI- or Anthropic-compatible endpoint
- alc injects `GOOSE_PROVIDER` plus the matching BYOK variables:
  `OPENROUTER_API_KEY` for OpenRouter, `OLLAMA_HOST` for Ollama,
  `ANTHROPIC_API_KEY` (and `ANTHROPIC_HOST`, only when it differs from
  goose's own default `https://api.anthropic.com`) for Anthropic-shaped
  providers, or `OPENAI_API_KEY`/`OPENAI_HOST`/`OPENAI_BASE_PATH` for
  everything else — plus `GOOSE_MODEL` and, when configured,
  `GOOSE_FAST_MODEL`.
- **Defaults to `session`:** with no arguments of your own, alc appends
  goose's interactive `session` subcommand; pass your own arguments
  (`alc goose run ...`) and they are forwarded exactly as given instead.

```sh
alc goose
alc --groq goose
alc --codex goose
```

## Qwen Code

- Binary: `qwen` — [install](https://github.com/QwenLM/qwen-code)
- Accepts: an OpenAI-, Anthropic-, or Gemini-compatible endpoint
- alc injects `--auth-type <anthropic|openai|gemini>` and `--model`, plus the
  matching environment: `ANTHROPIC_BASE_URL`/`ANTHROPIC_API_KEY` for
  Anthropic-shaped providers, `GEMINI_API_KEY` for the `google` kind, or
  `OPENAI_BASE_URL`/`OPENAI_API_KEY` for everything else.

```sh
alc qwen
alc --xai qwen
alc --codex qwen
```

## Kimi Code CLI

- Binary: `kimi` — [install](https://github.com/MoonshotAI/kimi-cli)
- Accepts: an OpenAI- or Anthropic-compatible endpoint
- alc injects nothing as an environment variable or flag except
  `--config-file`. It reads your existing config (`~/.kimi/config.toml`, or
  the path named by `ALC_KIMI_CONFIG` when set) if one exists, merges in
  `providers.alc-<profile>` (type `anthropic`, `openai_responses`, or
  `openai_legacy`, depending on the provider kind), `models.alc-<profile>`,
  and `default_model = "alc-<profile>"`, and writes the **merged** result to a
  brand-new temp file (mode `0600` on Unix).
- **Your real config file is never written to.** The temp file is passed as
  `--config-file <path>` and deleted once Kimi exits, so the real API key
  touches disk only for the life of that one process.
- Passing your own `--config-file` (or `--config`) disables all of this — alc
  forwards your arguments unchanged instead.

```sh
alc kimi
alc --moonshot kimi
alc --codex kimi
```

## Binary overrides

Every agent's binary can be pointed at a specific path instead of resolving
it from `PATH`:

| Agent | Override env |
| --- | --- |
| Claude Code | `ALC_CLAUDE_BIN` |
| Codex CLI | `ALC_CODEX_BIN` |
| OpenCode | `ALC_OPENCODE_BIN` |
| Pi | `ALC_PI_BIN` |
| Copilot CLI | `ALC_COPILOT_BIN` |
| Goose | `ALC_GOOSE_BIN` |
| Qwen Code | `ALC_QWEN_BIN` |
| Kimi Code CLI | `ALC_KIMI_BIN` |

See [Provider compatibility](./providers.md) for what each provider kind
speaks, and [Codex bridge](./codex-to-claude.md) for how `alc --codex <agent>`
works underneath.
