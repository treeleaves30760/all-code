# all-code (`alc`)

Configure LLM providers once, then launch Claude Code, Codex CLI, or OpenCode
with the provider you want.

```text
alc config
alc claude
alc codex
alc opencode
alc --codex claude
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
your user PATH when needed. Restart the terminal after the first installation.

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

Everything after the agent name is forwarded unchanged:

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc --ollama opencode run "fix the failing test"
```

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

The release archive bundles
[`claude-codex` 0.3.1](https://github.com/fcakyon/claude-code-with-codex), an
MIT-licensed helper. `alc` starts it on a random `127.0.0.1` port, points only
that Claude Code child process at it, and stops it when Claude exits. The helper
reads and may refresh `~/.codex/auth.json`; credentials are never copied into
the `alc` config.

The model comes from the selected alc profile. If that field is empty, `alc`
reads the selected `~/.codex/<profile>.config.toml`, then
`~/.codex/config.toml`, and finally falls back to `gpt-5.6`.

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
alc config upsert work --kind openrouter --model anthropic/claude-sonnet-4.6
printf '%s' "$OPENROUTER_API_KEY" | alc config key work --stdin
alc config set-default claude work
alc config remove work
```

The TUI keys are shown at the bottom of every screen. The primary controls are:

- `a`, `e`/Enter, `d`: add, edit, or delete a provider.
- `Tab`: switch between provider profiles and agent defaults.
- Arrow keys: navigate fields and cycle choices.
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
