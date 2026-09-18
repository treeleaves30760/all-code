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

  // app.js hands over null when reading the storage property itself threw.
  assert.equal(core.loadToken(fakeLocation('#k=xyz'), history, null), 'xyz');
  assert.equal(core.loadToken(fakeLocation(''), history, null), null);
  assert.doesNotThrow(() => core.forgetToken(null));
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

/* --------------------------------------------------------------- rail */

test('the rail is shown unless this viewer folded it away', () => {
  const storage = fakeStorage();
  assert.equal(core.loadRailFolded(storage), false, 'a first visit gets the page as it was');

  core.saveRailFolded(storage, true);
  assert.equal(storage.getItem(core.RAIL_KEY), 'folded');
  assert.equal(core.loadRailFolded(storage), true);

  core.saveRailFolded(storage, false);
  assert.equal(core.loadRailFolded(storage), false);
  assert.equal(storage.size, 0, 'unfolding leaves nothing behind');
});

/* Blocked site data throws on the property read itself, which app.js turns
 * into null, and a private window can throw on every call. Neither may cost
 * the page anything but the memory of a fold. */
test('a fold that cannot be read or written never breaks the page', () => {
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
  for (const storage of [hostile, null, undefined]) {
    assert.equal(core.loadRailFolded(storage), false);
    assert.doesNotThrow(() => core.saveRailFolded(storage, true));
    assert.doesNotThrow(() => core.saveRailFolded(storage, false));
  }
  assert.equal(
    core.loadRailFolded(fakeStorage({ 'alc.rail': 'yes' })),
    false,
    'only the exact value folds it',
  );
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

/* -------------------------------------------------------------- usage */

/** An account row shaped the way `alc usage --json` writes one. */
function account(overrides) {
  return Object.assign(
    {
      profile: 'codex',
      kind: 'codex',
      label: 'me@example.com',
      plan: 'plus',
      source: 'profile',
      state: 'ok',
      windows: [{ name: '5h', scope: null, used_percent: 37, resets_at: 8800 }],
      balance: null,
      error: null,
      fetched_at: 1000,
    },
    overrides,
  );
}

test('a quota level follows the same threshold the terminal report uses', () => {
  assert.equal(core.WARN_PERCENT, 75);
  assert.equal(core.quotaLevel(74), 'ok');
  assert.equal(core.quotaLevel(75), 'warn');
  assert.equal(core.quotaLevel(100), 'danger');
  assert.equal(core.quotaLevel(140), 'danger');
});

test('a percentage outside its range is clamped and anything unreadable is zero', () => {
  assert.equal(core.clampPercent(-5), 0);
  assert.equal(core.clampPercent(140), 100);
  assert.equal(core.clampPercent('nonsense'), 0);
  assert.equal(core.clampPercent(undefined), 0);
});

test('a countdown is coarse and never negative', () => {
  assert.equal(core.formatCountdown(-5), '0s');
  assert.equal(core.formatCountdown(45), '45s');
  assert.equal(core.formatCountdown(130), '2m');
  assert.equal(core.formatCountdown(7800), '2h 10m');
  assert.equal(core.formatCountdown(3 * 86400 + 3600), '3d 1h');
});

test('a token count is shown by its magnitude', () => {
  assert.equal(core.compactCount(999), '999');
  assert.equal(core.compactCount(1500), '1.5K');
  assert.equal(core.compactCount(88000), '88K');
  assert.equal(core.compactCount(1200000), '1.2M');
});

test('a window says how much is left, not how much is gone', () => {
  const t = core.strings('en');
  const described = core.describeWindow(
    { name: '5h', scope: null, used_percent: 37, resets_at: 8800 },
    1000,
    t,
  );
  assert.equal(described.leftPercent, 63);
  assert.equal(described.left, '63% left');
  assert.equal(described.resets, 'resets in 2h 10m');
  assert.equal(described.level, 'ok');
});

test('a scoped window is labelled by the model it covers', () => {
  const t = core.strings('en');
  const described = core.describeWindow(
    { name: 'week', scope: 'Fable', used_percent: 90, resets_at: 0 },
    1000,
    t,
  );
  assert.equal(described.label, 'Fable week');
  assert.equal(described.level, 'warn', 'red is reserved for a window that is actually gone');
  assert.equal(described.resets, '', 'no reset time means no countdown at all');
});

test('a per-model window nobody has touched is left out', () => {
  const windows = core.accountWindows(
    account({
      windows: [
        { name: '5h', scope: null, used_percent: 0, resets_at: 0 },
        { name: 'week', scope: 'Spark', used_percent: 0, resets_at: 0 },
        { name: 'week', scope: 'Astra', used_percent: 4, resets_at: 0 },
      ],
    }),
  );
  assert.deepEqual(
    windows.map((window) => window.scope),
    [null, 'Astra'],
  );
});

test('an account note shows a balance, or the sentence that replaces it', () => {
  assert.equal(
    core.accountNote(account({ balance: { remaining: 37.6, limit: 50, unit: 'USD' } })),
    '$37.60 / $50.00',
  );
  assert.equal(
    core.accountNote(account({ windows: [], error: 'no quota API' })),
    'no quota API',
  );
  assert.equal(core.accountNote(account({})), '');
});

test('a report that is missing or malformed yields nothing rather than throwing', () => {
  for (const payload of [null, undefined, 'nope', {}, { accounts: 'no' }]) {
    const model = core.normalizeUsage(payload);
    assert.deepEqual(model.accounts, []);
    assert.deepEqual(model.rows, []);
    assert.equal(model.hubEnv, false);
  }
});

test('the environment note appears only for accounts the hub resolved for itself', () => {
  const pinned = { resolved_by: 'hub', accounts: [account({ source: 'profile' })] };
  const ambient = { resolved_by: 'hub', accounts: [account({ source: 'env' })] };
  const local = { resolved_by: 'cli', accounts: [account({ source: 'env' })] };
  assert.equal(core.normalizeUsage(pinned).hubEnv, false);
  assert.equal(core.normalizeUsage(ambient).hubEnv, true);
  assert.equal(core.normalizeUsage(local).hubEnv, false, 'your own shell is not the hub');
});

test('a pair alc never carried traffic for shows dashes, not zeros', () => {
  assert.deepEqual(
    core.ledgerCells({
      provider: 'codex',
      agent: 'claude',
      launches: 14,
      turns: 231,
      input_tokens: 1200000,
      output_tokens: 88000,
    }),
    ['codex', 'claude', '14', '231', '1.3M'],
  );
  assert.deepEqual(
    core.ledgerCells({ provider: 'ollama', agent: 'claude', launches: 1, turns: 0 }),
    ['ollama', 'claude', '1', '—', '—'],
  );
});

/* The caveat under the table exists to explain a dash. A table with no dash
 * in it has nothing to explain, and a standing sentence under numbers that
 * are all real reads as a doubt about them. */
test('the token caveat is shown only when a row actually holds a dash', () => {
  const carried = {
    provider: 'codex',
    agent: 'claude',
    launches: 3,
    turns: 40,
    input_tokens: 10,
    output_tokens: 5,
  };
  const direct = { provider: 'ollama', agent: 'goose', launches: 1, turns: 0 };
  assert.equal(core.ledgerNeedsCaveat([carried]), false);
  assert.equal(core.ledgerNeedsCaveat([carried, direct]), true);
  assert.equal(core.ledgerNeedsCaveat([]), false, 'an empty table explains nothing either');
  assert.equal(core.ledgerNeedsCaveat(null), false);
});

/* `impl Default for Config` seeds every starter profile with a name identical
 * to its kind, so the card's heading and its first chip were the same word on
 * adjacent lines for anyone who never renamed a profile. */
test('an account does not repeat its own name as a chip', () => {
  assert.deepEqual(core.describeAccount(account({ profile: 'codex', kind: 'codex' })), ['plus']);
  assert.deepEqual(
    core.describeAccount(account({ profile: 'work', kind: 'codex' })),
    ['codex', 'plus'],
    'a renamed profile still says what kind it is',
  );
  assert.deepEqual(core.describeAccount(account({ profile: 'codex', kind: 'codex', plan: null })), []);
});

test('a string template fills what it is given and blanks what it is not', () => {
  assert.equal(core.fill('{p}% left', { p: 63 }), '63% left');
  assert.equal(core.fill('resets in {t}', {}), 'resets in ');
  assert.equal(core.fill('nothing to fill', { p: 1 }), 'nothing to fill');
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

/* The permission bar had one span, and nine different messages took turns in
 * it - persistent facts about the agent and one-off outcomes of a click
 * overwriting each other, which is why app.js grew a save-and-restore around
 * it. This decides each class of fact separately so the renderer can give
 * every one its own place instead of its own turn. */
test('an agent alc cannot move gets a dead control and the reason it reported', () => {
  const view = core.permissionView(
    {
      modes: [],
      flags_verified: true,
      set: { kind: 'unsupported', reason: 'Pi has no permission modes, by design.' },
    },
    { unsandboxed: true, permission: {} },
  );
  assert.equal(view.control, 'dead');
  assert.equal(view.reason, 'Pi has no permission modes, by design.');
  assert.equal(view.ungated, true);
});

/* This mode could be chosen, the POST round-tripped, and only then did a
 * sentence appear saying it was never selectable. RelaunchOnly is a property
 * of the whole agent, so the control is dead from the start instead. */
test('an agent that can only be relaunched is dead before the pick, not after', () => {
  const view = core.permissionView(
    { modes: [{ rung: 'plan', label: 'plan' }], flags_verified: true, set: { kind: 'relaunch-only' } },
    { unsandboxed: false, permission: { rung: 'plan' } },
  );
  assert.equal(view.control, 'dead');
  assert.equal(view.deadKind, 'relaunch-only');
  assert.deepEqual(view.options, [], 'nothing to choose from, so nothing is offered');
});

test('an agent that can only be cycled is never offered a dropdown', () => {
  const view = core.permissionView(
    {
      modes: [{ rung: 'plan', label: 'plan' }],
      flags_verified: true,
      set: { kind: 'cycle', order: 'plan → auto → full' },
    },
    { unsandboxed: false, permission: { rung: 'plan' } },
  );
  assert.equal(view.control, 'cycle');
  assert.equal(view.order, 'plan → auto → full');
});

test('a settable agent lists its modes with the reported one selected', () => {
  const view = core.permissionView(
    {
      modes: [
        { rung: 'plan', label: 'plan mode' },
        { rung: 'auto', label: 'auto' },
      ],
      flags_verified: true,
      set: { kind: 'absolute', template: '/mode {}' },
    },
    { unsandboxed: false, permission: { rung: 'auto', native: 'auto' } },
  );
  assert.equal(view.control, 'pick');
  assert.deepEqual(view.options.map((option) => option.value), ['plan', 'auto']);
  assert.deepEqual(view.options.map((option) => option.selected), [false, true]);
  assert.equal(
    view.options[0].label,
    'plan · plan mode',
    "alc's rung and the agent's own word, together, always",
  );
});

/* Two different doubts, and one marker for both would say neither. `assumed`
 * is about this session's current mode; `flags_verified` is about whether alc
 * ever checked this agent against an installed copy, and it decides whether
 * alc injects a flag at launch at all. */
test('an unchecked agent and an unconfirmed mode are two separate doubts', () => {
  const caps = {
    modes: [{ rung: 'plan', label: 'plan' }],
    flags_verified: false,
    set: { kind: 'absolute', template: '/mode {}' },
  };
  const guessing = core.permissionView(caps, {
    unsandboxed: false,
    permission: { rung: 'plan', confidence: 'assumed' },
  });
  assert.equal(guessing.unverified, true);
  assert.equal(guessing.assumed, true);

  const checked = core.permissionView(
    Object.assign({}, caps, { flags_verified: true }),
    { unsandboxed: false, permission: { rung: 'plan', confidence: 'reported' } },
  );
  assert.equal(checked.unverified, false);
  assert.equal(checked.assumed, false);
});

/* /api/caps can be a request that failed, or a hub one version behind that
 * has never heard of this agent. Neither may leave the bar half-drawn. */
test('a session whose capabilities never loaded still gets a control', () => {
  const view = core.permissionView(null, { unsandboxed: false, permission: {} });
  assert.equal(view.control, 'dead');
  assert.equal(view.reason, '');
  assert.deepEqual(view.options, []);
});

/* The row says why a session is gone; the header light says why the page is.
 * zh-TW had both as 已結束, so a dropped connection read exactly like an
 * agent that had finished - and the send-failure toast at app.js told a zh
 * reader their session had ended when only the socket had. */
test('a finished session and a dropped connection never read the same', () => {
  for (const locale of Object.keys(core.STRINGS)) {
    const t = core.strings(locale);
    assert.notEqual(t.exited, t.ended, locale);
  }
});

/* Nothing else fails when a key is added to one locale and forgotten in the
 * other: the page renders `undefined` in the language nobody on the team
 * reads. Key sets and non-emptiness only - a "every key is referenced" check
 * would be brittle against the dynamic `T[state]` lookup. */
test('every locale carries the same keys, and none of them are blank', () => {
  const reference = Object.keys(core.STRINGS.en).sort();
  for (const locale of Object.keys(core.STRINGS)) {
    assert.deepEqual(Object.keys(core.STRINGS[locale]).sort(), reference, locale);
    for (const [key, value] of Object.entries(core.STRINGS[locale])) {
      assert.ok(typeof value === 'string' && value.trim(), `${locale}.${key} is blank`);
    }
  }
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

/* ----------------------------------------------------------- geometry */

/* A grid whose size the page does not own has to be drawn as it is, as large
 * as it fits. `fitGrid` returns two things: the next font size to try, and
 * the scale that makes the grid AS IT IS NOW fit the frame. app.js applies
 * the font size and re-measures, and applies the scale once the size has
 * settled - so the scale is the safety net, not the mechanism. */

const CELL = { width: 8, height: 17 };
const view = (over) => ({ fontSize: 13, cell: CELL, cols: 80, rows: 24, ...over });
/* What a renderer reports back after the proposed font size is applied: a
 * cell measured from the font and rounded to whole device pixels, which is
 * the whole reason the fit is a loop and not one division. Measured from the
 * base font every time, the way a renderer does - compounding the rounding
 * of the previous pass would model a renderer that cannot shrink. */
const stepped = (from, placed) =>
  view({
    fontSize: placed.fontSize,
    cols: from.cols,
    rows: from.rows,
    cell: {
      width: Math.round(CELL.width * (placed.fontSize / 13)),
      height: Math.round(CELL.height * (placed.fontSize / 13)),
    },
  });
const fits = (v, box) =>
  v.cell.width * v.cols <= box.width + 0.5 && v.cell.height * v.rows <= box.height + 0.5;

test('a grid smaller than its frame is grown until one axis fills it', () => {
  // 640x408 into 1280x816: room for twice the font on both axes.
  const box = { width: 1280, height: 816 };
  const placed = core.fitGrid(view(), box);
  assert.equal(placed.fontSize, 26);
  assert.equal(placed.scale, 1, 'it already fits, so there is nothing to scale');
  assert.ok(fits(stepped(view(), placed), box), 'and the proposed size fits too');
});

test('the tighter axis decides, so the grid is never cropped', () => {
  // Twice the width but only a tenth more height: the height decides, and
  // 449/408 x 13 is 14.3 where the width alone would have said 26.
  const box = { width: 1280, height: 449 };
  const placed = core.fitGrid(view(), box);
  assert.equal(placed.fontSize, 14.3);

  // One step does not have to land inside the frame, and here it does not:
  // a cell that rounds to whole device pixels overshoots 17 -> 19, which is
  // 456 rows-deep in a 449 frame. What must be true is that iterating pulls
  // it back rather than leaving it there - and that at every step the scale
  // covers whatever is still hanging out.
  let current = view();
  let overshot = false;
  for (let pass = 0; pass < 5; pass += 1) {
    const step = core.fitGrid(current, box);
    if (!fits(current, box)) {
      overshot = true;
      assert.ok(step.scale < 1, `pass ${pass}: an overshoot must be covered`);
    }
    if (step.fontSize === current.fontSize) break;
    current = stepped(current, step);
  }
  assert.ok(overshot, 'this shape is here because rounding overshoots it');
  assert.ok(fits(current, box), 'and it settles inside the frame');
});

test('a grid larger than its frame is shrunk rather than clipped', () => {
  const box = { width: 400, height: 800 };
  const wide = view({ cols: 200 });
  const placed = core.fitGrid(wide, box);
  assert.equal(placed.fontSize, core.FIT_MIN_FONT, '200 cols in 400px wants 3.25px');
  // The frame is a quarter of the grid's width right now, so that is the
  // scale that makes what is on screen fit while the font catches up.
  assert.equal(placed.scale, 0.25);
  // And at the floor it does fit, so the transform falls away.
  const next = core.fitGrid(stepped(wide, placed), box);
  assert.equal(next.scale, 1);
});

test('the font size is capped, so a tiny session does not fill a monitor with one letter', () => {
  const placed = core.fitGrid(view({ cols: 4, rows: 2 }), { width: 3000, height: 2000 });
  assert.equal(placed.fontSize, core.FIT_MAX_FONT);
  assert.equal(placed.scale, 1, 'a cap is not a shortfall; the frame centres the rest');
});

test('the limits can be pinned, since the caps are a judgement and not arithmetic', () => {
  const placed = core.fitGrid(view(), { width: 1280, height: 816 }, { min: 6, max: 20 });
  assert.equal(placed.fontSize, 20);
});

/* Every one of these is reachable: a pane that is display:none measures
 * zero, and the renderer answers with a zero cell until it has laid the grid
 * out once. Returning the size it was given leaves the terminal exactly as
 * it is, which is the only safe answer - every ratio here is 0, Infinity or
 * NaN. */
test('a frame or a grid that has not been laid out yet is left alone', () => {
  for (const [what, placed] of [
    ['no frame', core.fitGrid(view(), { width: 0, height: 0 })],
    ['no height', core.fitGrid(view(), { width: 800, height: 0 })],
    ['no cell', core.fitGrid(view({ cell: { width: 0, height: 0 } }), { width: 800, height: 600 })],
    ['no rows', core.fitGrid(view({ rows: 0 }), { width: 800, height: 600 })],
    ['no font', core.fitGrid(view({ fontSize: 0 }), { width: 800, height: 600 })],
    ['nothing at all', core.fitGrid(null, null)],
  ]) {
    assert.equal(placed.scale, 1, what);
    assert.ok(Number.isFinite(placed.fontSize), `${what}: ${placed.fontSize}`);
  }
});

/* The loop in app.js re-measures after each step, so a step that landed a
 * hair over the frame would be undone on the next pass and redone on the one
 * after. Flooring every step to the step size is what stops that. */
test('a converged fit asks for no further change', () => {
  const box = { width: 1280, height: 816 };
  let current = view();
  for (let pass = 0; pass < 4; pass += 1) {
    const placed = core.fitGrid(current, box);
    if (placed.fontSize === current.fontSize) {
      assert.ok(pass > 0, 'it should take at least one step to get there');
      assert.equal(placed.scale, 1, 'and settle with nothing left to scale');
      return;
    }
    current = stepped(current, placed);
  }
  assert.fail('the fit never settled');
});

/* The property the whole thing rests on, over a spread of shapes: whatever
 * the caller does with the font size - applies it, runs out of passes, or
 * never converges - applying `scale` to the grid it was measured from always
 * fits the frame. That is what makes app.js's pass limit safe rather than a
 * silent crop of the agent's top and bottom rows. */
test('the scale always makes the measured grid fit, converged or not', () => {
  for (const cols of [2, 40, 80, 137, 300]) {
    for (const rows of [2, 24, 60, 200]) {
      for (const box of [
        { width: 320, height: 480 },
        { width: 1280, height: 800 },
        { width: 90, height: 2000 },
      ]) {
        const v = view({ cols, rows });
        const placed = core.fitGrid(v, box);
        const wide = v.cell.width * cols * placed.scale;
        const high = v.cell.height * rows * placed.scale;
        const where = `${cols}x${rows} in ${box.width}x${box.height}`;
        assert.ok(wide <= box.width + 0.5, `${where}: ${wide} wide`);
        assert.ok(high <= box.height + 0.5, `${where}: ${high} high`);
        assert.ok(placed.scale > 0 && placed.scale <= 1, `${where}: scale ${placed.scale}`);
        assert.ok(
          placed.fontSize >= core.FIT_MIN_FONT && placed.fontSize <= core.FIT_MAX_FONT,
          `${where}: font ${placed.fontSize}`,
        );
      }
    }
  }
});
