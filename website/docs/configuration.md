---
id: configuration
title: Configuration
sidebar_position: 6
description: Where alc stores provider profiles and API keys, how to edit them in the TUI, and the scripting commands that change configuration without it.
keywords:
  - alc config
  - provider profile
  - api key storage
---

# Configuration

## File locations

| Platform | Config directory |
| --- | --- |
| Windows | `%APPDATA%\alc` |
| macOS/Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/alc` |

Files:

- `config.toml`: provider metadata, models, defaults, URLs, and env-var names.
- `credentials.toml`: locally saved API keys. On Unix, alc writes this file
  with mode `0600`; on Windows it lives under the current user's AppData.

Override the directory with `ALC_CONFIG_DIR`.

## The configuration TUI

```sh
alc config
```

The keys are shown at the bottom of every screen. The primary controls are:

- `a`, `e`/Enter, `d`: add, edit, or delete a provider.
- `Tab`: switch between provider profiles and agent defaults.
- Arrow keys: navigate fields and cycle choices, including reasoning effort.
- On a Codex profile, `←`/`→` on the Model field opens the guided GPT model and
  effort chooser, which writes the launch defaults for `alc --codex claude`.
- `s`: save; `q`: save and quit; `Ctrl+C`: quit without saving.

## Scripting commands

```sh
alc config init
alc config show
alc config path
alc config upsert codex --kind codex --model gpt-5.6-terra --effort medium
alc config upsert work --kind openrouter --model anthropic/claude-sonnet-4.6
printf '%s' "$OPENROUTER_API_KEY" | alc config key work --stdin
alc config set-default claude work
alc config remove work
```

`alc config upsert` accepts `--kind`, `--model`, `--effort`, `--clear-effort`,
`--small-model`, `--base-url`, `--anthropic-base-url`, `--protocol`, `--auth`,
`--api-key-env`, `--codex-profile`, `--disable`, and `--enable`.

## Credential precedence

For each provider profile, alc resolves the API key in this order:

1. The environment variable named by `api_key_env`, when it is set and not
   empty.
2. The key saved in `credentials.toml`.

Profiles whose authentication style is `native` or `none` need no key at all —
that covers the Codex login and local runtimes such as Ollama.

## Setting precedence for Codex-to-Claude

1. This run's `--model` / `--effort`
2. The alc provider profile
3. `~/.codex/<profile>.config.toml`, then `~/.codex/config.toml`
4. The model catalog's documented default
