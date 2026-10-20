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
login](./agents.md#on-the-codex-login)). agy's own Google login serves every
other agent too ([Antigravity](#antigravity)).

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

`antigravity` launches the Antigravity CLI on its own login and serves every
other agent, Claude Code included, through alc's bridge
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
([Antigravity CLI](./agents.md#antigravity-cli-agy)). The same login also serves
every other agent, through alc's bridge:

```sh
alc --agy claude                          # Claude Code, on claude-opus-5-5 at high effort
alc --agy claude --model gemini-3.8-flash --effort low
alc --agy opencode                        # or pi, kimi, codex, copilot, goose, qwen
alc --agy claude --context max            # 840K for Claude, 900K Argon, 1,048K Flash
alc --agy usage                           # what is left of the plan's windows
```

alc borrows the access token agy already stored — the Windows Credential
Manager entry `gemini:antigravity`, or the file
`~/.gemini/antigravity-cli/antigravity-oauth-token` — and keeps nothing else: it
never stores a refresh token, never writes agy's login, and never runs a Google
sign-in of its own. agy still owns the login: when the token is about to
expire, alc runs `agy --output-format json models` (which has agy renew it) and
reads the store again. A login found in both places is refused rather than
guessed at; sign out of agy and in again. To Google, alc presents itself as the
agy client — agy's User-Agent and client header — because the endpoint chooses
what an account may run by that identity.

Use of this optional integration remains subject to the account provider's
terms and restrictions. There is no compatibility or account-availability
guarantee.

### Models, effort and context

Five model families run on this login: `claude-opus-5-5`, `claude-sonnet-5-5`,
`gemini-4-argon` (Google AI Ultra accounts), `gemini-3.8-flash` and
`gpt-oss-120b`, each at `low`, `medium` or `high` effort (GPT-OSS at `medium`
only). The id Google receives is the two joined, such as
`gemini-3.8-flash-low`. `alc models --agy` lists them with their windows, and
marks an id agy reports that no family serves ` (not bridgeable)`: agy may run
it, the other agents cannot.

Claude Code switches model and effort per request as usual: its `/model`
picker lists the five families, its effort picks the variant, `opus` lands on
Claude Opus 5.5, `sonnet` on the session's model and `haiku` on Gemini 3.8
Flash. An effort above `high` runs as `high`, and one a family lacks as one it
serves, each with a one-line note. `--model`, `--effort` and `--context` change
one run of `alc --agy claude`; `--save` is refused with the `alc config upsert`
command that saves the same choice. The other agents run one model for the
whole session — the profile's model at the profile's effort — so choose it on
the profile:

```sh
alc config upsert antigravity --model gemini-3.8-flash-low --effort low --context max
```

Each family starts on a 256K window (GPT-OSS on 131K). `context = "max"` on the
profile, or `--context max` for one run of Claude Code, gives 840K for Claude,
900K for Argon and 1,048K for Flash. Codex CLI is handed the window as
`model_context_window` (older Codex versions also needed
`model_auto_compact_token_limit` set by hand in their own config), and the
effort as `model_reasoning_effort`, so its footer shows the effort the session
runs at. Codex's in-session `/model` picker still offers OpenAI's slugs, which
alc's bridge refuses on this login: change the model on the profile, or pass
one of this login's ids with `--model`.

### Signed reasoning, quota and platforms

Gemini signs its reasoning, and the signature travels with the conversation. A
Gemini tool round that lost its signature — a session started elsewhere, an
effort changed mid-task, or a bridge restarted — still goes up as the tool
calls and results it was: closed rounds unsigned, which Google serves, and the
round in progress with Google's documented placeholder signature
([Troubleshooting](./troubleshooting.md#gemini-on-antigravity-writes-tool-calls-as-text)).

`alc usage` shows what is left of the plan ([Antigravity
login](./usage.md#antigravity-login)), and `alc doctor` checks agy, the login,
its Google project and the profile. Reading the login is verified on Windows;
on macOS and Linux it is best effort, and the token file works on every OS.

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
