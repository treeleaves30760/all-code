# alc remote page — customisation reference

This folder changes how the alc remote-control page looks and what it can do.
alc writes this file and `examples/`; it never touches anything else here.
The page reloads what changed within a few seconds of a save: settings, themes
and stylesheets apply in place, and a changed script asks the viewer to reload.

```text
web/
├── AGENTS.md            this reference (rewritten by alc; do not edit)
├── settings.json        theme, terminal, which mods are off, layout, background
├── themes/<id>.json     colour tokens for the page and the terminal
├── mods/<id>/           mod.json, mod.js, mod.css
├── layouts/<id>.json    a whole look: mods, widget places, theme, background, terminal font
├── backgrounds/<name>   png, jpg, webp or gif images for the background
└── examples/            official mods, layouts, themes and new-mod templates
    ├── mods/<id>/         copy, or `alc remote web add <id>`
    ├── layouts/<id>.json
    ├── themes/<id>.json
    └── templates/<kind>/  widget, page, window, background, style, blank
```

The page has a **Studio** tab that edits all of this: apply and save
layouts, drag widgets into slots, install mods (official or from GitHub),
start a new mod from a template, pick a background, and size the panels.

A **layout** arranges the whole page and its **mods** draw what is inside it:
a skin mod restyles the page, other mods add widgets, windows and
backgrounds. alc 2.8 called a layout a *package* and kept it in
`packages/<id>.json`; that folder is still read (a `layouts/` file of the same
id wins), and saves always go to `layouts/`.

Validate after every change:

```sh
alc remote web check     # prints "ok: …" or one "problem: …" line per issue
alc remote web           # prints this folder's path
alc remote web new my-widget --template widget   # start a mod from a template
```

Ids (mod folder names, theme file names) are `[a-z0-9_-]`, start with a
letter or digit, at most 48 characters. Symlinks are refused. A mod file is at
most 256 KiB; JSON files at most 64 KiB; all files are UTF-8.

## settings.json

Every field is optional. The settings page writes this same file.

```json
{
  "theme": "system",
  "accent": "#4f46e5",
  "density": "comfortable",
  "terminal": {
    "fontSize": 13,
    "fontFamily": "\"JetBrains Mono\", ui-monospace, monospace",
    "lineHeight": 1.0,
    "cursorStyle": "block",
    "cursorBlink": true,
    "scrollback": 5000
  },
  "disabledMods": ["some-mod"],
  "modSettings": { "scratchpad": { "text": "…" } },
  "layout": {
    "slots": {
      "bar": [{ "widget": "clock/clock", "options": { "seconds": false } }],
      "view.right": [{ "widget": "session-notes/note", "options": {} }]
    },
    "hidden": ["quota-meter/meter"],
    "explicit": false,
    "sizes": { "rail": 320, "files": 420, "left": 300, "right": 320 },
    "filesSide": "right",
    "railSide": "left"
  },
  "background": { "kind": "preset", "value": "aurora", "dim": 35, "blur": 0, "terminal": 100 },
  "activeLayout": "cockpit",
  "layoutMods": ["session-overview"],
  "layoutTerminal": { "fontSize": 14 }
}
```

- `theme`: `system`, `light`, `dark`, a built-in (`midnight`, `daylight`,
  `fjord`, `ember`, `paper`) or the id of a file in `themes/`.
- `accent`: `#rrggbb` or `null` for the theme's own.
- `density`: `comfortable` or `compact`.
- `terminal.fontSize` 8–28 (omit to let the page pick by screen size);
  `lineHeight` 1.0–2.0; `cursorStyle` `block` | `bar` | `underline`;
  `scrollback` 500–100000.
- `disabledMods`: mods present in `mods/` that are not loaded. Switch them with
  `alc remote web enable|disable <id>` or the page's switches; a settings
  save from the page cannot change this list (nor the four layout fields
  below), so a page holding an old copy never turns a mod back on.
