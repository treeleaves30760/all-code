---
id: quick-start
title: 快速開始
sidebar_position: 3
description: 在 alc 的 TUI 裡設定 provider、啟動八個 coding agent 中的任何一個、只為某次執行換掉 provider，並預覽實際會執行的指令。
keywords:
  - 切換 llm provider
  - 啟動 claude code
  - openrouter codex
---

# 快速開始

## 1. 設定 provider

開啟全螢幕設定介面：

```sh
alc config
```

初始設定內含 Anthropic、OpenAI、OpenRouter、Codex、Ollama，以及一個預設停用的
vLLM 範本。你可以在 TUI 裡填入 API key，或讓 profile 指向某個環境變數。環境變數
的優先權高於本機儲存的 key。

## 2. 啟動 agent

每個 agent 都會用自己設定的預設 provider 啟動：

```sh
alc claude
alc codex
alc opencode
alc pi
alc copilot
alc goose
alc qwen
alc kimi
```

各 agent 實際會被設定什麼，請見[支援的 agent](./agents.md)。

## 3. 只替這次執行指定 provider

```sh
alc --codex claude
alc --openrouter codex
alc --deepseek pi
alc --codex opencode
alc -p local-vllm opencode
```

`--provider`（或 `-p`）接受 profile 名稱；當某個 kind 只有一個 profile 時，也可以
直接寫 kind。捷徑旗標 `--anthropic`、`--openai`、`--openrouter`、`--codex`、
`--ollama`、`--vllm`、`--deepseek`、`--moonshot`、`--zai`、`--minimax`、`--groq`、
`--xai`、`--google` 效果相同。

## 參數傳遞

除了 Claude 專用的 `--model`、`--effort`、`--save` 之外，agent 名稱後的參數會原封
不動傳入：

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc --ollama opencode run "fix the failing test"
```

如果要把同名參數交給 Claude Code 本身，請放在 `--` 後面：

```sh
alc claude -- --model sonnet
```

## 先預覽不啟動

印出實際會使用的指令與環境變數，API key 會被遮蔽：

```sh
alc --openrouter --dry-run claude
```

## 檢查環境

```sh
alc doctor
```

`alc doctor` 會列出全部八個 agent 的執行檔狀態、憑證狀態、每個 provider profile
各自的 agent 相容性欄位、解析後的預設值，以及（當設定了 Codex provider 時）
Codex 橋接的登入狀態。
