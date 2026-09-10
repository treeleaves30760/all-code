/* Everything the page decides, with no DOM in sight.
 *
 * The page ships inside the alc binary with no build step, so there is no
 * bundler and no test runner that understands a browser. Keeping the
 * decisions here - what to show, in what order, for how long, and how to
 * read a frame off the wire - means they can be tested with plain
 * `node --test`, and it leaves app.js as wiring thin enough to read.
 *
 * Loaded as a classic script in the browser (it assigns `window.AlcCore`)
 * and as CommonJS by the tests. No module system is involved in either.
 */

(function (root, factory) {
  'use strict';
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.AlcCore = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, function () {
  'use strict';

  /* ----------------------------------------------------------- reactive */

  /* A signal is the smallest thing that fixes the bug class this page kept
   * hitting: state changed in one place and the DOM that showed it was
   * updated in another, or not at all. Subscribers re-run on change and
   * nowhere else, and `Object.is` equality means a poll that returns the
   * same value costs nothing. */
  function signal(initial) {
    let value = initial;
    const subscribers = new Set();
    return {
      get value() {
        return value;
      },
      set value(next) {
        if (Object.is(next, value)) return;
        value = next;
        // Copied before iterating: a subscriber is allowed to unsubscribe
        // itself, and mutating a Set mid-iteration silently skips entries.
        for (const run of [...subscribers]) run(value);
      },
      /** Runs `fn` now and on every change. Returns an unsubscribe. */
      subscribe(fn) {
        subscribers.add(fn);
        fn(value);
        return () => subscribers.delete(fn);
      },
      get size() {
        return subscribers.size;
      },
    };
  }

  /* ---------------------------------------------------------------- i18n */

  const STRINGS = {
    en: {
      empty: 'No sessions yet',
      emptyHow: 'Start one with',
      emptyCommand: 'alc share claude',
      copied: 'Copied',
      deniedTitle: 'This link has expired',
      deniedHow: 'Get a fresh one with',
      deniedCommand: 'alc sessions',
      send: 'Send',
      composerHint: 'Write a prompt, send it as one block',
      viewer: 'view only',
      operator: 'can type',
      back: 'Back to sessions',
      running: 'running',
      exited: 'exited',
      live: 'live',
      connecting: 'connecting',
      reconnecting: 'reconnecting',
      ended: 'ended',
      signalled: 'killed by',
      viewers: 'watching',
      readOnly: 'This link can watch but not type.',
      loadFailed: 'Could not reach alc.',
      denied: 'This link is no longer valid — open a fresh one from `alc sessions`.',
      unsandboxed: 'no permission model',
      permission: 'Permission',
      permUnknown: 'not set by alc',
      permCycle: 'Cycle (⇧Tab)',
      permCycleNote: 'Cycle only:',
      permSent: 'Sent',
      permPicker: 'The agent opened its own picker — finish it in the terminal.',
      permRelaunch: 'Only selectable at launch:',
      permConfirm: 'Run this on the machine running the agent, then retry:',
      permUnsupported: 'Not available:',
      permUnverified: 'alc has not checked this agent’s flags against an installed copy.',
      unsandboxedNote: 'This agent gates nothing.',
    },
    'zh-TW': {
      empty: '目前沒有 session',
      emptyHow: '用這個指令開一個',
      emptyCommand: 'alc share claude',
      copied: '已複製',
      deniedTitle: '這個連結已失效',
      deniedHow: '用這個指令取得新的',
      deniedCommand: 'alc sessions',
      send: '送出',
      composerHint: '輸入提示詞，整段送出',
      viewer: '唯讀',
      operator: '可輸入',
      back: '回到 session 清單',
      running: '執行中',
      exited: '已結束',
      live: '已連線',
      connecting: '連線中…',
      reconnecting: '重新連線中…',
      ended: '已結束',
      signalled: '被中止：',
      viewers: '人在看',
      readOnly: '這個連結只能觀看，不能輸入。',
      loadFailed: '無法連上 alc。',
      denied: '這個連結已失效 —— 請用 `alc sessions` 取得新的。',
      unsandboxed: '無權限模式',
      permission: '權限',
      permUnknown: 'alc 未設定',
      permCycle: '循環切換 (⇧Tab)',
      permCycleNote: '只能循環切換：',
      permSent: '已送出',
      permPicker: 'agent 開啟了自己的選單 —— 請在終端機裡完成。',
      permRelaunch: '這個模式只能在啟動時選擇：',
      permConfirm: '請在跑 agent 的那台機器上執行，然後再試一次：',
      permUnsupported: '不支援：',
      permUnverified: 'alc 尚未對照已安裝的版本確認這個 agent 的旗標。',
      unsandboxedNote: '這個 agent 沒有任何權限控制。',
    },
  };

  /* Every Chinese tag lands on zh-TW: alc's only Chinese copy is Traditional,
   * and showing a zh-CN reader English when they asked for Chinese is worse
   * than showing them the wrong variant of it. */
  function pickLocale(languages) {
    const tags = Array.isArray(languages) && languages.length ? languages : ['en'];
    for (const tag of tags) {
      const lower = String(tag || '').toLowerCase();
      if (lower.startsWith('zh')) return 'zh-TW';
      if (STRINGS[lower]) return lower;
      const base = lower.split('-')[0];
      if (STRINGS[base]) return base;
    }
    return 'en';
  }

  function strings(locale) {
    return STRINGS[locale] || STRINGS.en;
  }

  /* --------------------------------------------------------------- token */

  /* The token rides in the fragment, which browsers never send to a server.
   * It is lifted out and the address bar is rewritten immediately so it
   * cannot reach history, a screenshot, or a pasted link.
   *
   * It then goes to sessionStorage rather than nowhere, because "nowhere"
   * meant every refresh logged the user out of their own session list. A
   * sessionStorage entry dies with the tab, which is the same lifetime the
   * closure had - it survives F5 and nothing else. localStorage would not:
   * this is a remote shell, and a token that outlives the tab outlives the
   * user's intent to grant access. */
  const TOKEN_KEY = 'alc.token';

  function parseToken(hash) {
    const match = /[#&]k=([A-Za-z0-9_-]+)/.exec(hash || '');
    return match ? match[1] : null;
  }

  function loadToken(location, history, storage) {
    const fromUrl = parseToken(location.hash);
    if (fromUrl) {
      try {
        storage.setItem(TOKEN_KEY, fromUrl);
      } catch {
        // Private mode, or storage disabled. The token still works for this
        // page load; only surviving a refresh is lost.
      }
      history.replaceState(null, '', location.pathname + location.search);
      return fromUrl;
    }
    try {
      return storage.getItem(TOKEN_KEY);
    } catch {
      return null;
    }
  }

  function forgetToken(storage) {
    try {
      storage.removeItem(TOKEN_KEY);
    } catch {
      // Nothing to do: the in-memory copy is dropped by the caller anyway.
    }
  }

  /* ------------------------------------------------------------ sessions */

  /* How long an exited session stays on the list.
   *
   * The hub keeps its card for 15 minutes so `alc sessions` can still report
   * how it ended, but a list that accumulates every session of the afternoon
   * is a list nobody scans. Half a minute is long enough to see the exit
   * land on a session you were watching, short enough that the list is
   * always about what is running now. */
  const EXIT_GRACE_MS = 30_000;

  function createSessionStore(options) {
    const settings = options || {};
    const graceMs = settings.graceMs === undefined ? EXIT_GRACE_MS : settings.graceMs;
    // id -> the moment this page first saw the session exited. Kept here
    // rather than taken from the card because the card carries no exit
    // time, and the server's LINGER is far longer than this one.
    const seenExitedAt = new Map();

    function isExited(card) {
      return card.state === 'exited';
    }

    return {
      /* Folds a fresh /api/sessions payload into what should be on screen.
       *
       * Pure apart from the exit clock: same cards plus same `now` give the
       * same rows, which is what makes it testable. */
      reconcile(cards, now) {
        const list = Array.isArray(cards) ? cards : [];
        const at = now === undefined ? Date.now() : now;
        const present = new Set(list.map((card) => card.id));
        for (const id of [...seenExitedAt.keys()]) {
          // A session the server has reaped starts over if its id is ever
          // reused, and stops holding memory either way.
          if (!present.has(id)) seenExitedAt.delete(id);
        }

        const rows = [];
        for (const card of list) {
          if (!isExited(card)) {
            seenExitedAt.delete(card.id);
            rows.push({ card, exited: false, fading: false });
            continue;
          }
          let since = seenExitedAt.get(card.id);
          if (since === undefined) {
            since = at;
            seenExitedAt.set(card.id, since);
          }
          const age = at - since;
          if (age > graceMs) continue;
          // The last third is spent visibly on the way out, so a card never
          // just blinks out of existence while being read.
          rows.push({ card, exited: true, fading: age > graceMs * 0.66 });
        }

        rows.sort(compareRows);
        return rows;
      },

      /** Milliseconds until the next row needs to disappear, or null. */
      nextExpiry(now) {
        const at = now === undefined ? Date.now() : now;
        let soonest = null;
        for (const since of seenExitedAt.values()) {
          const left = since + graceMs - at;
          if (left < 0) continue;
          if (soonest === null || left < soonest) soonest = left;
        }
        return soonest;
      },

      forget(id) {
        seenExitedAt.delete(id);
      },
    };
  }

  /* Running first, then newest first. A session that just started is the one
   * the user is most likely to have come here for. */
  function compareRows(left, right) {
    if (left.exited !== right.exited) return left.exited ? 1 : -1;
    const byStart = (right.card.started_at || 0) - (left.card.started_at || 0);
    if (byStart) return byStart;
    return String(left.card.id).localeCompare(String(right.card.id));
  }

  /* The chips under a session's name: what it is, not a sentence about it. */
  function describeSession(card) {
    const bits = [card.agent];
    if (card.provider_kind && card.provider_kind !== card.agent) bits.push(card.provider_kind);
    if (card.model) bits.push(card.model);
    if (card.effort) bits.push(card.effort);
    // Worth a chip because it changes what this page can do: a tmux
    // session's size is the local terminal's, so resizing this window
    // refits the view rather than the agent.
    if (card.tmux) bits.push('tmux');
    return bits.filter(Boolean);
  }

  /* Uptime at a glance. Deliberately coarse: the exact second a session
   * started is never what the reader wants, and a ticking clock would
   * redraw the list every second for no gain. */
  function formatAge(startedAtSeconds, nowMs) {
    if (!startedAtSeconds) return '';
    const seconds = Math.max(0, Math.floor((nowMs - startedAtSeconds * 1000) / 1000));
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h ${minutes % 60}m`;
    return `${Math.floor(hours / 24)}d ${hours % 24}h`;
  }

  /* A working directory, shortened from the left.
   *
   * The end of a path is what identifies a session and the start is what
   * every session on a machine has in common, so the front is what goes.
   * Done here rather than with `direction: rtl` in CSS, which reorders the
   * leading slash of an absolute path and renders `/Users/me` as `Users/me/`.
   */
  function elidePath(path, keep) {
    const text = String(path || '');
    const segments = keep === undefined ? 3 : keep;
    const parts = text.split('/').filter(Boolean);
    if (parts.length <= segments) return text;
    return `…/${parts.slice(-segments).join('/')}`;
  }

  /** How a session ended, in the fewest words that stay accurate. */
  function formatExit(exit, t) {
    if (!exit) return t.exited;
    if (exit.signal) return `${t.signalled} ${exit.signal}`;
    return `${t.exited} ${exit.code === null || exit.code === undefined ? 0 : exit.code}`;
  }

  /* Tightest first, which is also the order alc's own SafetyRung is
   * declared in. */
  const RUNGS = ['plan', 'ask', 'auto-edit', 'auto', 'full'];

  /* The rung to send when all that is wanted is one press of a cycle key.
   *
   * A cycling agent ignores the requested rung entirely - alc just sends it
   * the keystroke - but the server still gates on the rung it was asked
   * for. Sending the session's *current* rung therefore demanded a typed
   * confirmation whenever the session already sat above the ceiling, which
   * is exactly when a user is most likely to be reaching for the button.
   * The tightest mode the agent offers can never be above a ceiling, so it
   * asks for nothing while landing the same keystroke. */
  function cycleRung(caps) {
    const offered = new Set(((caps && caps.modes) || []).map((mode) => mode.rung));
    return RUNGS.find((rung) => offered.has(rung)) || 'plan';
  }

  /* ------------------------------------------------------------ geometry */

  /* How to draw a grid whose size is not the page's to choose.
   *
   * Only a `--tmux` session's size belongs to the browser. Without it there
   * is one pty, its size is the terminal that launched it, and a page that
   * resized the agent to fit its own window left that terminal drawing for a
   * width it no longer had - which is the whole bug. So the page keeps the
   * session's real cols x rows and changes the only thing that is its own:
   * how big a character is.
   *
   * The font size is the mechanism rather than a CSS transform, and the
   * difference matters twice. Text stays real text laid out at its final
   * size, so it is sharp when the grid is blown up to fill a desktop window
   * as well as when it is shrunk onto a phone; and xterm's own hit-testing
   * divides pixels by the cell it measured, so a scaled transform would put
   * every click and drag-selection on the wrong cell while a resized font
   * leaves them exact.
   *
   * Cell metrics are not perfectly linear in the font size - a renderer
   * rounds to device pixels - so this is one step of a converging loop
   * rather than an answer: it is given the cell the last size actually
   * produced and returns the next size to try. Steps are floored to
   * `FIT_STEP`, so the grid never grows past the frame and the loop cannot
   * oscillate between two sizes that both round up.
   *
   * `scale` is the residue, and it is 1 unless the font floor was reached
   * before the grid fit - a very wide session on a phone. Then the page has
   * a choice between cropping the agent's screen and transforming what is
   * unreadable at 4px either way, and it transforms. Where the grid then
   * sits is not decided here: the frame centres it. */
  const FIT_MIN_FONT = 4;
  const FIT_MAX_FONT = 32;
  const FIT_STEP = 0.1;

  function fitGrid(view, box, limits) {
    const min = (limits && limits.min) || FIT_MIN_FONT;
    const max = (limits && limits.max) || FIT_MAX_FONT;
    const fontSize = view ? view.fontSize : 0;
    const cell = (view && view.cell) || {};
    const wide = cell.width * (view ? view.cols : 0);
    const high = cell.height * (view ? view.rows : 0);
    const room = box ? box.width : 0;
    const tall = box ? box.height : 0;
    // Not laid out yet, or measured before the renderer had an answer.
    // Leaving it alone is the only safe move: every ratio here would be
    // zero, infinity, or NaN.
    if (!(wide > 0) || !(high > 0) || !(room > 0) || !(tall > 0) || !(fontSize > 0)) {
      return { fontSize, scale: 1, left: 0, top: 0 };
    }

    // The smaller ratio wins, so the grid is never cropped.
    const want = Math.min(room / wide, tall / high);
    const stepped = Math.floor((fontSize * want) / FIT_STEP) * FIT_STEP;
    const next = Math.min(max, Math.max(min, Math.round(stepped * 10) / 10));
    // How much the clamp refused. Above the floor this is >= 1 and the
    // residue is dropped; at the floor it is the shortfall.
    return { fontSize: next, scale: Math.min(1, (fontSize * want) / next) };
  }

  /* ------------------------------------------------------------ protocol */

  const OP_OUTPUT = 0x01;
  const OP_SNAPSHOT = 0x02;

  /* `[opcode][u64 big-endian seq][payload]`. Returns null for anything that
   * is not one of those, so an unknown opcode is dropped rather than
   * written to the terminal as text. */
  function decodeFrame(bytes) {
    if (!bytes || bytes.length < 9) return null;
    const opcode = bytes[0];
    if (opcode !== OP_OUTPUT && opcode !== OP_SNAPSHOT) return null;
    const view = new DataView(bytes.buffer, bytes.byteOffset + 1, 8);
    return {
      opcode,
      // Sequences count bytes ever written; Number is exact to 2^53, which
      // is petabytes of terminal output.
      seq: Number(view.getBigUint64(0)),
      payload: bytes.subarray(9),
      snapshot: opcode === OP_SNAPSHOT,
    };
  }

  /* Exponential with a ceiling, so a hub that is down for an hour is polled
   * every few seconds rather than every few milliseconds. */
  function backoffDelay(attempt, base, cap) {
    const step = base === undefined ? 250 : base;
    const ceiling = cap === undefined ? 8000 : cap;
    if (attempt <= 0) return 0;
    return Math.min(ceiling, step * 2 ** (attempt - 1));
  }

  return {
    signal,
    STRINGS,
    pickLocale,
    strings,
    TOKEN_KEY,
    parseToken,
    loadToken,
    forgetToken,
    EXIT_GRACE_MS,
    createSessionStore,
    compareRows,
    describeSession,
    formatAge,
    elidePath,
    formatExit,
    RUNGS,
    cycleRung,
    FIT_MIN_FONT,
    FIT_MAX_FONT,
    fitGrid,
    OP_OUTPUT,
    OP_SNAPSHOT,
    decodeFrame,
    backoffDelay,
  };
});
