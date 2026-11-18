// Ambient: canvas backgrounds. Each draws at a modest frame rate, stops
// while the tab is hidden, and draws one still frame when the system asks
// for reduced motion.
alc.define('ambient', (mod) => {
  const zh = alc.locale === 'zh-TW';

  function canvasLoop(host, ctx, setup) {
    const canvas = document.createElement('canvas');
    host.append(canvas);
    const g = canvas.getContext('2d');
    let width = 0;
    let height = 0;
    let state = null;
    const resize = () => {
      const ratio = Math.min(2, window.devicePixelRatio || 1);
      width = host.clientWidth || window.innerWidth;
      height = host.clientHeight || window.innerHeight;
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      g.setTransform(ratio, 0, 0, ratio, 0, 0);
      state = setup.init(width, height);
    };
    resize();
    window.addEventListener('resize', resize);
    let frame = 0;
    let last = 0;
    const step = (time) => {
      if (!ctx.active()) return;
      frame = requestAnimationFrame(step);
      if (document.hidden || time - last < 1000 / (setup.fps || 30)) return;
      const dt = Math.min(100, time - (last || time));
      last = time;
      setup.draw(g, state, width, height, dt, time);
    };
    setup.draw(g, state, width, height, 16, 0);
    if (!ctx.reducedMotion()) frame = requestAnimationFrame(step);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener('resize', resize);
      canvas.remove();
    };
  }

  const rand = (min, max) => min + Math.random() * (max - min);

  mod.addBackground({
    id: 'starfield',
    name: zh ? '星空' : 'Starfield',
    render: (host, ctx) => canvasLoop(host, ctx, {
      fps: 30,
      init: (w, h) => Array.from({ length: Math.round((w * h) / 5000) }, () => ({ x: rand(0, w), y: rand(0, h), z: rand(0.2, 1), t: rand(0, 6) })),
      draw(g, stars, w, h, dt, time) {
        const sky = g.createLinearGradient(0, 0, 0, h);
        sky.addColorStop(0, '#05060f');
        sky.addColorStop(1, '#101634');
        g.fillStyle = sky;
        g.fillRect(0, 0, w, h);
        for (const star of stars) {
          star.x -= star.z * dt * 0.006;
          if (star.x < 0) star.x += w;
          const twinkle = 0.55 + 0.45 * Math.sin(time / 700 + star.t);
          g.fillStyle = `rgba(220,230,255,${(0.25 + star.z * 0.6) * twinkle})`;
          g.fillRect(star.x, star.y, star.z * 1.8, star.z * 1.8);
        }
      },
    }),
  });

  mod.addBackground({
    id: 'glyph-rain',
    name: zh ? '字元雨' : 'Glyph rain',
    render: (host, ctx) => canvasLoop(host, ctx, {
      fps: 20,
      init: (w) => Array.from({ length: Math.ceil(w / 16) }, () => rand(-40, 0)),
      draw(g, columns, w, h) {
        g.fillStyle = 'rgba(4,10,8,0.18)';
        g.fillRect(0, 0, w, h);
        g.font = '14px ui-monospace, monospace';
        const glyphs = '01<>{}[]/=+*アカサタナ#$%&';
        columns.forEach((y, i) => {
          const char = glyphs[Math.floor(Math.random() * glyphs.length)];
          g.fillStyle = Math.random() > 0.96 ? '#d1fae5' : 'rgba(52,211,153,0.75)';
          g.fillText(char, i * 16, y * 16);
          columns[i] = y * 16 > h && Math.random() > 0.975 ? 0 : y + 1;
        });
      },
    }),
  });

  mod.addBackground({
    id: 'waves',
    name: zh ? '波浪' : 'Waves',
    render: (host, ctx) => canvasLoop(host, ctx, {
      fps: 30,
      init: () => ({}),
      draw(g, state, w, h, dt, time) {
        const sky = g.createLinearGradient(0, 0, 0, h);
        sky.addColorStop(0, '#0b1324');
        sky.addColorStop(1, '#0e2a3a');
        g.fillStyle = sky;
        g.fillRect(0, 0, w, h);
        const layers = [['rgba(45,212,191,0.16)', 0.0006, 38, 0.62], ['rgba(56,189,248,0.14)', 0.0009, 30, 0.7], ['rgba(129,140,248,0.14)', 0.0012, 24, 0.8]];
        for (const [colour, speed, amp, base] of layers) {
          g.beginPath();
          g.moveTo(0, h);
          for (let x = 0; x <= w; x += 12) {
            const y = h * base + Math.sin(x * 0.006 + time * speed) * amp + Math.sin(x * 0.013 - time * speed * 1.7) * amp * 0.4;
            g.lineTo(x, y);
          }
          g.lineTo(w, h);
          g.closePath();
          g.fillStyle = colour;
          g.fill();
        }
      },
    }),
  });

  mod.addBackground({
    id: 'particles',
    name: zh ? '漂浮粒子' : 'Drifting particles',
    render: (host, ctx) => canvasLoop(host, ctx, {
      fps: 30,
      init: (w, h) => Array.from({ length: Math.round((w * h) / 22000) }, () => ({ x: rand(0, w), y: rand(0, h), vx: rand(-0.02, 0.02), vy: rand(-0.02, 0.02) })),
      draw(g, dots, w, h, dt) {
        const dark = ctx.mode() !== 'light';
        g.fillStyle = dark ? '#0b0c12' : '#f4f5fb';
        g.fillRect(0, 0, w, h);
        for (const dot of dots) {
          dot.x = (dot.x + dot.vx * dt + w) % w;
          dot.y = (dot.y + dot.vy * dt + h) % h;
        }
        for (let i = 0; i < dots.length; i += 1) {
          for (let j = i + 1; j < dots.length; j += 1) {
            const dx = dots[i].x - dots[j].x;
            const dy = dots[i].y - dots[j].y;
            const d = Math.hypot(dx, dy);
            if (d < 130) {
              g.strokeStyle = dark ? `rgba(129,140,248,${0.22 * (1 - d / 130)})` : `rgba(79,70,229,${0.18 * (1 - d / 130)})`;
              g.beginPath();
              g.moveTo(dots[i].x, dots[i].y);
              g.lineTo(dots[j].x, dots[j].y);
              g.stroke();
            }
          }
        }
        g.fillStyle = dark ? 'rgba(199,210,254,0.7)' : 'rgba(79,70,229,0.6)';
        for (const dot of dots) g.fillRect(dot.x - 1, dot.y - 1, 2, 2);
      },
    }),
  });
});
