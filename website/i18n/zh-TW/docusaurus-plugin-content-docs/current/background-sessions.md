---
id: background-sessions
title: 背景 session
sidebar_label: 背景 session
sidebar_position: 4
description: Claude Code 的 agent view、claude --bg 與 ← 在 alc 啟動的每一個 session 裡都能用，而且跑在 alc 給它的 provider 上 —— 在 alc --codex claude 底下，每一個 Claude 模型都變成 Codex 模型。
keywords:
  - claude code agent view
  - claude --bg
  - background agents
  - codex claude code
  - apiKeyHelper
  - 中文
---

# 背景 session

Claude Code 用 [agent view](https://code.claude.com/docs/en/agent-view) 把
session 放到背景跑：`claude agents` 派出它們並看著它們、`claude --bg` 從 shell
直接起一個，在空的提示列上按 `←` 則是把你正在用的這一個送過去。跑它們的是
Claude Code 自己的一個 supervisor，所以終端機關掉之後，它們還是繼續做事。

alc 啟動的每一個 Claude Code session 在那裡都能用，而且跑在 alc 給它的
provider 上：

```sh
alc --codex claude agents
alc --codex claude --bg "fix the flaky test"
alc --openrouter claude agents
```

一個被派出的 session 回答時用的是那個 session 當下的模型，不是啟動時就定死的
一個：`←` 帶走的是你正在進行的那段對話，所以一個在 `/model opus` 之後才送到
背景的 session，用的就是你剛剛選的那個模型。而在 `alc --codex claude` 底下，
不論哪一種情況，它們每一個都是 Codex 模型。

## alc 怎麼把 provider 交給一個 session

Claude Code 為背景 session 留下來的只有一樣東西：它啟動時收到的那串參數 ——
每次重新啟動那個 session，它都會再讀一次。所以 alc 改用一個設定檔交出
provider —— `--settings ~/.config/alc/claude/settings-<hash>.json` —— 而不是
原本那些環境變數，因為 supervisor 不會把它們帶著走。

| 檔案裡有什麼 | 它的作用 |
| --- | --- |
| `ANTHROPIC_BASE_URL` | 那個 provider 的 Anthropic 端點，Codex 則是 alc 的背景橋接 |
| 模型相關變數與 `modelPicker` | Claude Code 啟動時用哪個模型、選單裡又列出哪些 |
| `apiKeyHelper` | `alc claude-credential …`，Claude Code 會執行它來取得憑證 |

這個檔案裡永遠沒有金鑰。helper 印出來的是 alc 本來就存著的那一把 —— profile
的環境變數，或是用 `alc config key` 存下來的 key —— 若是 Codex，則是背景橋接
的 token。一把只活在某個 shell 環境裡的 key，在從那個 shell 啟動的 session 裡
有效；之後才開始跑的背景 session 可能看不到它，這時 helper 會叫你用
`alc config key <profile>` 把它存起來。它永遠不會退回去用你的 Claude 登入。

一個背景 session 會一直用著 alc 在啟動時合併好的那份設定。你自己傳
`--settings` 時，alc 會把它合併進自己寫出來的那份文件，衝突時以你的為準，因為
Claude Code 只讀一份 —— 所以你之後對自己那個檔案做的修改，只會傳到之後才啟動
的 session，不會傳到已經在跑的那些。

## 背景橋接

Codex 的轉接器現在是一個獨立的行程，因為背景 session 活得比啟動它的那個 `alc`
還久：

- 每一份 alc 設定一個，只綁 `127.0.0.1`，port 挑過一次之後就一直用它；
- 每一個模型請求都必須帶著它的 token；
- 需要它的 session 會透過 helper 把它叫起來；
- 閒置一小時後自己關掉，或用 `alc bridge stop` 關掉。

```sh
alc bridge          # running or not, pid, port, which alc started it
alc bridge stop     # the next session that needs it starts it again
```

`alc doctor` 的 **Background sessions** 區塊會顯示同樣的內容。

## 每一個 Claude 模型都變成 Codex 模型

在 `alc --codex claude` 底下，沒有任何一個請求會送到 Claude 模型：

| Claude Code 在哪裡挑模型 | 在 alc --codex claude 底下 |
| --- | --- |
| session 啟動時用的模型、`/model`、Default 那一列 | 只有 Codex 模型 |
| `opus`、`fable`、`best` | 能力最強的那個 Codex 模型 |
| `sonnet`，以及不在 plan 模式下的 `opusplan` | 這次 session 啟動時用的那個模型 |
| `haiku`，以及 Claude Code 自己的背景工作（標題、摘要、agent view 的那幾列） | 最便宜的那個 Codex 模型 |
| 指名完整名稱的 Claude 模型：`/model claude-opus-5`、subagent 的 `model:`、fallback 鏈 | 同一級的 Codex 模型 |
| `[1m]` 變體 | 比照不帶後綴的同一個模型，依 Codex 真正的 window 計算 |
| fast mode、advisor | 關閉：它們只存在於 Claude 模型上 |

`claude ultrareview` 與雲端 session 跑在 Anthropic 自己的伺服器上；它們仍然是
Anthropic 的功能。

## 管理 session

`alc claude attach <id>`、`logs`、`stop`、`respawn` 與 `rm` 都是直接交給 Claude
Code。直接用 `claude attach <id>` 一樣有效：那個 session 本來就帶著它的設定檔，
而把它喚醒時，需要橋接的話也會順手把橋接叫起來。

那些根本不會碰到模型的 Claude Code 指令 —— `mcp`、`doctor`、`plugin`、
`update`、`auth` 之類 —— 也一樣直接交出去。它們不會啟動橋接、不會寫設定檔，
也不會在 `alc usage` 裡被算成一個 session。

alc 自己的旗標放在 agent 名稱前面，Claude Code 自己的放在後面。兩邊搶同一個
寫法時 —— `-p`、`--name` —— 把 Claude Code 的那一個放到 `--` 之後：

```sh
alc --codex claude -- -p "fix the flaky test"
alc --codex claude -- --bg --name nightly "run the slow suite"
```

`--` 要緊接在 agent 名稱後面，排在它所有旗標之前。寫得比那更後面，它就會變成
一個參數本身，被傳給 Claude Code。

## 限制

- 在一個用普通 `claude attach` 接上的背景 session 裡用 `/model` 選模型，那個
  選擇會變成 Claude Code 之後新 session 的預設值，而現場沒有任何 alc 行程能把
  你原本的值放回去。`alc doctor` 會回報這件事，下一次 `alc --codex claude`
  會把它清掉。
- 萬一橋接非得換到另一個 port —— 它關著的時候，port 被別的程式佔走了 —— 還在
  用舊 port 的那些 session 重新啟動之後就會接上新的：`claude respawn <id>`，
  或 `claude respawn --all`。