- `modSettings`: each mod's own saved state, keyed by mod id. Use
  `mod.save()` rather than editing it by hand while the page is open.
- `terminal.fitPlain`: `true` fits sessions started without `--tmux` to the
  page that opens them (see "Fitting a plain session" below).
- `layout.slots`: slot name → widgets in order. Slots: `bar` (header),
  `list.top`, `list.bottom` (around the session list), `card` (inside every
  session card - display only, it is inside a button), `view.top`,
  `view.bottom` (above and below the terminal), `view.left`, `view.right`
  (docks beside it), `float` (each widget in its own window).
- `layout.hidden`: widgets taken out. A widget neither placed nor hidden is
  drawn in the slot its mod suggests (`defaultSlot`), unless
  `layout.explicit` is true - then only placed widgets are drawn. Applying a
  layout sets `explicit`.
- `layout.sizes`: panel widths in px - `rail` 220–640, `files` 260–1200,
  `left`/`right` docks 180–900. `filesSide`, `railSide`: `left` | `right`.
- `background.kind`: `none`, `preset` (`aurora`, `dusk`, `mesh`, `grid`,
  `dots`, `blueprint`, `sunrise`, `forest`), `image` (a file name in
  `backgrounds/`) or `mod` (`mod/id` from `mod.addBackground`). `dim` 0–95
  (% of the page colour over it), `blur` 0–40 px, `terminal` 30–100 (%
  terminal opacity).
- `activeLayout`: the layout in use, or `null` for Default. The page writes
  it to `body[data-layout]`. Read from the 2.8 `package` key too.
- `layoutMods`, `layoutTerminal`, `layoutRestore`: alc's own bookkeeping for
  the active layout (the mods it switched on, its terminal settings, your
  look from before it). Leave them to `layout apply`/`reset`. Switching a
  mod by hand takes it out of `layoutMods`: it is yours from then on.

If `settings.json` does not parse, the page uses the defaults, **loads no
mods**, and refuses to save over the file until it is fixed — so run
`alc remote web check` after editing it by hand.

## themes/<id>.json

```json
{
  "name": "Tidepool",
  "mode": "dark",
  "tokens": { "--bg": "#0b1416", "--surface": "#101c1f", "--accent": "#2dd4bf" },
  "terminal": { "background": "#071012", "foreground": "#d6f5ef", "cursor": "#2dd4bf" }
}
```

`mode` (`light` | `dark`) picks the built-in palette the tokens are laid over,
so a theme only names what it changes. Values may contain letters, digits,
spaces and `#%.,()-/+` — colours, `rgb()`, `color-mix()`, lengths. No `url()`.

Page tokens:

| Token | Used for |
|---|---|
| `--bg` | page background |
| `--surface` | cards, header, panels |
| `--raised` | hover fills, inputs, chips |
| `--line`, `--line-strong` | dividers, borders |
| `--text`, `--muted` | body text, secondary text |
| `--accent`, `--accent-ink`, `--accent-soft`, `--accent-line` | brand colour, text on it, tinted fill, tinted border |
| `--ok`, `--warn`, `--danger` (+ `-soft`) | state colours |
| `--term-bg`, `--letterbox` | around the terminal |
| `--chart-1` … `--chart-4` | usage chart series (uncached, cache read, cache write, output) |
| `--r-sm`, `--r-md`, `--r-lg` | corner radii |
| `--font`, `--mono` | not settable from a theme (no quotes allowed); use a skin mod's CSS, and a layout's `terminal.fontFamily` for the terminal |

Terminal keys are xterm.js theme names: `background`, `foreground`, `cursor`,
`cursorAccent`, `selectionBackground`, `black`, `red`, `green`, `yellow`,
`blue`, `magenta`, `cyan`, `white`, and `bright` + each of those.

## mods/<id>/

```text
mods/my-mod/
├── mod.json   {"name": "My mod", "description": "…", "version": "1.0.0", "author": "…"}
├── mod.js     optional; runs once when the page loads
└── mod.css    optional; injected after the page's own stylesheet
```

