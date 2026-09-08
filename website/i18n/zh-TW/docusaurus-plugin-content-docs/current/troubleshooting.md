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

從 1.4.0 起不會發生：橋接是編進 `alc` 裡的，不再是旁邊的另一個執行檔。如果你
是在舊版 `alc` 上看到這個訊息，請用一行安裝器升級。

## Ollama profile 出現 `API Error: Request timed out`（或 `500`）

Claude Code 會放棄六分鐘內還沒開始回應的請求並重試；Ollama 則把被放棄的請求
記成 `500`。原因單純是模型來不及在時限內讀完 Claude Code 的第一個請求
（25k 到 40k tokens）。現在的 alc 會為 Ollama profile 設定
`API_FORCE_IDLE_TIMEOUT=0` 與 `API_TIMEOUT_MS=1800000`，讓 Claude Code 改為
等待（如果還看到六分鐘的截止，請更新 alc）；沒有這兩個變數時，重試會從
Ollama 的 prompt cache 接續，工作階段通常在第二或第三次嘗試時才開始。要讓
第一輪一次就快：

- 看 `alc doctor` 的 **Ollama** 區塊：模型必須已 pull、能呼叫工具，
  而且 context 至少 64k。
- 啟動時少掛一些 MCP server、plugin 和 skill；每一個都會把工具 schema
  加進第一個請求，讀取時間隨長度增加。
- 小機器上把 Ollama 的 context 長度維持在 64k–128k，不要開到模型的上限，
  並用 `OLLAMA_KEEP_ALIVE=4h` 讓模型保持載入，prompt cache 才能跨輪保留。
- 先讓進行中的 `ollama pull` 跑完，並關掉其他吃記憶體的程式。

見[在本機 Ollama 模型上跑 Claude Code](./providers.md#在本機-ollama-模型上跑-claude-code)。

## Ollama 回傳 `404 model 'claude-…' not found`

Claude Code 向伺服器要求了它自己的 model ID —— 通常是背景工作用的 `haiku`
別名，或 `/model` 選單裡的某一列。現在的 alc 會把 Ollama profile 的每個別名
都釘在 profile 的模型上；請更新 alc，或把 profile 的 `small_model` 設成
你已經 pull 下來的模型。

## 模型清單看起來過期

模型目錄每 24 小時最多向本機 Codex CLI 同步一次：

```sh
alc models --refresh
```

## 輸出裡的機密資料

`alc --dry-run` 會遮蔽 API key 與 auth token；`alc config show` 不會印出憑證內容，
只會顯示每個 profile 有沒有設定。

## 在 `alc config` 裡找不到共享設定

它在第三個畫面。`alc config` 的標題列會列出三個畫面 —— `1 Providers`、
`2 Agent defaults`、`3 Sharing & remote` —— 用 `Tab`、`Shift+Tab` 或直接按數字鍵
就能切換。預設共享、綁定位址與權限上限都在第三個畫面裡。

在 TUI 之外，`alc remote auto-share on` 設定的是同一個值，`alc remote status`
與 `alc doctor` 都會顯示它，`alc config show` 則會印在 `# Remote control` 底下。

如果那一列顯示 `on (inactive)`，代表共享本身是關的：一個 session 要兩者都開才會
預設共享。把上面那列的 `sharing` 打開，或執行 `alc remote on`。

## `the model may not exist or you may not have access to it`

在 `--codex` session 裡看到這句，通常代表模型是真的存在，只是內建的
`claude-codex` 橋接還不能轉送它 —— Codex 推出新模型，總是比橋接學會服務它早一些。
alc 現在會在啟動時就擋下來，並列出橋接真的能轉送的模型，挑一個即可：

```sh
alc config upsert codex --model gpt-5.6-terra
```

如果 codex profile 的 model 是空的，它會改用 Codex CLI 自己的 `model` 設定，
而那通常正是不能轉送的那個來源。原生的 `alc codex` 不受影響。
