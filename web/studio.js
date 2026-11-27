/* The studio: the page's own mods creator.
 *
 * Five tabs over the same web/ folder the settings page edits:
 *
 * - Layouts: apply, export, import and save whole looks - mods, widget
 *   places, theme, background and terminal font - or go back to Default.
 * - Widgets: put each widget the loaded mods offer in a slot of the session
 *   list or the coding view, or in a floating window, and set its options.
 * - Mods: switch, install (official, or from GitHub), remove, and start a
 *   new one from a template.
 * - Background: a preset, an uploaded image, or one a mod draws.
 * - Panels: how wide the session list, file panel and docks are, and which
 *   side the list and the file panel sit on.
 *
 * Everything here is a write to the hub, so a viewer link sees it all and
 * changes nothing.
 */

(() => {
  'use strict';

  const shell = window.AlcShell;
  if (!shell || !shell.layout) return;
  const { core, T, el, h, icon, toast } = shell;
  const studio = el.studio;
  const { request, changeLayout } = shell.layout;

  const TABS = [
    ['layouts', () => T.studioLayouts, 'layers'],
    ['widgets', () => T.studioWidgets, 'grid'],
    ['mods', () => T.studioMods, 'puzzle'],
    ['background', () => T.studioBackground, 'image'],
    ['panels', () => T.studioPanels, 'files'],
  ];

  // 2.8 called the first two tabs `packages` and `layout`.
  const RENAMED = { packages: 'layouts', layout: 'widgets' };
  let tab = (() => {
    try {
      const saved = window.localStorage.getItem('alc.studio.tab') || 'layouts';
      return RENAMED[saved] || saved;
    } catch {
      return 'layouts';
    }
  })();
  /* The widget whose options are open in the layout tab. */
  let selected = null;
  let busy = false;

  const web = () => shell.web();
  const viewer = () => !shell.operator.value;

  function setTab(next) {
    tab = next;
    try {
      window.localStorage.setItem('alc.studio.tab', next);
    } catch {
      // Remembering the tab is a convenience.
    }
    render();
  }

  /* Runs a write, then re-reads the folder so the studio shows what is on
   * disk. Errors are the hub's own sentence. */
  async function act(work, done) {
    if (busy) return;
    busy = true;
    studio.classList.add('busy');
    try {
      const answer = await work();
      await shell.pull();
      if (done) done(answer);
    } catch (error) {
      toast(error.message || String(error), 'error');
    } finally {
      busy = false;
      studio.classList.remove('busy');
      render();
    }
  }

  function reloadToast(text) {
    toast(text || T.modsChanged, 'info', { label: T.reload, run: () => window.location.reload() });
  }

  function card(...children) {
    return h('article', { class: 'panel studio-card' }, ...children);
  }

  function head(title, help, ...actions) {
    return h('div', { class: 'studio-head' },
      h('div', {}, h('h2', { class: 'panel-title' }, title), help ? h('p', { class: 'studio-help' }, help) : null),
      actions.length ? h('div', { class: 'studio-actions' }, ...actions) : null);
  }

  function btn(label, onclick, extra) {
    return h('button', { type: 'button', class: `btn small${extra && extra.accent ? ' accent' : ''}${extra && extra.danger ? ' danger' : ''}`, disabled: viewer() || (extra && extra.disabled), onclick, 'data-key': extra && extra.key }, extra && extra.icon ? icon(extra.icon) : null, label);
  }

  function btnTip(label, tip, onclick, extra) {
    const node = btn(label, onclick, extra);
    if (tip) node.title = tip;
    return node;
  }


  /* ------------------------------------------------------------ layouts */

  const SVG = 'http://www.w3.org/2000/svg';
  function svg(tag, attrs, ...children) {
    const node = document.createElementNS(SVG, tag);
    for (const [key, value] of Object.entries(attrs || {})) node.setAttribute(key, String(value));
    for (const child of children.flat()) if (child) node.append(child);
    return node;
  }

  /* A picture of the page a layout makes, from core.layoutThumb: its
   * colours, which side the rail is on, which docks hold widgets. */
  function thumbnail(info) {
    const t = core.layoutThumb(info, web().themes, window.matchMedia('(prefers-color-scheme: dark)').matches);
    const W = 160;
    const H = 100;
    const rail = 44;
    const railX = t.railSide === 'right' ? W - rail : 0;
    const viewX = t.railSide === 'right' ? 0 : rail;
    const viewW = W - rail;
    let top = 12;
    let bottom = H;
    const parts = [svg('rect', { width: W, height: H, fill: t.bg })];
    if (t.background !== 'none') {
      parts.push(svg('circle', { cx: W * 0.78, cy: H * 0.2, r: 46, fill: t.accent, opacity: 0.22 }));
      parts.push(svg('circle', { cx: W * 0.18, cy: H * 0.95, r: 38, fill: t.accent, opacity: 0.14 }));
    }
    parts.push(svg('rect', { width: W, height: 12, fill: t.surface }));
    parts.push(svg('line', { x1: 0, y1: 12, x2: W, y2: 12, stroke: t.line }));
    parts.push(svg('rect', { x: 5, y: 4, width: 4, height: 4, rx: 1, fill: t.accent }));
    parts.push(svg('rect', { x: 13, y: 5, width: 18, height: 2, rx: 1, fill: t.muted }));
    if (t.docks.bar) parts.push(svg('rect', { x: W - 34, y: 3.5, width: 22, height: 5, rx: 2.5, fill: t.raised, stroke: t.line }));
    parts.push(svg('rect', { x: railX, y: 12, width: rail, height: H - 12, fill: t.surface, opacity: t.background !== 'none' ? 0.9 : 1 }));
    parts.push(svg('line', { x1: t.railSide === 'right' ? railX : rail, y1: 12, x2: t.railSide === 'right' ? railX : rail, y2: H, stroke: t.line }));
    for (let row = 0; row < 4; row += 1) {
      const y = 18 + row * (t.dense ? 12 : 15);
      if (row === 1) parts.push(svg('rect', { x: railX + 3, y: y - 3, width: rail - 6, height: 11, rx: 2, fill: t.accent, opacity: 0.16 }));
      parts.push(svg('rect', { x: railX + 6, y, width: rail - 18, height: 2.4, rx: 1, fill: t.text, opacity: 0.8 }));
      parts.push(svg('rect', { x: railX + 6, y: y + 4, width: rail - 24, height: 1.8, rx: 1, fill: t.muted, opacity: 0.7 }));
    }
    if (t.docks.top) {
      parts.push(svg('rect', { x: viewX + 4, y: 15, width: viewW - 8, height: 6, rx: 1.5, fill: t.raised, stroke: t.line }));
      top = 23;
    }
    if (t.docks.bottom) {
      parts.push(svg('rect', { x: viewX + 4, y: H - 10, width: viewW - 8, height: 6, rx: 1.5, fill: t.raised, stroke: t.line }));
      bottom = H - 12;
    }
    let termX = viewX + 4;
    let termW = viewW - 8;
    for (const side of ['left', 'right']) {
      if (!t.docks[side]) continue;
      const x = side === 'left' ? termX : termX + termW - 22;
      parts.push(svg('rect', { x, y: top + 3, width: 22, height: bottom - top - 7, rx: 2, fill: t.surface, stroke: t.line }));
      parts.push(svg('rect', { x: x + 3, y: top + 7, width: 12, height: 2, rx: 1, fill: t.muted }));
      if (side === 'left') termX += 25;
      termW -= 25;
    }
    parts.push(svg('rect', { x: termX, y: top + 3, width: termW, height: bottom - top - 7, rx: 2, fill: t.terminal }));
    const lines = [[0.55, '#7ee2a8'], [0.8, '#d7dae0'], [0.4, '#d7dae0'], [0.65, t.accent]];
    lines.forEach(([share, colour], index) => {
      parts.push(svg('rect', { x: termX + 4, y: top + 8 + index * 6, width: Math.max(8, (termW - 8) * share), height: 2, rx: 1, fill: colour, opacity: 0.85 }));
    });
    if (t.docks.float) {
      const fx = viewX + viewW - 46;
      parts.push(svg('rect', { x: fx, y: 50, width: 40, height: 30, rx: 3, fill: t.surface, stroke: t.line }));
      parts.push(svg('rect', { x: fx, y: 50, width: 40, height: 6, rx: 3, fill: t.accent, opacity: 0.85 }));
    }
    return svg('svg', { class: 'layout-thumb', viewBox: `0 0 ${W} ${H}`, 'aria-hidden': 'true', preserveAspectRatio: 'xMinYMin slice' }, parts);
  }

  /* What the page is now, as a layout. */
  function currentAsLayout(name, description) {
    const settings = web().settings;
    const mods = web().mods.filter((mod) => mod.enabled && !mod.error).map((mod) => mod.id);
    const out = {
      name,
      description,
      version: '1.0.0',
      author: '',
      mods,
      theme: settings.theme,
      accent: settings.accent,
      density: settings.density,
      background: settings.background,
      layout: core.materializeLayout(settings.layout, widgetsMap()),
    };
    if (settings.layoutTerminal) out.terminal = settings.layoutTerminal;
    return out;
  }

  function exportText(info) {
    const { id, error, legacy, installed, official, active, ...rest } = info;
    return JSON.stringify(rest, null, 2);
  }

  function download(name, text) {
    const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
    const link = h('a', { href: url, download: name });
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 2000);
  }

  /* An overflow menu: a <details>, so it opens by tap, click or keyboard. */
  function overflow(label, items) {
    const list = items.filter(Boolean);
    if (!list.length) return null;
    const menu = h('details', { class: 'menu' },
      h('summary', { class: 'icon menu-toggle', title: label, 'aria-label': label }, icon('more')),
      h('div', { class: 'menu-list', role: 'menu' }, list.map((item) => h('button', {
        type: 'button',
        role: 'menuitem',
        class: `menu-item${item.danger ? ' danger' : ''}`,
        disabled: item.disabled,
        onclick: () => {
          menu.open = false;
          item.run();
        },
      }, item.label))));
    return menu;
  }
  document.addEventListener('click', (event) => {
    for (const open of document.querySelectorAll('details.menu[open]')) {
      if (!open.contains(event.target)) open.open = false;
    }
  });

  async function apply(entry) {
    if (busy) return;
    busy = true;
    studio.classList.add('busy');
    try {
      await shell.applyLayout(entry);
    } finally {
      busy = false;
      studio.classList.remove('busy');
      render();
    }
  }

  function layoutCard(entry) {
    const count = (entry.mods || []).length;
    const meta = [
      entry.official ? h('span', { class: 'badge' }, T.official) : null,
      entry.legacy ? h('span', { class: 'muted', title: T.layoutLegacy }, 'packages/') : null,
      entry.id !== '' ? h('span', { class: 'layout-mods' }, core.fill(count === 1 ? T.layoutModsOne : T.layoutModsMany, { n: count })) : null,
    ];
    const action = entry.active
      ? h('span', { class: 'pill ok layout-active' }, icon('check'), T.layoutActive)
      : btn(T.layoutApply, () => (entry.default ? (async () => {
        busy = true;
        try {
          await shell.resetLayout();
        } finally {
          busy = false;
          render();
        }
      })() : apply(entry)), { accent: true, key: `apply-${entry.id || 'default'}`, disabled: !!entry.error });
    const menu = entry.default ? null : overflow(T.more, [
      entry.installed ? { label: T.layoutDownload, run: () => download(`${entry.id}.json`, exportText(entry)) } : null,
      entry.installed ? { label: T.layoutCopy, run: () => shell.copy(exportText(entry)) } : null,
      entry.installed && entry.official ? { label: T.reinstall, disabled: viewer(), run: () => act(() => request('POST', '/api/web/catalog', { kind: 'layout', id: entry.id, force: true })) } : null,
      entry.installed ? {
        label: T.remove,
        danger: true,
        disabled: viewer(),
        run: async () => {
          const ok = await shell.dialog({
            title: core.fill(T.layoutRemoveTitle, { l: entry.name || entry.id }),
            body: h('p', {}, entry.active ? T.layoutRemoveActive : T.layoutRemoveBody),
            actions: [{ label: T.remove, danger: true }],
          });
          if (ok) act(() => request('DELETE', `/api/web/layouts/${encodeURIComponent(entry.id)}`));
        },
      } : null,
    ]);
    return h('article', { class: `layout-card${entry.active ? ' active' : ''}${entry.error ? ' broken' : ''}`, 'data-layout-id': entry.id || 'default' },
      h('div', { class: 'layout-preview' }, thumbnail(entry)),
      h('div', { class: 'layout-body' },
        h('div', { class: 'layout-name' }, h('span', { class: 'layout-title', title: entry.name || entry.id }, entry.name || entry.id), ...meta),
        entry.error
          ? h('p', { class: 'mod-error', title: entry.error }, entry.error)
          : h('p', { class: 'layout-desc', title: entry.description || '' }, entry.description || '—')),
      h('div', { class: 'layout-actions' }, action, menu));
  }

  async function newLayout() {
    const mode = { value: 'save' };
    const idInput = h('input', { class: 'text-input', name: 'id', placeholder: 'my-layout', 'aria-label': T.layoutId, spellcheck: 'false', autocapitalize: 'off' });
    const nameInput = h('input', { class: 'text-input', name: 'name', placeholder: T.layoutName, 'aria-label': T.layoutName });
    const descInput = h('input', { class: 'text-input', name: 'desc', placeholder: T.layoutDesc, 'aria-label': T.layoutDesc });
    const jsonInput = h('textarea', { class: 'text-input code-input', rows: '6', placeholder: '{ "name": "…", "mods": [ … ] }', 'aria-label': T.layoutJson, spellcheck: 'false' });
    const sourceInput = h('input', { class: 'text-input', placeholder: 'owner/repo', 'aria-label': T.installSource, spellcheck: 'false', autocapitalize: 'off', title: T.installSourceTip });
    const file = h('input', {
      type: 'file',
      accept: 'application/json,.json',
      hidden: true,
      onchange: async (event) => {
        const picked = event.target.files && event.target.files[0];
        if (!picked) return;
        jsonInput.value = await picked.text();
        if (!idInput.value) idInput.value = picked.name.replace(/\.json$/i, '').toLowerCase().replace(/[^a-z0-9_-]/g, '-');
      },
    });
    const panes = {
      save: h('div', { class: 'dialog-fields' }, idInput, nameInput, descInput),
      import: h('div', { class: 'dialog-fields' }, jsonInput, h('button', { type: 'button', class: 'btn small', onclick: () => file.click() }, T.chooseFile), file),
      github: h('div', { class: 'dialog-fields' }, sourceInput, h('p', { class: 'muted small' }, T.installWarning)),
    };
    const holder = h('div', {});
    const tabs = h('div', { class: 'segmented', role: 'radiogroup', 'aria-label': T.layoutNew });
    const choices = [['save', T.layoutFromPage], ['import', T.layoutFromJson], ['github', T.layoutFromGithub]];
    const draw = () => {
      tabs.replaceChildren(...choices.map(([value, label]) => h('button', {
        type: 'button',
        role: 'radio',
        'aria-checked': String(mode.value === value),
        onclick: () => {
          mode.value = value;
          draw();
        },
      }, label)));
      holder.replaceChildren(mode.value === 'github' ? panes.github : h('div', { class: 'dialog-fields' }, idInput, mode.value === 'save' ? [nameInput, descInput] : panes.import));
    };
    draw();
    const answer = await shell.dialog({
      title: T.layoutNew,
      wide: true,
      body: [tabs, holder],
      actions: [{
        label: T.layoutCreate,
        check: () => {
          if (mode.value === 'github') return !!sourceInput.value.trim();
          const id = idInput.value.trim().toLowerCase();
          if (!/^[a-z0-9][a-z0-9_-]{0,47}$/.test(id)) {
            toast(T.badId, 'warn');
            idInput.focus();
            return false;
          }
          if (mode.value === 'import') {
            try {
              JSON.parse(jsonInput.value);
            } catch {
              toast(T.badJson, 'warn');
              return false;
            }
          }
          return true;
        },
        value: () => ({ mode: mode.value, id: idInput.value.trim().toLowerCase(), name: nameInput.value.trim(), desc: descInput.value.trim(), json: jsonInput.value, source: sourceInput.value.trim() }),
      }],
    });
    if (!answer) return;
    if (answer.mode === 'github') {
      act(() => request('POST', '/api/web/install', { source: answer.source }), (done) => {
        const what = [...(done.layouts || []), ...(done.mods || []), ...(done.themes || [])].join(', ');
        toast(core.fill(T.installedFrom, { w: what || '—' }));
      });
      return;
    }
    const body = answer.mode === 'save' ? currentAsLayout(answer.name || answer.id, answer.desc) : JSON.parse(answer.json);
    act(() => request('POST', `/api/web/layouts/${encodeURIComponent(answer.id)}`, body), () => toast(core.fill(T.layoutSaved, { l: answer.id })));
  }

  function layoutsTab() {
    const { layouts, catalog, settings } = web();
    const cards = core.layoutGallery(layouts, catalog.layouts, settings.activeLayout);
    const home = {
      id: '',
      default: true,
      name: T.layoutDefault,
      description: T.layoutDefaultDesc,
      theme: settings.layoutRestore && settings.layoutRestore.theme ? settings.layoutRestore.theme : settings.activeLayout ? 'system' : settings.theme,
      mods: [],
      active: !settings.activeLayout,
    };
    return [
      h('div', { class: 'studio-toolbar' },
        h('span', { class: 'muted' }, core.fill(T.layoutCount, { n: cards.length })),
        btn(T.layoutNew, newLayout, { icon: 'plus', key: 'layout-new' })),
      h('div', { class: 'layout-grid' }, layoutCard(home), cards.map(layoutCard)),
    ];
  }

  /* ------------------------------------------------------------- layout */

  const SCREENS = [
    {
      id: 'list',
      title: () => T.screenList,
      rows: [['bar'], ['list.top'], ['card'], ['list.bottom']],
    },
    {
      id: 'view',
      title: () => T.screenView,
      rows: [['bar'], ['view.top'], ['view.left', 'terminal', 'view.right'], ['view.bottom']],
    },
    {
      id: 'float',
      title: () => T.screenFloat,
      rows: [['float']],
    },
  ];

  function slotName(slot) {
    return T[`slot_${slot.replace('.', '_')}`] || slot;
  }

  function widgetsMap() {
    const out = {};
    for (const [key, widget] of Object.entries(shell.registry.widgets)) {
      out[key] = { slots: widget.slots, defaultSlot: widget.defaultSlot, options: widget.options };
    }
    return out;
  }

  function move(key, slot, index) {
    selected = slot ? key : selected === key ? null : selected;
    changeLayout((current) => core.placeWidget(current, key, slot, index), true);
  }

  function placedSlot(key) {
    const layout = web().settings.layout;
    for (const slot of core.SLOTS) {
      if (core.placementsFor(layout, slot, widgetsMap()).some((entry) => entry.widget === key)) return slot;
    }
    return null;
  }

  function dropZone(slot) {
    const layout = web().settings.layout;
    const placed = core.placementsFor(layout, slot, widgetsMap());
    const zone = h('div', { class: `slot-zone zone-${slot.replace('.', '-')}`, 'data-slot': slot },
      h('span', { class: 'zone-label' }, slotName(slot)),
      h('div', { class: 'zone-items' }, placed.map((entry, index) => {
        const widget = shell.registry.widgets[entry.widget];
        return h('button', {
          type: 'button',
          class: `zone-chip${selected === entry.widget ? ' on' : ''}`,
          draggable: viewer() ? 'false' : 'true',
          'data-widget': entry.widget,
          'data-index': String(index),
          title: widget.description || widget.title,
          onclick: () => {
            selected = selected === entry.widget ? null : entry.widget;
            render();
          },
          ondragstart: (event) => {
            event.dataTransfer.setData('text/x-alc-widget', entry.widget);
            event.dataTransfer.effectAllowed = 'move';
          },
        }, icon(widget.icon), h('span', {}, widget.title));
      })),
      placed.length ? null : h('span', { class: 'zone-empty' }, T.zoneEmpty));
    if (!viewer()) {
      zone.addEventListener('dragover', (event) => {
        if (![...event.dataTransfer.types].includes('text/x-alc-widget')) return;
        event.preventDefault();
        zone.classList.add('over');
      });
      zone.addEventListener('dragleave', () => zone.classList.remove('over'));
      zone.addEventListener('drop', (event) => {
        event.preventDefault();
        zone.classList.remove('over');
        const key = event.dataTransfer.getData('text/x-alc-widget');
        const widget = shell.registry.widgets[key];
        if (!widget) return;
        if (!widget.slots.includes(slot)) {
          toast(core.fill(T.slotNotAllowed, { w: widget.title, s: slotName(slot) }), 'warn');
          return;
        }
        const chip = event.target.closest('.zone-chip');
        const index = chip ? Number(chip.dataset.index) : undefined;
        move(key, slot, index);
      });
    }
    return zone;
  }

  function optionControl(widget, slot, option, value) {
    const set = (next) => changeLayout((current) => {
      const list = current.slots[slot] || [];
      const found = list.find((entry) => entry.widget === widget.key);
      if (found) {
        found.options = { ...found.options, [option.key]: next };
        return current;
      }
      // A widget drawn by default is not in the layout yet; placing it
      // there is what lets it keep an option.
      return core.placeWidget(current, widget.key, slot, undefined, { [option.key]: next });
    }, true);
    const common = { 'aria-label': option.label || option.key, disabled: viewer(), 'data-key': `opt-${option.key}` };
    switch (option.type) {
      case 'boolean':
        return h('label', { class: 'switch' },
          h('input', { ...common, type: 'checkbox', role: 'switch', checked: !!value, onchange: (event) => set(event.target.checked) }),
          h('span', { class: 'switch-track', 'aria-hidden': 'true' }),
          h('span', { class: 'switch-text' }, option.label || option.key));
      case 'select':
        return h('select', { ...common, onchange: (event) => set(event.target.value) },
          (option.choices || []).map((choice) => {
            const entry = typeof choice === 'object' ? choice : { value: choice, label: String(choice) };
            return h('option', { value: entry.value, selected: entry.value === value }, entry.label || entry.value);
          }));
      case 'number':
        return h('input', { ...common, type: 'number', class: 'text-input', min: option.min, max: option.max, step: option.step || 1, value: value ?? '', onchange: (event) => set(Number(event.target.value)) });
      case 'color':
        return h('input', { ...common, type: 'color', value: value || '#4f46e5', onchange: (event) => set(event.target.value) });
      case 'textarea':
        return h('textarea', { ...common, class: 'text-input', rows: '4', onchange: (event) => set(event.target.value) }, value || '');
      default:
        return h('input', { ...common, type: 'text', class: 'text-input', value: value ?? '', placeholder: option.placeholder || '', onchange: (event) => set(event.target.value) });
    }
  }

  function inspector() {
    const widget = selected && shell.registry.widgets[selected];
    if (!widget) {
      return card(head(T.inspector, T.inspectorEmpty));
    }
    const slot = placedSlot(widget.key);
    const layout = web().settings.layout;
    const placement = slot ? core.placementsFor(layout, slot, widgetsMap()).find((entry) => entry.widget === widget.key) : null;
    const options = placement ? placement.options : {};
    const where = h('select', {
      'aria-label': T.inspectorSlot,
      disabled: viewer(),
      'data-key': 'inspector-slot',
      onchange: (event) => move(widget.key, event.target.value || null),
    }, h('option', { value: '', selected: !slot }, T.slotHidden),
    widget.slots.map((name) => h('option', { value: name, selected: slot === name }, slotName(name))));
    const index = slot ? core.placementsFor(layout, slot, widgetsMap()).findIndex((entry) => entry.widget === widget.key) : -1;
    return card(
      head(widget.title, widget.description),
      h('div', { class: 'studio-fields' },
        h('label', { class: 'studio-field' }, h('span', { class: 'field-label' }, T.inspectorSlot), where),
        slot ? h('div', { class: 'studio-actions' },
          btn('↑', () => move(widget.key, slot, Math.max(0, index - 1)), { disabled: index <= 0 }),
          btn('↓', () => move(widget.key, slot, index + 2), {}),
          btn(T.slotHide, () => move(widget.key, null), { danger: true })) : null,
        widget.options.length
          ? widget.options.map((option) => h('label', { class: 'studio-field' },
            option.type === 'boolean' ? null : h('span', { class: 'field-label' }, option.label || option.key),
            slot ? optionControl(widget, slot, option, options[option.key]) : h('span', { class: 'muted' }, T.placeFirst),
            option.help ? h('span', { class: 'field-help' }, option.help) : null))
          : h('p', { class: 'muted' }, T.noOptions),
        h('code', { class: 'mod-id' }, `mods/${widget.mod} · ${widget.key}`)));
  }

  function layoutTab() {
    const widgets = Object.values(shell.registry.widgets);
    const library = card(
      head(T.library, null),
      widgets.length
        ? h('ul', { class: 'library' }, widgets.map((widget) => {
          const slot = placedSlot(widget.key);
          return h('li', {},
            h('button', {
              type: 'button',
              class: `library-item${selected === widget.key ? ' on' : ''}`,
              draggable: viewer() ? 'false' : 'true',
              ondragstart: (event) => {
                event.dataTransfer.setData('text/x-alc-widget', widget.key);
                event.dataTransfer.effectAllowed = 'move';
              },
              onclick: () => {
                selected = widget.key;
                render();
              },
            }, icon(widget.icon),
            h('span', { class: 'library-text' }, h('span', { class: 'library-name' }, widget.title), h('span', { class: 'library-where' }, slot ? slotName(slot) : T.slotHidden))));
        }))
        : h('p', { class: 'note' }, T.libraryEmpty));
    const screens = SCREENS.map((screen) => h('figure', { class: `screen screen-${screen.id}` },
      h('figcaption', {}, screen.title()),
      h('div', { class: 'screen-frame' }, screen.rows.map((row) => h('div', { class: 'screen-row' }, row.map((slot) => (slot === 'terminal'
        ? h('div', { class: 'slot-zone zone-terminal', 'aria-hidden': 'true' }, h('span', { class: 'zone-label' }, T.terminal))
        : dropZone(slot))))))));
    const reset = btn(T.layoutReset, () => {
      if (!window.confirm(T.layoutResetConfirm)) return;
      changeLayout((current) => ({ ...current, slots: {}, hidden: [], explicit: false }), true);
    }, { danger: true });
    return [
      h('div', { class: 'studio-layout' },
        h('div', { class: 'studio-side' }, library, inspector()),
        h('div', { class: 'studio-screens' }, screens, h('div', { class: 'studio-actions end' }, reset))),
    ];
  }

  /* --------------------------------------------------------------- mods */

  function modsTab() {
    const { mods, catalog } = web();
    /* One row per mod: installed ones first with their switch, then the
     * official ones not installed yet with Install. Reinstall and Remove
     * wait in the row's ⋯ menu. */
    const row = (mod, installed) => {
      // Installed: official only if alc wrote it (SOURCE), whatever its id.
      const official = installed ? mod.official === true : true;
      const state = !installed ? 'off' : mod.error ? 'error' : mod.enabled ? 'on' : 'off';
      const control = !installed
        ? btn(T.install, () => act(() => request('POST', '/api/web/catalog', { kind: 'mod', id: mod.id }), () => reloadToast()), { accent: true, key: `install-mod-${mod.id}` })
        : h('label', { class: 'switch', title: mod.enabled ? T.modOn : T.modOff },
          h('input', {
            type: 'checkbox',
            role: 'switch',
            checked: mod.enabled,
            'aria-label': mod.name,
            'data-key': `studio-mod-${mod.id}`,
            disabled: viewer() || !!mod.error,
            onchange: async (event) => {
              const on = event.target.checked;
              if (on && !(await shell.confirmModOn(mod))) {
                event.target.checked = false;
                return;
              }
              const ok = await shell.setModEnabled(mod.id, on);
              if (!ok) event.target.checked = !on;
              if (ok) reloadToast();
            },
          }),
          h('span', { class: 'switch-track', 'aria-hidden': 'true' }));
      const menu = installed ? overflow(T.more, [
        official ? { label: T.reinstall, disabled: viewer(), run: () => act(() => request('POST', '/api/web/catalog', { kind: 'mod', id: mod.id, force: true }), () => reloadToast()) } : null,
        {
          label: T.remove,
          danger: true,
          disabled: viewer(),
          run: async () => {
            const ok = await shell.dialog({
              title: core.fill(T.modRemoveTitle, { m: mod.name }),
              body: h('p', {}, T.modRemoveBody),
              actions: [{ label: T.remove, danger: true }],
            });
            if (ok) act(() => request('DELETE', `/api/web/mods/${encodeURIComponent(mod.id)}`), () => reloadToast());
          },
        },
      ]) : null;
      const files = installed ? [mod.hasJs ? 'mod.js' : '', mod.hasCss ? 'mod.css' : ''].filter(Boolean).join(' · ') : '';
      return h('li', { class: `mod-line ${state}`, 'data-mod': mod.id },
        h('div', { class: 'mod-line-main' },
          h('div', { class: 'mod-line-head' },
            h('span', { class: 'mod-name', title: mod.name }, mod.name),
            official ? h('span', { class: 'badge' }, T.official) : null,
            installed && mod.source && !official ? h('span', { class: 'mod-source', title: mod.source }, core.shortSource(mod.source)) : null),
          mod.error
            ? h('p', { class: 'mod-error', title: mod.error }, mod.error)
            : h('p', { class: 'mod-line-desc', title: [mod.description, files ? `mods/${mod.id} · ${files}` : ''].filter(Boolean).join('\n') }, mod.description || '—')),
        h('div', { class: 'mod-line-actions' }, control, menu));
    };
    const installedIds = new Set(mods.map((mod) => mod.id));
    const rows = [
      ...mods.map((mod) => row(mod, true)),
      ...catalog.mods.filter((entry) => !installedIds.has(entry.id)).map((entry) => row(entry, false)),
    ];

    const source = h('input', { class: 'text-input wide', placeholder: 'owner/repo, owner/repo@v1, https://github.com/owner/repo', 'aria-label': T.installSource, spellcheck: 'false', disabled: viewer() });
    const github = card(
      head(T.installGithub, null),
      h('div', { class: 'studio-form' }, source,
        btn(T.install, () => {
          if (!source.value.trim()) return undefined;
          return act(() => request('POST', '/api/web/install', { source: source.value.trim() }), (answer) => {
            const what = [...(answer.layouts || []), ...(answer.mods || []), ...(answer.themes || [])].join(', ');
            toast(core.fill(T.installedFrom, { w: what || '—' }));
          });
        }, { accent: true })),
      h('p', { class: 'field-help' }, T.installWarning));

    const newId = h('input', { class: 'text-input', placeholder: 'my-widget', 'aria-label': T.newModId, spellcheck: 'false', disabled: viewer() });
    const newName = h('input', { class: 'text-input', placeholder: T.newModNameHint, 'aria-label': T.newModName, disabled: viewer() });
    const template = h('select', { 'aria-label': T.newModTemplate, disabled: viewer() },
      catalog.templates.map((entry) => h('option', { value: entry.id }, `${entry.name} — ${entry.description}`)));
    const created = h('div', { class: 'created' });
    const maker = card(
      head(T.newMod, null),
      h('div', { class: 'studio-form' }, newId, newName, template,
        btnTip(T.newModCreate, T.newModCli, () => {
          const id = newId.value.trim().toLowerCase();
          if (!/^[a-z0-9][a-z0-9_-]{0,47}$/.test(id)) return toast(T.badId, 'warn');
          return act(() => request('POST', `/api/web/mods/${encodeURIComponent(id)}/create`, { template: template.value, name: newName.value.trim() }), (answer) => {
            const prompt = core.fill(T.newModPrompt, { d: answer.dir });
            created.replaceChildren(
              h('p', { class: 'note ok' }, core.fill(T.newModCreated, { d: answer.dir })),
              h('button', { type: 'button', class: 'command prompt', onclick: () => shell.copy(prompt) }, icon('copy'), h('span', { class: 'command-text' }, prompt)));
            reloadToast(T.newModReload);
          });
        }, { accent: true, icon: 'plus' })),
      created);

    return [
      card(rows.length ? h('ul', { class: 'mod-lines' }, rows) : h('p', { class: 'note' }, T.modsNone)),
      h('div', { class: 'studio-grid two' }, maker, github),
    ];
  }

  /* --------------------------------------------------------- background */

  function setBackground(mutate, quiet) {
    return shell.changeSettings((n) => {
      mutate(n.background);
    }, !quiet, !!quiet);
  }

  function slider(label, key, min, max, unit) {
    const value = web().settings.background[key];
    return h('label', { class: 'studio-field' },
      h('span', { class: 'field-label' }, label),
      h('div', { class: 'inline-control' },
        h('input', {
          type: 'range',
          min,
          max,
          value,
          disabled: viewer(),
          'data-key': `bg-${key}`,
          oninput: (event) => {
            event.target.nextElementSibling.textContent = `${event.target.value}${unit}`;
            setBackground((bg) => {
              bg[key] = Number(event.target.value);
            }, true);
          },
        }),
        h('output', { class: 'range-value' }, `${value}${unit}`)));
  }

  function swatch(kind, value, label, preview) {
    const bg = web().settings.background;
    const on = bg.kind === kind && (kind === 'none' || bg.value === value);
    return h('button', {
      type: 'button',
      class: `bg-swatch${on ? ' on' : ''}`,
      'aria-pressed': String(on),
      disabled: viewer(),
      'data-key': `bg-${kind}-${value}`,
      onclick: () => setBackground((next) => {
        next.kind = kind;
        next.value = value;
      }),
    }, h('span', { class: `bg-thumb ${preview || ''}` }), h('span', { class: 'bg-name' }, label));
  }

  const thumbs = new Map();
  async function imageThumb(name, node) {
    try {
      let url = thumbs.get(name);
      if (!url) {
        const response = await fetch(`/api/web/backgrounds/${encodeURIComponent(name)}`, { headers: { Authorization: `Bearer ${shell.token.value || ''}` } });
        if (!response.ok) return;
        url = URL.createObjectURL(await response.blob());
        thumbs.set(name, url);
      }
      node.style.backgroundImage = `url("${url}")`;
    } catch {
      // The name says what it is.
    }
  }

  function backgroundTab() {
    const { backgrounds, presets } = web();
    const presetList = (presets && presets.length ? presets : core.BACKGROUND_PRESETS);
    const images = backgrounds.map((image) => {
      const node = swatch('image', image.name, image.name);
      imageThumb(image.name, node.querySelector('.bg-thumb'));
      return h('div', { class: 'bg-image-item' }, node,
        btn('×', () => {
          if (window.confirm(core.fill(T.bgRemoveConfirm, { n: image.name }))) {
            act(() => request('DELETE', `/api/web/backgrounds/${encodeURIComponent(image.name)}`));
          }
        }, { danger: true }));
    });
    const modBackgrounds = Object.values(shell.registry.backgrounds).map((entry) => swatch('mod', entry.key, entry.name, 'bg-thumb-mod'));
    const upload = h('input', {
      type: 'file',
      accept: 'image/png,image/jpeg,image/webp,image/gif',
      hidden: true,
      onchange: async (event) => {
        const picked = event.target.files && event.target.files[0];
        if (!picked) return;
        if (picked.size > 1024 * 1024) {
          toast(T.bgTooBig, 'warn');
          return;
        }
        const ext = (picked.name.split('.').pop() || 'png').toLowerCase().replace('jpeg', 'jpg');
        const stem = picked.name.replace(/\.[^.]+$/, '').toLowerCase().replace(/[^a-z0-9_-]+/g, '-').replace(/^[^a-z0-9]+/, '').slice(0, 40) || 'background';
        const name = `${stem}.${ext}`;
        const bytes = await picked.arrayBuffer();
        act(() => request('POST', `/api/web/backgrounds/${encodeURIComponent(name)}`, bytes, true), () => setBackground((bg) => {
          bg.kind = 'image';
          bg.value = name;
        }));
      },
    });
    return [
      card(head(T.bgPresets, null),
        h('div', { class: 'bg-grid' }, swatch('none', '', T.bgNone, 'bg-thumb-none'),
          presetList.map((name) => swatch('preset', name, T[`bg_${name}`] || name, `bg-preset bg-${name}`)))),
      card(head(T.bgImages, null, h('button', { type: 'button', class: 'btn small accent', title: T.bgImagesHelp, disabled: viewer(), onclick: () => upload.click() }, icon('plus'), T.bgUpload), upload),
        images.length ? h('div', { class: 'bg-grid' }, images) : h('p', { class: 'note' }, T.bgImagesNone)),
      card(head(T.bgMods, null),
        modBackgrounds.length ? h('div', { class: 'bg-grid' }, modBackgrounds) : h('p', { class: 'note' }, T.bgModsNone)),
      card(head(T.bgTuning, null),
        h('div', { class: 'studio-fields' },
          slider(T.bgDim, 'dim', 0, 95, '%'),
          slider(T.bgBlur, 'blur', 0, 40, 'px'),
          slider(T.bgTerminal, 'terminal', 30, 100, '%'))),
    ];
  }

  /* ------------------------------------------------------------- panels */

  function sizeSlider(panel) {
    const [min, max] = core.PANEL_SIZES[panel];
    const sizes = web().settings.layout.sizes;
    const value = sizes[panel] || core.PANEL_DEFAULTS[panel];
    return h('label', { class: 'studio-field' },
      h('span', { class: 'field-label' }, T[`panel_${panel}`] || panel),
      h('div', { class: 'inline-control' },
        h('input', {
          type: 'range',
          min,
          max,
          step: 10,
          value,
          disabled: viewer(),
          'data-key': `size-${panel}`,
          oninput: (event) => {
            event.target.nextElementSibling.textContent = `${event.target.value}px`;
            shell.changeSettings((n) => {
              n.layout.sizes = { ...n.layout.sizes, [panel]: Number(event.target.value) };
            }, false, true);
          },
          onchange: () => shell.scheduleRelayout(),
        }),
        h('output', { class: 'range-value' }, `${value}px`),
        btn(T.reset, () => changeLayout((current) => {
          delete current.sizes[panel];
          return current;
        }, true), { disabled: !sizes[panel] })));
  }

  function sidePicker(label, key, current) {
    return h('div', { class: 'studio-field' },
      h('span', { class: 'field-label' }, label),
      h('div', { class: 'segmented', role: 'radiogroup', 'aria-label': label },
        ['left', 'right'].map((side) => h('button', {
          type: 'button',
          role: 'radio',
          'aria-checked': String(current === side),
          'data-key': `${key}-${side}`,
          disabled: viewer(),
          onclick: () => changeLayout((next) => ({ ...next, [key]: side }), true),
        }, side === 'left' ? T.sideLeft : T.sideRight))));
  }

  function panelsTab() {
    const layout = web().settings.layout;
    return [
      h('div', { class: 'studio-grid two' },
        card(head(T.panelWidths, null),
          h('div', { class: 'studio-fields' }, ['rail', 'files', 'left', 'right'].map(sizeSlider))),
        card(head(T.panelSides, null),
          h('div', { class: 'studio-fields' },
            sidePicker(T.panel_rail, 'railSide', layout.railSide),
            sidePicker(T.panel_files, 'filesSide', layout.filesSide),
            h('label', { class: 'studio-field' },
              h('span', { class: 'field-label' }, T.fitPlainLabel),
              h('label', { class: 'switch', title: T.fitPlainHelp },
                h('input', {
                  type: 'checkbox',
                  role: 'switch',
                  checked: web().settings.terminal.fitPlain,
                  disabled: viewer(),
                  'data-key': 'studio-fit',
                  onchange: (event) => shell.changeSettings((n) => {
                    n.terminal.fitPlain = event.target.checked;
                  }, true),
                }),
                h('span', { class: 'switch-track', 'aria-hidden': 'true' }),
                h('span', { class: 'switch-text' }, T.fitPlain))),
            h('div', { class: 'studio-field' },
              h('span', { class: 'field-label' }, T.windowsReset),
              h('button', { type: 'button', class: 'btn small', title: T.windowsResetHelp, onclick: () => shell.layout.resetWindows() }, T.windowsResetButton))))),
    ];
  }

  /* ------------------------------------------------------------- render */

  function render() {
    if (document.body.dataset.pane !== 'studio') return;
    const focusKey = document.activeElement && document.activeElement.dataset ? document.activeElement.dataset.key : null;
    const scroll = studio.scrollTop;
    const tabs = h('div', { class: 'studio-tabs', role: 'tablist' }, TABS.map(([id, label, iconName]) => h('button', {
      type: 'button',
      role: 'tab',
      class: 'studio-tab',
      'aria-selected': String(tab === id),
      'data-key': `tab-${id}`,
      onclick: () => setTab(id),
    }, icon(iconName), label())));
    const banner = viewer() ? h('p', { class: 'note settings-error' }, T.studioViewer) : null;
    const parts = { layouts: layoutsTab, widgets: layoutTab, mods: modsTab, background: backgroundTab, panels: panelsTab }[tab] || layoutsTab;
    studio.replaceChildren(
      h('div', { class: 'page-head' },
        h('div', {}, h('h1', { class: 'page-title' }, T.navStudio))),
      tabs,
      banner || '',
      h('div', { class: 'studio-body', role: 'tabpanel' }, parts()));
    studio.scrollTop = scroll;
    if (focusKey) {
      const again = studio.querySelector(`[data-key="${CSS.escape(focusKey)}"]`);
      if (again) again.focus({ preventScroll: true });
    }
  }

  let timer = 0;
  let quiet = false;
  function later() {
    clearTimeout(timer);
    timer = setTimeout(render, 80);
  }

  // A slider mid-drag must not be redrawn under the pointer; it changes
  // settings quietly, and the redraw waits for the next real change.
  studio.addEventListener('pointerdown', (event) => {
    if (event.target.matches('input[type="range"]')) quiet = true;
  });
  document.addEventListener('pointerup', () => {
    if (quiet) {
      quiet = false;
      later();
    }
  });
  for (const event of ['settings', 'web', 'widgets', 'backgrounds']) {
    shell.on(event, () => {
      if (!quiet) later();
    });
  }
  shell.on('page', (page) => {
    if (page === 'studio') render();
  });
})();
