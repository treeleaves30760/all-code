---
id: providers
title: Provider compatibility
sidebar_position: 5
description: Which LLM providers work with each of the nine coding agents, the sixteen built-in provider-kind presets and their default URLs, key envs and models.
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - provider presets
  - coding agent compatibility
---

# Provider compatibility

The nine agents do not all speak the same model protocol, so alc checks the
pair before launch instead of sending a request that cannot work.

| Agent | Accepts | Protocol |
| --- | --- | --- |
| Claude Code | An Anthropic-compatible endpoint | `anthropic-messages` |
| Codex CLI | The OpenAI Responses API | `openai-responses` |
| OpenCode | Any API-compatible provider | any |
| Pi | Anthropic, OpenAI, or OpenAI-compatible | any |
| Copilot CLI | OpenAI- or Anthropic-compatible | `openai-chat`, `anthropic-messages` |
| Goose | OpenAI- or Anthropic-compatible | `openai-chat`, `anthropic-messages` |
| Qwen Code | OpenAI-, Anthropic-, or Gemini-compatible | `openai-chat`, `anthropic-messages` |
| Kimi Code CLI | OpenAI- or Anthropic-compatible | `openai-chat`, `anthropic-messages` |
| Antigravity CLI | Its own Google login; the Codex login or any API through alc's bridge | `antigravity-native` |

