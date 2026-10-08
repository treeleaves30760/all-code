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

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

### macOS

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

### Linux / WSL

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

The installer puts `alc` in `~/.local/bin` (Windows: `%USERPROFILE%\.local\bin`)
and adds that directory to your user PATH when it can; when it cannot, it
prints the directory to add. On macOS/Linux, restart the terminal or source the
profile it names; PowerShell updates both User PATH and the current session.
`ALC_INSTALL_DIR` chooses another directory: custom directories are not added
to PATH on macOS/Linux, but are added on Windows. Windows PowerShell 5.1 and
PowerShell 7 are supported, including 32-bit PowerShell on 64-bit Windows.

The installer verifies the payload and runs alc's internal install transaction.
`alc` stays a full-binary stable entry point; adjacent `.alc/active.json` selects
`.alc/generations/<digest>/alc` (`alc.exe` on Windows). After initial migration,
future activation changes the manifest, not a locked Windows front. Keep the
adjacent `.alc` directory with the entry point.

### Optional tmux setup

After alc is downloaded, SHA-256 verified, and installed, the installer checks
`tmux -V` for **3.2 or newer**. Compatible versions are left alone. Otherwise it
tries to install or upgrade tmux using an existing system package manager:

- **Windows:** WinGet, `arndawg.tmux-windows`, user scope, with no forced CPU
  architecture. The automatic flow accepts package/source agreements and
  disables interaction. psmux and non-native ports are skipped; an old or
  unparseable first native port on PATH still blocks later versions.
- **macOS:** `brew install tmux`, or `brew upgrade tmux` when already installed;
  Homebrew is never run with sudo.
- **Linux / WSL:** the first available `apt-get`, `dnf`, `yum`, `pacman`,
  `zypper`, or `apk`. Non-root installs use cached sudo credentials, or prompt
  only through a controlling terminal when stdout or stderr is a terminal.
  Package commands use noninteractive sudo and never read the piped script.
  pacman does not separately refresh indexes, avoiding a partial upgrade.

**Only `--tmux` needs tmux; ordinary alc and plain `--share` do not.** Missing
managers, unavailable privileges, unsupported packages/architectures, failed
installs, or an old PATH entry hiding the new version produce warnings and
manual instructions, without failing the alc installation. No Homebrew/WinGet
bootstrap, source build, psmux removal, or tmux configuration changes are made.
The version is checked again rather than assuming the package operation worked.

PowerShell appends new User/Machine PATH entries without replacing session-only
paths. If tmux is still missing, restart the terminal and check `tmux -V` and
`alc doctor`. Manual fallbacks (choose your platform):

```powershell
winget install --id arndawg.tmux-windows --exact
```

```sh
brew install tmux                                      # macOS (upgrade if already installed)
sudo apt-get update && sudo apt-get install -y tmux     # Debian / Ubuntu / WSL
sudo dnf install -y tmux                               # Fedora / RHEL (or yum)
sudo pacman -S --needed tmux                           # Arch; keep the system fully updated
sudo zypper install tmux                               # openSUSE
sudo apk add --upgrade tmux                            # Alpine
```

### Installer opt-outs

`ALC_NO_TMUX_INSTALL=1` skips automatic tmux setup. Independently,
`ALC_NO_PATH_UPDATE=1` disables **alc's own** PATH edits and session PATH refresh;
open a new terminal for package-manager PATH changes. WinGet can still modify
persistent PATH itself. Set **both** to avoid dependency-install side effects as
well as installer PATH edits:

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | ALC_NO_TMUX_INSTALL=1 ALC_NO_PATH_UPDATE=1 sh
```

```powershell
$env:ALC_NO_TMUX_INSTALL = '1'
$env:ALC_NO_PATH_UPDATE = '1'
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
# Optional: clear these overrides for future installs in this session.
Remove-Item Env:ALC_NO_TMUX_INSTALL, Env:ALC_NO_PATH_UPDATE
```

From source, with Rust 1.88 or newer: `cargo build --release --locked`. The
Codex bridge is part of the binary, so nothing else is needed for the bridge.
Install tmux separately if you want `--tmux` with a source build.

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

Claude Code starts on `gpt-6.1-sol` at `low` effort — or on whatever your
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
alc --llamacpp claude
alc -p local-vllm opencode
```

