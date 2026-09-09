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

- **Session 清單**：agent、provider、模型、工作目錄、已執行多久、有幾個人在看。
  亮著的圓點代表執行中；結束的 session 會變灰、顯示它是怎麼結束的，然後自己從
  清單上消失。
- **即時畫面**：由真正的終端機模擬器繪製，所以全螢幕 TUI 看起來跟本機一樣。
- **快捷鍵列**：手機鍵盤沒有的那些鍵 —— Esc、Tab、Shift+Tab、Ctrl（黏著式，先按
  Ctrl 再按字母）、方向鍵。Claude Code 就是用 Shift+Tab 切換權限模式的。
- **輸入框**：把整段提示詞當成一次貼上送出。在手機上直接對著原始終端機打長提示詞
  等於跟自己的輸入法對抗 —— 輸入法會重寫它已經送出的字，而原始終端機收不回來。
- **重新連線不會失去位置**：走進隧道再回來 —— 頁面會要求它剛好漏掉的那些位元組；
  離開太久的話，改為直接取得目前畫面。標題列的燈號會顯示目前是連線中、重新連線中
  還是已結束。
- **重新整理不會弄丟清單**：連結裡的 token 會保留到分頁關閉為止，所以按 F5 仍然
  是登入狀態。關掉分頁就會失效，請用 `alc sessions` 取得新的連結。

頁面會跟隨裝置語言，提供繁體中文與英文。

## 從手機連上

預設只綁 loopback。在你明說之前，什麼都不會對外開放。

### Tailscale

