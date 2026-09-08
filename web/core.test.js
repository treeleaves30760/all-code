/* Tests for the page's decisions. `node --test web/` runs them.
 *
 * No dependencies and no DOM: everything asserted here is a pure function or
 * a store fed an explicit clock, which is the reason core.js exists apart
 * from app.js at all.
 */

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');

const core = require('./core.js');

/* ----------------------------------------------------------- reactive */

test('a signal notifies on change and not on a repeat', () => {
  const seen = [];
  const count = core.signal(0);
  count.subscribe((value) => seen.push(value));

  count.value = 1;
  count.value = 1;
  count.value = 2;

  // The first entry is the immediate call on subscribe: a subscriber that
  // has to be told the current value separately is one that can be wired up
  // out of date.
  assert.deepEqual(seen, [0, 1, 2]);
});

test('a subscriber may unsubscribe itself while being notified', () => {
  const value = core.signal('a');
  let calls = 0;
  let stop = null;
  // `subscribe` runs the callback before it returns, so the handle does not
  // exist yet on that first call - hence the guard rather than a bare stop().
  stop = value.subscribe(() => {
    calls += 1;
    if (stop) stop();
  });

  value.value = 'b';
  value.value = 'c';

  assert.equal(calls, 2, 'the immediate call plus one change, then never again');
  assert.equal(value.size, 0, 'unsubscribing mid-notification must not be skipped');
});

/* --------------------------------------------------------------- i18n */

test('every Chinese tag lands on Traditional Chinese', () => {
  for (const tag of ['zh', 'zh-TW', 'zh-CN', 'zh-Hans-CN', 'ZH-HK']) {
    assert.equal(core.pickLocale([tag]), 'zh-TW', tag);
  }
});

test('locale falls back through the list and then to English', () => {
  assert.equal(core.pickLocale(['fr-CA', 'en-GB']), 'en');
  assert.equal(core.pickLocale([]), 'en');
  assert.equal(core.pickLocale(undefined), 'en');
  assert.equal(core.pickLocale(['klingon']), 'en');
});

test('the locales carry the same keys, so no string can be missing', () => {
  const en = Object.keys(core.STRINGS.en).sort();
  const zh = Object.keys(core.STRINGS['zh-TW']).sort();
  assert.deepEqual(zh, en);
});

/* -------------------------------------------------------------- token */

function fakeStorage(initial) {
  const map = new Map(Object.entries(initial || {}));
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
    get size() {
      return map.size;
    },
  };
}

function fakeLocation(hash) {
  return { hash, pathname: '/', search: '' };
}

test('a token in the fragment is kept and stripped from the address bar', () => {
  const storage = fakeStorage();
  const replaced = [];
  const history = { replaceState: (_a, _b, url) => replaced.push(url) };

  const token = core.loadToken(fakeLocation('#k=abc123'), history, storage);

  assert.equal(token, 'abc123');
  assert.equal(storage.getItem(core.TOKEN_KEY), 'abc123');
  assert.deepEqual(replaced, ['/'], 'the fragment must not survive in history');
});

/* This is the bug where refreshing the page emptied the session list: the
 * token lived only in a closure, so F5 left the page unauthenticated. */
test('a refresh with no fragment recovers the token from the tab', () => {
  const storage = fakeStorage({ 'alc.token': 'abc123' });
  const history = { replaceState: () => assert.fail('nothing to strip') };

  assert.equal(core.loadToken(fakeLocation(''), history, storage), 'abc123');
});

test('storage that throws never breaks the page load', () => {
  const hostile = {
    getItem() {
      throw new Error('denied');
    },
    setItem() {
      throw new Error('denied');
    },
    removeItem() {
      throw new Error('denied');
    },
  };
  const history = { replaceState: () => {} };

  assert.equal(core.loadToken(fakeLocation('#k=xyz'), history, hostile), 'xyz');
  assert.equal(core.loadToken(fakeLocation(''), history, hostile), null);
  assert.doesNotThrow(() => core.forgetToken(hostile));
});

test('a fragment without a token is not mistaken for one', () => {
  const storage = fakeStorage();
  const history = { replaceState: () => {} };
  assert.equal(core.parseToken('#other=1'), null);
  assert.equal(core.loadToken(fakeLocation('#other=1'), history, storage), null);
});

test('a token later in the fragment is still found', () => {
  assert.equal(core.parseToken('#a=1&k=tok_en-9'), 'tok_en-9');
});

/* ----------------------------------------------------------- sessions */

function card(id, state, startedAt, extra) {
  return Object.assign(
    {
      id,
      name: id,
      agent: 'claude',
      provider: 'codex',
      provider_kind: 'codex',
      model: null,
      effort: null,
      cwd: '/tmp',
      pid: 1,
      state,
      exit: state === 'exited' ? { code: 0, signal: null } : null,
      cols: 80,
      rows: 24,
      viewers: 0,
      started_at: startedAt || 1000,
      unsandboxed: false,
      permission: {},
    },
    extra || {},
  );
}

