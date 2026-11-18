// Session overview: counts from the five-second session poll.
alc.define('session-overview', (mod) => {
  const zh = alc.locale === 'zh-TW';
  const L = {
    running: zh ? '執行中' : 'Running',
    agents: zh ? '代理' : 'Agents',
    watching: zh ? '觀看者' : 'Watching',
    longest: zh ? '最久' : 'Longest',
    none: zh ? '無' : 'none',
  };

  function age(seconds) {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    return h ? `${h}h ${m}m` : `${m}m`;
  }

  mod.addWidget({
    id: 'tiles',
    title: zh ? 'Session 總覽' : 'Session overview',
    description: zh ? '執行中、代理、觀看者與最久的 session。' : 'Running, agents, viewers and the longest session.',
    icon: 'chart',
    defaultSlot: 'list.top',
    slots: ['list.top', 'list.bottom', 'view.top', 'view.left', 'view.right', 'float', 'bar'],
    options: [
      { key: 'byAgent', type: 'boolean', label: zh ? '列出每個代理' : 'List each agent', default: true },
    ],
    render(root, ctx) {
      const draw = (cards) => {
        const running = cards.filter((card) => !card.exit);
        const agents = new Map();
        for (const card of running) agents.set(card.agent, (agents.get(card.agent) || 0) + 1);
        const viewers = running.reduce((sum, card) => sum + (card.viewers || 0), 0);
        const now = Date.now() / 1000;
        const oldest = running.slice().sort((a, b) => a.started_at - b.started_at)[0];
        if (ctx.slot === 'bar') {
          root.replaceChildren(alc.h('span', { class: 'so-inline' }, `${running.length} ${L.running.toLowerCase()} · ${viewers} ${L.watching.toLowerCase()}`));
          return;
        }
        const tile = (value, label, extra) => alc.h('div', { class: 'so-tile' },
          alc.h('span', { class: 'w-stat-value' }, value),
          alc.h('span', { class: 'w-stat-label' }, label),
          extra || null);
        root.replaceChildren(alc.h('div', { class: `so-tiles${ctx.slot.startsWith('view.') || ctx.slot === 'float' ? ' narrow' : ''}` },
          tile(String(running.length), L.running),
          tile(String(agents.size), L.agents, ctx.options.byAgent && agents.size
            ? alc.h('span', { class: 'so-agents' }, [...agents].map(([name, count]) => alc.h('span', { class: 'chip' }, `${name} ${count}`)))
            : null),
          tile(String(viewers), L.watching),
          tile(oldest ? age(now - oldest.started_at) : '—', oldest ? `${L.longest} · ${oldest.name}` : L.longest)));
      };
      draw(alc.sessions());
      ctx.on('sessions', draw);
    },
  });
});
