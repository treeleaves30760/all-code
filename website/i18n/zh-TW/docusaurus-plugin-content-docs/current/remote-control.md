---
id: remote-control
title: 遠端控制
sidebar_label: 遠端控制
sidebar_position: 5
description: 把任何 coding agent 的 session 鏡射到一個網頁，用手機操作它 —— 每個 agent 都是同一個頁面，還有權限模式、tmux 尺寸，以及一套把連結本身當成憑證的安全模型。
keywords:
  - claude code on phone
  - remote coding agent
  - share terminal session
  - tmux
  - tailscale cloudflare tunnel
  - 中文
---

# 遠端控制

把執行中的 session 鏡射到一個網頁，從另一台裝置操作它 —— 不管哪一個 agent、
哪一個 provider，都是同一個頁面。你的終端機照常運作；分享是把 session
鏡射出去，不是把它拿走。

```sh
alc --share claude
alc share opencode -- --mini
```

```text
alc session claude-7QK2M9XB4T (claude@all-code)
  open  http://127.0.0.1:8787/#k=…
  hub   127.0.0.1:8787 · loopback only (pid 48213) · this link grants input; keep it to yourself
  keys  ctrl-\ then d detaches; the session keeps running
```

## 從手機連上

預設只綁 loopback。在你開口之前，什麼都不會對外。

**Tailscale** —— alc 留在 loopback 上，對外的事交給 Tailscale。走 HTTPS，
中間沒有第三方。

```sh
alc remote allow-host box.tail1a2b.ts.net
tailscale serve 8787
alc claude --share
```

**你自己的 Wi-Fi** —— 什麼都不必裝，但走的是純 HTTP，token 會以明文穿過網路。
在家沒問題；在咖啡廳的 Wi-Fi 上請改用 tunnel。

```sh
alc claude --share --bind-lan
```

**Cloudflare Tunnel** —— 從任何地方都連得上，行動網路也行。TLS 由 Cloudflare
終結，在意這件事就在前面加上 Access。quick tunnel 每執行一次就換一個新的主機
名稱，所以要用萬用字元。

```sh
alc remote allow-host '*.trycloudflare.com'
cloudflared tunnel --url http://127.0.0.1:8787
alc claude --share
```

alc 只回應你允許過的名字：它拿 `Host` 標頭去比對一份清單，並且允許 host
落在同一份清單上的 `Origin`。攻擊者的網頁可以把 `evil.com` 指向 `127.0.0.1`，
借你自己的瀏覽器來操作你的 agent，而瀏覽器認定自己正在對話的那個名字，
正是它偽造不了的部分。

```sh
alc remote allow-host box.tail1a2b.ts.net   # exact
alc remote allow-host '*.trycloudflare.com' # any subdomain
alc remote status                           # what it answers to now
```

## 把連結再找回來

agent 一畫出自己的介面，那個連結就捲走了。

```sh
alc remote url        # just the link
alc sessions          # the link, then what is running
```

```text
page  http://192.168.1.42:8787/#k=…
      https://box.tail1a2b.ts.net/#k=…

claude-7QK2M9XB4T      claude   running   ask        ~/src/all-code
codex-68B8XMJ6F5       codex    running   plan       ~/src/api
```

每一個允許的名字各有一行。`alc remote token --rotate` 會讓目前為止發出去的
每一個連結全部失效。

## 預設就分享

```sh
alc remote auto-share on     # `alc claude` now behaves like `alc claude --share`
alc --no-share claude        # opt one launch out
```

`alc config` 的 **Sharing & remote** 畫面裡也有這個開關。被腳本呼叫的執行 ——
輸入或輸出被轉向的那種 —— 不管這裡設成什麼都不分享，所以一個長期留著的偏好，
不會讓某個 cron job 突然開始失敗。在那種情況下明確寫上 `--share`，仍然會直接
報錯停下來。

## 這個連結給出了什麼

一個能對 coding agent 打字的網頁，等於是在你的機器上遠端執行程式碼。

- **fragment 就是憑證。** 拿到 `#k=…` 那一段的人就能打字。fragment 永遠不會
  送到伺服器，所以它不會留在存取紀錄和 proxy 裡，但它會進到你的剪貼簿。
  請把它當成密碼看待。
