---
id: getting-started
title: Getting started
sidebar_label: Getting started
sidebar_position: 2
description: Install alc, run Claude Code on your Codex login, point any agent at another provider, forward arguments, preview a launch, and update.
keywords:
  - alc install
  - alc update
  - launch claude code
  - switch llm provider
---

# Getting started

Install, log in to Codex once, launch. Everything else on this page is optional.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

The installer puts `alc` in `~/.local/bin` (Windows: `%USERPROFILE%\.local\bin`)
and adds that directory to your user PATH when it can; when it cannot, it
prints the line to add. `ALC_INSTALL_DIR` chooses another directory;
`ALC_NO_PATH_UPDATE=1` leaves PATH alone.

From source, with Rust 1.88 or newer: `cargo build --release --locked`. The
Codex bridge is part of the binary, so nothing else is needed.

## First run

```sh
codex login
alc --codex claude
```

There is no configuration step. The starter configuration is compiled in, and
`alc --codex claude` reads it in memory. It needs the `auth.json` that
`codex login` writes and `claude` on PATH, and says which one is missing:

```text
error: Codex credentials were not found at ~/.codex/auth.json; run `codex login` and retry
error: 'claude' is not installed or not on PATH; install it first, then retry `alc claude`: cannot find binary path
```

Claude Code starts on `gpt-5.6-terra` at `medium` effort — or on whatever your
own `~/.codex/config.toml` names — with every GPT model in its `/model` picker.
[Codex bridge](./codex-to-claude.md) has the models, the effort tiers, and the
other seven agents.

## Other providers

```sh
alc config                 # keys and per-agent defaults
alc claude                 # each agent on its configured default
alc --openrouter codex
alc --deepseek pi
alc --ollama claude
alc -p local-vllm opencode
```

`--provider` (`-p`) takes a profile name, or a kind when only one profile of
that kind exists; `--anthropic`, `--openai`, `--openrouter`, `--codex`,
`--ollama`, `--vllm`, `--deepseek`, `--moonshot`, `--zai`, `--minimax`,
`--groq`, `--xai`, and `--google` are shortcuts. Keys are saved locally or read
from environment variables; the environment wins. See
[Providers](./providers.md) for what each kind speaks and
[Configuration](./configuration.md) for the files.

## Forwarding arguments

Arguments after the agent name go to the agent unchanged, except Claude's
alc-specific `--model`, `--effort`, and `--save`:

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc claude -- --model sonnet      # `--` hands even those names to Claude
```

alc's own flags — `--share`, `--no-share`, `--bind-lan`, `--name`,
`--permission`, `--tmux`, `-t` — go before the agent name. After it, alc stops
rather than passing them to the agent:

```text
error: `--share` is alc's own flag but it came after the agent's arguments, where it would be passed to claude instead; put it before the agent name, or use `alc share claude -- <args>`
```

## Preview and check

```sh
alc --codex --dry-run claude   # the resolved command, secrets redacted; says when a launch would be refused
alc doctor                     # binaries, credentials, compatibility, defaults, bridge, remote state
```

`alc doctor` exits non-zero when it finds an issue and names the fix for each.
[Troubleshooting](./troubleshooting.md) has the error messages.

## Update

```sh
alc update --check
alc update
```

`alc update` picks the release for this OS and CPU, verifies its SHA-256
checksum, and replaces `alc`. Linux and macOS update immediately; Windows
finishes the replacement after the running `alc.exe` exits. `--force`
reinstalls the current release. Running sessions keep the binary they started
with; restart them, and `alc hub stop` once its sessions are done.

## Uninstall

Delete `alc` from the install directory. `alc config path` names the
configuration directory; removing it also deletes locally saved API keys.
