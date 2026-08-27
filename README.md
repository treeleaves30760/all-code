# all-code (`alc`)

Configure LLM providers once, then launch Claude Code, Codex CLI, or OpenCode
with the provider you want.

```text
alc config
alc update
alc claude
alc codex
alc opencode
alc --codex claude
alc --codex claude --model gpt-5.6-terra --effort medium
alc --openrouter codex
alc --provider work opencode
```

[繁體中文快速開始](docs/README.zh-TW.md)

## Install

macOS, Linux, and WSL:

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

The installer puts `alc` and its Codex-to-Claude loopback helper in
`~/.local/bin` (Windows: `%USERPROFILE%\.local\bin`) and adds that directory to
your user PATH when needed. On macOS/Linux, restart the terminal or source the
profile named by the installer. PowerShell updates the current session and your
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
```

Override the provider for one session:

```sh
alc --codex claude
alc --openrouter codex
alc -p local-vllm opencode
```

`alc --codex claude` starts Claude Code straight away and lists every GPT model
in Claude Code's own `/model` picker, so you switch model and reasoning effort
inside the session. Set the launch defaults in `alc config`.

Apart from Claude's alc-specific `--model`, `--effort`, and `--save` options,
arguments after the agent name are forwarded unchanged:

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc --ollama opencode run "fix the failing test"
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

## Provider compatibility

The coding agents do not all speak the same model protocol. `alc` validates the
combination before launch instead of silently sending an incompatible request.

| Provider profile | Claude Code | Codex CLI | OpenCode |
| --- | --- | --- | --- |
| Anthropic | Yes | No | Yes |
| OpenAI API | Gateway required | Yes (Responses API) | Yes |
| OpenRouter | Yes (Anthropic skin) | Yes (Responses API) | Yes |
| Codex login | Yes (bundled adapter) | Yes (native) | No direct credential reuse |
| Ollama | Yes (Anthropic compatibility) | Yes (`--oss`) | Yes |
| vLLM | If it exposes Anthropic Messages | If it exposes Responses | Yes |
| Custom | According to configured protocol | According to configured protocol | Yes |

Relevant upstream behavior:

- Claude Code gateways must expose Anthropic Messages, Bedrock, or Vertex API
  formats. `ANTHROPIC_BASE_URL` selects the gateway.
- Codex custom providers use the OpenAI Responses wire API.
- OpenRouter and Ollama expose Anthropic-compatible endpoints that Claude Code
  can use directly.

If an OpenAI-compatible service only implements Chat Completions, use it with
OpenCode. Claude Code needs an Anthropic-compatible gateway, and current Codex
requires Responses rather than Chat Completions.

## `alc --codex claude`

This path lets Claude Code use GPT models available through your Codex/ChatGPT
login:

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
that Claude Code child process at it, and stops it when Claude exits. The helper
reads and may refresh `~/.codex/auth.json`; credentials are never copied into
the `alc` config.

Claude Code's built-in aliases stay on Codex as well: the picker's Default row
follows the alc default, `haiku` and background work use the cheapest catalog
model, `sonnet` follows the session's starting model, and `opus` uses the most
capable one.

A model chosen with `/model` applies to that Claude Code session. The next
`alc --codex claude` starts from the alc provider default again, so `alc config`
stays the source of truth.

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

The source build produces only `alc`. To use `alc --codex claude`, put a
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
