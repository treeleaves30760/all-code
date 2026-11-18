// Command deck: each agent's own slash commands, as buttons. The list
// follows the session on screen, since codex and claude speak differently.
alc.define('command-deck', (mod) => {
  const zh = alc.locale === 'zh-TW';
  const DECKS = {
    claude: ['/compact', '/clear', '/review', '/cost', '/model', '/memory', '/permissions'],
    codex: ['/status', '/diff', '/compact', '/review', '/model', '/approvals', '/new'],
    opencode: ['/compact', '/share', '/models', '/sessions', '/undo', '/redo'],
    gemini: ['/compress', '/clear', '/stats', '/memory show', '/tools'],
    goose: ['/plan', '/summarize', '/clear', '/mode'],
    pi: ['/compact', '/model', '/session', '/new'],
  };
  const FALLBACK = ['/help', '/compact', '/clear'];

  mod.addWidget({
    id: 'deck',
    title: zh ? '指令列' : 'Command deck',
    description: zh ? '目前代理的 slash 指令按鈕。' : "The current agent's slash commands as buttons.",
    icon: 'terminal',
    defaultSlot: 'view.bottom',
    slots: ['view.top', 'view.bottom', 'view.left', 'view.right', 'float'],
    options: [
      { key: 'extra', type: 'text', label: zh ? '額外指令（以逗號分隔）' : 'Extra commands (comma separated)', default: '' },
      { key: 'enter', type: 'boolean', label: zh ? '直接送出' : 'Send at once', default: true },
    ],
    render(root, ctx) {
      const card = ctx.session();
      if (!card) {
        root.append(alc.h('span', { class: 'muted' }, zh ? '開啟 session 以顯示指令。' : 'Open a session to see its commands.'));
        return;
      }
      const extra = String(ctx.options.extra || '').split(',').map((cmd) => cmd.trim()).filter(Boolean);
      const list = [...new Set([...(DECKS[card.agent] || FALLBACK), ...extra])];
      root.append(alc.h('div', { class: 'deck' },
        alc.h('span', { class: 'deck-agent' }, card.agent),
        list.map((cmd) => alc.h('button', {
          type: 'button',
          class: 'btn small deck-cmd',
          disabled: !alc.canType || !!card.exit,
          onclick: () => {
            if (alc.type(cmd) && ctx.options.enter) alc.key('\r');
          },
        }, cmd))));
    },
  });
});
