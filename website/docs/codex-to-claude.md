---
id: codex-to-claude
title: Codex bridge
sidebar_label: Codex bridge
sidebar_position: 3
description: One codex login reaches all eight coding agents through the bundled bridge — Claude Code with an in-session GPT model picker, and every other agent through one bridged model per session.
keywords:
  - claude code with gpt
  - codex subscription
  - chatgpt plan coding agent
  - gpt-6-astra
  - reasoning effort
---

# Codex bridge

`alc --codex <agent>` starts a loopback adapter and points one agent's session
at it. One `codex login` serves all eight.

```sh
codex login
alc --codex claude
alc --codex opencode      # or pi, copilot, goose, qwen, kimi
```

| Agent | What the bridge serves it | How it picks a model |
| --- | --- | --- |
| Claude Code | Anthropic Messages | `/model` picker, mid-session |
| OpenCode, Pi, Kimi Code CLI | OpenAI Responses | one model, chosen at launch |
| Copilot CLI, Goose, Qwen Code | OpenAI Chat Completions | one model, chosen at launch |

Claude Code is the only one that can switch mid-session, because it sends the
model and effort with every request, so alc pins neither on the adapter. The
others are wired through the same mechanism each already uses for the `openai`
kind, pointed at the adapter with a placeholder key — see
[Agents](./agents.md) for what each one receives.

## Models

Claude Code lists these in its own `/model` picker:

| Model | Use case | Codex default effort |
| --- | --- | --- |
| `gpt-6-astra` | GPT-6. Most capable; complex, demanding work | `medium` |
| `gpt-5.6-sol` | Frontier capability for the hardest professional work | `low` |
| `gpt-5.6-terra` | Balanced everyday coding; recommended starting point | `medium` |
| `gpt-5.6-luna` | Fast, affordable, high-volume work | `medium` |

The bridge keeps no allowlist: whatever slug it is handed goes upstream and
chatgpt.com decides, so a model alc does not track is still reachable with
`--model`. That is why a new model works on the day Codex ships it rather than
on the day alc catches up.

## Effort

`/model`'s left and right arrows move the effort slider; `/effort` sets one
directly. Every model takes `low`, `medium`, `high`, `xhigh` or `max`. Higher
effort gives the model more room to reason, and uses more of your quota.

`gpt-6-astra` and the GPT-5.6 models also offer `ultra`, which native `alc
codex` can reach but the bridge cannot. alc clamps it to `max` and says so at
launch, rather than letting the request be refused mid-session.

## Starting somewhere else

```sh
alc --codex claude --model gpt-5.6-luna --effort low
alc --codex claude --model gpt-5.6-terra --effort medium --save
```

`--save` stores both on the provider profile. Without them a session starts on
the profile's values, then the selected Codex profile, then the model's own
default. Anything after `--` goes to the agent untouched and wins.

A model chosen with `/model` applies to that session only; the next launch
starts from the profile again.

## Your plain `claude` still reaches Anthropic

Claude Code writes the model you settle on to `~/.claude/settings.json` as your
default for new sessions, and every session on the machine reads that file —
including the ones alc did not start, which have no adapter in front of them
and would ask Anthropic for a GPT model.

alc reads that one key before the launch and puts it back when the session
exits. Nothing else in the file is touched, and nothing is written at all
unless the value it finds is one only the adapter can serve.

Two cases it leaves alone. A real Claude model you switched to mid-session is
your choice about your own default, so it stands. A session killed outright
runs no cleanup — `alc doctor` names the file and the line, and the next
bridged launch clears it.

Do not try to isolate this with `CLAUDE_CONFIG_DIR`: that moves Claude Code's
whole configuration home, login included.

## Model catalog

Synced from your ChatGPT account — the same party the adapter posts every turn
to — so a model that account can drive is offered even when the installed Codex
CLI has never heard of it. `codex debug models` is the fallback when that fetch
cannot happen, and the catalog bundled into the binary is a floor neither of
them can drop below: a synced list may add models, never remove one. The sync
runs once a day, and again as soon as Codex is upgraded.

```sh
alc models
alc models --refresh
alc models --json
```

The synced context window reaches Claude Code as
[`CLAUDE_CODE_MAX_CONTEXT_TOKENS`](https://code.claude.com/docs/en/env-vars),
so a GPT model compacts at the real Codex limit rather than the 200k Claude
Code assumes for an ID it does not know.

## How it works

The bridge is alc's own code, running inside the `alc` process on a random
loopback port, serving only the agent it launched and stopping when that
session ends. It reads and may refresh `~/.codex/auth.json`; no credential is
ever copied into alc's own configuration.

:::caution[Third-party compatibility layer]

This adapter is not an official OpenAI or Anthropic integration. Review the
project's `THIRD_PARTY.md` and your provider terms before using subscription
credentials through it.

:::
