---
id: mods
title: Mods 與主題
sidebar_label: Mods 與主題
sidebar_position: 6
description: 用主題與 mod 自訂 alc 的遠端控制頁面 —— 自己寫、請 agent 幫你寫、把它當成 GitHub repository 發佈，並用一行指令安裝別人寫的。
keywords:
  - alc mods
  - alc themes
  - remote page customisation
  - herdr mods
  - install mod from github
  - 中文
---

# Mods 與主題

[遠端控制頁面](./remote-control.md)可以用跑 hub 那台機器上的純文字檔案來改：**主題**
替它換顏色，**mod** 替它加東西 —— 一個分頁、指令面板裡的指令、按鍵列上的按鈕、
session 卡片上的小標籤，或樣式。這個想法來自 herdr-app：頁面的行為是 agent
寫得出來的檔案，旁邊附一份寫給那個 agent 看的參考文件。

![「設定」分頁裡的 Mods](/img/remote/mods.png)

## 檔案放在哪裡

```sh
alc remote web          # prints the folder, usually ~/.config/alc/web
```

```text
web/
├── AGENTS.md          the API reference, written for an agent; alc keeps it current
├── settings.json      what the Settings tab saves (theme, terminal, which mods are off)
├── themes/<id>.json   colour tokens for the page and the terminal
├── mods/<id>/
│   ├── mod.json       {"name", "description", "version", "author"}
│   ├── mod.js         optional: runs once when the page loads
│   ├── mod.css        optional: added after the page's own stylesheet
│   └── README.md      optional: shown to people, not loaded
└── examples/          quick-prompts, scratchpad, session-age, tidepool; never loaded
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

從一個範例開始：

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
| `mod.state`、`mod.save(next)` | 這個 mod 存下來的物件 |

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
└── themes/
    └── harbour.json
```

- 單一 mod 的 repository 會以它的名字安裝，並去掉 `alc-mod-`、`alc-page-mod-` 或
  `alc-` 前綴；用 `--name` 可以改成別的名字。
- 從一個 mod 只會取 `mod.json`、`mod.js`、`mod.css` 與 `README.md`，另外只取
  `themes/*.json`；repository 裡其他的東西一律忽略，所以可以放測試、截圖或建置流程。
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
