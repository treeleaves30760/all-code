// __MOD_NAME__: a background. render(host, ctx) draws into a full-page
// element behind everything; return a function that stops it.
alc.define('__MOD_ID__', (mod) => {
  mod.addBackground({
    id: 'main',
    name: '__MOD_NAME__',
    render(host, ctx) {
      const canvas = document.createElement('canvas');
      host.append(canvas);
      const g = canvas.getContext('2d');
      let frame = 0;
      const draw = (time) => {
        canvas.width = host.clientWidth;
        canvas.height = host.clientHeight;
        const hue = (time / 100) % 360;
        const gradient = g.createLinearGradient(0, 0, canvas.width, canvas.height);
        gradient.addColorStop(0, `hsl(${hue} 60% 18%)`);
        gradient.addColorStop(1, `hsl(${(hue + 80) % 360} 60% 12%)`);
        g.fillStyle = gradient;
        g.fillRect(0, 0, canvas.width, canvas.height);
        // One still frame when the system asks for less motion.
        if (!ctx.reducedMotion() && ctx.active()) frame = requestAnimationFrame(draw);
      };
      draw(0);
      return () => cancelAnimationFrame(frame);
    },
  });
});