`--provider` (`-p`) takes a profile name, or a kind when only one profile of
that kind exists; `--anthropic`, `--openai`, `--openrouter`, `--codex`,
`--ollama`, `--vllm`, `--llamacpp`, `--deepseek`, `--moonshot`, `--zai`,
`--minimax`, `--groq`, `--xai`, and `--google` are shortcuts. Keys are saved locally or read
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

alc's own flags — `--metrics`, `--runtime`, `--share`, `--no-share`, `--bind-lan`,
`--name`, `--permission`, `--tmux`, `-t` — go before the agent name. After it, alc stops
rather than passing them to the agent, unless you put `--` straight after the
agent name, which says you meant the agent's flag:

```text
error: `--share` is alc's own flag but it came after the agent's arguments, where it would be passed to claude instead; put it before the agent name, or put the agent's own flags after `--`, as in `alc claude -- <args>`
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
alc update --download-only "$HOME/alc-bundle"
alc update --from "$HOME/alc-bundle" --offline
alc update --rollback previous --offline
```

`alc update` selects the release for this OS and CPU, verifies the published
SHA-256 and packaged binary version, publishes an immutable generation, then
atomically activates it. The full-binary stable front reads adjacent
`.alc/active.json` and dispatches new invocations to
`.alc/generations/<digest>/alc[.exe]`. A running generation never follows the
new active selection. Helpers and daemons use hash-pinned executable paths.

Existing foreground/background/shared sessions and old hosts are not restarted
or drained. New launches use their generation's hub/bridge. Runtime files are
namespaced under `<config>/run/g/<shortid>`; legacy keeps `<config>/run`.
Real provider configuration, credentials, sharing policy, and the usage ledger
stay shared. Old generations remain available; there is no automatic garbage
collection. Do not delete one that a session or helper still uses.

| Option | What it does |
| --- | --- |
| `--check` | Check online without applying. |
| `--force` | Reinstall the selected release even when its version is current. |
| `--download-only BUNDLE_DIR` | Save a verified bundle without applying, even if the latest version is already installed. |
| `--from BUNDLE_DIR --offline` | Verify and apply the complete local bundle with **no GitHub/network lookup**; check bounded metadata, archive checksum, platform, and packaged binary version before activation. |
| `--rollback previous` or `--rollback <digest>` | Activate a retained generation, without reverting config, credentials, or usage records. `--offline` is allowed. |

Download-only never activates; its destination must be new or empty. Keep each
bundle in its own directory. Bundle/verification failures do not switch the
active generation. Rollback keeps the other generations and does not restart
hosts. Self-update changes only alc: **`alc --codex update` is still alc
self-update, not a Codex CLI update**. No agent, tmux, or PATH update is performed.
External package updates or shared-auth rotation are outside this guarantee;
older hosts and external Codex do not cooperate with the new alc refresh lock.

### Managing older owners

`alc sessions` aggregates legacy and generation owners, with a browser URL per
owner. `attach`, `kill`, and `rename` find the actual owner across them; ambiguous
ID prefixes are refused. `--runtime <id|legacy>` is a global owner selector:

```sh
alc sessions
alc attach 7QK2
alc --runtime legacy bridge stop
alc --runtime legacy hub stop --drain
```

When multiple owners run, `hub stop` and `bridge stop` require an explicit
runtime target. A bridge stop can interrupt long requests; a hub drain ends
that owner's sessions. Neither is a zero-downtime operation or an update step.
Each owner's page lists only its own sessions, not an aggregate browser view.

### One-time migration from older alc

An old 2.0.0 updater, particularly its Windows exit-time finalizer, cannot be
retroactively fixed by a downloaded 2.0.1 payload. Use a 2.0.1-or-newer installer to
bootstrap the stable front. On Unix that one-time operation can atomically
replace the old front. If Windows has it locked, the new transaction exits with
an error and retains the verified payload for retry; it does not activate, kill
user processes, or schedule a new finalizer. Subsequent manifest activation
does not overwrite the front.

Wait for old processes to exit naturally and retry, or set a different
`ALC_INSTALL_DIR` for a side-by-side installation and invoke that path explicitly.
Pending is not complete; do not stop active work merely to satisfy an update.

## Uninstall

After sessions/helpers using retained generations are done, delete `alc` and
its adjacent `.alc` install directory. `alc config path` names the configuration
directory; removing it also deletes locally saved API keys.
