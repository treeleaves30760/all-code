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

它會列出 agent 執行檔、憑證狀態、Codex 登入、隨附的轉接器、Codex-to-Claude 的解析
結果，以及相容性矩陣。

## `'claude' is not installed or not on PATH`

alc 只負責啟動這台機器上已安裝的 agent。請先安裝該 agent，或用 `ALC_CLAUDE_BIN`、
`ALC_CODEX_BIN`、`ALC_OPENCODE_BIN` 指定執行檔位置。

## `provider '…' cannot be used with claude; Claude Code needs Anthropic Messages`

選到的 profile 使用了 Claude Code 無法接受的協定。請改用 Anthropic 相容端點、
OpenRouter 或 Ollama，或改用
[`alc --codex claude`](./codex-to-claude.md)。詳見
[Provider 相容性](./providers.md)。

## `provider '…' has no API key`

用 `alc config key <profile>` 儲存一組 key，或設定該 profile `api_key_env` 欄位
指定的環境變數。

## `Codex credentials were not found`

執行 `codex login` 後再試一次。`alc doctor` 的 **Codex login** 區塊會顯示登入狀態。

## `the bundled claude-codex … helper is missing`

從原始碼建置不會包含轉接器。請改用一行安裝器重新安裝、把相容的 `claude-codex` 放進
PATH，或設定 `ALC_CLAUDE_CODEX_BIN`。

## 模型清單看起來過期

模型目錄每 24 小時最多向本機 Codex CLI 同步一次：

```sh
alc models --refresh
```

## 輸出裡的機密資料

`alc --dry-run` 會遮蔽 API key 與 auth token；`alc config show` 不會印出憑證內容，
只會顯示每個 profile 有沒有設定。
