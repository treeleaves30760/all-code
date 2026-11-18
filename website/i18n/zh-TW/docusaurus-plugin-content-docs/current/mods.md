---
id: mods
title: Mods 與主題
sidebar_label: Mods 與主題
sidebar_position: 6
description: 用主題、mod、小工具與套件自訂 alc 的遠端控制頁面 —— 在頁面內建的工作室排列它、從範本寫自己的 mod、把它當成 GitHub repository 發佈，並用一行指令安裝別人寫的。
keywords:
  - alc mods
  - alc themes
  - remote page customisation
  - herdr mods
  - install mod from github
  - alc studio
  - mod packages
  - page widgets
  - 中文
---

# Mods 與主題

[遠端控制頁面](./remote-control.md)可以用跑 hub 那台機器上的純文字檔案來改：**主題**
替它換顏色，**mod** 替它加東西 —— 一個分頁、指令面板裡的指令、按鍵列上的按鈕、
session 卡片上的小標籤，或樣式 —— 還有可以放在 session 清單與編碼畫面任何位置的
**小工具**、**浮動視窗**與**背景**。**套件**把 mods 連同它們的版面、背景與主題打包在
一起。這個想法來自 herdr-app：頁面的行為是 agent 寫得出來的檔案，旁邊附一份寫給
那個 agent 看的參考文件。

這裡的每件事都可以在頁面的**工作室**分頁完成、直接在資料夾裡手動改，或請你的
agent 幫忙。

![工作室的套件](/img/remote/studio-packages.png)

## 工作室

**工作室**分頁是頁面內建的 mods 編輯器。它的五個分頁都寫進 hub 上的同一個資料夾，
所以在筆電上做的改動，手機上也看得到：

- **套件** —— 套用官方套件（Cockpit、Night shift、Deep focus、Retro terminal、
  Workbench）或你自己的；把目前的頁面存成套件；從 JSON 或檔案匯入；下載或複製
  一個套件來分享。
- **版面** —— 一個列出所有已開啟 mods 提供的小工具的工具庫，以及 session 清單與
  編碼畫面的縮圖。把小工具拖進某個位置（或點選它再選擇**位置**）、調整順序、隱藏，
  並在檢視器裡設定選項。
- **Mods** —— 開關 mods、安裝官方 mods、從 GitHub 安裝、移除，以及從範本建立
  **新 mod**。
- **背景** —— 內建的漸層或圖樣、你上傳的圖片，或 mod 繪製的動態背景；可調整
  變暗、模糊與終端機不透明度。
- **面板** —— session 清單、檔案面板與兩側面板的寬度，清單與檔案面板放在哪一側，
  以及一般 session 是否要符合頁面大小。

![在工作室裡放置小工具](/img/remote/studio-layout.png)

唯讀連結看得到工作室，但不能改任何東西。

### 位置（slot）

| 位置 | 在哪裡 |
| --- | --- |
| `bar` | 頁首，連線指示燈旁邊 —— 請保持一行 |
| `list.top`、`list.bottom` | session 卡片的上方與下方 |
| `card` | 每張 session 卡片裡（只能顯示：卡片本身是按鈕） |
| `view.top`、`view.bottom` | 終端機的上方與下方 |
| `view.left`、`view.right` | 終端機旁的側面板；每個小工具有一個可收合的區塊 |
| `float` | 自己的浮動視窗 |

沒有人放置過的小工具，會先畫在它的 mod 建議的位置，直到你移動或隱藏它，所以開啟
一個 mod 就會立刻看到它的小工具。套件的版面則是*明確的*：只畫它放置的東西。
**重設版面**會回到各 mod 的建議。

### 面板，以及讓一般 session 符合頁面

拖曳 session 清單、檔案面板或側面板的內側邊緣來調整寬度；雙擊邊緣可重設，也可以
在邊緣上用方向鍵。終端機會依剩下的空間重新調整。

用 `--tmux` 啟動的 session 本來就採用頁面的大小。一般的 session 則以它自己終端機的
大小顯示，所以以前打開檔案面板時，它會被縮小成上下留黑。**符合這個頁面**（檔案面板
按鈕旁的按鈕、大小提示、指令面板，或在工作室「面板」分頁中對所有 session 開啟
*一般 session*）讓頁面改變它的大小，終端機就會填滿面板留下的空間。啟動它的終端機
一旦調整大小，就會把大小拿回去；頁面放手或關閉時，hub 也會把大小交還。

![在檔案面板旁符合頁面的一般 session](/img/remote/fit.png)