test('running sessions come first, newest first', () => {
  const store = core.createSessionStore();
  const rows = store.reconcile(
    [card('old', 'running', 100), card('dead', 'exited', 900), card('new', 'running', 500)],
    0,
  );
  assert.deepEqual(
    rows.map((row) => row.card.id),
    ['new', 'old', 'dead'],
  );
});

/* The reported bug: closed sessions stayed on the list. The hub keeps their
 * cards for 15 minutes, so the page has to decide this for itself. */
test('an exited session is shown briefly, then drops off the list', () => {
  const store = core.createSessionStore({ graceMs: 1000 });
  const cards = [card('a', 'exited', 10)];

  assert.equal(store.reconcile(cards, 0).length, 1, 'visible when it first exits');
  assert.equal(store.reconcile(cards, 500)[0].fading, false);
  assert.equal(store.reconcile(cards, 900)[0].fading, true, 'warns before it goes');
  assert.equal(store.reconcile(cards, 1001).length, 0, 'gone once the grace is up');
});

test('the exit clock starts when the page first sees it, not when it exits', () => {
  const store = core.createSessionStore({ graceMs: 1000 });
  // A page opened long after the session ended still gets its full grace.
  assert.equal(store.reconcile([card('a', 'exited', 10)], 60_000).length, 1);
  assert.equal(store.reconcile([card('a', 'exited', 10)], 60_500).length, 1);
  assert.equal(store.reconcile([card('a', 'exited', 10)], 61_001).length, 0);
});

test('a session that comes back to life is shown again', () => {
  const store = core.createSessionStore({ graceMs: 1000 });
  store.reconcile([card('a', 'exited', 10)], 0);
  store.reconcile([card('a', 'exited', 10)], 900);
  const rows = store.reconcile([card('a', 'running', 10)], 950);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].exited, false);
  assert.equal(rows[0].fading, false, 'the old exit clock must not still be running');
});

test('the exit clock is forgotten once the server reaps the card', () => {
  const store = core.createSessionStore({ graceMs: 1000 });
  store.reconcile([card('a', 'exited', 10)], 0);
  store.reconcile([], 100);
  // Same id, reused: it gets a fresh grace rather than inheriting the old one.
  assert.equal(store.reconcile([card('a', 'exited', 10)], 2000).length, 1);
});

test('nextExpiry reports when the list will next need redrawing', () => {
  const store = core.createSessionStore({ graceMs: 1000 });
  assert.equal(store.nextExpiry(0), null, 'nothing exited, nothing to wait for');
  store.reconcile([card('a', 'exited', 10)], 0);
  assert.equal(store.nextExpiry(400), 600);
  store.reconcile([card('a', 'exited', 10), card('b', 'exited', 20)], 500);
  assert.equal(store.nextExpiry(500), 500, 'the soonest of the two');
});

test('a malformed payload is treated as an empty list', () => {
  const store = core.createSessionStore();
  assert.deepEqual(store.reconcile(null, 0), []);
  assert.deepEqual(store.reconcile(undefined, 0), []);
});

/* ------------------------------------------------------------ display */

test('a session is described by what it is, without repeating itself', () => {
  // agent and provider_kind differ in the fixture, so both are shown.
  assert.deepEqual(core.describeSession(card('a', 'running')), ['claude', 'codex']);
  // ...and a provider that merely repeats the agent's name is not shown twice.
  assert.deepEqual(
    core.describeSession(card('a', 'running', 1, { provider_kind: 'claude' })),
    ['claude'],
  );
  assert.deepEqual(
    core.describeSession(
      card('a', 'running', 1, { provider_kind: 'openrouter', model: 'sonnet', effort: 'high' }),
    ),
    ['claude', 'openrouter', 'sonnet', 'high'],
  );
});

test('age reads coarsely at every scale', () => {
  const now = 1_000_000_000_000;
  const ago = (seconds) => core.formatAge(now / 1000 - seconds, now);
  assert.equal(ago(5), '5s');
  assert.equal(ago(90), '1m');
  assert.equal(ago(3600), '1h 0m');
  assert.equal(ago(3600 * 25), '1d 1h');
  assert.equal(core.formatAge(0, now), '', 'no start time, no guess');
});

test('age never runs backwards on a skewed clock', () => {
  const now = 1_000_000;
  assert.equal(core.formatAge(now / 1000 + 60, now), '0s');
});

test('an exit says how it ended', () => {
  const t = core.strings('en');
  assert.equal(core.formatExit({ code: 0, signal: null }, t), 'exited 0');
  assert.equal(core.formatExit({ code: 1, signal: null }, t), 'exited 1');
  assert.equal(core.formatExit({ code: null, signal: 'SIGKILL' }, t), 'killed by SIGKILL');
  assert.equal(core.formatExit(null, t), 'exited');
});

/* ----------------------------------------------------------- protocol */

function frame(opcode, seq, payload) {
  const body = Buffer.from(payload || '');
  const bytes = new Uint8Array(9 + body.length);
  bytes[0] = opcode;
  new DataView(bytes.buffer).setBigUint64(1, BigInt(seq));
  bytes.set(body, 9);
  return bytes;
}

