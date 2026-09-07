---
id: remote-control
title: 遠端控制
sidebar_label: 遠端控制
---

把執行中的 session 鏡像到一個網頁，然後從另一台裝置操作它 —— 不管哪一個 agent、
哪一個 provider，都是同一個頁面。

```sh
alc --share claude
alc share opencode -- --mini
```

alc 會印出連結：

```text
alc session claude-7QK2M9XB4T (claude@all-code)
  open  http://127.0.0.1:8787/#k=…
  bind  127.0.0.1:8787 · this link grants input; keep it to yourself
  pid   48213
```

你自己的終端機完全照舊。共享是把 session 鏡像出去，不是把它拿走。

## 為什麼八個 agent 都能用

alc 啟動的八個 agent 幾乎在每件事上都不一樣：有些提供機器可讀的控制通道，有些
完全沒有，而有提供的那些，在傳輸方式、session 識別、甚至「核准」是什麼，都彼此
不相容。它們唯一共有的東西是終端機本身，所以 alc 鏡像的就是終端機。這讓 session
不論跑在哪個 agent、哪個 provider 上，行為都一致 —— 包括那些 agent 自己的遠端功能
拒絕支援的 provider 組合。

## 頁面上有什麼

- **Session 清單**：agent、provider、模型、工作目錄。
- **即時畫面**：由真正的終端機模擬器繪製，所以全螢幕 TUI 看起來跟本機一樣。
- **快捷鍵列**：手機鍵盤沒有的那些鍵 —— Esc、Tab、Shift+Tab、Ctrl（黏著式，先按
  Ctrl 再按字母）、方向鍵。Claude Code 就是用 Shift+Tab 切換權限模式的。
- **輸入框**：把整段提示詞當成一次貼上送出。在手機上直接對著原始終端機打長提示詞
  等於跟自己的輸入法對抗 —— 輸入法會重寫它已經送出的字，而原始終端機收不回來。
- **重新連線不會失去位置**：關掉分頁、走進隧道、再回來 —— 頁面會要求它剛好漏掉的
  那些位元組；離開太久的話，改為直接取得目前畫面。

頁面會跟隨裝置語言，提供繁體中文與英文。

## 從手機連上

預設只綁 loopback。在你明說之前，什麼都不會對外開放。

### 自己跑的隧道（建議）

