---
id: mods
title: Mods and themes
sidebar_label: Mods and themes
sidebar_position: 6
description: Customise alc's remote-control page with themes, mods, widgets and packages - arrange it in the in-page Studio, write your own mod from a template, publish it as a GitHub repository, and install other people's with one command.
keywords:
  - alc mods
  - alc themes
  - remote page customisation
  - herdr mods
  - install mod from github
  - alc studio
  - mod packages
  - page widgets
---

# Mods and themes

The [remote-control page](./remote-control.md) can be changed by plain files
on the machine running the hub: **themes** recolour it, **mods** add to it — a
tab, commands in the palette, buttons on the key bar, chips on session cards,
or styles — and **widgets** that you place wherever you want them in the session
list and the coding view, **floating windows**, and **backgrounds**. A
**package** bundles mods with their layout, background and theme. The idea
comes from herdr-app: the page's behaviour is files an agent can write, next to
a reference written for that agent.

Everything here can be done from the page's **Studio** tab, by hand in the
folder, or by asking your agent.

![The Studio's packages](/img/remote/studio-packages.png)

## The Studio

The **Studio** tab is the page's built-in mods creator. Its five tabs all write
to the same folder on the hub, so a change made on a laptop is there on the
phone:

- **Packages** — apply one of the official packages (Cockpit, Night shift,
  Deep focus, Retro terminal, Workbench) or your own; save the page as it is
  now as a package; import one from JSON or a file; download or copy one to
  share.
- **Layout** — a library of every widget the enabled mods offer, and
  miniatures of the session list and the coding view. Drag a widget into a
  slot (or pick it and choose **Where**), reorder it, hide it, and set its
  options in the inspector.
- **Mods** — switch mods on and off, install official ones, install from
  GitHub, remove, and start a **new mod** from a template.
- **Background** — a built-in gradient or pattern, an image you upload, or an
  animated one a mod draws; with dim, blur and terminal opacity.
- **Panels** — the widths of the session list, the file panel and the two
  docks, which side the list and the file panel sit on, and whether plain
  sessions are fitted to the page.

![Placing widgets in the Studio](/img/remote/studio-layout.png)

A view-only link sees the Studio but cannot change anything in it.

### Slots

| Slot | Where |
| --- | --- |
| `bar` | the header, beside the connection light — keep it to one line |
| `list.top`, `list.bottom` | above and below the session cards |
| `card` | inside every session card (display only: the card is a button) |
| `view.top`, `view.bottom` | above and below the terminal |
| `view.left`, `view.right` | docks beside the terminal; each widget gets a collapsible section |
| `float` | a floating window of its own |

A widget nobody has placed is drawn where its mod suggests until you move or
hide it, so turning a mod on shows its widget straight away. A package's layout
is *explicit* instead: only what it places is drawn. **Reset layout** goes back
to the suggestions.

### Panels, and fitting a plain session

Drag the inner edge of the session list, the file panel or a dock to resize it;
double-click the edge to reset it, or use the arrow keys on it. The terminal
refits to whatever is left.

A session started with `--tmux` already takes the page's size. A plain one is
drawn at its own terminal's size, which is why opening the file panel used to
shrink it into a letterbox. **Fit to this page** (the button beside the file
panel's, the size note, the palette, or *Plain sessions* in the Studio's Panels
tab for every session) lets the page resize it instead, so the terminal fills
the space the panels leave. The terminal that launched it takes the size back
the moment it is resized, and the hub hands it back when the page lets go or
closes.

![A plain session fitted beside the file panel](/img/remote/fit.png)

### Windows

A widget in the `float` slot, or a mod's own `mod.addWindow`, is a window you
drag by its title bar and resize from its corner. Where it is and whether it is
open are kept per device; a closed window waits in the header's window tray
(and in the palette). On a phone windows start closed and open as a sheet
above the keys.

## Official mods and packages

alc ships these in `examples/`. Install one from the Studio, or
`alc remote web add <id>` — official mods arrive turned on.

| Mod | What it adds |
| --- | --- |
| `clock` | the time, and a second zone, in the header or a window |
| `session-overview` | running, agents, viewers and the longest session as tiles over the list |
| `quota-meter` | each login's 5-hour and weekly quota as bars, or the worst one in the header |
| `session-notes` | a note per session beside its terminal, kept on the hub |
| `command-deck` | the open agent's slash commands as buttons (`/compact`, `/review`, `/status` …) |
| `pomodoro` | a focus timer window with notifications |
| `ambient` | animated backgrounds: starfield, glyph rain, waves, particles |
| `crt` | scanlines and phosphor glow on the terminal (CSS only) |
| `quick-prompts` | prompt buttons on the key bar, in the palette, and as an editable widget |
| `scratchpad` | a Notes tab and a notes widget |
| `session-age` | an hours chip, and an age bar on each card |

| Package | What it is |
| --- | --- |
| `cockpit` | tiles over the list, quota and a session note in the right dock, the command deck under the terminal, the clock, age bars |
| `night-shift` | a starfield behind a translucent terminal, the Midnight theme, the focus timer and the clock |
| `deep-focus` | compact, a blurred gradient, the timer in the header, prompt buttons under the terminal |
| `retro` | CRT terminal, glyph rain, the Ember theme, a notes window |
| `workbench` | files on the left, notes and quick prompts in a right dock, the overview in the header |

```sh
alc remote web catalog                 # everything official, and what is installed
alc remote web add cockpit             # an official package (or mod, or theme)
alc remote web package apply cockpit   # installs the official mods it needs, too
alc remote web package                 # list packages; * marks the applied one
```

![The Cockpit package](/img/remote/cockpit.png)

## Where the files live

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

An id is `[a-z0-9_-]`, starts with a letter or digit, and is at most 48
characters. A mod needs `mod.js`, `mod.css`, or both.

## The quickest way: ask your agent

Open **Settings → Mods** and copy the prompt under **Ask your agent**, or type
it yourself in any session:

```text
Read ~/.config/alc/web/AGENTS.md, then make me an alc page mod that adds a
"Deploy" key which types `npm run deploy` into the open session.
```

`AGENTS.md` tells the agent the folder layout, every API call, the theme
tokens, and to finish with `alc remote web check`. The page notices the new
files within a few seconds.

## Write one by hand

Start from a template — from the Studio's Mods tab, or:

```sh
alc remote web new my-widget --template widget   # or page, window, background, style, blank
```

That writes `mods/my-widget/` with its id and name filled in, turned on. Or
copy an official mod:

```sh
WEB="$(alc remote web)"
cp -R "$WEB/examples/mods/quick-prompts" "$WEB/mods/my-prompts"
```

Edit `mods/my-prompts/mod.json` to give it a name, then `mod.js`:

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

Turn it on and check it:

```sh
alc remote web enable my-prompts
alc remote web check            # "ok: …" or one "problem: …" line per issue
```

The page offers a **Reload** for a changed or newly enabled script, because it
only runs a script under the hash the hub computed when it served the page.
Stylesheets and themes apply without one.

### A mod with its own tab

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

![The scratchpad example's Notes tab](/img/remote/mod-page.png)

### A widget

A widget is something the Studio or a package places. The mod says where it
*may* go and where it goes until someone places it; options appear in the
Studio's inspector.

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

`ctx` also has `session()` (the open session), `card` (in the card slot),
`on(event, fn)`, `onCleanup(fn)`, `active()` and `setOptions({...})`. The view's
slots are redrawn for each session you open, so a widget there can simply read
`ctx.session()`.

### A window, a background

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

A background mod should draw one still frame when `ctx.reducedMotion()` is
true, and pause while the tab is hidden.

### Keeping state

`mod.state` is the mod's saved object and `mod.save(next)` writes it into
`settings.json` under `modSettings.<id>`, so it is the same on every device.
Saving needs a link that can type; on a view-only link it rejects.

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

### The API

| Call | What it does |
| --- | --- |
| `alc.define(id, setup)` | runs `setup(mod)` with errors attributed to the mod |
| `alc.on(event, fn)` | `sessions`, `attach`, `detach`, `page`, `usage`, `settings`; returns an unsubscribe |
| `alc.sessions()` / `alc.current()` | session cards / the open one |
| `alc.open(target)` | `sessions`, `usage`, `settings`, a mod tab, or `session:<id>` |
| `alc.api('/api/…')` | the page's JSON routes: sessions, usage, usage history, web, caps |
| `alc.type(text)` / `alc.key(data)` | paste into / send raw bytes to the open session |
| `alc.toast(text, level)` | `info`, `warn` or `error` |
| `alc.h(tag, props, ...children)` | a small element builder |
| `alc.settings()` / `alc.theme()` | the settings in effect / `{id, mode}` |
| `alc.canType`, `alc.locale`, `alc.version` | the link's grade, `en` or `zh-TW`, alc's version |
| `mod.addPage`, `addCommand`, `addKey`, `addSessionChip`, `addStyle`, `onCleanup` | add to the page |
| `mod.addWidget`, `addWindow`, `addBackground` | a placeable widget, a floating window, a background |
| `mod.state`, `mod.save(next)` | the mod's saved object |
| `mod.local.get(name)`, `mod.local.set(name, value)` | storage on this device only |
| `alc.layout()`, `alc.widgets()`, `alc.openWindow(key)`, `alc.notify(title, body)` | the layout, the widgets, open a window, a notification |

`AGENTS.md` in the folder is the full reference, including the card fields and
the CSS hooks (`body[data-pane]`, `#nav`, `#keys`, `.panel`, `.chip` …).

## Themes

![The Fjord theme](/img/remote/theme-fjord.png)

A theme lays colour tokens over the light or dark base and may set the
terminal's palette. Save it as `themes/<id>.json` and pick it in Settings:

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

Values may be colours, `rgb()`, `color-mix()` and lengths — no `url()`,
quotes or semicolons; alc drops anything else. `AGENTS.md` lists every token.

## Packages

A package is `packages/<id>.json`:

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

Applying it turns its mods on (installing any official ones that are missing),
and takes its layout, background, theme, accent and density. Mods it does not
name keep their switches. The easiest way to make one is to arrange the page,
then **Save this page as a package** in the Studio; **Download** gives you the
file to share, and a repository's `packages/` folder installs with the rest.

## Share your mods on GitHub

A repository is a mod source if it has either layout, or both:

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

- A single-mod repository is installed under its name with an `alc-mod-`,
  `alc-page-mod-` or `alc-` prefix removed; `--name` picks another.
- Only `mod.json`, `mod.js`, `mod.css` and `README.md` are taken from a mod,
  and only `themes/*.json` and `packages/*.json` besides; everything else in the repository is
  ignored, so it can hold tests, screenshots or a build.
- Put a screenshot and a sentence on what the mod does in the README, and
  tag the repository with the `alc-mod` topic so people can find it.
- A mod must not contain a secret: view-only links download mod files too.

## Install from a repository

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

**Installed mods start off.** Read `mod.js`, then turn the ones you trust on
with `alc remote web enable <id>` or the switch in **Settings → Mods**.
A theme is just colours and is ready to pick.

| Command | What it does |
| --- | --- |
| `alc remote web list` | every mod (on, off or error, and where it came from) and theme |
| `alc remote web enable <id>` / `disable <id>` | turn a mod on or off |
| `alc remote web install <source> --force` | update: replace what is installed; the mod goes off again unless `--enable` |
| `alc remote web install <source> --enable` | install and turn on in one step, for your own repositories |
| `alc remote web remove <id>` | delete the mod and its saved state |
| `alc remote web check` | report every problem in the folder |

Each installed mod records where it came from in `mods/<id>/SOURCE`, which
`list` prints. Downloads come from `codeload.github.com` over HTTPS, capped at
8 MB per repository and 256 KB per file; nothing outside `web/mods/<id>/` and
`web/themes/` is written.

## Safety

- **A mod is trusted code.** It runs in the page with the page's link — on an
  operator link it can type into your agents. Read one before turning it on,
  the same as a script you put on your PATH.
- **Only the bytes on disk run.** The hub puts the hash of each enabled
  `mod.js` in the page's content-security policy, so a mod cannot load other
  scripts, `eval`, or fetch from another origin, and a script edited after
  the page loaded does not run until the page reloads.
- **View-only links** run the same mods but cannot save settings or type
  through them, and the Settings tab is read-only for them.
- **A broken `settings.json` turns every mod off** and the page refuses to
  save over it until `alc remote web check` is clean, so a typo cannot
  re-enable a mod you had turned off or wipe a hand edit.

## Troubleshooting

| Symptom | Look at |
| --- | --- |
| The mod is not on the page | `alc remote web list` — is it `on`? Reload the page after enabling. |
| **Error** beside the mod in Settings | the message there, or the browser console (`[alc mod <id>]`) |
| "mod.js changed after this page loaded" | reload the page |
| `problem:` lines from `check` | fix the file it names; settings.json errors stop all mods |
| A theme looks unchanged | a token value with `url()`, quotes or `;` is dropped; `check` names the file |
