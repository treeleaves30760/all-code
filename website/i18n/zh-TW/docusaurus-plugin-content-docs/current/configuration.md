---
id: configuration
title: 設定
sidebar_position: 7
description: alc 把 provider profile 與 API key 存在哪裡、怎麼在 TUI 裡改，以及不開 TUI 也能改設定的那些指令。
keywords:
  - alc config
  - provider profile
  - api key storage
  - 中文
---

# 設定

## 檔案位置

| 平台 | 設定目錄 |
| --- | --- |
| Windows | `%APPDATA%\alc` |
| macOS/Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/alc` |

檔案：

- `config.toml`：provider 中繼資料、模型、預設值、URL 與環境變數名稱。
- `credentials.toml`：本機儲存的 API key，在 Unix 上權限是 `0600`。
- `remote.toml`：[遠端控制](./remote-control.md)的設定。
- `usage.jsonl`：[`alc usage`](./usage.md) 彙整的那份啟動與 turn
  帳本。想重新開始計算，把它刪掉就好。
- `claude/settings-*.json`：alc 用 `--settings` 交給 Claude Code 的那些設定
  文件 —— 端點、模型相關變數、選單，以及 `apiKeyHelper` 那一行，裡面沒有任何
  一種金鑰。每一份都以自己內容的雜湊命名，所以每一次解析到同一份文件的啟動，
  用的都是同一個檔案；在 Unix 上一律以 `0600` 權限寫入。alc 從不刪除它們，
  因為[背景 session](./background-sessions.md)每次被 Claude Code 重新啟動時，
  都會再讀一次自己的那個檔案。沒有背景 session 在跑的時候，把它們刪掉是安全
  的；刪掉某個還在跑的 session 正在用的那一份，那個 session 就會壞掉，直到它
  下一次重新啟動為止。把它們留在那裡之前有一件事要知道：你自己傳的
  `--settings` 會被合併進那份文件，所以你放進自己檔案裡的憑證，也會在 alc
  那一份裡。
- `run/bridge.port`、`run/bridge.token`：[背景橋接](./background-sessions.md#背景橋接)
  正在哪裡聽，以及每一個送到它那裡的請求都必須帶著的那個 token。
- `run/bridge/routes/`：橋接服務的每一條 route 各一個檔案 —— 它花掉的是哪個
  provider profile、它的請求用哪一份 Codex `auth.json` 簽署，以及 Claude Code
  自己的 model id 會落到哪裡。

可用 `ALC_CONFIG_DIR` 覆寫目錄位置。

## 設定用的 TUI

```sh
alc config
```

每個畫面底部都會列出可用按鍵，主要操作如下：

- `a`、`e`/Enter、`d`：新增、編輯、刪除 provider。
- `Tab`／`Shift+Tab`，或直接按 `1`／`2`／`3`：在標題列列出的三個畫面之間切換
  —— Providers、Agent defaults、Sharing & remote。最後一個畫面放的是
  「預設共享」、綁定位址與權限上限。
- 方向鍵：移動欄位與切換選項，包含推理強度。
- 在 Codex profile 上，把游標移到 Model 欄位按 `←`/`→`，會開啟 GPT
  模型與推理強度的引導式選單，選好的結果就寫成 `alc --codex claude`
  的啟動預設值。
- `s`：儲存；`q`：儲存並離開；`Ctrl+C`：不儲存離開。

## 用指令設定

```sh
alc config init
alc config show
alc config path
alc config upsert codex --kind codex --model gpt-6-sol --effort medium
alc config upsert work --kind openrouter --model anthropic/claude-sonnet-4.6
printf '%s' "$OPENROUTER_API_KEY" | alc config key work --stdin
alc config set-default claude work
alc config remove work
```

`alc config upsert` 支援 `--kind`、`--model`、`--effort`、`--clear-effort`、
`--small-model`、`--base-url`、`--anthropic-base-url`、`--protocol`、`--auth`、
`--api-key-env`、`--codex-profile`、`--codex-home`、`--claude-config-dir`、
`--disable` 與 `--enable`。

## 同一種的多個登入

第二個 ChatGPT 或 Claude 登入，就是第二個 profile，指向它自己的憑證目錄：

```toml
[providers.codex-work]
kind = "codex"
codex_home = "/Users/you/.codex-work"

[providers.anthropic-work]
kind = "anthropic"
claude_config_dir = "/Users/you/.claude-work"
```

兩個路徑都必須是絕對路徑，`codex_home` 屬於 `codex` profile、
`claude_config_dir` 屬於 `anthropic` profile，而且兩者都優先於對應的環境變數
—— 於是留在 shell 設定檔裡的一個變數，沒辦法偷偷改掉一個具名 profile
花的是哪個帳號。完整流程在[用量](./usage.md)。

## 憑證優先順序

每個 provider profile 的 API key 依序解析：

1. `api_key_env` 指定的環境變數，只要有設定且不是空字串。
2. `credentials.toml` 裡儲存的 key。

驗證方式是 `native` 或 `none` 的 profile 完全不需要 key —— Codex 登入與
Ollama 這類本機執行環境都屬於這種。

## Codex 橋接的設定優先順序

1. 這次執行的 `--model` / `--effort`
2. alc 的 provider profile
3. `<codex_home>/<profile>.config.toml`，接著 `<codex_home>/config.toml`
   —— 這裡的 `codex_home` 取 profile 自己的欄位，沒有就取 `CODEX_HOME`，
   再沒有就是 `~/.codex`
4. 模型目錄記載的預設值