test('a binary frame decodes its opcode, big-endian sequence and payload', () => {
  const decoded = core.decodeFrame(frame(core.OP_OUTPUT, 0x0102030405, 'hi'));
  assert.equal(decoded.opcode, core.OP_OUTPUT);
  assert.equal(decoded.seq, 0x0102030405);
  assert.equal(decoded.snapshot, false);
  assert.equal(Buffer.from(decoded.payload).toString(), 'hi');
});

test('a snapshot is flagged so the terminal knows to reset first', () => {
  assert.equal(core.decodeFrame(frame(core.OP_SNAPSHOT, 7, 'x')).snapshot, true);
});

test('a frame decodes correctly when it does not start at offset zero', () => {
  // Browsers hand over an ArrayBuffer that may be a slice of a larger one.
  const outer = new Uint8Array(20);
  outer.set(frame(core.OP_OUTPUT, 9, 'ok'), 5);
  const decoded = core.decodeFrame(outer.subarray(5, 16));
  assert.equal(decoded.seq, 9);
  assert.equal(Buffer.from(decoded.payload).toString(), 'ok');
});

test('short and unknown frames are dropped, never written to the terminal', () => {
  assert.equal(core.decodeFrame(new Uint8Array(8)), null);
  assert.equal(core.decodeFrame(frame(0x7f, 1, 'x')), null);
  assert.equal(core.decodeFrame(null), null);
});

test('backoff grows and then stops growing', () => {
  assert.equal(core.backoffDelay(0), 0);
  assert.equal(core.backoffDelay(1), 250);
  assert.equal(core.backoffDelay(2), 500);
  assert.equal(core.backoffDelay(6), 8000);
  assert.equal(core.backoffDelay(99), 8000, 'a long outage must not overflow into a huge wait');
});

/* `direction: rtl` was doing this in CSS and rendered `/Users/me` as
 * `Users/me/` — the leading slash is a neutral character and got reordered. */
test('a path is shortened from the left, keeping what identifies it', () => {
  assert.equal(core.elidePath('/Users/me/Self/Github_Local/all-code'), '…/Self/Github_Local/all-code');
  assert.equal(core.elidePath('/tmp'), '/tmp', 'short paths are left alone');
  assert.equal(core.elidePath('/a/b/c'), '/a/b/c');
  assert.equal(core.elidePath('/a/b/c/d'), '…/b/c/d');
  assert.equal(core.elidePath(''), '');
  assert.equal(core.elidePath(null), '');
});

test('the elided path keeps the number of segments asked for', () => {
  assert.equal(core.elidePath('/a/b/c/d/e', 2), '…/d/e');
  assert.equal(core.elidePath('/a/b/c/d/e', 1), '…/e');
});

/* A cycling agent ignores the rung it is sent, but the server still gates on
 * it — so sending the session's current rung demanded a typed confirmation
 * whenever the session already sat above the permission ceiling. */
test('the cycle button asks for the tightest mode the agent offers', () => {
  const caps = { modes: [{ rung: 'auto' }, { rung: 'ask' }, { rung: 'plan' }] };
  assert.equal(core.cycleRung(caps), 'plan');
  assert.equal(core.cycleRung({ modes: [{ rung: 'full' }, { rung: 'auto-edit' }] }), 'auto-edit');
  assert.equal(core.cycleRung({ modes: [] }), 'plan', 'a safe default, never a loose one');
  assert.equal(core.cycleRung(null), 'plan');
});

test('the rung order runs tightest to loosest', () => {
  assert.deepEqual(core.RUNGS, ['plan', 'ask', 'auto-edit', 'auto', 'full']);
});

/* The header light is the only place these four are shown, so two of them
 * reading identically makes the light unreadable. zh-TW had 'connecting' and
 * 'live' both as 連線中. */
test('the connection states are distinguishable in every locale', () => {
  for (const locale of Object.keys(core.STRINGS)) {
    const t = core.strings(locale);
    const labels = ['connecting', 'live', 'reconnecting', 'ended'].map((k) => t[k]);
    assert.equal(new Set(labels).size, labels.length, `${locale}: ${labels.join(' / ')}`);
  }
});

/* The page must keep feeding the store the server's whole payload. Feeding
 * back the *rendered* rows hid the expired card from the store, which then
 * forgot its exit time — and the next poll showed it again for another full
 * grace period. Exited sessions came back from the dead. */
test('a card stays expired while the server keeps reporting it', () => {
  const store = core.createSessionStore({ graceMs: 1000 });
  const cards = [card('a', 'exited', 10)];

  assert.equal(store.reconcile(cards, 0).length, 1);
  assert.equal(store.reconcile(cards, 1500).length, 0, 'gone once the grace is up');
  assert.equal(store.reconcile(cards, 2000).length, 0, 'and it stays gone');
  assert.equal(store.reconcile(cards, 9000).length, 0);
});
