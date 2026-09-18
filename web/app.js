/* The remote-control page: DOM wiring only.
 *
 * No framework and no build step, for the same reason xterm.js is vendored:
 * this file is served out of the alc binary, and a toolchain between the
 * source and what ships is a toolchain someone has to keep working. What the
 * page *decides* lives in core.js, which has no DOM and is unit-tested.
 *
 * The rule this file follows: anything that is a state is drawn as a state -
 * the connection light, the row's dot, the read-only pill - and the toast is
 * kept for things that merely happen. A toast fades, so anything said only
 * by toast is information the user cannot get back.
 */

(() => {
  'use strict';

  const core = window.AlcCore;
  const LOCALE = core.pickLocale(navigator.languages || [navigator.language]);
  const T = core.strings(LOCALE);
  document.documentElement.lang = LOCALE;

  /* ---------------------------------------------------------------- dom */

  const el = {};
  for (const id of [
    'bar', 'back', 'railToggle', 'subtitle', 'grade', 'link', 'linkLabel',
    'list', 'sessions', 'empty', 'emptyTitle', 'emptyHow', 'emptyCommand',
    'view', 'terminal', 'keys',
    'perm', 'permLabel', 'permPick', 'permCycle', 'permNote', 'permFlags', 'permCommand',
    'composer', 'compose', 'send', 'toast',
    'usage', 'usageToggle', 'accountsTitle', 'accounts', 'usageNote',
    'usageEmpty', 'usageEmptyTitle', 'usageEmptyHow', 'usageEmptyCommand',
    'ledgerTitle', 'ledger', 'ledgerProvider', 'ledgerAgent', 'ledgerLaunches',
    'ledgerTurns', 'ledgerTokens', 'ledgerNote',
  ]) {
    el[id] = document.getElementById(id);
  }

  el.back.setAttribute('aria-label', T.back);
  // The panes are landmarks, and the rail's fold points at #list by name, so
  // a screen reader announces them in the page's language along with the
  // button that controls one of them.
  el.list.setAttribute('aria-label', T.sessionsRegion);
  el.usage.setAttribute('aria-label', T.usage);
  el.view.setAttribute('aria-label', T.terminalRegion);
  el.send.textContent = T.send;
  el.compose.placeholder = T.composerHint;
  el.emptyTitle.textContent = T.empty;
  el.emptyHow.textContent = T.emptyHow;
  commandText(el.emptyCommand, T.emptyCommand);
  el.usageToggle.setAttribute('aria-label', T.usage);
  el.usageToggle.title = T.usage;
  el.accountsTitle.textContent = T.accounts;
  el.ledgerTitle.textContent = T.byAgent;
  el.usageEmptyTitle.textContent = T.usageEmpty;
  el.usageEmptyHow.textContent = T.usageEmptyHow;
  commandText(el.usageEmptyCommand, T.usageEmptyCommand);
  el.usageNote.textContent = T.hubNote;
  el.ledgerProvider.textContent = T.provider;
  el.ledgerAgent.textContent = T.agent;
  el.ledgerLaunches.textContent = T.launches;
  el.ledgerTurns.textContent = T.turns;
  el.ledgerTokens.textContent = T.tokens;

  let toastTimer = 0;
  function toast(message, level) {
    el.toast.textContent = message;
    el.toast.className = level || '';
    el.toast.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      el.toast.hidden = true;
    }, level === 'error' ? 8000 : 3200);
  }

  /* -------------------------------------------------------------- state */

  /* Web storage, or null when there is none to be had.
   *
   * With site data blocked, reading `window.sessionStorage` or
   * `window.localStorage` is itself what throws - before any getItem a
   * callee could have wrapped - and a throw here, at boot, took the whole
   * page down with it. core's storage helpers already read null as empty. */
  function storageArea(name) {
    try {
      return window[name];
    } catch {
      return null;
    }
  }

  const token = core.signal(
    core.loadToken(window.location, window.history, storageArea('sessionStorage')),
  );
  const rows = core.signal([]);
  /* idle | connecting | live | reconnecting | ended */
  const connection = core.signal('idle');
  const attached = core.signal(null);
  /* A viewer link cannot type. The server answers input, paste and resize
   * from one with "this link can watch but not type", so a page that sends
   * them anyway warns the user about itself on every window resize. */
  const operator = core.signal(true);
  /* Nothing has been fetched yet, so an empty list is "not loaded" rather
   * than "no sessions" - the empty state used to claim the latter for the
   * whole first round trip, and again after every failed poll. */
  let loaded = false;

  const usage = core.signal(null);
  let usageTimer = 0;
  let lastUsageError = '';

  const store = core.createSessionStore();

  /* ---------------------------------------------------------------- api */

  async function api(path) {
    const key = token.value;
    const response = await fetch(path, {
      headers: key ? { Authorization: `Bearer ${key}` } : {},
      cache: 'no-store',
    });
    if (response.status === 401 || response.status === 403) {
      // A token the server has stopped accepting is worse than none: it
      // would keep failing every poll for the life of the tab.
      core.forgetToken(storageArea('sessionStorage'));
      token.value = null;
      renderRows([]);
      throw new Error('denied');
    }
    if (!response.ok) throw new Error(`http ${response.status}`);
    return response.json();
  }

  /* ------------------------------------------------------- session list */

  let listTimer = 0;
  /* The last payload the server sent, kept whole.
   *
   * Deliberately not the rendered rows: those have already had expired cards
   * filtered out, and feeding them back would tell the store the session had
   * gone away, which resets its exit clock. The next poll would then show it
   * again for another full grace period. */
  let lastCards = [];

  let expiryTimer = 0;
  let lastListError = '';

  async function refresh() {
    if (!token.value) {
      rows.value = [];
      return;
    }
    let cards;
    try {
      cards = await api('/api/sessions');
      loaded = true;
      lastListError = '';
    } catch (error) {
      // One toast per distinct failure: a 5-second poll against a hub that
      // is down must not stack a notification every 5 seconds.
      const message = error.message === 'denied' ? T.denied : T.loadFailed;
      if (message !== lastListError) {
        lastListError = message;
        toast(message, 'error');
      }
      return;
    }
    lastCards = cards;
    rows.value = store.reconcile(cards, Date.now());
    scheduleExpiry();
    followSessionSize(cards);
  }

  /* The accounts and the ledger, on their own clock.
   *
   * A minute, not the list's five seconds: behind this route the hub asks
   * providers what is left, and a poll per five seconds would be a vendor
   * call per five seconds. Only runs while the pane is on screen. */
  async function refreshUsage() {
    if (!token.value) {
      usage.value = null;
      // The "this link has expired" state lives in the list pane, which is
      // hidden while usage is on screen, so go back to where it can be read.
      if (document.body.dataset.pane === 'usage') leaveUsage();
      return;
    }
    try {
      usage.value = core.normalizeUsage(await api('/api/usage'));
      lastUsageError = '';
    } catch (error) {
      const message = error.message === 'denied' ? T.denied : T.usageFailed;
      if (message !== lastUsageError) {
        lastUsageError = message;
        toast(message, 'error');
      }
    }
  }

  /* A plain session's size is the local terminal's, and it changes whenever
   * that terminal does. Nothing tells the page over the socket - the
   * server's control frames are notices, exits and keepalives, none of which
   * carries a size - so this poll is what notices, and the grid is redrawn
   * at the new shape. Five seconds late is a redraw, not a wrong one: what
   * is on screen in between is the agent's real output, drawn at the ratio
   * it had a moment ago.
   *
   * The condition is who is driving, not which mode the session is in. A
   * page that owns the size must not read it back off the card: it would be
   * fitting to a number it produced itself, and any disagreement - a clamp
   * the server applied, another browser on the same session - would return
   * every five seconds as a fresh resize. But a read-only link on a `--tmux`
   * session drives nothing and needs this exactly as much as a plain session
   * does, because the operator's window is moving the grid underneath it. */
  function followSessionSize(cards) {
    const card = attached.value;
    if (!card || browserOwnsSize(card)) return;
    const fresh = cards.find((row) => row.id === card.id);
    if (!fresh || (fresh.cols === card.cols && fresh.rows === card.rows)) return;
    // A fresh object rather than two fields written into the old one: every
    // other writer of this signal assigns a whole card, and a subscriber
    // that compared identities would never see an in-place edit.
    attached.value = { ...card, cols: fresh.cols, rows: fresh.rows };
    relayout();
  }

  /* An exited row leaves on its own schedule rather than on the poll's, so
   * it does not sit around for up to five extra seconds. */
  function scheduleExpiry() {
    clearTimeout(expiryTimer);
    const due = store.nextExpiry(Date.now());
    if (due === null) return;
    expiryTimer = setTimeout(() => {
      rows.value = store.reconcile(lastCards, Date.now());
      scheduleExpiry();
    }, Math.max(250, due + 50));
  }

  /* Keyed in place: the row a finger is on must not be replaced under it,
   * and a full replaceChildren() on every poll made the list flicker and
   * dropped focus. */
  const built = new Map();

  function renderRows(list) {
    const seen = new Set();
    let previous = null;

    for (const row of list) {
      const id = row.card.id;
      seen.add(id);
      let node = built.get(id);
      if (!node) {
        node = buildRow();
        built.set(id, node);
      }
      updateRow(node, row);
      // Move into place only when it is not already there, so untouched
      // rows keep their DOM identity (and their focus).
      const expected = previous ? previous.li.nextSibling : el.sessions.firstChild;
      if (expected !== node.li) el.sessions.insertBefore(node.li, expected);
      previous = node;
    }

    for (const [id, node] of built) {
      if (seen.has(id)) continue;
      node.li.remove();
      built.delete(id);
    }

    // A page with no token shows why, and what to run: it used to show
    // nothing at all, because the only mention of the refusal was a toast
    // that had already faded.
    const denied = !token.value;
    if (denied) {
      setText(el.emptyTitle, T.deniedTitle);
      setText(el.emptyHow, T.deniedHow);
      commandText(el.emptyCommand, T.deniedCommand);
    } else {
      setText(el.emptyTitle, T.empty);
      setText(el.emptyHow, T.emptyHow);
      commandText(el.emptyCommand, T.emptyCommand);
    }
    el.empty.hidden = !denied && (!loaded || list.length > 0);
  }

  function buildRow() {
    const li = document.createElement('li');
    const button = document.createElement('button');
    button.className = 'card';
    button.type = 'button';

    const head = document.createElement('div');
    head.className = 'row';
    const dot = document.createElement('span');
    dot.className = 'dot';
    const name = document.createElement('span');
    name.className = 'name';
    const age = document.createElement('span');
    age.className = 'age';
    head.append(dot, name, age);

    const chips = document.createElement('div');
    chips.className = 'chips';
    const cwd = document.createElement('div');
    cwd.className = 'cwd';

    button.append(head, chips, cwd);
    li.append(button);
    return { li, button, dot, name, age, chips, cwd, id: null };
  }

  function updateRow(node, row) {
    const card = row.card;
    node.id = card.id;
    node.button.classList.toggle('exited', row.exited);
    node.button.classList.toggle('fading', row.fading);
    node.dot.classList.toggle('exited', row.exited);

    const current = attached.value;
    node.button.setAttribute('aria-current', current && current.id === card.id ? 'true' : 'false');

    setText(node.name, card.name);
    setText(node.age, row.exited
      ? core.formatExit(card.exit, T)
      : core.formatAge(card.started_at, Date.now()));

    const chips = core.describeSession(card);
    if (card.viewers > 0) chips.push(`${card.viewers} ${T.viewers}`);
    renderChips(node.chips, chips, card.unsandboxed);
    setText(node.cwd, core.elidePath(card.cwd));
    if (node.cwd.title !== card.cwd) node.cwd.title = card.cwd;

    node.button.onclick = () => attach(card);
  }

  function renderChips(host, labels, unsandboxed) {
    const wanted = unsandboxed ? [...labels, T.unsandboxed] : labels;
    while (host.childElementCount > wanted.length) host.lastElementChild.remove();
    while (host.childElementCount < wanted.length) {
      host.append(document.createElement('span'));
    }
    wanted.forEach((label, index) => {
      const chip = host.children[index];
      const warn = unsandboxed && index === wanted.length - 1;
      chip.className = warn ? 'chip warn' : 'chip';
      setText(chip, label);
    });
  }

  /* textContent assignment forces a layout even when nothing changed, and
   * this runs for every row on every poll. */
  function setText(node, text) {
    const value = text == null ? '' : String(text);
    if (node.textContent !== value) node.textContent = value;
  }

  rows.subscribe(renderRows);

  /* A command button holds an icon as well as its command now, so the text
   * goes into its own span rather than over the whole button. The command is
   * also the button's accessible name, which on its own announced as "alc
   * share claude, button" and left what the button does to be guessed. */
  function commandText(button, command) {
    const slot = button.querySelector('.command-text');
    setText(slot || button, command);
    button.setAttribute('aria-label', `${T.copy}: ${command}`);
  }

  /* Every empty state offers the command that fills it, and every one of
   * them copies. */
  function wireCopy(button) {
    button.addEventListener('click', async () => {
      const slot = button.querySelector('.command-text');
      try {
        await navigator.clipboard.writeText((slot || button).textContent);
        toast(T.copied);
      } catch {
        // No clipboard permission, or an http:// origin, which a LAN hub
        // normally is: the command is on screen to be read either way, so
        // there is nothing to recover from.
      }
    });
  }

  wireCopy(el.emptyCommand);
  wireCopy(el.usageEmptyCommand);
  wireCopy(el.permCommand);

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

  /* One message at a time, and only what merely happened.
   *
   * The bar's three channels are separate on purpose: `flags()` draws what the
   * agent IS and stays for the session, `note()` explains, and `command()`
   * offers something to run. Before this they were one span, so a click's
   * outcome erased a standing warning about the agent - which is why the
   * resync had to save the note and put it back afterwards. */
  function note(text, level) {
    const body = text || '';
    setText(el.permNote, body);
    el.permNote.hidden = !body;
    el.permNote.className = `perm-note${level ? ` ${level}` : ''}`;
  }

  /* A command the user has to run somewhere else. It is a button rather than
   * a sentence because the ticket in it is unguessable and the device reading
   * it is not the device that must run it. */
  function command(text) {
    el.permCommand.hidden = !text;
    if (text) commandText(el.permCommand, text);
  }

  function flag(icon, label) {
    const chip = document.createElement('span');
    chip.className = 'perm-flag';
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('class', 'glyph');
    svg.setAttribute('aria-hidden', 'true');
    const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
    use.setAttribute('href', `#${icon}`);
    svg.append(use);
    const word = document.createElement('span');
    word.textContent = label;
    // Icon beside word, never instead of it: this chip is the only carrier of
    // a safety fact, and the border it sits in is a colour.
    chip.append(svg, word);
    return chip;
  }

  /* The control is a pure function of the agent's capabilities, and core.js
   * owns that function. An agent that can only be cycled gets a relative
   * button and never a dropdown, because a dropdown would promise alc can
   * land on a chosen mode - and from Claude Code's `auto` the first press
   * goes somewhere else. An agent whose mode is fixed until it is relaunched
   * gets no live control at all: it used to offer the choice, let the POST
   * round-trip, and only then write a sentence saying it was never on offer.
   */
  function renderPermission(card) {
    const view = core.permissionView(capsFor(card.agent), card);
    el.perm.hidden = false;
    el.perm.classList.toggle('unsandboxed', view.ungated);
    el.permCycle.hidden = true;
    el.permPick.hidden = false;
    el.permPick.disabled = false;
    el.permPick.replaceChildren();
    command('');

    const shown = view.rung
      ? `${view.rung}${view.native ? ` · ${view.native}` : ''}${view.assumed ? ' ?' : ''}`
      : T.permUnknown;
    setText(el.permLabel, `${T.permission}: ${shown}`);

    /* An agent that gates nothing is the one fact here that is about safety
     * rather than about what a control can do, so it is a chip that stays put
     * rather than a sentence that the next outcome overwrites. It lives in
     * this bar and not only in the session row, because the row is off screen
     * for as long as the terminal is on it. */
    el.permFlags.replaceChildren();
    if (view.ungated) el.permFlags.append(flag('i-ungated', T.unsandboxed));

    if (view.control === 'dead') {
      el.permPick.disabled = true;
      const only = document.createElement('option');
      only.textContent = '—';
      el.permPick.append(only);
      // Server-authored and shown verbatim: it is the only account of why the
      // control is dead. A relaunch-only agent has no such account to give,
      // and for it the dead control is the whole message.
      if (view.deadKind === 'relaunch-only') note(T.permFixed, 'warn');
      else note(view.reason ? `${T.permUnsupported} ${view.reason}` : '', 'danger');
      return;
    }

    if (view.control === 'cycle') {
      el.permPick.hidden = true;
      el.permCycle.hidden = false;
      el.permCycle.textContent = T.permCycle;
      // The order is the useful half and stays as text: a phone has nowhere
      // to hover, so a title would put it out of reach of everyone using one.
      note(`${T.permCycleNote} ${view.order}`, 'warn');
      return;
    }

    const placeholder = document.createElement('option');
    placeholder.textContent = '…';
    placeholder.value = '';
    el.permPick.append(placeholder);
    for (const option of view.options) {
      const node = document.createElement('option');
      node.value = option.value;
      node.textContent = option.label;
      if (option.selected) node.selected = true;
      el.permPick.append(node);
    }
    note(view.unverified ? T.permUnverified : '', 'warn');
  }

  async function setPermission(rung, ticket) {
    const card = attached.value;
    if (!card) return;
    let response;
    try {
      response = await fetch(`/api/sessions/${encodeURIComponent(card.id)}/permission`, {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${token.value || ''}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(ticket ? { rung, ticket } : { rung }),
      });
    } catch {
      note(T.loadFailed, 'danger');
      return;
    }
    let body = null;
    try {
      body = await response.json();
    } catch {
      // The server refuses a viewer in text/plain, so parsing it as JSON threw
      // and the user was shown the literal string `HTTP 403`. A status code is
      // not a sentence; say which of the two things went wrong instead.
      note(response.status === 403 ? T.permDenied : T.loadFailed, 'danger');
      return;
    }
    switch (body.outcome) {
      case 'done':
        note(`${T.permSent}: ${body.rung}`);
        resyncPermission();
        break;
      case 'sent':
        note(`${T.permSent}`, 'warn');
        resyncPermission();
        break;
      case 'picker-open':
        // A state, not an event: the picker stays open in the terminal until
        // a human finishes it, so this must not be a toast that fades out
        // from under them.
        note(T.permPicker, 'warn');
        if (term) term.focus();
        resyncPermission();
        break;
      case 'needs-relaunch':
        // Reachable only from a hub that reports capabilities this page did
        // not have when it drew the control; a relaunch-only agent is given a
        // dead one up front and never gets this far.
        note(`${T.permRelaunch} ${(body.cli || []).join(' ')}`, 'warn');
        break;
      case 'needs-confirmation':
        note(T.permConfirm, 'danger');
        command(`alc confirm ${body.ticket}`);
        pendingTicket = { ticket: body.ticket, rung: body.rung };
        break;
      case 'unsupported':
        note(`${T.permUnsupported} ${body.reason}`, 'danger');
        break;
      default:
        note('');
    }
  }

  /* The dropdown shows what the agent reports, never what was asked for.
   * Without this it displayed the chosen rung the instant it was picked,
   * including for the agents that ignore the request entirely. */
  async function resyncPermission() {
    const card = attached.value;
    if (!card) return;
    // The agent needs a moment to act on what it was sent before the probe
    // can see the result.
    await new Promise((resolve) => setTimeout(resolve, 600));
    const still = attached.value;
    if (!still || still.id !== card.id) return;
    let fresh;
    try {
      fresh = await api(`/api/sessions/${encodeURIComponent(card.id)}`);
    } catch {
      return;
    }
    if (!attached.value || attached.value.id !== fresh.id) return;
    /* The note is still one slot, and re-rendering would put the agent's
     * standing caveat back over the outcome of the change the user just made,
     * which is the more useful of the two. What no longer has to be rescued
     * is everything else: the ungated chip and the confirm command are drawn
     * from their own state and survive the re-render on their own. */
    const keptNote = el.permNote.textContent;
    const keptLevel = el.permNote.className;
    const keptCommand = el.permCommand.hidden
      ? ''
      : el.permCommand.querySelector('.command-text').textContent;
    attached.value = fresh;
    renderPermission(fresh);
    if (keptNote) {
      el.permNote.textContent = keptNote;
      el.permNote.className = keptLevel;
      el.permNote.hidden = false;
    }
    if (keptCommand) command(keptCommand);
  }

  el.permPick.addEventListener('change', () => {
    const rung = el.permPick.value;
    if (!rung) return;
    // A ticket is spent on the change it was minted for, and only that one.
    const ticket = pendingTicket && pendingTicket.rung === rung ? pendingTicket.ticket : undefined;
    pendingTicket = null;
    setPermission(rung, ticket);
  });

  el.permCycle.addEventListener('click', () => {
    // Relative movement: the agent ignores the rung and just takes the
    // keystroke, so ask for the tightest one it has. Sending the current
    // rung instead made the server demand `alc confirm` for a press that
    // loosens nothing.
    const card = attached.value;
    setPermission(core.cycleRung(card ? capsFor(card.agent) : null));
  });

  /* ----------------------------------------------------------- terminal */

  let term = null;
  let fit = null;
  /* The base font size, kept because the framed mode moves the live one and
   * every step of that fit is measured against where it started. */
  let baseFontSize = 13;
  let socket = null;
  let lastSeq = null;
  let attempt = 0;
  let retryTimer = 0;
  /* Whether this attachment has ever completed a handshake. The server
   * accepts the WebSocket upgrade before it checks the token or the session
   * id, so a bad token and a dead session both look exactly like a dropped
   * connection - and retrying one forever is a loop that never succeeds. */
  let everGreeted = false;
  const BLIND_RETRIES = 3;
  /* Bumped on every attach and detach. A socket or timer from an older
   * generation is ignored outright, which is what stops a reconnect for the
   * session you just left from reviving it over the one you are on. */
  let generation = 0;

  const DARK = {
    background: '#16161a',
    foreground: '#eceae5',
    cursor: '#d98b5f',
    selectionBackground: 'rgba(217,139,95,0.32)',
  };

  function ensureTerminal() {
    if (term) return;
    baseFontSize = window.matchMedia('(min-width: 900px)').matches ? 13 : 12;
    term = new window.Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      convertEol: false,
      fontSize: baseFontSize,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      scrollback: 5000,
      theme: DARK,
    });
    fit = new window.FitAddon.FitAddon();
    term.loadAddon(fit);
    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown' || !ctrlHeld) return true;
      if (event.key.length !== 1) return true;
      const code = event.key.toUpperCase().charCodeAt(0);
      if (code < 64 || code > 95) return true;
      send({ t: 'input', data: String.fromCharCode(code - 64) });
      releaseCtrl();
      // False stops xterm handling it, so the plain character is not sent
      // alongside the control code we just sent.
      return false;
    });
    term.open(el.terminal);
    term.onData((data) => send({ t: 'input', data }));
    // No term.onBinary: the only client->server channel is a JSON text
    // frame, and xterm's binary payload is a byte-per-char string. Encoding
    // that as JSON turns every byte above 0x7F into two, so the pty would
    // receive something other than what was sent. Dropping it loses mouse
    // reporting, which this page does not enable, rather than corrupting
    // input that it does.
  }

  /* Whether this page is the side that decides how large the session is.
   *
   * Only with `--tmux`, where the agent runs under a multiplexer and the
   * hub's mirror carries this window's size into it while the user's own
   * terminal abstains. `--tmux` is asked for by somebody who is about to go
   * and use the page, so the page drives.
   *
   * Without it there is one pty and its size belongs to the terminal that
   * launched it, which is still sitting there drawing at that size. A
   * read-only link never decides anything either way - the server drops its
   * resize frames, so acting as though it had one would only render the
   * screen at a shape the agent is not using. */
  function browserOwnsSize(card) {
    return !!(card && card.tmux) && operator.value;
  }

  function relayout() {
    if (!term) return;
    // Measuring a pane that is display:none measures nothing, and the fit
    // addon clamps to its own minimum rather than bailing - which would then
    // resize the real agent's pty down to a couple of columns.
    if (document.body.dataset.pane !== 'view') return;

    const card = attached.value;
    const drives = browserOwnsSize(card);
    // Set before anything is measured: the two modes give #terminal
    // different padding and a different background, so which box is being
    // measured depends on this attribute.
    document.body.dataset.sizing = drives ? 'browser' : 'session';
    if (drives) driveSize();
    else frameGrid(card, 0);
  }

  /* The page's window is the size, so the agent is told what fits in it. */
  function driveSize() {
    if (!fit) return;
    // The framed mode leaves a font size and a transform behind, and there
    // is one terminal for every session on the page: attaching to a plain
    // session and then to a `--tmux` one would otherwise fit this window
    // while still drawing it shrunk.
    term.element.style.transform = '';
    if (term.options.fontSize !== baseFontSize) {
      term.options.fontSize = baseFontSize;
      // The fit divides this box by a cell, and the cell is what the new
      // font size measures to only once the renderer has re-measured it.
      // Fitting now would divide by the old cell and tell the agent a width
      // it does not have, which is the bug this whole mode exists to avoid.
      requestAnimationFrame(() => {
        if (document.body.dataset.sizing === 'browser') driveSize();
      });
      return;
    }
    try {
      fit.fit();
    } catch {
      // The pane is not laid out yet (display:none, or a zero-height
      // parent). The next resize or attach will fit it.
      return;
    }
    send({ t: 'resize', cols: term.cols, rows: term.rows });
  }

  /* The session's own grid, drawn as large as the frame allows.
   *
   * `core.fitGrid` decides; this measures for it and applies the answer. The
   * loop is because a cell's pixel size is not perfectly linear in the font
   * size - a renderer rounds to device pixels - so each pass is given the
   * cell the previous font size actually produced. Passes are separated by a
   * frame, because that is when the renderer has re-measured and re-laid out
   * the grid, rather than assuming it did so on assignment. */
  const FIT_PASSES = 3;
  /* What the vendored fit addon reserves for the overview ruler when there is
   * scrollback, which is where this number comes from. */
  const SCROLLBAR_RESERVE = 14;

  function frameGrid(card, pass) {
    // xterm clamps `resize` to its own 2x1 minimum, so a card with no size
    // yet would land there and stay.
    if (card && card.cols && card.rows) term.resize(card.cols, card.rows);
    const cell = cellPixels();
    if (!cell) return;
    // The same reservation the fit addon makes for the other mode, and for
    // the same reason: this xterm draws its own overlay scrollbar inside
    // `.xterm`, over the grid's last column. Measuring it is not an option -
    // an overlay scrollbar takes no layout width on any platform, so
    // `offsetWidth - clientWidth` is zero everywhere and would reserve
    // nothing.
    const box = {
      width: el.terminal.clientWidth - SCROLLBAR_RESERVE,
      height: el.terminal.clientHeight,
    };

    const placed = core.fitGrid(
      { fontSize: term.options.fontSize, cell, cols: term.cols, rows: term.rows },
      box
    );
    if (placed.fontSize !== term.options.fontSize && pass < FIT_PASSES) {
      // Another pass. The transform is left as it is rather than guessed at
      // for a grid that is about to change size - the settled pass below is
      // what puts it right.
      term.options.fontSize = placed.fontSize;
      requestAnimationFrame(() => {
        // The mode or the session can change inside a frame, and this pass
        // would then be measuring the wrong one.
        if (document.body.dataset.sizing !== 'session') return;
        if (attached.value !== card) return;
        frameGrid(card, pass + 1);
      });
      return;
    }

    // Settled, or out of passes. Either way `scale` describes the grid that
    // is actually on screen, so applying it here is what guarantees the
    // agent's screen is never cropped - a fit that did not converge ends up
    // transformed instead, which costs xterm's hit-testing the same factor
    // and is the lesser of the two. In the ordinary case it is 1.
    term.element.style.transform = placed.scale < 1 ? `scale(${placed.scale})` : '';
  }

  /* One character's box, in CSS pixels.
   *
   * The cell rather than the grid, because the renderer defers writing the
   * grid's pixel size onto `.xterm-screen` while the element is off-screen,
   * and the first layout after an attach runs a frame too early to see it.
   * The cell does not depend on cols or rows, so it survives that.
   *
   * `_renderService.dimensions` is not public API. It is the same path the
   * vendored fit addon takes for the other mode, so it is a dependency this
   * page already ships; the DOM measurement below is the fallback for the
   * build where it stops being true. */
  function cellPixels() {
    const dimensions =
      term._core && term._core._renderService && term._core._renderService.dimensions;
    const cell = dimensions && dimensions.css && dimensions.css.cell;
    if (cell && cell.width > 0 && cell.height > 0) return cell;
    const screen = el.terminal.querySelector('.xterm-screen');
    if (!screen || !screen.offsetWidth || !term.cols || !term.rows) return null;
    return { width: screen.offsetWidth / term.cols, height: screen.offsetHeight / term.rows };
  }

  const NEEDS_OPERATOR = new Set(['input', 'paste', 'resize']);

  /* Returns whether the frame actually went. A caller that is about to
   * throw away what the user typed needs to know it did not. */
  function send(frame) {
    if (NEEDS_OPERATOR.has(frame.t) && !operator.value) return false;
    if (!socket || socket.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(frame));
    return true;
  }

  function setConnection(state) {
    connection.value = state;
  }

  connection.subscribe((state) => {
    if (state === 'idle') {
      el.link.hidden = true;
      return;
    }
    el.link.hidden = false;
    el.link.dataset.state = state;
    setText(el.linkLabel, T[state] || state);
  });

  function connect(card, mine) {
    const url = new URL('/ws', location.href);
    url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    url.searchParams.set('session', card.id);

    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';
    socket = ws;

    ws.addEventListener('open', () => {
      if (mine !== generation) return ws.close();
      // A size is stated only by the side that owns one. The server resizes
      // the session from this frame before it answers, so sending this
      // window's columns for a session the local terminal sizes is exactly
      // how a shared `alc claude` ended up at a phone's width - on every
      // reconnect, too. Omitted, both fields default to zero and the server
      // skips the resize.
      const frame = { t: 'auth', token: token.value || '', since: lastSeq };
      if (browserOwnsSize(card)) {
        frame.cols = term ? term.cols : 80;
        frame.rows = term ? term.rows : 24;
      }
      send(frame);
    });

    ws.addEventListener('message', (event) => {
      if (mine !== generation) return;
      if (typeof event.data === 'string') return onControl(JSON.parse(event.data));
      onBinary(new Uint8Array(event.data));
    });

    ws.addEventListener('close', () => {
      if (mine !== generation) return;
      if (connection.value === 'ended') return;
      attempt = Math.min(attempt + 1, 6);
      if (!everGreeted && attempt > BLIND_RETRIES) {
        // Never greeted, and out of tries: this is a refusal, not a blip.
        setConnection('ended');
        toast(T.denied, 'error');
        return;
      }
      setConnection('reconnecting');
      clearTimeout(retryTimer);
      retryTimer = setTimeout(() => {
        if (mine !== generation) return;
        const still = attached.value;
        if (still) connect(still, mine);
      }, core.backoffDelay(attempt));
    });
  }

  function onBinary(bytes) {
    const frame = core.decodeFrame(bytes);
    if (!frame) return;
    if (frame.snapshot) term.reset();
    term.write(frame.payload);
    lastSeq = frame.seq;
    // Data flowing is the definition of connected, and it is the only
    // signal that survives a reconnect the socket itself did not notice.
    if (connection.value !== 'ended') setConnection('live');
    attempt = 0;
  }

  function onControl(frame) {
    switch (frame.t) {
      case 'hello': {
        attached.value = frame.session;
        everGreeted = true;
        // Deliberately not `lastSeq = frame.seq`: that claims bytes the
        // terminal has not written, so a drop before the catch-up frame
        // would resume past them and lose that output for good. Leaving it
        // null asks for a snapshot instead, which is always safe.
        setGrade(frame.grade);
        setText(el.subtitle, `${frame.session.name} · ${core.describeSession(frame.session).join(' · ')}`);
        renderPermission(frame.session);
        setConnection('live');
        attempt = 0;
        relayout();
        break;
      }
      case 'notice':
        toast(frame.message, frame.level);
        break;
      case 'exit': {
        // An exit is a state, not an event: the light stays red until the
        // user leaves, rather than fading with a toast they may not see.
        setConnection('ended');
        // The agent is gone: the controls that would talk to it go too,
        // rather than staying live and erroring on every keystroke.
        setTypingAllowed(false);
        toast(core.formatExit({ code: frame.code, signal: frame.signal }, T), 'warn');
        refresh();
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
    const canType = grade === 'operator';
    operator.value = canType;
    el.grade.hidden = false;
    el.grade.textContent = canType ? T.operator : T.viewer;
    el.grade.className = canType ? 'pill operator' : 'pill';
    setTypingAllowed(canType);
  }

  function setTypingAllowed(allowed) {
    document.body.classList.toggle('viewer', !allowed);
    if (term) term.options.disableStdin = !allowed;
    el.compose.disabled = !allowed;
    el.send.disabled = !allowed;
    for (const button of el.keys.children) button.disabled = !allowed;
  }

  function attach(card) {
    // Everything belonging to the previous session goes first, or its
    // scrollback, its sequence and its socket all leak into this one.
    teardown();
    generation += 1;
    const mine = generation;

    attached.value = card;
    lastSeq = null;
    attempt = 0;
    everGreeted = false;
    document.body.dataset.pane = 'view';
    coverRail();
    el.back.hidden = false;
    // The header is full while a session is open, and back means the session
    // here rather than the accounts.
    el.usageToggle.hidden = true;
    clearInterval(usageTimer);
    usageTimer = 0;

    ensureTerminal();
    term.reset();
    buildKeys();
    setTypingAllowed(true);
    setConnection('connecting');
    connect(card, mine);

    requestAnimationFrame(() => {
      relayout();
      if (!document.body.classList.contains('viewer')) term.focus();
    });
    renderRows(rows.value);
  }

  function teardown() {
    generation += 1;
    clearTimeout(retryTimer);
    if (socket) {
      // `generation` was bumped above, so this socket's own close handler
      // will see it is stale and return without scheduling a reconnect.
      // (Nulling `onclose` would not help: the handlers are registered with
      // addEventListener, which that property does not touch.)
      try {
        socket.close();
      } catch {
        // Already closing; nothing to do.
      }
    }
    socket = null;
    lastSeq = null;
    attempt = 0;
  }

  function detach() {
    teardown();
    // A sticky Ctrl and a half-written prompt belong to the session they
    // were made for: carrying either into the next one sends it somewhere
    // the user never meant.
    releaseCtrl();
    el.compose.value = '';
    autoGrow();
    attached.value = null;
    document.body.dataset.pane = 'list';
    // The list is the whole page now, and an inert one could not be used.
    coverRail();
    // The next session decides this for itself, and a stale value would
    // paint the list pane's frame for a mode it is not in.
    delete document.body.dataset.sizing;
    el.back.hidden = true;
    el.grade.hidden = true;
    el.perm.hidden = true;
    // A ticket is minted for one change on one session. Leaving it here meant
    // the next session attached to could spend it.
    pendingTicket = null;
    el.usageToggle.hidden = false;
    document.body.classList.remove('viewer');
    setText(el.subtitle, '');
    setConnection('idle');
    refresh();
    renderRows(rows.value);
  }

  /* ---------------------------------------------------------- usage pane */

  /* Rebuilt rather than reconciled in place: there are a handful of accounts,
   * they change once a minute, and none of them holds focus or selection the
   * way a session card does. */
  function renderUsage(model) {
    el.accounts.textContent = '';
    const nowSeconds = Math.floor(Date.now() / 1000);

    for (const account of model.accounts) {
      const li = document.createElement('li');
      const card = document.createElement('div');
      card.className = 'account';

      const head = document.createElement('div');
      head.className = 'row';
      const name = document.createElement('span');
      name.className = 'name';
      name.textContent = account.profile;
      const label = document.createElement('span');
      label.className = 'age';
      label.textContent = account.label || '';
      head.append(name, label);

      const chips = document.createElement('div');
      chips.className = 'chips';
      for (const text of core.describeAccount(account)) {
        const chip = document.createElement('span');
        chip.className = 'chip';
        chip.textContent = text;
        chips.append(chip);
      }

      card.append(head, chips);

      const windows = core.accountWindows(account);
      if (windows.length) {
        const meters = document.createElement('div');
        meters.className = 'meters';
        for (const window of windows) {
          const described = core.describeWindow(window, nowSeconds, T);
          const row = document.createElement('div');
          row.className = 'meter-row';

          /* Label and countdown are what the number is about; the number is
           * the answer. It used to carry `.age` - the 12px muted class a
           * session's timestamp uses - so the one figure this pane exists to
           * report was set lighter than the text around it, and the eye fell
           * back to reading the sentence instead. */
          const head = document.createElement('div');
          head.className = 'meter-head';

          const label = document.createElement('span');
          label.className = 'meter-label';
          label.textContent = described.label;

          const resets = document.createElement('span');
          resets.className = 'meter-resets';
          resets.textContent = described.resets;

          /* The whole phrase, not the bare figure. `{p}% left` and `剩 {p}%`
           * put the number in different places, and a percentage on its own
           * beside a bar reads as easily as "63% spent". */
          const value = document.createElement('span');
          value.className = 'quota';
          value.textContent = described.left;

          head.append(label, resets, value);

          const track = document.createElement('div');
          track.className = 'meter';
          track.setAttribute('role', 'meter');
          track.setAttribute('aria-valuemin', '0');
          track.setAttribute('aria-valuemax', '100');
          track.setAttribute('aria-valuenow', String(described.leftPercent));
          track.setAttribute('aria-label', `${described.label} ${described.left}`);
          const fill = document.createElement('span');
          fill.className = `meter-fill ${described.level}`;
          fill.style.width = `${described.leftPercent}%`;
          track.append(fill);

          row.append(head, track);
          meters.append(row);
        }
        card.append(meters);
      }

      const note = core.accountNote(account);
      if (note) {
        const line = document.createElement('p');
        line.className = 'note';
        line.textContent = note;
        card.append(line);
      }

      li.append(card);
      el.accounts.append(li);
    }

    el.usageEmpty.hidden = model.accounts.length > 0;
    el.usageNote.hidden = !model.hubEnv;

    const body = el.ledger.tBodies[0];
    body.textContent = '';
    for (const row of model.rows) {
      const tr = document.createElement('tr');
      for (const cell of core.ledgerCells(row)) {
        const td = document.createElement('td');
        td.textContent = cell;
        tr.append(td);
      }
      body.append(tr);
    }
    el.ledger.hidden = model.rows.length === 0;
    /* The caveat explains a dash, so it appears when there is one. A footnote
     * is the right control for a measurement caveat - `—` says "no value" and
     * no glyph can say "because alc did not carry this traffic" - but under a
     * table whose figures are all real it reads as a doubt about them. */
    const caveat = core.ledgerNeedsCaveat(model.rows) ? T.directOnly : '';
    const ledgerNote = model.rows.length ? caveat : T.ledgerEmpty;
    el.ledgerNote.textContent = ledgerNote;
    el.ledgerNote.hidden = !ledgerNote;
  }

  /* Null is drawn rather than skipped: a pane that has never loaded, or whose
   * link has just expired, must say so instead of leaving the last good
   * numbers on screen or showing a bare table header. */
  usage.subscribe((model) =>
    renderUsage(model || { accounts: [], rows: [], hubEnv: false }),
  );

  function showUsage() {
    document.body.dataset.pane = 'usage';
    el.back.hidden = false;
    el.usageToggle.hidden = true;
    refreshUsage();
    clearInterval(usageTimer);
    usageTimer = setInterval(refreshUsage, 60000);
  }

  function leaveUsage() {
    clearInterval(usageTimer);
    usageTimer = 0;
    document.body.dataset.pane = 'list';
    el.back.hidden = true;
    el.usageToggle.hidden = false;
    refresh();
  }

  el.usageToggle.addEventListener('click', showUsage);

  /* One back button, two places to go back from. */
  el.back.addEventListener('click', () => {
    if (document.body.dataset.pane === 'usage') leaveUsage();
    else detach();
  });

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

  function releaseCtrl() {
    ctrlHeld = false;
    for (const button of el.keys.children) button.classList.remove('held');
  }

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

  /* Sticky Ctrl folds the next printable character to its control code.
   *
   * The terminal is handled by xterm's own key hook above, which can stop
   * the plain character being sent as well; this covers a press made while
   * focus is elsewhere on the page. The composer is excluded outright: a
   * sticky Ctrl left on must not silently eat what is typed into it. */
  document.addEventListener('keydown', (event) => {
    if (!ctrlHeld || !attached.value) return;
    if (event.target === el.compose) return;
    if (el.terminal.contains(event.target)) return;
    if (event.key.length !== 1) return;
    const code = event.key.toUpperCase().charCodeAt(0);
    if (code < 64 || code > 95) return;
    event.preventDefault();
    send({ t: 'input', data: String.fromCharCode(code - 64) });
    releaseCtrl();
  });

  el.composer.addEventListener('submit', (event) => {
    event.preventDefault();
    const text = el.compose.value;
    if (!text) return;
    // One paste, not a stream of keystrokes: a mobile IME rewrites what it
    // has already emitted, which a raw terminal cannot take back.
    if (!send({ t: 'paste', data: text })) {
      // Keep what was written: silently clearing the box loses a prompt the
      // user may have spent a minute on.
      toast(connection.value === 'ended' ? T.ended : T.reconnecting, 'warn');
      return;
    }
    el.compose.value = '';
    autoGrow();
    el.compose.focus();
  });

  function autoGrow() {
    el.compose.style.height = 'auto';
    el.compose.style.height = `${el.compose.scrollHeight}px`;
  }

  el.compose.addEventListener('input', autoGrow);

  el.compose.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      el.composer.requestSubmit();
    }
  });

  /* -------------------------------------------------------------- layout */

  let resizeTimer = 0;
  function scheduleRelayout() {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(relayout, 120);
  }

  /* The box, not the window, is what both modes measure against, and this
   * list of everything that moves it kept growing: the rail appears at
   * 900px and folds away, the composer and the key bar leave with a
   * read-only link or an exit, the permission bar is the agent's business.
   * Watching the element is one subscription instead. Nothing loops back:
   * #terminal is a flex item with `flex: 1 1 0%` and `min-width/height: 0`,
   * so its size comes from the pane around it and never from the grid inside
   * it - and a transform changes no layout at all. */
  if (window.ResizeObserver) new window.ResizeObserver(scheduleRelayout).observe(el.terminal);
  else window.addEventListener('resize', scheduleRelayout);
  window.addEventListener('orientationchange', scheduleRelayout);
  if (window.visualViewport) {
    // The on-screen keyboard resizes the visual viewport, not the layout, so
    // no element changes size and the observer above never fires.
    window.visualViewport.addEventListener('resize', scheduleRelayout);
  }
  // Which mode this is depends on the grade, and the grade arrives with the
  // hello - after the first attach has already laid the terminal out.
  operator.subscribe(scheduleRelayout);

  /* The session rail, folded away on a wide screen.
   *
   * The fold exists to give the terminal the rail's width, which for a
   * `--tmux` session is the agent's width too, so the terminal is fitted
   * once, where it comes to rest: a fit partway through the slide would tell
   * tmux a width it keeps for a fifth of a second, and the agent would
   * redraw twice. With a ResizeObserver every frame of the slide restarts the
   * debounce anyway. Without one, nothing is scheduled until the slide says
   * it is over - a timer started at the press would go off halfway through
   * it. With no slide to wait for, under reduced motion, the fit is
   * scheduled at once.
   *
   * Remembered per viewer, and written only when pressed, never at boot. No
   * keyboard shortcut: every chord on this page belongs to the terminal, and
   * Ctrl+B is tmux's own prefix. */
  const preferences = storageArea('localStorage');
  const railFolded = core.signal(core.loadRailFolded(preferences));

  /* Covered means gone: out of the tab order and away from a screen reader
   * from the moment of the press, not from when the slide ends and the
   * stylesheet hides it. Tab straight after folding otherwise landed on a
   * card that went invisible under the focus a moment later, and focus fell
   * to the page. Asked again on attach and detach, since a rail is only
   * covered beside a terminal; on a phone it is display:none there already,
   * and this merely agrees. */
  function coverRail() {
    el.list.inert = railFolded.value && document.body.dataset.pane === 'view';
  }

  railFolded.subscribe((folded) => {
    document.body.dataset.rail = folded ? 'folded' : 'open';
    const label = folded ? T.showSessions : T.hideSessions;
    el.railToggle.setAttribute('aria-expanded', String(!folded));
    el.railToggle.setAttribute('aria-label', label);
    el.railToggle.title = label;
    coverRail();
  });

  /* The press is answered: the attribute that lets the terminal slide goes,
   * so a later window resize or attach cannot set the slide off, and the
   * terminal is fitted where it stopped. */
  let railSlideTimer = 0;
  function railSettled() {
    clearTimeout(railSlideTimer);
    delete document.body.dataset.railMoving;
    scheduleRelayout();
  }

  el.view.addEventListener('transitionend', (event) => {
    if (event.target === el.view && event.propertyName === 'margin-left') railSettled();
  });

  // A pointer press leaves focus where it was, which is usually the
  // terminal: folding is something done mid-sentence, and a focused button
  // would take the next space typed as a second press.
  el.railToggle.addEventListener('mousedown', (event) => event.preventDefault());

  el.railToggle.addEventListener('click', () => {
    // Asked before the fold, which makes the rail inert and can take the
    // focus out of it before this handler looks.
    const focusInRail = el.list.contains(document.activeElement);
    clearTimeout(railSlideTimer);
    // Set in the same change as the fold: a transition is taken from the
    // style after the change, so this is what lets this one change animate.
    document.body.dataset.railMoving = '';
    railFolded.value = !railFolded.value;
    core.saveRailFolded(preferences, railFolded.value);
    // A card that had focus has just gone inert, and focus would fall to the
    // page with it. The button has not moved, so it is where a keyboard can
    // press again from.
    if (railFolded.value && focusInRail) el.railToggle.focus();
    // The stylesheet decides whether this slides - reduced motion says no -
    // so it is asked rather than second-guessed. The timer is for a slide
    // that never reports back: cut short by leaving the session, say.
    if (parseFloat(getComputedStyle(el.view).transitionDuration) > 0) {
      railSlideTimer = setTimeout(railSettled, 500);
    } else {
      railSettled();
    }
  });

  /* ---------------------------------------------------------------- boot */

  if (!token.value) toast(T.denied, 'error');

  document.body.dataset.pane = 'list';
  loadCaps().then(refresh);

  listTimer = setInterval(refresh, 5000);
  window.addEventListener('beforeunload', () => {
    clearInterval(listTimer);
    clearInterval(usageTimer);
    clearTimeout(expiryTimer);
  });

  // Coming back to a backgrounded tab should show the truth immediately
  // rather than up to five seconds of stale list.
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) return;
    if (document.body.dataset.pane === 'usage') refreshUsage();
    else refresh();
  });
})();
