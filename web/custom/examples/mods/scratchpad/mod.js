// Scratchpad: notes whose text lives in settings.json under
// modSettings.scratchpad, so every device that opens the page sees it.
// One text, two places: a tab of its own and a widget for a dock or window.
alc.define('scratchpad', (mod) => {
  const zh = alc.locale === 'zh-TW';

  function editor(root, ctx, big) {
    const area = alc.h('textarea', {
      class: `scratchpad-text${big ? '' : ' small'}`,
      placeholder: zh ? '這台機器的筆記…' : 'Notes for this machine…',
      spellcheck: 'false',
      disabled: !alc.canType,
    });
    area.value = mod.state.text || '';
    const status = alc.h('span', { class: 'scratchpad-status' }, alc.canType ? (zh ? '已儲存' : 'Saved') : (zh ? '唯讀' : 'Read only'));
    const send = alc.h('button', {
      class: 'btn small',
      type: 'button',
      disabled: !alc.canType,
      onclick: () => {
        const text = area.value.trim();
        if (text && alc.current()) alc.type(text);
        else alc.toast(zh ? '請先開啟 session 再送出筆記' : 'Open a session to send notes to it', 'warn');
      },
    }, zh ? '送到 session' : 'Send to session');

    let timer = 0;
    area.addEventListener('input', () => {
      status.textContent = zh ? '儲存中…' : 'Saving…';
      clearTimeout(timer);
      timer = setTimeout(async () => {
        if (!ctx.active()) return;
        try {
          await mod.save({ ...mod.state, text: area.value });
          status.textContent = zh ? '已儲存' : 'Saved';
        } catch {
          status.textContent = zh ? '無法儲存' : 'Could not save';
        }
      }, 600);
    });
    // Another device saved: show it, unless this one is being typed in.
    alc.on('settings', (settings) => {
      if (!ctx.active() || document.activeElement === area) return;
      const text = (settings.modSettings.scratchpad || {}).text || '';
      if (text !== area.value) area.value = text;
    });
    return { area, status, send };
  }

  mod.addPage({
    id: 'notes',
    title: zh ? '筆記' : 'Notes',
    icon: 'note',
    render(root, ctx) {
      const { area, status, send } = editor(root, ctx, true);
      root.append(
        alc.h('div', { class: 'panel scratchpad' },
          alc.h('div', { class: 'scratchpad-head' },
            alc.h('h2', { class: 'pane-title' }, zh ? '筆記' : 'Notes'), status, send),
          area),
      );
      area.focus();
    },
  });

  mod.addWidget({
    id: 'notes',
    title: zh ? '筆記' : 'Notes',
    description: zh ? '與「筆記」分頁同一份文字。' : 'The same text as the Notes tab.',
    icon: 'note',
    slots: ['list.top', 'list.bottom', 'view.left', 'view.right', 'float'],
    size: { width: 340, height: 320 },
    render(root, ctx) {
      const { area, status, send } = editor(root, ctx, false);
      root.append(alc.h('div', { class: 'scratchpad-widget' }, area, alc.h('div', { class: 'scratchpad-foot' }, status, send)));
    },
  });
});
