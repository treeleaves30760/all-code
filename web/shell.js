/* The page's frame: tabs, the command palette, the settings page, themes,
 * and the host that loads mods from the hub's web/ folder.
 *
 * Settings and themes are files on the hub (see web/custom/AGENTS.md), so
 * the page polls one cheap route for a revision and applies what changed.
 * Mods run under a content-security policy that names each enabled script's
 * hash, which the server computes when it serves the index - so a mod is
 * loaded from the exact bytes on disk or not at all, and a script edited
 * since the page loaded needs a reload, which the page offers.
 */

(() => {
  'use strict';

  const shell = window.AlcShell;
  if (!shell) return;
  const { core, T, LOCALE, el, api, toast } = shell;
  const root = document.documentElement;
  const $ = (id) => document.getElementById(id);

  /* ---------------------------------------------------------------- dom */

  /* `h('button', {class: 'btn', onclick}, 'Go')`. Exposed to mods as
   * `alc.h`. Attributes that are false or null are left off, so
   * `{disabled: !canType}` does what it reads as. */
  function h(tag, props, ...children) {
    const node = document.createElement(tag);
    for (const [key, value] of Object.entries(props || {})) {
      if (value === false || value === null || value === undefined) continue;
      if (key === 'class' || key === 'className') node.className = value;
      else if (key === 'style' && typeof value === 'object') {
        for (const [name, part] of Object.entries(value)) {
          if (name.startsWith('--')) node.style.setProperty(name, part);
          else node.style[name] = part;
        }
      }
      else if (key === 'value') node.value = value;
      else if (key === 'checked') node.checked = !!value;
      else if (key.startsWith('on') && typeof value === 'function') node.addEventListener(key.slice(2), value);
      else node.setAttribute(key, value === true ? '' : String(value));
    }
    for (const child of children.flat()) {
      if (child === null || child === undefined || child === false) continue;
      node.append(child instanceof Node ? child : document.createTextNode(String(child)));
    }
    return node;
  }

  const ICONS = {
    spark: 'i-spark',
    note: 'i-note',
    bolt: 'i-bolt',
    grid: 'i-grid',
    chart: 'i-chart',
    usage: 'i-usage',
    clock: 'i-clock',
    star: 'i-star',
    terminal: 'i-terminal',
    files: 'i-files',
    settings: 'i-settings',
    sessions: 'i-sessions',
    puzzle: 'i-puzzle',
    palette: 'i-palette',
    search: 'i-search',
    check: 'i-check',
    copy: 'i-copy',
  };

  /* A sprite icon by name, or a mod's own path on the same 16px grid. */
  function icon(name) {
    const NS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('class', 'glyph');
    svg.setAttribute('aria-hidden', 'true');
    if (ICONS[name] || !name) {
      const use = document.createElementNS(NS, 'use');
      use.setAttribute('href', `#${ICONS[name] || 'i-puzzle'}`);
      svg.append(use);
    } else {
      svg.setAttribute('viewBox', '0 0 16 16');
      const path = document.createElementNS(NS, 'path');
      path.setAttribute('d', String(name).slice(0, 2000));
      svg.append(path);
    }
    return svg;
  }

  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      toast(T.copied);
    } catch {
      // An http:// LAN origin has no clipboard; the text is on screen.
    }
  }

  /* -------------------------------------------------------------- state */

  let web = {
    settings: core.normalizeWebSettings({}),
    themes: [],
    mods: [],
    revision: '',
    dir: '',
    hub: {},
    settingsError: null,
  };
  /* The mods as they were when this page loaded their scripts. */
  let loadedMods = [];
  const systemDark = window.matchMedia('(prefers-color-scheme: dark)');
  let appliedTokens = [];

  /* ------------------------------------------------------------- themes */

  function currentTheme() {
    return core.resolveTheme(web.settings, web.themes, systemDark.matches);
  }

  function applyLook() {
    const theme = currentTheme();
    root.dataset.themeMode = theme.mode;
    root.dataset.theme = theme.id;
    root.style.colorScheme = theme.mode;
    for (const key of appliedTokens) root.style.removeProperty(key);
    appliedTokens = Object.keys(theme.tokens);
    for (const [key, value] of Object.entries(theme.tokens)) root.style.setProperty(key, value);
    document.body.dataset.density = web.settings.density;
    shell.setTerminalLook(web.settings.terminal, theme.terminal);
    const colour = getComputedStyle(document.body).backgroundColor;
    let meta = document.querySelector('meta[name="theme-color"]');
    if (!meta) {
      meta = h('meta', { name: 'theme-color' });
      document.head.append(meta);
    }
    meta.setAttribute('content', colour);
  }

  systemDark.addEventListener('change', () => {
    applyLook();
    if (document.body.dataset.pane === 'settings') renderSettings();
  });

  /* ------------------------------------------------------------- saving */

  let saveTimer = 0;
  let saving = false;
  let dirty = false;
  /* Bumped by every local change and every finished save, so a poll that
   * was already in flight can tell its answer is older than what is here. */
  let saveSeq = 0;
  /* The promises of every change waiting on the next write; all of them
   * settle with that write's outcome. */
  let waiters = [];
  /* Set by a change made mid-drag (a slider, the colour picker): the look
   * follows at once, the controls are not redrawn under the pointer. */
  let quietRender = false;

  function setSaveState(text, level) {
    const node = $('saveState');
    node.textContent = text || '';
    node.className = `save-state${level ? ` ${level}` : ''}`;
  }

  /* Every change applies at once and is written a moment later, so dragging
   * a slider is one save rather than thirty. Resolves true once written. */
  function changeSettings(mutate, immediate, quiet) {
    if (!shell.operator.value) return Promise.resolve(false);
    // The page is holding defaults in place of a file it could not read;
    // writing them would erase the hand edit. The server refuses too.
    if (web.settingsError) {
      toast(`${T.settingsFileError} ${web.settingsError}`, 'error');
      return Promise.resolve(false);
    }
    const next = structuredClone(web.settings);
    mutate(next);
    web.settings = core.normalizeWebSettings(next);
    saveSeq += 1;
    dirty = true;
    quietRender = !!quiet;
    applyLook();
    shell.emit('settings', structuredClone(web.settings));
    setSaveState(T.settingsSaving);
    return new Promise((resolve) => {
      waiters.push(resolve);
      clearTimeout(saveTimer);
      saveTimer = setTimeout(flush, immediate ? 0 : 450);
    });
  }

  async function flush() {
    if (saving) {
      // The write in flight carries an older copy; this one follows it.
      clearTimeout(saveTimer);
      saveTimer = setTimeout(flush, 200);
      return;
    }
    const settle = waiters;
    waiters = [];
    saving = true;
    dirty = false;
    let ok = false;
    try {
      const saved = await api('/api/web/settings', { body: web.settings });
      if (!dirty) web.settings = core.normalizeWebSettings(saved);
      setSaveState(T.settingsSaved, 'ok');
      ok = true;
    } catch {
      setSaveState(T.settingsFailed, 'error');
      toast(T.settingsFailed, 'error');
    } finally {
      saving = false;
      saveSeq += 1;
    }
    for (const resolve of settle) resolve(ok);
  }

  /* ---------------------------------------------------------------- nav */

  const BASE_PAGES = [
    { id: 'sessions', label: () => T.navSessions, icon: 'sessions', pane: 'list' },
    { id: 'usage', label: () => T.navUsage, icon: 'usage', pane: 'usage' },
    { id: 'settings', label: () => T.navSettings, icon: 'settings', pane: 'settings' },
  ];
  /* Tabs mods added: `{id: 'mod:<mod>/<page>', title, icon, render, mod}`. */
  const modPages = [];
  let currentPage = 'sessions';
  let pageToken = 0;

  function drawNav() {
    el.nav.setAttribute('aria-label', T.navLabel);
    el.nav.replaceChildren();
    const pages = [
      ...BASE_PAGES.map((page) => ({ id: page.id, title: page.label(), icon: page.icon })),
      ...modPages.map((page) => ({ id: page.id, title: page.title, icon: page.icon })),
    ];
    for (const page of pages) {
      const active = currentPage === page.id || (page.id === 'sessions' && currentPage === 'session');
      const button = h('button', {
        type: 'button',
        class: 'nav-tab',
        'aria-current': active ? 'page' : 'false',
        title: page.title,
        onclick: () => open(page.id),
      }, icon(page.icon), h('span', { class: 'nav-label' }, page.title));
      el.nav.append(button);
    }
  }

  function open(target) {
    const id = String(target || '');
    if (id === 'sessions') return shell.showPane('list');
    if (id === 'usage') return shell.showPane('usage');
    if (id === 'settings') return shell.showPane('settings');
    if (id.startsWith('session:')) {
      const card = shell.lastCards().find((entry) => entry.id === id.slice(8));
      if (card) shell.attach(card);
      return undefined;
    }
    const page = modPages.find((entry) => entry.id === id);
    if (page) return shell.showPane('page', page.id);
    return undefined;
  }

  shell.on('page', (page) => {
    currentPage = page;
    drawNav();
    if (page === 'settings') renderSettings();
    const modPage = modPages.find((entry) => entry.id === page);
    pageToken += 1;
    if (modPage) renderModPage(modPage);
    else el.page.replaceChildren();
  });

  function renderModPage(page) {
    const mine = pageToken;
    el.page.replaceChildren();
    const host = h('div', { class: 'mod-page', 'data-mod': page.mod });
    el.page.append(host);
    try {
      page.render(host, { active: () => mine === pageToken && currentPage === page.id, mod: page.mod });
    } catch (error) {
      reportModError(page.mod, error);
      host.append(h('p', { class: 'note error' }, core.fill(T.modFailed, { m: page.mod, e: error.message || error })));
    }
  }

  /* ------------------------------------------------------------ palette */

  const palette = $('palette');
  const paletteInput = $('paletteInput');
  const paletteList = $('paletteList');
  const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');
  /* Commands mods added. */
  const modCommands = [];
  let paletteItems = [];
  let paletteIndex = 0;
  let paletteReturn = null;

  el.paletteToggle.setAttribute('aria-label', T.palette);
  el.paletteToggle.title = `${T.palette} (${isMac ? '⌘K' : 'Ctrl+K'})`;
  el.paletteToggle.querySelector('.kbd').textContent = isMac ? '⌘K' : 'Ctrl K';
  palette.setAttribute('aria-label', T.palette);
  paletteInput.placeholder = T.paletteHint;
  paletteInput.setAttribute('aria-label', T.palette);

  function themeChoices() {
    return [
      { id: 'system', name: T.themeSystem },
      { id: 'light', name: T.themeLight },
      { id: 'dark', name: T.themeDark },
      ...Object.entries(core.THEMES).map(([id, theme]) => ({ id, name: theme.name })),
      ...web.themes.filter((theme) => !theme.error).map((theme) => ({ id: theme.id, name: theme.name, custom: true })),
    ];
  }

  function commands() {
    const list = [];
    const go = (id, title, iconName) =>
      list.push({ group: T.paletteGroupNav, title: `${T.paletteGo}: ${title}`, icon: iconName, run: () => open(id) });
    go('sessions', T.navSessions, 'sessions');
    go('usage', T.navUsage, 'usage');
    go('settings', T.navSettings, 'settings');
    for (const page of modPages) go(page.id, page.title, page.icon);
    for (const card of shell.lastCards()) {
      if (card.exit) continue;
      list.push({
        group: T.paletteGroupSessions,
        title: `${T.paletteOpen}: ${card.name}`,
        hint: [card.agent, card.model, core.elidePath(card.cwd)].filter(Boolean).join(' · '),
        icon: 'terminal',
        run: () => shell.attach(card),
      });
    }
    if (shell.attached.value) {
      list.push({ group: T.paletteGroupView, title: T.paletteToggleFiles, icon: 'files', run: shell.toggleFiles });
      list.push({ group: T.paletteGroupView, title: T.paletteToggleRail, icon: 'sessions', run: shell.toggleRail });
    }
    if (shell.operator.value) {
      for (const theme of themeChoices()) {
        list.push({
          group: T.paletteGroupTheme,
          title: `${T.paletteTheme}: ${theme.name}`,
          hint: theme.id === web.settings.theme ? '✓' : '',
          icon: 'palette',
          run: () => changeSettings((s) => {
            s.theme = theme.id;
          }, true),
        });
      }
      list.push({
        group: T.paletteGroupTheme,
        title: T.paletteDensity,
        icon: 'grid',
        run: () => changeSettings((s) => {
          s.density = s.density === 'compact' ? 'comfortable' : 'compact';
        }, true),
      });
    }
    for (const command of modCommands) list.push(command);
    list.push({ group: T.paletteGroupMods, title: T.paletteReloadMods, icon: 'puzzle', run: () => window.location.reload() });
    return list;
  }

  function drawPalette() {
    paletteItems = core.rankCommands(commands(), paletteInput.value);
    paletteIndex = Math.min(paletteIndex, Math.max(0, paletteItems.length - 1));
    paletteList.replaceChildren();
    if (!paletteItems.length) {
      paletteList.append(h('li', { class: 'palette-empty' }, T.paletteEmpty));
      return;
    }
    let group = null;
    const grouped = !paletteInput.value.trim();
    paletteItems.forEach((item, index) => {
      if (grouped && item.group !== group) {
        group = item.group;
        paletteList.append(h('li', { class: 'palette-group', role: 'presentation' }, group));
      }
      const row = h('li', {
        class: 'palette-item',
        role: 'option',
        id: `palette-${index}`,
        'aria-selected': String(index === paletteIndex),
        onclick: () => runPalette(index),
        onpointermove: () => {
          if (paletteIndex !== index) {
            paletteIndex = index;
            markPalette();
          }
        },
      }, icon(item.icon || 'spark'), h('span', { class: 'palette-title' }, item.title),
      item.hint ? h('span', { class: 'palette-hint' }, item.hint) : null);
      paletteList.append(row);
    });
    markPalette();
  }

  function markPalette() {
    for (const row of paletteList.querySelectorAll('.palette-item')) {
      const selected = row.id === `palette-${paletteIndex}`;
      row.setAttribute('aria-selected', String(selected));
      if (selected) row.scrollIntoView({ block: 'nearest' });
    }
    paletteInput.setAttribute('aria-activedescendant', `palette-${paletteIndex}`);
  }

  function openPalette() {
    if (!palette.hidden) return;
    paletteReturn = document.activeElement;
    palette.hidden = false;
    paletteInput.value = '';
    paletteIndex = 0;
    drawPalette();
    paletteInput.focus();
  }

  function closePalette() {
    if (palette.hidden) return;
    palette.hidden = true;
    if (paletteReturn && paletteReturn.focus) paletteReturn.focus();
  }

  function runPalette(index) {
    const item = paletteItems[index];
    closePalette();
    if (!item) return;
    try {
      item.run();
    } catch (error) {
      if (item.mod) reportModError(item.mod, error);
      else console.error(error);
    }
  }

  el.paletteToggle.addEventListener('click', openPalette);
  paletteInput.addEventListener('input', () => {
    paletteIndex = 0;
    drawPalette();
  });
  paletteInput.addEventListener('keydown', (event) => {
    if (event.key === 'ArrowDown' || (event.ctrlKey && event.key === 'n')) {
      event.preventDefault();
      paletteIndex = (paletteIndex + 1) % Math.max(1, paletteItems.length);
      markPalette();
    } else if (event.key === 'ArrowUp' || (event.ctrlKey && event.key === 'p')) {
      event.preventDefault();
      paletteIndex = (paletteIndex - 1 + paletteItems.length) % Math.max(1, paletteItems.length);
      markPalette();
    } else if (event.key === 'Enter') {
      event.preventDefault();
      runPalette(paletteIndex);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      closePalette();
    }
  });
  palette.addEventListener('mousedown', (event) => {
    if (event.target === palette) closePalette();
  });

  /* ⌘K anywhere. Ctrl+K only away from the terminal and the composer: there
   * it is the shell's kill-line, and the terminal owns every chord. */
  document.addEventListener('keydown', (event) => {
    if (event.key !== 'k' && event.key !== 'K') return;
    const inTerminal = el.terminal.contains(event.target) || event.target === el.compose;
    const chord = (isMac && event.metaKey) || (!isMac && event.ctrlKey && !inTerminal) || (isMac && event.ctrlKey && !inTerminal);
    if (!chord || event.altKey || event.shiftKey) return;
    event.preventDefault();
    event.stopPropagation();
    if (palette.hidden) openPalette();
    else closePalette();
  }, true);

  /* ----------------------------------------------------------- settings */

  const settingsBody = $('settingsBody');
  const settingsToc = $('settingsToc');
  $('settingsHeading').textContent = T.settingsTitle;
  el.settings.setAttribute('aria-label', T.settingsTitle);

  const FONTS = [
    { value: '', label: () => T.fontSystem },
    { value: '"JetBrains Mono"', label: () => 'JetBrains Mono' },
    { value: '"Fira Code"', label: () => 'Fira Code' },
    { value: '"Cascadia Code"', label: () => 'Cascadia Code' },
    { value: '"SF Mono"', label: () => 'SF Mono' },
    { value: 'Menlo', label: () => 'Menlo' },
    { value: '"IBM Plex Mono"', label: () => 'IBM Plex Mono' },
    { value: '"Source Code Pro"', label: () => 'Source Code Pro' },
  ];

  /* The page's own default palettes, for drawing a theme's preview card. */
  const BASE = {
    light: { '--bg': '#f6f7f9', '--surface': '#ffffff', '--raised': '#f0f1f4', '--line': '#e4e6eb', '--text': '#15171c', '--muted': '#5a606c', '--accent': '#4f46e5' },
    dark: { '--bg': '#0b0c0f', '--surface': '#131418', '--raised': '#1b1d22', '--line': '#24262c', '--text': '#e8e9ed', '--muted': '#9a9fab', '--accent': '#818cf8' },
  };

  function previewTokens(themeId) {
    const theme = core.resolveTheme({ ...web.settings, theme: themeId, accent: null }, web.themes, systemDark.matches);
    return { ...BASE[theme.mode], ...theme.tokens };
  }

  function themeCard(choice, viewer) {
    const tokens = previewTokens(choice.id);
    const selected = web.settings.theme === choice.id;
    const swatch = (name) => tokens[name] || 'transparent';
    const mock = h('span', { class: 'theme-mock', style: { background: swatch('--bg'), borderColor: swatch('--line') } },
      h('span', { class: 'mock-bar', style: { background: swatch('--surface'), borderColor: swatch('--line') } },
        h('span', { class: 'mock-dot', style: { background: swatch('--accent') } })),
      h('span', { class: 'mock-card', style: { background: swatch('--surface'), borderColor: swatch('--line') } },
        h('span', { class: 'mock-line', style: { background: swatch('--text') } }),
        h('span', { class: 'mock-line short', style: { background: swatch('--muted') } }),
        h('span', { class: 'mock-pill', style: { background: swatch('--accent') } })));
    if (choice.id === 'system') mock.classList.add('split');
    return h('button', {
      type: 'button',
      class: 'theme-card',
      role: 'radio',
      'aria-checked': String(selected),
      'data-key': `theme-${choice.id}`,
      disabled: viewer,
      onclick: () => changeSettings((s) => {
        s.theme = choice.id;
      }, true),
    }, mock, h('span', { class: 'theme-name' }, choice.name, choice.custom ? h('span', { class: 'theme-tag' }, T.themeCustom) : null),
    selected ? h('span', { class: 'theme-check' }, icon('check')) : null);
  }

  function segment(options, current, onPick, key, viewer) {
    return h('div', { class: 'segmented', role: 'radiogroup' }, options.map((option) => h('button', {
      type: 'button',
      role: 'radio',
      'aria-checked': String(option.value === current),
      'data-key': `${key}-${option.value}`,
      disabled: viewer,
      onclick: () => onPick(option.value),
    }, option.label)));
  }

  function field(label, help, control, wide) {
    return h('div', { class: `field${wide ? ' wide' : ''}` },
      h('div', { class: 'field-text' }, h('span', { class: 'field-label' }, label), help ? h('span', { class: 'field-help' }, help) : null),
      h('div', { class: 'field-control' }, control));
  }

  function section(id, title, help, ...body) {
    return h('section', { class: 'settings-section', id: `set-${id}` },
      h('header', { class: 'section-head' }, h('h2', { class: 'section-title' }, title), help ? h('p', { class: 'section-help' }, help) : null),
      h('div', { class: 'panel settings-panel' }, ...body));
  }

  function appearanceSection(viewer) {
    const s = web.settings;
    const accentSwatches = h('div', { class: 'swatches', role: 'radiogroup', 'aria-label': T.accent },
      h('button', {
        type: 'button',
        class: 'swatch-btn default',
        role: 'radio',
        'aria-checked': String(!s.accent),
        'aria-label': T.accentDefault,
        title: T.accentDefault,
        'data-key': 'accent-default',
        disabled: viewer,
        onclick: () => changeSettings((n) => {
          n.accent = null;
        }, true),
      }),
      core.ACCENTS.map((colour) => h('button', {
        type: 'button',
        class: 'swatch-btn',
        role: 'radio',
        style: { '--swatch': colour },
        'aria-checked': String(s.accent === colour),
        'aria-label': colour,
        title: colour,
        'data-key': `accent-${colour}`,
        disabled: viewer,
        onclick: () => changeSettings((n) => {
          n.accent = colour;
        }, true),
      })),
      h('label', { class: 'swatch-custom', title: T.accentCustom },
        h('input', {
          type: 'color',
          value: s.accent || '#4f46e5',
          'aria-label': T.accentCustom,
          'data-key': 'accent-custom',
          disabled: viewer,
          oninput: (event) => changeSettings((n) => {
            n.accent = event.target.value;
          }, false, true),
          onchange: () => renderSettings(),
        })));
    return section('appearance', T.appearance, T.appearanceHow,
      field(T.theme, null, h('div', { class: 'theme-grid', role: 'radiogroup', 'aria-label': T.theme }, themeChoices().map((choice) => themeCard(choice, viewer))), true),
      field(T.accent, null, accentSwatches),
      field(T.density, null, segment([
        { value: 'comfortable', label: T.densityComfortable },
        { value: 'compact', label: T.densityCompact },
      ], s.density, (value) => changeSettings((n) => {
        n.density = value;
      }, true), 'density', viewer)));
  }

  function terminalPreview() {
    const theme = currentTheme();
    const colours = { background: '#0c0d10', foreground: '#e4e5e9', green: '#4ade80', blue: '#60a5fa', yellow: '#facc15', magenta: '#c084fc', brightBlack: '#7c818d', cursor: '#e4e5e9', ...theme.terminal };
    const t = web.settings.terminal;
    const span = (colour, text) => h('span', { style: { color: colours[colour] || colours.foreground } }, text);
    const cursor = h('span', { class: `preview-cursor ${t.cursorStyle}${t.cursorBlink ? ' blink' : ''}`, style: { '--cursor': colours.cursor } });
    return h('div', {
      class: 'term-preview',
      'aria-label': T.preview,
      style: {
        background: colours.background,
        color: colours.foreground,
        fontFamily: t.fontFamily ? `${t.fontFamily}, ui-monospace, monospace` : 'ui-monospace, "SF Mono", Menlo, Consolas, monospace',
        fontSize: `${t.fontSize || 13}px`,
        lineHeight: String(1.25 * t.lineHeight),
      },
    },
    h('div', {}, span('green', '✓'), ' ', span('foreground', 'Edited src/remote/web.rs'), ' ', span('brightBlack', '(+42 −7)')),
    h('div', {}, span('blue', '●'), ' ', span('foreground', 'Running cargo test'), span('brightBlack', ' … 214 passed')),
    h('div', {}, span('magenta', '❯'), ' ', span('yellow', 'alc'), ' share claude ', cursor));
  }

  /* The preview follows a slider without the section being redrawn. */
  function refreshPreview() {
    const old = settingsBody.querySelector('.term-preview');
    if (old) old.replaceWith(terminalPreview());
  }

  function terminalSection(viewer) {
    const t = web.settings.terminal;
    const auto = !t.fontSize;
    const known = FONTS.some((font) => font.value === (t.fontFamily || ''));
    const size = h('div', { class: 'inline-control' },
      h('input', {
        type: 'range',
        min: 8,
        max: 28,
        step: 1,
        value: t.fontSize || 13,
        'aria-label': T.fontSize,
        'data-key': 'font-size',
        disabled: viewer,
        oninput: (event) => {
          event.target.nextElementSibling.textContent = `${event.target.value}px`;
          changeSettings((n) => {
            n.terminal.fontSize = Number(event.target.value);
          }, false, true);
          refreshPreview();
        },
        onchange: () => renderSettings(),
      }),
      h('output', { class: 'range-value' }, auto ? T.fontAuto : `${t.fontSize}px`),
      h('button', {
        type: 'button',
        class: 'btn small',
        'aria-pressed': String(auto),
        'data-key': 'font-auto',
        disabled: viewer,
        onclick: () => changeSettings((n) => {
          n.terminal.fontSize = null;
        }, true),
      }, T.fontAuto));
    const family = h('div', { class: 'inline-control' },
      h('select', {
        'aria-label': T.fontFamily,
        'data-key': 'font-family',
        disabled: viewer,
        onchange: (event) => changeSettings((n) => {
          n.terminal.fontFamily = event.target.value || null;
        }, true),
      }, FONTS.map((font) => h('option', { value: font.value, selected: (t.fontFamily || '') === font.value }, font.label())),
      known ? null : h('option', { value: t.fontFamily, selected: true }, t.fontFamily)),
      h('input', {
        type: 'text',
        class: 'text-input',
        placeholder: '"Your Font", monospace',
        value: known ? '' : t.fontFamily || '',
        spellcheck: 'false',
        'aria-label': T.fontFamily,
        'data-key': 'font-custom',
        disabled: viewer,
        onchange: (event) => changeSettings((n) => {
          n.terminal.fontFamily = event.target.value.trim() || null;
        }, true),
      }));
    const line = h('div', { class: 'inline-control' },
      h('input', {
        type: 'range',
        min: 1,
        max: 2,
        step: 0.05,
        value: t.lineHeight,
        'aria-label': T.lineHeight,
        'data-key': 'line-height',
        disabled: viewer,
        oninput: (event) => {
          event.target.nextElementSibling.textContent = Number(event.target.value).toFixed(2);
          changeSettings((n) => {
            n.terminal.lineHeight = Number(event.target.value);
          }, false, true);
          refreshPreview();
        },
        onchange: () => renderSettings(),
      }),
      h('output', { class: 'range-value' }, t.lineHeight.toFixed(2)));
    const blink = h('label', { class: 'switch' },
      h('input', {
        type: 'checkbox',
        role: 'switch',
        checked: t.cursorBlink,
        'data-key': 'blink',
        disabled: viewer,
        onchange: (event) => changeSettings((n) => {
          n.terminal.cursorBlink = event.target.checked;
        }, true),
      }),
      h('span', { class: 'switch-track', 'aria-hidden': 'true' }),
      h('span', { class: 'switch-text' }, T.cursorBlink));
    const scroll = h('select', {
      'aria-label': T.scrollback,
      'data-key': 'scrollback',
      disabled: viewer,
      onchange: (event) => changeSettings((n) => {
        n.terminal.scrollback = Number(event.target.value);
      }, true),
    }, [1000, 5000, 10000, 25000, 50000, 100000].map((lines) => h('option', { value: lines, selected: t.scrollback === lines }, core.fill(T.scrollbackLines, { n: lines.toLocaleString(LOCALE) }))));

    return section('terminal', T.terminal, T.terminalHow,
      h('div', { class: 'field wide' }, terminalPreview()),
      field(T.fontSize, null, size),
      field(T.fontFamily, null, family),
      field(T.lineHeight, null, line),
      field(T.cursorStyle, null, h('div', { class: 'inline-control' }, segment([
        { value: 'block', label: T.cursorBlock },
        { value: 'bar', label: T.cursorBar },
        { value: 'underline', label: T.cursorUnderline },
      ], t.cursorStyle, (value) => changeSettings((n) => {
        n.terminal.cursorStyle = value;
      }, true), 'cursor', viewer), blink)),
      field(T.scrollback, null, scroll));
  }

  function modRow(mod, viewer) {
    const failed = modErrors.get(mod.id) || mod.error;
    const status = failed ? 'error' : mod.enabled ? 'on' : 'off';
    const label = { on: T.modOn, off: T.modOff, error: T.modError }[status];
    return h('li', { class: `mod-row ${status}` },
      h('span', { class: 'mod-icon' }, icon('puzzle')),
      h('div', { class: 'mod-main' },
        h('div', { class: 'mod-head' },
          h('span', { class: 'mod-name' }, mod.name),
          mod.version ? h('span', { class: 'mod-version' }, `v${mod.version}`) : null,
          h('span', { class: `pill mod-state ${status}` }, label)),
        mod.description ? h('p', { class: 'mod-desc' }, mod.description) : null,
        h('code', { class: 'mod-id' }, `mods/${mod.id}`, mod.hasJs ? ' · mod.js' : '', mod.hasCss ? ' · mod.css' : ''),
        failed ? h('p', { class: 'mod-error' }, String(failed)) : null),
      h('label', { class: 'switch' },
        h('input', {
          type: 'checkbox',
          role: 'switch',
          checked: mod.enabled,
          'aria-label': mod.name,
          'data-key': `mod-${mod.id}`,
          disabled: viewer || !!mod.error,
          onchange: async (event) => {
            const on = event.target.checked;
            const ok = await changeSettings((n) => {
              n.disabledMods = on ? n.disabledMods.filter((id) => id !== mod.id) : [...n.disabledMods, mod.id];
            }, true);
            if (ok) toast(T.modsChanged, 'info', { label: T.reload, run: () => window.location.reload() });
          },
        }),
        h('span', { class: 'switch-track', 'aria-hidden': 'true' })));
  }

  function modsSection(viewer) {
    const dir = web.dir;
    const prompt = core.fill(T.modsPrompt, { d: dir || '~/.config/alc/web' });
    const list = web.mods.length
      ? h('ul', { class: 'mod-list' }, web.mods.map((mod) => modRow(mod, viewer)))
      : h('div', { class: 'mods-empty' }, icon('puzzle'), h('p', {}, T.modsNone));
    const folder = dir
      ? h('div', { class: 'field wide folder' },
        h('div', { class: 'field-text' }, h('span', { class: 'field-label' }, T.modsFolder), h('span', { class: 'field-help' }, T.modsExamples)),
        h('button', { type: 'button', class: 'command', onclick: () => copy(dir) }, icon('copy'), h('span', { class: 'command-text' }, dir)))
      : null;
    const ask = shell.operator.value
      ? h('div', { class: 'field wide ask' },
        h('div', { class: 'field-text' }, h('span', { class: 'field-label' }, T.modsAsk)),
        h('button', { type: 'button', class: 'command prompt', onclick: () => copy(prompt) }, icon('copy'), h('span', { class: 'command-text' }, `${prompt}…`)))
      : null;
    return section('mods', T.mods, T.modsHow, h('div', { class: 'field wide' }, list), folder, ask);
  }

  function aboutSection() {
    const hub = web.hub || {};
    const row = (label, value) => h('div', { class: 'about-row' }, h('dt', {}, label), h('dd', {}, value));
    const kbd = (text) => h('kbd', { class: 'kbd' }, text);
    return section('about', T.about, null,
      h('dl', { class: 'about' },
        row(T.aboutVersion, hub.version || '—'),
        row(T.aboutCeiling, hub.ceiling || '—'),
        row(T.aboutLink, shell.operator.value ? T.operator : T.viewer),
        row(T.aboutSessions, String(shell.lastCards().filter((card) => !card.exit).length)),
        row(T.aboutLanguage, LOCALE),
        row(T.aboutShortcuts, h('span', { class: 'shortcuts' },
          h('span', {}, kbd(isMac ? '⌘K' : 'Ctrl K'), ' ', T.shortcutPalette),
          h('span', {}, kbd(isMac ? '⌘↵' : 'Ctrl ↵'), ' ', T.shortcutSend)))));
  }

  const SECTIONS = [
    ['appearance', () => T.appearance, 'palette'],
    ['terminal', () => T.terminal, 'terminal'],
    ['mods', () => T.mods, 'puzzle'],
    ['about', () => T.about, 'spark'],
  ];

  function renderSettings() {
    if (document.body.dataset.pane !== 'settings') return;
    // Keep the control under the pointer or keyboard where it was: the page
    // is redrawn on every change, and focus is the one thing a redraw loses.
    const focusKey = document.activeElement && document.activeElement.dataset ? document.activeElement.dataset.key : null;
    const scroll = el.settings.scrollTop;
    const viewer = !shell.operator.value;
    $('settingsSub').textContent = viewer ? T.settingsViewer : T.settingsSubtitle;
    settingsToc.replaceChildren(...SECTIONS.map(([id, label, iconName]) => h('a', {
      href: `#set-${id}`,
      class: 'toc-link',
      onclick: (event) => {
        event.preventDefault();
        $(`set-${id}`).scrollIntoView({ behavior: 'smooth', block: 'start' });
      },
    }, icon(iconName), label())));
    const banner = web.settingsError
      ? h('p', { class: 'note error settings-error' }, `${T.settingsFileError} ${web.settingsError}`)
      : null;
    settingsBody.replaceChildren(...[banner, appearanceSection(viewer), terminalSection(viewer), modsSection(viewer), aboutSection()].filter(Boolean));
    el.settings.scrollTop = scroll;
    if (focusKey) {
      const again = settingsBody.querySelector(`[data-key="${CSS.escape(focusKey)}"]`);
      if (again) again.focus({ preventScroll: true });
    }
  }

  // Redrawn when a value changes, but not while a slider is being dragged:
  // replacing the element under the pointer would end the drag.
  let renderTimer = 0;
  shell.on('settings', () => {
    if (quietRender) {
      quietRender = false;
      return;
    }
    clearTimeout(renderTimer);
    renderTimer = setTimeout(renderSettings, 60);
  });

  /* ---------------------------------------------------------------- mods */

  const modErrors = new Map();
  let loadingMod = null;

  function reportModError(id, error) {
    const message = (error && (error.message || String(error))) || 'error';
    modErrors.set(id, message);
    console.error(`[alc mod ${id}]`, error);
    toast(core.fill(T.modFailed, { m: id, e: message }), 'error');
  }

  /* A script's top-level throw surfaces here, synchronously, while the
   * script element is being inserted - which is how it is attributed. */
  window.addEventListener('error', (event) => {
    if (loadingMod) reportModError(loadingMod, event.error || event.message);
  });
  /* The first characters of each mod's script, for attributing a refusal.
   * The violation event arrives in a later task, after `loadingMod` has
   * been cleared, carrying the start of the refused script as its sample. */
  const scriptStarts = new Map();
  document.addEventListener('securitypolicyviolation', (event) => {
    if (!String(event.violatedDirective || '').startsWith('script-src')) return;
    const sample = String(event.sample || '');
    if (!sample) return;
    for (const [id, start] of scriptStarts) {
      if (start.slice(0, sample.length) !== sample) continue;
      reportModError(id, new Error('mod.js changed after this page loaded'));
      toast(T.modsChanged, 'info', { label: T.reload, run: () => window.location.reload() });
      if (document.body.dataset.pane === 'settings') renderSettings();
      return;
    }
  });

  const modStyles = new Map();

  function addStyle(id, css) {
    const style = h('style', { 'data-mod': id });
    style.textContent = String(css);
    document.head.append(style);
    return style;
  }

  async function loadStyles(mods) {
    for (const mod of mods) {
      if (!mod.enabled || !mod.hasCss || mod.error) {
        const old = modStyles.get(mod.id);
        if (old) old.remove();
        modStyles.delete(mod.id);
        continue;
      }
      try {
        const css = await api(`/api/web/mods/${encodeURIComponent(mod.id)}/mod.css`, { text: true });
        const old = modStyles.get(mod.id);
        if (old && old.textContent === css) continue;
        if (old) old.remove();
        modStyles.set(mod.id, addStyle(mod.id, css));
      } catch (error) {
        reportModError(mod.id, error);
      }
    }
    for (const id of [...modStyles.keys()]) {
      if (!mods.some((mod) => mod.id === id)) {
        modStyles.get(id).remove();
        modStyles.delete(id);
      }
    }
  }

  async function loadScripts(mods) {
    for (const mod of mods) {
      if (!mod.enabled || !mod.hasJs || mod.error) continue;
      let text;
      try {
        text = await api(`/api/web/mods/${encodeURIComponent(mod.id)}/mod.js`, { text: true });
      } catch (error) {
        reportModError(mod.id, error);
        continue;
      }
      const script = document.createElement('script');
      script.dataset.mod = mod.id;
      script.textContent = text;
      scriptStarts.set(mod.id, text.slice(0, 40));
      loadingMod = mod.id;
      try {
        document.head.append(script);
      } finally {
        loadingMod = null;
      }
    }
    loadedMods = mods;
    drawNav();
    shell.rebuildKeys();
    shell.rerenderRows();
  }

  /* What a mod's `setup` is handed. */
  function modContext(id) {
    const state = () => {
      const saved = web.settings.modSettings[id];
      return saved && typeof saved === 'object' ? structuredClone(saved) : {};
    };
    const guard = (fn) => (...args) => {
      try {
        return fn(...args);
      } catch (error) {
        reportModError(id, error);
        return undefined;
      }
    };
    return {
      id,
      get state() {
        return state();
      },
      save(next) {
        if (!shell.operator.value) return Promise.reject(new Error('this link cannot save'));
        return changeSettings((n) => {
          n.modSettings = { ...n.modSettings, [id]: next };
        }, true).then((ok) => {
          if (!ok) throw new Error('could not save');
          return true;
        });
      },
      addPage(page) {
        if (!page || !page.id || typeof page.render !== 'function') throw new Error('addPage needs {id, title, render}');
        const pageId = `mod:${id}/${page.id}`;
        if (modPages.some((entry) => entry.id === pageId)) return;
        modPages.push({ id: pageId, title: String(page.title || page.id), icon: page.icon || 'puzzle', render: page.render, mod: id });
        drawNav();
      },
      addCommand(command) {
        if (!command || typeof command.run !== 'function') throw new Error('addCommand needs {title, run}');
        modCommands.push({
          group: T.paletteGroupMods,
          title: String(command.title || command.id),
          hint: command.hint ? String(command.hint) : '',
          icon: command.icon || 'bolt',
          run: guard(command.run),
          mod: id,
        });
      },
      addKey(key) {
        if (!key || !key.label || (!key.run && typeof key.data !== 'string')) throw new Error('addKey needs {label, data} or {label, run}');
        shell.extraKeys.push({ label: String(key.label), data: key.data, run: key.run ? guard(key.run) : null, mod: id });
        shell.rebuildKeys();
      },
      addSessionChip(fn) {
        if (typeof fn !== 'function') throw new Error('addSessionChip needs a function');
        shell.chipDecorators.push(guard(fn));
        shell.rerenderRows();
      },
      addStyle(css) {
        return addStyle(id, css);
      },
      onCleanup(fn) {
        window.addEventListener('beforeunload', guard(fn));
      },
    };
  }

  window.alc = Object.freeze({
    get version() {
      return (web.hub && web.hub.version) || '';
    },
    locale: LOCALE,
    get canType() {
      return shell.operator.value;
    },
    define(id, setup) {
      const name = String(id || loadingMod || '');
      if (typeof setup !== 'function') throw new Error('alc.define(id, setup) needs a function');
      try {
        setup(modContext(name));
      } catch (error) {
        reportModError(name, error);
      }
    },
    on: (event, fn) => shell.on(event, fn),
    sessions: () => shell.lastCards().slice(),
    current: () => shell.attached.value,
    page: () => currentPage,
    open,
    api: (path) => {
      if (typeof path !== 'string' || !path.startsWith('/api/')) return Promise.reject(new Error('alc.api takes an /api/ path'));
      // Soft: a route this link may not use (a viewer asking for files)
      // is the mod's problem, not a reason to forget the page's token.
      return api(path, { soft: true });
    },
    toast: (text, level) => toast(String(text), ['warn', 'error'].includes(level) ? level : ''),
    type: (text) => !!shell.attached.value && shell.operator.value && shell.send({ t: 'paste', data: String(text) }),
    key: (data) => !!shell.attached.value && shell.operator.value && shell.send({ t: 'input', data: String(data) }),
    h,
    settings: () => structuredClone(web.settings),
    theme: () => {
      const theme = currentTheme();
      return { id: theme.id, mode: theme.mode };
    },
  });

  /* -------------------------------------------------------------- polls */

  let scriptsLoaded = false;
  let warnedRevision = '';

  async function pull(first) {
    if (!shell.token.value) return;
    const seqAtStart = saveSeq;
    let next;
    try {
      next = await api('/api/web');
    } catch {
      return;
    }
    const changed = next.revision !== web.revision;
    if (next.grade === 'operator' || next.grade === 'viewer') {
      const canType = next.grade === 'operator';
      if (shell.operator.value !== canType) {
        shell.operator.value = canType;
        if (document.body.dataset.pane === 'settings') renderSettings();
      }
    }
    // Anything changed here since this request left - a pending write, one
    // in flight, or one that finished meanwhile - is newer than the answer.
    const keepLocal = dirty || saving || seqAtStart !== saveSeq;
    web = {
      settings: keepLocal ? web.settings : core.normalizeWebSettings(next.settings),
      themes: Array.isArray(next.themes) ? next.themes : [],
      mods: Array.isArray(next.mods) ? next.mods : [],
      // Kept blank when the answer was set aside, so the next poll applies
      // what the disk holds rather than skipping an unchanged revision.
      revision: keepLocal ? '' : next.revision || '',
      dir: next.dir || '',
      hub: next.hub || {},
      settingsError: next.settingsError || null,
    };
    if (!changed && !first) return;
    applyLook();
    shell.emit('settings', structuredClone(web.settings));
    await loadStyles(web.mods);
    if (!scriptsLoaded) {
      scriptsLoaded = true;
      await loadScripts(web.mods);
    } else if (core.changedScripts(loadedMods, web.mods).length && warnedRevision !== web.revision) {
      warnedRevision = web.revision;
      toast(T.modsChanged, 'info', { label: T.reload, run: () => window.location.reload() });
    }
    if (document.body.dataset.pane === 'settings') renderSettings();
  }

  /* -------------------------------------------------------------- boot */

  drawNav();
  applyLook();
  pull(true);
  setInterval(() => {
    if (!document.hidden) pull(false);
  }, 4000);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) pull(false);
  });
})();
