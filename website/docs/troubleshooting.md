---
id: troubleshooting
title: Troubleshooting
sidebar_label: Troubleshooting
sidebar_position: 8
description: The errors alc prints, and what clears each one — missing agents, incompatible providers, Codex logins, Ollama timeouts, and a GPT model left in Claude Code's settings.
keywords:
  - alc doctor
  - claude code error
  - codex login
  - ollama timeout
  - model not found
---

# Troubleshooting

Start with `alc doctor`. It reports every agent binary, credential status,
provider profiles with a per-agent compatibility column, the resolved defaults,
and the Codex login state.

## `'claude' is not installed or not on PATH`

alc launches agents that already exist. Install the agent, or point alc at a
binary with the [override variables](./agents.md#binary-overrides).

## `provider '…' cannot be used with claude; Claude Code needs Anthropic Messages`

That profile speaks a protocol Claude Code cannot use. Pick an
Anthropic-compatible endpoint, or use [`alc --codex
claude`](./codex-to-claude.md). [Provider compatibility](./providers.md) has
the matrix.

## `provider '…' has no API key`

Save one with `alc config key <profile>`, or set the variable named in the
profile's `api_key_env`.

## `Codex credentials were not found`

Run `codex login`, then retry. [`alc usage`](./usage.md) shows which logins are
current.

## `API Error: Request timed out` (or `500`) with an Ollama profile

The model did not get through Claude Code's 25k–40k-token first request before
it gave up. alc sets `API_FORCE_IDLE_TIMEOUT=0` and `API_TIMEOUT_MS=1800000`
for Ollama profiles so it waits; retries resume from Ollama's prompt cache, so
the session usually starts on the second attempt either way.

To make the first turn quick rather than merely survivable, see [Local
models](./local-models.md). Check the **Ollama** section of `alc doctor` first:
the model must be pulled, able to call tools, and have at least a 64k context.

## `404 model 'claude-…' not found` from Ollama

Claude Code asked the server for one of its own model IDs, usually through the
`haiku` alias it uses for background work. alc pins every alias to the
profile's model for Ollama profiles; set the profile's `small_model` to a model
you have actually pulled.

## The model list looks out of date

The catalog syncs from your ChatGPT account once a day, and again as soon as
Codex is upgraded. Sync it now:

```sh
alc models --refresh
```

`alc models` says where the list it printed came from. A line reading
`fallback:` means the account could not be asked and the installed Codex CLI
answered instead — an older Codex is shown fewer models than your account can
actually drive, so that is the line to read first. A model marked as coming
`from the catalog alc ships` is one the answering source did not report and alc
put back: the models alc bundles are a floor, so a source that answers short
costs freshness and never a model.

## `the model may not exist or you may not have access to it`

Two things cause this.

**A GPT model left in Claude Code's settings.** The model a session settles on
is saved to `~/.claude/settings.json` as your default for new sessions, and a
plain `claude` afterwards has no adapter in front of it. alc puts that key back
when a bridged session exits, so one you still find is a leftover from a
session that was killed outright or a value set by hand.

```sh
alc doctor            # names the file and the line when it finds one
alc --codex claude    # clears it on exit; alc passes the model itself
```

**A hub still running an older build.** A hub outlives terminals by design, so
it also outlives an upgrade, and alc refuses to hand a bridged session to a hub
of another version. `alc doctor` names one that is behind:

```sh
alc hub stop
```

## `alc update` cannot find the release archive

An installation from before 1.4.0 looks for a second binary that no longer
ships. Run the installer again; it replaces the whole installation.

## I cannot find the sharing setting in `alc config`

It is the third screen — `Tab`, `Shift+Tab` or the number key moves between
`1 Providers`, `2 Agent defaults` and `3 Sharing & remote`. Share-by-default,
the bind address and the permission ceiling are all there.

Outside the TUI, `alc remote auto-share on` sets the same thing and `alc remote
status` reports it. A row reading `on (inactive)` means sharing itself is off:
run `alc remote on`.

## `--tmux` on Windows finds no tmux, or the wrong one

On Windows, `--tmux` needs the native Windows port of tmux. Install it, then
open a new terminal so PATH picks it up:

```powershell
winget install arndawg.tmux-windows
```

psmux also installs a `tmux.exe`, but alc cannot drive it — it cannot run the
command sequence alc creates a session with. The two can be installed side by
side; alc looks past psmux on PATH for the native port. MSYS2, Cygwin and WSL
builds of tmux are not used on Windows either. The **tmux** row of `alc doctor`
says whether the native port was found. If you would rather not install it,
drop `--tmux`; [remote control](./remote-control.md) works without it.

## `alc is installed at …, and tmux for Windows cannot start a program whose path is not plain ASCII`

The Windows port of tmux passes command lines through the ANSI code page, so it
cannot start alc from a folder whose path is not plain ASCII. alc falls back to
the Windows 8.3 short path when there is one, but this drive has short names
turned off. Install alc under an ASCII path, or drop `--tmux`.

Only alc's own path is affected: non-ASCII project folders, arguments and
environment values work under `--tmux`.

## Secrets in output

`alc --dry-run` redacts API keys and auth tokens, `alc config show` prints only
whether a profile has a key, and `alc usage` never prints a credential of any
kind.
