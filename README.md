# all-code (`alc`)

**Run Claude Code on the Codex/ChatGPT subscription you already pay for** — and
seven other coding agents besides, on that same login or on any provider you
point them at. Any session can be mirrored to a browser page and driven from
another device.

[![CI](https://github.com/treeleaves30760/all-code/actions/workflows/ci.yml/badge.svg)](https://github.com/treeleaves30760/all-code/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/treeleaves30760/all-code?logo=github)](https://github.com/treeleaves30760/all-code/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)](#install)

📖 **[Documentation](https://treeleaves30760.github.io/all-code/)** ·
🇹🇼 **[繁體中文](https://treeleaves30760.github.io/all-code/zh-TW/)**

## Claude Code on your ChatGPT plan, in three commands

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
codex login
alc --codex claude
```

Windows PowerShell: `irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex`

**There is no configuration step.** No `alc config init`, no file to edit. The
starter configuration is compiled into the binary and already carries a Codex
profile; `alc --codex claude` reads it in memory and writes nothing to disk.

**What it needs** is the `auth.json` that `codex login` writes, and `claude` on
PATH — alc launches coding agents, it does not bundle them. **What it does not
need** is an API key, an alc configuration file, or the `codex` binary at launch;
that one is for running `codex login` itself and for keeping the model catalog
fresh. alc checks the login before it looks for the agent, so somebody missing
both is told about `codex login` first:

```text
error: Codex credentials were not found at ~/.codex/auth.json; run `codex login` and retry
error: 'claude' is not installed or not on PATH; install it first, then retry `alc claude`
```

**What you get.** Claude Code starts on `gpt-5.6-terra` at `medium` effort — or
on whatever model and effort your own `~/.codex/config.toml` already names, if
you have used Codex CLI before — with the real 272k Codex context window rather
than the 200k Claude Code assumes for a model ID it does not recognize, and
every GPT model Codex serves in its own `/model` picker:

| Model | Beginner-friendly use case | Codex default effort |
| --- | --- | --- |
| `gpt-6-astra` | GPT-6. Most capable; complex, demanding work | `medium` |
| `gpt-5.6-sol` | Frontier capability for the hardest professional work | `low` |
| `gpt-5.6-terra` | Balanced everyday coding; recommended starting point | `medium` |
| `gpt-5.6-luna` | Fast, affordable, high-volume work | `medium` |

Inside the session, `/model` switches the model and its left/right arrows move
the effort slider; `/effort` sets a level directly. To start somewhere else for
one run, or in scripts:

```sh
alc --codex claude --model gpt-5.6-luna --effort low
```

**Your plain `claude` still reaches Anthropic afterwards.** That picker writes
its choice to `~/.claude/settings.json`, which every Claude Code session on the
machine reads — including the ones alc did not start, which have no adapter in
front of them. alc reads that one key before the launch and puts it back when
the session exits. [Codex bridge](#codex-bridge) has the details, and the two
cases where it deliberately leaves the file alone.

alc prints nothing at launch: what you are looking at is Claude Code. The
adapter in between is a third-party compatibility layer, not an official OpenAI
or Anthropic integration — review [THIRD_PARTY.md](THIRD_PARTY.md) and your
provider terms before routing subscription credentials through it.

## One login, every agent

The same `codex login` drives every agent alc launches. No second key, no
per-agent setup.

```sh
alc --codex claude       # in-session /model picker
alc --codex opencode
alc --codex pi
alc --codex copilot
alc --codex goose
alc --codex qwen
alc --codex kimi
alc codex                # Codex CLI itself, on its own login, no adapter
```

| Agent | Reaches the bridge over | Switching models |
| --- | --- | --- |
| Claude Code | Anthropic Messages | `/model` picker, mid-session |
| OpenCode, Pi, Kimi Code CLI | OpenAI Responses | one model, chosen at launch |
| Copilot CLI, Goose, Qwen Code | OpenAI Chat Completions | one model, chosen at launch |
| Codex CLI | native, no bridge | Codex's own picker |

alc starts its own Codex adapter on a loopback port and points only the launched
agent's process at it — three wire protocols, one login, one process that stops
when the session does. Claude Code is the only agent that can switch mid-session,
because it sends the model and effort with every request, so alc never pins
either on the adapter; the others pick one model and one reasoning effort at
launch.

[Codex bridge](#codex-bridge), below, has the model catalog, the effort tiers,
and what the adapter does with your credentials.

## Drive it from your phone

Any session, any agent, any provider — mirrored to a browser page you can open
from anywhere that can reach the machine.

```sh
alc --share claude          # or: alc share claude
```

```text
alc session claude-7QK2M9XB4T (claude@all-code)
  open  http://127.0.0.1:8787/#k=…
  hub   127.0.0.1:8787 · loopback only (pid 48213) · this link grants input; keep it to yourself
  keys  ctrl-\ then d detaches; the session keeps running
```

**Your terminal keeps working.** Sharing mirrors a session, it does not take it
away. What is mirrored is the terminal itself, which is why every agent and
every provider works the same way — there is nothing per-agent to support.

**What the page gives you** is the session list, the live screen, a key bar for
the keys a phone keyboard does not have (Esc, Tab, Shift+Tab, Ctrl, arrows), and
a composer that sends a whole prompt as one block instead of fighting a mobile
keyboard inside a raw terminal.

**Sessions outlive the terminal that started them**, because a background hub
owns them:

```sh
alc sessions               # the link, then what is running
alc attach 7QK2            # back on it, from any terminal
alc kill 7QK2
```

Ids can be given as any unambiguous prefix, git-short-hash style. `alc sessions`
leads with the link because the one `--share` printed scrolls away the moment the
agent draws its own interface.

**What the link can do.** A shared session starts in **ask** mode — the link does
not hand anyone an autonomous agent — and loosening it past the ceiling you
configure needs `alc confirm <ticket>` typed at a terminal on the host machine.
[Remote control](#remote-control) has the permission ladder and the threat model.

Remote control needs macOS or Linux for now; on Windows `--share` and `alc hub`
refuse with a message and everything else works normally.

## Any provider, not just Codex

`codex login` is the shortest path, not the only one. Point any of the eight
agents at Anthropic, the OpenAI API, OpenRouter, a local Ollama or vLLM server,
DeepSeek, Moonshot, Z.ai, MiniMax, Groq, xAI, Google, or a custom endpoint — and
change it for a single run without editing anything.

```sh
alc config                 # keys and per-agent defaults live here
alc claude                 # each agent on its configured default
alc --openrouter codex
alc --deepseek pi
alc --ollama claude
alc -p local-vllm opencode
```

`--provider` (or `-p`) takes a profile name, or a provider kind when only one
profile of that kind exists. The shortcut flags `--anthropic`, `--openai`,
`--openrouter`, `--codex`, `--ollama`, `--vllm`, `--deepseek`, `--moonshot`,
`--zai`, `--minimax`, `--groq`, `--xai`, and `--google` are equivalent. The
starter configuration ships Anthropic, OpenAI, OpenRouter, Codex, Ollama, and a
disabled vLLM template; keys are saved locally or read from environment
variables, and environment variables win.

The eight agents do not all speak the same model protocol, and the fourteen
provider kinds do not all expose the same one, so alc checks the combination
before launch instead of sending a request that cannot work.
[Providers and agents](#providers-and-agents) has the endpoint, key variable,
and protocol for every kind.

## Command reference

| Command | What it does |
| --- | --- |
| `alc claude`, `codex`, `opencode`, `pi`, `copilot`, `goose`, `qwen`, `kimi` | Launch that agent on its configured provider |
| `alc config` | The configuration TUI; also `init`, `show`, `path`, `upsert`, `key`, `set-default`, `remove` |
| `alc doctor` | Binaries, credentials, compatibility, defaults, bridge and remote state |
| `alc models` | The GPT models the Codex bridge offers; `--refresh`, `--json` |
| `alc update` | Update `alc` in place; `--check`, `--force` |
| `alc share <agent>` | Launch with the session mirrored to a browser page |
| `alc sessions` | The page link, then the shared sessions (tmux ones marked) |
| `alc attach <id>` | Put this terminal back on a shared session |
| `alc rename <id> <name>` | Rename a session's card on the page |
| `alc kill <id>` | Stop a shared session |
| `alc hub` | `status`, `start`, `stop --drain` for the process that owns sessions |
| `alc remote` | `status`, `url`, `on`/`off`, `auto-share`, `allow-host`, `token --rotate` |
| `alc confirm <ticket>` | Approve a permission change a shared session asked for |

`alc <command> --help` has the flags for each.

## Running agents

**Forwarding.** Apart from Claude's alc-specific `--model`, `--effort`, and
`--save`, arguments after the agent name are forwarded unchanged:

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc --ollama opencode run "fix the failing test"
alc goose run --name my-session
```

To pass an option with one of those same names to Claude itself, put it after
`--`: `alc claude -- --model sonnet`.

alc's own flags — `--share`, `--no-share`, `--bind-lan`, `--name`,
`--permission`, `--tmux`, `-t` — have to come *before* the agent name. After it
they would be handed to the agent as prompt text, so alc stops and says so
instead.

**Previewing.** `alc --openrouter --dry-run claude` prints the resolved agent and
provider, the command with secrets redacted, the adapter's loopback port, and
every file it would write — and says when a launch would be refused rather than
only what would succeed.

## Diagnostics

```sh
alc doctor
```

`alc doctor` reports the environment and credential paths, all eight agent
binaries, every provider profile against all eight agents, the resolved per-agent
defaults, a leftover GPT model pinned in `~/.claude/settings.json`, the Codex
bridge's model, effort, and `codex login` state, an enabled Ollama profile
checked against the running server, and the remote-control posture — then a
summary of issues with a fix for each. It exits non-zero when it finds one.

For named errors and their fixes, see the
[troubleshooting guide](https://treeleaves30760.github.io/all-code/troubleshooting).

## Providers and agents

The two lookup tables, resolved for your own configuration by `alc doctor`.

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

### Agents

| Agent | Binary | Accepts | alc injects | Codex bridge |
| --- | --- | --- | --- | --- |
| [Claude Code](https://code.claude.com/docs/en/setup) | `claude` | Anthropic-compatible endpoint | env (`ANTHROPIC_BASE_URL`/`ANTHROPIC_MODEL`/`ANTHROPIC_API_KEY`) | Yes (`/model` picker) |
| [Codex CLI](https://learn.chatgpt.com/docs/codex/cli) | `codex` | OpenAI Responses API | flags + `--config` overrides | Yes (native login) |
| [OpenCode](https://opencode.ai/docs) | `opencode` | Any API-compatible provider | inline `OPENCODE_CONFIG_CONTENT` env | Yes |
| [Pi](https://github.com/earendil-works/pi) | `pi` | Anthropic-, OpenAI-, or OpenAI-compatible endpoint | `models.json` merge + flags | Yes |
| [Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli) | `copilot` | OpenAI- or Anthropic-compatible endpoint | `COPILOT_PROVIDER_*` env | Yes |
| [Goose](https://block.github.io/goose/) | `goose` | OpenAI- or Anthropic-compatible endpoint | `GOOSE_*` + provider key env | Yes |
| [Qwen Code](https://github.com/QwenLM/qwen-code) | `qwen` | OpenAI-, Anthropic-, or Gemini-compatible endpoint | `--auth-type` flag + env | Yes |
| [Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli) | `kimi` | OpenAI- or Anthropic-compatible endpoint | temp `--config-file` (merged TOML, deleted after the run) | Yes |

`alc` launches agents that are already installed — install the ones you plan to
use from the links above (Pi is `npm install -g @earendil-works/pi-coding-agent`).
Every agent reaches the Codex bridge with one `codex login` whatever it accepts
natively, and `ALC_CLAUDE_BIN` and its siblings override a binary path.

## Codex bridge

alc offers the models Codex lists, and the bridge keeps no allowlist of its own:
whatever slug it is handed goes upstream, and chatgpt.com decides. A model is
therefore usable on the day Codex ships it rather than the day alc catches up —
which is what made `gpt-6-astra` unreachable before 1.5.0, while `codex` itself
was already serving it.

**Effort.** Every model accepts `low`, `medium`, `high`, `xhigh`, or `max`.
Higher effort gives the model more room to reason, but can take longer and use
more quota. `gpt-6-astra` and the newer GPT-5.6 models also offer an `ultra` tier
above `max`. That tier is reachable with native `alc codex`, but **not** through
the bridge: the built-in helper's own effort range stops at `max`, so alc clamps
it there and says so at launch rather than letting the request be refused
mid-session.

See OpenAI's [model selection guide](https://developers.openai.com/api/docs/guides/latest-model),
[Luna reference](https://developers.openai.com/api/docs/models/gpt-5.6-luna),
and [Sol reference](https://developers.openai.com/api/docs/models/gpt-5.6-sol)
for current upstream details.

**Choosing the defaults.**

```sh
alc --codex claude --model gpt-5.6-terra --effort medium --save
```

`--save` stores both in the selected alc provider. Without them the session
starts on the alc provider's values, then the selected Codex profile's, then the
model's documented default. A `--model`, `--effort`, or `--settings` placed after
`--` is forwarded to Claude Code untouched and wins over what alc would inject. A
model chosen with `/model` applies to that session only; the next launch starts
from the alc default again, so `alc config` stays the source of truth.

**The picker.** alc passes the model list through Claude Code's
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
setting, added in Claude Code 2.1.243. The picker shows only these GPT models and
the Default row, because Claude's own lineup cannot be served through the
adapter; older clients ignore the setting and still get the launch default as a
selectable entry. Claude Code's built-in aliases stay on Codex as well: the
Default row follows the alc default, `haiku` and background work use the cheapest
catalog model, `sonnet` follows the session's starting model, and `opus` uses the
most capable one.

**Your Claude Code default.** One thing to know about that picker, because it is
Claude Code's and not alc's: the model it settles on is also written to
`~/.claude/settings.json` as your default for new sessions. That file is read by
every Claude Code session on the machine, including the ones alc did not start,
and those have no adapter in front of them — a plain `claude` afterwards would
ask Anthropic for a GPT model and be told it does not exist.

alc puts that one key back when the session exits. It reads the value before the
launch and restores it afterwards, so your own default survives a trip through
the adapter. Two things it deliberately leaves alone: a real Claude model you
switched to mid-session, which is your choice and not alc's to overrule, and a
session that was killed outright, where nothing ran to restore anything. For that
last case `alc doctor` still reports the file and the line — and the next
`alc --codex claude` clears it, because a value that is *already* one only the
adapter can serve is removed rather than put back.

### Every other agent

OpenCode, Pi, and Kimi Code CLI speak the adapter's OpenAI Responses surface
directly; Copilot CLI, Goose, and Qwen Code speak its OpenAI Chat Completions
surface. Each is wired in with its own mechanism (`alc-codex` in
`OPENCODE_CONFIG_CONTENT`, an `alc-codex` `models.json` entry, an `alc-codex`
temp config, or the same BYOK environment variables and `--auth-type` each
already uses for the `openai` kind) pointed at the loopback adapter instead of an
in-session picker.

The model catalog is synchronized from the installed Codex CLI at most once every
24 hours. A catalog bundled into the binary keeps the list working offline, and
with no `codex` installed at all:

```sh
alc models
alc models --refresh
alc models --json
```

The synchronized Codex context window is also passed to Claude Code through its
documented
[`CLAUDE_CODE_MAX_CONTEXT_TOKENS`](https://code.claude.com/docs/en/env-vars)
gateway setting, so unknown GPT IDs compact at the correct Codex limit instead of
Claude's generic fallback.

### How the bridge works

The bridge is alc's own code (`src/bridge/`). It runs inside the `alc` process on
a random `127.0.0.1` port, points only the launched agent at it, and stops when
that session ends. It reads and may refresh `~/.codex/auth.json`; credentials are
never copied into the `alc` config.

## Local models

`alc --ollama claude` points Claude Code at the Ollama server's Anthropic Messages
endpoint. A local server serves only the models it has pulled and answers one
request at a time, so alc sets the session up differently from a hosted provider:
every model alias (`ANTHROPIC_DEFAULT_MODEL` and the sonnet/opus/haiku tiers)
pinned to the profile's model so Claude Code never asks Ollama for a model ID it
does not have, non-essential traffic off, the real context window read from
`/api/ps` or `/api/show`, and the first-token timeouts raised to thirty minutes.

That last one matters more than it sounds. Claude Code opens every session with a
request of roughly 25k to 40k tokens — system prompt, tool schemas, project
context — and a laptop-sized model reads that at a few dozen tokens per second:
on an M3 MacBook Air, `gemma4:12b` needs about six minutes before the first token
of a 22k-token request, and fifteen for a 39k one. Without the raised timeouts
Claude Code abandons each attempt after six minutes and starts over.

What makes it pleasant on a laptop: a model whose `ollama show <model>` lists the
`tools` capability, a 64k to 128k context window, a first request kept small
(every MCP server, plugin, and skill adds tool schemas to it), and the model kept
loaded so the prompt cache survives between turns. `alc doctor` prints an
**Ollama** section with the server version, whether the model is pulled, whether
it can call tools, and the context it actually gets.

Full tuning notes — KV cache type, keep-alive, why prompt length costs more than
linearly on Gemma 4 — are in the
[provider guide](https://treeleaves30760.github.io/all-code/providers#claude-code-on-a-local-ollama-model).

## Remote control

[Drive it from your phone](#drive-it-from-your-phone) has the short version; this
is the rest.

```sh
alc sessions                 # the link, then what is running (tmux sessions marked)
alc attach 7QK2              # any unambiguous id prefix
alc rename 7QK2 review
alc kill 7QK2
alc hub status               # or bare `alc hub`
alc hub stop --drain
alc remote url               # the link again, after it scrolled away
alc remote status            # on/off, bind, ceiling, where the files are
alc remote auto-share on     # share every session without --share
alc remote off               # refuse to share sessions at all
alc remote token --rotate    # invalidate every link handed out so far
```

Sessions are owned by a background hub, which is why they survive the terminal
that started them and why they all appear on one page; `ctrl-\` then `d`
detaches. Sharing by default also lives in `alc config`, on the **Sharing &
remote** screen.

### Who owns the size

A shared session without `--tmux` is one terminal with two viewers, and a
terminal has one size. That size belongs to the terminal you launched from, which
is still sitting there drawing at it — so the page does not touch it. It draws
the agent's real grid instead, as large as it fits, centred, with black where the
ratio does not match. Resize your terminal and the page follows within a few
seconds.

`--tmux` is for when the page is the side you are actually going to use:

```sh
alc --share --tmux --codex claude    # or -t
```

Your terminal and the hub each attach as their own tmux client, so nothing has to
agree on a size. The trade is that it runs the opposite way round — the page sets
the size, and a narrower or shorter terminal shows the top-left corner of the
screen (pan with `ctrl-b :refresh-client -L/-R/-U/-D`). With no browser attached
the size stays where it launched. tmux owns the keyboard too, so detaching is
`ctrl-b` then `d`, and in exchange you get tmux's scrollback and a session that
survives an ssh drop.

alc runs its own tmux server per session and starts it with no configuration
file, so your own tmux is untouched, alc's sessions behave the same for
everybody, and a `~/.tmux.conf` is never a way into a session's environment.
Running alc from inside tmux is fine. Needs tmux 3.2 or newer; `alc doctor` says
what you have. One caveat worth stating plainly: your local terminal is now a
direct tmux client rather than a mirror, so it shows the agent's raw output. The
browser still sees API keys alc injected masked; your own terminal does not.

### Reaching it from a phone

Three ways, all supported:

```sh
# Your own Wi-Fi — nothing to install
alc claude --share --bind-lan          # prints http://192.168.1.42:8787/#k=…

# Tailscale — alc stays on loopback, HTTPS, no third party
alc remote allow-host box.tail1a2b.ts.net
tailscale serve 8787

# Cloudflare Tunnel — works over cellular, no VPN
alc remote allow-host '*.trycloudflare.com'
cloudflared tunnel --url http://127.0.0.1:8787
```

alc answers only to names you allowed. Loopback is always allowed, and
`--bind-lan` adds this machine's own addresses; `alc remote allow-host` adds a
tunnel's hostname, exactly or as `*.example.com` for a tunnel that renames itself
every run. Restart a running hub (`alc hub stop --drain`) for a new allowed host
to take effect. A LAN link is plain HTTP, so the token crosses your local network
in clear — fine at home, use a tunnel on café Wi-Fi.

### Permissions

alc has five rungs, loosest last: `plan` (reads and plans, writes nothing), `ask`
(asks before anything that changes the world), `auto-edit` (edits files without
asking, still asks for commands), `auto` (acts inside whatever sandbox the agent
has), and `full` (no gate). `--permission <rung>` sets where a session starts.

A shared session with no `--permission` starts at **ask**, so a link opened on a
phone is not looking at an autonomous agent. The page can change the rung while
the session runs, up to a ceiling — `auto-edit` by default, set on the **Sharing
& remote** screen of `alc config`. Tightening is never gated.

Past the ceiling, the page shows a ticket and you type it at a terminal on the
host machine:

```sh
alc confirm 7QK2M9XB4T
```

It prints `granted: <rung>`, and the page may apply that one change within the
next minute. Tickets expire unredeemed after five minutes, and `alc confirm`
refuses to run without a controlling terminal — which is the point: the agent
cannot redeem its own ticket.

The eight agents do not agree on what a permission mode is, so alc records for
each one whether it verified the flags against a real `--help`, whether the mode
can be changed mid-session at all (Kimi is relaunch-only; Pi has no permission
model and is not sandboxed), and whether the current mode was set at launch, read
back off the screen, or merely assumed. The page shows the agent's own word
beside alc's rung — `Auto-edit · Claude Code: acceptEdits` — because a single
shared label would mislead.

### What this actually grants

A page that types into a coding agent is remote code execution on your machine,
so it is worth being plain about the model:

- The link's fragment (`#k=…`) **is** the credential. Anyone who has it can type
  into the session. It never reaches the server, a proxy, or an access log — but
  it is in your clipboard, so treat it like a password.
- alc checks the `Host` header down to the port, requires an `Origin` on the
  WebSocket upgrade, and compares tokens in constant time. That is what stops a
  page at some other origin from driving your agent through your browser.
- The browser's HTTP plane and the channel that creates processes are different
  sockets with different credentials — on Unix the control channel is a `0600`
  unix socket in a `0700` directory, so a browser token cannot reach session
  creation.
- A shared session is screen sharing. alc masks the API keys **it** put into the
  environment, but anything else the agent prints, a viewer sees.
- `alc remote token --rotate` invalidates every link handed out so far.
- alc reads no configuration from the working repository, so a checked-in file
  can never turn sharing on.

`--share` needs a real terminal on both ends and refuses when input or output is
redirected, so a scripted `alc claude -p … > out.txt` keeps behaving exactly as it
does today.

## Configuration

| Platform | Config directory |
| --- | --- |
| Windows | `%APPDATA%\alc` |
| macOS/Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/alc` |

- `config.toml`: provider metadata, models, defaults, URLs, and env-var names.
- `credentials.toml`: locally saved API keys. On Unix, alc writes this file with
  mode `0600`; on Windows it lives under the current user's AppData.
- `remote.toml`: sharing — on/off, share-by-default, bind address, port, and the
  permission ceiling. `alc config show` prints these as comments at the end.

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

The TUI keys are shown at the bottom of every screen. `Tab`/`Shift+Tab`, or
`1`/`2`/`3`, move between the three screens named in the header — Providers,
Agent defaults, and Sharing & remote. On a Codex profile, `←`/`→` on the Model
field opens the guided GPT model and effort chooser, which writes the launch
defaults for `alc --codex claude`.

Credential precedence, the full `alc config upsert` flag list, and the
Codex-to-Claude setting precedence are in the
[configuration guide](https://treeleaves30760.github.io/all-code/configuration).

## Install

macOS, Linux, and WSL:

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

The installer puts `alc` in `~/.local/bin` (Windows:
`%USERPROFILE%\.local\bin`) and adds that directory to your user PATH when
needed. On macOS/Linux, restart the terminal or source the profile named by the
installer; PowerShell updates the current session and your User PATH. If PATH
cannot be changed, the installer prints the exact directory to add manually. To
install into a different directory, set `ALC_INSTALL_DIR` — custom directories
are not added silently — or set `ALC_NO_PATH_UPDATE=1` to disable automatic PATH
changes explicitly. The Windows installer is tested with both Windows PowerShell
5.1 and PowerShell 7, including 32-bit PowerShell running on 64-bit Windows.

### Updating

```sh
alc update --check
alc update
```

`alc update` selects the correct release for the current OS and CPU, verifies the
archive against the release's published SHA-256 checksum, checks the packaged
version, and then replaces `alc`. Linux and macOS update immediately. Windows
stages the verified files and finishes replacement just after the running
`alc.exe` exits; wait a moment before checking `alc --version`. Use
`alc update --force` to reinstall the current latest release.

Running sessions keep the binary they started with, so restart any open
`alc`-launched agent after updating, and `alc hub stop` once its sessions are
done.

## Build from source

Rust 1.88 or newer:

```sh
cargo build --release --locked
```

The Codex bridge is alc's own code (`src/bridge/`), compiled into the binary, so
a source build is a complete one — `alc --codex <agent>` works with nothing else
installed. Release archives contain only `alc` for the same reason.

Useful development checks:

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## Uninstall

Remove `alc` from the install directory, then optionally remove the config
directory listed by `alc config path`. Removing the config also deletes locally
saved API keys and cannot be undone.

## License

`alc` is MIT licensed. Bundled third-party notices are in
[THIRD_PARTY.md](THIRD_PARTY.md).
