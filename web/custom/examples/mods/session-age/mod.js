// Session age: a chip on each running session row once it is long-lived.
alc.define('session-age', (mod) => {
  const HOUR = 3600;
  mod.addSessionChip((card) => {
    if (card.exit || !card.started_at) return null;
    const age = Date.now() / 1000 - card.started_at;
    if (age < HOUR) return null;
    const hours = Math.floor(age / HOUR);
    return { text: `${hours}h+`, tone: hours >= 4 ? 'warn' : 'accent' };
  });
});
