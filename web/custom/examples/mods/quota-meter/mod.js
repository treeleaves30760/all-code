// Quota meter: reads /api/usage (the same report as `alc usage`) once a
// minute while it is on screen.
alc.define('quota-meter', (mod) => {
  const zh = alc.locale === 'zh-TW';
  const L = {
    loading: zh ? '讀取中…' : 'Loading…',
    none: zh ? '沒有可顯示的配額。' : 'No quota windows to show.',
    failed: zh ? '無法讀取用量。' : 'Could not read usage.',
    resets: zh ? '重置' : 'resets',
  };

  function until(epoch) {
    const seconds = Math.max(0, epoch - Date.now() / 1000);
    const d = Math.floor(seconds / 86400);
    const h = Math.floor((seconds % 86400) / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
  }

  function windows(report) {
    const out = [];
    for (const account of (report && report.accounts) || []) {
      for (const win of account.windows || []) {
        if (typeof win.used_percent !== 'number') continue;
        out.push({
          label: `${account.profile} · ${win.name}${win.scope ? ` ${win.scope}` : ''}`,
          used: Math.max(0, Math.min(100, win.used_percent)),
          resets: win.resets_at,
        });
      }
    }
    return out;
  }

  mod.addWidget({
    id: 'meter',
    title: zh ? '配額' : 'Quota',
    description: zh ? '每個登入的 5 小時與每週配額。' : "Each login's 5-hour and weekly windows.",
    icon: 'usage',
    defaultSlot: 'view.right',
    slots: ['bar', 'list.top', 'list.bottom', 'view.left', 'view.right', 'view.top', 'float'],
    size: { width: 320, height: 260 },
    options: [
      { key: 'warn', type: 'number', label: zh ? '警示門檻（%）' : 'Warn at (%)', default: 80, min: 10, max: 100 },
    ],
    render(root, ctx) {
      root.append(alc.h('p', { class: 'muted' }, L.loading));
      const warn = Number(ctx.options.warn) || 80;
      const draw = async () => {
        let report;
        try {
          report = await alc.api('/api/usage');
        } catch {
          root.replaceChildren(alc.h('p', { class: 'muted' }, L.failed));
          return;
        }
        if (!ctx.active()) return;
        const list = windows(report);
        if (ctx.slot === 'bar') {
          const worst = list.sort((a, b) => b.used - a.used)[0];
          root.replaceChildren(worst
            ? alc.h('span', { class: `qm-inline${worst.used >= warn ? ' hot' : ''}`, title: list.map((w) => `${w.label}: ${Math.round(w.used)}%`).join('\n') }, `${worst.label} ${Math.round(worst.used)}%`)
            : '');
          return;
        }
        if (!list.length) {
          root.replaceChildren(alc.h('p', { class: 'muted' }, L.none));
          return;
        }
        root.replaceChildren(alc.h('ul', { class: 'w-list qm-list' }, list.map((win) => {
          const tone = win.used >= 95 ? ' danger' : win.used >= warn ? ' warn' : '';
          return alc.h('li', {},
            alc.h('div', { class: 'w-row' }, alc.h('span', {}, win.label), alc.h('strong', {}, `${Math.round(win.used)}%`)),
            alc.h('div', { class: `w-meter${tone}` }, alc.h('span', { style: { width: `${win.used}%` } })),
            win.resets ? alc.h('div', { class: 'qm-reset' }, `${L.resets} ${until(win.resets)}`) : null);
        })));
      };
      draw();
      ctx.every(60000, draw);
    },
  });
});
