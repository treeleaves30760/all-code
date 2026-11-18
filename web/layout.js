/* Where mods draw: widget slots, the docks beside the terminal, floating
 * windows, the background behind the page, and the panels a person can drag
 * wider or move to the other side.
 *
 * A widget is registered by a mod (`mod.addWidget`) and placed by the
 * layout in settings.json - written by the studio, or by a package. The
 * layout is data and the widgets are code, so a package can rearrange the
 * page without carrying any code of its own, and a mod can be written
 * without knowing where anyone will want it.
 *
 * Every slot is redrawn only when what it holds changed, so a widget keeps
 * its own state (a running timer, a half-typed note) across the five-second
 * polls and the settings saves that do not concern it. The view's slots are
 * redrawn on each attach, because a widget there is usually about the
 * session on screen and the simplest contract is "render runs again for the
 * next one".
 */

(() => {
  'use strict';

  const shell = window.AlcShell;
  if (!shell || !shell.registry) return;
  const { core, T, el, h, icon } = shell;
  const root = document.documentElement;
  const body = document.body;
  const registry = shell.registry;

  /* ------------------------------------------------------------ helpers */

  function settings() {
    return shell.web().settings;
  }

  function layout() {
    return settings().layout;
  }

  /* A request the studio and the backdrop need: any method, JSON or raw
   * bytes, with the page's token, and the server's own error sentence on a
   * refusal rather than a status code. */
  async function request(method, path, payload, raw) {
    const headers = { Authorization: `Bearer ${shell.token.value || ''}` };
    const init = { method, headers, cache: 'no-store' };
    if (payload !== undefined) {
      if (raw) {
        init.body = payload;
        headers['Content-Type'] = 'application/octet-stream';
      } else {
        init.body = JSON.stringify(payload);
        headers['Content-Type'] = 'application/json';
      }
    }
    const response = await fetch(path, init);
    let answer = null;
    const type = response.headers.get('content-type') || '';
    if (type.includes('json')) answer = await response.json().catch(() => null);
    else if (!response.ok) answer = { error: (await response.text().catch(() => '')) || `http ${response.status}` };
    if (!response.ok) throw new Error((answer && answer.error) || `http ${response.status}`);
    return answer;
  }

  const storage = (() => {
    try {
      return window.localStorage;
    } catch {
      return null;
    }
  })();

  function readLocal(name, fallback) {
    try {
      const raw = storage && storage.getItem(name);
      return raw ? JSON.parse(raw) : fallback;
    } catch {
      return fallback;
    }
  }

  function writeLocal(name, value) {
    try {
      if (storage) storage.setItem(name, JSON.stringify(value));
    } catch {
      // Private mode or a full quota; the page works without it.
    }
  }

  /* Saves a layout change, or for a viewer link (which cannot save) applies
   * it to this page only. */
  function changeLayout(mutate, immediate) {
    if (shell.operator.value && !shell.web().settingsError) {
      return shell.changeSettings((next) => {
        next.layout = core.normalizeLayout(mutate(core.normalizeLayout(next.layout)) || next.layout);
      }, immediate);
    }
    const web = shell.web();
    web.settings.layout = core.normalizeLayout(mutate(core.normalizeLayout(web.settings.layout)) || web.settings.layout);
    shell.emit('settings', structuredClone(web.settings));
    return Promise.resolve(false);
  }

  /* ------------------------------------------------------------ widgets */

  /* Draws one placement into `host` and returns how to take it down. */
  function mount(host, placement, slot, extra, own) {
    const widget = own || registry.widgets[placement.widget];
    const cleanups = [];
    let alive = true;
    const ctx = {
      slot,
      key: placement.widget,
      mod: widget.mod,
      options: structuredClone(placement.options || {}),
      card: extra && extra.card ? extra.card : null,
      session: () => shell.attached.value,
      canType: shell.operator.value,
      active: () => alive,
      onCleanup: (fn) => {
        if (typeof fn === 'function') cleanups.push(fn);
      },
      on: (event, fn) => {
        const off = shell.on(event, (value) => {
          if (alive) fn(value);
        });
        cleanups.push(off);
        return off;
      },
      every: (ms, fn) => {
        const timer = setInterval(() => {
          if (alive && !document.hidden) fn();
        }, Math.max(250, Number(ms) || 1000));
        cleanups.push(() => clearInterval(timer));
        return timer;
      },
      /* Saves this placement's options, so a widget can remember a choice
       * made in it (a collapsed group, a picked tab) without a mod state. */
      setOptions: (options) => changeLayout((next) => {
        const list = next.slots[slot] || [];
        const found = list.find((entry) => entry.widget === placement.widget);
        if (found) found.options = { ...found.options, ...options };
        else return core.placeWidget(next, placement.widget, slot, undefined, { ...placement.options, ...options });
        return next;
      }, true),
    };
    let returned;
    try {
      returned = widget.render(host, ctx);
    } catch (error) {
      console.error(error);
    }
    if (typeof returned === 'function') cleanups.push(returned);
    return () => {
      alive = false;
      for (const fn of cleanups.splice(0)) {
        try {
          fn();
        } catch (error) {
          console.error(error);
        }
      }
    };
  }

  function widgetMap() {
    const out = {};
    for (const [key, widget] of Object.entries(registry.widgets)) {
      out[key] = { slots: widget.slots, defaultSlot: widget.defaultSlot, options: widget.options };
    }
    return out;
  }

  function placements(slot) {
    return core.placementsFor(layout(), slot, widgetMap());
  }

  /* A slot that is a plain element: bar, list strips, view strips. */
  const SIMPLE = {
    bar: () => el.barSlot,
    'list.top': () => el.listTop,
    'list.bottom': () => el.listBottom,
    'view.top': () => el.viewTop,
    'view.bottom': () => el.viewBottom,
  };
  const DOCKS = { 'view.left': () => el.dockLeft, 'view.right': () => el.dockRight };
  const drawn = new Map();

  function drawSimple(slot, force) {
    const host = SIMPLE[slot]();
    const list = placements(slot);
    const signature = JSON.stringify(list);
    const before = drawn.get(slot);
    if (!force && before && before.signature === signature) return;
    if (before) before.destroy();
    host.replaceChildren();
    const downs = [];
    for (const placement of list) {
      const widget = registry.widgets[placement.widget];
      const box = h('div', { class: 'widget', 'data-widget': placement.widget, 'data-slot': slot, title: slot === 'bar' ? widget.title : null });
      host.append(box);
      downs.push(mount(box, placement, slot));
    }
    host.hidden = !list.length;
    drawn.set(slot, { signature, destroy: () => downs.forEach((down) => down()) });
  }

  const collapsed = new Set(readLocal('alc.dock.collapsed', []));

  function drawDock(slot, force) {
    const host = DOCKS[slot]();
    const list = placements(slot);
    const signature = JSON.stringify(list);
    const before = drawn.get(slot);
    const side = slot === 'view.left' ? 'left' : 'right';
    body.dataset[side === 'left' ? 'dockLeft' : 'dockRight'] = list.length ? 'on' : 'off';
    if (!force && before && before.signature === signature) return;
    if (before) before.destroy();
    host.replaceChildren(splitter(side === 'left' ? 'left' : 'right'));
    const scroll = h('div', { class: 'dock-scroll' });
    host.append(scroll);
    const downs = [];
    for (const placement of list) {
      const widget = registry.widgets[placement.widget];
      const id = `${slot}:${placement.widget}`;
      const content = h('div', { class: 'dock-body' });
      const section = h('section', { class: `dock-widget${collapsed.has(id) ? ' collapsed' : ''}`, 'data-widget': placement.widget },
        h('button', {
          type: 'button',
          class: 'dock-head',
          'aria-expanded': String(!collapsed.has(id)),
          onclick: (event) => {
            const now = !section.classList.toggle('collapsed');
            event.currentTarget.setAttribute('aria-expanded', String(now));
            if (now) collapsed.delete(id);
            else collapsed.add(id);
            writeLocal('alc.dock.collapsed', [...collapsed]);
          },
        }, icon(widget.icon), h('span', { class: 'dock-title' }, widget.title), h('span', { class: 'dock-caret', 'aria-hidden': 'true' })),
        content);
      scroll.append(section);
      downs.push(mount(content, placement, slot));
    }
    drawn.set(slot, { signature, destroy: () => downs.forEach((down) => down()) });
  }

  /* Card widgets: drawn per card by app.js on each poll, so they are cheap
   * by contract, and taken down before the next draw of the same card. */
  shell.cardSlot.render = (host, card) => {
    if (host._alcDown) host._alcDown();
    host._alcDown = null;
    const list = placements('card');
    if (!list.length) {
      if (host.childElementCount) host.replaceChildren();
      host.hidden = true;
      return;
    }
    host.hidden = false;
    host.replaceChildren();
    const downs = [];
    for (const placement of list) {
      const box = h('span', { class: 'widget', 'data-widget': placement.widget, 'data-slot': 'card' });
      host.append(box);
      downs.push(mount(box, placement, 'card', { card }));
    }
    host._alcDown = () => downs.forEach((down) => down());
  };

  /* ------------------------------------------------------------ windows */

  /* Geometry and open state live on this device: a window placed for a
   * laptop's screen is in the wrong place on a phone. */
  let geometry = readLocal('alc.windows', {});
  const open = new Map();
  let zTop = 10;

  function saveGeometry() {
    writeLocal('alc.windows', geometry);
  }

  /* Every window there is: float-slot widgets and mods' own windows. */
  function windowSources() {
    const out = new Map();
    for (const placement of placements('float')) {
      const widget = registry.widgets[placement.widget];
      const size = widget.size || {};
      out.set(`w:${placement.widget}`, {
        key: `w:${placement.widget}`,
        title: widget.title,
        icon: widget.icon,
        width: Number(size.width) || 340,
        height: Number(size.height) || 260,
        openByDefault: true,
        draw: (host) => mount(host, placement, 'float'),
        signature: JSON.stringify(placement),
      });
    }
    for (const win of Object.values(registry.windows)) {
      out.set(`m:${win.key}`, {
        key: `m:${win.key}`,
        title: win.title,
        icon: win.icon,
        width: win.width,
        height: win.height,
        openByDefault: win.open,
        draw: (host) => mount(host, { widget: win.key, options: {} }, 'float', null, win),
        signature: win.key,
        mod: true,
      });
    }
    return out;
  }

  /* A phone has no room for a window over the terminal, so there they wait
   * in the tray until asked for. */
  const narrow = window.matchMedia('(max-width: 599px)');

  function isOpen(source) {
    const saved = geometry[source.key];
    if (saved && typeof saved.open === 'boolean') return saved.open;
    return source.openByDefault && !narrow.matches;
  }

  function drawWindows(force) {
    const sources = windowSources();
    for (const [key, entry] of open) {
      const source = sources.get(key);
      if (!source || !isOpen(source) || force || entry.signature !== source.signature) {
        entry.destroy();
        open.delete(key);
      }
    }
    for (const source of sources.values()) {
      if (isOpen(source) && !open.has(source.key)) open.set(source.key, makeWindow(source));
    }
    drawTray(sources);
  }

  function makeWindow(source) {
    const saved = geometry[source.key] || {};
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const count = open.size;
    let rect = core.clampWindow({
      x: saved.x ?? viewport.width - source.width - 24 - count * 24,
      y: saved.y ?? 76 + count * 24,
      width: saved.width ?? source.width,
      height: saved.height ?? source.height,
    }, viewport);
    const content = h('div', { class: 'win-body' });
    const close = h('button', { type: 'button', class: 'icon small win-close', 'aria-label': T.winClose, title: T.winClose }, icon('close'));
    const head = h('div', { class: 'win-head' }, icon(source.icon), h('span', { class: 'win-title' }, source.title), close);
    const grip = h('div', { class: 'win-grip', 'aria-hidden': 'true' });
    const node = h('div', { class: 'win', role: 'dialog', 'aria-label': source.title, 'data-window': source.key }, head, content, grip);
    const place = () => {
      node.style.left = `${rect.x}px`;
      node.style.top = `${rect.y}px`;
      node.style.width = `${rect.width}px`;
      node.style.height = `${rect.height}px`;
    };
    place();
    const raise = () => {
      zTop += 1;
      node.style.zIndex = String(zTop);
    };
    raise();
    node.addEventListener('pointerdown', raise);
    const remember = () => {
      geometry[source.key] = { ...rect, open: true };
      saveGeometry();
    };
    drag(head, (dx, dy, start) => {
      rect = core.clampWindow({ ...start, x: start.x + dx, y: start.y + dy }, { width: window.innerWidth, height: window.innerHeight });
      place();
    }, () => ({ ...rect }), remember);
    drag(grip, (dx, dy, start) => {
      rect = core.clampWindow({ ...start, width: start.width + dx, height: start.height + dy }, { width: window.innerWidth, height: window.innerHeight });
      place();
    }, () => ({ ...rect }), remember);
    close.addEventListener('click', () => closeWindow(source.key));
    el.windows.append(node);
    const down = source.draw(content);
    const onResize = () => {
      rect = core.clampWindow(rect, { width: window.innerWidth, height: window.innerHeight });
      place();
    };
    window.addEventListener('resize', onResize);
    return {
      signature: source.signature,
      node,
      destroy: () => {
        window.removeEventListener('resize', onResize);
        down();
        node.remove();
      },
    };
  }

  /* Pointer drag with a starting snapshot; `done` runs once on release. */
  function drag(handle, move, snapshot, done) {
    handle.addEventListener('pointerdown', (event) => {
      if (event.button !== 0 || event.target.closest('button:not(.win-grip)')) return;
      if (window.matchMedia('(max-width: 599px)').matches) return;
      event.preventDefault();
      const start = snapshot();
      const x0 = event.clientX;
      const y0 = event.clientY;
      handle.setPointerCapture(event.pointerId);
      body.classList.add('dragging');
      const onMove = (e) => move(e.clientX - x0, e.clientY - y0, start);
      const onUp = () => {
        handle.removeEventListener('pointermove', onMove);
        handle.removeEventListener('pointerup', onUp);
        handle.removeEventListener('pointercancel', onUp);
        body.classList.remove('dragging');
        done();
      };
      handle.addEventListener('pointermove', onMove);
      handle.addEventListener('pointerup', onUp);
      handle.addEventListener('pointercancel', onUp);
    });
  }

  function setOpen(key, value) {
    geometry[key] = { ...(geometry[key] || {}), open: value };
    saveGeometry();
    drawWindows(false);
  }

  function openWindow(key) {
    const id = key.startsWith('w:') || key.startsWith('m:') ? key : (registry.windows[key] ? `m:${key}` : `w:${key}`);
    setOpen(id, true);
    const entry = open.get(id);
    if (entry) entry.node.dispatchEvent(new Event('pointerdown'));
  }

  function closeWindow(key) {
    const id = key.startsWith('w:') || key.startsWith('m:') ? key : (registry.windows[key] ? `m:${key}` : `w:${key}`);
    setOpen(id, false);
  }

  function toggleWindow(key) {
    const id = key.startsWith('w:') || key.startsWith('m:') ? key : (registry.windows[key] ? `m:${key}` : `w:${key}`);
    if (open.has(id)) closeWindow(id);
    else openWindow(id);
  }

  /* The header's tray: every window that exists, to open or bring forward. */
  let trayOpen = false;
  function drawTray(sources) {
    const list = [...(sources || windowSources()).values()];
    el.winTray.hidden = !list.length;
    if (!list.length) return;
    const closed = list.filter((source) => !open.has(source.key)).length;
    const button = h('button', {
      type: 'button',
      class: 'icon tray-button',
      'aria-haspopup': 'menu',
      'aria-expanded': String(trayOpen),
      'aria-label': T.winTray,
      title: T.winTray,
      onclick: (event) => {
        event.stopPropagation();
        trayOpen = !trayOpen;
        drawTray();
      },
    }, icon('window'), closed ? h('span', { class: 'tray-count' }, String(closed)) : null);
    const menu = trayOpen
      ? h('div', { class: 'tray-menu', role: 'menu' }, list.map((source) => h('button', {
        type: 'button',
        role: 'menuitemcheckbox',
        'aria-checked': String(open.has(source.key)),
        class: `tray-item${open.has(source.key) ? ' on' : ''}`,
        onclick: () => {
          toggleWindow(source.key);
          trayOpen = false;
          drawTray();
        },
      }, icon(source.icon), h('span', {}, source.title), h('span', { class: 'tray-state' }, open.has(source.key) ? T.winShown : T.winHidden))))
      : null;
    el.winTray.replaceChildren(button, menu || '');
  }

  document.addEventListener('click', (event) => {
    if (trayOpen && !el.winTray.contains(event.target)) {
      trayOpen = false;
      drawTray();
    }
  });

  /* ------------------------------------------------------------- panels */

  const PANEL_VAR = { rail: '--rail', files: '--files', left: '--dock-left', right: '--dock-right' };
  /* A drag in progress, applied to this page only until it is let go. */
  const liveSizes = {};

  function applyPanels() {
    const sizes = { ...layout().sizes, ...liveSizes };
    for (const [name, variable] of Object.entries(PANEL_VAR)) {
      const value = sizes[name] || core.PANEL_DEFAULTS[name];
      root.style.setProperty(variable, `${value}px`);
    }
    body.dataset.filesSide = layout().filesSide;
    body.dataset.railSide = layout().railSide;
  }

  /* A handle on a panel's inner edge. Drag to resize, double-click (or
   * Enter) to reset, arrow keys to nudge. */
  function splitter(panel) {
    const handle = h('div', {
      class: 'splitter',
      role: 'separator',
      tabindex: '0',
      'aria-orientation': 'vertical',
      'aria-label': core.fill(T.panelResize, { p: T[`panel_${panel}`] || panel }),
      'data-panel': panel,
      title: T.panelResizeHint,
    });
    const [min, max] = core.PANEL_SIZES[panel];
    const current = () => liveSizes[panel] || layout().sizes[panel] || core.PANEL_DEFAULTS[panel];
    // Which way a drag to the right moves this panel's edge.
    const direction = () => {
      if (panel === 'left') return 1;
      if (panel === 'right') return -1;
      if (panel === 'rail') return layout().railSide === 'right' ? -1 : 1;
      return layout().filesSide === 'left' ? 1 : -1;
    };
    const commit = (value) => {
      delete liveSizes[panel];
      changeLayout((next) => {
        if (value === null) delete next.sizes[panel];
        else next.sizes[panel] = Math.round(value);
        return next;
      }, true);
      applyPanels();
      shell.scheduleRelayout();
    };
    handle.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      const start = current();
      const x0 = event.clientX;
      handle.setPointerCapture(event.pointerId);
      body.classList.add('dragging', 'dragging-x');
      const onMove = (e) => {
        liveSizes[panel] = Math.min(max, Math.max(min, start + (e.clientX - x0) * direction()));
        applyPanels();
      };
      const onUp = () => {
        handle.removeEventListener('pointermove', onMove);
        handle.removeEventListener('pointerup', onUp);
        handle.removeEventListener('pointercancel', onUp);
        body.classList.remove('dragging', 'dragging-x');
        if (liveSizes[panel] !== undefined && liveSizes[panel] !== start) commit(liveSizes[panel]);
        else delete liveSizes[panel];
      };
      handle.addEventListener('pointermove', onMove);
      handle.addEventListener('pointerup', onUp);
      handle.addEventListener('pointercancel', onUp);
    });
    handle.addEventListener('dblclick', () => commit(null));
    handle.addEventListener('keydown', (event) => {
      const step = event.shiftKey ? 64 : 16;
      if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
        event.preventDefault();
        const sign = (event.key === 'ArrowRight' ? 1 : -1) * direction();
        commit(Math.min(max, Math.max(min, current() + step * sign)));
      } else if (event.key === 'Enter' || event.key === 'Home') {
        event.preventDefault();
        commit(null);
      }
    });
    return handle;
  }

  el.list.append(splitter('rail'));
  el.files.append(splitter('files'));

  /* ----------------------------------------------------------- backdrop */

  let backdropKey = '';
  let backdropDown = null;
  const imageCache = new Map();

  async function drawBackdrop() {
    const bg = settings().background;
    const revision = shell.web().revision;
    const key = `${bg.kind}:${bg.value}:${bg.kind === 'image' ? revision : ''}:${bg.kind === 'mod' && registry.backgrounds[bg.value] ? 'loaded' : ''}`;
    el.backdropDim.style.opacity = String(bg.dim / 100);
    el.backdropArt.style.filter = bg.blur ? `blur(${bg.blur}px)` : '';
    el.backdropArt.style.inset = bg.blur ? `-${bg.blur * 2}px` : '';
    if (bg.kind === 'none') delete body.dataset.backdrop;
    else body.dataset.backdrop = bg.kind;
    root.style.setProperty('--term-alpha', String(bg.kind === 'none' ? 1 : bg.terminal / 100));
    if (key === backdropKey) return;
    backdropKey = key;
    if (backdropDown) backdropDown();
    backdropDown = null;
    const art = el.backdropArt;
    art.replaceChildren();
    art.className = '';
    art.style.backgroundImage = '';
    if (bg.kind === 'preset') {
      art.className = `bg-preset bg-${bg.value}`;
    } else if (bg.kind === 'image') {
      try {
        let url = imageCache.get(key);
        if (!url) {
          const response = await fetch(`/api/web/backgrounds/${encodeURIComponent(bg.value)}`, {
            headers: { Authorization: `Bearer ${shell.token.value || ''}` },
            cache: 'no-store',
          });
          if (!response.ok) throw new Error(`http ${response.status}`);
          url = URL.createObjectURL(await response.blob());
          imageCache.set(key, url);
        }
        if (backdropKey === key) art.style.backgroundImage = `url("${url}")`;
        art.className = 'bg-image';
      } catch {
        art.className = '';
      }
    } else if (bg.kind === 'mod') {
      const source = registry.backgrounds[bg.value];
      if (source) {
        art.className = 'bg-mod';
        const host = h('div', { class: 'bg-mod-host' });
        art.append(host);
        const cleanups = [];
        let alive = true;
        const ctx = {
          active: () => alive,
          onCleanup: (fn) => cleanups.push(fn),
          mode: () => root.dataset.themeMode,
          reducedMotion: () => window.matchMedia('(prefers-reduced-motion: reduce)').matches,
        };
        const returned = source.render(host, ctx);
        if (typeof returned === 'function') cleanups.push(returned);
        backdropDown = () => {
          alive = false;
          for (const fn of cleanups) {
            try {
              fn();
            } catch (error) {
              console.error(error);
            }
          }
        };
      }
    }
  }

  /* ------------------------------------------------------------ redraws */

  let timer = 0;
  function redraw(force) {
    clearTimeout(timer);
    timer = setTimeout(() => {
      applyPanels();
      for (const slot of Object.keys(SIMPLE)) drawSimple(slot, force === true || (force === 'view' && slot.startsWith('view.')));
      for (const slot of Object.keys(DOCKS)) drawDock(slot, force === true || force === 'view');
      drawWindows(force === true);
      drawBackdrop();
      shell.rerenderRows();
    }, 30);
  }

  shell.on('settings', () => redraw(false));
  shell.on('web', () => redraw(false));
  shell.on('widgets', () => redraw(false));
  shell.on('windows', () => redraw(false));
  shell.on('backgrounds', () => redraw(false));
  shell.on('attach', () => redraw('view'));
  shell.on('detach', () => redraw('view'));

  /* The palette: open each window, and reach the studio. */
  shell.layoutCommands = () => {
    const list = [];
    for (const source of windowSources().values()) {
      list.push({
        group: T.paletteGroupWindows,
        title: `${open.has(source.key) ? T.winHide : T.winShow}: ${source.title}`,
        icon: source.icon,
        run: () => toggleWindow(source.key),
      });
    }
    return list;
  };

  shell.layout = {
    request,
    changeLayout,
    openWindow,
    closeWindow,
    toggleWindow,
    redraw: () => redraw(true),
    resetWindows: () => {
      geometry = {};
      saveGeometry();
      redraw(true);
    },
  };

  applyPanels();
  redraw(true);
})();
