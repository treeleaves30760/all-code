---
id: quick-start
title: 快速開始
sidebar_position: 3
description: 在 alc 的 TUI 裡設定 provider、啟動 Claude Code、Codex CLI 或 OpenCode、只為這次執行換掉 provider，並預覽實際會執行的指令。
keywords:
  - 切換 llm provider
  - 啟動 claude code
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
```

## 3. 只替這次執行指定 provider

```sh
alc --codex claude
alc --openrouter codex
alc -p local-vllm opencode
```

`--provider`（或 `-p`）接受 profile 名稱；當某個 kind 只有一個 profile 時，也可以
直接寫 kind。捷徑旗標 `--anthropic`、`--openai`、`--openrouter`、`--codex`、
`--ollama`、`--vllm` 效果相同。

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

`alc doctor` 會列出 agent 執行檔、憑證狀態、Codex 登入、隨附的轉接器、
Codex-to-Claude 的解析結果，以及相容性矩陣。
