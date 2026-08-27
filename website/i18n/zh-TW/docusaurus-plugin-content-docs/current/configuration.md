---
id: configuration
title: 設定
sidebar_position: 6
description: alc 把 provider profile 與 API key 存在哪裡、如何在 TUI 中編輯，以及不開 TUI 也能修改設定的指令。
keywords:
  - alc config
  - provider profile
  - api key 儲存
---

# 設定

## 檔案位置

| 平台 | 設定目錄 |
| --- | --- |
| Windows | `%APPDATA%\alc` |
| macOS/Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/alc` |

檔案：

- `config.toml`：provider 中繼資料、模型、預設值、URL 與環境變數名稱。
- `credentials.toml`：本機儲存的 API key。在 Unix 上 alc 會以 `0600` 權限寫入；
  在 Windows 則位於目前使用者的 AppData 底下。

可用 `ALC_CONFIG_DIR` 覆寫目錄位置。

## 設定用的 TUI

```sh
alc config
```

每個畫面底部都會顯示可用按鍵，主要操作如下：

- `a`、`e`/Enter、`d`：新增、編輯、刪除 provider。
- `Tab`：在 provider 清單與 agent 預設值之間切換。
- 方向鍵：移動欄位與切換選項，包含推理強度。
- 在 Codex profile 上，把游標移到 Model 欄位並按 `←`/`→`，會開啟模型與推理強度的
  選擇畫面，選好即成為 `alc --codex claude` 的啟動預設值。
- `s`：儲存；`q`：儲存並離開；`Ctrl+C`：不儲存離開。

## 用指令設定

```sh
alc config init
alc config show
alc config path
alc config upsert codex --kind codex --model gpt-5.6-terra --effort medium
alc config upsert work --kind openrouter --model anthropic/claude-sonnet-4.6
printf '%s' "$OPENROUTER_API_KEY" | alc config key work --stdin
alc config set-default claude work
alc config remove work
```

`alc config upsert` 支援 `--kind`、`--model`、`--effort`、`--clear-effort`、
`--small-model`、`--base-url`、`--anthropic-base-url`、`--protocol`、`--auth`、
`--api-key-env`、`--codex-profile`、`--disable`、`--enable`。

## 憑證優先順序

每個 provider profile 的 API key 依序解析：

1. `api_key_env` 指定的環境變數，只要有設定且不是空字串。
2. `credentials.toml` 裡儲存的 key。

驗證方式為 `native` 或 `none` 的 profile 完全不需要 key，例如 Codex 登入與 Ollama
這類本機執行環境。

## Codex-to-Claude 的設定優先順序

1. 這次指令的 `--model` / `--effort`
2. alc 的 provider profile
3. `~/.codex/<profile>.config.toml`，接著 `~/.codex/config.toml`
4. 模型目錄記載的預設值
