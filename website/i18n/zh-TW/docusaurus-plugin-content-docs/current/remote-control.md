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

![開著一個 session 的遠端頁面：session 列、終端機、按鍵列與輸入框](/img/remote/session.png)

## 頁面

頁面有幾個分頁 ——「Session」「用量」「工作室」「設定」，以及任何 [mod](#mods) 加上的分頁。
寬螢幕時它們在標題列，手機上則變成底部列，打開的 session 會佔滿整個畫面。

- **Session** 是清單，旁邊是打開的 session：終端機、按鍵列（Esc、Tab、⇧Tab、
  黏著的 Ctrl、方向鍵）、寫長提示用的輸入框、[權限](#權限模式)列與唯讀的檔案面板。
- **用量**是一張儀表板，列出這台機器上的 agent 花了多少 —— 見
  [頁面上的用量](#頁面上的用量)。
- **工作室**負責排列頁面：版面（一次換掉整個外觀，也能一鍵回到預設）、每個 mod
  的小工具放在哪裡、背景、浮動視窗與面板寬度 —— 見 [Mods 與主題](./mods.md#工作室)。
- **設定**是頁面與終端機的外觀，存在 hub 上 —— 見[設定與主題](#設定與主題)。

在 Mac 上按 `⌘K`，或在終端機以外的任何地方按 `Ctrl+K`，會開啟「指令面板」：跳到
某個 session 或分頁、切換主題或密度、開關檔案面板，或執行 mod 的指令。標題列的
搜尋按鈕也能打開它。

![指令面板](/img/remote/palette.png)

![在手機上：session 清單、用量與一個打開的 session](/img/remote/phone.png)

## 頁面上的用量

![「用量」分頁：數字卡、每日圖表、時段與模型](/img/remote/usage.png)

「用量」分頁畫的是 `alc usage` 印出的統計，範圍是最近 7、30 或 90 天，以你瀏覽器
的時區計算：

- token、估算成本、輸出、快取命中比例與 session 數的數字卡，前兩張附迷你走勢圖；
- 一張堆疊的每日圖表，分成輸入、快取讀取、快取寫入與輸出 —— 或改看成本 ——
  滑過或點一下就能看到那一天的數字；
- 你一天中工作的時段，以及模型、agent、provider 的排名；
- 再往下是每個登入的額度視窗與啟動帳本，和以前一樣。

hub 讀的是 alc 自己的 `usage.jsonl`，加上它那台機器上 Claude Code 與 Codex 的歷史，
用內建與快取的價格表離線計價，每個結果保留一分鐘。唯讀連結看得到什麼，見
[用量](./usage.md#在遠端控制頁面上)。

## 設定與主題

![「設定」分頁：主題卡片、強調色與密度](/img/remote/settings.png)

設定存在設定目錄的 `web/settings.json`，所以每台打開頁面的裝置看到的樣子都一樣。
唯讀連結會以唯讀方式顯示它們。

- **主題** ——「跟隨系統」（隨裝置）、「淺色」、「深色」、五個內建主題（Midnight、
  Daylight、Fjord、Ember、Paper），或 `web/themes/` 裡的任何檔案。主題也會替終端機
  換上配色。
- **強調色**與**密度**（「舒適」或「緊湊」）。
- **終端機** —— 字級（或依螢幕自動）、字型、行高、游標樣式與閃爍、捲動緩衝，
  附即時預覽。

![Fjord 主題](/img/remote/theme-fjord.png)

主題檔把顏色 token 疊在淺色或深色的底上，也可以設定終端機的調色盤：

```json
{
  "name": "Tidepool",
  "mode": "dark",
  "tokens": { "--bg": "#081214", "--surface": "#0d1b1e", "--accent": "#2dd4bf" },
  "terminal": { "background": "#061012", "foreground": "#d6f1ec", "cursor": "#2dd4bf" }
}
```

`web/AGENTS.md` 列出了每一個 token。值只能是顏色與長度 —— 不能有 `url()`、引號或
分號 —— 其他的 alc 都會丟掉。

## Mods

![「設定」分頁裡的 Mods](/img/remote/mods.png)

mod 會替頁面加東西：一個分頁、指令面板裡的指令、按鍵列上的按鈕、session 卡片上的
小標籤，或樣式。這個想法來自 herdr-app：頁面的行為是 agent 寫得出來的純文字檔案，
旁邊附一份寫給 agent 看的參考文件。

```text
<config dir>/web/
├── AGENTS.md          the reference; alc rewrites it to match the running version
├── settings.json
├── themes/<id>.json
├── mods/<id>/
│   ├── mod.json       {"name", "description", "version", "author"}
│   ├── mod.js         optional
│   └── mod.css        optional
└── examples/          never loaded; copy from here
```

[Mods 與主題](./mods.md)是完整的指南：怎麼寫一個 mod、API、把 mod 當成 GitHub
repository 發佈，以及安裝別人寫的 mod：

```sh
alc remote web install someone/alc-mod-standup   # arrives off; read it, then:
alc remote web enable standup
```

`alc remote web` 會印出這個資料夾。做出一個 mod 最快的方法，是請你正在跑的 agent
來做 ——「設定」的「Mods」區塊有一段可以複製的提示，開頭是「Read …/AGENTS.md, then
make me an alc page mod that」。或者複製一個範例：

```sh
cp -R "$(alc remote web)/examples/mods/scratchpad" "$(alc remote web)/mods/"
alc remote web check
```

| 範例 | 加上什麼 |
| --- | --- |
| `quick-prompts` | 按鍵列與指令面板上的 Continue、Run tests、Summarise 與 `/compact` |
| `scratchpad` | 一個存在 hub 上的 **Notes** 分頁，附一個把筆記送到打開的 session 的按鈕 |
| `session-age` | 在執行超過一小時的 session 上顯示時數小標籤 |
| `themes/tidepool.json` | 一個藍綠色的深色主題 |

mod 的腳本會用 `alc.define` 把自己包起來：

```js
alc.define('hello', (mod) => {
  mod.addPage({
    id: 'today',
    title: 'Today',
    icon: 'clock',
    render(root) {
      const running = alc.sessions().filter((card) => !card.exit).length;
      root.append(alc.h('div', { class: 'panel' }, `${running} sessions running`));
    },
  });
  mod.addKey({ label: 'Continue', run: () => alc.type('continue') && alc.key('\r') });
  mod.addSessionChip((card) => (card.viewers > 1 ? { text: 'shared', tone: 'accent' } : null));
});
```

![一個 mod 的分頁：scratchpad 範例](/img/remote/mod-page.png)

`alc` 還給 mod 這些東西：`on(event)`，事件有 sessions、attach、detach、page、usage
與 settings；`api('/api/…')`，呼叫頁面自己的 JSON 路由；`toast`、`type`、`key`、
`open`、`settings`、`theme`；以及 `mod.save(state)`，把狀態存進 `settings.json`。
完整參考見 `web/AGENTS.md`。

頁面大約四秒內就會注意到存檔的檔案：設定、主題與樣式表直接套用，而腳本一改就會
出現「重新載入」按鈕，因為腳本只能在 hub 送出頁面時算好的雜湊之下執行。在「設定」
裡開關 mod 也是同樣的道理。丟出例外的 mod 會在「設定」裡標成「錯誤」並附上訊息，
頁面其他部分照常運作。

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
- **檔案面板只看得到 session 的資料夾。** 能打字的連結可以讀 agent 啟動時所在
  資料夾裡的檔案 —— 往上爬的路徑、指向外面的 symlink 都會被拒絕，`.git` 不會列出，
  檔案上限 512 KB，alc 放進去的 API key 在這裡一樣會被遮蔽。唯讀連結完全拿不到檔案。
- **Mods 是受信任的程式碼。** mod 以頁面的連結身分在頁面中執行，不在沙盒裡 ——
  開啟之前先讀過。頁面只執行每個已啟用 `mod.js` 的確切位元組：hub 送出頁面時會把
  它們的雜湊寫進 content-security policy。唯讀連結執行同樣的 mods，但無法透過它們
  存檔或打字，而且也會下載它們，所以永遠不要把秘密放進 mod。
- **alc 永遠不回應剪貼簿讀取**，所以 agent 印出來的一個惡意檔案，沒辦法把你
  上次複製的東西拉進模型的 context。
- **alc 不從工作中的 repository 讀任何設定**，所以一個 checked-in 的檔案
  永遠不可能把分享打開。

## 權限模式

這九個 agent 對「權限模式是什麼」、「那些模式叫什麼名字」、「啟動之後還能不能
改」，看法都不一樣。頁面畫出來的，是眼前這個 agent 真正做得到的事：可以直接
指定模式的給下拉選單；只能循環切換的，顯示目前模式的小標籤加一顆「循環」按鈕
（點一下標籤會展開循環的順序）；啟動後就不能改、或根本沒有這個概念的 agent，
只顯示模式標籤，旁邊註明原因。它會同時顯示 alc 的層級和 agent
自己的說法，因為共用一個標籤只會誤導人 —— `auto` 在 Goose 是最寬鬆的設定，
在 Claude Code 卻只是中間的一級。

| Agent | alc 傳出去的旗標 | session 進行中改變它 |
| --- | --- | --- |
| claude | `--permission-mode plan\|manual\|acceptEdits\|auto\|bypassPermissions` | 只能用 Shift+Tab 循環，所以頁面給的是「循環」 |
| codex | `-s read-only\|workspace-write\|danger-full-access` 與 `-a on-request\|never` | `/permissions` 會開啟 Codex 自己的選單 |
| opencode | inline config 的 `default_agent` plan\|build、`--auto` | Tab 在 build ↔ plan 之間切換 |
| goose | `GOOSE_MODE=chat\|approve\|smart_approve\|auto` | `/mode <name>` |
| qwen | `--approval-mode plan\|default\|auto-edit\|auto\|yolo` | `/approval-mode <name>` |
| kimi | `--plan`、`--yolo` | 只能重新啟動 |
| copilot | `--mode plan\|interactive`、`--allow-all-tools` | `/permissions` 會開啟它的選單 |
| pi | — | 不支援：Pi 刻意沒有權限模式，也沒有沙箱 |
| agy | `--mode plan`、`--mode accept-edits`、`--sandbox --dangerously-skip-permissions`、`--dangerously-skip-permissions` | 只能重新啟動 |

alc 只對已經拿真實的 `--help` 核對過的 agent 注入權限旗標；其餘的，除非你用
`--permission` 開口，否則它什麼都不動。agy 核對過了，但它自己的預設本來就會先
問，所以它的 `ask` 層級不加任何旗標：沒有指定 `--permission` 的共享 agy，啟動起來
就跟直接跑 `agy` 一模一樣。共享的 `--dry-run` 會印出真正共享時會加上的旗標，每個
agent 都一樣。每張卡片都會說 alc 對自己顯示的東西有
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
alc hub stop --all           # 停掉所有 owner 與它們全部的 session（-a）
```

id 長得像 `claude-7QK2M9XB4T`；任何不會有歧義的前綴，或者只寫後面那一段，
都可以，大小寫不拘。`attach`、`kill`、`rename` 會跨命名空間找到真正的 owner。
前綴符合多個 owner 就拒絕；請用完整 ID 或全域 `--runtime <id|legacy>` 指定 owner。

新 runtime 檔放在 `<config>/run/g/<shortid>`；legacy 保留 `<config>/run`。
設定、憑證、用量紀錄與 `remote.toml` 共享政策仍共用。既有 listener 保留綁定的
位址／port；新世代可以選可用的 port，請用目標 owner 的連結，不要假設所有頁面
都在 8787。更新不重啟或 drain 舊 hub。多個 owner 執行中時，`hub stop` 與
`bridge stop` 須明確指定 runtime，或用 `--all`（`-a`）停掉所有 owner 並結束它們
全部的 session。`hub stop` 有 session 在跑就拒絕，除非加
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

當那個格線得縮小才放得下 —— 例如在手機上看 120 欄的終端機，或檔案面板旁的窄空間
—— 頁面會在終端機上方說明這件事，並提供兩個解法：**符合這個頁面**，讓這個頁面改變
session 的大小（檔案面板按鈕旁的按鈕也一樣，而設定或工作室裡的*一般 session* 會把它
設為預設），或複製指令 `alc share --tmux <agent>`。符合頁面的 session，只要它自己的
終端機一調整大小、或頁面放手或關閉，就會回到原本終端機的大小。按提示上的 × 就能
把它關掉。

![手機上的一般 session，附上建議改用 --tmux 的提示](/img/remote/size-note.png)

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
alc remote web               # the page's settings, themes and mods folder
alc remote web check         # validate it
alc remote web list          # mods (on/off) and themes
alc remote web catalog       # official mods, layouts, themes and templates
alc remote web add <id>      # install an official one
alc remote web new <id> --template widget        # start a mod from a template
alc remote web layout [list | apply <id> [--enable-mods] | reset | remove <id>]
alc remote web install <owner/repo|url|folder>   # mods arrive off
alc remote web enable|disable|remove <id>

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

頁面自己的外觀與 mods 不放在 `remote.toml`，而是在它旁邊的 `web/` 裡 —— 見
[設定與主題](#設定與主題)與 [Mods](#mods)。
