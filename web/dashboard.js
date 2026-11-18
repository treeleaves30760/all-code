/* The usage page's history: tiles, a daily chart, an hour strip and ranked
 * lists, drawn from /api/usage/history.
 *
 * Plain SVG built by hand, for the same reason the rest of the page has no
 * framework: it ships inside the alc binary, and a charting library is half a
 * megabyte to draw a few dozen rectangles. Every colour is a token
 * (`--chart-1` … `--chart-4`), so a theme or a mod restyles the charts with
 * the rest of the page.
 *
 * The accounts and the ledger below the charts are still app.js's: they were
 * there first and poll on their own clock.
 */

(() => {
  'use strict';

  const shell = window.AlcShell;
  if (!shell) return;
  const { core, T, LOCALE, api } = shell;
  const SVG = 'http://www.w3.org/2000/svg';

  const $ = (id) => document.getElementById(id);
  const dom = {
    heading: $('usageHeading'),
    sub: $('usageSub'),
    range: $('rangePick'),
    stats: $('stats'),
    dailyTitle: $('dailyTitle'),
    metric: $('metricPick'),
    legend: $('legend'),
    daily: $('daily'),
    hoursTitle: $('hoursTitle'),
    hours: $('hours'),
    hoursNote: $('hoursNote'),
    modelsTitle: $('modelsTitle'),
    models: $('models'),
    agentsTitle: $('agentsTitle'),
    agentBars: $('agentBars'),
    providersTitle: $('providersTitle'),
    providerBars: $('providerBars'),
    note: $('historyNote'),
  };

  dom.heading.textContent = T.navUsage;
  dom.dailyTitle.textContent = T.chartDaily;
  dom.hoursTitle.textContent = T.chartHours;
  dom.modelsTitle.textContent = T.topModels;
  dom.agentsTitle.textContent = T.byAgentShort;
  dom.providersTitle.textContent = T.byProvider;

  function storage() {
    try {
      return window.localStorage;
    } catch {
      return null;
    }
  }
  function remember(key, value) {
    try {
      const area = storage();
      if (area) area.setItem(key, value);
    } catch {
      // Blocked site data: the choice lasts for this visit only.
    }
  }
  function recall(key) {
    try {
      const area = storage();
      return area ? area.getItem(key) : null;
    } catch {
      return null;
    }
  }

  const RANGES = [7, 30, 90];
  let days = RANGES.includes(Number(recall('alc.range'))) ? Number(recall('alc.range')) : 30;
  let metric = recall('alc.metric') === 'cost' ? 'cost' : 'tokens';
  let history = null;
  let loading = false;
  let lastError = '';
  const zone = (() => {
    try {
      return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
    } catch {
      return 'UTC';
    }
  })();

  /* ---------------------------------------------------------- controls */

  /* A segmented control: radio semantics, buttons for looks. */
  function segmented(host, options, current, onPick) {
    host.replaceChildren();
    host.setAttribute('aria-label', T.rangeLabel);
    for (const option of options) {
      const button = document.createElement('button');
      button.type = 'button';
      button.setAttribute('role', 'radio');
      button.setAttribute('aria-checked', String(option.value === current));
      button.textContent = option.label;
      button.addEventListener('click', () => onPick(option.value));
      host.append(button);
    }
  }

  function drawControls() {
    segmented(
      dom.range,
      RANGES.map((value) => ({ value, label: T[`range${value}`] })),
      days,
      (value) => {
        days = value;
        remember('alc.range', String(value));
        drawControls();
        load(true);
      },
    );
    segmented(
      dom.metric,
      [
        { value: 'tokens', label: T.chartTokens },
        { value: 'cost', label: T.chartCost },
      ],
      metric,
      (value) => {
        metric = value;
        remember('alc.metric', value);
        drawControls();
        draw();
      },
    );
  }

  /* ------------------------------------------------------------- data */

  async function load(force) {
    if (loading && !force) return;
    loading = true;
    document.body.classList.toggle('history-loading', !history);
    try {
      history = await api(`/api/usage/history?days=${days}&tz=${encodeURIComponent(zone)}`);
      lastError = '';
    } catch (error) {
      if (error.message !== 'denied' && lastError !== T.historyFailed) {
        lastError = T.historyFailed;
        shell.toast(T.historyFailed, 'error');
      }
    } finally {
      loading = false;
      document.body.classList.remove('history-loading');
    }
    draw();
  }

  /* ---------------------------------------------------------- drawing */

  function svg(tag, attrs, parent) {
    const node = document.createElementNS(SVG, tag);
    for (const [key, value] of Object.entries(attrs || {})) node.setAttribute(key, String(value));
    if (parent) parent.append(node);
    return node;
  }

  function h(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  const SERIES = [
    { key: 'uncached', label: () => T.seriesUncached, colour: 'var(--chart-1)' },
    { key: 'cacheRead', label: () => T.seriesCacheRead, colour: 'var(--chart-2)' },
    { key: 'cacheWrite', label: () => T.seriesCacheWrite, colour: 'var(--chart-3)' },
    { key: 'output', label: () => T.seriesOutput, colour: 'var(--chart-4)' },
  ];

  const dayLabel = (() => {
    let format;
    try {
      format = new Intl.DateTimeFormat(LOCALE, { month: 'short', day: 'numeric', timeZone: 'UTC' });
    } catch {
      format = null;
    }
    return (date) => (format ? format.format(new Date(`${date}T00:00:00Z`)) : date.slice(5));
  })();

  function sparkline(values) {
    const width = 96;
    const height = 28;
    const max = Math.max(...values, 0);
    const root = svg('svg', { viewBox: `0 0 ${width} ${height}`, class: 'spark', 'aria-hidden': 'true', preserveAspectRatio: 'none' });
    if (!values.length) return root;
    const step = values.length > 1 ? width / (values.length - 1) : width;
    const y = (value) => (max ? height - 2 - (value / max) * (height - 4) : height - 2);
    const points = values.map((value, index) => `${(index * step).toFixed(1)},${y(value).toFixed(1)}`);
    svg('path', { d: `M0,${height} L${points.join(' L')} L${width},${height} Z`, class: 'spark-area' }, root);
    svg('path', { d: `M${points.join(' L')}`, class: 'spark-line' }, root);
    return root;
  }

  function tile(label, value, sub, extra) {
    const node = h('div', 'stat');
    node.append(h('span', 'stat-label', label), h('span', 'stat-value', value));
    if (sub) node.append(h('span', 'stat-sub', sub));
    if (extra) node.append(extra);
    return node;
  }

  function drawStats(series) {
    const totals = (history && history.totals) || {};
    const share = core.cacheShare(totals);
    dom.stats.replaceChildren(
      tile(
        T.statTokens,
        core.compactTokens(totals.tokens),
        core.fill(T.statActiveDays, { n: totals.activeDays || 0 }),
        sparkline(series.map((day) => day.total)),
      ),
      tile(
        T.statCost,
        core.formatUsd(totals.usd),
        totals.usd && !totals.usdExact ? T.statApprox : '',
        sparkline(series.map((day) => day.usd)),
      ),
      tile(T.statOutput, core.compactTokens(totals.output), core.fill(T.statRecords, { n: core.compactCount(totals.records) })),
      tile(T.statCache, share === null ? '—' : `${share}%`, T.statOfInput),
      tile(T.statSessions, core.compactCount(totals.sessions), ''),
    );
  }

  function drawLegend() {
    dom.legend.replaceChildren();
    const entries = metric === 'cost' ? [{ label: () => T.chartCost, colour: 'var(--chart-1)' }] : SERIES;
    for (const entry of entries) {
      const item = h('span', 'legend-item');
      const swatch = h('span', 'swatch');
      swatch.style.background = entry.colour;
      item.append(swatch, document.createTextNode(entry.label()));
      dom.legend.append(item);
    }
  }

  /* The tooltip is one element, moved, rather than one per bar. */
  const tip = h('div', 'chart-tip');
  tip.hidden = true;

  function showTip(day, x, chartWidth) {
    tip.replaceChildren(h('strong', '', dayLabel(day.date)));
    const rows = metric === 'cost'
      ? [[T.chartCost, core.formatUsd(day.usd), 'var(--chart-1)']]
      : [
        ...SERIES.map((entry) => [entry.label(), core.compactTokens(day[entry.key]), entry.colour]),
        [T.chartTokens, core.compactTokens(day.total), ''],
        [T.chartCost, core.formatUsd(day.usd), ''],
      ];
    for (const [label, value, colour] of rows) {
      const row = h('span', 'tip-row');
      const swatch = h('span', 'swatch');
      if (colour) swatch.style.background = colour;
      else swatch.classList.add('blank');
      row.append(swatch, h('span', 'tip-label', label), h('span', 'tip-value', value));
      tip.append(row);
    }
    tip.hidden = false;
    const left = Math.min(Math.max(x - 80, 0), Math.max(0, chartWidth - 170));
    tip.style.left = `${left}px`;
  }

  function drawDaily(series) {
    const width = Math.max(280, dom.daily.clientWidth || 600);
    const height = window.matchMedia('(max-width: 640px)').matches ? 180 : 230;
    const pad = { top: 10, right: 6, bottom: 24, left: 46 };
    const plotW = width - pad.left - pad.right;
    const plotH = height - pad.top - pad.bottom;
    const value = (day) => (metric === 'cost' ? day.usd : day.total);
    const scale = core.niceScale(Math.max(...series.map(value), 0), 4);
    const y = (v) => pad.top + plotH - (v / scale.max) * plotH;
    const band = plotW / Math.max(1, series.length);
    const barW = Math.max(2, Math.min(28, band * 0.68));

    const root = svg('svg', { viewBox: `0 0 ${width} ${height}`, width, height, role: 'img', 'aria-label': T.chartDaily });
    const grid = svg('g', { class: 'grid' }, root);
    for (const tick of scale.ticks) {
      const at = y(tick);
      svg('line', { x1: pad.left, x2: width - pad.right, y1: at, y2: at }, grid);
      const label = svg('text', { x: pad.left - 8, y: at + 4, 'text-anchor': 'end' }, grid);
      label.textContent = metric === 'cost' ? core.formatUsd(tick) : core.compactTokens(tick);
    }

    const hover = svg('rect', { class: 'hover-band', y: pad.top, height: plotH, width: band, x: -999 }, root);
    const bars = svg('g', { class: 'bars' }, root);
    const stride = Math.max(1, Math.ceil(series.length / (width < 520 ? 4 : 7)));
    series.forEach((day, index) => {
      const cx = pad.left + band * index + band / 2;
      const x = cx - barW / 2;
      if (metric === 'cost') {
        if (day.usd > 0) {
          const top = y(day.usd);
          svg('rect', { x, y: top, width: barW, height: Math.max(1, pad.top + plotH - top), rx: Math.min(3, barW / 3), fill: 'var(--chart-1)' }, bars);
        }
      } else {
        let base = pad.top + plotH;
        const parts = SERIES.filter((entry) => day[entry.key] > 0);
        parts.forEach((entry, part) => {
          const h = (day[entry.key] / scale.max) * plotH;
          const top = base - h;
          svg('rect', {
            x,
            y: top,
            width: barW,
            height: Math.max(part === parts.length - 1 ? 1 : 0, h),
            rx: part === parts.length - 1 ? Math.min(3, barW / 3) : 0,
            fill: entry.colour,
          }, bars);
          base = top;
        });
      }
      if (index % stride === (series.length - 1) % stride) {
        const label = svg('text', { x: cx, y: height - 6, 'text-anchor': 'middle', class: 'x-label' }, root);
        label.textContent = dayLabel(day.date);
      }
      const hit = svg('rect', { x: pad.left + band * index, y: 0, width: band, height, class: 'hit' }, root);
      const enter = () => {
        hover.setAttribute('x', pad.left + band * index);
        showTip(day, cx, width);
      };
      hit.addEventListener('pointerenter', enter);
      hit.addEventListener('pointerdown', enter);
    });
    root.addEventListener('pointerleave', () => {
      hover.setAttribute('x', -999);
      tip.hidden = true;
    });
    svg('line', { class: 'baseline', x1: pad.left, x2: width - pad.right, y1: pad.top + plotH, y2: pad.top + plotH }, root);
    dom.daily.replaceChildren(root, tip);
  }

  function drawHours() {
    const hours = (history && history.hours) || new Array(24).fill(0);
    const max = Math.max(...hours, 0);
    dom.hours.replaceChildren();
    hours.forEach((value, hour) => {
      const cell = h('span', 'hour');
      const fill = h('span', 'hour-fill');
      const share = max ? value / max : 0;
      fill.style.height = `${Math.max(share * 100, value ? 6 : 0)}%`;
      fill.style.opacity = String(0.35 + share * 0.65);
      cell.title = `${String(hour).padStart(2, '0')}:00 · ${core.compactTokens(value)}`;
      cell.append(fill);
      if (hour % 6 === 0) cell.append(h('span', 'hour-label', String(hour)));
      dom.hours.append(cell);
    });
    dom.hoursNote.textContent = core.fill(T.chartHoursNote, { z: (history && history.timezone) || zone });
  }

  function drawBars(host, list, label, value) {
    host.replaceChildren();
    if (!list.length) {
      host.append(h('li', 'bars-empty', '—'));
      return;
    }
    for (const item of core.shareBars(list)) {
      const li = h('li', 'bar-row');
      const head = h('div', 'bar-head');
      const name = h('span', 'bar-name', label(item));
      name.title = name.textContent;
      head.append(name, h('span', 'bar-value', value(item)));
      const track = h('div', 'bar-track');
      const fill = h('span', 'bar-fill');
      fill.style.width = `${Math.max(item.share * 100, 1.5)}%`;
      track.append(fill);
      li.append(head, track);
      if (item.agents && item.agents.length) {
        const chips = h('div', 'bar-tags');
        for (const agent of item.agents) chips.append(h('span', 'chip', agent));
        li.append(chips);
      }
      host.append(li);
    }
  }

  function draw() {
    drawControls();
    const today = core.todayIn((history && history.timezone) || zone);
    const series = core.fillDays(history ? history.daily : [], days, today);
    const label = core.fill('{a} · {z}', { a: T[`range${days}`], z: (history && history.timezone) || zone });
    dom.sub.textContent = label;
    drawStats(series);
    drawLegend();
    drawDaily(series);
    drawHours();
    drawBars(dom.models, (history && history.models) || [], (item) => item.model,
      (item) => (item.usd > 0 ? `${core.compactTokens(item.tokens)} · ${core.formatUsd(item.usd)}` : core.compactTokens(item.tokens)));
    drawBars(dom.agentBars, (history && history.agents) || [], (item) => item.name, (item) => core.compactTokens(item.tokens));
    drawBars(dom.providerBars, (history && history.providers) || [], (item) => item.name, (item) => core.compactTokens(item.tokens));

    const empty = history && !(history.totals && history.totals.tokens);
    const priced = history && history.pricingSnapshot ? core.fill(T.pricedWith, { s: history.pricingSnapshot }) : '';
    dom.note.textContent = empty ? T.historyEmpty : priced;
    dom.note.hidden = !dom.note.textContent;
  }

  /* -------------------------------------------------------------- wire */

  shell.on('page', (page) => {
    if (page === 'usage') load();
  });
  // The account report refreshes every minute while the page is open; the
  // history follows it, and the hub's cache keeps that to one read a minute.
  shell.on('usage', () => {
    if (document.body.dataset.pane === 'usage') load();
  });

  let resizeTimer = 0;
  const redraw = () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      if (document.body.dataset.pane === 'usage') draw();
    }, 120);
  };
  if (window.ResizeObserver) new window.ResizeObserver(redraw).observe(dom.daily);
  else window.addEventListener('resize', redraw);

  // Charts re-read their colours from tokens; a theme change only needs a
  // redraw for the ones computed in script, which today is none - but the
  // hook is here so a mod that changes tokens sees the charts follow.
  shell.on('settings', redraw);

  draw();
})();
