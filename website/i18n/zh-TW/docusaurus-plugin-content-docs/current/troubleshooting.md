---
id: troubleshooting
title: 疑難排解
sidebar_position: 8
description: 用 alc doctor 診斷常見問題，包含找不到 agent、provider 不相容、缺少 API key 與 Codex 登入失效。
keywords:
  - alc doctor
  - codex 登入過期
---

# 疑難排解

先執行：

```sh
alc doctor
```

它會列出全部八個 agent 的執行檔狀態、憑證狀態、每個 provider profile 各自的
agent 相容性欄位、解析後的預設值，以及（當設定了 Codex provider 時）Codex
橋接的登入狀態。

## `'claude' is not installed or not on PATH`

alc 只負責啟動這台機器上已安裝的 agent。請先安裝該 agent，或用
`ALC_CLAUDE_BIN`、`ALC_CODEX_BIN`、`ALC_OPENCODE_BIN`、`ALC_PI_BIN`、
`ALC_COPILOT_BIN`、`ALC_GOOSE_BIN`、`ALC_QWEN_BIN`、`ALC_KIMI_BIN` 指定執行檔
位置。

## `provider '…' cannot be used with claude; Claude Code needs Anthropic Messages`

選到的 profile 使用了 Claude Code 無法接受的協定。請改用 Anthropic 相容端點、
OpenRouter 或 Ollama，或改用
[`alc --codex claude`](./codex-to-claude.md)。詳見
[Provider 相容性](./providers.md)。

## `provider '…' has no API key`

用 `alc config key <profile>` 儲存一組 key，或設定該 profile `api_key_env` 欄位
指定的環境變數。

## `Codex credentials were not found`

執行 `codex login` 後再試一次。登入狀態會顯示在 `alc doctor` 輸出的
**Codex bridge** 底下。

## `the bundled claude-codex … helper is missing`

從原始碼建置不會包含轉接器。請改用一行安裝器重新安裝、把相容的 `claude-codex`
放進 PATH，或設定 `ALC_CLAUDE_CODEX_BIN`。

## 模型清單看起來過期

模型目錄每 24 小時最多向本機 Codex CLI 同步一次：

```sh
alc models --refresh
```

## 輸出裡的機密資料

`alc --dry-run` 會遮蔽 API key 與 auth token；`alc config show` 不會印出憑證內容，
只會顯示每個 profile 有沒有設定。
