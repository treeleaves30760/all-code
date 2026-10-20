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
登入也能給其他每個 agent 使用（見 [Antigravity](#antigravity)）。

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

`antigravity` 會在 Antigravity CLI 自己的登入上啟動它，也會透過 alc 的橋接服務
其他每個 agent，Claude Code 也包括在內（見 [Antigravity](#antigravity)）。在它
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
[Antigravity CLI](./agents.md#antigravity-cli-agy)）。同一個登入也能透過 alc 的
橋接，給其他每個 agent 使用：

```sh
alc --agy claude                          # Claude Code，用 claude-opus-5-5，強度 high
alc --agy claude --model gemini-3.8-flash --effort low
alc --agy opencode                        # 或 pi、kimi、codex、copilot、goose、qwen
alc --agy claude --context max            # Claude 840K、Argon 900K、Flash 1,048K
alc --agy usage                           # 方案各時間窗還剩多少
```

alc 借用 agy 已經存好的 access token —— Windows 認證管理員裡的
`gemini:antigravity` 項目，或檔案
`~/.gemini/antigravity-cli/antigravity-oauth-token` —— 其他什麼都不留：它從不
儲存 refresh token、不改寫 agy 的登入，也從不自己跑 Google 登入流程。登入仍歸
agy 管：token 快過期時，alc 會執行 `agy --output-format json models`（這會讓 agy
自己更新 token），再重新讀一次。兩個地方都有登入時，alc 會拒絕而不去猜；請在
agy 裡登出再登入。對 Google 來說，alc 會以 agy 用戶端的身分出現（agy 的
User-Agent 與用戶端標頭），因為那個端點是依這個身分決定帳號能用哪些模型。

使用這項選用整合仍受帳號提供者的條款與限制約束，不保證相容性，也不保證帳號可用。

### 模型、強度與 context

這個登入上有五個模型家族：`claude-opus-5-5`、`claude-sonnet-5-5`、
`gemini-4-argon`（Google AI Ultra 帳號）、`gemini-3.8-flash` 與
`gpt-oss-120b`，每個都有 `low`、`medium`、`high` 三種強度（GPT-OSS 只有
`medium`）。Google 收到的 id 是兩者相連，例如 `gemini-3.8-flash-low`。
`alc models --agy` 會連同 window 一起列出它們，並把 agy 回報、但沒有任何家族
提供的 id 標上 ` (not bridgeable)`：agy 也許能跑，其他 agent 不行。

Claude Code 照常逐次切換模型與強度：它的 `/model` 選單列出這五個家族，它的強度
決定用哪個變體，`opus` 對應 Claude Opus 5.5、`sonnet` 對應這個 session 的模型、
`haiku` 對應 Gemini 3.8 Flash。高於 `high` 的強度會以 `high` 執行，家族沒有的強度
則換成它有的，兩者都會印出一行說明。`--model`、`--effort` 與 `--context` 只改
這一次的 `alc --agy claude`；`--save` 會被拒絕，並附上能存下同樣選擇的
`alc config upsert` 指令。其他 agent 整個 session 都用同一個模型 —— profile 的
模型配上 profile 的強度 —— 所以請在 profile 上選：

```sh
alc config upsert antigravity --model gemini-3.8-flash-low --effort low --context max
```

每個家族一開始用 256K 的 window（GPT-OSS 是 131K）。在 profile 上設
`context = "max"`，或只限 Claude Code 這一次的 `--context max`，Claude 會變成
840K、Argon 900K、Flash 1,048K。Codex CLI 會以 `model_context_window` 拿到這個
window（較舊的 Codex 版本還得在它自己的設定裡手動設
`model_auto_compact_token_limit`），並以 `model_reasoning_effort` 拿到推理強度，
所以它的頁尾顯示的就是這個 session 實際用的強度。Codex session 內的 `/model`
選單仍會列出 OpenAI 的模型名稱，這些在這個登入上會被 alc 的橋接拒絕：請改在
profile 上換模型，或用 `--model` 傳入這個登入的 id。

### 推理簽章、額度與平台

Gemini 會替推理內容簽章，簽章會跟著對話走。Gemini 的工具呼叫若遺失了簽章 ——
例如在別處開始的 session、任務進行中改了強度，或橋接重新啟動過 —— 仍會以原本的
工具呼叫與結果送出：已結束的回合不帶簽章（Google 接受），進行中的回合則帶上
Google 文件記載的佔位簽章（見[疑難排解](./troubleshooting.md#antigravity-上的-gemini-把工具呼叫寫成文字)）。

`alc usage` 會顯示方案還剩多少（見 [Antigravity 登入](./usage.md#antigravity-登入)），
`alc doctor` 會檢查 agy、登入、它的 Google 專案與 profile。讀取登入的方式在
Windows 上驗證過；在 macOS 與 Linux 上屬於盡力而為，token 檔案則在每個作業系統上
都能用。

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
