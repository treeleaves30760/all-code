// Clock: a widget that is happiest in the header. Options choose seconds,
// 12/24-hour, and a second zone for the teammate on the other side of the
// world.
alc.define('clock', (mod) => {
  const zh = alc.locale === 'zh-TW';
  mod.addWidget({
    id: 'clock',
    title: zh ? '時鐘' : 'Clock',
    description: zh ? '目前時間，可加上第二個時區。' : 'The time, with an optional second zone.',
    icon: 'clock',
    defaultSlot: 'bar',
    slots: ['bar', 'list.top', 'view.top', 'view.left', 'view.right', 'float'],
    size: { width: 260, height: 140 },
    options: [
      { key: 'seconds', type: 'boolean', label: zh ? '顯示秒數' : 'Show seconds', default: false },
      {
        key: 'hour12',
        type: 'select',
        label: zh ? '格式' : 'Format',
        default: '24',
        choices: [{ value: '24', label: '24h' }, { value: '12', label: '12h' }],
      },
      { key: 'zone', type: 'text', label: zh ? '第二時區（例如 Europe/London）' : 'Second zone (e.g. Europe/London)', default: '', placeholder: 'America/New_York' },
    ],
    render(root, ctx) {
      const big = ctx.slot !== 'bar';
      const format = (zone) => {
        try {
          return new Intl.DateTimeFormat(alc.locale, {
            hour: '2-digit',
            minute: '2-digit',
            second: ctx.options.seconds ? '2-digit' : undefined,
            hour12: ctx.options.hour12 === '12',
            timeZone: zone || undefined,
          }).format(new Date());
        } catch {
          return '—';
        }
      };
      const date = () => new Intl.DateTimeFormat(alc.locale, { weekday: 'short', month: 'short', day: 'numeric' }).format(new Date());
      const main = alc.h('span', { class: 'clock-main' });
      const second = alc.h('span', { class: 'clock-zone' });
      const day = alc.h('span', { class: 'clock-day' });
      root.append(alc.h('div', { class: `clock${big ? ' big' : ''}` }, main, big ? day : null, second));
      const zone = String(ctx.options.zone || '').trim();
      const tick = () => {
        main.textContent = format();
        day.textContent = date();
        second.textContent = zone ? `${zone.split('/').pop().replace(/_/g, ' ')} ${format(zone)}` : '';
      };
      tick();
      ctx.every(ctx.options.seconds ? 1000 : 10000, tick);
    },
  });
});