A mod needs at least one of `mod.js` or `mod.css`. **Mods are not sandboxed:**
`mod.js` runs in the page with the page's link token, the same as the page's
own code. Read any mod before installing it. A viewer (read-only) link runs the
same mods, but every call that types or saves is refused for it.

The page runs only the exact bytes of each enabled `mod.js` (the server puts
their hashes in the content-security policy). So `mod.js` cannot load other
scripts, `eval`, or fetch from another origin; keep a mod in its one file.

### mod.js

Wrap the mod in `alc.define` so errors are attributed to it and shown on the
settings page instead of breaking the page:

```js
alc.define('my-mod', (mod) => {
  mod.addCommand({ id: 'hello', title: 'Say hello', run: () => alc.toast('Hello') });
});
```

`alc` (global):

| Member | What it does |
|---|---|
| `alc.version` | alc's version string |
| `alc.locale` | `en` or `zh-TW` |
| `alc.canType` | `true` for an operator link, `false` for a viewer link |
| `alc.define(id, setup)` | runs `setup(mod)`; `id` must be the folder name |
| `alc.on(event, fn)` | subscribe; returns an unsubscribe function (events below) |
| `alc.sessions()` | the current session cards (array) |
| `alc.current()` | the open session's card, or `null` |
| `alc.page()` | the page on screen: `sessions`, `session`, `usage`, `studio`, `settings`, or `mod:<id>/<page>` |
| `alc.open(page)` | go to `sessions`, `usage`, `studio`, `settings`, a mod page, or `session:<id>` |
| `alc.api(path)` | authenticated `GET` of a same-origin JSON route (see Routes) |
| `alc.toast(text, level)` | a short message; level `info` (default), `warn`, `error` |
| `alc.type(text)` | paste text into the open session; returns `false` if it could not |
| `alc.key(data)` | send raw bytes to the open session (`'\r'`, `'\x1b'`, `'\x03'`) |
| `alc.h(tag, props, ...children)` | small DOM helper: `alc.h('button', {class: 'btn', onclick}, 'Go')` |
| `alc.settings()` | a copy of `settings.json` as loaded |
| `alc.theme()` | `{ id, mode }` of the theme in effect |

`mod` (passed to `setup`):

| Member | What it does |
|---|---|
| `mod.id` | the mod's id |
| `mod.state` | its saved state from `modSettings[id]` (object, `{}` if none) |
| `mod.save(state)` | persist `state` to `modSettings[id]`; a promise; operator links only |
| `mod.addPage({id, title, icon, render})` | adds a tab; `render(root, ctx)` runs each time it is opened |
| `mod.addCommand({id, title, hint, run})` | adds an entry to the command palette (⌘K / Ctrl+K) |
| `mod.addKey({label, data})` or `{label, run}` | adds a button to the session key bar |
| `mod.addSessionChip(fn)` | `fn(card)` returns a string, `{text, tone}` (`ok`, `warn`, `danger`, `accent`) or `null`; shown on each session row |
| `mod.addStyle(css)` | injects CSS (also how `mod.css` arrives) |
| `mod.onCleanup(fn)` | runs before the mod is reloaded |
| `mod.addWidget({id, title, render, ...})` | a widget the studio or a layout places in a slot (below) |
| `mod.addWindow({id, title, render, width, height, open})` | a floating window; returns `{open, close, toggle}` |
| `mod.addBackground({id, name, render})` | a background the studio can pick |
| `mod.local.get(name, fallback)` / `mod.local.set(name, value)` | storage on this device only (never throws) |

More on `alc`:

| Member | What it does |
|---|---|
| `alc.layout()` | a copy of `settings.layout` |
| `alc.widgets()` | every registered widget (without `render`) |
| `alc.openWindow(key)` | opens a window: `mod/window`, or a float widget's `mod/widget` |
| `alc.notify(title, body)` | a toast, and a system notification where the page may show one |

### Widgets

