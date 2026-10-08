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
| `ANTHROPIC_BASE_URL` | 那個 provider 的 Anthropic 端點、alc 的 Codex 背景橋接，或選擇啟用的 API 觀測 route |
| 模型相關變數與 `modelPicker` | Claude Code 啟動時用哪個模型、選單裡又列出哪些 |
| `apiKeyHelper` | `alc claude-credential …`，Claude Code 會執行它來取得憑證 |

產生的 provider 設定裡沒有金鑰。一般直接 API-key session 的 helper 印出 profile
環境變數或 `alc config key` 裡的金鑰；Codex 協定轉譯則印出橋接 token。Claude
API-key `--metrics` 改為回傳下文說明的密封本機觀測憑證，不會透過 loopback HTTP
傳原始服務商金鑰。只在原本 shell 可用的 key，之後背景重啟可能讀不到；請用
`alc config key <profile>` 儲存。helper 永遠不會退回去用你的 Claude 登入。

一個背景 session 會一直用著 alc 在啟動時合併好的那份設定。你自己傳
`--settings` 時，alc 會把它合併進自己寫出來的那份文件，衝突時以你的為準，因為
Claude Code 只讀一份 —— 所以你之後對自己那個檔案做的修改，只會傳到之後才啟動
的 session，不會傳到已經在跑的那些。alc 新增的相容性預設值，也只會套用到升級後
才啟動的 session。

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

## 背景 session 的 API-key 量測

```sh
alc --openrouter --metrics claude agents
alc --openrouter --metrics claude --bg "fix the flaky test"
alc tps --filter-agent claude
```

一般 API-key session 維持直接連線。加上 `--metrics` 後，支援的 Claude Code
API-key 啟動改用持續運作的背景 host 上的**轉送觀測 route**，而不是歸啟動終端機
所有、短命的 listener。route 轉送 provider 原生的 Anthropic 相容協定，不是
Codex 協定轉譯器。新請求會記錄 [TTFT/TPS 與用量估算](./usage.md)的中繼資料，
包含 supervisor 重新啟動之後的請求。

持續保存的設定檔放 loopback 端點與 alc helper，不放金鑰。helper 每次重新啟動時
解析同一個 profile 的金鑰，需要時啟動 host，再用獨立、只有擁有者可讀的
`run/bridge.observer-key` 與新的 challenge 驗證 host。它只在記憶體註冊金鑰摘要，
回傳綁定固定 route 與本次 host instance 的 **AEAD 密封替代憑證**。host 只有在
派送到該固定上游端點時，才還原原始金鑰／header。觀測產生的檔案不寫入明文服務商
金鑰；target 註冊只保留摘要，不保留金鑰。host 重啟後，舊替代憑證會收到 HTTP
401；重新執行 helper 就會產生新 instance 對應的憑證。經驗證的握手／控制
challenge 不會公開觀測 secret。

route 檔只存固定的 profile/kind/upstream 中繼資料，所以只在原本 shell 裡可用的
金鑰，仍須用 `alc config key <profile>` 儲存，之後的背景重啟才能使用。修改或
停用 profile 的端點後需要重新啟動；helper 會拒絕把新金鑰送往舊 route。

保留的是原生 SDK、協定、模型與**上游驗證**，不是本機這一段的明文憑證。請求資料
平面仍是 loopback HTTP，不是 TLS，也不保證完整的本機機密性。必須信任本機行程：
替代憑證避免原始服務商金鑰外洩，但本機 port 被劫持時，仍可攔截明文請求內容。

Claude 的原生登入、不需要金鑰的端點，以及被覆寫的端點／驗證或 `apiKeyHelper`
設定都不在觀測範圍內；alc 無法安全使用自己管理的 API-key 設定位置時，明確要求
`--metrics` 會被拒絕。所選 `api_key_env` 為 `ANTHROPIC_API_KEY` 或
`ANTHROPIC_AUTH_TOKEN`、且已匯出非空值的 profile 也會被拒絕，因為它會繞過密封
helper：請用 `alc config key <profile>` 存金鑰、取消匯出該變數，再重新啟動；
一般沒加 metrics 的驗證行為不變。量測要求 host 具備經驗證的 `forward-observer-v2` 能力；
舊 daemon 即使 alc 版本字串相同也會被拒絕，此時執行 `alc bridge stop` 再重新
啟動。量測 dry-run 不啟動 host／listener，也不寫設定、route 或 observer-key 檔。
Codex 協定轉譯請求會自動觀測，不必加 `--metrics`；原生／只有歷史的紀錄無法提供
過去的 TTFT。

## 每一個 Claude 模型都變成 Codex 模型

在 `alc --codex claude` 底下，沒有任何一個請求會送到 Claude 模型：

| Claude Code 在哪裡挑模型 | 在 alc --codex claude 底下 |
| --- | --- |
| session 啟動時用的模型、`/model`、Default 那一列 | 只有 Codex 模型 |
| `opus`、`fable`、`best` | alc catalog 裡排第一的模型（目前是預設主力 GPT-6.1 Sol） |
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