兩台裝置都裝 [Tailscale](https://tailscale.com/) 之後：

```sh
tailscale serve 8787
```

然後在手機上開那個 `ts.net` 位址。alc 永遠不必是面對網路的那一層，中間也沒有第三方
需要信任。

### 區域網路

這需要兩道開關，是刻意的。在 `remote.toml`：

```toml
allow_lan = true
bind      = "lan"
```

命令列上：

```sh
alc --share --bind-lan claude
```

單一開關太容易不小心留著沒關，而那個 socket 的另一端是一個 shell。

## 共享實際上授予了什麼

一個能對 coding agent 輸入的網頁，等於你機器上的遠端程式碼執行。這件事值得講清楚。

- **連結的 fragment 就是憑證。** 拿到 `#k=…` 那一段的人就能對 session 輸入。
  Fragment 永遠不會送到伺服器，所以不會出現在存取紀錄或 proxy 裡 —— 但它會在你的
  剪貼簿裡。把它當密碼看待。
- **alc 會比對 `Host` 到連接埠**，並要求 WebSocket 升級帶上 `Origin`。這正是用來
  阻擋「某個網域被解析到 `127.0.0.1`、然後透過你自己的瀏覽器操控你的 agent」的
  DNS rebinding 攻擊。Token 以固定時間比對。
- **共享的 session 就是螢幕分享。** alc 會遮蔽**它自己**放進環境變數的 API key，
  所以 agent 印出自己的環境變數時，不會把你的 provider key 散播給每一個觀看者。
  遮蔽適用於**每一個**視圖，包含你啟動 session 的那個終端機 —— 對所有觀看者用同一條
  規則比較好推理，而代價只是看不到自己的 key 被回顯出來。但 agent 印出的其他任何東西，
  觀看者都看得到。這一點無法解決，只能界定範圍。
- **alc 永遠不回應剪貼簿讀取請求。** 終端機可以被要求把觀看者的剪貼簿內容打回程式的
  輸入；否則 agent 印出的一個惡意檔案就能把你剛剛複製的東西拉進模型的 context。
  alc 拒絕，並且在頁面上說出來。
- **alc 不讀取工作目錄裡的任何設定。** 被 commit 進 repo 的檔案永遠無法開啟共享。

`--share` 需要兩端都是真正的終端機。輸入或輸出被重導向時它會拒絕，所以像
`alc claude -p "…" > out.txt` 這種腳本用法，行為跟今天完全一樣。

## 各 agent 的權限模式

八個 agent 對「權限模式是什麼」、「模式叫什麼名字」、甚至「啟動後能不能改」都
沒有共識。頁面會依照眼前這個 agent 真正能做的事來繪製控制項 —— 能直接指定的給
下拉選單，不能的給相對的「循環切換」按鈕，完全沒有這個概念的則顯示停用的控制項
並附上原因。

畫面上永遠同時顯示 **alc 的等級和 agent 自己的用語**，因為只給共用標籤會誤導：
`auto` 是 Goose 最寬鬆的設定，但在 Claude Code 是中階分類器，比
`bypassPermissions` **更嚴格**。

| Agent | alc 傳入的啟動旗標 | Session 中如何切換 | 已驗證 |
| --- | --- | --- | --- |
| claude | `--permission-mode plan\|manual\|acceptEdits\|auto\|bypassPermissions` | 只能 Shift+Tab 循環（`ESC [ Z`）—— 是相對的，所以頁面給「循環」按鈕，不給下拉選單 | ✅ 對照 `claude --help` |
| codex | `-s read-only\|workspace-write\|danger-full-access` **加上** `-a on-request\|never`，或 `--approve-for-me` | `/permissions` 開啟 Codex 自己的選單，需要人在終端機面板完成 | ✅ 對照 `codex --help` |
| opencode | `--agent plan\|build`、`--auto` | Tab 在 build ↔ plan 間切換 | ✅ 對照 `opencode --help` |
| goose | `GOOSE_MODE=chat\|approve\|smart_approve\|auto` | `/mode <名稱>` —— 直接指定 | 來自文件 |
| qwen | `--approval-mode plan\|default\|auto-edit\|auto\|yolo` | `/approval-mode <名稱>` | 來自文件 |
| kimi | `--plan`、`--yolo` | 只能重新啟動 | 來自文件 |
| copilot | `--mode plan\|interactive`、`--allow-all-tools` | `/permissions` 開啟選單 | 來自文件 |
| pi | — | **不支援。** Pi 在設計上就沒有權限模式、沒有 plan 模式、沒有權限提示、也沒有沙箱。控制項會停用並顯示這句話，卡片上帶紅色標記。 | — |

兩個值得知道的細節，因為它們正是那種會靜默過期的東西：

- Claude Code 的 CLI 吃 `manual`，沒有 `default`；它的 SDK 控制通道剛好相反。
  alc 兩種拼法都保留，不共用同一個常數 —— 共用會靜默弄壞其中一條路徑。
- `--full-auto` 出現在很多 Codex 文件裡，但 codex-cli 0.153.2 根本沒有這個旗標。
  alc 永遠不會送出它，而且有測試在把關。

**alc 只會對「已經對照真實 `--help` 確認過」的 agent 注入權限旗標。** 其餘的除非
你用 `--permission` 明確要求，否則 alc 不會動它們的參數。把猜的旗標名塞進 agent
的參數不會讓 session 變安全 —— 只會讓它啟動失敗。

alc 也會標示它對顯示內容的信心程度：`launched`（alc 自己傳的旗標，之後沒送過任何
東西）、`reported`（從 agent 自己的狀態列讀回來），或用 `?` 表示只是推測。

### 提高上限

`remote.toml` 的 `max_permission`（預設 `auto-edit`）是頁面自己能達到的最寬鬆模式。
收緊永遠不需要確認；超過上限的則會回傳一張票券：

```text
$ # 頁面顯示：alc confirm 7QK2M9XB4T
$ alc confirm 7QK2M9XB4T
granted: auto
the page can apply it once, within the next minute.
```

`alc confirm` 沒有終端機時會拒絕執行，所以確認必須來自坐在這台機器前的人 ——
不是拿到連結的人，也不是把指令 pipe 進 shell 的 agent。超過上限的等級**每一次**
都會被擋，不是只有第一次：alc 對「session 現在是什麼模式」的認知通常只是推測而非
事實，而一道依賴這個推測的關卡是可以被繞過的。

## Session 的生命週期

共享的 session 由 **hub** 擁有 —— 你第一次共享時 alc 會自動啟動的一個背景程序。
這就是「一個頁面看到所有 session」以及「session 活過啟動它的終端機」的來源。

```sh
alc claude --share           # 沒有 hub 就順便啟動一個
# ctrl-\ 然後 d              # 卸離；session 繼續跑
alc sessions                 # 有哪些在跑
alc attach 7QK2             # 從任何終端機接回去
alc kill 7QK2               # 停掉一個
alc rename 7QK2 review      # 改卡片名稱
alc hub status              # hub 在不在、頁面在哪
alc hub stop [--drain]      # 停掉 hub；--drain 連它的 session 一起停
```

Session id 長得像 `claude-7QK2M9XB4T`。指令接受任何不含歧義的前綴，也接受後半段
那串有辨識度的字元 —— `alc attach 7QK2` 就夠了，而且不分大小寫。

`alc hub stop` 在還有 session 在跑時會拒絕，除非你加 `--drain`，所以停 hub 不會
變成意外殺掉工作的方式。

### hub 怎麼處理你的環境

hub 是長期存在的，而且是由第一個執行 `alc --share` 的那個 shell 啟動的。如果 agent
被生在**它的**目錄、用**它的**環境，那從某個 repo 開的 session 就會偷偷去改另一個
repo —— 所以每個請求都帶著發出請求的那個 shell 的工作目錄和完整環境。從兩個專案開的
兩個 session，各自拿到自己的。

啟動時解析出來的變數仍然優先於你 shell 的：環境裡既有的 `ANTHROPIC_API_KEY`
不會蓋掉 alc 為那個 profile 解析出來的 provider key。

### 如果 hub 掛了

被直接砍掉的 hub（`kill -9`、重開機）會把頁面一起帶走，但在 unix 上 agent 本身是自己
的 session leader，會繼續執行、變成孤兒。alc 為每個 session 寫一筆記錄，下一個啟動的
hub 會清理上一個來不及清的東西 —— 特別是 Kimi builder 寫入 provider key 的那個暫存
檔，否則它會一直留在磁碟上。

hub 沒有終端機也沒有 log 檔：它能寫的東西，都不值得為此制定「log 裡有啟動環境」所需要
的那套遮蔽規則。當 hub 起不來時，就在前景跑它、直接看：

```sh
alc hub start --foreground
```

## 這個功能不做什麼

直說，因為之後才發現更糟：

- **核准提示是終端機文字，不是手機原生對話框。** 你看到的是 agent 自己的提示，用
  快捷鍵列回答。真正的 Approve/Deny 卡片需要每個 agent 的結構化通道，而只有一半的
  agent 有。
- **每個 operator 連結都能同時輸入。** 沒有「取得控制權」的仲裁 —— 兩個人拿著
  operator 連結會互相交錯輸入，就像兩個人共用一個 tmux pane。不是自己在操作的，
  請發 viewer 連結。
- **hub 掛掉帶走的是頁面，不是 agent。** 它們會繼續以孤兒狀態執行；下一個啟動的
  alc 會清理上一個留下的東西。Session 不會活過重開機。
- **alc 無法接管不是它啟動的 session。** 你自己手動開的 `claude` 對頁面是不可見的；
  請用 `alc --share` 啟動。
- **多數 agent 的權限模式狀態是「相信」而非「知道」。** 只有三個能被直接指定模式，
  其餘只能循環切換、交給它自己的選單，或根本不能改。每張卡片上的 `confidence`
  標示你正在看的是哪一種情況。
- **全螢幕 agent 在瀏覽器裡沒有捲動歷史。** Codex、OpenCode 和 Qwen 會進入替代螢幕，
  和本機行為完全一樣。`codex --no-alt-screen` 和 `opencode --mini` 在手機上好用得多，
  頁面也會這樣提示你。

## 指令

```sh
alc --share <agent>          # 鏡像這個 session
alc share <agent> -- <args>  # 不會有歧義的寫法
alc share <agent> --name x   # 自訂卡片名稱，取代 <agent>@<目錄>
alc --no-share <agent>       # 不論設定為何都不鏡像

alc remote status            # 開/關、如何綁定、檔案在哪
alc remote on
alc remote off
alc remote token --rotate    # 讓已經發出去的連結全部失效

alc --share --permission plan <agent>   # 以指定模式啟動
alc confirm <ticket>         # 核准頁面請求的權限變更
```

`--share` 是 alc 自己的旗標，所以必須放在 agent 的參數之前。放在後面的話，alc 會
告訴你，而不是把它傳給 agent：

```text
$ alc claude "review this" --share
error: `--share` is alc's own flag but it came after the agent's arguments,
where it would be passed to claude instead; put it before the agent name, or
use `alc share claude -- <args>`
```

## 設定

`remote.toml` 和 `config.toml` 放在一起 —— `alc remote status` 會印出路徑。它刻意
是獨立的檔案：`config.toml` 會拒絕它不認得的鍵，把這些設定放進去會讓每一個舊版
`alc` 都無法讀取同一個檔案。

| 鍵 | 預設 | 作用 |
| --- | --- | --- |
| `enabled` | `true` | 總開關。`alc remote off` 設定的就是這個。 |
| `bind` | `"loopback"` | `loopback` 或 `lan`。 |
| `allow_lan` | `false` | 必須為 true **且**傳入 `--bind-lan` 才會綁 LAN。 |
| `port` | `8787` | `0` 表示隨機連接埠。連接埠被佔用時會自動退回隨機。 |
| `allowed_origins` | `[]` | 額外允許的 origin，給隧道的主機名稱用。 |
| `extra_hosts` | `[]` | 額外允許的 `Host` 值，含連接埠。 |
| `scrollback_bytes` | `1048576` | 重新連線的觀看者最多能被精確補回多少位元組。 |
| `max_connections` | `64` | 同時服務的連線數。 |
