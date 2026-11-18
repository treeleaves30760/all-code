/* The studio: the page's own mods creator.
 *
 * Five tabs over the same web/ folder the settings page edits:
 *
 * - Packages: apply, export, import and save sets of mods with their
 *   layout, theme and background.
 * - Layout: put each widget the loaded mods offer in a slot of the session
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
    ['packages', () => T.studioPackages, 'layers'],
    ['layout', () => T.studioLayout, 'grid'],
    ['mods', () => T.studioMods, 'puzzle'],
    ['background', () => T.studioBackground, 'image'],
    ['panels', () => T.studioPanels, 'files'],
  ];

  let tab = (() => {
    try {
      return window.localStorage.getItem('alc.studio.tab') || 'packages';
    } catch {
      return 'packages';
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

  function pill(text, tone) {
    return h('span', { class: `pill ${tone || ''}` }, text);
  }

  /* ----------------------------------------------------------- packages */

  /* What the page is now, as a package. */
  function currentAsPackage(name, description) {
    const settings = web().settings;
    const mods = web().mods.filter((mod) => mod.enabled && !mod.error).map((mod) => mod.id);
    return {
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
  }

  function exportText(info) {
    const { id, error, ...rest } = info;
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

  function apply(id) {
    return act(() => request('POST', `/api/web/packages/${encodeURIComponent(id)}/apply`, {}), (applied) => {
      if (applied.missing && applied.missing.length) toast(core.fill(T.packageMissing, { m: applied.missing.join(', ') }), 'warn');
      else if (applied.review && applied.review.length) toast(core.fill(T.packageReview, { m: applied.review.join(', ') }), 'warn');
      else if (applied.reload) reloadToast(T.packageReload);
      else toast(T.packageApplied);
    });
  }

  function packagesTab() {
    const { packages, catalog, settings } = web();
    const active = settings.package;
    const installed = packages.map((info) => card(
      h('div', { class: 'studio-row' },
        h('span', { class: 'studio-glyph' }, icon('layers')),
        h('div', { class: 'studio-main' },
          h('div', { class: 'studio-title' }, info.name || info.id,
            info.version ? h('span', { class: 'muted' }, ` v${info.version}`) : null,
            active === info.id ? pill(T.packageActive, 'ok') : null,
            info.error ? pill(T.modError, 'danger') : null),
          info.description ? h('p', { class: 'studio-desc' }, info.description) : null,
          h('div', { class: 'chips' }, (info.mods || []).map((id) => h('span', { class: 'chip' }, id)),
            info.background && info.background.kind !== 'none' ? h('span', { class: 'chip accent' }, `${T.studioBackground}: ${info.background.value}`) : null,
            info.theme ? h('span', { class: 'chip' }, `${T.theme}: ${info.theme}`) : null),
          info.error ? h('p', { class: 'mod-error' }, info.error) : null)),
      h('div', { class: 'studio-actions' },
        btn(active === info.id ? T.packageReapply : T.packageApply, () => apply(info.id), { accent: active !== info.id, key: `apply-${info.id}`, disabled: !!info.error }),
        h('button', { type: 'button', class: 'btn small', onclick: () => download(`${info.id}.json`, exportText(info)) }, T.packageExport),
        h('button', { type: 'button', class: 'btn small', onclick: () => shell.copy(exportText(info)) }, T.packageCopy),
        btn(T.remove, () => {
          if (window.confirm(core.fill(T.packageRemoveConfirm, { p: info.name || info.id }))) {
            act(() => request('DELETE', `/api/web/packages/${encodeURIComponent(info.id)}`));
          }
        }, { danger: true }))));

    const official = catalog.packages.map((entry) => card(
      h('div', { class: 'studio-row' },
        h('span', { class: 'studio-glyph accent' }, icon('star')),
        h('div', { class: 'studio-main' },
          h('div', { class: 'studio-title' }, entry.name, pill(T.official, 'accent'), entry.installed ? pill(T.installed, 'ok') : null),
          h('p', { class: 'studio-desc' }, entry.description))),
      h('div', { class: 'studio-actions' },
        entry.installed
          ? btn(T.packageApply, () => apply(entry.id), { accent: true })
          : btn(T.installApply, () => act(async () => {
            await request('POST', '/api/web/catalog', { kind: 'package', id: entry.id });
            return request('POST', `/api/web/packages/${encodeURIComponent(entry.id)}/apply`, {});
          }, (applied) => {
            if (applied && applied.reload) reloadToast(T.packageReload);
            else toast(T.packageApplied);
          }), { accent: true, key: `install-${entry.id}` }),
        entry.installed ? btn(T.reinstall, () => act(() => request('POST', '/api/web/catalog', { kind: 'package', id: entry.id, force: true }))) : null)));

    // Save the page as it is now.
    const idInput = h('input', { class: 'text-input', placeholder: 'my-setup', 'aria-label': T.packageId, spellcheck: 'false', disabled: viewer() });
    const nameInput = h('input', { class: 'text-input', placeholder: T.packageNameHint, 'aria-label': T.packageName, disabled: viewer() });
    const descInput = h('input', { class: 'text-input wide', placeholder: T.packageDescHint, 'aria-label': T.packageDesc, disabled: viewer() });
    const save = card(
      head(T.packageSave, T.packageSaveHelp),
      h('div', { class: 'studio-form' }, idInput, nameInput, descInput,
        btn(T.packageSaveButton, () => {
          const id = idInput.value.trim().toLowerCase();
          if (!/^[a-z0-9][a-z0-9_-]{0,47}$/.test(id)) return toast(T.badId, 'warn');
          const body = currentAsPackage(nameInput.value.trim() || id, descInput.value.trim());
          return act(() => request('POST', `/api/web/packages/${encodeURIComponent(id)}`, body), () => toast(core.fill(T.packageSaved, { p: id })));
        }, { accent: true, icon: 'plus' })));

    const importId = h('input', { class: 'text-input', placeholder: 'imported', 'aria-label': T.packageId, spellcheck: 'false', disabled: viewer() });
    const importText = h('textarea', { class: 'text-input code-input', rows: '4', placeholder: '{ "name": "…", "mods": [ … ], "layout": { … } }', 'aria-label': T.packageImport, spellcheck: 'false', disabled: viewer() });
    const file = h('input', {
      type: 'file',
      accept: 'application/json,.json',
      hidden: true,
      onchange: async (event) => {
        const picked = event.target.files && event.target.files[0];
        if (!picked) return;
        importText.value = await picked.text();
        if (!importId.value) importId.value = picked.name.replace(/\.json$/i, '').toLowerCase().replace(/[^a-z0-9_-]/g, '-');
      },
    });
    const importer = card(
      head(T.packageImport, T.packageImportHelp),
      h('div', { class: 'studio-form' }, importId,
        h('button', { type: 'button', class: 'btn small', disabled: viewer(), onclick: () => file.click() }, T.chooseFile), file,
        importText,
        btn(T.packageImportButton, () => {
          const id = importId.value.trim().toLowerCase();
          if (!/^[a-z0-9][a-z0-9_-]{0,47}$/.test(id)) return toast(T.badId, 'warn');
          let parsed;
          try {
            parsed = JSON.parse(importText.value);
          } catch {
            return toast(T.badJson, 'warn');
          }
          return act(() => request('POST', `/api/web/packages/${encodeURIComponent(id)}`, parsed), () => toast(core.fill(T.packageSaved, { p: id })));
        }, { accent: true })));

    return [
      h('p', { class: 'studio-lede' }, T.packagesLede),
      h('h3', { class: 'studio-sub' }, T.packagesYours),
      installed.length ? h('div', { class: 'studio-grid' }, installed) : h('p', { class: 'note' }, T.packagesNone),
      h('h3', { class: 'studio-sub' }, T.packagesOfficial),
      h('div', { class: 'studio-grid' }, official),
      h('div', { class: 'studio-grid two' }, save, importer),
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
      head(T.library, T.libraryHelp),
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
    const explicit = web().settings.layout.explicit;
    return [
      h('p', { class: 'studio-lede' }, explicit ? T.layoutLedeExplicit : T.layoutLede),
      h('div', { class: 'studio-layout' },
        h('div', { class: 'studio-side' }, library, inspector()),
        h('div', { class: 'studio-screens' }, screens, h('div', { class: 'studio-actions end' }, reset))),
    ];
  }

  /* --------------------------------------------------------------- mods */

  function modsTab() {
    const { mods, catalog } = web();
    const installed = mods.map((mod) => {
      const official = catalog.mods.some((entry) => entry.id === mod.id);
      return h('li', { class: `mod-row ${mod.error ? 'error' : mod.enabled ? 'on' : 'off'}` },
        h('span', { class: 'mod-icon' }, icon('puzzle')),
        h('div', { class: 'mod-main' },
          h('div', { class: 'mod-head' },
            h('span', { class: 'mod-name' }, mod.name),
            mod.version ? h('span', { class: 'mod-version' }, `v${mod.version}`) : null,
            official ? pill(T.official, 'accent') : null),
          mod.description ? h('p', { class: 'mod-desc' }, mod.description) : null,
          h('code', { class: 'mod-id' }, `mods/${mod.id}`, mod.hasJs ? ' · mod.js' : '', mod.hasCss ? ' · mod.css' : ''),
          mod.error ? h('p', { class: 'mod-error' }, mod.error) : null),
        h('div', { class: 'studio-actions' },
          h('label', { class: 'switch' },
            h('input', {
              type: 'checkbox',
              role: 'switch',
              checked: mod.enabled,
              'aria-label': mod.name,
              'data-key': `studio-mod-${mod.id}`,
              disabled: viewer() || !!mod.error,
              onchange: async (event) => {
                const on = event.target.checked;
                const ok = await shell.changeSettings((n) => {
                  n.disabledMods = on ? n.disabledMods.filter((id) => id !== mod.id) : [...n.disabledMods, mod.id];
                }, true);
                if (ok) reloadToast();
              },
            }),
            h('span', { class: 'switch-track', 'aria-hidden': 'true' })),
          btn(T.remove, () => {
            if (window.confirm(core.fill(T.modRemoveConfirm, { m: mod.name }))) {
              act(() => request('DELETE', `/api/web/mods/${encodeURIComponent(mod.id)}`), () => reloadToast());
            }
          }, { danger: true })));
    });

    const official = catalog.mods.map((entry) => h('li', { class: 'catalog-item' },
      h('div', { class: 'mod-main' },
        h('div', { class: 'mod-head' }, h('span', { class: 'mod-name' }, entry.name), entry.installed ? pill(T.installed, 'ok') : null),
        h('p', { class: 'mod-desc' }, entry.description)),
      entry.installed
        ? btn(T.reinstall, () => act(() => request('POST', '/api/web/catalog', { kind: 'mod', id: entry.id, force: true }), () => reloadToast()))
        : btn(T.install, () => act(() => request('POST', '/api/web/catalog', { kind: 'mod', id: entry.id }), () => reloadToast()), { accent: true, key: `install-mod-${entry.id}` })));

    const source = h('input', { class: 'text-input wide', placeholder: 'owner/repo, owner/repo@v1, https://github.com/owner/repo', 'aria-label': T.installSource, spellcheck: 'false', disabled: viewer() });
    const github = card(
      head(T.installGithub, T.installGithubHelp),
      h('div', { class: 'studio-form' }, source,
        btn(T.install, () => {
          if (!source.value.trim()) return undefined;
          return act(() => request('POST', '/api/web/install', { source: source.value.trim() }), (answer) => {
            const what = [...(answer.mods || []), ...(answer.packages || []), ...(answer.themes || [])].join(', ');
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
      head(T.newMod, T.newModHelp),
      h('div', { class: 'studio-form' }, newId, newName, template,
        btn(T.newModCreate, () => {
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
      created,
      h('p', { class: 'field-help' }, T.newModCli));

    return [
      h('p', { class: 'studio-lede' }, T.modsLede),
      h('div', { class: 'studio-grid two' },
        card(head(T.modsInstalled, null), installed.length ? h('ul', { class: 'mod-list' }, installed) : h('p', { class: 'note' }, T.modsNone)),
        card(head(T.modsOfficial, T.modsOfficialHelp), h('ul', { class: 'catalog' }, official))),
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
      h('p', { class: 'studio-lede' }, T.bgLede),
      card(head(T.bgPresets, null),
        h('div', { class: 'bg-grid' }, swatch('none', '', T.bgNone, 'bg-thumb-none'),
          presetList.map((name) => swatch('preset', name, T[`bg_${name}`] || name, `bg-preset bg-${name}`)))),
      card(head(T.bgImages, T.bgImagesHelp, h('button', { type: 'button', class: 'btn small accent', disabled: viewer(), onclick: () => upload.click() }, icon('plus'), T.bgUpload), upload),
        images.length ? h('div', { class: 'bg-grid' }, images) : h('p', { class: 'note' }, T.bgImagesNone)),
      card(head(T.bgMods, T.bgModsHelp),
        modBackgrounds.length ? h('div', { class: 'bg-grid' }, modBackgrounds) : h('p', { class: 'note' }, T.bgModsNone)),
      card(head(T.bgTuning, T.bgTuningHelp),
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
      h('p', { class: 'studio-lede' }, T.panelsLede),
      h('div', { class: 'studio-grid two' },
        card(head(T.panelWidths, T.panelWidthsHelp),
          h('div', { class: 'studio-fields' }, ['rail', 'files', 'left', 'right'].map(sizeSlider))),
        card(head(T.panelSides, T.panelSidesHelp),
          h('div', { class: 'studio-fields' },
            sidePicker(T.panel_rail, 'railSide', layout.railSide),
            sidePicker(T.panel_files, 'filesSide', layout.filesSide),
            h('label', { class: 'studio-field' },
              h('span', { class: 'field-label' }, T.fitPlainLabel),
              h('label', { class: 'switch' },
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
                h('span', { class: 'switch-text' }, T.fitPlain)),
              h('span', { class: 'field-help' }, T.fitPlainHelp)),
            h('div', { class: 'studio-field' },
              h('span', { class: 'field-label' }, T.windowsReset),
              h('button', { type: 'button', class: 'btn small', onclick: () => shell.layout.resetWindows() }, T.windowsResetButton),
              h('span', { class: 'field-help' }, T.windowsResetHelp))))),
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
    const parts = { packages: packagesTab, layout: layoutTab, mods: modsTab, background: backgroundTab, panels: panelsTab }[tab] || packagesTab;
    studio.replaceChildren(
      h('div', { class: 'page-head' },
        h('div', {}, h('h1', { class: 'page-title' }, T.navStudio), h('p', { class: 'page-sub' }, T.studioSub))),
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
