---
id: agents
title: Agents
sidebar_label: Agents
sidebar_position: 6
description: What alc sets for each of the eight coding agents — environment variables, flags, or a merged config file — so your chosen provider works without editing anything by hand.
keywords:
  - claude code
  - codex cli
  - opencode
  - pi coding agent
  - copilot cli
  - goose
  - qwen code
  - kimi code cli
---

# Agents

Each agent has its own idea of how a provider is configured, so alc translates
one provider list into whatever the agent you are launching expects. This page
is what it sets, agent by agent.

Every agent also runs on one `codex login` through the [Codex
bridge](./codex-to-claude.md); `alc --codex <agent>` is the same command
throughout, so it is not repeated below.

## Claude Code

`claude` — [install](https://code.claude.com/docs/en/setup) · accepts an
Anthropic-compatible endpoint.

Claude Code gets a settings file (`--settings`) holding the endpoint, the model
variables and the picker, and fetches its credential through `apiKeyHelper` from
`alc claude-credential`; see [Background sessions](./background-sessions.md).
An Ollama profile gets more — see [Local models](./local-models.md).

```sh
alc claude
alc --openrouter claude
```

## Codex CLI

`codex` — [install](https://learn.chatgpt.com/docs/codex/cli) · accepts the
OpenAI Responses API.

alc sets `--model` and, when configured, `--config
model_reasoning_effort=<level>`. A non-Codex provider also gets a full
`model_providers.<id>.*` override (`base_url`, `wire_api=responses`,
`requires_openai_auth=false`) and, when it needs a key, `env_key` plus
`ALC_PROVIDER_API_KEY`. An Ollama profile gets `--oss --local-provider ollama`
instead.

Codex CLI is the one agent that never goes through the bridge: a `codex`-kind
profile runs it directly on your native login.

```sh
alc codex
alc --openrouter codex
```

## OpenCode

`opencode` — [install](https://opencode.ai/docs) · accepts any
API-compatible provider.

alc sets an inline `OPENCODE_CONFIG_CONTENT` JSON variable, writing no file,
naming the model as `<provider-id>/<model>`. The provider id is the kind name
for Anthropic, OpenAI, OpenRouter and Ollama profiles, and `alc-<profile>` for
every other kind.

A full `provider.<id>` object goes into the same JSON: always for Ollama,
vLLM, custom and the newer presets; for the first four only when the base URL
has been pointed away from that kind's default. `options.apiKey` appears only
when the profile needs a key, so a default Ollama profile gets none.

```sh
alc opencode
alc --zai opencode
```

## Pi

`pi` — [install](https://github.com/earendil-works/pi) (`npm install -g
@earendil-works/pi-coding-agent`) · accepts Anthropic, OpenAI or
OpenAI-compatible.

alc merges an `alc-<profile>` entry into
`$PI_CODING_AGENT_DIR/models.json` (default `~/.pi/agent/models.json`) and
passes `--provider`, `--model` and, with an effort configured, `--thinking`.

The merge is additive: alc only ever writes keys named `alc-*`, the write is
atomic, and a `models.json` that fails to parse makes alc refuse rather than
replace it. An `anthropic` profile with no stored key skips the merge entirely
and launches with `--provider anthropic`, so Pi uses its own subscription
login.

```sh
alc pi
alc --minimax pi
```

## Copilot CLI

`copilot` — [install](https://docs.github.com/en/copilot/how-tos/copilot-cli)
· accepts OpenAI- or Anthropic-compatible.

alc sets `COPILOT_PROVIDER_TYPE`, `COPILOT_PROVIDER_BASE_URL`,
`COPILOT_PROVIDER_API_KEY` (skipped for keyless providers) and
`COPILOT_MODEL`. Pure BYOK: no file is written and no GitHub Copilot login is
needed.

```sh
alc copilot
alc --deepseek copilot
```

## Goose

`goose` — [install](https://block.github.io/goose/) · accepts OpenAI- or
Anthropic-compatible.

alc sets `GOOSE_PROVIDER`, `GOOSE_MODEL`, an optional `GOOSE_FAST_MODEL`, and
the BYOK variables that provider needs: `OPENROUTER_API_KEY`, `OLLAMA_HOST`,
`ANTHROPIC_API_KEY` (plus `ANTHROPIC_HOST` when it differs from goose's
default), or the `OPENAI_*` set.

With no arguments of your own, alc appends goose's interactive `session`
subcommand; pass your own and they are forwarded exactly as given.

```sh
alc goose
alc --groq goose
```

## Qwen Code

`qwen` — [install](https://github.com/QwenLM/qwen-code) · accepts OpenAI-,
Anthropic- or Gemini-compatible.

alc sets `--auth-type <anthropic|openai|gemini>` and `--model`, plus the
matching environment: `ANTHROPIC_*`, `GEMINI_API_KEY` for the `google` kind,
or `OPENAI_*`.

```sh
alc qwen
alc --xai qwen
```

## Kimi Code CLI

`kimi` — [install](https://github.com/MoonshotAI/kimi-cli) · accepts OpenAI-
or Anthropic-compatible.

alc reads your existing config (`~/.kimi/config.toml`, or `ALC_KIMI_CONFIG`),
merges in `providers.alc-<profile>`, `models.alc-<profile>` and
`default_model`, and writes the merged result to a new temp file (mode 0600)
passed as `--config-file`. Your real config is never written to, and the temp
file is deleted when Kimi exits, so the key touches disk only for the life of
that process. Passing your own `--config-file` disables all of this.

```sh
alc kimi
alc --moonshot kimi
```

## Binary overrides

Point any agent at a specific binary instead of resolving it from `PATH`:

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
