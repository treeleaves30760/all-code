---
id: troubleshooting
title: 疑難排解
sidebar_label: 疑難排解
sidebar_position: 8
description: alc 會印出哪些錯誤，以及各自要做什麼才會消失 —— 找不到 agent、provider 不相容、Codex 登入、Ollama 逾時，還有留在 Claude Code 設定裡的 GPT 模型。
keywords:
  - alc doctor
  - claude code error
  - codex login
  - ollama timeout
  - model not found
  - 中文
---

# 疑難排解

先跑 `alc doctor`。它會回報每個 agent 的執行檔、憑證狀態、每個 provider profile
以及它各自的 agent 相容性欄位、解析後的預設值，還有 Codex 的登入狀態。

## `'claude' is not installed or not on PATH`

alc 只啟動已經裝好的 agent。請先安裝那個 agent，或用[覆寫變數](./agents.md#執行檔覆寫)把
alc 指向某個執行檔。

## `provider '…' cannot be used with claude; Claude Code needs Anthropic Messages`

那個 profile 講的協定 Claude Code 用不了。請換一個 Anthropic 相容的端點，或改用
[`alc --codex claude`](./codex-to-claude.md)。對照表在 [Provider 相容性](./providers.md)。

## `provider '…' has no API key`

用 `alc config key <profile>` 存一把，或設定那個 profile 的 `api_key_env` 指名的
環境變數。

## `Codex credentials were not found`

執行 `codex login`，然後再試一次。[`alc usage`](./usage.md) 會告訴你哪些登入還有效。

## Ollama profile 出現 `API Error: Request timed out`（或 `500`）

模型沒能在 Claude Code 放棄之前，讀完那個 25k 到 40k tokens 的第一個請求。alc 會為
Ollama profile 設定 `API_FORCE_IDLE_TIMEOUT=0` 與 `API_TIMEOUT_MS=1800000`，讓它
願意等下去；而重試本來就會從 Ollama 的 prompt cache 接續，所以不管走哪一條路，
session 通常在第二次嘗試就會開始。

想讓第一輪不只是撐得過去，而是真的快，請看[本機模型](./local-models.md)。先看
`alc doctor` 的 **Ollama** 區塊：模型必須已經 pull 下來、能呼叫工具，而且 context
至少 64k。

## Ollama 回傳 `404 model 'claude-…' not found`

Claude Code 向伺服器要了一個它自己的 model ID，通常是透過它拿來做背景工作的
`haiku` 別名。alc 會把 Ollama profile 的每個別名都釘在 profile 的模型上；請把那個
profile 的 `small_model` 設成一個你真的 pull 下來的模型。

## 模型清單看起來過期了

模型目錄每天最多向已安裝的 Codex CLI 同步一次：

```sh
alc models --refresh
```

## `the model may not exist or you may not have access to it`

有兩種成因。

**留在 Claude Code 設定裡的 GPT 模型。** 一個 session 最後落在哪個模型，就會被寫進
`~/.claude/settings.json`，成為你之後新 session 的預設值，而之後直接跑的 `claude`
前面並沒有轉接器。alc 會在轉接過的 session 結束時把那個欄位放回去，所以你現在還找
得到的，是被直接砍掉的 session 留下的殘留，或是手動設進去的值。

```sh
alc doctor            # names the file and the line when it finds one
alc --codex claude    # clears it on exit; alc passes the model itself
```

**還在跑舊版本的 hub。** hub 的設計本來就是比終端機活得久，所以它也會比一次升級活
得久，而 alc 寧可拒絕，也不會把需要轉接的 session 交給版本不同的 hub。`alc doctor`
會指出落後的那一個：

```sh
alc hub stop
```

## `alc update` 找不到釋出的壓縮檔

1.4.0 之前裝好的版本，會去找一個已經不再發布的第二個執行檔。重新跑一次安裝器就好，
它會把整份安裝換掉。

## 在 `alc config` 裡找不到共享設定

它在第三個畫面 —— `Tab`、`Shift+Tab` 或直接按數字鍵，就能在 `1 Providers`、
`2 Agent defaults` 與 `3 Sharing & remote` 之間切換。預設共享、綁定位址與權限上限
都在那裡。

在 TUI 之外，`alc remote auto-share on` 設定的是同一個東西，`alc remote status`
會回報它。某一列顯示 `on (inactive)`，代表共享本身是關的：請執行 `alc remote on`。

## 輸出裡的機密資料

`alc --dry-run` 會遮蔽 API key 與 auth token，`alc config show` 只會顯示某個 profile
有沒有金鑰，而 `alc usage` 不管哪一種憑證都不會印出來。
