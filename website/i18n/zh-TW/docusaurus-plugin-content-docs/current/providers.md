---
id: providers
title: Provider 相容性
sidebar_position: 5
description: 哪些 LLM provider 能搭配八個 coding agent 中的每一個、內建的十四種 provider kind 預設值與它們各自的預設 URL／模型／金鑰環境變數，以及為什麼需要 Anthropic Messages 或 OpenAI Responses 端點。
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - ollama claude code
  - provider presets
---

# Provider 相容性

這八個 coding agent 講的模型協定並不相同。`alc` 會在啟動前先驗證組合，而不是
靜靜送出一個不相容的請求。

| Agent | 支援端點 |
| --- | --- |
| Claude Code | Anthropic 相容端點 |
| Codex CLI | OpenAI Responses API |
| OpenCode | 任何 API 相容的 provider |
| Pi | Anthropic、OpenAI，或 OpenAI 相容端點 |
| Copilot CLI | OpenAI 或 Anthropic 相容端點 |
| Goose | OpenAI 或 Anthropic 相容端點 |
| Qwen Code | OpenAI、Anthropic，或 Gemini 相容端點 |
| Kimi Code CLI | OpenAI 或 Anthropic 相容端點 |

不論原生支援什麼，每個 agent 都能透過 [Codex 橋接](./codex-to-claude.md)搭配
一次 `codex login` 運作 —— `codex` 這個 provider kind 支援全部八個 agent。

## 差異從何而來

- Claude Code 的 gateway 必須提供 Anthropic Messages、Bedrock 或 Vertex API
  格式，由 `ANTHROPIC_BASE_URL` 指定使用哪一個 gateway。
- Codex CLI 自訂的 provider 使用 OpenAI Responses wire API。
- OpenRouter、Ollama，以及四個較新的預設值（DeepSeek、Moonshot、Z.ai、
  MiniMax）除了 OpenAI 格式的端點之外，也各自提供一個 Anthropic 相容端點，
  Claude Code 可以直接使用。
- OpenCode、Pi、Copilot CLI、Goose、Qwen Code、Kimi Code CLI 都能接受只有
  Chat Completions 的服務；只有 Claude Code 與 Codex CLI 需要更多。

## Provider kind 預設值

`alc config` 內建十四種 provider kind。選擇 `--kind` 會自動帶入預設端點、金鑰
環境變數與起始模型；每個值都只是 `config.toml` 裡的一般欄位，`alc config
upsert` 可以覆寫。

| Kind | 預設端點 | 金鑰環境變數 | 起始模型 |
| --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `sonnet` |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-5.6-terra` |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4.6` |
| `codex` | —（原生 `codex login`） | — | —（見 [Codex 橋接](./codex-to-claude.md)） |
| `ollama` | `http://localhost:11434` | — | `qwen3-coder` |
| `vllm` | `http://localhost:8000/v1` | — | —（依部署而定；預設為停用狀態） |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-v4-pro` |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3` |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | `MiniMax-M3` |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-build-0.1` |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-3.7-flash` |
| `custom` | —（自行提供） | —（透過 `--api-key-env` 自行命名） | —（自行提供） |

`deepseek`、`moonshot`、`zai`、`minimax` 都在主要的 OpenAI-chat 端點之外，各自
另外附帶*第二個* Anthropic 相容 base URL —— 這就是它們不用額外設定就
「Claude 可用」的原因：

| Kind | Anthropic 相容 URL |
| --- | --- |
| `deepseek` | `https://api.deepseek.com/anthropic` |
| `moonshot` | `https://api.moonshot.ai/anthropic` |
| `zai` | `https://api.z.ai/api/anthropic` |
| `minimax` | `https://api.minimax.io/anthropic` |

這些預設值只是起始值，不是永久不變的：上游 model ID 改變的速度比 alc 發版
還快，所以請把上面每一個「起始模型」都當成一個可以在 `alc config` 裡修改的
預設值，而不是 provider 目前實際提供內容的保證。

## alc 認得的協定

每個 provider profile 都會宣告協定，決定 alc 允許哪些組合：

| 協定 | 意義 |
| --- | --- |
| `anthropic-messages` | Anthropic Messages API |
| `openai-responses` | OpenAI Responses API |
| `openai-chat` | 只支援 Chat Completions |
| `codex-native` | Codex CLI 登入，透過隨附的橋接使用 |
| `dual` | 同時提供 Anthropic Messages 與 OpenAI Responses |

執行 `alc doctor` 可以印出你自己設定的相容性矩陣 —— 每個 provider profile
對照全部八個 agent。
