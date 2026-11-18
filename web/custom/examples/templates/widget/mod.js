// __MOD_NAME__: a widget. Place it from the studio (Studio -> Layout), or
// let `defaultSlot` decide until someone moves it.
//
// Slots: bar, list.top, list.bottom, card, view.top, view.bottom,
// view.left, view.right, float. See AGENTS.md next to mods/ for the API.
alc.define('__MOD_ID__', (mod) => {
  mod.addWidget({
    id: 'main',
    title: '__MOD_NAME__',
    description: 'What this widget shows.',
    icon: 'spark',
    defaultSlot: 'view.right',
    slots: ['list.top', 'view.left', 'view.right', 'view.bottom', 'float'],
    // Options appear in the studio's inspector and arrive as ctx.options.
    options: [
      { key: 'greeting', type: 'text', label: 'Greeting', default: 'Hello' },
      { key: 'showCount', type: 'boolean', label: 'Show the session count', default: true },
    ],
    render(root, ctx) {
      // root: an empty element in the slot. ctx: slot, options, session(),
      // card (card slot only), on(event, fn), every(ms, fn), onCleanup(fn),
      // active(), setOptions({...}). Everything registered through ctx is
      // undone when the widget is moved, re-rendered or removed.
      const line = alc.h('p', { class: '__MOD_ID__-line' });
      const draw = () => {
        const running = alc.sessions().filter((card) => !card.exit).length;
        const card = ctx.session();
        line.textContent = [
          ctx.options.greeting,
          card ? `you are looking at ${card.name}` : null,
          ctx.options.showCount ? `${running} running` : null,
        ].filter(Boolean).join(' · ');
      };
      root.append(line);
      draw();
      ctx.on('sessions', draw);
    },
  });
});
