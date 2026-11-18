// __MOD_NAME__: a floating window. It can be dragged, resized and closed;
// its place is remembered on each device.
alc.define('__MOD_ID__', (mod) => {
  const win = mod.addWindow({
    id: 'main',
    title: '__MOD_NAME__',
    icon: 'window',
    width: 340,
    height: 240,
    open: false,
    render(root, ctx) {
      const out = alc.h('pre', { class: '__MOD_ID__-out' });
      const draw = () => {
        const card = ctx.session();
        out.textContent = card ? JSON.stringify({ name: card.name, agent: card.agent, size: `${card.cols}x${card.rows}` }, null, 2) : 'No session open.';
      };
      root.append(out);
      draw();
      ctx.on('attach', draw);
      ctx.on('detach', draw);
    },
  });
  mod.addKey({ label: '__MOD_NAME__', run: () => win.toggle() });
  mod.addCommand({ id: 'toggle', title: 'Toggle __MOD_NAME__', run: () => win.toggle() });
});