```js
alc.define('my-mod', (mod) => {
  mod.addWidget({
    id: 'status',                       // key is "my-mod/status"
    title: 'Status',
    description: 'Shown in the studio.',
    icon: 'spark',                      // as for addPage
    defaultSlot: 'view.right',          // where it appears until placed; omit for nowhere
    slots: ['bar', 'view.right', 'float'], // where it may go; omit for anywhere
    size: { width: 320, height: 240 },  // a floating window's first size
    options: [                          // edited in the studio's inspector
      { key: 'label', type: 'text', label: 'Label', default: 'Hi' },
      { key: 'on', type: 'boolean', label: 'Enabled', default: true },
      { key: 'mode', type: 'select', label: 'Mode', default: 'a',
        choices: [{ value: 'a', label: 'A' }, { value: 'b', label: 'B' }] },
      { key: 'n', type: 'number', label: 'Count', default: 3, min: 1, max: 9 },
      // also 'color' and 'textarea'
    ],
    render(root, ctx) {
      root.append(alc.h('span', {}, ctx.options.label));
      ctx.every(5000, () => { /* refresh */ });
      return () => { /* optional: stop anything ctx does not own */ };
    },
  });
});
```

`render(root, ctx)` runs whenever the widget is drawn: when it is placed or
moved, when its options change, and - for the `view.*` slots - each time a
session is opened. Everything registered through `ctx` is undone before it
runs again. Keep `card` widgets cheap: they are redrawn on every poll.

| `ctx` | What it is |
|---|---|
| `ctx.slot` | the slot it is in |
| `ctx.options` | its options, defaults filled in |
| `ctx.session()` | the open session's card, or `null` |
| `ctx.card` | in the `card` slot: the card being drawn |
| `ctx.canType` | as `alc.canType` |
| `ctx.on(event, fn)` | `alc.on`, unsubscribed when the widget goes |
| `ctx.every(ms, fn)` | an interval (at least 250 ms) paused while the tab is hidden |
| `ctx.onCleanup(fn)` | runs when the widget goes |
| `ctx.active()` | `false` once it has gone |
| `ctx.setOptions(obj)` | saves options for this placement |

Widgets in `bar` should stay one line; a `view.left`/`view.right` dock gives
each widget a collapsible section with its title. Shared classes for widgets:
`.w-stat`, `.w-stat-value`, `.w-stat-label`, `.w-meter` (+ `.warn`,
`.danger`, a `<span style="width:…%">` inside), `.w-list`, `.w-row`, `.btn`,
`.btn.small`, `.btn.accent`, `.chip`, `.pill`, `.muted`.

### Windows and backgrounds

`mod.addWindow` takes the same `render(root, ctx)` as a widget. A window is
dragged by its title bar and resized from its corner; position, size and
whether it is open are kept on each device. Closed windows wait in the
header's window tray, and every window has a palette command.

`mod.addBackground({id, name, render(host, ctx)})` draws into a full-page
element behind everything; return a function that stops it. `ctx.active()`,
`ctx.onCleanup(fn)`, `ctx.mode()` (`light`/`dark`) and `ctx.reducedMotion()`
(draw one still frame when it is true). Pause work while `document.hidden`.

### Fitting a plain session

A session started without `--tmux` is drawn at its own terminal's size. An
operator page can take that size ("Fit to this page" in the header, the
size note, the palette, or `terminal.fitPlain`): the terminal then refits to
whatever room the file panel and docks leave. The launching terminal gets
the size back as soon as it is resized, and the hub hands it back when the
page lets go or disconnects.

`icon` is a built-in name (`spark`, `note`, `bolt`, `grid`, `chart`, `clock`,
`star`, `terminal`, `files`, `settings`) or an SVG path `d` string on a 16×16
grid, drawn with a 1.5px stroke.

`render(root, ctx)`: `root` is an empty element filling the page area;
`ctx.active()` is `false` once the user has left the page (stop timers then).

Events for `alc.on`:

