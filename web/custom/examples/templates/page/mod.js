// __MOD_NAME__: a page with its own tab, and a palette command to reach it.
alc.define('__MOD_ID__', (mod) => {
  mod.addPage({
    id: 'main',
    title: '__MOD_NAME__',
    icon: 'grid',
    render(root, ctx) {
      const list = alc.h('ul', { class: '__MOD_ID__-list' });
      const draw = () => {
        if (!ctx.active()) return stop();
        list.replaceChildren(...alc.sessions().filter((card) => !card.exit).map((card) =>
          alc.h('li', {}, alc.h('button', { class: 'btn', onclick: () => alc.open(`session:${card.id}`) }, `${card.name} · ${card.agent}`))));
        return undefined;
      };
      const stop = alc.on('sessions', draw);
      root.append(alc.h('div', { class: 'panel' }, alc.h('h2', { class: 'pane-title' }, '__MOD_NAME__'), list));
      draw();
    },
  });
  mod.addCommand({ id: 'open', title: 'Open __MOD_NAME__', run: () => alc.open('mod:__MOD_ID__/main') });
});
