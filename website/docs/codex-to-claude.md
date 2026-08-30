---
id: codex-to-claude
title: Codex bridge
sidebar_label: Codex bridge
sidebar_position: 4
description: One codex login reaches all eight coding agents through the bundled bridge — Claude Code with an in-session GPT model picker, and every other agent through one bridged model per session.
keywords:
  - claude code with gpt
  - codex subscription
  - gpt-5.6
  - claude code model picker
  - codex bridge
---

# Codex bridge

One `codex login` serves every agent alc launches. `alc --codex <agent>`
starts the bundled `claude-codex` adapter on a loopback port and points that
one agent's session at it — no separate login for OpenCode, Pi, Copilot CLI,
Goose, Qwen Code, or Kimi Code CLI.

```sh
codex login
alc --codex claude
alc --codex opencode
alc --codex pi
alc --codex copilot
alc --codex goose
alc --codex qwen
alc --codex kimi
```

The adapter speaks a different wire protocol depending on the agent, all
backed by the same Codex login:

| Agent | Wire protocol the bridge serves |
| --- | --- |
| Claude Code | Anthropic Messages |
| OpenCode, Pi, Kimi Code CLI | OpenAI Responses |
| Copilot CLI, Goose, Qwen Code | OpenAI Chat Completions |

Claude Code is the only agent with in-session switching: because it sends the
model and reasoning effort with every request, alc never pins either one on
the adapter, and `/model`/`/effort` change the running session (see below).
Every other agent picks one model — and, for that session, one pinned
reasoning effort — at launch, using its own mechanism instead of a picker.

## Claude Code

Claude Code starts immediately on your saved default and offers every model
in its own `/model` picker:

| Model | Beginner-friendly use case | Codex default effort |
| --- | --- | --- |
| `gpt-5.6-sol` | Frontier capability for the hardest professional work | `low` |
| `gpt-5.6-terra` | Balanced everyday coding; recommended starting point | `medium` |
| `gpt-5.6-luna` | Fast, affordable, high-volume work | `medium` |

The list is ordered by capability, most capable first, matching Codex's own
tiers for this family. See OpenAI's
[model selection guide](https://developers.openai.com/api/docs/guides/latest-model),
[Luna reference](https://developers.openai.com/api/docs/models/gpt-5.6-luna),
and [Sol reference](https://developers.openai.com/api/docs/models/gpt-5.6-sol)
for current upstream details.

### Switching model and effort in-session

Inside the session, `/model` switches the GPT model and its left/right arrows
adjust the effort slider; `/effort` sets a level directly. Every model accepts
`low`, `medium`, `high`, `xhigh`, or `max`. Higher effort gives the model more
room to reason, but can take longer and use more quota.

alc passes the model list through Claude Code's
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
setting, added in Claude Code 2.1.243. The picker shows only these GPT models
and the Default row, because Claude's own lineup cannot be served through the
Codex adapter. Older clients ignore the setting and still get the launch
default as a selectable entry.

### Choosing the launch defaults

To choose a different starting point for one run, or in scripts and CI:

```sh
alc --codex claude --model gpt-5.6-luna --effort low
alc --codex claude --model gpt-5.6-terra --effort medium --save
```

`--save` stores the model and effort in the selected alc provider. Without
these options the session starts on the alc provider's values, then the
selected Codex profile, then the model's documented default. An explicit
`--model`, `--effort`, or `--settings` placed after `--` is forwarded to Claude
Code untouched and wins over what alc would inject.

A model chosen with `/model` applies to that Claude Code session. The next
`alc --codex claude` starts from the alc provider default again, so
[`alc config`](./configuration.md) stays the source of truth.

## OpenCode, Pi, and Kimi Code CLI

These three speak the adapter's OpenAI Responses surface directly. Each picks
one model, and one reasoning effort, at launch (the alc provider's configured
values, or the model catalog's default) and wires it in with its own
mechanism instead of an in-session picker: OpenCode gets an `alc-codex`
provider in `OPENCODE_CONFIG_CONTENT`, Pi gets an `alc-codex` entry merged
into `models.json`, and Kimi Code CLI gets an `alc-codex` provider in a
temporary `--config-file`. See [Supported agents](./agents.md) for the
non-bridge details of each.

```sh
alc --codex opencode
alc --codex pi
alc --codex kimi
```

## Copilot CLI, Goose, and Qwen Code

These three speak the adapter's OpenAI Chat Completions surface, wired
through the same mechanism each already uses for the `openai` provider kind —
`COPILOT_PROVIDER_*` environment variables for Copilot CLI, `OPENAI_*` for
Goose, and `OPENAI_*` plus `--auth-type openai` for Qwen Code — pointed at the
loopback adapter with a placeholder key instead of a real one.

```sh
alc --codex copilot
alc --codex goose
alc --codex qwen
```

## Model catalog

The model catalog is synchronized from the installed Codex CLI at most once
every 24 hours. A bundled catalog keeps the model list working offline:

```sh
alc models
alc models --refresh
alc models --json
```

The synchronized Codex context window is also passed to Claude Code through
its documented
[`CLAUDE_CODE_MAX_CONTEXT_TOKENS`](https://code.claude.com/docs/en/env-vars)
gateway setting, so unknown GPT IDs compact at the correct Codex limit instead
of Claude's generic fallback.

Claude Code's built-in aliases stay on Codex as well: the picker's Default row
follows the alc default, `haiku` and background work use the cheapest catalog
model, `sonnet` follows the session's starting model, and `opus` uses the most
capable one.

## How the bridge works

The release archive bundles
[`claude-codex` 0.3.1](https://github.com/fcakyon/claude-code-with-codex), an
MIT-licensed helper. `alc` starts it on a random `127.0.0.1` port, points only
the launched agent's process at it, and stops it when that process exits. The
helper reads and may refresh `~/.codex/auth.json`; credentials are never
copied into the alc configuration.

:::caution Third-party compatibility layer

This adapter is not an official OpenAI or Anthropic integration. Review the
project's `THIRD_PARTY.md` and your provider terms before using subscription
credentials through it.

:::
