// Session age: a chip on each running session row once it is long-lived,
// and a card widget that draws the age as a bar against a working day.
alc.define('session-age', (mod) => {
  const zh = alc.locale === 'zh-TW';
  const HOUR = 3600;
  mod.addSessionChip((card) => {
    if (card.exit || !card.started_at) return null;
    const age = Date.now() / 1000 - card.started_at;
    if (age < HOUR) return null;
    const hours = Math.floor(age / HOUR);
    return { text: `${hours}h+`, tone: hours >= 4 ? 'warn' : 'accent' };
  });

  mod.addWidget({
    id: 'bar',
    title: zh ? '執行時間條' : 'Age bar',
    description: zh ? '每張卡片上的一條細線，顯示 session 跑了多久。' : 'A thin bar on each card for how long the session has run.',
    icon: 'clock',
    slots: ['card'],
    options: [
      { key: 'hours', type: 'number', label: zh ? '滿格時數' : 'Hours for a full bar', default: 8, min: 1, max: 48 },
    ],
    render(root, ctx) {
      const card = ctx.card;
      if (!card || card.exit || !card.started_at) return;
      const full = Math.max(1, Number(ctx.options.hours) || 8) * HOUR;
      const age = Date.now() / 1000 - card.started_at;
      const share = Math.min(1, age / full);
      const tone = share > 0.75 ? ' danger' : share > 0.5 ? ' warn' : '';
      root.append(alc.h('span', { class: `w-meter age-bar${tone}`, title: `${Math.floor(age / HOUR)}h / ${Math.round(full / HOUR)}h` },
        alc.h('span', { style: { width: `${Math.round(share * 100)}%` } })));
    },
  });
});
