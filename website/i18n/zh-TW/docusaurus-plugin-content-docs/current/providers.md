---
id: providers
title: Provider 相容性
sidebar_position: 5
description: 九個 coding agent 各自能搭配哪些 LLM provider，以及十六種內建 provider kind 預設值的端點、金鑰環境變數與起始模型。
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - provider presets
  - coding agent compatibility
  - 中文
---

# Provider 相容性

這九個 agent 講的模型協定並不相同，所以 alc 會在啟動前先檢查這個組合，而不是
把一個注定失敗的請求送出去。

| Agent | 支援端點 | 協定 |
| --- | --- | --- |
| Claude Code | Anthropic 相容端點 | `anthropic-messages` |
| Codex CLI | OpenAI Responses API | `openai-responses` |
| OpenCode | 任何 API 相容的 provider | 皆可 |
| Pi | Anthropic、OpenAI，或 OpenAI 相容端點 | 皆可 |
| Copilot CLI | OpenAI 或 Anthropic 相容端點 | `openai-chat`、`anthropic-messages` |
| Goose | OpenAI 或 Anthropic 相容端點 | `openai-chat`、`anthropic-messages` |
| Qwen Code | OpenAI、Anthropic，或 Gemini 相容端點 | `openai-chat`、`anthropic-messages` |
| Kimi Code CLI | OpenAI 或 Anthropic 相容端點 | `openai-chat`、`anthropic-messages` |
| Antigravity CLI | 它自己的 Google 登入；透過 alc 橋接的 Codex 登入或任何 API | `antigravity-native` |