| Event | Argument |
|---|---|
| `sessions` | the session cards, after each poll (every 5 s) |
| `attach` | the card of the session just opened |
| `detach` | — |
| `page` | the page id now on screen |
| `usage` | the account report, when the usage page loads it |
| `settings` | the settings object, after any change |

Session card fields: `id`, `name`, `agent`, `provider`, `model`, `effort`, `cwd`,
`state`, `started_at` (unix
seconds), `exit` (null while running), `viewers`, `cols`, `rows`, `tmux`,
`permission`.

### Routes for `alc.api`

| Route | Answers |
|---|---|
| `/api/sessions` | session cards |
| `/api/usage` | accounts and quota windows, the launch ledger |
| `/api/usage/history?days=30&tz=Asia/Taipei` | daily tokens and cost, hours, providers, agents, models |
| `/api/web` | settings, themes, mods, hub version |
| `/api/caps` | what alc can drive on each agent |

### Style hooks for mod.css

The page is plain HTML. Stable hooks: `body[data-layout]` (the active
layout's id; absent for Default), `body[data-pane]` (`list`, `view`,
`usage`, `settings`, `studio`, `page`), `html[data-theme-mode]` (`light`/`dark`),
`#bar` (header), `#nav` (tabs), `#sessions .card`, `#terminal`, `#keys`,
`#composer`, `#usage`, `#settings`, `#page` (mod pages), `.panel`, `.btn`,
`.chip`, and for this release's parts `body[data-pane="studio"]`, `#studio`,
`.dock`, `#dockLeft`, `#dockRight`, `#viewTop`, `#viewBottom`, `#listTop`,
`#listBottom`, `#barSlot`, `.card-slot`, `.widget[data-widget="mod/id"]`,
`.win`, `.win-head`, `.win-body`, `#backdrop`, `body[data-backdrop]`.
Prefer tokens over hard-coded colours so a mod works in every theme.

## layouts/<id>.json

```json
{
  "name": "Cockpit",
  "description": "…",
  "version": "1.0.0",
  "author": "you",
  "mods": ["cockpit-skin", "clock", "session-notes"],
  "theme": "midnight",
  "accent": "#4f46e5",
  "density": "compact",
  "background": { "kind": "preset", "value": "aurora", "dim": 40 },
  "layout": { "slots": { "bar": [{ "widget": "clock/clock" }] } },
  "terminal": { "fontFamily": "\"Iosevka\", monospace", "fontSize": 14, "lineHeight": 1.1, "cursorStyle": "bar", "cursorBlink": false }
}
```

Every field but `name` is optional; `terminal` takes any of those five keys,
clamped like `settings.json`'s.

Applying a layout (Studio → Layouts, the palette's `Layout: …`, or
`alc remote web layout apply <id>`):

1. switches off the mods the previous layout switched on that this one does
   not name (a mod you had on yourself is never switched off);
2. switches its mods on, installing official ones that are missing;
3. takes its theme, accent, density, widget places (as `explicit`),
   background and terminal settings, and sets `activeLayout`.

