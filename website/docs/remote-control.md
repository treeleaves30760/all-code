---
id: remote-control
title: Remote control
sidebar_label: Remote control
---

Mirror a running session to a web page and drive it from another device — the
same page for every agent and every provider.

```sh
alc --share claude
alc share opencode -- --mini
```

alc prints a link:

```text
alc session claude-7QK2M9XB4T (claude@all-code)
  open  http://127.0.0.1:8787/#k=…
  bind  127.0.0.1:8787 · this link grants input; keep it to yourself
  pid   48213
```

Your own terminal keeps working exactly as before. Sharing mirrors the
session; it does not take it away.

## Why it works for every agent

The eight agents alc launches disagree about almost everything: some expose a
machine-readable control channel, some expose nothing, and the ones that do
disagree on transport, session identity, and what an approval even is. The one
thing they all share is the terminal, so that is what alc mirrors. A session
behaves the same whichever agent and whichever provider it runs on — including
provider setups that agents' own remote features refuse to work with.

## What the page gives you

- **The session list**, with the agent, provider, model and working directory.
- **The live screen**, rendered by a real terminal emulator, so full-screen
  TUIs look the way they do locally.
- **A key bar** for the keys a phone keyboard does not have — Esc, Tab,
  Shift+Tab, Ctrl (sticky, so you can press it then a letter), and arrows.
  Shift+Tab is how Claude Code cycles its permission mode.
- **A composer** that sends a whole prompt as one block. Typing a long prompt
  into a raw terminal on a phone means fighting your own IME, which rewrites
  text it has already emitted; a raw terminal cannot take that back.
- **Reconnection that keeps your place.** Close the tab, walk into a tunnel,
  come back — the page asks for exactly the bytes it missed, and gets the
  current screen instead when it has been away too long.

The page follows your device's language: it is available in English and
Traditional Chinese.

## Reaching it from a phone

The default binds to loopback only. Nothing is exposed until you say so.

### A tunnel you run (recommended)