### 視窗

放在 `float` 位置的小工具，或 mod 自己用 `mod.addWindow` 建立的視窗，可以拖曳標題列
移動、從角落調整大小。位置與是否開啟會在每台裝置各自記住；關閉的視窗會待在頁首的
視窗匣（以及指令面板）裡。在手機上，視窗一開始是關閉的，打開時會以面板形式出現在
按鍵列上方。

## 官方 mods 與套件

alc 把這些放在 `examples/`。從工作室安裝，或用 `alc remote web add <id>` —— 官方
mods 安裝後即開啟。

| Mod | 加上什麼 |
| --- | --- |
| `clock` | 時間（可加第二時區），放在頁首或視窗 |
| `session-overview` | 在清單上方以方塊顯示執行中、代理、觀看者與最久的 session |
| `quota-meter` | 每個登入的 5 小時與每週配額長條，或在頁首顯示最高的一個 |
| `session-notes` | 每個 session 一份筆記，放在它的終端機旁，存在 hub 上 |
| `command-deck` | 目前代理的 slash 指令按鈕（`/compact`、`/review`、`/status` …） |
| `pomodoro` | 會發出通知的專注計時器視窗 |
| `ambient` | 動態背景：星空、字元雨、波浪、粒子 |
| `crt` | 終端機上的掃描線與螢光光暈（只有 CSS） |
| `quick-prompts` | 按鍵列、指令面板上的提示按鈕，以及可編輯的小工具 |
| `scratchpad` | 筆記分頁與筆記小工具 |
| `session-age` | 執行時數標籤，以及每張卡片上的時間條 |

| 套件 | 內容 |
| --- | --- |
| `cockpit` | 清單上方的方塊、右側面板的配額與 session 筆記、終端機下方的指令列、時鐘、時間條 |
| `night-shift` | 半透明終端機後的星空、Midnight 主題、專注計時器與時鐘 |
| `deep-focus` | 緊湊密度、模糊漸層、頁首的計時器、終端機下方的提示按鈕 |
| `retro` | CRT 終端機、字元雨、Ember 主題、筆記視窗 |
| `workbench` | 檔案在左、右側面板放筆記與快速提示、頁首放總覽 |

```sh
alc remote web catalog                 # everything official, and what is installed
alc remote web add cockpit             # an official package (or mod, or theme)
alc remote web package apply cockpit   # installs the official mods it needs, too
alc remote web package                 # list packages; * marks the applied one
```

![Cockpit 套件](/img/remote/cockpit.png)

## 檔案放在哪裡

```sh
alc remote web          # prints the folder, usually ~/.config/alc/web
```

```text
web/
├── AGENTS.md          the API reference, written for an agent; alc keeps it current
├── settings.json      what Settings and the Studio save (theme, terminal, mods, layout, background)
├── themes/<id>.json   colour tokens for the page and the terminal
├── mods/<id>/
│   ├── mod.json       {"name", "description", "version", "author"}
│   ├── mod.js         optional: runs once when the page loads
│   ├── mod.css        optional: added after the page's own stylesheet
│   └── README.md      optional: shown to people, not loaded
├── packages/<id>.json a set of mods with a layout, background and theme
├── backgrounds/       images for the background (png, jpg, webp, gif)
└── examples/          the official mods, packages and themes, and new-mod templates; never loaded
```

id 只能用 `[a-z0-9_-]`，開頭必須是字母或數字，最長 48 個字元。一個 mod 需要
`mod.js`、`mod.css`，或兩者都有。

## 最快的方法：請你的 agent 幫忙

打開「設定」→「Mods」，複製「請 agent 幫忙」底下的提示，或在任何 session 裡自己
打：

```text
Read ~/.config/alc/web/AGENTS.md, then make me an alc page mod that adds a
"Deploy" key which types `npm run deploy` into the open session.
```

`AGENTS.md` 會告訴 agent 資料夾的結構、每一個 API 呼叫、主題 token，並要它最後跑
`alc remote web check`。頁面幾秒內就會注意到新的檔案。

## 自己動手寫

從範本開始 —— 在工作室的 Mods 分頁，或：

```sh
alc remote web new my-widget --template widget   # or page, window, background, style, blank
```

這會寫出 `mods/my-widget/`，id 與名稱都已填好，並且已開啟。或者複製一個官方 mod：

```sh
WEB="$(alc remote web)"
cp -R "$WEB/examples/mods/quick-prompts" "$WEB/mods/my-prompts"
```

