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
      copy: 'Copy',
      copied: 'Copied',
      deniedTitle: 'This link has expired',
      deniedHow: 'Get a fresh one with',
      deniedCommand: 'alc sessions',
      send: 'Send',
      composerHint: 'Prompt',
      viewer: 'view only',
      operator: 'can type',
      back: 'Back to sessions',
      hideSessions: 'Hide sessions',
      showSessions: 'Show sessions',
      sessionsRegion: 'Sessions',
      terminalRegion: 'Terminal',
      exited: 'exited',
      live: 'live',
      connecting: 'connecting',
      reconnecting: 'reconnecting',
      ended: 'ended',
      signalled: 'killed by',
      viewers: 'watching',
      loadFailed: 'Could not reach alc.',
      denied: 'This link is no longer valid — open a fresh one from `alc sessions`.',
      unsandboxed: 'no permission model',
      permission: 'Permission',
      permUnknown: 'not set by alc',
      permCycle: 'Cycle',
      permOrder: 'Order: {o}',
      permOrderShow: 'Show the cycle order',
      permCycleTip: 'Move to the next mode, as ⇧Tab does in the terminal.',
      permFixedShort: 'fixed at launch',
      permUnverifiedShort: 'unverified',
      permSent: 'Sent',
      permPicker: 'The agent opened its own picker — finish it in the terminal.',
      permRelaunch: 'Only selectable at launch:',
      permConfirm: 'Run this on the machine running the agent, then pick again.',
      permFixed: 'This agent keeps the mode it was launched with.',
      permDenied: 'This link can watch but not change the mode.',
      permUnsupported: 'Not available:',
      permUnverified: 'alc has not checked this agent’s flags against an installed copy.',
      usage: 'Usage',
      accounts: 'Accounts',
      byAgent: 'By provider and agent',
      usageFailed: 'Could not load usage.',
      usageEmpty: 'No accounts to show',
      usageEmptyHow: 'Add a provider with',
      usageEmptyCommand: 'alc config',
      left: '{p}% left',
      resetsIn: 'resets in {t}',
      hubNote: 'Found through this machine’s environment. Set codex_home or claude_config_dir on the profile to pin an account.',
      provider: 'Provider',
      agent: 'Agent',
      launches: 'Launches',
      turns: 'Turns',
      input: 'Input',
      cached: 'Cached',
      cacheShare: 'Cache %',
      output: 'Output',
      ledgerEmpty: 'No launches recorded yet.',
      directOnly: 'Tokens are counted only where alc carries the traffic; cache totals may also be unknown when an older hub produced the report.',
      files: 'Files',
      showFiles: 'Show files',
      hideFiles: 'Hide files',
      filesUp: 'Up one folder',
      filesEmpty: 'This folder is empty.',
      filesPick: 'Pick a file to read it here.',
      filesBinary: 'Binary file — not shown.',
      filesTruncated: 'Only the first {s} are shown.',
      filesCut: 'Only the first 1,000 entries are listed.',
      filesFailed: 'Could not open that.',
      filesDenied: 'This link can watch but not open files.',
      filesReload: 'Reload',
      filesClose: 'Close file',
      filesFilter: 'Filter',
      noMatch: 'Nothing matches.',
    },
    'zh-TW': {
      empty: '目前沒有 session',
      emptyHow: '用這個指令開一個',
      emptyCommand: 'alc share claude',
      copy: '複製',
      copied: '已複製',
      deniedTitle: '這個連結已失效',
      deniedHow: '用這個指令取得新的',
      deniedCommand: 'alc sessions',
      send: '送出',
      composerHint: '提示詞',
      viewer: '唯讀',
      operator: '可輸入',
      back: '回到 session 清單',
      hideSessions: '隱藏 session 清單',
      showSessions: '顯示 session 清單',
      sessionsRegion: 'Session 清單',
      terminalRegion: '終端機',
      exited: '已結束',
      live: '已連線',
      connecting: '連線中…',
      reconnecting: '重新連線中…',
      ended: '已斷線',
      signalled: '被中止：',
      viewers: '人在看',
      loadFailed: '無法連上 alc。',
      denied: '這個連結已失效 —— 請用 `alc sessions` 取得新的。',
      unsandboxed: '無權限模式',
      permission: '權限',
      permUnknown: 'alc 未設定',
      permCycle: '循環',
      permOrder: '順序：{o}',
      permOrderShow: '顯示循環順序',
      permCycleTip: '切到下一個模式，等同在終端機按 ⇧Tab。',
      permFixedShort: '啟動時決定',
      permUnverifiedShort: '未確認',
      permSent: '已送出',
      permPicker: 'agent 開啟了自己的選單 —— 請在終端機裡完成。',
      permRelaunch: '這個模式只能在啟動時選擇：',
      permConfirm: '請在跑 agent 的那台機器上執行，然後再選一次。',
      permFixed: '這個 agent 只能用啟動時的模式。',
      permDenied: '這個連結只能觀看，不能更改模式。',
      permUnsupported: '不支援：',
      permUnverified: 'alc 尚未對照已安裝的版本確認這個 agent 的旗標。',
      usage: '用量',
      accounts: '帳號',
      byAgent: '各 provider 與 agent',
      usageFailed: '無法載入用量。',
      usageEmpty: '沒有可顯示的帳號',
      usageEmptyHow: '用這個指令新增 provider',
      usageEmptyCommand: 'alc config',
      left: '剩 {p}%',
      resetsIn: '{t} 後重置',
      hubNote: '這些帳號來自本機的環境變數。在 profile 設定 codex_home 或 claude_config_dir 可固定帳號。',
      provider: 'Provider',
      agent: 'Agent',
      launches: '啟動',
      turns: '回合',
      input: '輸入',
      cached: '快取',
      cacheShare: '快取 %',
      output: '輸出',
      ledgerEmpty: '尚未記錄任何啟動。',
      directOnly: '只有經 alc 轉送流量的 session 才會計算 token；舊版 hub 產生的報告也可能沒有快取總數。',
      files: '檔案',
      showFiles: '顯示檔案',
      hideFiles: '隱藏檔案',
      filesUp: '上一層資料夾',
      filesEmpty: '這個資料夾是空的。',
      filesPick: '選一個檔案在這裡閱讀。',
      filesBinary: '二進位檔案 —— 不顯示。',
      filesTruncated: '只顯示前 {s}。',
      filesCut: '只列出前 1,000 個項目。',
      filesFailed: '無法開啟。',
      filesDenied: '這個連結只能觀看，不能開啟檔案。',
      filesReload: '重新載入',
      filesClose: '關閉檔案',
      filesFilter: '篩選',
      noMatch: '沒有符合的項目。',
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

  /* ---------------------------------------------------------------- rail */

  /* Whether this viewer has folded the session rail away on a wide screen.
   *
   * localStorage, which the token above deliberately avoids: a fold is a
   * layout preference, not a grant, and it follows the screen - which is
   * still the same screen tomorrow. Only the folded state is ever written,
   * and unfolding removes the key, so the default leaves nothing behind.
   * Anything but that exact value - a first visit, a foreign value, storage
   * that throws or is not there at all - reads as the rail shown, which is
   * the page as it always was. */
  const RAIL_KEY = 'alc.rail';

  function loadRailFolded(storage) {
    try {
      return storage.getItem(RAIL_KEY) === 'folded';
    } catch {
      return false;
    }
  }

  function saveRailFolded(storage, folded) {
    try {
      if (folded) storage.setItem(RAIL_KEY, 'folded');
      else storage.removeItem(RAIL_KEY);
    } catch {
      // Private mode, storage disabled, or none at all. The fold still holds
      // for this page load; only remembering it is lost.
    }
  }

  /* The file panel, remembered the same way the rail's fold is: per viewer,
   * written only when pressed. */
  const FILES_KEY = 'alc.files';

  function loadFilesOpen(storage) {
    try {
      return storage.getItem(FILES_KEY) === 'open';
    } catch {
      return false;
    }
  }

  function saveFilesOpen(storage, open) {
    try {
      if (open) storage.setItem(FILES_KEY, 'open');
      else storage.removeItem(FILES_KEY);
    } catch {
      // Remembering is lost; the panel still opens for this page load.
    }
  }

  /* A file size the way a person reads one. */
  function formatBytes(bytes) {
    const n = Number(bytes);
    if (!Number.isFinite(n) || n < 0) return '';
    if (n < 1024) return `${n} B`;
    const units = ['KB', 'MB', 'GB'];
    let value = n / 1024;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit += 1;
    }
    return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
  }

  /* A panel path is relative to the session's folder and `/`-separated; the
   * empty string is the folder itself. */
  function joinPath(folder, name) {
    const base = String(folder || '').replace(/\/+$/, '');
    return base ? `${base}/${name}` : String(name);
  }

  function parentPath(path) {
    const parts = String(path || '').split('/').filter(Boolean);
    parts.pop();
    return parts.join('/');
  }

  /* The breadcrumb: one crumb per folder, each knowing the path it opens. The
   * first is the session's folder itself, named after its last segment. */
  function crumbs(rootPath, path) {
    const rootName = String(rootPath || '').split('/').filter(Boolean).pop() || '/';
    const out = [{ label: rootName, path: '' }];
    let walked = '';
    for (const part of String(path || '').split('/').filter(Boolean)) {
      walked = joinPath(walked, part);
      out.push({ label: part, path: walked });
    }
    return out;
  }

  /* Rows whose text contains every word of the query, case-insensitively. */
  function matchesQuery(haystack, query) {
    const words = String(query || '').toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return true;
    const text = String(haystack || '').toLowerCase();
    return words.every((word) => text.includes(word));
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
    // session's size is this window's, so resizing it resizes the agent -
    // where a plain session's belongs to the terminal that launched it and
    // this page only scales what it is given.
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

  /* What the permission bar should show, as data rather than as a sentence.
   *
   * The bar used to own one free-text span, and nine different messages took
   * turns in it. Two unrelated kinds of fact were competing for that turn:
   * properties of the agent, true for as long as the session lives, and the
   * outcome of a click, true for a moment. They overwrote each other, which
   * is why the resync grew a save-and-restore around the note and why the bar
   * itself had to scroll sideways.
   *
   * Each field below is one class of fact, so the renderer can give every one
   * a place of its own instead of a turn in the same one. Nothing here is
   * translated: the renderer owns the words, this owns the decision.
   */
  function permissionView(caps, card) {
    const session = card || {};
    const state = session.permission || {};
    const set = (caps && caps.set) || {};
    const modes = (caps && caps.modes) || [];

    const view = {
      control: 'dead',
      options: [],
      order: '',
      reason: '',
      deadKind: '',
      rung: state.rung || '',
      native: state.native || '',
      /* This agent gates nothing at all. A property of the agent, not of the
       * mode, and the one fact here that is about safety rather than about
       * what a control can do. */
      ungated: !!session.unsandboxed,
      /* alc never checked this agent's flags against an installed copy, so it
       * injected no permission flag at launch. About the whole agent. */
      unverified: !!caps && !caps.flags_verified,
      /* alc sent a command and cannot see whether the agent took it. About
       * this session's current mode, which is why it is not the same doubt as
       * `unverified` and must not share a marker with it. */
      assumed: state.confidence === 'assumed',
    };

    /* `modes` says what the agent has; `set.kind` says whether alc can put it
     * into one. Both have to hold, and `relaunch-only` fails the second: the
     * mode is fixed for the life of the process. That is a property of the
     * whole agent rather than of any one mode, so every option would fail -
     * which is why the control is dead here rather than partly disabled.
     * Deciding it now is the point: the bar used to offer the choice, let the
     * POST round-trip, and only then write a sentence saying it was never
     * available. */
    if (!caps || !modes.length || set.kind === 'unsupported' || set.kind === 'relaunch-only') {
      view.deadKind = set.kind === 'relaunch-only' ? 'relaunch-only' : 'unsupported';
      // Server-authored and shown verbatim; it is the only account of why the
      // control is dead, so it is never summarised away.
      view.reason = set.reason || '';
      return view;
    }

    /* Relative movement only, so there is no dropdown to land on a chosen
     * mode. The order stays visible text: it is the useful half, and a phone
     * has nowhere to hover. */
    if (set.kind === 'cycle') {
      view.control = 'cycle';
      view.order = set.order || '';
      return view;
    }

    view.control = 'pick';
    view.options = modes.map((mode) => ({
      value: mode.rung,
      // alc's rung and the agent's own word, together, always: a shared label
      // misleads, because `auto` is the loosest setting one agent has and a
      // middling one for another.
      label: `${mode.rung} · ${mode.label}`,
      selected: mode.rung === state.rung,
    }));
    return view;
  }

  /* --------------------------------------------------------------- usage */

  /* The same threshold `alc usage` paints amber at, so the terminal and the
   * page never disagree about whether a window is worth worrying about. */
  const WARN_PERCENT = 75;

  /* `{p}% left` with the number in it. The strings keep their placeholders so
   * a translation can put the number where its own grammar wants it. */
  function fill(template, values) {
    return String(template).replace(/\{(\w+)\}/g, (whole, key) =>
      values[key] === undefined || values[key] === null ? '' : String(values[key])
    );
  }

  function clampPercent(value) {
    const number = Number(value);
    if (!Number.isFinite(number)) return 0;
    return Math.min(100, Math.max(0, number));
  }

  /* Green, amber, red - the meter's only three states. */
  function quotaLevel(usedPercent) {
    const used = clampPercent(usedPercent);
    if (used >= 100) return 'danger';
    if (used >= WARN_PERCENT) return 'warn';
    return 'ok';
  }

  /* Coarse, and never negative: a window whose reset has passed while the
   * page sat open should read as due, not as a negative countdown. */
  function formatCountdown(seconds) {
    const left = Math.max(0, Math.floor(Number(seconds) || 0));
    if (left < 60) return `${left}s`;
    const minutes = Math.floor(left / 60);
    if (minutes < 60) return `${minutes}m`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h ${minutes % 60}m`;
    return `${Math.floor(hours / 24)}d ${hours % 24}h`;
  }

  /* The magnitude is the information; the digits are not. */
  function compactCount(value) {
    const count = Math.max(0, Math.floor(Number(value) || 0));
    if (count < 1000) return String(count);
    const trim = (text) => text.replace('.0', '');
    if (count < 1e6) return trim(`${(count / 1000).toFixed(1)}K`);
    return trim(`${(count / 1e6).toFixed(1)}M`);
  }

  /* One meter's worth of a window: how much is left, how alarmed to look,
   * and when it comes back. */
  function describeWindow(window, nowSeconds, t) {
    const used = clampPercent(window && window.used_percent);
    const name = window && window.scope ? `${window.scope} ${window.name}` : (window && window.name) || '';
    const resetsAt = window && window.resets_at;
    return {
      key: `${name}`,
      label: name,
      leftPercent: Math.round(100 - used),
      level: quotaLevel(used),
      left: fill(t.left, { p: Math.round(100 - used) }),
      resets: resetsAt ? fill(t.resetsIn, { t: formatCountdown(resetsAt - nowSeconds) }) : '',
    };
  }

  /* What an account is, as chips. A per-model window nobody has touched is
   * left out for the same reason `alc usage` leaves it out of its table: a
   * row of "100% left" is not worth the space.
   *
   * The kind is dropped when it is already the heading. Every starter profile
   * `impl Default for Config` writes is named after its own kind, so for
   * anyone who never renamed one the card read "codex" twice on adjacent
   * lines - a chip that told you only what you had just finished reading. */
  function describeAccount(account) {
    const kind = account.kind === account.profile ? null : account.kind;
    return [kind, account.plan].filter(Boolean);
  }

  function accountWindows(account) {
    return (account.windows || []).filter(
      (window) => !window.scope || clampPercent(window.used_percent) > 0
    );
  }

  /* The sentence a row shows instead of meters, if it has one. */
  function accountNote(account) {
    if (account.balance) {
      const money = (value) =>
        account.balance.unit === 'USD' ? `$${Number(value).toFixed(2)}` : `${Number(value).toFixed(2)} ${account.balance.unit}`.trim();
      const { remaining, limit } = account.balance;
      if (remaining !== null && remaining !== undefined && limit) {
        return `${money(remaining)} / ${money(limit)}`;
      }
      if (remaining !== null && remaining !== undefined) return money(remaining);
    }
    return account.error || '';
  }

  /* Whatever the server sent, as something the renderer can iterate.
   *
   * The page is served by the same binary that builds the report, but it can
   * also be looking at a hub that is mid-restart or a build that is one
   * version behind, so nothing here assumes a field exists. */
  function normalizeUsage(payload) {
    const report = payload && typeof payload === 'object' ? payload : {};
    const accounts = Array.isArray(report.accounts) ? report.accounts : [];
    return {
      accounts,
      rows: Array.isArray(report.ledger && report.ledger.rows) ? report.ledger.rows : [],
      generatedAt: Number(report.generated_at) || 0,
      /* Only worth saying when the hub resolved an account from its own
       * environment, which belongs to whichever shell started it. */
      hubEnv:
        report.resolved_by === 'hub' &&
        accounts.some((account) => account.source === 'env' || account.source === 'default'),
    };
  }

  /* One ledger row's cells. A pair alc never carried traffic for shows a dash
   * rather than a zero: the tokens are unknown, not nil. Older hubs did not
   * report cache totals either, so those rows use the same honest unknown. */
  function ledgerCells(row) {
    const carried = Number(row.turns) > 0;
    const count = (value) => {
      const numeric = Number(value);
      return carried && Number.isFinite(numeric) && numeric >= 0 ? compactCount(numeric) : '—';
    };
    const input = Number(row.input_tokens);
    const cacheKnown =
      carried && typeof row.cached_tokens === 'number' && Number.isFinite(row.cached_tokens) &&
      Number.isFinite(input) && input >= 0 && row.cached_tokens >= 0 && row.cached_tokens <= input;
    const cached = Number(row.cached_tokens) || 0;
    const share = cacheKnown && input > 0 ? `${Math.round((cached * 100) / input)}%` : '—';
    return [
      row.provider,
      row.agent,
      String(row.launches || 0),
      count(row.turns),
      count(row.input_tokens),
      cacheKnown ? compactCount(cached) : '—',
      share,
      count(row.output_tokens),
    ];
  }

  /* Whether the table below needs its caveat.
   *
   * A footnote under a column of numbers is the right control for a
   * measurement caveat - a dash says "no value", and no glyph can say "because
   * alc did not carry this traffic". But it only earns its place when there is
   * a dash to explain. Standing under a table whose numbers are all real, it
   * reads as a doubt about them. */
  function ledgerNeedsCaveat(rows) {
    return (Array.isArray(rows) ? rows : []).some((row) => {
      if (!(Number(row.turns) > 0)) return true;
      const input = Number(row.input_tokens);
      return typeof row.cached_tokens !== 'number' || !Number.isFinite(row.cached_tokens) ||
        !Number.isFinite(input) || input < 0 || row.cached_tokens < 0 || row.cached_tokens > input;
    });
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
   * `scale` is the safety net, and it describes the grid AS IT IS now, not
   * as it will be at `fontSize`: whatever does not fit the frame at the
   * current cell. The caller applies it once the font size has settled, so
   * a fit that ran out of passes still cannot crop the agent's screen - it
   * only ends up transformed, which is the lesser of the two. It is 1
   * whenever the grid already fits, which is the ordinary case. Where the
   * grid then sits is not decided here: the frame centres it. */
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
      return { fontSize, scale: 1 };
    }

    // The smaller ratio wins, so the grid is never cropped.
    const want = Math.min(room / wide, tall / high);
    const stepped = Math.floor((fontSize * want) / FIT_STEP) * FIT_STEP;
    const next = Math.min(max, Math.max(min, Math.round(stepped * 10) / 10));
    return { fontSize: next, scale: Math.min(1, want) };
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

  /* ------------------------------------------------------- page strings */

  /* The tabs, settings and dashboard arrived after the strings above, and
   * are kept apart so a translator can see what is new. */
  Object.assign(STRINGS.en, {
    navSessions: 'Sessions',
    navUsage: 'Usage',
    navSettings: 'Settings',
    navLabel: 'Pages',
    palette: 'Command palette',
    paletteHint: 'Type a command or a session name…',
    paletteEmpty: 'No matching commands.',
    paletteGo: 'Go to',
    paletteOpen: 'Open session',
    paletteTheme: 'Theme',
    paletteGroupNav: 'Navigate',
    paletteGroupSessions: 'Sessions',
    paletteGroupTheme: 'Appearance',
    paletteGroupMods: 'Mods',
    paletteGroupView: 'View',
    paletteToggleFiles: 'Toggle file panel',
    paletteToggleRail: 'Toggle session list',
    paletteReloadMods: 'Reload mods',
    paletteDensity: 'Toggle compact density',
    rangeLabel: 'Range',
    range7: '7 days',
    range30: '30 days',
    range90: '90 days',
    statTokens: 'Tokens',
    statCost: 'Estimated cost',
    statOutput: 'Output',
    statCache: 'Cache hits',
    statSessions: 'Sessions',
    statActiveDays: '{n} active days',
    statApprox: 'lower bound',
    statOfInput: 'of input from cache',
    statRecords: '{n} records',
    chartDaily: 'Daily usage',
    chartTokens: 'Tokens',
    chartCost: 'Cost',
    chartHours: 'When you work',
    chartHoursNote: 'Tokens by hour of day, {z}',
    seriesUncached: 'Input',
    seriesCacheRead: 'Cache read',
    seriesCacheWrite: 'Cache write',
    seriesOutput: 'Output',
    topModels: 'Models',
    byAgentShort: 'Agents',
    byProvider: 'Providers',
    historyEmpty: 'No token records in this range yet.',
    historyFailed: 'Could not load usage history.',
    pricedWith: 'Prices from {s}. Estimates exclude taxes and discounts.',
    settingsTitle: 'Settings',
    settingsViewer: 'View only — these are the operator’s settings.',
    settingsSaved: 'Saved',
    settingsSaving: 'Saving…',
    settingsFailed: 'Could not save settings.',
    settingsFileError: 'settings.json has an error, so the defaults are in use:',
    appearance: 'Appearance',
    theme: 'Theme',
    themeSystem: 'System',
    themeLight: 'Light',
    themeDark: 'Dark',
    themeCustom: 'from themes/',
    accent: 'Accent',
    accentDefault: 'Theme default',
    accentCustom: 'Custom colour',
    density: 'Density',
    densityComfortable: 'Comfortable',
    densityCompact: 'Compact',
    terminal: 'Terminal',
    fontSize: 'Font size',
    fontAuto: 'Auto',
    fontFamily: 'Font',
    fontSystem: 'System monospace',
    lineHeight: 'Line height',
    cursorStyle: 'Cursor',
    cursorBlock: 'Block',
    cursorBar: 'Bar',
    cursorUnderline: 'Underline',
    cursorBlink: 'Blinking cursor',
    scrollback: 'Scrollback',
    scrollbackLines: '{n} lines',
    preview: 'Preview',
    mods: 'Mods',
    modsNone: 'No mods installed yet.',
    modsFolder: 'Folder',
    modsAsk: 'Ask your agent',
    modsPrompt: 'Read {d}/AGENTS.md, then make me an alc page mod that ',
    modOn: 'On',
    modOff: 'Off',
    modError: 'Error',
    modFailed: 'Mod {m} failed: {e}',
    modsChanged: 'Mods changed on disk.',
    reload: 'Reload',
    about: 'About',
    aboutVersion: 'alc version',
    aboutCeiling: 'Permission ceiling',
    aboutLink: 'This link',
    aboutLanguage: 'Language',
    aboutSessions: 'Sessions on this hub',
    aboutShortcuts: 'Shortcuts',
    shortcutPalette: 'Command palette',
    shortcutSend: 'Send prompt',
    sizeFramed: 'Drawn at {c}×{r}, its own terminal’s size. To fill this screen, start it with --tmux:',
    sizeHide: 'Hide',
  });

  Object.assign(STRINGS['zh-TW'], {
    navSessions: 'Session',
    navUsage: '用量',
    navSettings: '設定',
    navLabel: '頁面',
    palette: '指令面板',
    paletteHint: '輸入指令或 session 名稱…',
    paletteEmpty: '沒有符合的指令。',
    paletteGo: '前往',
    paletteOpen: '開啟 session',
    paletteTheme: '主題',
    paletteGroupNav: '導覽',
    paletteGroupSessions: 'Session',
    paletteGroupTheme: '外觀',
    paletteGroupMods: 'Mods',
    paletteGroupView: '檢視',
    paletteToggleFiles: '切換檔案面板',
    paletteToggleRail: '切換 session 清單',
    paletteReloadMods: '重新載入 mods',
    paletteDensity: '切換緊湊密度',
    rangeLabel: '範圍',
    range7: '7 天',
    range30: '30 天',
    range90: '90 天',
    statTokens: 'Token',
    statCost: '估計費用',
    statOutput: '輸出',
    statCache: '快取命中',
    statSessions: 'Session',
    statActiveDays: '{n} 天有使用',
    statApprox: '下限值',
    statOfInput: '的輸入來自快取',
    statRecords: '{n} 筆紀錄',
    chartDaily: '每日用量',
    chartTokens: 'Token',
    chartCost: '費用',
    chartHours: '工作時段',
    chartHoursNote: '各時段的 token 數，{z}',
    seriesUncached: '輸入',
    seriesCacheRead: '快取讀取',
    seriesCacheWrite: '快取寫入',
    seriesOutput: '輸出',
    topModels: '模型',
    byAgentShort: 'Agent',
    byProvider: 'Provider',
    historyEmpty: '這段期間還沒有 token 紀錄。',
    historyFailed: '無法載入用量歷史。',
    pricedWith: '價格來自 {s}。估計值不含稅與折扣。',
    settingsTitle: '設定',
    settingsViewer: '唯讀：這些是操作者的設定。',
    settingsSaved: '已儲存',
    settingsSaving: '儲存中…',
    settingsFailed: '無法儲存設定。',
    settingsFileError: 'settings.json 有錯誤，目前使用預設值：',
    appearance: '外觀',
    theme: '主題',
    themeSystem: '跟隨系統',
    themeLight: '淺色',
    themeDark: '深色',
    themeCustom: '來自 themes/',
    accent: '強調色',
    accentDefault: '主題預設',
    accentCustom: '自訂顏色',
    density: '密度',
    densityComfortable: '舒適',
    densityCompact: '緊湊',
    terminal: '終端機',
    fontSize: '字級',
    fontAuto: '自動',
    fontFamily: '字型',
    fontSystem: '系統等寬字型',
    lineHeight: '行高',
    cursorStyle: '游標',
    cursorBlock: '方塊',
    cursorBar: '直線',
    cursorUnderline: '底線',
    cursorBlink: '游標閃爍',
    scrollback: '捲動緩衝',
    scrollbackLines: '{n} 行',
    preview: '預覽',
    mods: 'Mods',
    modsNone: '還沒有安裝任何 mod。',
    modsFolder: '資料夾',
    modsAsk: '請 agent 幫忙',
    modsPrompt: '閱讀 {d}/AGENTS.md，然後幫我做一個 alc 頁面 mod，功能是',
    modOn: '開',
    modOff: '關',
    modError: '錯誤',
    modFailed: 'Mod {m} 執行失敗：{e}',
    modsChanged: '磁碟上的 mods 有變更。',
    reload: '重新載入',
    about: '關於',
    aboutVersion: 'alc 版本',
    aboutCeiling: '權限上限',
    aboutLink: '這個連結',
    aboutLanguage: '語言',
    aboutSessions: '這個 hub 上的 session',
    aboutShortcuts: '快捷鍵',
    shortcutPalette: '指令面板',
    shortcutSend: '送出提示詞',
    sizeFramed: '以 {c}×{r} 顯示，也就是它自己終端機的大小。要填滿畫面，請用 --tmux 啟動：',
    sizeHide: '隱藏',
  });

  /* ------------------------------------------------------------- themes */

  /* Built-in themes, as overrides on the light or dark base palette that
   * app.css defines. A file in themes/ has exactly this shape, so the
   * built-ins double as the examples. */
  const THEMES = {
    midnight: {
      name: 'Midnight',
      mode: 'dark',
      tokens: {
        '--bg': '#070a14',
        '--surface': '#0d1222',
        '--raised': '#141a2e',
        '--line': '#1b2236',
        '--line-strong': '#2a3350',
        '--text': '#e6e9f5',
        '--muted': '#97a0bd',
        '--accent': '#7c9cff',
        '--chart-1': '#7c9cff',
        '--chart-2': '#3b4a8f',
        '--chart-3': '#c084fc',
        '--chart-4': '#34d399',
      },
      terminal: { background: '#060913', cursor: '#7c9cff' },
    },
    daylight: {
      name: 'Daylight',
      mode: 'light',
      tokens: {
        '--bg': '#f4f6fb',
        '--surface': '#ffffff',
        '--raised': '#eef1f7',
        '--accent': '#2563eb',
        '--chart-1': '#2563eb',
        '--chart-2': '#93c5fd',
        '--chart-3': '#a78bfa',
        '--chart-4': '#059669',
      },
      terminal: {},
    },
    fjord: {
      name: 'Fjord',
      mode: 'dark',
      tokens: {
        '--bg': '#242933',
        '--surface': '#2e3440',
        '--raised': '#3b4252',
        '--line': '#3b4252',
        '--line-strong': '#4c566a',
        '--text': '#eceff4',
        '--muted': '#a7b1c2',
        '--accent': '#88c0d0',
        '--ok': '#a3be8c',
        '--warn': '#ebcb8b',
        '--danger': '#bf616a',
        '--term-bg': '#2e3440',
        '--chart-1': '#88c0d0',
        '--chart-2': '#5e81ac',
        '--chart-3': '#b48ead',
        '--chart-4': '#a3be8c',
      },
      terminal: {
        background: '#2e3440',
        foreground: '#d8dee9',
        cursor: '#88c0d0',
        cursorAccent: '#2e3440',
        black: '#3b4252',
        red: '#bf616a',
        green: '#a3be8c',
        yellow: '#ebcb8b',
        blue: '#81a1c1',
        magenta: '#b48ead',
        cyan: '#88c0d0',
        white: '#e5e9f0',
        brightBlack: '#616e88',
        brightRed: '#d08770',
        brightGreen: '#b5d19c',
        brightYellow: '#f0d9a6',
        brightBlue: '#8fbcbb',
        brightMagenta: '#c9a7c3',
        brightCyan: '#a3d6e2',
        brightWhite: '#eceff4',
      },
    },
    ember: {
      name: 'Ember',
      mode: 'dark',
      tokens: {
        '--bg': '#0f0d0c',
        '--surface': '#181412',
        '--raised': '#221c19',
        '--line': '#2a2320',
        '--line-strong': '#3d332e',
        '--text': '#f3ece7',
        '--muted': '#b3a59c',
        '--accent': '#fb923c',
        '--chart-1': '#fb923c',
        '--chart-2': '#9a3412',
        '--chart-3': '#facc15',
        '--chart-4': '#f472b6',
      },
      terminal: { background: '#0c0a09', cursor: '#fb923c' },
    },
    paper: {
      name: 'Paper',
      mode: 'light',
      tokens: {
        '--bg': '#f6f3ec',
        '--surface': '#fffdf8',
        '--raised': '#efeadf',
        '--line': '#e5dfd2',
        '--line-strong': '#d3cab8',
        '--text': '#24201a',
        '--muted': '#6b6253',
        '--accent': '#b45309',
        '--chart-1': '#b45309',
        '--chart-2': '#e7c08f',
        '--chart-3': '#7c3aed',
        '--chart-4': '#047857',
      },
      terminal: {},
    },
  };

  const ACCENTS = ['#4f46e5', '#2563eb', '#0891b2', '#059669', '#65a30d', '#d97706', '#e11d48', '#c026d3'];

  /* The studio, widgets, windows, backgrounds, panels and fitting. */
  Object.assign(STRINGS.en, {
    navStudio: 'Studio',
    modEnableTitle: 'Turn on {m}?',
    modEnableBody: 'It was installed from {s}. Once on, it runs in this page with its access to your sessions, and it is not sandboxed.',
    modEnableConfirm: 'Turn on',
    modRemoveTitle: 'Remove {m}?',
    modRemoveBody: 'Its folder and its saved state are deleted. A layout that names it will be missing it.',
    studioLayouts: 'Layouts',
    studioWidgets: 'Widgets',
    layoutDefault: 'Default',
    layoutDefaultDesc: 'alc as it ships: your own theme, no layout mods.',
    layoutActive: 'Active',
    layoutApply: 'Apply',
    layoutApplied: 'Applied {l}.',
    layoutDefaultApplied: 'Back to the default layout.',
    layoutMissing: 'Applied, but these mods are not installed: {m}',
    layoutModsOne: '{n} mod',
    layoutModsMany: '{n} mods',
    layoutCount: '{n} layouts',
    layoutLegacy: 'Read from the packages/ folder alc 2.8 used.',
    layoutDownload: 'Download JSON',
    layoutCopy: 'Copy JSON',
    layoutRemoveTitle: 'Remove {l}?',
    layoutRemoveBody: 'Its file is deleted. Its mods stay installed.',
    layoutRemoveActive: 'It is the active layout, so the page goes back to Default first. Its mods stay installed.',
    layoutReviewTitle: 'Turn on mods for {l}?',
    layoutReviewBody: 'This layout needs mods that were installed from elsewhere and are off. Once on, they run in this page with its access to your sessions, and they are not sandboxed.',
    layoutReviewFrom: 'from {s}',
    layoutReviewWhere: 'Read them in {d}.',
    layoutReviewConfirm: 'Turn on and apply',
    layoutNew: 'New layout',
    layoutFromPage: 'This page',
    layoutFromJson: 'JSON',
    layoutFromGithub: 'GitHub',
    layoutCreate: 'Create',
    layoutId: 'Id',
    layoutName: 'Name',
    layoutDesc: 'Description',
    layoutJson: 'Layout JSON',
    layoutSaved: 'Saved layouts/{l}.json',
    layoutChip: 'layout',
    layoutCovers: 'The {l} layout draws the terminal with its own value. Yours is kept and comes back when you leave it.',
    installSourceTip: 'owner/repo, owner/repo@v1 or a github.com URL.',
    paletteGroupLayouts: 'Layouts',
    paletteLayout: 'Layout',
    more: 'More',
    cancel: 'Cancel',
    studioViewer: 'View only — this is the operator’s page.',
    studioOpen: 'Open the studio',
    studioMods: 'Mods',
    studioBackground: 'Background',
    studioPanels: 'Panels',
    chooseFile: 'Choose file…',
    badId: 'An id is lowercase letters, digits, - and _, up to 48.',
    badJson: 'That is not valid JSON.',
    official: 'Official',
    install: 'Install',
    reinstall: 'Reinstall',
    remove: 'Remove',
    reset: 'Reset',
    library: 'Widget library',
    libraryEmpty: 'No widgets yet — turn on a mod that has some.',
    inspector: 'Widget',
    inspectorEmpty: 'Pick a widget to place it and set its options.',
    inspectorSlot: 'Where',
    slotHidden: 'Hidden',
    slotHide: 'Hide',
    placeFirst: 'Place it to set this.',
    noOptions: 'This widget has no options.',
    layoutReset: 'Reset layout',
    layoutResetConfirm: 'Put every widget back where its mod suggests?',
    screenList: 'Session list',
    screenView: 'Coding view',
    screenFloat: 'Floating windows',
    zoneEmpty: 'Drop a widget here',
    slotNotAllowed: '{w} cannot go in {s}.',
    slot_bar: 'Header',
    slot_list_top: 'Above the sessions',
    slot_list_bottom: 'Below the sessions',
    slot_card: 'On each session card',
    slot_view_top: 'Above the terminal',
    slot_view_bottom: 'Below the terminal',
    slot_view_left: 'Left dock',
    slot_view_right: 'Right dock',
    slot_float: 'Floating window',
    installGithub: 'Install from GitHub',
    installSource: 'GitHub repository',
    installWarning: 'Mods arrive off; read them before you turn them on.',
    installedFrom: 'Installed: {w}',
    newMod: 'New mod',
    newModId: 'Mod id',
    newModName: 'Name',
    newModNameHint: 'Name (optional)',
    newModTemplate: 'Template',
    newModCreate: 'Create',
    newModCreated: 'Created {d}. Edit mod.js, then reload.',
    newModReload: 'Mod created. Reload to run it.',
    newModPrompt: 'Read the AGENTS.md next to {d}, then change this mod so that ',
    newModCli: 'Same from a shell: alc remote web new <id> --template widget',
    bgPresets: 'Built in',
    bgNone: 'None',
    bgImages: 'Your images',
    bgImagesHelp: 'PNG, JPEG, WebP or GIF up to 1 MiB from here; larger files can go in web/backgrounds/ directly.',
    bgUpload: 'Upload',
    bgImagesNone: 'No images uploaded yet.',
    bgRemoveConfirm: 'Delete {n}?',
    bgTooBig: 'That image is over 1 MiB; put it in web/backgrounds/ on the hub instead.',
    bgMods: 'From mods',
    bgModsNone: 'No mod draws a background yet.',
    bgTuning: 'Tuning',
    bgDim: 'Dim',
    bgBlur: 'Blur',
    bgTerminal: 'Terminal opacity',
    bg_aurora: 'Aurora',
    bg_dusk: 'Dusk',
    bg_mesh: 'Mesh',
    bg_grid: 'Grid',
    bg_dots: 'Dots',
    bg_blueprint: 'Blueprint',
    bg_sunrise: 'Sunrise',
    bg_forest: 'Forest',
    panelWidths: 'Widths',
    panelSides: 'Sides',
    panel_rail: 'Session list',
    panel_files: 'File panel',
    panel_left: 'Left dock',
    panel_right: 'Right dock',
    panelResize: 'Resize {p}',
    panelResizeHint: 'Drag to resize · double-click to reset',
    sideLeft: 'Left',
    sideRight: 'Right',
    windowsReset: 'Floating windows',
    windowsResetButton: 'Reset positions',
    windowsResetHelp: 'Puts every window back and shows the closed ones (this device only).',
    winClose: 'Close window',
    winTray: 'Windows',
    winShown: 'shown',
    winHidden: 'hidden',
    winShow: 'Show window',
    winHide: 'Hide window',
    paletteGroupWindows: 'Windows',
    fitTake: 'Fit to this page',
    fitRelease: 'Give the size back',
    fitTaken: 'The terminal that started this session was resized, so it has its size back.',
    fitTakenPage: 'Another page took this session’s size, so this one shows it framed.',
    fitPlain: 'Fit plain sessions to this page',
    fitPlainLabel: 'Plain sessions',
    fitPlainHelp: 'A session started without --tmux is drawn at its own terminal’s size. This lets the page resize it instead, until that terminal is resized again.',
    sizeFramedFit: 'Drawn at {c}×{r}, its own terminal’s size.',
  });

  Object.assign(STRINGS['zh-TW'], {
    navStudio: '工作室',
    modEnableTitle: '要開啟「{m}」嗎？',
    modEnableBody: '它是從 {s} 安裝的。開啟後，它會在這個頁面裡執行，擁有這個頁面對你所有 session 的權限，而且沒有沙箱。',
    modEnableConfirm: '開啟',
    modRemoveTitle: '要移除「{m}」嗎？',
    modRemoveBody: '會刪除它的資料夾與儲存的狀態。用到它的版面會少了它。',
    studioLayouts: '版面',
    studioWidgets: '小工具',
    layoutDefault: '預設',
    layoutDefaultDesc: 'alc 原本的樣子：你自己的主題，不開任何版面 mod。',
    layoutActive: '使用中',
    layoutApply: '套用',
    layoutApplied: '已套用「{l}」。',
    layoutDefaultApplied: '已回到預設版面。',
    layoutMissing: '已套用，但這些 mod 尚未安裝：{m}',
    layoutModsOne: '{n} 個 mod',
    layoutModsMany: '{n} 個 mod',
    layoutCount: '{n} 個版面',
    layoutLegacy: '從 alc 2.8 使用的 packages/ 資料夾讀取。',
    layoutDownload: '下載 JSON',
    layoutCopy: '複製 JSON',
    layoutRemoveTitle: '要移除「{l}」嗎？',
    layoutRemoveBody: '會刪除它的檔案，它的 mod 會保留。',
    layoutRemoveActive: '這是使用中的版面，所以會先回到預設。它的 mod 會保留。',
    layoutReviewTitle: '要為「{l}」開啟 mod 嗎？',
    layoutReviewBody: '這個版面需要從其他來源安裝、目前關閉的 mod。開啟後，它們會在這個頁面裡執行，擁有這個頁面對你所有 session 的權限，而且沒有沙箱。',
    layoutReviewFrom: '來自 {s}',
    layoutReviewWhere: '請先在 {d} 讀過。',
    layoutReviewConfirm: '開啟並套用',
    layoutNew: '新增版面',
    layoutFromPage: '目前頁面',
    layoutFromJson: 'JSON',
    layoutFromGithub: 'GitHub',
    layoutCreate: '建立',
    layoutId: 'id',
    layoutName: '名稱',
    layoutDesc: '說明',
    layoutJson: '版面 JSON',
    layoutSaved: '已儲存 layouts/{l}.json',
    layoutChip: '版面',
    layoutCovers: '「{l}」版面用它自己的值畫終端機。你的設定會保留，離開版面後就會恢復。',
    installSourceTip: 'owner/repo、owner/repo@v1 或 github.com 網址。',
    paletteGroupLayouts: '版面',
    paletteLayout: '版面',
    more: '更多',
    cancel: '取消',
    studioViewer: '唯讀：這是操作者的頁面。',
    studioOpen: '開啟工作室',
    studioMods: 'Mods',
    studioBackground: '背景',
    studioPanels: '面板',
    chooseFile: '選擇檔案…',
    badId: 'id 只能用小寫字母、數字、- 與 _，最多 48 字元。',
    badJson: '這不是有效的 JSON。',
    official: '官方',
    install: '安裝',
    reinstall: '重新安裝',
    remove: '移除',
    reset: '重設',
    library: '小工具庫',
    libraryEmpty: '還沒有小工具，先開啟一個提供小工具的 mod。',
    inspector: '小工具',
    inspectorEmpty: '選一個小工具來放置並設定選項。',
    inspectorSlot: '位置',
    slotHidden: '隱藏',
    slotHide: '隱藏',
    placeFirst: '放置後才能設定。',
    noOptions: '這個小工具沒有選項。',
    layoutReset: '重設版面',
    layoutResetConfirm: '把每個小工具放回它的 mod 建議的位置？',
    screenList: 'Session 清單',
    screenView: '編碼畫面',
    screenFloat: '浮動視窗',
    zoneEmpty: '把小工具拖到這裡',
    slotNotAllowed: '{w} 不能放在{s}。',
    slot_bar: '頁首',
    slot_list_top: 'Session 上方',
    slot_list_bottom: 'Session 下方',
    slot_card: '每張 session 卡片',
    slot_view_top: '終端機上方',
    slot_view_bottom: '終端機下方',
    slot_view_left: '左側面板',
    slot_view_right: '右側面板',
    slot_float: '浮動視窗',
    installGithub: '從 GitHub 安裝',
    installSource: 'GitHub repo',
    installWarning: '裝好的 mod 預設關閉，開啟前先讀過。',
    installedFrom: '已安裝：{w}',
    newMod: '新 mod',
    newModId: 'Mod id',
    newModName: '名稱',
    newModNameHint: '名稱（選填）',
    newModTemplate: '範本',
    newModCreate: '建立',
    newModCreated: '已建立 {d}。編輯 mod.js 後重新載入。',
    newModReload: '已建立 mod。重新載入以執行。',
    newModPrompt: '讀 {d} 旁邊的 AGENTS.md，然後修改這個 mod，讓它',
    newModCli: '在 shell 中也可以：alc remote web new <id> --template widget',
    bgPresets: '內建',
    bgNone: '無',
    bgImages: '你的圖片',
    bgImagesHelp: '可從這裡上傳 1 MiB 以內的 PNG、JPEG、WebP 或 GIF；更大的檔案可直接放到 web/backgrounds/。',
    bgUpload: '上傳',
    bgImagesNone: '還沒有上傳圖片。',
    bgRemoveConfirm: '刪除 {n}？',
    bgTooBig: '這張圖片超過 1 MiB，請直接放到 hub 的 web/backgrounds/。',
    bgMods: '來自 mods',
    bgModsNone: '還沒有 mod 提供背景。',
    bgTuning: '微調',
    bgDim: '變暗',
    bgBlur: '模糊',
    bgTerminal: '終端機不透明度',
    bg_aurora: '極光',
    bg_dusk: '黃昏',
    bg_mesh: '漸層網',
    bg_grid: '格線',
    bg_dots: '點陣',
    bg_blueprint: '藍圖',
    bg_sunrise: '日出',
    bg_forest: '森林',
    panelWidths: '寬度',
    panelSides: '位置',
    panel_rail: 'Session 清單',
    panel_files: '檔案面板',
    panel_left: '左側面板',
    panel_right: '右側面板',
    panelResize: '調整{p}寬度',
    panelResizeHint: '拖曳調整 · 雙擊重設',
    sideLeft: '左',
    sideRight: '右',
    windowsReset: '浮動視窗',
    windowsResetButton: '重設位置',
    windowsResetHelp: '把每個視窗放回原位並顯示已關閉的視窗（僅限這台裝置）。',
    winClose: '關閉視窗',
    winTray: '視窗',
    winShown: '顯示中',
    winHidden: '已隱藏',
    winShow: '顯示視窗',
    winHide: '隱藏視窗',
    paletteGroupWindows: '視窗',
    fitTake: '符合這個頁面',
    fitRelease: '交還大小',
    fitTaken: '啟動這個 session 的終端機調整了大小，所以大小已交還給它。',
    fitTakenPage: '另一個頁面取得了這個 session 的大小，所以這裡改以原尺寸顯示。',
    fitPlain: '讓一般 session 符合這個頁面',
    fitPlainLabel: '一般 session',
    fitPlainHelp: '沒有用 --tmux 啟動的 session 會以它自己終端機的大小顯示。開啟後由頁面決定大小，直到那個終端機再次調整大小。',
    sizeFramedFit: '以 {c}×{r} 顯示，也就是它自己終端機的大小。',
  });

  /* The settings the page draws from, with every field present. Whatever the
   * server sent is trusted for shape only after this. */
  function normalizeWebSettings(raw) {
    const input = raw && typeof raw === 'object' ? raw : {};
    const terminal = input.terminal && typeof input.terminal === 'object' ? input.terminal : {};
    const size = Number(terminal.fontSize);
    const line = Number(terminal.lineHeight);
    const scroll = Number(terminal.scrollback);
    return {
      theme: typeof input.theme === 'string' && input.theme ? input.theme : 'system',
      accent: typeof input.accent === 'string' && /^#[0-9a-f]{6}$/i.test(input.accent) ? input.accent : null,
      density: input.density === 'compact' ? 'compact' : 'comfortable',
      terminal: {
        fontSize: Number.isFinite(size) && size > 0 ? Math.min(28, Math.max(8, Math.round(size))) : null,
        fontFamily: typeof terminal.fontFamily === 'string' && terminal.fontFamily.trim() ? terminal.fontFamily.trim() : null,
        lineHeight: Number.isFinite(line) ? Math.min(2, Math.max(1, line)) : 1,
        cursorStyle: ['block', 'bar', 'underline'].includes(terminal.cursorStyle) ? terminal.cursorStyle : 'block',
        cursorBlink: terminal.cursorBlink !== false,
        scrollback: Number.isFinite(scroll) ? Math.min(100000, Math.max(500, Math.round(scroll))) : 5000,
        fitPlain: terminal.fitPlain === true,
      },
      disabledMods: Array.isArray(input.disabledMods) ? input.disabledMods.filter((id) => typeof id === 'string') : [],
      modSettings: input.modSettings && typeof input.modSettings === 'object' ? input.modSettings : {},
      layout: normalizeLayout(input.layout),
      background: normalizeBackground(input.background),
      // `package` is what 2.8 called the active layout.
      activeLayout: [input.activeLayout, input.package].find((id) => typeof id === 'string' && SLUG.test(id)) || null,
      layoutMods: Array.isArray(input.layoutMods) ? input.layoutMods.filter((id) => typeof id === 'string' && SLUG.test(id)) : [],
      layoutTerminal: normalizeTerminalOverride(input.layoutTerminal),
      // The person's own look from before a layout; the hub's to use, the
      // page only carries it so a settings save does not drop it.
      layoutRestore: input.layoutRestore && typeof input.layoutRestore === 'object' ? input.layoutRestore : null,
    };
  }

  /* A layout's terminal settings: only the fields it sets, clamped like the
   * person's own, or null when it sets none. */
  function normalizeTerminalOverride(raw) {
    if (!raw || typeof raw !== 'object') return null;
    const out = {};
    const size = Number(raw.fontSize);
    if (raw.fontSize != null && Number.isFinite(size) && size > 0) out.fontSize = Math.min(28, Math.max(8, Math.round(size)));
    if (typeof raw.fontFamily === 'string' && raw.fontFamily.trim()) out.fontFamily = raw.fontFamily.trim();
    const line = Number(raw.lineHeight);
    if (raw.lineHeight != null && Number.isFinite(line)) out.lineHeight = Math.min(2, Math.max(1, line));
    if (['block', 'bar', 'underline'].includes(raw.cursorStyle)) out.cursorStyle = raw.cursorStyle;
    if (typeof raw.cursorBlink === 'boolean') out.cursorBlink = raw.cursorBlink;
    return Object.keys(out).length ? out : null;
  }

  /* What the terminal is drawn with: the person's settings with the active
   * layout's laid over them. Leaving the layout gives theirs back. */
  function effectiveTerminal(settings) {
    const own = (settings && settings.terminal) || {};
    const layer = (settings && settings.layoutTerminal) || {};
    return { ...own, ...layer };
  }

  /* Which of the person's terminal settings a layout is covering, so the
   * settings page can say so beside each one. */
  function terminalOverrides(settings) {
    return Object.keys((settings && settings.layoutTerminal) || {});
  }

  /* One gallery of layouts: the installed ones, then the official ones not
   * installed yet, each once. `active` marks the one in use. */
  function layoutGallery(installed, catalog, active) {
    const seen = new Set();
    const cards = [];
    for (const info of installed || []) {
      if (!info || !SLUG.test(String(info.id)) || seen.has(info.id)) continue;
      seen.add(info.id);
      // Official is what alc installed (the server compares the file with the
      // one it ships), never a matching id: a repo can ship cockpit.json.
      cards.push({ ...info, installed: true, official: info.official === true, active: info.id === active });
    }
    for (const entry of catalog || []) {
      if (!entry || seen.has(entry.id)) continue;
      seen.add(entry.id);
      const { layout: def, ...rest } = entry;
      cards.push({ ...(def || {}), ...rest, mods: (def && def.mods) || [], installed: false, official: true, active: false });
    }
    return cards;
  }

  const BASE_TOKENS = {
    light: { '--bg': '#f6f7f9', '--surface': '#ffffff', '--raised': '#f0f1f4', '--line': '#e4e6eb', '--text': '#15171c', '--muted': '#5a606c', '--accent': '#4f46e5' },
    dark: { '--bg': '#0b0c0f', '--surface': '#131418', '--raised': '#1b1d22', '--line': '#24262c', '--text': '#e8e9ed', '--muted': '#9a9fab', '--accent': '#818cf8' },
  };

  /* Where a mod came from, short enough for a list row: a GitHub source
   * as it is, a local folder by its last name only (the full path, which
   * names the machine's user, stays for tooltips and the review dialog). */
  function shortSource(source) {
    const text = String(source || '').trim();
    if (!/^(\/|~\/|[A-Za-z]:[\\/]|\\\\)/.test(text)) return text;
    const parts = text.split(/[\\/]+/).filter(Boolean);
    return parts.length ? `…/${parts[parts.length - 1]}` : text;
  }

  /* What a layout's thumbnail draws: its theme's colours, which side the
   * rail is on, which docks hold widgets and what kind of background it
   * has. Pure, so the studio's SVG is only a picture of this. */
  function layoutThumb(info, customThemes, systemDark) {
    const def = info || {};
    const settings = { theme: def.theme || 'system', accent: def.accent || null };
    const theme = resolveTheme(settings, customThemes, !!systemDark);
    const colour = (key) => {
      const value = theme.tokens[key] || BASE_TOKENS[theme.mode][key];
      // Thumbnails draw plain colours; a token that is an expression falls
      // back to the base palette.
      return /^#[0-9a-f]{3,8}$/i.test(String(value)) ? value : BASE_TOKENS[theme.mode][key];
    };
    const layout = normalizeLayout(def.layout);
    const has = (slot) => !!(layout.slots[slot] && layout.slots[slot].length);
    const background = normalizeBackground(def.background);
    return {
      mode: theme.mode,
      bg: colour('--bg'),
      surface: colour('--surface'),
      raised: colour('--raised'),
      line: colour('--line'),
      text: colour('--text'),
      muted: colour('--muted'),
      accent: colour('--accent'),
      terminal: /^#[0-9a-f]{6}$/i.test(String(theme.terminal.background || '')) ? theme.terminal.background : theme.mode === 'light' ? '#1d1f24' : '#0c0d10',
      railSide: layout.railSide,
      docks: { bar: has('bar'), top: has('view.top'), bottom: has('view.bottom'), left: has('view.left'), right: has('view.right'), float: has('float'), list: has('list.top') || has('list.bottom') },
      background: background.kind,
      dense: def.density === 'compact',
    };
  }

  /* ------------------------------------------------------------- layout */

  const SLUG = /^[a-z0-9][a-z0-9_-]{0,47}$/i;
  const SLOTS = ['bar', 'list.top', 'list.bottom', 'card', 'view.top', 'view.bottom', 'view.left', 'view.right', 'float'];
  const PANEL_SIZES = { rail: [220, 640], files: [260, 1200], left: [180, 900], right: [180, 900] };
  const PANEL_DEFAULTS = { rail: 320, files: 420, left: 300, right: 320 };
  const BACKGROUND_PRESETS = ['aurora', 'dusk', 'mesh', 'grid', 'dots', 'blueprint', 'sunrise', 'forest'];

  function isWidgetKey(value) {
    if (typeof value !== 'string') return false;
    const parts = value.split('/');
    return parts.length === 2 && SLUG.test(parts[0]) && SLUG.test(parts[1]);
  }

  /* The same shape and the same limits the hub's `Layout::clamp` keeps. */
  function normalizeLayout(raw) {
    const input = raw && typeof raw === 'object' ? raw : {};
    const slots = {};
    if (input.slots && typeof input.slots === 'object') {
      for (const slot of SLOTS) {
        const list = Array.isArray(input.slots[slot]) ? input.slots[slot] : [];
        const kept = list
          .filter((entry) => entry && isWidgetKey(entry.widget))
          .slice(0, 24)
          .map((entry) => ({
            widget: entry.widget,
            options: entry.options && typeof entry.options === 'object' && !Array.isArray(entry.options) ? entry.options : {},
          }));
        if (kept.length) slots[slot] = kept;
      }
    }
    const sizes = {};
    if (input.sizes && typeof input.sizes === 'object') {
      for (const [key, [min, max]] of Object.entries(PANEL_SIZES)) {
        const value = Number(input.sizes[key]);
        if (Number.isFinite(value) && value > 0) sizes[key] = Math.min(max, Math.max(min, Math.round(value)));
      }
    }
    return {
      slots,
      hidden: Array.isArray(input.hidden) ? [...new Set(input.hidden.filter(isWidgetKey))].sort() : [],
      sizes,
      filesSide: input.filesSide === 'left' ? 'left' : 'right',
      railSide: input.railSide === 'right' ? 'right' : 'left',
      explicit: input.explicit === true,
    };
  }

  function normalizeBackground(raw) {
    const input = raw && typeof raw === 'object' ? raw : {};
    const kind = ['preset', 'image', 'mod'].includes(input.kind) ? input.kind : 'none';
    const value = typeof input.value === 'string' ? input.value : '';
    const valid = (kind === 'preset' && BACKGROUND_PRESETS.includes(value))
      || (kind === 'image' && /^[a-z0-9][a-z0-9_-]{0,47}\.(png|jpe?g|webp|gif)$/i.test(value))
      || (kind === 'mod' && isWidgetKey(value));
    const num = (key, min, max, fallback) => {
      const n = Number(input[key]);
      return Number.isFinite(n) ? Math.min(max, Math.max(min, Math.round(n))) : fallback;
    };
    return {
      kind: valid ? kind : 'none',
      value: valid ? value : '',
      dim: num('dim', 0, 95, 35),
      blur: num('blur', 0, 40, 0),
      terminal: num('terminal', 30, 100, 100),
    };
  }

  /* Which widgets a slot draws, in order: the ones the layout puts there,
   * then any widget the layout neither places nor hides whose mod suggests
   * this slot - so turning a mod on shows its widget without a trip to the
   * studio. `widgets` is `{key: {slots, defaultSlot, options}}`; a placement
   * naming a widget that is not loaded, or a slot it does not allow, is
   * skipped rather than drawn empty. */
  function placementsFor(layout, slot, widgets) {
    const known = widgets || {};
    const allowed = (key) => known[key] && (!known[key].slots || known[key].slots.includes(slot));
    const placed = new Set();
    for (const list of Object.values(layout.slots || {})) {
      for (const entry of list) placed.add(entry.widget);
    }
    const hidden = new Set(layout.hidden || []);
    const out = (layout.slots && layout.slots[slot] ? layout.slots[slot] : [])
      .filter((entry) => allowed(entry.widget))
      .map((entry) => ({ widget: entry.widget, options: withDefaults(known[entry.widget], entry.options) }));
    if (layout.explicit) return out;
    for (const [key, widget] of Object.entries(known)) {
      if (placed.has(key) || hidden.has(key) || widget.defaultSlot !== slot || !allowed(key)) continue;
      out.push({ widget: key, options: withDefaults(widget, {}) });
    }
    return out;
  }

  function withDefaults(widget, options) {
    const out = {};
    for (const option of (widget && widget.options) || []) {
      if (option && option.key !== undefined && option.default !== undefined) out[option.key] = option.default;
    }
    return { ...out, ...(options || {}) };
  }

  /* Moves a widget to `slot` at `index` (end when omitted), taking it out of
   * wherever it was; `slot` null hides it. Returns a new layout. */
  function placeWidget(layout, key, slot, index, options) {
    const next = normalizeLayout(JSON.parse(JSON.stringify(layout)));
    // With an explicit layout a hidden widget is simply one not placed.
    if (!slot && next.explicit) {
      for (const name of Object.keys(next.slots)) {
        next.slots[name] = next.slots[name].filter((entry) => entry.widget !== key);
        if (!next.slots[name].length) delete next.slots[name];
      }
      return next;
    }
    let kept = options;
    for (const name of Object.keys(next.slots)) {
      const found = next.slots[name].find((entry) => entry.widget === key);
      if (found && kept === undefined) kept = found.options;
      next.slots[name] = next.slots[name].filter((entry) => entry.widget !== key);
      if (!next.slots[name].length) delete next.slots[name];
    }
    next.hidden = next.hidden.filter((entry) => entry !== key);
    if (!slot) {
      next.hidden = [...next.hidden, key].sort();
      return next;
    }
    const list = next.slots[slot] ? next.slots[slot] : [];
    const at = Number.isInteger(index) ? Math.max(0, Math.min(list.length, index)) : list.length;
    list.splice(at, 0, { widget: key, options: kept || {} });
    next.slots[slot] = list;
    return next;
  }

  /* The layout as drawn, written out: every slot's widgets including the
   * ones drawn by default, marked explicit. What a layout saves, so it
   * reproduces the page exactly wherever it is applied. */
  function materializeLayout(layout, widgets) {
    const out = normalizeLayout(JSON.parse(JSON.stringify(layout)));
    out.slots = {};
    for (const slot of SLOTS) {
      const list = placementsFor(layout, slot, widgets);
      if (list.length) out.slots[slot] = list;
    }
    out.hidden = [];
    out.explicit = true;
    return out;
  }

  /* `#rrggbb` (or `#rgb`) at an alpha, as `rgba()`. Null for anything else. */
  function hexToRgba(hex, alpha) {
    let text = String(hex || '').trim();
    if (/^#[0-9a-f]{3}$/i.test(text)) text = `#${[...text.slice(1)].map((c) => c + c).join('')}`;
    const match = /^#([0-9a-f]{6})$/i.exec(text);
    if (!match) return null;
    const value = parseInt(match[1], 16);
    const a = Math.round(Math.min(1, Math.max(0, alpha)) * 100) / 100;
    return `rgba(${(value >> 16) & 255},${(value >> 8) & 255},${value & 255},${a})`;
  }

  /* Keeps a floating window on screen: at least `grip` pixels of its title
   * bar stay reachable whatever the viewport did since it was placed. */
  function clampWindow(rect, viewport, grip) {
    const g = grip || 48;
    const width = Math.min(Math.max(220, rect.width || 360), Math.max(220, viewport.width - 16));
    const height = Math.min(Math.max(120, rect.height || 260), Math.max(120, viewport.height - 16));
    const x = Math.min(Math.max(rect.x || 0, g - width), viewport.width - g);
    const y = Math.min(Math.max(rect.y || 0, 0), viewport.height - 40);
    return { x: Math.round(x), y: Math.round(y), width: Math.round(width), height: Math.round(height) };
  }

  /* Which palette is in effect: the theme's mode decides the base, then its
   * tokens, then the accent on top. `custom` is the list from themes/. */
  function resolveTheme(settings, custom, systemDark) {
    const id = settings.theme;
    let theme = null;
    if (id === 'light' || id === 'dark') theme = { id, name: id, mode: id, tokens: {}, terminal: {} };
    else if (THEMES[id]) theme = { id, ...THEMES[id] };
    else {
      const found = (custom || []).find((entry) => entry.id === id && !entry.error);
      if (found) theme = { id, name: found.name, mode: found.mode === 'light' ? 'light' : 'dark', tokens: found.tokens || {}, terminal: found.terminal || {} };
    }
    if (!theme) theme = { id: 'system', name: 'system', mode: systemDark ? 'dark' : 'light', tokens: {}, terminal: {} };
    const tokens = { ...theme.tokens };
    if (settings.accent) tokens['--accent'] = settings.accent;
    if (tokens['--accent']) {
      if (!theme.tokens['--accent-ink'] || settings.accent) tokens['--accent-ink'] = inkFor(tokens['--accent']);
      if (!theme.tokens['--accent-soft'] || settings.accent) tokens['--accent-soft'] = `color-mix(in srgb, ${tokens['--accent']} 12%, transparent)`;
      if (!theme.tokens['--accent-line'] || settings.accent) tokens['--accent-line'] = `color-mix(in srgb, ${tokens['--accent']} 45%, transparent)`;
      tokens['--ring'] = `0 0 0 3px color-mix(in srgb, ${tokens['--accent']} 28%, transparent)`;
      if (!tokens['--chart-1']) tokens['--chart-1'] = tokens['--accent'];
    }
    return { id: theme.id, name: theme.name, mode: theme.mode, tokens, terminal: { ...theme.terminal } };
  }

  /* Black or white text on a colour, whichever reads better (WCAG). */
  function inkFor(hex) {
    const match = /^#([0-9a-f]{6})$/i.exec(String(hex || ''));
    if (!match) return '#ffffff';
    const value = parseInt(match[1], 16);
    const channel = (shift) => {
      const c = ((value >> shift) & 255) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    };
    const luminance = 0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0);
    return (luminance + 0.05) / 0.05 > 1.05 / (luminance + 0.05) ? '#0b0c10' : '#ffffff';
  }

  /* Whether a framed grid is drawn too small to read comfortably: shrunk
   * below the page's own font, or past 9px. */
  function framedTooSmall(fontSize, baseFontSize, scale) {
    return scale < 1 || fontSize < Math.min(9, baseFontSize - 2);
  }

  /* The command that starts the same agent so this page owns its size. */
  function tmuxCommand(card) {
    return `alc share --tmux ${(card && card.agent) || 'claude'}`;
  }

  /* ----------------------------------------------------------- dashboard */

  /* `YYYY-MM-DD` plus `n` days, computed on the calendar, never the clock. */
  function shiftDate(date, n) {
    const [y, m, d] = String(date).split('-').map(Number);
    const when = new Date(Date.UTC(y, m - 1, d + n));
    return when.toISOString().slice(0, 10);
  }

  /* Today in a timezone, as `YYYY-MM-DD`. */
  function todayIn(zone, now) {
    try {
      return new Intl.DateTimeFormat('en-CA', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit' })
        .format(now || new Date());
    } catch {
      return (now || new Date()).toISOString().slice(0, 10);
    }
  }

  /* One entry per day of the range, oldest first, with the days the server
   * had nothing for filled as zeros: a gap on a chart is a day off, not a
   * missing bar. */
  function fillDays(daily, days, today) {
    const byDate = new Map((daily || []).map((day) => [day.date, day]));
    const out = [];
    for (let back = days - 1; back >= 0; back -= 1) {
      const date = shiftDate(today, -back);
      const day = byDate.get(date) || {};
      const uncached = Number(day.uncached) || 0;
      const cacheRead = Number(day.cacheRead) || 0;
      const cacheWrite = Number(day.cacheWrite) || 0;
      const output = Number(day.output) || 0;
      out.push({
        date,
        uncached,
        cacheRead,
        cacheWrite,
        output,
        total: uncached + cacheRead + cacheWrite + output,
        usd: Number(day.usd) || 0,
        requests: Number(day.requests) || 0,
      });
    }
    return out;
  }

  /* An axis maximum and its ticks on a 1-2-5 ladder, so the grid lines land
   * on numbers a person would write. */
  function niceScale(max, count) {
    const want = count || 4;
    if (!(max > 0)) return { max: 1, ticks: [0, 1] };
    const raw = max / want;
    const power = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 2.5, 5, 10].map((m) => m * power).find((s) => s >= raw) || power * 10;
    const top = Math.ceil(max / step) * step;
    const ticks = [];
    for (let value = 0; value <= top + step / 2; value += step) ticks.push(Number(value.toPrecision(12)));
    return { max: top, ticks };
  }

  function formatUsd(value) {
    const amount = Number(value) || 0;
    if (amount === 0) return '$0';
    if (amount < 0.01) return '<$0.01';
    if (amount < 1000) return `$${amount.toFixed(2)}`;
    return `$${compactCount(Math.round(amount))}`;
  }

  /* Larger magnitudes than compactCount, for token totals in the billions. */
  function compactTokens(value) {
    const count = Math.max(0, Number(value) || 0);
    if (count >= 1e9) return `${(count / 1e9).toFixed(count >= 1e10 ? 0 : 1).replace('.0', '')}B`;
    return compactCount(count);
  }

  /* What share of input came from cache, as a whole percent, or null. */
  function cacheShare(totals) {
    const t = totals || {};
    const input = (Number(t.uncached) || 0) + (Number(t.cacheRead) || 0) + (Number(t.cacheWrite) || 0);
    if (!input) return null;
    return Math.round(((Number(t.cacheRead) || 0) / input) * 100);
  }

  /* Bars for a ranked list, each as a share of the largest. */
  function shareBars(list, key) {
    const field = key || 'tokens';
    const top = Math.max(0, ...(list || []).map((item) => Number(item[field]) || 0));
    return (list || []).map((item) => ({ ...item, share: top ? (Number(item[field]) || 0) / top : 0 }));
  }

  /* -------------------------------------------------------------- palette */

  /* Commands ranked against a query: a title that starts with it first, then
   * a word in it that does, then anything containing every word. */
  function rankCommands(commands, query) {
    const q = String(query || '').trim().toLowerCase();
    if (!q) return commands.slice();
    const scored = [];
    commands.forEach((command, index) => {
      const title = String(command.title || '').toLowerCase();
      const text = `${title} ${String(command.hint || '').toLowerCase()} ${String(command.group || '').toLowerCase()}`;
      if (!matchesQuery(text, q)) return;
      let score = 3;
      if (title.startsWith(q)) score = 0;
      else if (title.split(/[\s/:·-]+/).some((word) => word.startsWith(q))) score = 1;
      else if (title.includes(q)) score = 2;
      scored.push({ command, score, index });
    });
    scored.sort((a, b) => a.score - b.score || a.index - b.index);
    return scored.map((entry) => entry.command);
  }

  /* ----------------------------------------------------------------- mods */

  /* What a mod's chip function returned, as `{text, tone}` or null. */
  function normalizeChip(value) {
    if (value === null || value === undefined || value === false || value === '') return null;
    if (typeof value === 'string' || typeof value === 'number') return { text: String(value), tone: '' };
    if (typeof value === 'object' && value.text) {
      const tone = ['ok', 'warn', 'danger', 'accent'].includes(value.tone) ? value.tone : '';
      return { text: String(value.text), tone };
    }
    return null;
  }

  /* Which mods' scripts differ between two /api/web answers. */
  function changedScripts(before, after) {
    const key = (mods) => new Map((mods || []).filter((m) => m.enabled && m.hasJs).map((m) => [m.id, m.scriptHash || '']));
    const a = key(before);
    const b = key(after);
    const changed = [];
    for (const [id, hash] of b) if (a.get(id) !== hash) changed.push(id);
    for (const id of a.keys()) if (!b.has(id)) changed.push(id);
    return changed.sort();
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
    RAIL_KEY,
    loadRailFolded,
    saveRailFolded,
    loadFilesOpen,
    saveFilesOpen,
    formatBytes,
    joinPath,
    parentPath,
    crumbs,
    matchesQuery,
    createSessionStore,
    describeSession,
    formatAge,
    elidePath,
    formatExit,
    RUNGS,
    cycleRung,
    permissionView,
    WARN_PERCENT,
    fill,
    clampPercent,
    quotaLevel,
    formatCountdown,
    compactCount,
    describeWindow,
    describeAccount,
    accountWindows,
    accountNote,
    normalizeUsage,
    ledgerCells,
    ledgerNeedsCaveat,
    FIT_MIN_FONT,
    FIT_MAX_FONT,
    fitGrid,
    OP_OUTPUT,
    OP_SNAPSHOT,
    decodeFrame,
    backoffDelay,
    THEMES,
    ACCENTS,
    normalizeWebSettings,
    normalizeTerminalOverride,
    effectiveTerminal,
    terminalOverrides,
    layoutGallery,
    layoutThumb,
    shortSource,
    resolveTheme,
    inkFor,
    shiftDate,
    todayIn,
    fillDays,
    niceScale,
    formatUsd,
    compactTokens,
    cacheShare,
    shareBars,
    rankCommands,
    normalizeChip,
    changedScripts,
    framedTooSmall,
    tmuxCommand,
    SLOTS,
    PANEL_SIZES,
    PANEL_DEFAULTS,
    BACKGROUND_PRESETS,
    isWidgetKey,
    normalizeLayout,
    normalizeBackground,
    placementsFor,
    placeWidget,
    materializeLayout,
    hexToRgba,
    clampWindow,
  };
});
