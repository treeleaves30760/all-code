// Session notes: one text per session name, saved in this mod's state on
// the hub. Keyed by name rather than id, so a session restarted under the
// same name finds its note again.
alc.define('session-notes', (mod) => {
  const zh = alc.locale === 'zh-TW';
  const MAX = 4000;
  mod.addWidget({
    id: 'note',
    title: zh ? 'Session 筆記' : 'Session note',
    description: zh ? '每個 session 一份筆記。' : 'One note for each session.',
    icon: 'note',
    defaultSlot: 'view.right',
    slots: ['view.left', 'view.right', 'view.top', 'view.bottom', 'float'],
    render(root, ctx) {
      const card = ctx.session();
      if (!card) {
        root.append(alc.h('p', { class: 'muted' }, zh ? '開啟 session 來寫筆記。' : 'Open a session to keep a note for it.'));
        return;
      }
      const notes = mod.state.notes || {};
      const area = alc.h('textarea', {
        class: 'text-input sn-text',
        rows: '6',
        maxlength: String(MAX),
        placeholder: zh ? `${card.name} 的筆記…` : `Notes for ${card.name}…`,
        disabled: !alc.canType,
      });
      area.value = notes[card.name] || '';
      const status = alc.h('span', { class: 'muted sn-status' }, '');
      let timer = 0;
      area.addEventListener('input', () => {
        status.textContent = zh ? '儲存中…' : 'Saving…';
        clearTimeout(timer);
        timer = setTimeout(async () => {
          const next = { ...(mod.state.notes || {}) };
          if (area.value.trim()) next[card.name] = area.value.slice(0, MAX);
          else delete next[card.name];
          // Only the 50 most recently written, so the settings file stays small.
          const names = Object.keys(next);
          if (names.length > 50) delete next[names[0]];
          try {
            await mod.save({ notes: next });
            status.textContent = zh ? '已儲存' : 'Saved';
          } catch {
            status.textContent = zh ? '無法儲存' : 'Could not save';
          }
        }, 700);
      });
      ctx.onCleanup(() => clearTimeout(timer));
      root.append(area, status);
    },
  });
});
