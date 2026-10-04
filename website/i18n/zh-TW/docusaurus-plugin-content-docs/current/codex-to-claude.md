---
id: codex-to-claude
title: Codex 橋接
sidebar_label: Codex 橋接
sidebar_position: 3
description: 一次 codex login，透過內建的橋接就能餵飽全部八個 coding agent —— Claude Code 有 session 內的 GPT 模型選單，其他 agent 則是每個 session 用一個橋接過的模型。
keywords:
  - claude code with gpt
  - codex subscription
  - chatgpt plan coding agent
  - gpt-6.1-sol
  - gpt-6-astra
  - gpt-6-sol
  - gpt-6-luna
  - reasoning effort
  - 中文
---

# Codex 橋接

`alc --codex <agent>` 會啟動一個 loopback 轉接器，並把某一個 agent 的 session
指向它。一次 `codex login` 就夠八個 agent 一起用。

```sh
codex login
alc --codex claude
alc --codex opencode      # or pi, copilot, goose, qwen, kimi
```

| Agent | 橋接提供給它的協定 | 它怎麼選模型 |
| --- | --- | --- |
| Claude Code | Anthropic Messages | `/model` 選單，session 中途也能換 |
| OpenCode、Pi、Kimi Code CLI | OpenAI Responses | 一個模型，啟動時選定 |
| Copilot CLI、Goose、Qwen Code | OpenAI Chat Completions | 一個模型，啟動時選定 |

只有 Claude Code 能在 session 中途切換，因為它每次請求都會把模型與推理強度一起
送出，所以 alc 兩者都不必釘在轉接器上。其他 agent 則是沿用它們各自處理
`openai` 這個 kind 的那套既有機制接上來，只是改指向轉接器、帶一個佔位用的
key —— 每個 agent 實際收到什麼，見[支援的 agent](./agents.md)。

## 模型

Claude Code 會把這幾個列進它自己的 `/model` 選單：

| 模型 | 適合的情境 | Codex 預設強度 |
| --- | --- | --- |
| `gpt-6.1-sol` | GPT-6.1 Sol。最新的主力模型，適合寫程式與日常工作，建議從這個開始 | `low` |
| `gpt-6-astra` | GPT-6。適合複雜吃重的工作 | `medium` |
| `gpt-6-sol` | GPT-6。日常寫程式與代理式工作 | `medium` |
| `gpt-5.6-sol` | 上一代，適合複雜的專業工作 | `low` |
| `gpt-5.6-terra` | 上一代，日常寫程式的均衡選擇 | `medium` |
| `gpt-5.6-luna` | 上一代，快、便宜 | `medium` |
| `gpt-6-luna` | GPT-6。快、最便宜，適合快速修正與大量的例行工作 | `medium` |