編輯 `mods/my-prompts/mod.json` 替它取個名字，接著是 `mod.js`：

```js
// Wrap everything in alc.define so an error is shown on the Settings
// page as this mod's, instead of breaking the page. The id is the folder name.
alc.define('my-prompts', (mod) => {
  const send = (text) => {
    if (!alc.current()) return alc.toast('Open a session first', 'warn');
    if (alc.type(text)) alc.key('\r');
  };

  // A button on the key bar under the terminal.
  mod.addKey({ label: 'Review', run: () => send('Review the diff you just made.') });

  // An entry in the command palette (⌘K / Ctrl+K).
  mod.addCommand({ id: 'deploy', title: 'Type: npm run deploy', run: () => send('npm run deploy') });

  // A chip on every session card it returns something for.
  mod.addSessionChip((card) => (card.viewers > 1 ? { text: 'shared', tone: 'accent' } : null));
});
```

把它打開並檢查：

```sh
alc remote web enable my-prompts
alc remote web check            # "ok: …" or one "problem: …" line per issue
```

腳本一改或剛被開啟，頁面就會出現「重新載入」，因為頁面只會執行雜湊符合 hub 送出
頁面時所算出那一份的腳本。樣式表與主題不必重新載入就會套用。

### 有自己分頁的 mod

```js
alc.define('today', (mod) => {
  mod.addPage({
    id: 'today',
    title: 'Today',
    icon: 'clock',               // or an SVG path on a 16×16 grid
    render(root, ctx) {
      const draw = () => {
        const running = alc.sessions().filter((card) => !card.exit);
        root.replaceChildren(
          alc.h('h2', { class: 'pane-title' }, 'Running now'),
          alc.h('ul', {}, running.map((card) =>
            alc.h('li', {}, alc.h('button', {
              class: 'btn',
              onclick: () => alc.open(`session:${card.id}`),
            }, `${card.name} · ${card.agent}`)))),
        );
      };
      draw();
      // Follow the 5-second session poll while this tab is open.
      const stop = alc.on('sessions', () => (ctx.active() ? draw() : stop()));
    },
  });
});
```

![scratchpad 範例的 Notes 分頁](/img/remote/mod-page.png)

### 小工具

小工具是工作室或套件可以放置的東西。mod 說明它*可以*放在哪裡，以及在有人放置
之前它在哪裡；選項會出現在工作室的檢視器裡。

```js
alc.define('build-light', (mod) => {
  mod.addWidget({
    id: 'light',
    title: 'Build light',
    icon: 'bolt',
    defaultSlot: 'bar',
    slots: ['bar', 'view.right', 'float'],
    options: [{ key: 'label', type: 'text', label: 'Label', default: 'CI' }],
    render(root, ctx) {
      const dot = alc.h('span', { class: 'chip' }, `${ctx.options.label}: …`);
      root.append(dot);
      // Undone automatically when the widget is moved or removed.
      ctx.every(30000, async () => {
        const cards = await alc.api('/api/sessions');
        dot.textContent = `${ctx.options.label}: ${cards.length} sessions`;
      });
    },
  });
});
```

`ctx` 還有 `session()`（打開的 session）、`card`（在卡片位置時）、`on(event, fn)`、
`onCleanup(fn)`、`active()` 與 `setOptions({...})`。編碼畫面的位置在你打開每個
session 時都會重畫，所以放在那裡的小工具直接讀 `ctx.session()` 就好。

### 視窗與背景

```js
alc.define('peek', (mod) => {
  const win = mod.addWindow({
    id: 'main', title: 'Peek', width: 320, height: 200,
    render(root, ctx) {
      root.append(alc.h('pre', {}, JSON.stringify(ctx.session(), null, 2)));
    },
  });
  mod.addKey({ label: 'Peek', run: () => win.toggle() });

  mod.addBackground({
    id: 'glow', name: 'Glow',
    render(host, ctx) {
      host.style.background = 'radial-gradient(circle at 30% 20%, #4f46e5, #050608 70%)';
      return () => { host.style.background = ''; };
    },
  });
});
```

背景 mod 在 `ctx.reducedMotion()` 為 true 時應該只畫一張靜止畫面，並在分頁隱藏時
暫停。

### 保存狀態

`mod.state` 是這個 mod 存下來的物件，`mod.save(next)` 會把它寫進 `settings.json` 的
`modSettings.<id>` 底下，所以每台裝置上都一樣。存檔需要能打字的連結；在唯讀連結上
它會被拒絕。

