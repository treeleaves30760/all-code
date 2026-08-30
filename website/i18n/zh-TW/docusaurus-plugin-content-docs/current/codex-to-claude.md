---
id: codex-to-claude
title: Codex 橋接
sidebar_label: Codex 橋接
sidebar_position: 4
description: 透過隨附的橋接，一次 codex login 就能讓全部八個 coding agent 使用你的 Codex／ChatGPT 登入 —— Claude Code 有工作階段內的 GPT 模型選單，其他每個 agent 則是每個工作階段使用一個橋接模型。
keywords:
  - claude code 用 gpt
  - codex 訂閱
  - gpt-5.6
  - claude code 模型選單
  - codex bridge
---

# Codex 橋接

一次 `codex login` 就能讓 alc 啟動的每個 agent 使用。`alc --codex <agent>`
會在 loopback port 上啟動內建的 `claude-codex` 轉接器，並讓那一個 agent 的
工作階段指向它 —— OpenCode、Pi、Copilot CLI、Goose、Qwen Code、Kimi Code
CLI 都不需要另外登入。

```sh
codex login
alc --codex claude
alc --codex opencode
alc --codex pi
alc --codex copilot
alc --codex goose
alc --codex qwen
alc --codex kimi
```

轉接器會依 agent 不同，說不同的 wire protocol，但背後都是同一個 Codex 登入：

| Agent | 橋接提供的 wire protocol |
| --- | --- |
| Claude Code | Anthropic Messages |
| OpenCode、Pi、Kimi Code CLI | OpenAI Responses |
| Copilot CLI、Goose、Qwen Code | OpenAI Chat Completions |

Claude Code 是唯一能在工作階段中切換的 agent：因為它每次請求都會帶上模型與
推理強度，alc 從不會把任何一項鎖在轉接器上，`/model`／`/effort` 可以直接
變更正在執行的工作階段（詳見下方）。其他每個 agent 都是在啟動時就選定一個
模型 —— 以及該次工作階段固定的一個推理強度 —— 並使用各自的機制，而不是
選單。

## Claude Code

Claude Code 會直接以你儲存的預設值啟動，並把所有模型都放進它自己的 `/model`
選單：

| 模型 | 適合的情境 | Codex 預設強度 |
| --- | --- | --- |
| `gpt-5.6-sol` | 能力最完整，適合架構、困難除錯與大型重構 | `low` |
| `gpt-5.6-terra` | 速度、能力、成本均衡，建議新手從這個開始 | `medium` |
| `gpt-5.6-luna` | 速度快、費用低，適合簡單修改與大量例行工作 | `medium` |

清單依能力由強到弱排列，與 Codex 對這個系列公布的分級一致。上游細節可參考
OpenAI 的
[模型選擇指南](https://developers.openai.com/api/docs/guides/latest-model)、
[Luna 說明](https://developers.openai.com/api/docs/models/gpt-5.6-luna)與
[Sol 說明](https://developers.openai.com/api/docs/models/gpt-5.6-sol)。

### 在工作階段中切換模型與強度

進到工作階段後，用 `/model` 換模型，該畫面的左右方向鍵可調整推理強度；也可以
用 `/effort` 直接指定等級。每個模型都接受 `low`、`medium`、`high`、`xhigh`、
`max`。強度越高，模型思考的空間越大，但也會花更多時間與額度。

alc 會透過 Claude Code 的
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
設定傳入模型清單，這個設定自 Claude Code 2.1.243 起提供。選單只會顯示這些
GPT 模型與 Default 一列，因為 Claude 自家的模型無法經由 Codex 轉接器服務；
舊版會忽略這個設定，仍可拿到啟動時的預設模型作為可選項目。

### 設定啟動預設值

想單次換掉起始值，或用在腳本與 CI：

```sh
alc --codex claude --model gpt-5.6-luna --effort low
alc --codex claude --model gpt-5.6-terra --effort medium --save
```

`--save` 會把模型與 effort 存入目前選定的 alc provider。沒有這些參數時，
起始值依序取自 alc provider、選定的 Codex profile、模型內建預設值。放在
`--` 之後的 `--model`、`--effort`、`--settings` 會原樣交給 Claude Code，並
蓋過 alc 的注入值。

用 `/model` 選的模型只影響那一次 Claude Code 工作階段。下次執行
`alc --codex claude` 會再從 alc provider 的預設值開始，所以
[`alc config`](./configuration.md) 仍然是唯一的真實來源。

## OpenCode、Pi、Kimi Code CLI

這三個 agent 會直接使用轉接器的 OpenAI Responses 介面。每一個都會在啟動時
選定一個模型與一個推理強度（採用 alc provider 設定的值，或模型目錄的預設
值），並用各自的機制接上，而不是工作階段內的選單：OpenCode 會在
`OPENCODE_CONFIG_CONTENT` 裡拿到一個 `alc-codex` provider，Pi 會拿到合併進
`models.json` 的一筆 `alc-codex` 項目，Kimi Code CLI 則會在暫時的
`--config-file` 裡拿到一個 `alc-codex` provider。三者非橋接相關的細節，請見
[支援的 agent](./agents.md)。

```sh
alc --codex opencode
alc --codex pi
alc --codex kimi
```

## Copilot CLI、Goose、Qwen Code

這三個 agent 會使用轉接器的 OpenAI Chat Completions 介面，透過它們原本用在
`openai` 這個 provider kind 上的同一套機制接上 —— Copilot CLI 用
`COPILOT_PROVIDER_*` 環境變數、Goose 用 `OPENAI_*`、Qwen Code 用 `OPENAI_*`
加上 `--auth-type openai` —— 只是改指向 loopback 轉接器，並帶一個佔位用的
key，而不是真正的 key。

```sh
alc --codex copilot
alc --codex goose
alc --codex qwen
```

## 模型目錄

模型清單每天最多自動向本機 Codex CLI 同步一次；離線時會使用內建清單：

```sh
alc models
alc models --refresh
alc models --json
```

同步到的 Codex context window 也會透過 Claude Code 官方的
[`CLAUDE_CODE_MAX_CONTEXT_TOKENS`](https://code.claude.com/docs/en/env-vars)
設定傳入，讓 Claude Code 不認得的 GPT ID 依照 Codex 的實際上限壓縮對話，而
不是用它的通用預設值。

Claude Code 的內建別名也一併留在 Codex 上：選單的 Default 一列跟著 alc 的
預設值，`haiku` 與背景工作使用最便宜的模型，`sonnet` 跟著本次的起始模型，
`opus` 使用最強的模型。

## 橋接如何運作

發行包會附帶
[`claude-codex` 0.3.1](https://github.com/fcakyon/claude-code-with-codex)，
一個 MIT 授權的 helper。`alc` 會把它綁在隨機的 `127.0.0.1` port 上，只讓
啟動的那個 agent 行程指向它，並在該行程結束時關閉。Helper 會讀取並可能
更新 `~/.codex/auth.json`；憑證不會被複製到 alc 的設定裡。

:::caution 這是第三方相容層

這個轉接器不是 OpenAI 或 Anthropic 的官方整合。使用訂閱帳號前，請先檢閱
專案的 `THIRD_PARTY.md` 與你的 provider 條款。

:::
