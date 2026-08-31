---
id: intro
slug: /
title: all-code (alc)
sidebar_label: 簡介
sidebar_position: 1
description: 一個 CLI 只要設定一次 LLM provider，就能啟動八個 coding agent 中的任何一個，也能讓其中任何一個改用 Codex／ChatGPT 登入。
keywords:
  - claude code
  - codex cli
  - opencode
  - pi coding agent
  - copilot cli
  - goose
  - qwen code
  - kimi code cli
  - llm provider
  - coding agent
  - 中文
---

# all-code (`alc`)

**一個 CLI，就能管好八個 coding agent。** 先設定好你的 LLM
provider，之後就能用任何一家 provider 啟動
[Claude Code](https://code.claude.com/docs/en/setup)、
[Codex CLI](https://learn.chatgpt.com/docs/codex/cli)、
[OpenCode](https://opencode.ai/docs)、
[Pi](https://github.com/earendil-works/pi)、
[Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli)、
[Goose](https://block.github.io/goose/)、
[Qwen Code](https://github.com/QwenLM/qwen-code)，或
[Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli) ——
包括讓其中任何一個都能使用你的 Codex／ChatGPT 訂閱。

```sh
alc config
alc claude
alc codex
alc opencode
alc pi
alc copilot
alc goose
alc qwen
alc kimi
alc --codex opencode
alc --deepseek claude
alc --provider work opencode
```

## alc 能做什麼

- **每個 agent 各自選 provider。** 讓八個 agent 中的任何一個指向
  Anthropic、OpenAI API、OpenRouter、Ollama、vLLM、DeepSeek、Moonshot、
  Z.ai、MiniMax、Groq、xAI、Google，或任何自訂端點，也可以只為這次執行
  換掉，不用手改設定檔。
- **讓每個 agent 都能用 GPT 模型執行。** [`alc --codex <agent>`](./codex-to-claude.md)
  會把你的 Codex／ChatGPT 登入橋接給你啟動的那個 agent。Claude Code 會
  把所有 GPT 模型列進自己的 `/model` 選單，讓你在工作階段中隨時切換模型
  與推理強度；其他每個 agent 則是在啟動時就選定一個模型，整個工作階段
  都使用它。
- **啟動前先驗證相容性。** alc 會先確認 agent 與 provider
  的[模型協定相容](./providers.md)，而不是送出一個注定失敗的請求。
- **憑證分開存放。** API key 放在獨立檔案或改用環境變數，不會在 agent
  之間被複製。

## 為什麼需要它

每個 coding agent 對「如何設定 provider」都有自己的一套做法：Claude
Code、Copilot CLI、Goose 讀取環境變數；Codex CLI 與 Qwen Code 走命令列
上的旗標；OpenCode 需要一份行內 JSON 設定；Pi 會把一筆項目合併進自己的
`models.json`；Kimi Code CLI 則是合併進一份 TOML 設定檔。想讓同一組
provider 在全部八個 agent 上都能用，就等於同樣的事要用八種格式各做一
次，而且每次換 key 或換端點都要重做一輪。

`alc` 只維護一份 provider 清單，再翻譯成你要啟動的那個 agent 看得懂的
格式 —— 確切會設定哪些內容，請見[支援的 agent](./agents.md)。

## 事前準備

`alc` 只負責啟動已經安裝好的 coding agent，請自行安裝你要用的：

- [Claude Code](https://code.claude.com/docs/en/setup)
- [Codex CLI](https://learn.chatgpt.com/docs/codex/cli)
- [OpenCode](https://opencode.ai/docs)
- [Pi](https://github.com/earendil-works/pi)（`npm install -g @earendil-works/pi-coding-agent`）
- [Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli)
- [Goose](https://block.github.io/goose/)
- [Qwen Code](https://github.com/QwenLM/qwen-code)
- [Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli)

## 下一步

- [安裝 alc](./installation.md)
- [快速開始](./quick-start.md)
- [支援的 agent](./agents.md)
- [Codex 橋接](./codex-to-claude.md)
