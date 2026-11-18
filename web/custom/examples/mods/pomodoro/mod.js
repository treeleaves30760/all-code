// Focus timer: the end time is what is stored (per device, mod.local), so
// the timer survives a reload and keeps counting while the tab sleeps.
alc.define('pomodoro', (mod) => {
  const zh = alc.locale === 'zh-TW';
  const L = {
    focus: zh ? '專注' : 'Focus',
    rest: zh ? '休息' : 'Break',
    start: zh ? '開始' : 'Start',
    pause: zh ? '暫停' : 'Pause',
    reset: zh ? '重設' : 'Reset',
    skip: zh ? '跳過' : 'Skip',
    done: zh ? '時間到' : "Time's up",
    toRest: zh ? '休息一下。' : 'Take a break.',
    toFocus: zh ? '回到工作。' : 'Back to it.',
    rounds: zh ? '完成回合' : 'Rounds',
  };

  function load() {
    return mod.local.get('timer', { phase: 'focus', endsAt: 0, left: 0, rounds: 0 });
  }

  mod.addWidget({
    id: 'timer',
    title: zh ? '專注計時器' : 'Focus timer',
    description: zh ? '番茄鐘：專注、休息、重複。' : 'Pomodoro: focus, break, repeat.',
    icon: 'clock',
    defaultSlot: 'float',
    slots: ['float', 'view.left', 'view.right', 'list.top', 'bar'],
    size: { width: 260, height: 330 },
    options: [
      { key: 'focus', type: 'number', label: zh ? '專注（分鐘）' : 'Focus (minutes)', default: 25, min: 1, max: 180 },
      { key: 'rest', type: 'number', label: zh ? '休息（分鐘）' : 'Break (minutes)', default: 5, min: 1, max: 60 },
    ],
    render(root, ctx) {
      const minutes = (phase) => Math.max(1, Number(ctx.options[phase]) || (phase === 'focus' ? 25 : 5));
      let timer = load();
      const save = () => mod.local.set('timer', timer);
      const length = () => minutes(timer.phase) * 60000;
      const remaining = () => (timer.endsAt ? Math.max(0, timer.endsAt - Date.now()) : timer.left || length());

      const face = alc.h('div', { class: 'pomo-face' });
      const label = alc.h('div', { class: 'pomo-phase' });
      const ring = alc.h('div', { class: 'pomo-ring' }, face);
      const toggle = alc.h('button', { type: 'button', class: 'btn small accent' });
      const reset = alc.h('button', { type: 'button', class: 'btn small' }, L.reset);
      const skip = alc.h('button', { type: 'button', class: 'btn small' }, L.skip);
      const rounds = alc.h('div', { class: 'pomo-rounds' });
      const compact = ctx.slot === 'bar';
      root.append(compact
        ? alc.h('button', { type: 'button', class: 'pomo-inline', onclick: () => toggle.click() }, face)
        : alc.h('div', { class: 'pomo' }, label, ring, alc.h('div', { class: 'pomo-actions' }, toggle, reset, skip), rounds));

      const next = () => {
        if (timer.phase === 'focus') timer.rounds += 1;
        timer.phase = timer.phase === 'focus' ? 'rest' : 'focus';
        timer.endsAt = 0;
        timer.left = 0;
        save();
      };

      const draw = () => {
        const left = remaining();
        if (timer.endsAt && left <= 0) {
          const wasFocus = timer.phase === 'focus';
          next();
          alc.notify(L.done, wasFocus ? L.toRest : L.toFocus);
        }
        const ms = remaining();
        const m = Math.floor(ms / 60000);
        const s = Math.floor((ms % 60000) / 1000);
        face.textContent = `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
        label.textContent = timer.phase === 'focus' ? L.focus : L.rest;
        toggle.textContent = timer.endsAt ? L.pause : L.start;
        rounds.textContent = `${L.rounds}: ${timer.rounds}`;
        ring.style.setProperty('--pomo', `${Math.round((1 - ms / length()) * 360)}deg`);
        root.dataset.phase = timer.phase;
        root.dataset.running = timer.endsAt ? 'yes' : 'no';
      };

      toggle.addEventListener('click', () => {
        if (timer.endsAt) {
          timer.left = remaining();
          timer.endsAt = 0;
        } else {
          timer.endsAt = Date.now() + (timer.left || length());
          timer.left = 0;
        }
        save();
        draw();
      });
      reset.addEventListener('click', () => {
        timer = { phase: 'focus', endsAt: 0, left: 0, rounds: 0 };
        save();
        draw();
      });
      skip.addEventListener('click', () => {
        next();
        draw();
      });
      // Another window of the same page changed it.
      const sync = (event) => {
        if (event.key === `alc.mod.${mod.id}.timer`) {
          timer = load();
          draw();
        }
      };
      window.addEventListener('storage', sync);
      ctx.onCleanup(() => window.removeEventListener('storage', sync));
      draw();
      ctx.every(1000, draw);
    },
  });

  mod.addCommand({
    id: 'toggle',
    title: zh ? '專注計時器：顯示／隱藏' : 'Focus timer: show or hide',
    run: () => alc.openWindow('pomodoro/timer'),
  });
});
