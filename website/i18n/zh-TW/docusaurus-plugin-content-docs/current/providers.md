---
id: providers
title: Provider 相容性
sidebar_position: 5
description: 哪些 LLM provider 能搭配 Claude Code、Codex CLI 與 OpenCode，以及為什麼需要 Anthropic Messages 或 OpenAI Responses 端點。
keywords:
  - anthropic messages api
  - openai responses api
  - ollama claude code
---

# Provider 相容性

這些 coding agent 講的模型協定並不相同。`alc` 會在啟動前先驗證組合，而不是靜靜送出
一個不可能成功的請求。

| Provider profile | Claude Code | Codex CLI | OpenCode |
| --- | --- | --- | --- |
| Anthropic | 可 | 不可 | 可 |
| OpenAI API | 需要 gateway | 可（Responses API） | 可 |
| OpenRouter | 可（Anthropic 相容層） | 可（Responses API） | 可 |
| Codex 登入 | 可（隨附轉接器） | 可（原生） | 無法直接沿用憑證 |
| Ollama | 可（Anthropic 相容） | 可（`--oss`） | 可 |
| vLLM | 若提供 Anthropic Messages | 若提供 Responses | 可 |
| 自訂 | 依設定的協定而定 | 依設定的協定而定 | 可 |

## 差異從何而來

- Claude Code 的 gateway 必須提供 Anthropic Messages、Bedrock 或 Vertex 格式，
  由 `ANTHROPIC_BASE_URL` 指定。
- Codex 的自訂 provider 使用 OpenAI Responses wire API。
- OpenRouter 與 Ollama 提供 Anthropic 相容端點，Claude Code 可以直接使用。

如果某個 OpenAI 相容服務只實作 Chat Completions，請搭配 OpenCode 使用。Claude Code
需要 Anthropic 相容 gateway，而目前的 Codex 需要 Responses 而非 Chat Completions。

## alc 認得的協定

每個 provider profile 都會宣告協定，決定 alc 允許哪些組合：

| 協定 | 意義 |
| --- | --- |
| `anthropic-messages` | Anthropic Messages API |
| `openai-responses` | OpenAI Responses API |
| `openai-chat` | 只支援 Chat Completions，適用於 OpenCode |
| `codex-native` | Codex CLI 登入，透過隨附轉接器使用 |
| `dual` | 同時提供 Anthropic Messages 與 OpenAI Responses |

執行 `alc doctor` 可以印出你自己設定的相容性矩陣。