兩台裝置都裝 [Tailscale](https://tailscale.com/) 之後，alc 維持只綁 loopback，
由 Tailscale 負責對外：

```sh
alc remote allow-host box.tail1a2b.ts.net   # 你這台機器在 tailnet 上的名字
tailscale serve 8787
alc claude --share
```

在手機上開那個 `ts.net` 位址。中間沒有第三方，而且連線是 HTTPS，token 不會以明文
出現在網路上。

### 自己的 Wi-Fi（LAN）

最直接，而且什麼都不用裝：

```sh
alc claude --share --bind-lan
```

alc 會印出這台機器自己的位址 —— `http://192.168.1.42:8787/#k=…` —— 同一個網路上的
手機直接開就好。也可以設定一次就好：

```toml
# remote.toml
bind = "lan"
```

要知道的一件事：這是純 HTTP，所以 token 會以未加密的形式經過你的區域網路。在家裡或
辦公室的網路通常沒問題；在咖啡廳的 Wi-Fi 上請改用隧道。

### Cloudflare Tunnel

不需要 VPN，從任何地方（包含行動網路）都連得到：

```sh
alc remote allow-host '*.trycloudflare.com'
cloudflared tunnel --url http://127.0.0.1:8787
alc claude --share
```

`cloudflared` 會印出一個 `https://<三個隨機英文字>.trycloudflare.com` 位址。這裡用
萬用字元是因為 quick tunnel 每次執行都會產生新的主機名 —— 否則你會在最想趕快連上的
那一刻被迫回頭改 alc 的設定。如果你用的是具名隧道加自己的網域，就精確允許那個主機名。

Cloudflare 會終結 TLS，所以和另外兩種方式不同，中間有一個第三方看得到流量。如果這對你
重要，在前面加一層 Cloudflare Access。

### alc 怎麼決定要回應哪些名字

alc 會比對 `Host` 標頭是否在允許清單上，而 `Origin` 則是「它的主機部分在同一份清單上」
才允許。Loopback 一定在清單上；加了 `--bind-lan` 時這台機器自己的位址也會在；其餘的用
`alc remote allow-host` 加。

```sh
alc remote allow-host box.tail1a2b.ts.net   # 精確
alc remote allow-host '*.trycloudflare.com' # 任意子網域
alc remote status                           # 目前會回應哪些名字
```

這個檢查不是形式。攻擊者的網頁可以把 `evil.com` 指向 `127.0.0.1`，用你自己的瀏覽器去
操控你的 agent；瀏覽器認為自己在跟誰講話，是它偽造不了的那一部分 —— 所以一個主機名只有
在你說可以的時候才被允許。

## 找回連結

`alc <agent> --share` 印出的連結，會在 agent 畫出自己的介面那一刻捲走。它是找得回來的：

```sh
alc remote url        # 只印連結
alc sessions          # 連結，然後是有哪些在跑
```

```text
$ alc sessions
page  http://192.168.1.42:8787/#k=…
      https://box.tail1a2b.ts.net/#k=…

claude-7QK2M9XB4T      claude   running   ask        ~/src/all-code
codex-68B8XMJ6F5       codex    running   plan       ~/src/api
```

每一個你允許過的名字都會有一行，所以要在手機上開哪一個不用去回想。萬用字元是一個
樣式而不是一個名字，所以它變不成連結 —— 請用隧道自己印出來的位址。

Token 就在那段輸出裡，這表示它會留在你的 shell 捲動歷史中。那和它第一次被印出來的
地方是同一個，而一個找不回來的 token 是沒有人能用的功能。
`alc remote token --rotate` 會讓已經發出去的連結全部失效。

## 每個 session 都自動共享

如果你幾乎總是想要那個頁面，說一次就好：

```sh
alc remote auto-share on     # `alc claude` 現在等同於 `alc claude --share`
alc --no-share claude        # 讓單一次啟動不共享
```

`alc config` 裡也有 —— 標題列會列出三個畫面，用 `Tab`／`Shift+Tab` 切到
**Sharing & remote**，共享、預設共享、綁定位址、權限上限都可以在那裡改。

腳本式的執行 —— 也就是輸入或輸出被重導向的那種，像 `alc claude -p "…" > out.txt`
—— 不論這個設定為何都**不會**共享。一個長期偏好不該成為某個 cron job 開始失敗的原因。
在那種情況下明確傳 `--share` 仍然會大聲失敗，因為那是使用者要求了 alc 做不到的事。

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

## 用 `--tmux` 讓兩邊各有自己的寬度

被共享的 session 是一個終端機、兩個觀看者，而一個終端機只有一種尺寸。瀏覽器和你的
終端機會輪流去設定它：誰最後改變大小誰說了算，另一邊就得用它其實沒有的寬度去畫一個
全螢幕 TUI —— 邊框折行、線條重複、游標跑掉。只要兩邊不一樣寬，其中一邊看起來就是
壞的。

`--tmux` 改成讓 agent 跑在 tmux 裡。tmux 正是為此而生的工具：一個程式，多個各自
附著的 client，每個都有自己的尺寸。

```sh
alc --share --tmux --codex claude    # 或 -t
```

你的終端機以一個 tmux client 附著，hub 以另一個附著，於是誰都不必遷就誰。

### 有什麼不同

| | 不加 `--tmux` | 加了 `--tmux` |
| --- | --- | --- |
| 尺寸 | 誰最後改變誰說了算，兩邊共用 | 你的終端機決定，頁面跟著走 |
| 卸離 | `ctrl-\` 然後 `d` | `ctrl-b` 然後 `d` |
| 捲動紀錄 | 頁面的 | 頁面的，外加你終端機裡 tmux 自己的複製模式 |
| 你本機看到的畫面 | 經過 hub 鏡像，金鑰被遮蔽 | 直接的 tmux client，原始輸出 |

最後一列值得看兩次。不加 `--tmux` 時，這個 session 沒有任何未經遮蔽的視角：你自己
的終端機讀的是和瀏覽器同一份被遮蔽過的串流。加了 `--tmux` 之後，你的終端機是一個
tmux client，所以它顯示的是 agent 真正印出來的東西 —— 包括 agent 如果把 alc 放進
環境的 API key 回顯出來的話。**瀏覽器那邊仍然是遮蔽過的。**

### 需求與限制

- tmux **3.2 以上**。`alc doctor` 會回報它找到的版本，而且是一列資訊而不是一個
  問題 —— 沒裝 tmux 的機器並不是壞掉的機器。
- `--tmux` 只對被共享的 session 有意義。沒有鏡像就只有一個觀看者，也就沒有什麼好
  吵的，所以 alc 直接拒絕這個旗標，而不是長出一條答案更差的程式路徑。
- 和遠端控制的其他部分一樣，支援 macOS 與 Linux。

alc 會為每個 session 開自己的 tmux server，用自己的 socket，以 session id 命名，
而且**完全不讀設定檔**。你原本的 tmux —— 設定、按鍵綁定、session —— 完全不會被
動到；alc 的 session 對每個人的行為都一樣，所以 prefix 就是 `ctrl-b`，不管你自己
綁了什麼；`~/.tmux.conf` 也永遠不會變成在某個 session 的 server 裡執行指令的途徑
—— 這件事有意義，因為 agent 寫得了那個檔案。第二次 `alc --tmux` 也絕不會接到第一次
那個 agent 上；在 tmux 裡面跑 alc 也沒問題。`alc sessions` 會標示哪些是 tmux
session，`alc kill` 停的是 agent 而不只是鏡像。

從**頁面**來的按鍵是直接送進 agent 的 pane，而不是打進鏡像那個 tmux client，所以
觀看者沒辦法用 prefix 鍵叫出 tmux 自己的指令列、拿到一個權限上限看不到的 shell。
你自己的終端機是完整的 tmux client，可以。

不過它不是 alc 和 agent 之間的界線。pane 連得到自己所在的 server，所以一個本來就
能執行 shell 指令的 agent 也能對自己打字；alc 會把 `TMUX` 從 pane 的環境裡拿掉、把
憑證從 server 的環境裡拿掉，但一個能執行任意指令的 agent 從來就不是權限閘門攔得住
的東西。

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

- **目前不支援 Windows。** `alc <agent> --share` 和 `alc hub` 系列指令在 Windows 上
  會直接拒絕並說明原因。Hub 會啟動一個分離的程序並透過 loopback 控制通道跟它溝通；
  在 Windows 上那個程序起不來、也不會結束，而發布一個會卡住並留下殘留程序的 `--share`
  比不發布更糟。alc 的其他功能在 Windows 上都正常。

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
alc --share --tmux <agent>   # ……用 tmux，兩邊各保有自己的尺寸
alc share <agent> -- <args>  # 不會有歧義的寫法
alc share <agent> --name x   # 自訂卡片名稱，取代 <agent>@<目錄>
alc --no-share <agent>       # 不論設定為何都不鏡像

alc remote status            # 開/關、如何綁定、檔案在哪
alc remote on
alc remote off
alc remote token --rotate    # 讓已經發出去的連結全部失效
alc remote url               # 連結捲走之後再拿一次
alc remote auto-share on     # 每個 session 都共享
alc remote allow-host <host> # 回應隧道的名字

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
| `auto_share` | `false` | 每個 session 都共享，不必加 `--share`；需要 `enabled` 也是開的才生效。`alc remote auto-share on`。 |
| `bind` | `"loopback"` | `loopback` 或 `lan`。 |
| `port` | `8787` | `0` 表示隨機連接埠。連接埠被佔用時會自動退回隨機。 |
| `allowed_hosts` | `[]` | 除了這台機器自己的位址之外，還要回應哪些名字 —— 隧道的主機名，可精確指定或用 `*.example.com`。`alc remote allow-host` 會編輯這一項。 |
| `scrollback_bytes` | `1048576` | 重新連線的觀看者最多能被精確補回多少位元組。 |
| `max_connections` | `64` | 同時服務的連線數。 |
