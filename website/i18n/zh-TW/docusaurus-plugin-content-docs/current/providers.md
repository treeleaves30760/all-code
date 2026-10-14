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
| Antigravity CLI | 它自己的 Google 登入 | `antigravity-native` |

不論一個 agent 原生支援什麼，它同樣能透過 [Codex 橋接](./codex-to-claude.md)，
只靠一次 `codex login` 運作 —— 只有 Antigravity CLI 例外，它只跑在自己的 Google
登入上（見 [Antigravity CLI](./agents.md#antigravity-cli-agy)）。

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

`antigravity` 只會啟動 Antigravity CLI，不會啟動別的，所以它還不能跑 Claude。
讓其他 agent 也經由那個登入連線的功能即將推出，而且會是需要你明確選擇開啟的
選項，因為 Google 曾經停權把那個登入拿給第三方使用的帳號（論壇討論串見
[Antigravity CLI](./agents.md#antigravity-cli-agy)）。在它出現之前寫出的
`config.toml` 沒有這個 profile；
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