不論一個 agent 原生支援什麼，它同樣能透過 [Codex 橋接](./codex-to-claude.md)，
只靠一次 `codex login` 運作。Antigravity CLI 也能跑在那裡，自己的模型仍用它的
Google 登入（見[在 Codex 登入上](./agents.md#在-codex-登入上)）。agy 自己的 Google
登入只屬於 agy；其他 agent 可以改把工作交給 agy（見 [Antigravity](#antigravity)）。

## 任何 provider 接任何 agent

上表排除的組合照樣能跑：alc 會為那個 session 在行程內啟動橋接，把每一輪轉譯成
provider 自己的協定。Claude Code 接 OpenAI、Groq、xAI 或 Gemini，Codex CLI 接只有
Chat Completions 或只有 Anthropic 的 provider，以及 agy 接任何有 API 的 provider，
都是這樣啟動的。

| Profile 提供 | 橋接使用 |
| --- | --- |
| `openai-responses` | OpenAI Responses（`…/responses`） |
| `openai-chat` 或 `dual` | Chat Completions（`…/chat/completions`） |
| 只有 `anthropic-messages` | Anthropic Messages（`…/v1/messages`） |

橋接用 profile 的金鑰簽署請求，金鑰不會交給 agent；provider 若點名拒絕某個欄位
（例如推理模型上的 `max_tokens`），橋接會拿掉它再試一次。工具呼叫與結果以工具
呼叫的形式往返；provider 串流回來的推理會顯示出來，但不會重送。走這條路的
Claude Code，每個模型別名都對應到 profile 的模型，它的[背景
session](./background-sessions.md) 會隨啟動它的 alc 一起結束。agy 會在 `/model`
選單裡列出 profile 的模型，標上 profile 名稱，而且啟動時仍需要它自己的 Google
登入。

`alc doctor` 會把這張表換成你自己的 profile，印出實際的結果。

## 預設值

選定一個 `--kind`，端點、金鑰環境變數和起始模型就都填好了。這些值在
`config.toml` 裡都只是一般欄位，`alc config upsert` 隨時可以覆寫。

| Kind | 預設端點 | 金鑰環境變數 | 起始模型 |
| --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `sonnet` |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-6.1-sol` |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4.6` |
| `codex` | —（原生 `codex login`） | — | — |
| `antigravity` | —（原生 `agy` 登入） | — | `claude-opus-5-5-high` |
| `ollama` | `http://localhost:11434` | — | `qwen3-coder` |
| `vllm` | `http://localhost:8000/v1` | — | —（預設為停用狀態） |
| `llamacpp` | `http://localhost:8080/v1` | `LLAMA_API_KEY` | —（填伺服器列出的名稱） |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-v4-pro` |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3` |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | `MiniMax-M3` |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-build-0.1` |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-3.7-flash` |
| `custom` | —（自行提供） | —（用 `--api-key-env` 指定名稱） | — |

新建的 `openai` profile 也會以 `low` 推理強度啟動。既有 profile 已儲存的模型
與強度維持原樣。

`antigravity` 會在 Antigravity CLI 自己的登入上啟動它，而且只給 Antigravity CLI
用：其他 agent 在這個 profile 上都會被拒絕（見 [Antigravity](#antigravity)）。在它
出現之前寫出的 `config.toml` 沒有這個 profile；
`alc config upsert antigravity --kind antigravity` 會把它加上。

model ID 換得比 alc 發版還快，所以上面每一個起始模型都只是拿來改的值，不是
provider 今天真的提供什麼的保證。

## 不用額外設定就能跑 Claude

DeepSeek、Moonshot、Z.ai 與 MiniMax 在 OpenAI 格式的端點之外，各自還公開了
第二個 Anthropic 相容的 base URL，所以 Claude Code 可以直接跑在它們上面：

| Kind | Anthropic 相容 URL |
| --- | --- |
| `deepseek` | `https://api.deepseek.com/anthropic` |
| `moonshot` | `https://api.moonshot.ai/anthropic` |
| `zai` | `https://api.z.ai/api/anthropic` |
| `minimax` | `https://api.minimax.io/anthropic` |

## 在本機伺服器上跑 Claude Code

Ollama、llama.cpp 與 vLLM 都在根路徑上提供 Anthropic Messages，和 OpenAI 路由
並列，所以不論 profile 替其他 agent 指定哪一種 OpenAI 協定，Claude Code 在這三者
上都能跑。[本機模型](./local-models.md)說明 alc 替每一種設定了什麼，以及怎麼讓
第一輪不要拖太久。

## Antigravity

`alc agy` 會在 Google 的 Antigravity CLI 自己的 Google 登入上啟動它（見
[Antigravity CLI](./agents.md#antigravity-cli-agy)）。這個登入只屬於 agy。
Antigravity 的[服務條款](https://antigravity.google/terms)第 6 條寫明：「Using
third party software, tools, or services to access the Service (e.g. using
OpenClaw with Antigravity OAuth) is a breach of this Agreement」（用第三方軟體、
工具或服務存取本服務即違反本協議），Google 也曾因此停用帳號。舊版 alc 會讀取
agy 存好的 token、以 agy 的身分向 Google 連線，讓其他每個 agent 跑在這個登入上；
這條路已經移除，alc 也不再讀取 agy 的登入。

其他任何 agent 跑在 `antigravity` profile 上 —— `alc --agy claude`、
`alc --agy opencode`、`alc -p antigravity pi` —— 都會在啟動前被拒絕：

```text
error: provider 'antigravity' is agy's own Google login, which only the Antigravity CLI may use: Antigravity's terms forbid other tools from signing in with it, and alc no longer routes claude through it. Run agy itself with `alc agy`, or use `alc claude --with-agy` (Claude Code on its own provider, with agy as a tool it can hand tasks to)
```

Claude Code 以外的 agent，最後一段會是「or use <agent> on another provider, with
agy added as an MCP server: `alc mcp agy` (stdio)」。`[defaults]` 裡把其他 agent
指向 antigravity profile 的設定，不會再讓設定檢查失敗；改由真正用到它的那次啟動
回報上面的錯誤。舊版 alc 替 Antigravity 寫下的背景橋接路由已經無法解析，會被
拒絕；請在其他 provider 上重新開始那個 session。

`alc usage` 會列出 antigravity profile，並附註「agy's own login; see its quota
inside agy」；`alc doctor` 會檢查 agy 的執行檔、profile 與它的模型。

### 把工作交給 agy

`alc mcp agy` 是一個 stdio MCP 伺服器，只有一個工具 `agy`（在 Claude Code 裡
顯示為 `mcp__agy__agy`）。每次呼叫都會在伺服器的工作目錄 —— 也就是呼叫端
agent 的專案 —— 執行你自己那份未經修改的
`agy --print <prompt> --output-format stream-json`，用的是 agy 自己的登入，並回傳
agy 最後的回答與一個 `conversation_id`。這條路上 alc 不經手任何 Google 憑證。

`alc claude --with-agy` 會替這一次的 Claude Code session 接上它。Claude Code
照常跑在 `alc claude` 會用的 provider 上，alc 再附加一個 `--mcp-config`，指向
這個 alc 與它的設定目錄：

```sh
alc claude --with-agy
alc --codex claude --with-agy
```

`agy` 不在 `PATH` 上時它會被拒絕；和會直接執行的 Claude 子指令（例如
`alc claude mcp ...`）一起用時也會被拒絕。其他 MCP 用戶端請自行加入這個伺服器：

```sh
claude mcp add agy -- alc mcp agy
```

```json
{"mcpServers": {"agy": {"command": "alc", "args": ["mcp", "agy"]}}}
```

| 參數 | 意義 |
| --- | --- |
| `prompt` | 必填。完整、能獨立成立的任務：agy 看不到呼叫端的對話內容。 |
| `mode` | `plan`、`accept-edits` 或 `auto`。`plan` 只讀取與規劃、不改檔案。省略時用 agy 自己的預設，會修改檔案。 |
| `model` | `agy models` 列出的 agy 模型 id，例如 `gemini-3.8-flash-low`。省略時用 antigravity profile 的模型；只寫家族 id 時，會依強度換成對應的 wire id。 |
| `effort` | `low`、`medium`、`high`、`xhigh` 或 `max`。省略時用 antigravity profile 的；`ultra` 以 `max` 執行。 |
| `conversation_id` | 接續先前的 agy 對話（以 `agy --conversation` 傳入）。 |

多個呼叫可以同時執行。進度通知會回報 agy 的每一步（`agy: write_file`），
`notifications/cancelled` 會停掉那一次 agy 執行。沒有安裝 agy 時，呼叫會回傳
錯誤，請你安裝它並執行 `agy` 登入。這些 agy 執行的用量不會記進 alc 的用量紀錄。

無頭模式的 agy 和互動式 session 不一樣（在 agy 1.5.18 上驗證過）。在 print 模式
下，它會直接寫入工作區裡的檔案、不會先問。它沒辦法在執行 shell 指令前詢問，
所以會拒絕那個指令，工具結果最後會附上「agy was denied: Bash. Headless agy
cannot ask for permission; the user can allow it under permissions.allow in agy's
settings.json」。alc 從不傳入 `--dangerously-skip-permissions`。

### agy 用 Codex 登入

`alc --codex agy` 會把 agy 的 `CLOUD_CODE_URL` 指向 alc 的橋接，橋接會把 Codex
模型加進 agy 的選單，並把 agy 自己的流量原封不動轉給 Google，帶的是 agy 自己送出
的 token（見[在 Codex 登入上](./agents.md#在-codex-登入上)）。

:::caution

這會改變官方用戶端連線的端點，Antigravity 並不認可這種做法；在 agy 自己登入以外的任何 provider 上跑 agy 也一樣，因為它同樣是經由 alc 的 bridge 連線。請自行承擔風險。

:::

## Auto 模式在每個 provider 上都能用

Claude Code 的 Auto 模式會用一個分類器檢查每一次工具呼叫。Anthropic 在自己的
伺服器上執行這個檢查，其他 provider 都做不到，所以在其他每個 provider 上，alc
會讓 Claude Code 改用本機的分類器，並跑在 profile 的模型上：它把分類器使用的
`sonnet` 別名（`ANTHROPIC_DEFAULT_SONNET_MODEL`）釘到那個模型，並設定
`CLAUDE_CODE_AUTO_MODE_SERVER=0` 跳過伺服器端的嘗試。這樣 Auto 模式在
OpenRouter、DeepSeek、自訂端點，或任何 Claude Code 連得到的地方都能運作，分類器
請求使用的是那個 provider 的額度。`/model sonnet` 也會指向 profile 的模型。

有四種情況例外：

- `anthropic` 這個 kind —— 你自己的 Claude 登入或 API key —— 保留 Anthropic
  的伺服器端檢查與它的別名對照表。
- profile 的模型本身就是 Claude Code 的別名（例如 `sonnet` 或 `opus`）時不會
  被釘住：否則 Claude Code 會把那個別名原封不動當成模型名稱送出。伺服器端的
  嘗試仍會跳過。
- profile 的模型是完整的 Claude 模型 id（例如 `claude-opus-5`）時也不會被釘住：
  它的端點是架在 Anthropic 前面的閘道（LiteLLM、公司內部的 proxy），原本就以
  名稱提供 `claude-sonnet-*`，所以 `/model sonnet` 與每個宣告 `model: sonnet`
  的 subagent 仍使用 Claude Sonnet。伺服器端的嘗試仍會跳過。provider 自己的
  寫法（例如 OpenRouter 的 `anthropic/claude-sonnet-5`）不算完整的 Claude 模型
  id，仍會被釘住。
- 你自己的 `--settings` 若指定了 `ANTHROPIC_DEFAULT_SONNET_MODEL`，仍以你的
  為準。寫在 `~/.claude/settings.json` 裡的則不會：alc 的設定優先於它。

[Codex 橋接](./codex-to-claude.md#auto-模式與提示快取)與本機伺服器原本就是這樣
運作。
