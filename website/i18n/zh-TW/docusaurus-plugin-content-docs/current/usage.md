---
id: usage
title: 用量
sidebar_label: 用量
sidebar_position: 6
description: 看每個 Claude 與 Codex 登入還剩多少額度、每個 API key provider 還剩多少，以及是哪個 agent 用掉的 —— 在終端機裡、在遠端控制頁面上，或輸出成 JSON。
keywords:
  - alc usage
  - claude code usage
  - codex quota
  - chatgpt plan limit
  - openrouter credits
  - multiple accounts
---

# 用量

每個登入還剩多少，以及是哪個 agent 用掉的。

```sh
alc usage
```

```text
Accounts
     PROFILE     ACCOUNT                  PLAN  REMAINING
  ✓  anthropic   ~/.claude                max   5h 97% left, resets in 2h 53m — week 79% left, resets in 6d 4h — Fable week 62% left, resets in 6d 4h
  ✓  codex       you@example.com          pro   week 66% left, resets in 5d 9h — no credits
  ·  ollama      —                        —     no quota API
  ·  openrouter  —                        —     no API key; run `alc config key openrouter`

Usage by provider and agent
  PROVIDER  AGENT     LAUNCHES  TURNS  INPUT  OUTPUT  LAST
  codex     claude    1         1      20.8K  35      7m ago
  ollama    opencode  1         —      —      —       12m ago
  source: ~/.config/alc/usage.jsonl — tokens are counted only where alc carries the traffic; a direct launch counts as a launch alone

✓ ready
```

每個啟用的 provider profile 一列。`REMAINING` 是倒數的：`63% left` 指的是你還剩
多少，不是你花掉了多少。當登入過期或被拒絕，或是連不上服務商、服務商回了錯誤時，
結束碼是 1；其餘情況都是 0，所以單純把方案用完並不會讓腳本失敗。

`alc usage --json` 會把同一份報告印成 JSON。`alc --provider codex-work usage` 或
`alc --codex usage` 則把範圍縮到單一 profile 或單一種類。

## Codex 登入

alc 讀 `codex login` 寫下的 `auth.json`，然後向 chatgpt.com 問這份登入還剩多少。
它不寫回任何東西：OpenAI 的 refresh token 是一次性的，一個只是查狀態的指令若把它
換掉，正在執行的 session 手上那份就會失效。過期的 token 會直接叫你執行
`codex login`，而下一次真正的 session 本來就會替你更新它。

## Claude 登入

alc 讀 Claude Code 自己的登入 —— 先找 macOS 鑰匙圈，再找它設定目錄裡的
`.credentials.json` —— 然後向 api.anthropic.com 問五小時與每週這兩個額度視窗，
方案裡有的話再多問一個各別模型的視窗。只讀，永遠不更新。macOS 可能會問你一次，
要不要允許讀取那個鑰匙圈項目。

改用 API key 的 Anthropic profile 則顯示 `no quota API`：那把金鑰是按 token 計費的，
沒有「剩餘多少」這回事可以回報。

## 同一種的多個登入

兩個 ChatGPT 登入就是兩個 profile。讓每個 profile 指向自己的目錄，之後每一次從那個
profile 啟動都用那個帳號 —— 於是你讀到的那一列，就是你實際花掉的那個帳號：

```sh
CODEX_HOME=~/.codex-work codex login
alc config upsert codex-work --kind codex --codex-home ~/.codex-work
alc --provider codex-work claude
```

Claude Code 也一樣，用它存放登入的那個目錄：

```sh
CLAUDE_CONFIG_DIR=~/.claude-work claude          # sign in once
alc config upsert anthropic-work --kind anthropic --claude-config-dir ~/.claude-work
```

兩個路徑都必須是絕對路徑。profile 自己的目錄優先於你 shell 裡的 `CODEX_HOME` 或
`CLAUDE_CONFIG_DIR`，所以留在 shell 設定檔裡的一個變數，沒辦法偷偷改掉一個具名
profile 花的是哪個帳號。

`--provider` 與各種捷徑參數會先比對 profile 名稱，再比對種類。所以同時有 `codex`
與 `codex-work` 兩個 profile 時，`alc --codex claude` 會解析到名字就叫 `codex`
的那一個，而不會反問你指的是哪一個 —— 另一個請照上面的寫法指名。只有在沒有任何
profile 叫這個種類的名字、而又有好幾個是同一種時，捷徑才會拒絕替你選。

## 有餘額 API 的 provider

| Kind | 那一列會顯示什麼 |
| --- | --- |
| `openrouter` | 額度上限、已用與剩餘 |
| `deepseek` | 餘額，以這個帳號計費的幣別顯示 |
| `moonshot` | 可用餘額 |
| `minimax` | 每個視窗剩下的請求數 |
| `zai` | token 視窗與點數餘額 |

其餘的 —— OpenAI、Groq、xAI、Google、Ollama、vLLM、llama.cpp、自訂端點 —— 一律回報
`no quota API`，因為沒有一家為 API key 提供這種查詢。這些查詢一定是送到廠商自己的
端點，所以指向 proxy 的 profile 會被回報成沒有額度 API，而不是把金鑰送去一台並非
發出這把金鑰的主機。

## 各 provider 與 agent 的用量

每一次啟動都會往[設定目錄](./configuration.md)裡的 `usage.jsonl` 追加一行；每一個
由 [Codex 橋接](./codex-to-claude.md)承載的 turn 也會追加一行，帶著 chatgpt.com
回報的 token 數。資料來源就只有這個檔案。

alc 從來沒有承載過流量的那一組 provider 與 agent，token 欄位顯示的是 `—` 而不是
0：`alc claude` 走 Anthropic 時是直接跟 Anthropic 講話，alc 根本看不到那些 turn。
想重新開始計算，把這個檔案刪掉就好。

## 在遠端控制頁面上

[遠端控制頁面](./remote-control.md)標題列的用量按鈕後面是同樣的兩個區塊：每個額度
視窗一條量表，接著是那份帳本。那個面板開著的時候，它每分鐘更新一次。

只能看、不能輸入的連結看得到數字，但看不到 email、帳號 id 與憑證路徑。頁面是由 hub
提供的，而 hub 不讀任何 shell 變數，也永遠不會去開鑰匙圈 —— 所以在 macOS 上，那裡的
Claude 那一列會請你回到終端機執行 `alc usage`。