- **Host 與 Origin 都會檢查**，連 port 都算，而 token 是以固定時間比對的。
- **分享出去的 session 就是分享螢幕。** alc 會遮蔽它自己放進環境裡的 API
  key，每一個畫面都遮，包括你自己的終端機。除此之外 agent 印出來的東西，
  看的人都看得到。
- **alc 永遠不回應剪貼簿讀取**，所以 agent 印出來的一個惡意檔案，沒辦法把你
  上次複製的東西拉進模型的 context。
- **alc 不從工作中的 repository 讀任何設定**，所以一個 checked-in 的檔案
  永遠不可能把分享打開。

## 權限模式

這八個 agent 對「權限模式是什麼」、「那些模式叫什麼名字」、「啟動之後還能不能
改」，看法都不一樣。頁面畫出來的，是眼前這個 agent 真正做得到的事：可以直接
指定模式的給下拉選單，只能循環切換的給一個循環按鈕，而根本沒有這個概念的
agent，給的是一個停用的控制項，上面寫著原因。它會同時顯示 alc 的層級和 agent
自己的說法，因為共用一個標籤只會誤導人 —— `auto` 在 Goose 是最寬鬆的設定，
在 Claude Code 卻只是中間的一級。

| Agent | alc 傳出去的旗標 | session 進行中改變它 |
| --- | --- | --- |
| claude | `--permission-mode plan\|manual\|acceptEdits\|auto\|bypassPermissions` | 只能用 Shift+Tab 循環，所以頁面給的是「循環」 |
| codex | `-s read-only\|workspace-write\|danger-full-access` 與 `-a on-request\|never` | `/permissions` 會開啟 Codex 自己的選單 |
| opencode | `--agent plan\|build`、`--auto` | Tab 在 build ↔ plan 之間切換 |
| goose | `GOOSE_MODE=chat\|approve\|smart_approve\|auto` | `/mode <name>` |
| qwen | `--approval-mode plan\|default\|auto-edit\|auto\|yolo` | `/approval-mode <name>` |
| kimi | `--plan`、`--yolo` | 只能重新啟動 |
| copilot | `--mode plan\|interactive`、`--allow-all-tools` | `/permissions` 會開啟它的選單 |
| pi | — | 不支援：Pi 刻意沒有權限模式，也沒有沙箱 |

alc 只對已經拿真實的 `--help` 核對過的 agent 注入權限旗標；其餘的，除非你用
`--permission` 開口，否則它什麼都不動。每張卡片都會說 alc 對自己顯示的東西有
多少把握：`launched`、`reported`，或者一個代表用猜的 `?`。

`remote.toml` 裡的 `max_permission`（預設 `auto-edit`）是頁面自己能達到的
最寬鬆模式。收緊永遠不必問人；比它更寬鬆的，一律換來一張票：

```text
$ alc confirm 7QK2M9XB4T
granted: auto
the page can apply it once, within the next minute.
```

`alc confirm` 沒有終端機就拒絕執行，所以這個確認來自坐在機器前面的人，
而不是握有連結的人。超過上限的每一級，每一次都要重新放行，因為 alc 對
「現在是哪個模式」的認知是一種相信，不是事實。

## session 與 hub

分享出去的 session 屬於一個 **hub** —— 你第一次分享時 alc 會啟動的背景行程。
就是它讓同一個頁面能列出每一個 session，也讓 session 活得比啟動它的那個
終端機還久。

```sh
alc claude --share           # starts a hub if one is not running
# ctrl-\ then d              # detach; the session keeps running
alc sessions                 # what is running
alc attach 7QK2              # back on it, from any terminal
alc kill 7QK2                # stop one
alc rename 7QK2 review       # rename its card
alc hub status
alc hub stop [--drain]       # refuses while sessions run unless --drain
```

id 長得像 `claude-7QK2M9XB4T`；任何不會有歧義的前綴，或者只寫後面那一段，
都可以，大小寫不拘。

每一次啟動都帶著提出要求的那個 shell 的工作目錄與環境，所以在某個 repository
裡開的 session，永遠不會去改另一個。hub 若被直接砍掉，agent 會繼續以 detached
的狀態跑下去，而下一個 hub 會把上一個來不及收的東西清掉。它不留 log 檔；
hub 起不來的時候，執行 `alc hub start --foreground` 看它說什麼。

## 尺寸歸誰管

