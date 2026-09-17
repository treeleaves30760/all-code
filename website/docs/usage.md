---
id: usage
title: Usage
sidebar_label: Usage
sidebar_position: 6
description: See what is left on every Claude and Codex login, what each API-key provider has left, and which agent spent it — in the terminal, on the remote-control page, or as JSON.
keywords:
  - alc usage
  - claude code usage
  - codex quota
  - chatgpt plan limit
  - openrouter credits
  - multiple accounts
---

# Usage

What is left on each login, and which agent spent it.

```sh
alc usage
```

```text
Accounts
     PROFILE     ACCOUNT                  PLAN  REMAINING
  ✓  anthropic   ~/.claude                max   5h 97% left, resets in 2h 53m — week 79% left, resets in 6d 4h — Fable week 62% left, resets in 6d 4h
  ✓  codex       you@example.com          pro   week 66% left, resets in 5d 9h — no credits
  ·  ollama      —                        —     no quota API
  ·  openrouter  —                        —     no API key; run `alc config key openrouter`

Usage by provider and agent
  PROVIDER  AGENT     LAUNCHES  TURNS  INPUT  OUTPUT  LAST
  codex     claude    1         1      20.8K  35      7m ago
  ollama    opencode  1         —      —      —       12m ago
  source: ~/.config/alc/usage.jsonl — tokens are counted only where alc carries the traffic; a direct launch counts as a launch alone

✓ ready
```

One row per enabled provider profile. `REMAINING` counts down: `63% left` is
what you have, not what you have spent. The exit code is 1 when a login has
expired or been refused, or when a vendor could not be reached or answered an
error; 0 otherwise, so a plan that is simply used up does not fail a script.

`alc usage --json` prints the same report as JSON. `alc --provider codex-work
usage` or `alc --codex usage` narrows it to one profile or one kind.

## Codex logins

alc reads the `auth.json` that `codex login` wrote and asks chatgpt.com what is
left on it. Nothing is written back: OpenAI's refresh tokens are single-use, so
a status command that rotated one would invalidate the token a running session
is holding. An expired token says `codex login`, and the next real session
renews it anyway.

## Claude logins

alc reads Claude Code's own login — the macOS Keychain first, then
`.credentials.json` in its config directory — and asks api.anthropic.com for
the five-hour and weekly windows, plus a per-model window where your plan has
one. Read-only, never refreshed. macOS may ask once for permission to read the
Keychain item.

An Anthropic profile that uses an API key instead shows `no quota API`: that
key bills per token and has no remaining balance to report.

## Several logins of one kind

Two ChatGPT logins are two profiles. Point each at its own directory and every
launch through that profile uses that account, so the row you read is the
account you spend:

```sh
CODEX_HOME=~/.codex-work codex login
alc config upsert codex-work --kind codex --codex-home ~/.codex-work
alc --provider codex-work claude
```

The same for Claude Code, with the directory it keeps its login in:

```sh
CLAUDE_CONFIG_DIR=~/.claude-work claude          # sign in once
alc config upsert anthropic-work --kind anthropic --claude-config-dir ~/.claude-work
```

Both paths must be absolute. A profile's directory beats `CODEX_HOME` or
`CLAUDE_CONFIG_DIR` in your shell, so a variable left in a shell profile cannot
quietly move which account a named profile spends.

`--provider` and the kind shortcuts match an exact profile name first, then a
kind. So with profiles named `codex` and `codex-work`, `alc --codex claude`
resolves to the one actually named `codex` rather than asking which you meant —
name the other explicitly, as above. The shortcut only refuses to choose when
no profile carries the kind's own name and several share that kind.

## Providers with a balance API

| Kind | What the row shows |
| --- | --- |
| `openrouter` | Credit limit, used and remaining |
| `deepseek` | Balance, in the currency the account is billed in |
| `moonshot` | Available balance |
| `minimax` | Remaining requests per window |
| `zai` | Token windows and credit balance |

Everything else — OpenAI, Groq, xAI, Google, Ollama, vLLM, custom endpoints —
reports `no quota API`, because none publishes one for an API key. These
requests go to the vendor's own endpoint, so a profile pointed at a proxy is
reported as having no quota API rather than having its key sent to a host that
did not issue it.

## Usage by provider and agent

Every launch appends a line to `usage.jsonl` in the [config
directory](./configuration.md), and every turn the [Codex
bridge](./codex-to-claude.md) carries appends another with the tokens
chatgpt.com reported. That is the whole source.

A pair alc never carried traffic for shows `—` in the token columns rather than
a zero: `alc claude` on Anthropic talks to Anthropic directly, and alc never
sees the turn. Delete the file to start counting again.

## On the remote-control page

The [remote-control page](./remote-control.md) has the same two sections behind
the usage button in its header: one meter per window, then the ledger. It
refreshes once a minute while that pane is open.

A link that can watch but not type sees the numbers without the email, the
account id or the credential path. The page is served by the hub, which reads
no shell variable and never opens the Keychain — so on macOS the Claude row
there points you back at `alc usage` in a terminal.
