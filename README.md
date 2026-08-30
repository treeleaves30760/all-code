# all-code (`alc`)

**One CLI for eight coding agents.** Configure your LLM providers once, then
launch [Claude Code](https://code.claude.com/docs/en/setup),
[Codex CLI](https://learn.chatgpt.com/docs/codex/cli),
[OpenCode](https://opencode.ai/docs),
[Pi](https://github.com/earendil-works/pi),
[Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli),
[Goose](https://block.github.io/goose/),
[Qwen Code](https://github.com/QwenLM/qwen-code), or
[Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli) with any provider —
including running any of them on your Codex/ChatGPT subscription.

[![CI](https://github.com/treeleaves30760/all-code/actions/workflows/ci.yml/badge.svg)](https://github.com/treeleaves30760/all-code/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/treeleaves30760/all-code?logo=github)](https://github.com/treeleaves30760/all-code/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)](#install)

📖 **[Documentation](https://treeleaves30760.github.io/all-code/)** ·
🇹🇼 **[繁體中文](https://treeleaves30760.github.io/all-code/zh-TW/)**

```text
alc config
alc update
alc claude
alc codex
alc opencode
alc pi
alc copilot
alc goose
alc qwen
alc kimi
alc --codex claude
alc --codex opencode
alc --codex claude --model gpt-5.6-terra --effort medium
alc --deepseek claude
alc --openrouter codex
alc --provider work opencode
```

## What alc does

- **Switch LLM provider per agent.** Point any of the eight agents at
  Anthropic, the OpenAI API, OpenRouter, Ollama, vLLM, DeepSeek, Moonshot,
  Z.ai, MiniMax, Groq, xAI, Google, or any custom endpoint, and change it for
  a single run without editing config files.
- **Run every agent on GPT models.** `alc --codex <agent>` bridges your Codex /
  ChatGPT login to whichever agent you launch. Claude Code lists every GPT
  model in its own `/model` picker so you switch model and reasoning effort
  mid-session; every other agent picks one model for the session.
- **Validate before launching.** alc checks that the agent and provider speak a
  compatible model protocol instead of sending a request that cannot work.
- **Keep credentials out of the way.** API keys live in a separate file or come
  from environment variables; nothing is copied between agents.

## Install

macOS, Linux, and WSL:

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

The installer puts `alc` and its Codex bridge helper in `~/.local/bin`
(Windows: `%USERPROFILE%\.local\bin`) and adds that directory to your user
PATH when needed. On macOS/Linux, restart the terminal or source the profile
named by the installer. PowerShell updates the current session and your
User PATH. If PATH cannot be changed, the installer prints the exact directory
to add manually.

The Windows installer is tested with both Windows PowerShell 5.1 and PowerShell
7, including 32-bit PowerShell running on 64-bit Windows.

To install into a different directory, set `ALC_INSTALL_DIR`. Custom directories
are not added silently; the installer tells you when a manual PATH change is
needed. Set `ALC_NO_PATH_UPDATE=1` to disable automatic PATH changes explicitly.

## Update

Check for a new release or update both `alc` and its bundled helper:

```sh
alc update --check
alc update
```

`alc update` selects the correct release for the current OS and CPU, verifies
the archive against the release's published SHA-256 checksum, checks the
packaged version, and then replaces both binaries. Linux and macOS update
immediately. Windows stages the verified files and finishes replacement just
after the running `alc.exe` exits; wait a moment before checking `alc --version`.
Use `alc update --force` to reinstall the current latest release.

`alc` launches existing coding-agent installations; install the agents you
plan to use separately:

- [Claude Code](https://code.claude.com/docs/en/setup)
- [Codex CLI](https://learn.chatgpt.com/docs/codex/cli)
- [OpenCode](https://opencode.ai/docs)
- [Pi](https://github.com/earendil-works/pi) (`npm install -g @earendil-works/pi-coding-agent`)
- [Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli)
- [Goose](https://block.github.io/goose/)
- [Qwen Code](https://github.com/QwenLM/qwen-code)
- [Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli)

## Quick start

Open the full-screen configuration UI:

```sh
alc config
```

The starter config includes Anthropic, OpenAI, OpenRouter, Codex, Ollama, and a
disabled vLLM template. Add API keys in the TUI or point a profile at an
environment variable. Environment variables take precedence over locally saved
keys.

Then launch an agent with its configured default:

```sh
alc claude
alc codex
alc opencode
alc pi
alc copilot
alc goose
alc qwen
alc kimi
```

Override the provider for one session:

```sh
alc --codex claude
alc --openrouter codex
alc --deepseek pi
alc --codex opencode
alc -p local-vllm opencode
```

`--provider` (or `-p`) takes a profile name, or a provider kind when only one
profile of that kind exists. The shortcut flags `--anthropic`, `--openai`,
`--openrouter`, `--codex`, `--ollama`, `--vllm`, `--deepseek`, `--moonshot`,
`--zai`, `--minimax`, `--groq`, `--xai`, and `--google` are equivalent.

`alc --codex claude` starts Claude Code straight away and lists every GPT model
in Claude Code's own `/model` picker, so you switch model and reasoning effort
inside the session. `alc --codex <agent>` bridges every other agent onto one
model for the session. Set the launch defaults in `alc config`.

Apart from Claude's alc-specific `--model`, `--effort`, and `--save` options,
arguments after the agent name are forwarded unchanged:

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc --ollama opencode run "fix the failing test"
alc goose run --name my-session
```

To pass an option with one of those same names to Claude itself, place it after
`--`, for example `alc claude -- --model sonnet`.

Preview the exact adapter command without launching it. Secrets are redacted:

```sh
alc --openrouter --dry-run claude
```

Run diagnostics:

```sh
alc doctor
```

`alc doctor` reports all eight agent binaries, credential status, provider
profiles with a per-agent compatibility column each, the resolved defaults,
and, when a Codex provider is configured, the Codex bridge's login state.

## Providers and agents

The eight agents do not all speak the same model protocol, and the fourteen
provider kinds do not all expose the same one either. `alc` validates the
combination before launch instead of silently sending a request that cannot
work.

### Provider kinds

| Kind | Default endpoint | Key env | Protocols | Claude-ready? |
| --- | --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | anthropic | Yes |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | responses, chat | No |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | anthropic, responses, chat | Yes |
| `codex` | — (native `codex login`) | — | native | Yes (bridge) |
| `ollama` | `http://localhost:11434` | — | anthropic, responses, chat | Yes |
| `vllm` | `http://localhost:8000/v1` | — | responses, chat | No |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | chat (+ anthropic) | Yes |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | chat (+ anthropic) | Yes |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | chat (+ anthropic) | Yes |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | chat (+ anthropic) | Yes |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | chat | No |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | chat | No |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | chat | No |
| `custom` | user-defined | user-defined (`--api-key-env`) | configurable | No, unless configured |

`deepseek`, `moonshot`, `zai`, and `minimax` each also ship a separate
Anthropic-compatible base URL alongside their primary OpenAI-chat one (see
`alc config show`) — that is what makes those four "Claude-ready" without any
extra configuration. Presets are starting values: run `alc config show` to see
the exact model ID a profile currently uses, and edit it with `alc config
upsert` when upstream renames or retires a model.

### Agent requirements

| Agent | Binary | Accepts | alc injects | Codex bridge |
| --- | --- | --- | --- | --- |
| Claude Code | `claude` | Anthropic-compatible endpoint | env (`ANTHROPIC_BASE_URL`/`ANTHROPIC_MODEL`/`ANTHROPIC_API_KEY`) | Yes (`/model` picker) |
| Codex CLI | `codex` | OpenAI Responses API | flags + `--config` overrides | Yes (native login) |
| OpenCode | `opencode` | Any API-compatible provider | inline `OPENCODE_CONFIG_CONTENT` env | Yes |
| Pi | `pi` | Anthropic-, OpenAI-, or OpenAI-compatible endpoint | `models.json` merge + flags | Yes |
| Copilot CLI | `copilot` | OpenAI- or Anthropic-compatible endpoint | `COPILOT_PROVIDER_*` env | Yes |
| Goose | `goose` | OpenAI- or Anthropic-compatible endpoint | `GOOSE_*` + provider key env | Yes |
| Qwen Code | `qwen` | OpenAI-, Anthropic-, or Gemini-compatible endpoint | `--auth-type` flag + env | Yes |
| Kimi Code CLI | `kimi` | OpenAI- or Anthropic-compatible endpoint | temp `--config-file` (merged TOML, deleted after the run) | Yes |

Every agent reaches the Codex bridge with one `codex login`, regardless of
what it accepts natively — see [Codex bridge](#codex-bridge) below. Run `alc
doctor` for the full compatibility matrix (every provider profile against all
eight agents) resolved for your own configuration.

## Codex bridge

One `codex login` serves every agent alc launches:

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

`alc` starts the bundled `claude-codex` adapter on a loopback port and points
only the launched agent's process at it. The adapter speaks Anthropic Messages
for Claude Code, OpenAI Responses for OpenCode/Pi/Kimi Code CLI, and OpenAI
Chat Completions for Copilot CLI/Goose/Qwen Code — three different wire
protocols backed by the same login. Claude Code is the only agent with
in-session switching (it sends the model and effort with every request, so alc
never pins either on the adapter); every other agent picks one model, and one
pinned reasoning effort, at launch.

### Claude Code

Claude Code starts immediately on your saved default and offers every model in
its own `/model` picker:

| Model | Beginner-friendly use case | Codex default effort |
| --- | --- | --- |
| `gpt-5.6-sol` | Frontier capability for the hardest professional work | `low` |
| `gpt-5.6-terra` | Balanced everyday coding; recommended starting point | `medium` |
| `gpt-5.6-luna` | Fast, affordable, high-volume work | `medium` |

The list is ordered by capability, most capable first, matching Codex's own
tiers for this family.

See OpenAI's [model selection guide](https://developers.openai.com/api/docs/guides/latest-model),
[Luna reference](https://developers.openai.com/api/docs/models/gpt-5.6-luna),
and [Sol reference](https://developers.openai.com/api/docs/models/gpt-5.6-sol)
for current upstream details.

Inside the session, `/model` switches the GPT model and its left/right arrows
adjust the effort slider; `/effort` sets a level directly. Every model accepts
`low`, `medium`, `high`, `xhigh`, or `max`. Higher effort gives the model more
room to reason, but can take longer and use more quota.

alc passes the model list through Claude Code's
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
setting, added in Claude Code 2.1.243. The picker shows only these GPT models
and the Default row, because Claude's own lineup cannot be served through the
Codex adapter. Older clients ignore the setting and still get the launch default
as a selectable entry. Because Claude Code sends the model and effort with every
request, alc never pins either one on the adapter.

To choose a different starting point for one run, or in scripts and CI:

```sh
alc --codex claude --model gpt-5.6-luna --effort low
alc --codex claude --model gpt-5.6-terra --effort medium --save
```

`--save` stores the model and effort in the selected alc provider. Without
these options the session starts on the alc provider's values, then the selected
Codex profile, then the model's documented default. An explicit `--model`,
`--effort`, or `--settings` placed after `--` is forwarded to Claude Code
untouched and wins over what alc would inject.

Claude Code's built-in aliases stay on Codex as well: the picker's Default row
follows the alc default, `haiku` and background work use the cheapest catalog
model, `sonnet` follows the session's starting model, and `opus` uses the most
capable one.

A model chosen with `/model` applies to that Claude Code session. The next
`alc --codex claude` starts from the alc provider default again, so `alc config`
stays the source of truth.

### Every other agent

OpenCode, Pi, and Kimi Code CLI speak the adapter's OpenAI Responses surface
directly; Copilot CLI, Goose, and Qwen Code speak its OpenAI Chat Completions
surface. Each is wired in with its own mechanism (`alc-codex` in
`OPENCODE_CONFIG_CONTENT`, an `alc-codex` `models.json` entry, an `alc-codex`
temp config, or the same BYOK environment variables/`--auth-type` each already
uses for the `openai` kind) pointed at the loopback adapter instead of an
in-session picker.

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

The release archive bundles
[`claude-codex` 0.3.1](https://github.com/fcakyon/claude-code-with-codex), an
MIT-licensed helper. `alc` starts it on a random `127.0.0.1` port, points only
the launched agent's process at it, and stops it when that process exits. The
helper reads and may refresh `~/.codex/auth.json`; credentials are never copied
into the `alc` config.

This adapter is a third-party compatibility layer, not an official OpenAI or
Anthropic integration. Review [THIRD_PARTY.md](THIRD_PARTY.md) and your provider
terms before using subscription credentials through it.

## Configuration

Locations:

| Platform | Config directory |
| --- | --- |
| Windows | `%APPDATA%\alc` |
| macOS/Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/alc` |

Files:

- `config.toml`: provider metadata, models, defaults, URLs, and env-var names.
- `credentials.toml`: locally saved API keys. On Unix, alc writes this file
  with mode `0600`; on Windows it lives under the current user's AppData.

Override the directory with `ALC_CONFIG_DIR`. Useful scripting commands:

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

The TUI keys are shown at the bottom of every screen. The primary controls are:

- `a`, `e`/Enter, `d`: add, edit, or delete a provider.
- `Tab`: switch between provider profiles and agent defaults.
- Arrow keys: navigate fields and cycle choices, including reasoning effort.
- On a Codex profile, `←`/`→` on the Model field opens the guided GPT model and
  effort chooser, which writes the launch defaults for `alc --codex claude`.
- `s`: save; `q`: save and quit; `Ctrl+C`: quit without saving.

## Build from source

Rust 1.88 or newer:

```sh
cargo build --release --locked
```

The source build produces only `alc`. To use `alc --codex <agent>`, put a
compatible `claude-codex` binary on PATH or set `ALC_CLAUDE_CODEX_BIN`. Official
`alc` release archives already bundle the pinned helper.

Useful development checks:

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## Uninstall

Remove `alc` and `claude-codex` from the install directory, then optionally
remove the config directory listed by `alc config path`. Removing the config
also deletes locally saved API keys and cannot be undone.

## License

`alc` is MIT licensed. Bundled third-party notices are in
[THIRD_PARTY.md](THIRD_PARTY.md).
