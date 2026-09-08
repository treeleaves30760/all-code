# The Codex bridge alc needs

Scope for replacing the vendored `claude-codex` dependency with an
implementation alc owns. Everything here was measured or captured against a
running bridge on 2026-09-08, not inferred.

## Why do this at all

`gpt-6-astra` could not be used through the bridge. The fix turned out to be
two lines — the slug missing from two hard-coded allowlists
(`registry.rs::CODEX_MODELS` and
`providers/codex/translate/model_allowlist.rs::ALLOWED_MODELS`) — and once
added, non-streaming, streaming and tool-use all worked unchanged against
GPT-6.

That is the argument for owning this layer, and also the warning attached to
it: the translation was already correct, and only a list was stale. Whatever
replaces it has to be at least as correct on everything that was *not* stale.

## What alc actually uses

Three wire formats, because the eight agents disagree:

| Surface | Endpoint | Agents |
| --- | --- | --- |
| Anthropic Messages | `POST /v1/messages`, `POST /v1/messages/count_tokens` | claude |
| OpenAI Responses | `POST /v1/responses` | opencode, pi, kimi |
| OpenAI Chat Completions | `POST /v1/chat/completions` | copilot, goose, qwen |

`/v1/responses` and `/v1/chat/completions` are only routed when
`CCP_CODEX_RESPONSES_API=1`, which is exactly what alc sets for every
non-Messages plan (`launch.rs::bridge_child_env`). A `/healthz` returning 200
is required — alc polls it to decide the bridge is up.

Claude Code additionally switches model and reasoning effort *per request*, so
the Messages surface must honour the model in the body rather than a pinned
one. Every other agent picks one at launch.

## Upstream contract

`POST https://chatgpt.com/backend-api/codex/responses`, authenticated from
`~/.codex/auth.json` (OAuth access token, refreshed against
`https://auth.openai.com`; the account id comes from the `chatgpt_account_id`
claim inside the JWT).

A captured request, verbatim, for a streaming Messages call carrying one tool:

```json
{
  "model": "gpt-5.6-terra",
  "type": "response.create",
  "store": false,
  "parallel_tool_calls": false,
  "reasoning": { "context": "all_turns" },
  "text": { "verbosity": "low" },
  "client_metadata": {
    "ws_request_header_x_openai_internal_codex_responses_lite": "true"
  },
  "input": [
    { "role": "developer", "type": "additional_tools",
      "tools": [ { "type": "function", "name": "read_file",
                   "description": "Read a file", "strict": false,
                   "parameters": { "type": "object",
                                   "properties": { "path": { "type": "string" } },
                                   "required": ["path"] } } ] },
    { "role": "developer", "type": "message",
      "content": [ { "type": "input_text", "text": "Be terse." } ] },
    { "role": "user", "type": "message",
      "content": [ { "type": "input_text", "text": "Read /etc/hosts using the tool." } ] }
  ]
}
```

Note the shapes that are not obvious: tools ride as a `developer` input item of
type `additional_tools` rather than a top-level `tools` array; the system
prompt is a `developer` message; `store` must be `false` and `input` must be a
list (the server rejects both otherwise, verified).

### Response events

The upstream answers Server-Sent Events. Every type observed in the capture:

```
response.created            response.output_item.added
response.in_progress        response.output_item.done
response.content_part.added response.content_part.done
response.output_text.delta  response.output_text.done
response.function_call_arguments.delta
response.function_call_arguments.done
response.completed          codex.response.metadata
responsesapi.websocket_timing
```

Upstream also offers a WebSocket transport (`CCP_CODEX_TRANSPORT=auto` falls
back to HTTP when the upgrade is refused). SSE alone is sufficient for a first
version; the WebSocket lane is an optimisation.

## What we do not need

The vendored crate is 68,722 lines because it serves far more than alc does.
Excluded outright: the kimi, grok, cursor and anthropic providers (15,783
lines), the terminal monitor UI, image generation, audio transcription, and
Anthropic-name aliasing (`sonnet` → `gpt-5.6-terra`) — alc always sends a real
Codex slug.

What remains as reference, by measured size:

| Area | Lines |
| --- | --- |
| `providers/codex/*.rs` | 17,870 |
| `providers/codex/translate/` | 7,845 |
| `providers/codex/chat_completions/` | 1,572 |
| `providers/codex/auth/` | 953 |
| `openai_compat/` | 3,038 |
| `server.rs`, `config.rs`, `auth.rs`, `registry.rs`, others | 5,011 |

Those totals include the upstream's own tests and its handling of cases alc
never reaches. A version covering only the contract above is far smaller — but
"far smaller" is the estimate to be sceptical of, and the reason this ships
behind the existing dependency rather than instead of it.

## Ground truth

A 67-file corpus captured with `CCP_TRAFFIC_LOG=1` covering all three
surfaces, each with its client request, the upstream request, upstream
headers, and the raw SSE response. Golden tests replay these: a translator
that reproduces the captured upstream request byte-for-byte, and the captured
client response from the captured SSE, is correct by construction on
everything that was exercised.

## Acceptance

1. All three surfaces answer, streaming and not, with tools.
2. `gpt-6-astra` works — the reason this exists.
3. Every one of alc's eight agents starts and completes a turn.
4. Golden tests over the captured corpus pass.
5. It degrades honestly: an unknown model, an expired token and a refused
   upstream each produce a message naming the real cause.

## Staging

The new bridge lands behind `ALC_BRIDGE=native`, with the vendored crate as
the default, so it can be exercised against real agents before it becomes the
default in a later release. That ordering is deliberate: this replaces a layer
that currently works.
