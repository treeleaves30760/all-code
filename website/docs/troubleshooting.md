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
Anthropic-compatible endpoint or a local server (`--kind ollama`, `llamacpp` or
`vllm`), or use [`alc --codex claude`](./codex-to-claude.md). [Provider
compatibility](./providers.md) has the matrix.

## `provider '…' is turned off`

The profile exists but is disabled — the starter `vllm` template ships that
way. Turn it on with `alc config upsert <profile> --enable`, adding
`--model <id>` if it has no model yet.

## `provider '…' has no API key`

Save one with `alc config key <profile>`, or set the variable named in the
profile's `api_key_env`.

## `Codex credentials were not found`

Run `codex login`, then retry. [`alc usage`](./usage.md) shows which logins are
current.

## Auto mode says server-side checks are unavailable

Upgrade alc and start a new session. No extra flag is needed; running background
sessions keep their launch settings. If the message remains, check whether your
own `--settings` overrides alc's defaults. [Auto mode and prompt
caching](./codex-to-claude.md#auto-mode-and-prompt-caching) explains the fallback:
Auto mode remains available, but its classifier calls use Codex quota.

## `CACHED` is zero or `—`

`0` is a real upstream count; `—` means alc cannot determine the value. Cache
reuse is automatic but best-effort, so misses are normal when the reusable prompt
prefix changes or expires. See [Usage](./usage.md#usage-by-provider-and-agent)
and [Auto mode and prompt caching](./codex-to-claude.md#auto-mode-and-prompt-caching).

## `API Error: Request timed out` (or `500`) with an Ollama profile

The model did not get through Claude Code's 25k–40k-token first request before
it gave up. alc sets `API_FORCE_IDLE_TIMEOUT=0`, `API_TIMEOUT_MS=1800000` and
`CLAUDE_STREAM_IDLE_TIMEOUT_MS=1800000` for Ollama, llama.cpp and vLLM profiles
so it waits; retries resume from Ollama's prompt cache, so
the session usually starts on the second attempt either way.

To make the first turn quick rather than merely survivable, see [Local
models](./local-models.md). Check the **Ollama** section of `alc doctor` first:
the model must be pulled, able to call tools, and have at least a 64k context.

## `500 System message must be at the beginning` from llama.cpp or vLLM

The model's chat template refused a system message that was not the first
one, which Claude Code sends to a model it does not recognise. alc turns that
off for `llamacpp` and `vllm` profiles; this appears when Claude Code runs on
such a server some other way, or on a `custom` profile. Use a `llamacpp` or
`vllm` profile, or set
`CLAUDE_CODE_MODEL_CAPABILITIES=-mid_conv_system,-mid_conv_tool_change` yourself.
[Local models](./local-models.md#why-two-more-settings) has the details.

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

## Your apiKeyHelper script is failing

Claude Code says this when `alc claude-credential` — the helper named in the
settings file alc wrote — exits without printing a credential. Four things stop
it:

- **No Codex login.** Run `codex login` and retry.
- **A key only one shell knew.** A profile whose key lives in an environment
  variable is readable by the sessions started from that shell and by nothing
  else, so a background session started later asks and gets nothing. Save it
  with `alc config key <profile>`.
- **The bridge will not start.** `alc bridge serve` runs it in this terminal
  and prints why it would not come up.
- **A route that is gone.** The bridge's record of that Codex profile was
  removed; one `alc --codex claude` writes it again.

`alc doctor` reports the login, the saved keys and the bridge in one pass. The
helper never falls back to your Claude login: a session alc set up either
reaches the provider you asked for or says it could not.

## A background session cannot connect after the bridge moved

A generation bridge's remembered port is part of its frozen settings/origin.
If an unrelated program occupies it while the host is down, alc fails rather
than rotating the token or rewriting that origin. Resolve the conflicting
listener; do not stop another active alc generation just to update. Initial
allocation can use an ephemeral port, and `alc sessions` shows separate owners
and their links.

Legacy keeps its older port-migration behavior. If a legacy bridge returns on a
new port, only its affected legacy sessions need to pick it up:

```sh
claude respawn <id>
```

An alc update itself neither moves old sessions nor requires their restart.

## Sessions I dispatched answer from Anthropic, not Codex

Agent view belongs to the Claude Code you opened it from. Dispatch from a plain
`claude agents` and you get plain Claude Code sessions on your Anthropic login,
whatever alc is doing in another terminal. Open agent view through alc instead:

```sh
alc --codex claude agents
```

Those sessions carry the settings file alc wrote, and keep it every time Claude
Code restarts them. [Background sessions](./background-sessions.md) has the
rest.

## Two notices on every Codex or local-server run

Every print-mode run on a Codex profile, or on an Ollama, llama.cpp or vLLM
one, writes two lines to stderr: one saying
`CLAUDE_CODE_DISABLE_1M_CONTEXT is set, but the 200K limit isn't enforced for <model>`,
and one reading `[claude-code:unrecognized_model]`. Both are Claude Code telling
you it does not recognise the model id alc gave it, which is the point: the
model is a Codex or local model. They are diagnostics, not errors; the session is
working.

Two documented cures exist and alc uses neither. A `modelOverrides` entry would
make Claude Code treat the Codex id as a Claude model for context budgeting,
undoing the real Codex window alc passes it as
`CLAUDE_CODE_MAX_CONTEXT_TOKENS`. `CLAUDE_CODE_AUTO_COMPACT_WINDOW` would
silence the first line by pinning the session to 200K, and then the status
line's percentage stops meaning anything.

## `/model` says `ANTHROPIC_MODEL` overrides my choice

Picking a model in an alc session answers with two lines, the second of them a
warning:

```text
Set model to GPT-6-Astra and saved as your default for new sessions
ANTHROPIC_MODEL is set to GPT-5.6-Sol — new sessions use that while it is set
```

Both are true. Your pick applies to the session you are in, and alc pins the
model for every session it starts, so a later `alc --codex claude` starts on
alc's model again rather than the one you chose. Change what alc starts on:

```sh
alc --codex claude --model gpt-6-astra
alc --codex claude --model gpt-6-astra --save
```

## Watching a background session from a script

`claude agents --json --all` carries both `state` and `status`, and the one
that says whether the work finished is `state`: `working` becomes `done`.
`status` only goes `busy` to `idle`.

A conversation you sent to the background with `←` ends at `state: "blocked"` —
"Needs input" on the card — rather than `done`, because it is a live session
waiting for your next turn.

## `alc update` cannot find the release archive

An installation from before 1.4.0 looks for a second binary that no longer
ships. Use a 2.0.1-or-newer installer for the stable-front migration; if Windows
has the old executable locked, defer or install side-by-side rather than
stopping active work. See [one-time migration](./getting-started.md#one-time-migration-from-older-alc).

## `alc tps` has no measured rows, or old rows are all `N/A`

Older v1/v2 turns, launches, native-history records, and cumulative checkpoints
have no observed request timing. They are not zero-speed requests, and alc
cannot reconstruct past TTFT/TPS. A reused older bridge may keep writing such
turns after the CLI was updated.

The default report selects timed `Request` records before sorting/limiting;
coverage counts explain excluded history. Inspect it with:

```sh
alc tps --include-unmeasured --source all --json
```

For future measurements, start a new session on the new generation; its host
must advertise `request-metrics-v3`. Existing older hosts/sessions can keep
running. Even measured failed/cancelled requests or requests without usage can
still have unavailable individual metrics; see [timing definitions](./usage.md#ttft-and-tokens-per-second).

## A Windows migration is pending, not complete

Old 2.0.0 self-update used an exit-time finalizer. The new payload cannot
retroactively change that old updater. Use a 2.0.1-or-newer installer to bootstrap
the stable front. A locked old entry point causes an explicit error with the
verified payload retained for retry; no new finalizer, process kill, or active
switch is performed. Wait until the old processes exit naturally, or choose
another `ALC_INSTALL_DIR` and invoke that side-by-side path explicitly. After
bootstrap, updates activate a manifest without replacing the front.

## A local update bundle fails verification

Keep the complete directory produced by `alc update --download-only BUNDLE_DIR`;
the destination must be new or empty. Keep separate bundles in separate directories.
`alc update --from BUNDLE_DIR --offline` makes no GitHub/network lookup and checks
bounded metadata, archive checksum, platform, and packaged binary version before
activation. Do not bypass a mismatch or run scripts from the bundle; obtain a
fresh verified bundle for this platform. A failed verification leaves the
active generation unchanged. Rollback requires a retained generation and does
not roll back config/credentials or restart hosts.

## A host stop or session prefix is ambiguous

`alc sessions` lists owners and their page URLs across legacy and generations.
Use a full session ID or global `--runtime <id|legacy>` for the intended owner.
When several owners run, `hub stop` and `bridge stop` require an explicit target.
Stopping a bridge can interrupt long requests; `hub stop --drain` ends its
sessions. Do not use either merely as an update step.

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
