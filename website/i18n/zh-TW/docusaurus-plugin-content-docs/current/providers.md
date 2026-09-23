---
id: providers
title: Provider 相容性
sidebar_position: 5
description: 八個 coding agent 各自能搭配哪些 LLM provider，以及十四種內建 provider kind 預設值的端點、金鑰環境變數與起始模型。
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - provider presets
  - coding agent compatibility
  - 中文
---

# Provider 相容性

這八個 agent 講的模型協定並不相同，所以 alc 會在啟動前先檢查這個組合，而不是
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

不論一個 agent 原生支援什麼，它同樣能透過 [Codex 橋接](./codex-to-claude.md)，
只靠一次 `codex login` 運作。

`alc doctor` 會把這張表換成你自己的 profile，印出實際的結果。

## 預設值

選定一個 `--kind`，端點、金鑰環境變數和起始模型就都填好了。這些值在
`config.toml` 裡都只是一般欄位，`alc config upsert` 隨時可以覆寫。

| Kind | 預設端點 | 金鑰環境變數 | 起始模型 |
| --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `sonnet` |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-6-sol` |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4.6` |
| `codex` | —（原生 `codex login`） | — | — |
| `ollama` | `http://localhost:11434` | — | `qwen3-coder` |
| `vllm` | `http://localhost:8000/v1` | — | —（預設為停用狀態） |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-v4-pro` |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3` |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | `MiniMax-M3` |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-build-0.1` |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-3.7-flash` |
| `custom` | —（自行提供） | —（用 `--api-key-env` 指定名稱） | — |

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

## 在本機 Ollama 模型上跑 Claude Code

已移到[本機模型](./local-models.md)：那一頁說明 `alc --ollama claude` 會設定
什麼，以及在筆電上怎麼讓第一輪不要拖太久。