```js
alc.define('counter', (mod) => {
  mod.addCommand({
    id: 'bump',
    title: 'Count a coffee',
    run: async () => {
      const cups = (mod.state.cups || 0) + 1;
      await mod.save({ cups });
      alc.toast(`${cups} today`);
    },
  });
});
```

### API

| 呼叫 | 作用 |
| --- | --- |
| `alc.define(id, setup)` | 執行 `setup(mod)`，錯誤會算在這個 mod 頭上 |
| `alc.on(event, fn)` | `sessions`、`attach`、`detach`、`page`、`usage`、`settings`；回傳取消訂閱的函式 |
| `alc.sessions()` / `alc.current()` | session 卡片／目前打開的那一個 |
| `alc.open(target)` | `sessions`、`usage`、`settings`、某個 mod 的分頁，或 `session:<id>` |
| `alc.api('/api/…')` | 頁面的 JSON 路由：sessions、usage、usage history、web、caps |
| `alc.type(text)` / `alc.key(data)` | 貼上到／送出原始位元組到打開的 session |
| `alc.toast(text, level)` | `info`、`warn` 或 `error` |
| `alc.h(tag, props, ...children)` | 一個小小的元素建構器 |
| `alc.settings()` / `alc.theme()` | 目前生效的設定／`{id, mode}` |
| `alc.canType`、`alc.locale`、`alc.version` | 連結的權限等級、`en` 或 `zh-TW`、alc 的版本 |
| `mod.addPage`、`addCommand`、`addKey`、`addSessionChip`、`addStyle`、`onCleanup` | 替頁面加東西 |
| `mod.addWidget`、`addWindow`、`addBackground` | 可放置的小工具、浮動視窗、背景 |
| `mod.state`、`mod.save(next)` | 這個 mod 存下來的物件 |
| `mod.local.get(name)`、`mod.local.set(name, value)` | 只存在這台裝置上的儲存空間 |
| `alc.layout()`、`alc.widgets()`、`alc.openWindow(key)`、`alc.notify(title, body)` | 版面、小工具、打開視窗、通知 |

資料夾裡的 `AGENTS.md` 是完整的參考，包括卡片的欄位與 CSS 掛鉤（`body[data-pane]`、
`#nav`、`#keys`、`.panel`、`.chip` …）。

## 主題

![Fjord 主題](/img/remote/theme-fjord.png)

主題把顏色 token 疊在淺色或深色的底上，也可以設定終端機的調色盤。把它存成
`themes/<id>.json`，再到「設定」裡挑選：

```json
{
  "name": "Tidepool",
  "mode": "dark",
  "tokens": {
    "--bg": "#081214",
    "--surface": "#0d1b1e",
    "--accent": "#2dd4bf",
    "--chart-1": "#2dd4bf"
  },
  "terminal": { "background": "#061012", "foreground": "#d6f1ec", "cursor": "#2dd4bf" }
}
```

值可以是顏色、`rgb()`、`color-mix()` 與長度 —— 不能有 `url()`、引號或分號；其他的
alc 都會丟掉。`AGENTS.md` 列出了每一個 token。

## 套件

套件是 `packages/<id>.json`：

```json
{
  "name": "My setup",
  "description": "Notes on the right, the clock up top, a quiet background.",
  "mods": ["clock", "session-notes"],
  "theme": "fjord",
  "density": "compact",
  "background": { "kind": "preset", "value": "aurora", "dim": 50, "blur": 0, "terminal": 92 },
  "layout": {
    "slots": {
      "bar": [{ "widget": "clock/clock", "options": { "seconds": false } }],
      "view.right": [{ "widget": "session-notes/note", "options": {} }]
    },
    "sizes": { "right": 300 },
    "filesSide": "left"
  }
}
```

套用時會開啟它的 mods（缺少的官方 mods 會自動安裝），並採用它的版面、背景、主題、
強調色與密度。它沒有列出的 mods 維持原本的開關。最簡單的做法是先把頁面排好，再在
工作室按**把目前的頁面存成套件**；**下載**會給你可以分享的檔案，repository 裡的
`packages/` 資料夾也會跟其他東西一起安裝。

## 在 GitHub 上分享你的 mod

一個 repository 只要符合下面任一種結構（或兩種都有），就是 mod 的來源：

```text
alc-mod-standup/              one mod: installed as mods/standup
├── mod.json
├── mod.js
├── mod.css
└── README.md

my-alc-mods/                  a collection: each folder keeps its id
├── mods/
│   ├── standup/mod.json, mod.js
│   └── focus-timer/mod.json, mod.js, mod.css
├── themes/
│   └── harbour.json
└── packages/
    └── standup-desk.json
```

