/* The remote-control page.
 *
 * No framework and no build step, for the same reason xterm.js is vendored:
 * this file is served out of the alc binary, and a toolchain between the
 * source and what ships is a toolchain someone has to keep working.
 *
 * The token arrives in the URL fragment, which browsers never send to a
 * server. It is read once, kept in a closure, and stripped from the address
 * bar immediately so it does not land in history, a screenshot, or a link
 * the user pastes to someone else. It is never written to localStorage:
 * this page is a remote shell, and a token that survives the tab is a token
 * that outlives the user's intent to grant access.
 */

(() => {
  'use strict';

  const OP_OUTPUT = 0x01;
  const OP_SNAPSHOT = 0x02;

  /* ---------------------------------------------------------------- i18n */

  const STRINGS = {
    en: {
      appName: 'alc',
      noSessions: 'No shared sessions. Start one with `alc <agent> --share`.',
      listHint: 'Sessions appear here while the alc process that started them is running.',
      send: 'Send',
      viewer: 'view only',
      operator: 'can type',
      connecting: 'Connecting…',
      reconnecting: 'Connection lost — reconnecting…',
      reconnected: 'Reconnected.',
      exited: 'The agent exited',
      exitedSignal: 'The agent was stopped by',
      readOnly: 'This link can watch but not type.',
      unsandboxed: 'no permission model',
      loadFailed: 'Could not load sessions.',
      denied: 'This link is no longer valid. Ask for a fresh one.',
      composerHint: 'Type a prompt here and send it as one block.',
      running: 'running',
      stopped: 'stopped',
      permission: 'Permission',
      permUnknown: 'not set by alc',
      permCycle: 'Cycle (Shift+Tab)',
      permCycleNote: 'This agent can only be cycled, not set directly.',
      permSent: 'Sent',
      permPicker: 'The agent opened its own picker — finish it in the terminal below.',
      permRelaunch: 'This mode can only be chosen at launch:',
      permConfirm: 'Run this on the machine running the agent, then try again:',
      permUnsupported: 'Not available:',
      permUnverified: 'alc has not confirmed this agent\u2019s flags against an installed copy.',
      unsandboxedNote: 'This agent gates nothing.',
    },
    'zh-TW': {
      appName: 'alc',
      noSessions: '目前沒有共享的 session。用 `alc <agent> --share` 開一個。',
      listHint: 'Session 會在啟動它的 alc 程序執行期間顯示於此。',
      send: '送出',
      viewer: '唯讀',
      operator: '可輸入',
      connecting: '連線中…',
      reconnecting: '連線中斷 — 重新連線中…',
      reconnected: '已重新連線。',
      exited: 'Agent 已結束',
      exitedSignal: 'Agent 被以下訊號中止：',
      readOnly: '這個連結只能觀看，不能輸入。',
      unsandboxed: '無權限模式',
      loadFailed: '無法載入 session 清單。',
      denied: '這個連結已失效，請重新取得。',
      composerHint: '在這裡輸入提示詞，整段送出。',
      running: '執行中',
      stopped: '已停止',
      permission: '權限',
      permUnknown: 'alc 未設定',
      permCycle: '循環切換 (Shift+Tab)',
      permCycleNote: '這個 agent 只能循環切換，無法直接指定。',
      permSent: '已送出',
      permPicker: 'agent 開啟了自己的選單 —— 請在下方終端機完成。',
      permRelaunch: '這個模式只能在啟動時選擇：',
      permConfirm: '請在跑 agent 的那台機器上執行，然後再試一次：',
      permUnsupported: '不支援：',
      permUnverified: 'alc 尚未對照已安裝的版本確認這個 agent 的旗標。',
      unsandboxedNote: '這個 agent 沒有任何權限控制。',
    },
  };

  const LOCALE = (navigator.languages || [navigator.language || 'en'])
    .map((tag) => (String(tag).toLowerCase().startsWith('zh') ? 'zh-TW' : 'en'))
    .find((tag) => STRINGS[tag]) || 'en';
  const T = STRINGS[LOCALE];
  document.documentElement.lang = LOCALE;

  for (const node of document.querySelectorAll('[data-i18n]')) {
    node.textContent = T[node.dataset.i18n] ?? node.textContent;
  }
  const compose = document.getElementById('compose');
  compose.placeholder = T.composerHint;

  /* --------------------------------------------------------------- token */

  const token = (() => {
    const match = /[#&]k=([A-Za-z0-9_-]+)/.exec(location.hash || '');
    if (!match) return null;
    // Strip it before anything can read location.hash again.
    history.replaceState(null, '', location.pathname + location.search);
    return match[1];
  })();

  /* ---------------------------------------------------------------- dom */

  const el = {
    bar: document.getElementById('bar'),
    back: document.getElementById('back'),
    subtitle: document.getElementById('subtitle'),
    grade: document.getElementById('grade'),
    list: document.getElementById('list'),
    sessions: document.getElementById('sessions'),
    empty: document.getElementById('empty'),
    view: document.getElementById('view'),
    terminal: document.getElementById('terminal'),
    keys: document.getElementById('keys'),
    perm: document.getElementById('perm'),
    permLabel: document.getElementById('permLabel'),
    permPick: document.getElementById('permPick'),
    permCycle: document.getElementById('permCycle'),
    permNote: document.getElementById('permNote'),
    composer: document.getElementById('composer'),
    send: document.getElementById('send'),
    toast: document.getElementById('toast'),
  };

  let toastTimer = 0;
  function toast(message, level) {
    el.toast.textContent = message;
    el.toast.className = level || '';
    el.toast.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      el.toast.hidden = true;
    }, level === 'error' ? 8000 : 3500);
  }

  /* --------------------------------------------------------------- list */

  async function api(path) {
    const response = await fetch(path, {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
      cache: 'no-store',
    });
    if (response.status === 401 || response.status === 403) {
      throw new Error('denied');
    }
    if (!response.ok) throw new Error(`http ${response.status}`);
    return response.json();
  }

  function describe(card) {
    const bits = [card.agent];
    if (card.provider_kind && card.provider_kind !== card.agent) bits.push(card.provider_kind);
    if (card.model) bits.push(card.model);
    if (card.effort) bits.push(card.effort);
    return bits.join(' · ');
  }

  function renderList(cards) {
    el.sessions.replaceChildren();
    el.empty.hidden = cards.length > 0;
    for (const card of cards) {
      const item = document.createElement('li');
      const button = document.createElement('button');
      button.className = 'card';
      button.type = 'button';

      const row = document.createElement('div');
      row.className = 'row';
      const dot = document.createElement('span');
      dot.className = card.state === 'running' ? 'dot' : 'dot exited';
      const name = document.createElement('span');
      name.className = 'name';
      name.textContent = card.name;
      row.append(dot, name);
      if (card.unsandboxed) {
        const badge = document.createElement('span');
        badge.className = 'badge';
        badge.textContent = T.unsandboxed;
        row.append(badge);
      }

      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = describe(card);
      const where = document.createElement('div');
      where.className = 'meta';
      where.textContent = card.cwd;

      button.append(row, meta, where);
      button.addEventListener('click', () => attach(card));
      item.append(button);
      el.sessions.append(item);
    }
  }

  async function refresh() {
    try {
      renderList(await api('/api/sessions'));
    } catch (error) {
      toast(error.message === 'denied' ? T.denied : T.loadFailed, 'error');
    }
  }

  /* --------------------------------------------------------- permission */

  let CAPS = null;
  let pendingTicket = null;

  async function loadCaps() {
    try {
      CAPS = await api('/api/caps');
    } catch {
      CAPS = null;
    }
  }

  function capsFor(agent) {
    return CAPS && CAPS.agents ? CAPS.agents.find((row) => row.agent === agent) : null;
  }

  function note(text, level) {
    el.permNote.textContent = text || '';
    el.permNote.className = `perm-note${level ? ` ${level}` : ''}`;
  }

  /* The control is a pure function of the agent's capabilities. An agent
   * that can only be cycled gets a relative button and never a dropdown,
   * because a dropdown would promise alc can land on a chosen mode - and
   * from Claude Code's `auto` the first press goes somewhere else. */
  function renderPermission(card) {
    const caps = capsFor(card.agent);
    el.perm.hidden = false;
    el.perm.classList.toggle('unsandboxed', !!card.unsandboxed);
    el.permCycle.hidden = true;
    el.permPick.hidden = false;
    el.permPick.disabled = false;
    el.permPick.replaceChildren();

    const state = card.permission || {};
    const shown = state.rung
      ? `${state.rung}${state.native ? ` · ${state.native}` : ''}${state.confidence === 'assumed' ? ' ?' : ''}`
      : T.permUnknown;
    el.permLabel.textContent = `${T.permission}: ${shown}`;

    if (!caps || !caps.modes.length) {
      el.permPick.disabled = true;
      const only = document.createElement('option');
      only.textContent = '—';
      el.permPick.append(only);
      const reason = caps && caps.set && caps.set.reason ? caps.set.reason : '';
      note(`${T.permUnsupported} ${reason}`, 'danger');
      return;
    }

    if (caps.set.kind === 'cycle') {
      el.permPick.hidden = true;
      el.permCycle.hidden = false;
      el.permCycle.textContent = T.permCycle;
      note(`${T.permCycleNote} ${caps.set.order || ''}`, 'warn');
      return;
    }

    const placeholder = document.createElement('option');
    placeholder.textContent = '…';
    placeholder.value = '';
    el.permPick.append(placeholder);
    for (const mode of caps.modes) {
      const option = document.createElement('option');
      option.value = mode.rung;
      // alc's rung and the agent's own word, together, always.
      option.textContent = `${mode.rung} · ${mode.label}`;
      if (mode.rung === state.rung) option.selected = true;
      el.permPick.append(option);
    }
    if (card.unsandboxed) note(T.unsandboxedNote, 'danger');
    else if (!caps.flags_verified) note(T.permUnverified, 'warn');
    else note('');
  }

  async function setPermission(rung, ticket) {
    if (!current) return;
    const response = await fetch(`/api/sessions/${current.id}/permission`, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(ticket ? { rung, ticket } : { rung }),
    });
    let body = null;
    try {
      body = await response.json();
    } catch {
      note(`HTTP ${response.status}`, 'danger');
      return;
    }
    switch (body.outcome) {
      case 'done':
        note(`${T.permSent}: ${body.rung}`);
        break;
      case 'sent':
        note(`${T.permSent}: ${body.bytes}`, 'warn');
        break;
      case 'picker-open':
        note(`${T.permPicker} (${body.command})`, 'warn');
        term.focus();
        break;
      case 'needs-relaunch':
        note(`${T.permRelaunch} ${(body.cli || []).join(' ')}`, 'warn');
        break;
      case 'needs-confirmation':
        note(`${T.permConfirm} alc confirm ${body.ticket}`, 'danger');
        pendingTicket = { ticket: body.ticket, rung: body.rung };
        break;
      case 'unsupported':
        note(`${T.permUnsupported} ${body.reason}`, 'danger');
        break;
      default:
        note('');
    }
  }


  el.permPick.addEventListener('change', () => {
    const rung = el.permPick.value;
    if (!rung) return;
    // A ticket is spent on the change it was minted for, and only that one.
    const ticket =
      pendingTicket && pendingTicket.rung === rung ? pendingTicket.ticket : undefined;
    pendingTicket = null;
    setPermission(rung, ticket);
  });

  el.permCycle.addEventListener('click', () => {
    // Relative movement: the rung asked for is ignored by the agent, so
    // send the current one and let the probe report where it landed.
    const rung = (current && current.permission && current.permission.rung) || 'ask';
    setPermission(rung);
  });

  /* ----------------------------------------------------------- terminal */

  let term = null;
  let fit = null;
  let socket = null;
  let current = null;
  let lastSeq = null;
  let retry = 0;
  let closing = false;

  const THEME = {
    background: '#16161a',
    foreground: '#eceae5',
    cursor: '#d98b5f',
    selectionBackground: 'rgba(217,139,95,0.32)',
  };

  function ensureTerminal() {
    if (term) return;
    term = new window.Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      convertEol: false,
      fontSize: window.matchMedia('(min-width: 900px)').matches ? 13 : 12,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      scrollback: 5000,
      theme: THEME,
    });
    fit = new window.FitAddon.FitAddon();
    term.loadAddon(fit);
    term.open(el.terminal);
    term.onData((data) => send({ t: 'input', data }));
    term.onBinary((data) => send({ t: 'input', data }));
  }

  function refit() {
    if (!fit || !term) return;
    try {
      fit.fit();
    } catch {
      return;
    }
    send({ t: 'resize', cols: term.cols, rows: term.rows });
  }

  function send(frame) {
    if (socket && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(frame));
    }
  }

  function connect(card) {
    const url = new URL('/ws', location.href);
    url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    url.searchParams.set('session', card.id);

    socket = new WebSocket(url);
    socket.binaryType = 'arraybuffer';

    socket.addEventListener('open', () => {
      retry = 0;
      send({
        t: 'auth',
        token: token || '',
        cols: term ? term.cols : 80,
        rows: term ? term.rows : 24,
        since: lastSeq,
      });
    });

    socket.addEventListener('message', (event) => {
      if (typeof event.data === 'string') return onControl(JSON.parse(event.data));
      onBinary(new Uint8Array(event.data));
    });

    socket.addEventListener('close', () => {
      if (closing) return;
      toast(T.reconnecting, 'warn');
      retry = Math.min(retry + 1, 6);
      setTimeout(() => {
        if (current) connect(current);
      }, 250 * 2 ** (retry - 1));
    });
  }

  function onBinary(frame) {
    if (frame.length < 9) return;
    const opcode = frame[0];
    const view = new DataView(frame.buffer, frame.byteOffset + 1, 8);
    const seq = Number(view.getBigUint64(0));
    const payload = frame.subarray(9);

    if (opcode === OP_SNAPSHOT) {
      term.reset();
      term.write(payload);
    } else if (opcode === OP_OUTPUT) {
      term.write(payload);
    } else {
      return;
    }
    if (lastSeq !== null && retry > 0) toast(T.reconnected);
    lastSeq = seq;
    retry = 0;
  }

  function onControl(frame) {
    switch (frame.t) {
      case 'hello': {
        current = frame.session;
        lastSeq = frame.seq;
        setGrade(frame.grade);
        el.subtitle.textContent = `${frame.session.name} · ${describe(frame.session)}`;
        renderPermission(frame.session);
        refit();
        break;
      }
      case 'notice':
        toast(frame.message, frame.level);
        break;
      case 'exit': {
        const how = frame.signal
          ? `${T.exitedSignal} ${frame.signal}`
          : `${T.exited} (${frame.code ?? 0})`;
        toast(how, 'warn');
        closing = true;
        break;
      }
      case 'ping':
        send({ t: 'pong' });
        break;
      default:
        break;
    }
  }

  function setGrade(grade) {
    const operator = grade === 'operator';
    document.body.classList.toggle('viewer', !operator);
    el.grade.hidden = false;
    el.grade.textContent = operator ? T.operator : T.viewer;
    el.grade.className = operator ? 'pill operator' : 'pill';
    if (term) term.options.disableStdin = !operator;
    if (!operator) toast(T.readOnly, 'warn');
  }

  function attach(card) {
    current = card;
    lastSeq = null;
    closing = false;
    el.list.hidden = true;
    el.view.hidden = false;
    el.back.hidden = false;
    ensureTerminal();
    buildKeys();
    requestAnimationFrame(() => {
      refit();
      term.focus();
    });
    toast(T.connecting);
    connect(card);
  }

  function detach() {
    closing = true;
    if (socket) socket.close();
    socket = null;
    current = null;
    el.view.hidden = true;
    el.perm.hidden = true;
    el.list.hidden = false;
    el.back.hidden = true;
    el.grade.hidden = true;
    el.subtitle.textContent = '';
    refresh();
  }

  el.back.addEventListener('click', detach);

  /* ------------------------------------------------------- mobile input */

  /* A touch keyboard has no Esc, no Tab, no Ctrl and no arrows, and those
   * are most of how a terminal agent is actually driven: Esc to cancel,
   * Shift+Tab to cycle Claude Code's permission mode, Ctrl-C to interrupt.
   * Ctrl is sticky - press it, then a letter - because a phone cannot hold
   * two keys at once. */

  const KEYS = [
    { label: 'Esc', data: '\x1b' },
    { label: 'Tab', data: '\t' },
    { label: '⇧Tab', data: '\x1b[Z' },
    { label: 'Ctrl', sticky: true },
    { label: '↑', data: '\x1b[A' },
    { label: '↓', data: '\x1b[B' },
    { label: '←', data: '\x1b[D' },
    { label: '→', data: '\x1b[C' },
    { label: '⏎', data: '\r' },
  ];

  let ctrlHeld = false;

  function buildKeys() {
    if (el.keys.childElementCount) return;
    for (const key of KEYS) {
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = key.label;
      button.addEventListener('click', (event) => {
        event.preventDefault();
        if (key.sticky) {
          ctrlHeld = !ctrlHeld;
          button.classList.toggle('held', ctrlHeld);
          return;
        }
        send({ t: 'input', data: key.data });
        term.focus();
      });
      el.keys.append(button);
    }
  }

  // Sticky Ctrl folds the next printable character to its control code.
  document.addEventListener('keydown', (event) => {
    if (!ctrlHeld || !current) return;
    if (event.key.length !== 1) return;
    const code = event.key.toUpperCase().charCodeAt(0);
    if (code < 64 || code > 95) return;
    event.preventDefault();
    send({ t: 'input', data: String.fromCharCode(code - 64) });
    ctrlHeld = false;
    for (const button of el.keys.children) button.classList.remove('held');
  });

  el.composer.addEventListener('submit', (event) => {
    event.preventDefault();
    const text = compose.value;
    if (!text) return;
    // One paste, not a stream of keystrokes: a mobile IME rewrites what it
    // has already emitted, which a raw terminal cannot take back.
    send({ t: 'paste', data: text });
    compose.value = '';
    compose.style.height = 'auto';
  });

  compose.addEventListener('input', () => {
    compose.style.height = 'auto';
    compose.style.height = `${Math.min(compose.scrollHeight, window.innerHeight * 0.3)}px`;
  });

  compose.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      el.composer.requestSubmit();
    }
  });

  /* -------------------------------------------------------------- layout */

  let resizeTimer = 0;
  function scheduleRefit() {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(refit, 120);
  }
  window.addEventListener('resize', scheduleRefit);
  window.addEventListener('orientationchange', scheduleRefit);
  if (window.visualViewport) {
    // The on-screen keyboard resizes the visual viewport, not the window.
    window.visualViewport.addEventListener('resize', scheduleRefit);
  }

  /* ---------------------------------------------------------------- boot */

  if (!token) {
    toast(T.denied, 'error');
  }
  loadCaps().then(refresh);
  setInterval(() => {
    if (!current) refresh();
  }, 5000);
})();