一個終端機只有一個尺寸，而那個尺寸屬於你啟動它的那個終端機。所以頁面不會去改
agent 的尺寸：它把真正的字元格線在放得下的範圍內畫到最大、置中，比例對不上的
地方留黑。你調整終端機大小，頁面幾秒內就跟上。

`--tmux` 是給「你真正要用的其實是頁面那一邊」準備的。它讓 agent 跑在 tmux
裡，於是你的終端機和 hub 各自以獨立的 client 連上、各有各的尺寸 ——
而這一次，決定 agent 尺寸的是頁面。

```sh
alc --share --tmux --codex claude    # or -t
```

| | 不用 `--tmux` | 用 `--tmux` |
| --- | --- | --- |
| 尺寸 | 你終端機的；頁面負責縮放 | 頁面的；你的終端機顯示放得下的部分 |
| Detach | `ctrl-\` 然後 `d` | `ctrl-b` 然後 `d` |
| Scrollback | 頁面的 | 頁面的，本機再加上 tmux 的 copy mode |
| 你終端機上看到的 | 經由 hub 鏡射，金鑰已遮蔽 | 一個完整的 tmux client，原樣呈現 |

最後一列很重要：用了 `--tmux`，你的終端機顯示的是 agent 真正印出來的東西，
包括它自己回顯的金鑰。瀏覽器那邊看到的仍然是遮蔽過的。

需要 tmux 3.2 以上，只對分享出去的 session 有作用，而且只跑在 macOS 與 Linux
上。alc 會為每個 session 啟動它自己的 tmux server，不讀任何設定檔，所以你自己
的 tmux 完全不會被動到，agent 也沒辦法透過 `~/.tmux.conf` 碰到某個 session 的
server。從頁面送出的按鍵直接進 agent 的 pane，所以看頁面的人碰不到 tmux 的
指令列；你自己的終端機是完整的 client，碰得到。

## 它做不到的事

- **目前還不支援 Windows。** `--share` 和 `alc hub` 那組指令在那裡會直接拒絕
  並說明原因。alc 其他的事在 Windows 上都能做。
- **核准提示是以終端機文字出現的**，不是手機上的對話框。
- **每一條 operator 連結都能同時打字。** 沒有「取得控制權」這種仲裁機制；
  不是你在操作的對象，請發唯讀連結給他們。
- **session 撐不過重開機**，而且 alc 接不上不是它自己啟動的 session。
- **大多數 agent 的權限狀態是相信，不是知道。** 每張卡片上的把握度標記會告訴
  你眼前是哪一種情況。
- **全螢幕的 agent 在瀏覽器裡沒有 scrollback。** `codex --no-alt-screen` 和
  `opencode --mini` 在手機上好用太多，頁面也會這樣提醒你。

## 指令

```sh
alc --share <agent>          # mirror this session
alc --share --tmux <agent>   # ...and let the page own the agent's size
alc share <agent> -- <args>  # the unambiguous form
alc share <agent> --name x   # name the card
alc --no-share <agent>       # never mirror, whatever the settings say

alc remote status
alc remote on | off
alc remote token --rotate
alc remote url
alc remote auto-share on
alc remote allow-host <host>

alc --share --permission plan <agent>
alc confirm <ticket>
```

`--share` 是 alc 自己的旗標，所以要寫在 agent 的參數前面；寫在後面，alc
會直接說，而不是把它傳下去。

## 設定

`remote.toml` 就放在 `config.toml` 旁邊；`alc remote status` 會印出它的路徑。
它之所以是獨立的一個檔案，是因為 `config.toml` 會拒絕它不認得的鍵。

| 鍵 | 預設 | 作用 |
| --- | --- | --- |
| `enabled` | `true` | 總開關。`alc remote off` 設的就是這個。 |
| `auto_share` | `false` | 不寫 `--share` 也分享每一個 session。 |
| `bind` | `"loopback"` | `loopback` 或 `lan`。 |
| `port` | `8787` | `0` 表示隨機挑一個 port；被佔用時會自動換一個。 |
| `allowed_hosts` | `[]` | 除了這台機器自己的名字，還要回應哪些名字。 |
| `max_permission` | `"auto-edit"` | 頁面自己能達到的最寬鬆模式。 |
| `scrollback_bytes` | `1048576` | 重新連上的觀看者能往回補多少內容。 |
| `max_connections` | `64` | 同時服務的連線數。 |
