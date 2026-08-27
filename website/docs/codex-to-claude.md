---
id: codex-to-claude
title: Run Claude Code on GPT models
sidebar_label: Claude Code on GPT
sidebar_position: 4
description: Use alc --codex claude to run Claude Code on GPT-5.6 models through your Codex or ChatGPT login, and switch model and reasoning effort from inside the session.
keywords:
  - claude code with gpt
  - codex subscription claude code
  - gpt-5.6
  - claude code model picker
---

# Run Claude Code on GPT models

This path lets Claude Code use the GPT models available through your Codex /
ChatGPT login:

```sh
codex login
alc --codex claude
```

Claude Code starts immediately on your saved default and offers every model in
its own `/model` picker:

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

## Switching model and effort in-session

Inside the session, `/model` switches the GPT model and its left/right arrows
adjust the effort slider; `/effort` sets a level directly. Every model accepts
`low`, `medium`, `high`, `xhigh`, or `max`. Higher effort gives the model more
room to reason, but can take longer and use more quota.

alc passes the model list through Claude Code's
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
setting, added in Claude Code 2.1.243. The picker shows only these GPT models
and the Default row, because Claude's own lineup cannot be served through the
Codex adapter. Older clients ignore the setting and still get the launch
default as a selectable entry. Because Claude Code sends the model and effort
with every request, alc never pins either one on the adapter.

## Choosing the launch defaults

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

## Model catalog

The model catalog is synchronized from the installed Codex CLI at most once
every 24 hours. A bundled catalog keeps the model list working offline:

```sh
alc models
alc models --refresh
alc models --json
```

The synchronized Codex context window is also passed to Claude Code through its
documented
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
that Claude Code child process at it, and stops it when Claude exits. The
helper reads and may refresh `~/.codex/auth.json`; credentials are never copied
into the alc configuration.

:::caution Third-party compatibility layer

This adapter is not an official OpenAI or Anthropic integration. Review the
project's `THIRD_PARTY.md` and your provider terms before using subscription
credentials through it.

:::
