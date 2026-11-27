---
id: remote-control
title: Remote control
sidebar_label: Remote control
sidebar_position: 5
description: Mirror any coding-agent session to its owner's web page and drive it from a phone, with permission modes, tmux sizing, cross-generation session discovery, and a link-as-credential security model.
keywords:
  - claude code on phone
  - remote coding agent
  - share terminal session
  - tmux
  - tailscale cloudflare tunnel
---

# Remote control

Mirror a running session to its owner's web page and drive it from another
device — the same interface for every agent and provider, with a separate page
per runtime owner. Your terminal keeps working; sharing mirrors the session
rather than taking it away.

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

![The remote page with a session open: the session rail, the terminal, the key bar and the composer](/img/remote/session.png)

## The page

The page has tabs — **Sessions**, **Usage**, **Studio**, **Settings**, and any a
[mod](#mods) adds. They sit in the header on a wide screen and become a bottom
bar on a phone, where an open session takes the whole screen.

- **Sessions** is the list, and beside it the open session: terminal, key bar
  (Esc, Tab, ⇧Tab, sticky Ctrl, arrows), a composer for long prompts, the
  [permission](#permission-modes) bar and the read-only file panel.
- **Usage** is a dashboard of what the agents on this machine have spent — see
  [Usage on the page](#usage-on-the-page).
- **Studio** arranges the page: layouts (a whole look at a time, and
  Default to come back), where each mod's widgets go, backgrounds, floating
  windows and panel widths — see
  [Mods and themes](./mods.md#the-studio).
- **Settings** is the page's look and the terminal's, saved on the hub — see
  [Settings and themes](#settings-and-themes).

`⌘K` on a Mac, or `Ctrl+K` anywhere outside the terminal, opens a command
palette: jump to a session or tab, switch theme or density, toggle the file
panel, or run a mod's command. The search button in the header opens it too.

![The command palette](/img/remote/palette.png)

![On a phone: the session list, usage, and an open session](/img/remote/phone.png)

## Usage on the page

![The Usage tab: tiles, the daily chart, hours, and models](/img/remote/usage.png)

The Usage tab draws the statistics `alc usage` prints, over the last 7, 30 or
90 days in your browser's timezone:

- tiles for tokens, estimated cost, output, cache-hit share and sessions, with
  a sparkline for the first two;
- a stacked daily chart of input, cache read, cache write and output — or cost
  — with the figures for a day on hover or tap;
- the hours of the day you work in, and models, agents and providers ranked;
- below them, each login's quota windows and the launch ledger, as before.

The hub reads alc's own `usage.jsonl` plus the Claude Code and Codex histories
on its machine, prices them offline from the bundled and cached price maps,
and keeps each answer for a minute. See [Usage](./usage.md#on-the-remote-control-page)
for what a view-only link sees.

## Settings and themes

![The Settings tab: theme cards, accent and density](/img/remote/settings.png)

Settings are saved to `web/settings.json` in the config directory, so every
device that opens the page gets the same look. A view-only link shows them
read-only.

- **Theme** — System (follows the device), Light, Dark, five built-ins
  (Midnight, Daylight, Fjord, Ember, Paper), or any file in `web/themes/`.
  A theme also recolours the terminal.
- **Accent** and **density** (comfortable or compact).
- **Terminal** — font size (or automatic by screen), font, line height,
  cursor style and blink, scrollback, with a live preview.

![The Fjord theme](/img/remote/theme-fjord.png)

A theme file lays colour tokens over the light or dark base and may set the
terminal's palette:

```json
{
  "name": "Tidepool",
  "mode": "dark",
  "tokens": { "--bg": "#081214", "--surface": "#0d1b1e", "--accent": "#2dd4bf" },
  "terminal": { "background": "#061012", "foreground": "#d6f1ec", "cursor": "#2dd4bf" }
}
```

`web/AGENTS.md` lists every token. Values are colours and lengths only — no
`url()`, quotes or semicolons — and alc drops anything else.

## Mods

![Mods in the Settings tab](/img/remote/mods.png)

A mod adds to the page: a tab, commands in the palette, buttons on the key bar,
chips on session cards, or styles. The idea is herdr-app's: the page's
behaviour is plain files an agent can write, next to a reference written for
the agent.

```text
<config dir>/web/
├── AGENTS.md          the reference; alc rewrites it to match the running version
├── settings.json
├── themes/<id>.json
├── mods/<id>/
│   ├── mod.json       {"name", "description", "version", "author"}
│   ├── mod.js         optional
│   └── mod.css        optional
└── examples/          never loaded; copy from here
```

[Mods and themes](./mods.md) is the full guide: writing one, the API,
publishing a mod as a GitHub repository, and installing someone else's:

```sh
alc remote web install someone/alc-mod-standup   # arrives off; read it, then:
alc remote web enable standup
```

`alc remote web` prints the folder. The quickest way to a mod is to ask the
agent you are already running — the Mods section of Settings has a prompt to
copy that starts "Read …/AGENTS.md, then make me an alc page mod that". Or copy
an example:

```sh
cp -R "$(alc remote web)/examples/mods/scratchpad" "$(alc remote web)/mods/"
alc remote web check
```

| Example | What it adds |
| --- | --- |
| `quick-prompts` | Continue, Run tests, Summarise and `/compact` on the key bar and in the palette |
| `scratchpad` | a **Notes** tab saved on the hub, with a button to send the notes to the open session |
| `session-age` | an hours chip on sessions running for over an hour |
| `themes/tidepool.json` | a teal dark theme |

A mod's script wraps itself in `alc.define`:

```js
alc.define('hello', (mod) => {
  mod.addPage({
    id: 'today',
    title: 'Today',
    icon: 'clock',
    render(root) {
      const running = alc.sessions().filter((card) => !card.exit).length;
      root.append(alc.h('div', { class: 'panel' }, `${running} sessions running`));
    },
  });
  mod.addKey({ label: 'Continue', run: () => alc.type('continue') && alc.key('\r') });
  mod.addSessionChip((card) => (card.viewers > 1 ? { text: 'shared', tone: 'accent' } : null));
});
```

![A mod's tab: the scratchpad example](/img/remote/mod-page.png)

`alc` also gives a mod `on(event)` for sessions, attach, detach, page, usage
and settings; `api('/api/…')` for the page's own JSON routes; `toast`, `type`,
`key`, `open`, `settings`, `theme`; and `mod.save(state)` to keep state in
`settings.json`. `web/AGENTS.md` is the full reference.

The page notices a saved file within about four seconds: settings, themes and
stylesheets apply in place, and a changed script brings up a **Reload** button,
because a script only runs under the hash the hub computed when it served the
page. Turning a mod on or off in Settings works the same way. A mod that throws
is marked **Error** in Settings with its message, and the rest of the page
carries on.

## Reaching it from a phone

The default binds to loopback. Nothing is exposed until you say so. With
multiple runtime owners, use the port from the intended owner's page URL for
Tailscale/Cloudflare rather than assuming it is always 8787. The commands below
use the default-port example. Existing hubs keep their launch-time allowlist;
let sessions finish before explicitly stopping/restarting that owner to pick
up new allowed names. A drain ends sessions and is not an update step.

**Tailscale** — alc stays on loopback and Tailscale does the exposing. HTTPS,
no third party in the middle.

```sh
alc remote allow-host box.tail1a2b.ts.net
tailscale serve 8787
alc --share claude
```

**Your own Wi-Fi** — nothing to install, but plain HTTP, so the token crosses
the network in clear. Fine at home; use a tunnel on café Wi-Fi.

```sh
alc --share --bind-lan claude
```

**Cloudflare Tunnel** — works from anywhere, including cellular. Cloudflare
terminates the TLS, so put Access in front of it if that matters. A quick
tunnel mints a new hostname every run, hence the wildcard.

```sh
alc remote allow-host '*.trycloudflare.com'
cloudflared tunnel --url http://127.0.0.1:8787
alc --share claude
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
alc remote url        # the selected runtime's link
alc sessions          # all owners, their links, then their sessions
```

```text
page  http://192.168.1.42:8787/#k=…
      https://box.tail1a2b.ts.net/#k=…

claude-7QK2M9XB4T      claude   running   ask        ~/src/all-code
codex-68B8XMJ6F5       codex    running   plan       ~/src/api
```

Every allowed name gets a line. `alc remote token --rotate` invalidates links
for the selected runtime, not every other owner's links.

## Sharing by default

```sh
alc remote auto-share on     # `alc claude` now behaves like `alc --share claude`
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
- **The file panel stays in the session's folder.** A link that can type can
  read files under the folder the agent started in — paths that climb out of it
  or symlinks that point out of it are refused, `.git` is not listed, files are
  capped at 512 KB, and the API keys alc injected are masked there too. A
  view-only link gets no files at all.
- **Mods are trusted code.** A mod runs in the page with the page's link, not
  in a sandbox — read one before you turn it on. The page runs only the exact
  bytes of each enabled `mod.js`: the hub hashes them into the
  content-security policy when it serves the page. A view-only link runs the
  same mods but cannot save or type through them, and downloads them too, so
  never put a secret in one.
- **alc never answers a clipboard read**, so a hostile file an agent prints
  cannot pull what you last copied into the model's context.
- **alc reads no configuration from the working repository**, so a checked-in
  file can never turn sharing on.

## Permission modes

The nine agents disagree about what a permission mode is, what the modes are
called, and whether one can change after launch. The page renders what the
agent in front of it can actually do: a dropdown where a mode can be set
outright, the current mode as a chip with a **Cycle** button where it can
only be cycled (tap the chip for the order the cycle goes in), and a mode
chip with no control where it is fixed at launch or the agent has no such
concept, with the reason beside it. It shows alc's rung
and the agent's own word for it, because a shared label misleads — `auto` is
the most permissive setting Goose has and a mid-tier one for Claude Code.

| Agent | Flag alc passes | Changing it mid-session |
| --- | --- | --- |
| claude | `--permission-mode plan\|manual\|acceptEdits\|auto\|bypassPermissions` | Shift+Tab cycling only, so the page offers "cycle" |
| codex | `-s read-only\|workspace-write\|danger-full-access` and `-a on-request\|never` | `/permissions` opens Codex's own picker |
| opencode | `default_agent` plan\|build (inline config), `--auto` | Tab toggles build ↔ plan |
| goose | `GOOSE_MODE=chat\|approve\|smart_approve\|auto` | `/mode <name>` |
| qwen | `--approval-mode plan\|default\|auto-edit\|auto\|yolo` | `/approval-mode <name>` |
| kimi | `--plan`, `--yolo` | relaunch only |
| copilot | `--mode plan\|interactive`, `--allow-all-tools` | `/permissions` opens its picker |
| pi | — | not supported: Pi has no permission modes or sandbox, by design |
| agy | `--mode plan`, `--mode accept-edits`, `--sandbox --dangerously-skip-permissions`, `--dangerously-skip-permissions` | relaunch only |

alc injects a permission flag only for agents it has confirmed against a real
`--help`; for the rest it changes nothing unless you ask with `--permission`.
agy is confirmed, but its own default already asks, so its `ask` rung adds no
flag: a shared agy with no `--permission` starts exactly as a bare `agy` would.
A shared `--dry-run` prints the flag a real share would add, on every agent.
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

Shared sessions belong to a **hub** in their runtime generation. A hub starts
when that generation first shares a session, which lets the session outlive the
terminal it started in. Each owner serves its own page; `alc sessions` aggregates
legacy and generation owners and prints each page URL.

```sh
alc --share claude            # starts this generation's hub if needed
# ctrl-\ then d              # detach; the session keeps running
alc sessions                 # all owners and their sessions
alc attach 7QK2              # back on it, from any terminal
alc kill 7QK2                # stop one at its actual owner
alc rename 7QK2 review       # rename its card at its owner
alc hub status
alc --runtime legacy hub stop --drain # explicitly end that owner's sessions
alc hub stop --all           # stop every owner and all of their sessions (-a)
```

Ids look like `claude-7QK2M9XB4T`; any unambiguous prefix or the tail alone
works, case-insensitively. `attach`, `kill`, and `rename` locate the actual owner
across namespaces. A prefix matching several owners is refused; use a full ID
or global `--runtime <id|legacy>` to target an owner.

New runtime files live under `<config>/run/g/<shortid>`; legacy keeps
`<config>/run`. Configuration, credentials, usage records, and `remote.toml`
sharing policy remain shared. Existing listeners keep their bound address/port;
new generations can choose an available port, so use the link for the intended
owner rather than assuming every page uses 8787. Updates do not restart or drain
old hubs. With several owners running, `hub stop` and `bridge stop` require an
explicit runtime target, or `--all` (`-a`), which stops every owner and ends
all of their sessions. `hub stop` refuses live sessions unless `--drain`;
a drain ends those sessions, and a bridge stop can interrupt in-flight requests.
Neither stop is a zero-downtime operation or an update prerequisite.

Each launch carries the working directory and environment of the shell that
asked for it. If a hub is killed outright, agents can remain detached;
replacement-host cleanup is confined to its own namespace after acquiring the
owner lock, not another generation's sessions. It keeps no log file; when one
will not start, run `alc hub start --foreground` and watch.

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

When that grid has to shrink to fit — a 120-column terminal on a phone, or a
narrow space beside the file panel — the page says so above the terminal and
offers two ways out: **Fit to this page**, which lets this page resize the
session (the button beside the file panel's does the same, and *Plain sessions*
in Settings or the Studio makes it the default), or the command
`alc share --tmux <agent>`, to copy. A fitted session goes back to its own
terminal's size as soon as that terminal is resized, or when the page lets go
or closes. Hide the note with its ×.

![A plain session on a phone, with the note that offers --tmux](/img/remote/size-note.png)

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
alc share <agent> --name x   # name the card (after `--` it is the agent's own)
alc --no-share <agent>       # never mirror, whatever the settings say

alc remote status
alc remote on | off
alc remote token --rotate
alc remote url
alc remote auto-share on
alc remote allow-host <host>
alc remote web               # the page's settings, themes and mods folder
alc remote web check         # validate it
alc remote web list          # mods (on/off) and themes
alc remote web catalog       # official mods, layouts, themes and templates
alc remote web add <id>      # install an official one
alc remote web new <id> --template widget        # start a mod from a template
alc remote web layout [list | apply <id> [--enable-mods] | reset | remove <id>]
alc remote web install <owner/repo|url|folder>   # mods arrive off
alc remote web enable|disable|remove <id>

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

The page's own look and mods live apart from `remote.toml`, in `web/` beside
it — see [Settings and themes](#settings-and-themes) and [Mods](#mods).
