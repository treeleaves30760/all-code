---
id: mods
title: Mods and themes
sidebar_label: Mods and themes
sidebar_position: 6
description: Customise alc's remote-control page with themes and mods - write your own, ask your agent to write one, publish it as a GitHub repository, and install other people's with one command.
keywords:
  - alc mods
  - alc themes
  - remote page customisation
  - herdr mods
  - install mod from github
---

# Mods and themes

The [remote-control page](./remote-control.md) can be changed by plain files
on the machine running the hub: **themes** recolour it, **mods** add to it — a
tab, commands in the palette, buttons on the key bar, chips on session cards,
or styles. The idea comes from herdr-app: the page's behaviour is files an
agent can write, next to a reference written for that agent.

![Mods in the Settings tab](/img/remote/mods.png)

## Where the files live

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

Start from an example:

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
| `mod.state`, `mod.save(next)` | the mod's saved object |

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
└── themes/
    └── harbour.json
```

- A single-mod repository is installed under its name with an `alc-mod-`,
  `alc-page-mod-` or `alc-` prefix removed; `--name` picks another.
- Only `mod.json`, `mod.js`, `mod.css` and `README.md` are taken from a mod,
  and only `themes/*.json` besides; everything else in the repository is
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
