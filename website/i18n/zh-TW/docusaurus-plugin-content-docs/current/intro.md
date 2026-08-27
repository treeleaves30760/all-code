---
id: intro
slug: /
title: all-code (alc)
sidebar_label: 簡介
sidebar_position: 1
description: 一個 CLI 先設定好 LLM provider，之後用同一套指令啟動 Claude Code、Codex CLI 或 OpenCode，也能讓 Claude Code 直接使用 Codex／ChatGPT 登入。
keywords:
  - claude code
  - codex cli
  - opencode
  - llm provider
  - coding agent
  - 中文
---

# all-code (`alc`)

**一個 CLI 同時管好 Claude Code、Codex CLI 與 OpenCode。** 先設定好 LLM
provider，之後用同一套指令啟動任何一個 coding agent，搭配任何一家 provider —
包含讓 Claude Code 直接使用你的 Codex／ChatGPT 訂閱。

```sh
alc config
alc claude
alc codex
alc opencode
alc --codex claude
alc --openrouter codex
alc --provider work opencode
```

## alc 能做什麼

- **每個 agent 各自選 provider。** 讓 Claude Code、Codex CLI、OpenCode 指向
  Anthropic、OpenAI API、OpenRouter、Ollama、vLLM 或自訂端點，也可以只為這次
  執行換掉，不用手改設定檔。
- **用 GPT 模型跑 Claude Code。** [`alc --codex claude`](./codex-to-claude.md)
  會把 Codex／ChatGPT 登入橋接給 Claude Code，並把所有 GPT 模型列進 Claude
  Code 自己的 `/model` 選單，讓你在工作階段中隨時換模型與推理強度。
- **啟動前先驗證相容性。** alc 會先確認 agent 與 provider
  的[模型協定相容](./providers.md)，而不是送出一個注定失敗的請求。
- **憑證分開存放。** API key 放在獨立檔案或改用環境變數，不會在 agent 之間被
  複製。

## 為什麼需要它

每個 coding agent 設定 provider 的方式都不一樣：Claude Code 讀 Anthropic 風格
的環境變數，Codex CLI 走命令列上的 TOML 覆寫，OpenCode 則要一份行內 JSON 設定。
想讓同一組 provider 在三個工具上都能用，等於同樣的事要用三種格式各做一次，而且
每次換 key 或換端點都要重做。

`alc` 只維護一份 provider 清單，再翻譯成你要啟動的那個 agent 看得懂的形式。

## 事前準備

`alc` 只負責啟動已經安裝好的 coding agent，請自行安裝你要用的：

- [Claude Code](https://code.claude.com/docs/en/setup)
- [Codex CLI](https://learn.chatgpt.com/docs/codex/cli)
- [OpenCode](https://opencode.ai/docs)

## 下一步

- [安裝 alc](./installation.md)
- [快速開始](./quick-start.md)
- [用 GPT 模型跑 Claude Code](./codex-to-claude.md)
