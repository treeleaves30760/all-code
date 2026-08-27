---
id: codex-to-claude
title: 用 GPT 模型跑 Claude Code
sidebar_label: 用 GPT 跑 Claude Code
sidebar_position: 4
description: 用 alc --codex claude 讓 Claude Code 透過你的 Codex 或 ChatGPT 登入使用 GPT-5.6 模型，並在工作階段中直接切換模型與推理強度。
keywords:
  - claude code 用 gpt
  - codex 訂閱 claude code
  - gpt-5.6
---

# 用 GPT 模型跑 Claude Code

這條路徑會讓 Claude Code 使用你的 Codex／ChatGPT 登入所能存取的 GPT 模型：

```sh
codex login
alc --codex claude
```

這個指令會直接啟動 Claude Code，不會先跳選單。三個模型都會出現在 Claude Code
自己的 `/model` 選單裡：

| 模型 | 適合的情境 | Codex 預設強度 |
| --- | --- | --- |
| `gpt-5.6-sol` | 能力最完整，適合架構、困難除錯與大型重構 | `low` |
| `gpt-5.6-terra` | 速度、能力、成本均衡，建議新手從這個開始 | `medium` |
| `gpt-5.6-luna` | 速度快、費用低，適合簡單修改與大量例行工作 | `medium` |

清單依能力由強到弱排列，與 Codex 對這個系列公布的分級一致。上游細節可參考 OpenAI 的
[模型選擇指南](https://developers.openai.com/api/docs/guides/latest-model)、
[Luna 說明](https://developers.openai.com/api/docs/models/gpt-5.6-luna)與
[Sol 說明](https://developers.openai.com/api/docs/models/gpt-5.6-sol)。

## 在工作階段中切換模型與強度

進到 Claude Code 後，用 `/model` 換模型，該畫面的左右方向鍵可調整推理強度；也可以
用 `/effort` 直接指定。每個模型都接受 `low`、`medium`、`high`、`xhigh`、`max`。
強度越高，模型思考的空間越大，但也會花更多時間與額度。

模型清單是透過 Claude Code 的
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
設定傳入，這個設定自 Claude Code 2.1.243 起提供。選單只會顯示這些 GPT 模型與
Default 一列，因為 Claude 自家的模型無法經由 Codex 轉接器服務；舊版會忽略這個設定，
仍可拿到啟動時的預設模型。因為 Claude Code 每次請求都會帶上模型與強度，alc 不會把
任何一項鎖在轉接器上。

## 設定啟動預設值

想單次換掉起始值，或用在腳本與 CI：

```sh
alc --codex claude --model gpt-5.6-luna --effort low
alc --codex claude --model gpt-5.6-terra --effort medium --save
```

`--save` 會把模型與 effort 存入目前的 Codex provider。沒有這些參數時，起始值依序
取自 alc provider、Codex profile、模型內建預設值。放在 `--` 之後的 `--model`、
`--effort`、`--settings` 會原樣交給 Claude Code，並蓋過 alc 的注入值。

在 Claude Code 裡用 `/model` 換的模型只影響那一次工作階段；下次執行
`alc --codex claude` 會再從 alc provider 的預設值開始，所以
[`alc config`](./configuration.md) 仍然是唯一的真實來源。

## 模型目錄

模型清單每天最多自動向本機 Codex CLI 同步一次；離線時會使用內建清單：

```sh
alc models
alc models --refresh
alc models --json
```

同步到的 Codex context window 也會透過 Claude Code 官方的
[`CLAUDE_CODE_MAX_CONTEXT_TOKENS`](https://code.claude.com/docs/en/env-vars)
設定傳入，讓 Claude Code 不認得的 GPT ID 依照 Codex 的實際上限壓縮對話，而不是用
它的通用預設值。

Claude Code 的內建別名也一併留在 Codex 上：選單的 Default 一列跟著 alc 的預設值，
`haiku` 與背景工作使用最便宜的模型，`sonnet` 跟著本次的起始模型，`opus` 使用最強的
模型。

## 轉接器如何運作

發行包會附帶
[`claude-codex` 0.3.1](https://github.com/fcakyon/claude-code-with-codex)，
一個 MIT 授權的 helper。`alc` 會把它綁在隨機的 `127.0.0.1` port，只讓那個 Claude
Code 子程序指向它，並在 Claude 結束時關閉。Helper 會讀取並可能更新
`~/.codex/auth.json`；憑證不會被複製到 alc 的設定檔。

:::caution 這是第三方相容層

這個轉接器不是 OpenAI 或 Anthropic 的官方整合。使用訂閱帳號前，請先檢閱專案的
`THIRD_PARTY.md` 與你的 provider 條款。

:::