上游的最新細節可參考 OpenAI 的
[GPT-6.1 Sol 說明](https://developers.openai.com/api/docs/models/gpt-6.1-sol)與
[GPT-6 Luna 說明](https://developers.openai.com/api/docs/models/gpt-6-luna)。

橋接本身不保留任何允許清單：收到什麼 slug 就往上游送，由 chatgpt.com 決定，
所以 alc 沒有追蹤的模型，一樣可以用 `--model` 指名使用。這正是為什麼一個新模型
在 Codex 推出的那天就能用，而不是等 alc 追上的那天。

## 每一個 Claude 模型都變成 Codex 模型

在 `alc --codex claude` 底下，沒有任何一個請求會送到 Claude 模型：`/model`
選單列出這七個，以及同步時新增的模型。Claude Code 所有的別名 —— `opus`、
`sonnet`、`haiku`、`fable`、`best`、`opusplan` —— 都會落在 Codex 模型上，而
指名完整名稱的 Claude 模型，則由同一級的 Codex 模型來回答。`opus` 與 `fable`
使用 GPT-6.1 Sol；`haiku` 與 small-fast 工作維持 GPT-6 Luna；`sonnet` 跟著
session 的起始模型。Claude Code 自己的背景工作也走同一條路，所以一個標題或
一份摘要花掉的是 Codex 的額度，而不是一個送去 Anthropic
的請求。[背景
session](./background-sessions.md#每一個-claude-模型都變成-codex-模型)有整張
表，一個別名一個別名地列出來。

## 推理強度

`/model` 畫面的左右方向鍵可以移動強度滑桿，`/effort` 則能直接指定一個等級。
每個模型都接受 `low`、`medium`、`high`、`xhigh` 或 `max`。強度越高，模型思考的
空間越大，也越吃你的額度。

除了兩個 Luna 之外，每個模型都另外提供 `ultra`：原生的 `alc codex` 用得到，橋接
則不行。alc 會在啟動時把它降到 `max` 並明說，而不是讓請求在 session 進行到一半
才被拒絕。

## 換一個起點

沒有設定過的 session 會以 GPT-6.1 Sol、`low` 強度啟動。你已在 alc profile 或
Codex 設定裡選好的模型與強度仍然優先。

```sh
alc --codex claude --model gpt-6-luna --effort low
alc --codex claude --model gpt-6.1-sol --effort low --save
```

`--save` 會把兩者一起存進 provider profile。沒有這些參數時，session 的起始值
依序取自 profile 的設定、選定的 Codex profile，最後是模型自己的預設值。放在
`--` 之後的東西會原封不動交給 agent，而且蓋過前面所有來源。

用 `/model` 選的模型只對那一次 session 有效；下次啟動又會從 profile 的值開始。

## 你原本的 `claude` 仍然連得到 Anthropic

Claude Code 會把你最後停在的那個模型寫進 `~/.claude/settings.json`，當成之後
新 session 的預設值，而這台機器上每一個 session 都會讀那個檔案 —— 包含不是
alc 啟動的那些：它們前面沒有轉接器，卻會拿一個 GPT 模型去向 Anthropic 要。

alc 會在啟動前把那一個欄位讀下來，並在 session 結束時放回去。檔案裡其他東西
一概不動；而且除非它讀到的值只有轉接器服務得了，否則根本不會寫。

有兩種情況它刻意不碰。一是你在 session 中途自己切到某個真正的 Claude 模型：
那是你對自己預設值做的決定，就該算數。二是 session 被直接砍掉，任何收尾都不會
跑 —— 這時 `alc doctor` 會指出那個檔案與那一行，而下一次經過橋接的啟動會把它
清掉。

不要想用 `CLAUDE_CONFIG_DIR` 來隔離這件事：那會把 Claude Code 的整個設定家目錄
搬走，連登入資訊也一起搬走。

## 模型目錄

直接向你的 ChatGPT 帳號同步——也就是轉接器每一輪請求送去的同一方——所以只要那個
帳號跑得動的模型，即使已安裝的 Codex CLI 沒聽過，也照樣會出現。抓不到時才退回
`codex debug models`，而執行檔內建的那份清單是兩者都不能低於的底線：同步回來的
清單只能新增模型，不能拿掉。同步每天一次，Codex 一升級就會立刻再同步一次。

alc 宣告的 Codex client 版本至少是 `0.159.1`，也就是第一個內含 GPT-6.1 Sol 的
穩定版；已安裝的版本更新時，則使用那個版本。這個運作上的版本底線，與上游模型
那一列實際填的 `minimal_client_version` 值 `0.153.0` 是兩件事；不必只為了
看見內建的模型就升級 Codex。

```sh
alc models
alc models --refresh
alc models --json
```

同步到的 context window 會以
[`CLAUDE_CODE_MAX_CONTEXT_TOKENS`](https://code.claude.com/docs/en/env-vars)
傳給 Claude Code，讓 GPT 模型依 Codex 的實際上限壓縮對話，而不是照 Claude Code
對不認得的 ID 假設的 200k。

## Auto 模式與提示快取

不用額外設定或旗標。

**Auto 模式仍然可用。** Codex 無法執行 Claude Code 的伺服器端檢查，因此 Claude Code
會改用自己的分類器。分類器請求會經過 Codex，也會使用 Codex 額度。

**提示快取路由會自動設定。** alc 會讓同一個 Claude Code session 保持穩定的 Codex
快取路由。快取重用仍需要相同的提示前綴；切換模型或 effort、修改工具或提示、壓縮
對話、快取到期，以及 Claude Code 升級都可能造成未命中。

用 [`alc usage`](./usage.md) 查看 Codex 實際重用了多少輸入。`CACHED` 是從快取讀取的
輸入 token 數，`CACHE %` 是它占總輸入的比例，不是請求命中率。快取重用也不保證
減少額度消耗或費用。

升級 alc 後請開啟新的 session，才能取得這些預設值；已在執行的背景 session 會保留
啟動時的設定。你自己的 `--settings` 仍可覆寫 alc 的 Claude Code 預設值。

## 運作方式

橋接是 alc 自己的程式碼。給 Claude Code 用的時候，它是一個獨立的背景行程，綁在
一個它會一直留著的 loopback port 上（`alc bridge`），因為一個被移到背景的
session，活得比啟動它的那個 `alc` 還久 —— 見[背景
session](./background-sessions.md#背景橋接)。其他每一個 agent 則仍然跑在 `alc`
行程內、綁在隨機的 port 上，只服務它啟動的那個 agent，並在該 session 結束時
關閉。它會讀取、必要時更新 `~/.codex/auth.json`；憑證絕不會被複製進 alc 自己的
設定裡。

:::caution[這是第三方相容層]

這個轉接器不是 OpenAI 或 Anthropic 的官方整合。在把訂閱憑證交給它之前，請先
看過專案的 `THIRD_PARTY.md` 與你的 provider 條款。

:::