- 單一 mod 的 repository 會以它的名字安裝，並去掉 `alc-mod-`、`alc-page-mod-` 或
  `alc-` 前綴；用 `--name` 可以改成別的名字。
- 從一個 mod 只會取 `mod.json`、`mod.js`、`mod.css` 與 `README.md`，另外只取
  `themes/*.json` 與 `packages/*.json`；repository 裡其他的東西一律忽略，所以可以放測試、截圖或建置流程。
- 在 README 裡放一張截圖和一句說明 mod 做什麼的話，並替 repository 加上 `alc-mod`
  這個 topic，讓別人找得到。
- mod 裡不能有任何機密：唯讀連結也會下載 mod 的檔案。

## 從 repository 安裝

```sh
alc remote web install someone/alc-mod-standup          # default branch
alc remote web install someone/my-alc-mods@v1.2         # a tag, branch or commit
alc remote web install someone/monorepo/alc/mods@main   # a folder inside a repository
alc remote web install https://github.com/someone/my-alc-mods/tree/main/packs/focus
alc remote web install ./path/to/a/clone                # a folder on this machine
```

```text
fetching someone/my-alc-mods@v1.2
installed mods/standup (off)
installed mods/focus-timer (off)
installed themes/harbour.json

mods run with the page's link and are not sandboxed; read them first:
  /home/you/.config/alc/web/mods
then turn one on with `alc remote web enable <id>` or the switch in Settings
```

**裝好的 mod 預設是關的。** 先讀過 `mod.js`，再用 `alc remote web enable <id>` 或
「設定」→「Mods」裡的開關，把你信得過的打開。主題只是顏色，裝好就能挑。

| 指令 | 作用 |
| --- | --- |
| `alc remote web list` | 每個 mod（開、關或錯誤，以及它從哪裡來）與主題 |
| `alc remote web enable <id>` / `disable <id>` | 開啟或關閉一個 mod |
| `alc remote web install <source> --force` | 更新：取代已安裝的版本；除非加 `--enable`，mod 會再次變成關閉 |
| `alc remote web install <source> --enable` | 安裝並一次打開，適合你自己的 repository |
| `alc remote web remove <id>` | 刪除這個 mod 與它存下來的狀態 |
| `alc remote web check` | 回報資料夾裡的每一個問題 |

每個裝好的 mod 會把來源記在 `mods/<id>/SOURCE`，`list` 會把它印出來。下載一律透過
HTTPS 從 `codeload.github.com` 取得，每個 repository 上限 8 MB、每個檔案上限
256 KB；除了 `web/mods/<id>/` 與 `web/themes/`，其他地方一律不寫入。

## 安全性

- **mod 是你信任的程式碼。** 它在頁面裡以頁面的連結身分執行 —— 在 operator 連結上，
  它可以替你的 agent 打字。開啟之前先讀過，就像對待你放進 PATH 的腳本一樣。
- **只有磁碟上的位元組會執行。** hub 會把每個已啟用 `mod.js` 的雜湊放進頁面的
  content-security policy，所以 mod 不能載入其他腳本、不能 `eval`，也不能向其他
  origin 發出請求；頁面載入之後才被修改的腳本，要等頁面重新載入才會執行。
- **唯讀連結**執行同樣的 mods，但無法透過它們存設定或打字，「設定」分頁對它們也是
  唯讀的。
- **壞掉的 `settings.json` 會讓所有 mod 都關掉**，而且在 `alc remote web check`
  沒有問題之前，頁面拒絕覆寫它，所以一個打錯的字不會讓你關掉的 mod 重新被打開，也
  不會抹掉你手動改過的內容。

## 疑難排解

| 症狀 | 要看哪裡 |
| --- | --- |
| mod 沒出現在頁面上 | `alc remote web list` —— 它是 `on` 嗎？開啟之後要重新載入頁面。 |
| 「設定」裡的 mod 旁邊出現「錯誤」 | 那裡顯示的訊息，或瀏覽器主控台（`[alc mod <id>]`） |
| 「mod.js changed after this page loaded」 | 重新載入頁面 |
| `check` 印出 `problem:` 開頭的行 | 修好它指出的檔案；settings.json 的錯誤會讓所有 mod 停擺 |
| 主題看起來沒變 | 含有 `url()`、引號或 `;` 的 token 值會被丟掉；`check` 會指出是哪個檔案 |
