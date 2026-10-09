---
id: remote-control
title: 遠端控制
sidebar_label: 遠端控制
sidebar_position: 5
description: 把任何 coding agent 的 session 鏡射到其 owner 的網頁，用手機操作；包含權限模式、tmux 尺寸、跨世代 session 搜尋，以及連結本身就是憑證的安全模型。
keywords:
  - claude code on phone
  - remote coding agent
  - share terminal session
  - tmux
  - tailscale cloudflare tunnel
  - 中文
---

# 遠端控制

把執行中的 session 鏡射到其 owner 的網頁，從另一台裝置操作；每個 agent、
provider 的介面都一樣，但各 runtime owner 各有自己的頁面。你的終端機照常
運作；分享是把 session 鏡射出去，不是把它拿走。

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

遠端控制在 macOS、Linux 與 Windows 10／11 上都能用；在 Windows 上搭配 `--tmux`
要有原生 Windows 版 tmux（安裝器會嘗試自動補裝），見[尺寸歸誰管](#尺寸歸誰管)。

## 從手機連上

預設只綁 loopback。在你開口之前，什麼都不會對外。多個 runtime owner 時，
Tailscale／Cloudflare 請用目標 owner 頁面 URL 的 port，不要假設永遠是 8787。
以下指令以預設 port 為例。既有 hub 保留啟動時的 allowlist；等 session 結束後，
再明確停止／重啟該 owner 才能讀到新允許的名字。Drain 會結束 session，不是
更新步驟。

**Tailscale** —— alc 留在 loopback 上，對外的事交給 Tailscale。走 HTTPS，
中間沒有第三方。

```sh
alc remote allow-host box.tail1a2b.ts.net
tailscale serve 8787
alc --share claude
```

**你自己的 Wi-Fi** —— 什麼都不必裝，但走的是純 HTTP，token 會以明文穿過網路。
在家沒問題；在咖啡廳的 Wi-Fi 上請改用 tunnel。

```sh
alc --share --bind-lan claude
```

**Cloudflare Tunnel** —— 從任何地方都連得上，行動網路也行。TLS 由 Cloudflare
終結，在意這件事就在前面加上 Access。quick tunnel 每執行一次就換一個新的主機
名稱，所以要用萬用字元。

```sh
alc remote allow-host '*.trycloudflare.com'
cloudflared tunnel --url http://127.0.0.1:8787
alc --share claude
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
alc remote url        # 選定 runtime 的連結
alc sessions          # 所有 owner、各自的連結與 session
```

```text
page  http://192.168.1.42:8787/#k=…
      https://box.tail1a2b.ts.net/#k=…

claude-7QK2M9XB4T      claude   running   ask        ~/src/all-code
codex-68B8XMJ6F5       codex    running   plan       ~/src/api
```

每一個允許的名字各有一行。`alc remote token --rotate` 讓選定 runtime 的連結
失效，不會讓其他 owner 的所有連結一起失效。

## 預設就分享

```sh
alc remote auto-share on     # `alc claude` now behaves like `alc --share claude`
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

共享 session 屬於其 runtime 世代的 **hub**。該世代第一次分享時會啟動 hub，
讓 session 活得比啟動它的終端機久。各 owner 提供自己的頁面；`alc sessions`
彙整 legacy 與各世代 owner，列出各自的頁面 URL。

```sh
alc --share claude            # 需要時啟動本世代的 hub
# ctrl-\ then d              # 卸離；session 繼續執行
alc sessions                 # 所有 owner 與其 session
alc attach 7QK2              # 從任何終端機接回去
alc kill 7QK2                # 到真正的 owner 停掉一個 session
alc rename 7QK2 review       # 到 owner 替卡片改名
alc hub status
alc --runtime legacy hub stop --drain # 明確結束該 owner 的 session
```

id 長得像 `claude-7QK2M9XB4T`；任何不會有歧義的前綴，或者只寫後面那一段，
都可以，大小寫不拘。`attach`、`kill`、`rename` 會跨命名空間找到真正的 owner。
前綴符合多個 owner 就拒絕；請用完整 ID 或全域 `--runtime <id|legacy>` 指定 owner。

新 runtime 檔放在 `<config>/run/g/<shortid>`；legacy 保留 `<config>/run`。
設定、憑證、用量紀錄與 `remote.toml` 共享政策仍共用。既有 listener 保留綁定的
位址／port；新世代可以選可用的 port，請用目標 owner 的連結，不要假設所有頁面
都在 8787。更新不重啟或 drain 舊 hub。多個 owner 執行中時，`hub stop` 與
`bridge stop` 須明確指定 runtime。`hub stop` 有 session 在跑就拒絕，除非加
`--drain`；drain 會結束那些 session，停止橋接則可能中斷處理中的請求。兩者都
不是零中斷，也不是更新的必要步驟。

每一次啟動都帶著提出要求的 shell 的工作目錄與環境。Hub 若被直接砍掉，agent
可能繼續 detached 執行；替代 host 先取得 owner lock，再只清理自己的命名空間，
不碰另一個世代的 session。它不留 log 檔；hub 起不來的時候，用
`alc hub start --foreground` 看它說什麼。

## 尺寸歸誰管

一個終端機只有一個尺寸，而那個尺寸屬於你啟動它的那個終端機。所以頁面不會去改
agent 的尺寸：它把真正的字元格線在放得下的範圍內畫到最大、置中，比例對不上的
地方留黑。你調整終端機大小，頁面幾秒內就跟上。

在寬螢幕上（900px 以上）開著某個 session 時，返回按鈕旁的「隱藏 session 清單」
按鈕能把 session 清單收起來，讓終端機用滿整個寬度，按「顯示 session 清單」就放
回來。一般的 session 只是畫得更大；用了下面的 `--tmux`，尺寸歸頁面管，agent 會
真的拿到多出來的欄數。這個選擇會記在那個瀏覽器裡。這個按鈕刻意沒有快捷鍵，因為
每一個按鍵都屬於終端機 —— `ctrl-b` 就是 tmux 的 prefix。手機上什麼都沒變，它本來
就一次只顯示一個窗格。

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

需要 tmux 3.2 以上，而且只對分享出去的 session 有作用。alc 會為每個 session
啟動它自己的 tmux server，不讀任何設定檔，所以你自己的 tmux 完全不會被動到，
agent 也沒辦法透過 `~/.tmux.conf` 碰到某個 session 的 server。從頁面送出的按鍵
直接進 agent 的 pane，所以看頁面的人碰不到 tmux 的指令列；你自己的終端機是
完整的 client，碰得到。

Windows 上要用 tmux 的原生 Windows 移植版（測過的是 tmux 3.6a-win32）。alc
安裝器會自動檢查並嘗試補裝，詳見[安裝與停用選項](./getting-started.md#選用的-tmux-補裝)。
若跳過補裝或未能完成，以下是手動備援；裝完之後開一個新的終端機，讓它讀到
更新後的 PATH。`alc doctor` 的「tmux」那一列會告訴你有沒有找到原生版。

```powershell
winget install --id arndawg.tmux-windows --exact
```

psmux 也會裝一個 `tmux.exe`，但 alc 驅動不了它：它跑不了 alc 用來建立 session
的那一串指令。alc 會跳過 PATH 上的 psmux 去找原生版，所以兩個可以同時裝著。
MSYS2、Cygwin 與 WSL 的 tmux 在 Windows 上一樣不會被用到。

Windows 版的 tmux 用 ANSI 字碼頁傳遞命令列、環境變數和工作目錄，所以在
Windows 上，tmux pane 裡跑的是一個小小的啟動器（就是 alc 自己），它透過
loopback 向 hub 取回 agent 確切的啟動內容。結果是：在 `--tmux` 底下，非 ASCII
的資料夾名稱、參數和環境變數值都沒問題，provider 的 API key 也永遠不會進到
tmux 自己的環境裡。如果 alc 本身裝在路徑不是純 ASCII 的資料夾，alc 會改用
Windows 的 8.3 短路徑；在關掉短檔名的磁碟上，`--tmux` 會拒絕執行，並請你把
alc 裝到 ASCII 路徑底下。

在 Windows 上停掉一個 `--tmux` session（`alc kill` 或 `alc hub stop --drain`），
會結束它的 tmux server，agent 也跟著結束。Windows 沒有 hangup 訊號，所以這時 session 卡片只會說 session 已結束，沒有結束狀態；
macOS 與 Linux 上仍然會顯示那個訊號。

## 它做不到的事

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
alc share <agent> --name x   # name the card (after `--` it is the agent's own)
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
