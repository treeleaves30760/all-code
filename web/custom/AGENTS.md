# alc remote page — customisation reference

This folder changes how the alc remote-control page looks and what it can do.
alc writes this file and `examples/`; it never touches anything else here.
The page reloads what changed within a few seconds of a save: settings, themes
and stylesheets apply in place, and a changed script asks the viewer to reload.

```text
web/
├── AGENTS.md          this reference (rewritten by alc; do not edit)
├── settings.json      theme, accent, terminal font, which mods are off
├── themes/<id>.json   colour tokens for the page and the terminal
├── mods/<id>/         mod.json, mod.js, mod.css
└── examples/          mods and a theme to copy from; never loaded
```

Validate after every change:

```sh
alc remote web check     # prints "ok: …" or one "problem: …" line per issue
alc remote web           # prints this folder's path
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
  "modSettings": { "scratchpad": { "text": "…" } }
}
```

- `theme`: `system`, `light`, `dark`, a built-in (`midnight`, `daylight`,
  `fjord`, `ember`, `paper`) or the id of a file in `themes/`.
- `accent`: `#rrggbb` or `null` for the theme's own.
- `density`: `comfortable` or `compact`.
- `terminal.fontSize` 8–28 (omit to let the page pick by screen size);
  `lineHeight` 1.0–2.0; `cursorStyle` `block` | `bar` | `underline`;
  `scrollback` 500–100000.
- `disabledMods`: mods present in `mods/` that are not loaded.
- `modSettings`: each mod's own saved state, keyed by mod id. Use
  `mod.save()` rather than editing it by hand while the page is open.

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
| `--font`, `--mono` | not settable from a theme (no quotes allowed); use a mod's CSS |

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
| `alc.page()` | the page on screen: `sessions`, `session`, `usage`, `settings`, or `mod:<id>/<page>` |
| `alc.open(page)` | go to `sessions`, `usage`, `settings`, a mod page, or `session:<id>` |
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

The page is plain HTML. Stable hooks: `body[data-pane]` (`list`, `view`,
`usage`, `settings`, `page`), `body[data-theme-mode]` (`light`/`dark`),
`#bar` (header), `#nav` (tabs), `#sessions .card`, `#terminal`, `#keys`,
`#composer`, `#usage`, `#settings`, `#page` (mod pages), `.panel`, `.btn`,
`.chip`. Prefer tokens over hard-coded colours so a mod works in every theme.

## Rules

1. Copy from `examples/` into `mods/` or `themes/`; do not edit `examples/`
   (alc rewrites it).
2. Keep every mod inside `alc.define`, and one mod per folder.
3. Run `alc remote web check` after each change and fix what it reports.
4. Never put a secret in a mod: viewer links download mod files too.
