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
- `usage.jsonl`：啟動紀錄，以及 Codex 協定轉譯橋接與選擇啟用的 `--metrics`
  觀測產生的純中繼資料請求紀錄。[`alc usage` 與 `alc tps`](./usage.md)會讀取它。
  刪除只會重設 alc 紀錄，不會重設獨立的 Claude／Codex 原生歷史。
- `pricing.toml`：用量估算選用的精確 USD token 費率覆寫。這是[獨立的
  sidecar](#價格-sidecar)，不是 `config.toml` 裡的一張 table。
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
  的監聽位置與控制／轉譯 token；這個 token 不會送給 API 服務商。持續 Claude
  API 觀測在本機請求這一段使用密封替代憑證。
- `run/bridge.observer-key`：獨立、只有擁有者可讀的本機觀測 secret（Unix 上為
  `0600`），不是服務商 API key。它驗證新的 host／控制 challenge，並把 Claude
  量測憑證密封綁定到固定 route／本次 host instance。握手不會公開它，也不會送到
  上游。資料平面仍是 loopback HTTP；這個 secret 不提供 TLS，也不能讓明文請求
  內容免於遭劫持的本機 port 攔截。
- `run/bridge/routes/`：Codex 協定轉譯的每一條 route 各一個檔案 —— 它花掉的是
  哪個 provider profile、請求用哪一份 Codex `auth.json` 簽署，以及 Claude Code
  自己的 model id 會落到哪裡。
- `run/bridge/forward/`：`--metrics` 使用的持續 Claude API-key 觀測 route；
  只放固定的 profile/kind/upstream 中繼資料，不放 API key。[helper](./background-sessions.md#背景-session-的-api-key-量測)
  解析金鑰、驗證 host、只在記憶體註冊摘要，再回傳 AEAD 密封的本機替代憑證。
  host 只有在派送時才還原上游驗證；觀測檔案從不儲存明文服務商金鑰。host 重啟會
  讓舊替代憑證失效；請重新執行 helper。

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
alc config upsert codex --kind codex --model gpt-6.1-sol --effort low
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

## 價格 sidecar

[`alc usage`](./usage.md#一個估算美元代表什麼)使用內建、離線且精選的 LiteLLM 子集，
加上選用的本機覆寫來估算；產生報告時不會下載價格。內建快照日期為 2026-10-08，
固定 LiteLLM commit `33d908e0ae2c0a257eeb5d546df08527d348a670`、上游 SHA-256 與
MIT 授權來源。報告的 `pricing_snapshot` 用雜湊識別內建資料與選用覆寫；歷史用量
依這份快照重新定價，不會還原成歷史發票。

覆寫放在 **`<alc-config-dir>/pricing.toml`**，或用
`alc usage --pricing-file /absolute/path/pricing.toml` 選另一個檔案。不要在
`config.toml` 加 `[pricing]`。以下範例把一個本機模型明確設為免費 —— 只在你要
零 API-token 參考費率時使用，不代表已計入電費或硬體：

```toml
version = 1
currency = "USD"
units = "USD-per-million-tokens"

[[models]]
provider = "custom"
model = "local-model"
profile = "local-work"
endpoint = "http://127.0.0.1:8080/v1"
input = "0"
output = "0"
cache_read = "0"
cache_write = "0"
```

| 欄位 | 規則 |
| --- | --- |
| `version`、`currency`、`units` | 必填，必須正好是上面的 `1`、`"USD"`、`"USD-per-million-tokens"`。 |
| `[[models]].provider`、`model` | 必填，精確比對有紀錄的 provider kind／參考 provider 與 model ID；profile 名稱放在 `profile`，不是 `provider`。不模糊比對、不自動剝除前綴。 |
| `aliases` | 選用的額外精確 model ID 清單。同一範圍內，每個名稱必須明確只屬於一個模型 family。 |
| `profile`、`endpoint` | 選用的精確選擇條件。Endpoint 是不含憑證、query 或 fragment 的絕對 HTTP(S) URL，比對的是有紀錄的上游中繼資料，不是 loopback 觀測 route。 |
| `tier` | 選用的服務層級條件，預設 `"standard"`。紀錄未帶 tier 時假設 standard；OpenAI 的 `"default"` 對應 standard。其他實際 tier 需要各自費率。 |
| `context_min_tokens`、`context_max_tokens` | 選用、含邊界的總輸入 token 範圍。選到的費率套用整個請求，不只邊際 token；累計差值無法選逐請求的範圍。 |
| `input`、`output`、`cache_read`、`cache_write` | 選用、非負的十進位**字串**，單位為每百萬 token 的 USD，最多六位小數。每筆至少要有一個明確費率。 |
| `cache_write_5m`、`cache_write_1h` | 依 TTL 分開的寫入費率，取代 `cache_write`；同一筆不可混用總寫入與 TTL 費率。計數取代總寫入，不再另收一次。 |

未知欄位、格式錯誤的費率、重疊的 context band、重複 tier，以及有歧義的
profile-only／endpoint-only 範圍都會被拒絕。更精確的 profile／endpoint 範圍可以
取代較廣的範圍。**選到的覆寫 family 會整體取代內建價格**：省略的費率、tier 與
context band 不會繼承備援價格。缺少計數、或正用量卻沒有費率時，仍是未知／部分
估算；只有明寫 `"0"` 才是免費費率。本機／自訂模型需要精確覆寫，除非支援的精確
官方端點能提供參考。覆寫不會編造未知的原生 provider／profile 中繼資料。

成本不含訂閱費、稅、折扣、工具與其他非 token 費用。它是 API-token 或
API-equivalent 參考估算，不是帳單。

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
