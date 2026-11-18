// Scratchpad: a notes page whose text lives in settings.json under
// modSettings.scratchpad, so every device that opens the page sees it.
alc.define('scratchpad', (mod) => {
  mod.addPage({
    id: 'notes',
    title: 'Notes',
    icon: 'note',
    render(root, ctx) {
      const area = alc.h('textarea', {
        class: 'scratchpad-text',
        placeholder: 'Notes for this machine…',
        spellcheck: 'false',
        disabled: !alc.canType,
      });
      area.value = mod.state.text || '';
      const status = alc.h('span', { class: 'scratchpad-status' }, alc.canType ? 'Saved' : 'Read only');
      const send = alc.h('button', {
        class: 'btn',
        type: 'button',
        disabled: !alc.canType,
        onclick: () => {
          const text = area.value.trim();
          if (text && alc.current()) alc.type(text);
          else alc.toast('Open a session to send notes to it', 'warn');
        },
      }, 'Send to session');

      let timer = 0;
      area.addEventListener('input', () => {
        status.textContent = 'Saving…';
        clearTimeout(timer);
        timer = setTimeout(async () => {
          if (!ctx.active()) return;
          try {
            await mod.save({ ...mod.state, text: area.value });
            status.textContent = 'Saved';
          } catch {
            status.textContent = 'Could not save';
          }
        }, 600);
      });

      root.append(
        alc.h('div', { class: 'panel scratchpad' },
          alc.h('div', { class: 'scratchpad-head' },
            alc.h('h2', { class: 'pane-title' }, 'Notes'), status, send),
          area),
      );
      area.focus();
    },
  });
});