With [Tailscale](https://tailscale.com/) on both devices:

```sh
tailscale serve 8787
```

Then open the `ts.net` address on your phone. alc never has to be the thing
facing the network, and there is no third party in the middle to trust.

### Your LAN

This needs two switches, on purpose. In `remote.toml`:

```toml
allow_lan = true
bind      = "lan"
```

and on the command line:

```sh
alc --share --bind-lan claude
```

One switch is too easy to leave on by accident, and what is on the other side
of that socket is a shell.

## What sharing actually grants

A page that types into a coding agent is remote code execution on your
machine. The model is worth stating plainly.

- **The link's fragment is the credential.** Anyone who has the `#k=…` part
  can type into the session. A fragment is never sent to a server, so it does
  not land in an access log or a proxy — but it does land in your clipboard.
  Treat it like a password.
- **alc checks `Host` down to the port** and requires an `Origin` on the
  WebSocket upgrade. That is what stops a page at some other origin, resolved
  to `127.0.0.1`, from driving your agent through your own browser. Tokens are
  compared in constant time.
- **A shared session is screen sharing.** alc masks the API keys *it* put into
  the environment, so an agent that prints its own environment does not fan
  your provider key out to every viewer. Masking applies to every view,
  including the terminal you started the session from — one rule for every
  viewer is easier to reason about, and the only thing it costs is seeing
  your own key echoed back. Anything else the agent prints, a viewer sees.
  That part cannot be solved, only scoped.
- **alc never answers a clipboard read.** A terminal can be asked to type the
  viewer's clipboard back into the program's input; a hostile file an agent
  prints would otherwise pull whatever you last copied into the model's
  context. alc refuses, and says so on the page when something tries.
- **alc reads no configuration from the working repository.** A checked-in
  file can never turn sharing on.

`--share` needs a real terminal on both ends. It refuses when input or output
is redirected, so a scripted `alc claude -p "…" > out.txt` keeps behaving
exactly as it does today.

## Permission modes, agent by agent

The eight agents disagree about what a permission mode is, what the modes are
called, and whether one can be changed at all after launch. The page renders
whatever the agent in front of it can actually do — a dropdown where a mode
can be set outright, a relative "cycle" button where it cannot, and a disabled
control carrying the reason where the agent has no such concept.

It always shows **alc's rung and the agent's own word for it**, because a
shared label on its own misleads: `auto` is the most permissive setting Goose
has, and a mid-tier classifier for Claude Code that is *stricter* than
`bypassPermissions`.

| Agent | Launch flag alc passes | Changing it mid-session | Verified |
| --- | --- | --- | --- |
| claude | `--permission-mode plan\|manual\|acceptEdits\|auto\|bypassPermissions` | Shift+Tab cycling only (`ESC [ Z`) — relative, so the page offers "cycle", never a dropdown | ✅ against `claude --help` |
| codex | `-s read-only\|workspace-write\|danger-full-access` **and** `-a on-request\|never`, or `--approve-for-me` | `/permissions` opens Codex's own picker; a human finishes it in the terminal pane | ✅ against `codex --help` |
| opencode | `--agent plan\|build`, `--auto` | Tab toggles build ↔ plan | ✅ against `opencode --help` |
| goose | `GOOSE_MODE=chat\|approve\|smart_approve\|auto` | `/mode <name>` — sets it outright | from documentation |
| qwen | `--approval-mode plan\|default\|auto-edit\|auto\|yolo` | `/approval-mode <name>` | from documentation |
| kimi | `--plan`, `--yolo` | relaunch only | from documentation |
| copilot | `--mode plan\|interactive`, `--allow-all-tools` | `/permissions` opens its picker | from documentation |
| pi | — | **not supported.** Pi has no permission modes, no plan mode, no permission prompts and no sandbox, by design. The control is disabled with that sentence and the card carries a red badge. | — |

Two details worth knowing, because they are the kind that rot silently:

- Claude Code's CLI takes `manual`, and has no `default`. Its SDK control
  channel is the other way round. alc keeps both spellings rather than sharing
  one constant, because a single value would quietly break one of the paths.
- `--full-auto` appears in a lot of Codex documentation and does not exist in
  codex-cli 0.153.2. alc never emits it, and a test enforces that.

**alc only injects a permission flag for the agents it has confirmed against a
real `--help`.** For the rest it changes nothing unless you ask with
`--permission`. Guessing a flag name into an agent's arguments does not produce
a tightened session — it produces one that will not start.

alc also reports how much it trusts what it is showing: `launched` (alc passed
the flag and has sent nothing since), `reported` (read back off the agent's own
status line), or a `?` for a guess.

### Raising the ceiling

`max_permission` in `remote.toml` (default `auto-edit`) is the loosest mode the
page can reach on its own. Tightening is always free. Anything past the ceiling
returns a ticket instead:

```text
$ # the page shows: alc confirm 7QK2M9XB4T
$ alc confirm 7QK2M9XB4T
granted: auto
the page can apply it once, within the next minute.
```

`alc confirm` refuses to run without a terminal, so the confirmation has to
come from a person at the machine — not from whoever holds the link, and not
from the agent piping a command into a shell. Every rung past the ceiling is
gated **every time**, not only the first: alc's idea of the mode a session is
currently in is usually a belief rather than a fact, and a gate that depended
on that belief could be walked past.

## Session lifetime

Shared sessions are owned by a **hub** — a small background process alc starts
the first time you share something. That is what makes one page show every
session, and what lets a session outlive the terminal it was started from.

```sh
alc claude --share           # starts a hub if one is not already running
# ctrl-\ then d              # detach; the session keeps running
alc sessions                 # what is running
alc attach 7QK2             # back on it, from any terminal
alc kill 7QK2               # stop one
alc rename 7QK2 review      # rename its card
alc hub status              # is a hub running, and where is its page
alc hub stop [--drain]      # stop it; --drain stops its sessions too
```

Session ids look like `claude-7QK2M9XB4T`. Commands take any unambiguous
prefix, and the distinctive tail on its own works too — `alc attach 7QK2` is
enough. Matching is case-insensitive.

`alc hub stop` refuses while sessions are still running unless you pass
`--drain`, so stopping the hub is never an accidental way to kill work.

### What the hub does with your environment

The hub is long-lived and was started from whichever shell first ran
`alc --share`. Spawning agents into *its* directory with *its* environment
would mean a session started in one repository quietly editing another, so
each request carries the working directory and full environment of the shell
that made it. Two sessions started from two projects each get their own.

The launch's own variables still win over your shell's: an ambient
`ANTHROPIC_API_KEY` does not override the provider key alc resolved for that
profile.

### If the hub dies

A hub killed outright (`kill -9`, a reboot) takes its page with it, but on
unix the agents themselves are their own session leaders and keep running,
detached. alc writes a record per session so the next hub to start cleans up
what the last one could not — in particular the temporary file the Kimi
builder writes the provider key into, which would otherwise sit on disk.

The hub runs with no terminal and no log file: nothing it could write would be
worth the redaction rules a log holding launch environments would need. When
one will not start, run it in the foreground and watch:

```sh
alc hub start --foreground
```

## What this does not do

Stated plainly, because finding out later is worse:

- **Approval prompts arrive as terminal text, not as mobile dialogs.** You see
  the agent's own prompt and answer it with the key bar. Real Approve/Deny
  cards need a per-agent structured channel, and only half the agents have one.
- **Every operator link can type at once.** There is no "take control"
  arbitration — two people holding operator links interleave their keystrokes,
  the same as two people sharing a tmux pane. Hand out viewer links for
  anything you are not driving yourself.
- **A hub crash takes the page, not the agents.** They keep running detached;
  the next `alc` to start cleans up what the crashed one left. Sessions do not
  survive a reboot.
- **alc cannot attach to sessions it did not start.** A `claude` you launched
  by hand is invisible to the page; start it with `alc --share`.
- **Permission-mode state is believed, not known, for most agents.** Only three
  can be told a mode outright; the rest are cycled, hand off to their own
  picker, or cannot be changed at all. The `confidence` marker on every card
  says which case you are looking at.
- **Full-screen agents have no browser scrollback.** Codex, OpenCode and Qwen
  draw into the alternate screen, exactly as they do locally. `codex
  --no-alt-screen` and `opencode --mini` are dramatically better on a phone,
  and the page tells you so.

## Commands

```sh
alc --share <agent>          # mirror this session
alc share <agent> -- <args>  # the unambiguous form
alc share <agent> --name x   # name the card, instead of <agent>@<directory>
alc --no-share <agent>       # never mirror, whatever the settings say

alc remote status            # on/off, how it binds, where the files live
alc remote on
alc remote off
alc remote token --rotate    # invalidate every link handed out so far

alc --share --permission plan <agent>   # start in a mode
alc confirm <ticket>         # approve a change the page asked for
```

`--share` is alc's own flag, so it has to appear before the agent's arguments.
Put it after them and alc tells you so rather than passing it to the agent:

```text
$ alc claude "review this" --share
error: `--share` is alc's own flag but it came after the agent's arguments,
where it would be passed to claude instead; put it before the agent name, or
use `alc share claude -- <args>`
```

## Settings

`remote.toml` lives beside `config.toml` — `alc remote status` prints the
path. It is a separate file on purpose: `config.toml` refuses keys it does not
recognise, so putting these there would break every older `alc` reading the
same file.

| Key | Default | What it does |
| --- | --- | --- |
| `enabled` | `true` | Master switch. `alc remote off` sets this. |
| `bind` | `"loopback"` | `loopback` or `lan`. |
| `allow_lan` | `false` | Must be true *and* `--bind-lan` passed for a LAN bind. |
| `port` | `8787` | `0` picks an ephemeral port. A busy port falls back to one. |
| `allowed_origins` | `[]` | Extra origins, for a tunnel's hostname. |
| `extra_hosts` | `[]` | Extra `Host` values to answer to, port included. |
| `scrollback_bytes` | `1048576` | How far back a reconnecting viewer can be caught up exactly. |
| `max_connections` | `64` | Connections served at once. |
