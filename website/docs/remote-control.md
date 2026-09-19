---
id: remote-control
title: Remote control
sidebar_label: Remote control
sidebar_position: 5
description: Mirror any coding-agent session to a web page and drive it from a phone — the same page for every agent, with permission modes, tmux sizing and a security model that assumes the link is the credential.
keywords:
  - claude code on phone
  - remote coding agent
  - share terminal session
  - tmux
  - tailscale cloudflare tunnel
---

# Remote control

Mirror a running session to a web page and drive it from another device — the
same page for every agent and every provider. Your terminal keeps working;
sharing mirrors the session rather than taking it away.

```sh
alc --share claude
alc share opencode -- --mini
```

```text
alc session claude-7QK2M9XB4T (claude@all-code)
  open  http://127.0.0.1:8787/#k=…
  hub   127.0.0.1:8787 · loopback only (pid 48213) · this link grants input; keep it to yourself
  keys  ctrl-\ then d detaches; the session keeps running
```

Remote control works on Windows 10 and 11 as it does on macOS and Linux;
`--tmux` there needs the native Windows port of tmux, covered in
[Who owns the size](#who-owns-the-size).

## Reaching it from a phone

The default binds to loopback. Nothing is exposed until you say so.

**Tailscale** — alc stays on loopback and Tailscale does the exposing. HTTPS,
no third party in the middle.

```sh
alc remote allow-host box.tail1a2b.ts.net
tailscale serve 8787
alc claude --share
```

**Your own Wi-Fi** — nothing to install, but plain HTTP, so the token crosses
the network in clear. Fine at home; use a tunnel on café Wi-Fi.

```sh
alc claude --share --bind-lan
```

**Cloudflare Tunnel** — works from anywhere, including cellular. Cloudflare
terminates the TLS, so put Access in front of it if that matters. A quick
tunnel mints a new hostname every run, hence the wildcard.

```sh
alc remote allow-host '*.trycloudflare.com'
cloudflared tunnel --url http://127.0.0.1:8787
alc claude --share
```

alc answers only to names you allowed: it checks the `Host` header against a
list and allows an `Origin` whose host is on the same list. An attacker's page
can point `evil.com` at `127.0.0.1` and have your own browser drive your agent,
and the name the browser thinks it is talking to is the part it cannot forge.

```sh
alc remote allow-host box.tail1a2b.ts.net   # exact
alc remote allow-host '*.trycloudflare.com' # any subdomain
alc remote status                           # what it answers to now
```

## Finding the link again

The link scrolls away as soon as the agent draws its interface.

```sh
alc remote url        # just the link
alc sessions          # the link, then what is running
```

```text
page  http://192.168.1.42:8787/#k=…
      https://box.tail1a2b.ts.net/#k=…

claude-7QK2M9XB4T      claude   running   ask        ~/src/all-code
codex-68B8XMJ6F5       codex    running   plan       ~/src/api
```

Every allowed name gets a line. `alc remote token --rotate` invalidates every
link handed out so far.

## Sharing by default

```sh
alc remote auto-share on     # `alc claude` now behaves like `alc claude --share`
alc --no-share claude        # opt one launch out
```

It is also on the **Sharing & remote** screen of `alc config`. A scripted run —
one with redirected input or output — never shares whatever this is set to, so
a standing preference cannot make a cron job start failing. An explicit
`--share` there still fails loudly.

## What the link grants

A page that types into a coding agent is remote code execution on your machine.

- **The fragment is the credential.** Anyone with the `#k=…` part can type. A
  fragment never reaches a server, so it stays out of access logs and proxies,
  but it does land in your clipboard. Treat it like a password.
- **Host and Origin are checked**, down to the port, and tokens are compared in
  constant time.
- **A shared session is screen sharing.** alc masks the API keys it put into
  the environment, in every view including your own terminal. Anything else the
  agent prints, a viewer sees.
- **alc never answers a clipboard read**, so a hostile file an agent prints
  cannot pull what you last copied into the model's context.
- **alc reads no configuration from the working repository**, so a checked-in
  file can never turn sharing on.

## Permission modes

The eight agents disagree about what a permission mode is, what the modes are
called, and whether one can change after launch. The page renders what the
agent in front of it can actually do: a dropdown where a mode can be set
outright, a cycle button where it can only be cycled, and a disabled control
carrying the reason where the agent has no such concept. It shows alc's rung
and the agent's own word for it, because a shared label misleads — `auto` is
the most permissive setting Goose has and a mid-tier one for Claude Code.

| Agent | Flag alc passes | Changing it mid-session |
| --- | --- | --- |
| claude | `--permission-mode plan\|manual\|acceptEdits\|auto\|bypassPermissions` | Shift+Tab cycling only, so the page offers "cycle" |
| codex | `-s read-only\|workspace-write\|danger-full-access` and `-a on-request\|never` | `/permissions` opens Codex's own picker |
| opencode | `--agent plan\|build`, `--auto` | Tab toggles build ↔ plan |
| goose | `GOOSE_MODE=chat\|approve\|smart_approve\|auto` | `/mode <name>` |
| qwen | `--approval-mode plan\|default\|auto-edit\|auto\|yolo` | `/approval-mode <name>` |
| kimi | `--plan`, `--yolo` | relaunch only |
| copilot | `--mode plan\|interactive`, `--allow-all-tools` | `/permissions` opens its picker |
| pi | — | not supported: Pi has no permission modes or sandbox, by design |

alc injects a permission flag only for agents it has confirmed against a real
`--help`; for the rest it changes nothing unless you ask with `--permission`.
Each card says how much alc trusts what it shows: `launched`, `reported`, or a
`?` for a guess.

`max_permission` in `remote.toml` (default `auto-edit`) is the loosest mode the
page can reach on its own. Tightening is always free; anything looser returns a
ticket:

```text
$ alc confirm 7QK2M9XB4T
granted: auto
the page can apply it once, within the next minute.
```

`alc confirm` refuses to run without a terminal, so the confirmation comes from
a person at the machine rather than from whoever holds the link. Every rung
past the ceiling is gated every time, because alc's idea of the current mode is
a belief rather than a fact.

## Sessions and the hub

Shared sessions belong to a **hub**, a background process alc starts the first
time you share. That is what makes one page show every session and lets a
session outlive the terminal it started in.

```sh
alc claude --share           # starts a hub if one is not running
# ctrl-\ then d              # detach; the session keeps running
alc sessions                 # what is running
alc attach 7QK2              # back on it, from any terminal
alc kill 7QK2                # stop one
alc rename 7QK2 review       # rename its card
alc hub status
alc hub stop [--drain]       # refuses while sessions run unless --drain
```

Ids look like `claude-7QK2M9XB4T`; any unambiguous prefix or the tail alone
works, case-insensitively.

Each launch carries the working directory and environment of the shell that
asked for it, so a session started in one repository never edits another. If
the hub is killed outright the agents keep running detached, and the next hub
cleans up what the last one could not. It keeps no log file; when one will not
start, run `alc hub start --foreground` and watch.

## Who owns the size

A terminal has one size, and it belongs to the terminal you launched from. So
the page does not resize the agent: it draws the real grid as large as it fits,
centred, with black where the ratio does not match. Resize your terminal and
the page follows within a few seconds.

On a screen 900px or wider with a session open, the **Hide sessions** button
beside Back folds the session list away so the terminal gets the page's full
width, and **Show sessions** brings it back. A plain session is drawn larger; a
`--tmux` session (below) actually gets the extra columns, because the page owns
its size. The browser remembers the choice. There is no keyboard shortcut for
it on purpose: every key belongs to the terminal, and `ctrl-b` is tmux's
prefix. A phone already shows one pane at a time, so nothing changes there.

`--tmux` is for when the page is the side you will actually use. It runs the
agent inside tmux, so your terminal and the hub each attach as their own
client with their own size — and this time the page decides the agent's.

```sh
alc --share --tmux --codex claude    # or -t
```

| | Without `--tmux` | With `--tmux` |
| --- | --- | --- |
| Size | Your terminal's; the page scales it | The page's; your terminal shows what fits |
| Detach | `ctrl-\` then `d` | `ctrl-b` then `d` |
| Scrollback | The page's | The page's, plus tmux copy mode locally |
| Your terminal's view | Mirrored through the hub, keys masked | A direct tmux client, raw |

That last row matters: with `--tmux` your terminal shows what the agent
actually printed, including a key it echoes. The browser still sees those
masked.

Needs tmux 3.2 or newer and applies only to a shared session. alc starts its
own tmux server per session with no configuration file, so your own tmux is
never touched and an agent cannot reach a session's server through a
`~/.tmux.conf`. Keystrokes from the page go to the agent's pane directly, so a
viewer cannot reach tmux's command prompt; your own terminal is a full client
and can.

On Windows, `--tmux` needs the native Windows port of tmux (tested: tmux
3.6a-win32). The alc installer checks for it and tries to install it automatically;
see [installation and opt-outs](./getting-started.md#optional-tmux-setup). If setup
was skipped or failed, use this manual fallback, then open a new terminal so PATH
picks it up. The tmux row of `alc doctor` says whether it was found.

```powershell
winget install --id arndawg.tmux-windows --exact
```

psmux also installs a `tmux.exe`, but alc cannot drive it (it cannot run the
command sequence alc creates a session with), so alc looks past it on PATH for
the native port and both can stay installed. MSYS2, Cygwin and WSL builds of
tmux are not used on Windows either.

The Windows port passes command lines, environment and working directory
through the ANSI code page, so the pane runs a small alc launcher that collects
the agent's exact launch from the hub over loopback. Non-ASCII folder names,
arguments and environment values therefore work, and the provider API key never
enters tmux's own environment. If alc itself is installed under a path that is
not plain ASCII, it uses the Windows 8.3 short path; on a drive with short
names turned off, `--tmux` refuses and says to install alc under an ASCII path.

Stopping a `--tmux` session on Windows (`alc kill` or `alc hub stop --drain`)
ends its tmux server and the agent with it. Windows has no hangup signal, so the card says the session ended without
an exit status, where macOS and Linux show the signal.

## What this does not do

- **Approval prompts arrive as terminal text**, not as mobile dialogs.
- **Every operator link can type at once.** There is no take-control
  arbitration; hand out viewer links for anything you are not driving.
- **Sessions do not survive a reboot**, and alc cannot attach to a session it
  did not start.
- **Permission state is believed, not known, for most agents.** The confidence
  marker on each card says which case you are looking at.
- **Full-screen agents have no browser scrollback.** `codex --no-alt-screen`
  and `opencode --mini` are dramatically better on a phone, and the page says
  so.

## Commands

```sh
alc --share <agent>          # mirror this session
alc --share --tmux <agent>   # ...and let the page own the agent's size
alc share <agent> -- <args>  # the unambiguous form
alc share <agent> --name x   # name the card
alc --no-share <agent>       # never mirror, whatever the settings say

alc remote status
alc remote on | off
alc remote token --rotate
alc remote url
alc remote auto-share on
alc remote allow-host <host>

alc --share --permission plan <agent>
alc confirm <ticket>
```

`--share` is alc's own flag, so it comes before the agent's arguments; put it
after and alc says so rather than passing it on.

## Settings

`remote.toml` lives beside `config.toml`; `alc remote status` prints the path.
It is a separate file because `config.toml` refuses keys it does not recognise.

| Key | Default | What it does |
| --- | --- | --- |
| `enabled` | `true` | Master switch. `alc remote off` sets this. |
| `auto_share` | `false` | Share every session without `--share`. |
| `bind` | `"loopback"` | `loopback` or `lan`. |
| `port` | `8787` | `0` picks an ephemeral port; a busy one falls back. |
| `allowed_hosts` | `[]` | Names to answer to besides this machine's own. |
| `max_permission` | `"auto-edit"` | The loosest mode the page can reach alone. |
| `scrollback_bytes` | `1048576` | How far back a reconnecting viewer is caught up. |
| `max_connections` | `64` | Connections served at once. |