A mod it names that was installed from elsewhere and is still off has not
been read yet. Then the layout is **not applied at all** - the page asks once,
listing each mod and where it came from, and the CLI needs
`--enable-mods`. **Default** (the first card in Studio → Layouts, the
palette's `Layout: Default`, `alc remote web layout reset`) switches the
layout's mods off and gives back your own theme, widgets, background and
terminal.

### Skins

A layout changes the whole look through a **skin mod** (`<layout>-skin`) whose
CSS is written entirely under `body[data-layout="<layout-id>"]`, so a skin
left switched on does nothing under another layout:

```css
body[data-layout="retro98"] #bar { background: var(--surface); }
body[data-layout="retro98"] .btn { border-radius: 0; }
```

Hooks a skin may rely on, beyond the ones above (kept stable across alc
releases):

| Area | Hooks |
|---|---|
| Header | `#bar`, `#title`, `#subtitle` (`.sub-name`, `.sub-facts`), `.nav`, `.nav-tab[aria-current="page"]`, `#barSlot`, `#link` (`.status-dot`), `#grade` (`.pill.operator`), `#paletteToggle`, `#filesToggle`, `#fitToggle`, `#winTray` |
| Session list | `#list`, `.list-head`, `.filter`, `#sessions .card[aria-current]` (`.avatar`, `.dot`, `.name`, `.meta`, `.cwd`, `.age`, `.chips`), `.empty` |
| Coding view | `#view`, `.perm`, `.perm-label`, `.perm-chip`, `.perm-order`, `.perm-note`, `.perm-flag`, `#sizeNote`, `#stage`, `#center`, `#terminal`, `.dock`, `.keys button` (`.held`), `#composer`, `#compose`, `#send`, `#files` |
| Pages | `#usage`, `#settings`, `#studio`, `#page`, `.page-head`, `.page-title`, `.page-sub`, `.panel`, `.stat`, `.settings-panel`, `.section-title`, `.settings-toc`, `.toc-link`, `.studio-tabs`, `.studio-tab[aria-selected]`, `.layout-card` (`.active`, `.layout-preview`), `.mod-line`, `.theme-card` |
| Overlays | `.win`, `.win-head`, `.win-title`, `.win-close`, `.win-body`, `.dialog`, `.dialog-title`, `.palette-box`, `.palette-item[aria-selected]`, `.menu-list`, `.menu-item`, `#toast` |
| Controls | `.btn` (`.accent`, `.danger`, `.small`), `.pill`, `.badge`, `.chip`, `.segmented button[aria-checked]`, `.switch-track`, `.text-input` |

`body` is a flex column (`#bar`, then `main`), so `order` moves the header
below the page. A skin can also restyle every button at once with a
low-specificity selector such as `body[data-layout="x"] :where(button)`.

Use the hooks above and the tokens; keep the operator/viewer pill, the
permission control, the size note and `:focus-visible` visible, and text at
4.5:1 against what it sits on. Web fonts may be embedded in `mod.css` as
`@font-face { src: url(data:font/woff2;base64,…) }` (the policy allows
`font-src data:`); keep a system stack behind them. The terminal measures its
cells again when a font finishes loading.

## Installing and sharing

```sh
alc remote web list                          # what is installed, on or off
alc remote web catalog                       # official mods, layouts, themes, templates
alc remote web add <id>                      # install an official mod (on), layout or theme
alc remote web layout [list | apply <id> [--enable-mods] | reset | remove <id>]
alc remote web new <id> --template widget    # widget, page, window, background, style, blank
alc remote web install owner/repo[@ref]      # also owner/repo/folder@ref, a github.com URL, or a local folder
alc remote web enable <id>                   # installed mods arrive off
alc remote web disable <id> | remove <id>
```

A repository is a source when it has `mod.json` + `mod.js`/`mod.css` at its
root (one mod, named after the repository without an `alc-mod-` prefix) or
`mods/<id>/…`, `themes/<id>.json` and `layouts/<id>.json` (a collection; a
2.8 `packages/<id>.json` is installed as a layout too). Only `mod.json`, `mod.js`,
`mod.css` and `README.md` are copied from a mod. Each installed mod records its
source in `mods/<id>/SOURCE`. To publish a mod, push that layout to a public
GitHub repository with a README and the `alc-mod` topic.
`treeleaves30760/alc-layouts` is a collection of ten layouts to read and copy:

```sh
alc remote web install treeleaves30760/alc-layouts   # everything arrives off
alc remote web layout apply retro98 --enable-mods    # after reading its mods
```

## Rules

1. Start from `alc remote web new`, or copy from `examples/` into `mods/`,
   `themes/` or `layouts/`; do not edit `examples/` (alc rewrites it).
2. Keep every mod inside `alc.define`, and one mod per folder.
3. Run `alc remote web check` after each change and fix what it reports.
4. Never put a secret in a mod: viewer links download mod files too.