Whatever an agent accepts natively, it also runs on one `codex login` through
the [Codex bridge](./codex-to-claude.md). The Antigravity CLI runs there too,
still signed in to Google for its own models ([On the Codex
login](./agents.md#on-the-codex-login)). agy's own Google login is for agy
alone; other agents can hand tasks to agy instead ([Antigravity](#antigravity)).

## Any provider, any agent

A pair the table rules out still runs: alc starts its bridge in-process for the
session and translates each turn to the provider's own wire. Claude Code on
OpenAI, Groq, xAI or Gemini, Codex CLI on a Chat Completions or Anthropic-only
provider, and agy on any provider with an API all launch this way.

| Profile offers | The bridge speaks |
| --- | --- |
| `openai-responses` | OpenAI Responses (`…/responses`) |
| `openai-chat` or `dual` | Chat Completions (`…/chat/completions`) |
| `anthropic-messages` only | Anthropic Messages (`…/v1/messages`) |

The bridge signs with the profile's key, which never reaches the agent, and
retries once without a field the provider refuses by name (`max_tokens` on a
reasoning model, say). Tool calls and their results travel as tool calls;
reasoning a provider streams is shown, never replayed. Claude Code on this path
runs every model alias on the profile's model, and its [background
sessions](./background-sessions.md) end with the alc that started them. agy
lists the profile's model in its `/model` picker, tagged with the profile name,
and still needs its own Google sign-in to start.

`alc doctor` prints this matrix resolved against your own profiles.

## Presets

Choosing a `--kind` fills in an endpoint, a key variable and a starting model.
Every value is a plain field in `config.toml` that `alc config upsert`
overrides.

| Kind | Default endpoint | Key env | Starting model |
| --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `sonnet` |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-6.1-sol` |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4.6` |
| `codex` | — (native `codex login`) | — | — |
| `antigravity` | — (native `agy` login) | — | `claude-opus-5-5-high` |
| `ollama` | `http://localhost:11434` | — | `qwen3-coder` |
| `vllm` | `http://localhost:8000/v1` | — | — (ships disabled) |
| `llamacpp` | `http://localhost:8080/v1` | `LLAMA_API_KEY` | — (the name the server lists) |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-v4-pro` |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3` |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | `MiniMax-M3` |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-build-0.1` |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-3.7-flash` |
| `custom` | — (you provide it) | — (name it with `--api-key-env`) | — |

New `openai` profiles also start with `low` reasoning effort. Existing profiles
keep their saved model and effort.

`antigravity` launches the Antigravity CLI on its own login, and only the
Antigravity CLI: every other agent is refused on it
([Antigravity](#antigravity)). A `config.toml` written before it existed lacks
the profile; `alc config upsert antigravity --kind antigravity` adds it.

Model IDs drift faster than alc releases, so treat every starting model as a
value to edit rather than a promise about what the provider serves today.

## Claude-ready without extra configuration

DeepSeek, Moonshot, Z.ai and MiniMax each publish a second,
Anthropic-compatible base URL beside their OpenAI-shaped one, so Claude Code
runs on them directly:

| Kind | Anthropic-compatible URL |
| --- | --- |
| `deepseek` | `https://api.deepseek.com/anthropic` |
| `moonshot` | `https://api.moonshot.ai/anthropic` |
| `zai` | `https://api.z.ai/api/anthropic` |
| `minimax` | `https://api.minimax.io/anthropic` |

## Claude Code on a local server

Ollama, llama.cpp and vLLM each answer Anthropic Messages at their root, beside
the OpenAI routes, so Claude Code runs on all three whichever OpenAI protocol a
profile names for the other agents. [Local models](./local-models.md) covers
what alc sets for each and how to keep the first turn short.

## Antigravity

`alc agy` launches Google's Antigravity CLI on its own Google login
([Antigravity CLI](./agents.md#antigravity-cli-agy)). That login is agy's
alone. Antigravity's [terms](https://antigravity.google/terms) (clause 6) say
that "using third party software, tools, or services to access the Service
(e.g. using OpenClaw with Antigravity OAuth) is a breach of this Agreement",
and Google has suspended accounts for it. Older alc releases ran every other
agent on that login by reading the token agy stores and presenting themselves
to Google as agy; that route is gone. alc no longer reads agy's login at all.

Any other agent on the `antigravity` profile — `alc --agy claude`,
`alc --agy opencode`, `alc -p antigravity pi` — is refused before launch:

```text
error: provider 'antigravity' is agy's own Google login, which only the Antigravity CLI may use: Antigravity's terms forbid other tools from signing in with it, and alc no longer routes claude through it. Run agy itself with `alc agy`, or use `alc claude --with-agy` (Claude Code on its own provider, with agy as a tool it can hand tasks to)
```

For an agent other than Claude Code the last part reads "or use <agent> on
another provider, with agy added as an MCP server: `alc mcp agy` (stdio)". A
`[defaults]` entry that points another agent at the antigravity profile no
longer fails config validation; the launch that would use it reports the error
above. A background bridge route an older alc wrote for Antigravity no longer
parses and is refused; start the session again on another provider.

`alc usage` lists the antigravity profile with the note "agy's own login; see
its quota inside agy", and `alc doctor` checks agy's binary, the profile and its
models.

### Handing tasks to agy

`alc mcp agy` is a stdio MCP server with one tool, `agy` (`mcp__agy__agy` in
Claude Code). Each call runs your own, unmodified
`agy --print <prompt> --output-format stream-json` in the server's working
directory — the calling agent's project — on agy's own sign-in, and returns
agy's final answer with a `conversation_id`. alc handles no Google credential
on this path.

`alc claude --with-agy` wires it into Claude Code for one session. Claude Code
runs on the provider `alc claude` would use, and alc appends an
`--mcp-config` naming this alc and its config directory:

```sh
alc claude --with-agy
alc --codex claude --with-agy
```

It is refused when `agy` is not on `PATH`, and together with Claude
subcommands that run directly, such as `alc claude mcp ...`. Any other MCP
client adds the server itself:

```sh
claude mcp add agy -- alc mcp agy
```

```json
{"mcpServers": {"agy": {"command": "alc", "args": ["mcp", "agy"]}}}
```

| Argument | Meaning |
| --- | --- |
| `prompt` | Required. The task, complete on its own: agy sees nothing of the calling conversation. |
| `mode` | `plan`, `accept-edits` or `auto`. `plan` reads and plans without editing. Omitted, agy uses its own default, which edits files. |
| `model` | An agy model id, as `agy models` lists them, such as `gemini-3.8-flash-low`. Omitted, the antigravity profile's model; a bare family id becomes its wire id for the effort. |
| `effort` | `low`, `medium`, `high`, `xhigh` or `max`. Omitted, the antigravity profile's; `ultra` runs as `max`. |
| `conversation_id` | Continue an earlier agy conversation (passed as `agy --conversation`). |

Calls run concurrently. Progress notifications report agy's steps
(`agy: write_file`), and a `notifications/cancelled` stops that agy run. If agy
is not installed, the call returns an error saying to install it and sign in
with `agy`. Usage of these agy runs is not recorded in alc's usage ledger.

Headless agy behaves differently from an interactive session (verified on agy
1.5.18). In print mode it writes files in the workspace without asking. It
cannot ask before a shell command, so it denies the command, and the tool
result then ends with "agy was denied: Bash. Headless agy cannot ask for
permission; the user can allow it under permissions.allow in agy's
settings.json". alc never passes `--dangerously-skip-permissions`.

### agy on the Codex login

`alc --codex agy` points agy's `CLOUD_CODE_URL` at alc's bridge, which adds the
Codex models to agy's picker and relays agy's own traffic to Google untouched,
with the token agy itself sent ([On the Codex
login](./agents.md#on-the-codex-login)).

:::caution

This changes the official client's endpoint, which Antigravity does not
sanction, and so does agy on any provider other than its own login, which
reaches alc's bridge the same way. Use it at your own risk.

:::

## Auto mode on every provider

Claude Code's auto mode checks each tool call with a classifier. Anthropic runs
that check on its own servers, which no other provider can, so on every other
provider alc has Claude Code run its local classifier on the profile's model:
it pins `ANTHROPIC_DEFAULT_SONNET_MODEL` — the `sonnet` alias the classifier
uses — to that model and sets `CLAUDE_CODE_AUTO_MODE_SERVER=0` to skip the
server attempt. Auto mode then works on OpenRouter, DeepSeek, a custom endpoint
or anything else Claude Code can reach, and its classifier calls spend that
provider's quota. `/model sonnet` names the profile's model too.

Four cases differ:

- The `anthropic` kind — your own Claude login or API key — keeps Anthropic's
  server-side check and its alias table.
- A profile whose model is itself a Claude Code alias, such as `sonnet` or
  `opus`, is not pinned: Claude Code would send that alias verbatim as the
  model's name. The server attempt is still skipped.
- A profile whose model is a Claude id in full, such as `claude-opus-5`, is not
  pinned either: its endpoint is a gateway in front of Anthropic (LiteLLM, a
  corporate proxy) that already serves `claude-sonnet-*` by name, so `/model
  sonnet` and every subagent declared `model: sonnet` keep Claude Sonnet. The
  server attempt is still skipped. A provider's own spelling, such as
  OpenRouter's `anthropic/claude-sonnet-5`, is not a Claude id in full and is
  pinned.
- A `--settings` of your own that names `ANTHROPIC_DEFAULT_SONNET_MODEL` still
  wins over alc's. One set in `~/.claude/settings.json` does not: alc's
  settings take precedence over it.

The [Codex bridge](./codex-to-claude.md#auto-mode-and-prompt-caching) and local
servers already worked this way.
