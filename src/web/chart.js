//
// The portfolio time-series graph (market value / unrealised gain over the
// stored daily snapshots) plus the pure range helpers that drive its date
// controls. The SVG is built directly (no build step, no chart library): two
// polylines over the snapshot dates, with stale points hollow and provisional
// (fallback-FX) points ringed.
//
// `presetRange`/`sliceSeries` are pure functions (no DOM) so they are
// unit-tested directly by chart.test.js; `svgEl`/`seriesChart` need a DOM and
// are exercised indirectly via the served-bundle assertions in web.rs.
//
import { el } from './util.js';

export function svgEl(tag, attrs) {
  const n = document.createElementNS('http://www.w3.org/2000/svg', tag);
  if (attrs) for (const k in attrs) { if (attrs[k] != null) n.setAttribute(k, attrs[k]); }
  return n;
}

export function seriesChart(points) {
  if (!points || points.length < 2) {
    return el('p', { class: 'hint' }, 'The graph appears once two or more daily snapshots are stored.');
  }
  const W = 860, H = 280, padL = 84, padR = 16, padT = 12, padB = 30;
  const xs = points.map(function (p) { return new Date(p.snapshot_date + 'T00:00:00Z').getTime(); });
  let yMin = 0, yMax = 1;
  points.forEach(function (p) {
    [Number(p.market_value), Number(p.unrealised_gain), 0].forEach(function (v) {
      if (v < yMin) yMin = v;
      if (v > yMax) yMax = v;
    });
  });
  const xMin = xs[0], xMax = xs[xs.length - 1];
  const x = function (t) { return padL + (t - xMin) / (xMax - xMin || 1) * (W - padL - padR); };
  const y = function (v) { return H - padB - (v - yMin) / (yMax - yMin) * (H - padT - padB); };
  const chart = svgEl('svg', { viewBox: '0 0 ' + W + ' ' + H, class: 'series-chart', role: 'img' });
  // Horizontal gridlines with AUD labels.
  for (let i = 0; i <= 4; i++) {
    const v = yMin + (yMax - yMin) * i / 4;
    chart.appendChild(svgEl('line', { x1: padL, x2: W - padR, y1: y(v), y2: y(v), class: 'grid' }));
    const label = svgEl('text', { x: padL - 6, y: y(v) + 4, 'text-anchor': 'end', class: 'axis' });
    label.textContent = Math.round(v).toLocaleString();
    chart.appendChild(label);
  }
  // First and last snapshot dates on the x axis.
  [0, points.length - 1].forEach(function (i) {
    const label = svgEl('text', {
      x: x(xs[i]), y: H - 8, 'text-anchor': i === 0 ? 'start' : 'end', class: 'axis',
    });
    label.textContent = points[i].snapshot_date;
    chart.appendChild(label);
  });
  // One line + point markers per series; a stale snapshot's point is hollow,
  // a provisional one's (valued at a fallback-month FX rate) has a dashed
  // ring, and one valued at a carried-forward close (a listing the provider
  // stopped quoting) is hollow-square-ish via its own class — each says why
  // the point may not be what a live report would show.
  [['market_value', 'line-mv'], ['unrealised_gain', 'line-ug']].forEach(function (s) {
    const field = s[0], klass = s[1];
    const path = points.map(function (p, i) { return x(xs[i]) + ',' + y(Number(p[field])); }).join(' ');
    chart.appendChild(svgEl('polyline', { points: path, class: klass, fill: 'none' }));
    points.forEach(function (p, i) {
      const dot = svgEl('circle', {
        cx: x(xs[i]), cy: y(Number(p[field])), r: 3,
        class: klass + (p.stale ? ' stale' : '') + (p.provisional ? ' provisional' : '')
          + (p.price_carried_forward ? ' carried' : ''),
      });
      const tip = svgEl('title');
      tip.textContent = p.snapshot_date + ': ' + p[field]
        + (p.stale ? ' (stale)' : '') + (p.provisional ? ' (provisional)' : '')
        + (p.price_carried_forward ? ' (carried-forward price)' : '');
      dot.appendChild(tip);
      chart.appendChild(dot);
    });
  });
  return el('div', null, [
    chart,
    el('p', { class: 'hint' }, [
      el('span', { class: 'legend-mv' }, '— market value'),
      ' ',
      el('span', { class: 'legend-ug' }, '— unrealised gain'),
      ' (AUD; hollow points are stale snapshots, dashed rings provisional FX, '
      + 'amber rings a carried-forward price)',
    ]),
  ]);
}

// ---- range helpers (pure — no DOM) ---------------------------------------

// 'YYYY-MM-DD' `n` months on from `dateStr` (n may be negative), via UTC
// calendar arithmetic — matches the plain-date snapshot_date strings, no
// timezone involved.
function addMonths(dateStr, n) {
  const d = new Date(dateStr + 'T00:00:00Z');
  d.setUTCMonth(d.getUTCMonth() + n);
  return d.toISOString().slice(0, 10);
}

// The Australian financial year's start (1 July) for the FY containing
// `dateStr` — July counts as the *next* FY, same rule as the backend's
// `domain::tax_year::tax_year_for`.
function fyStart(dateStr) {
  const d = new Date(dateStr + 'T00:00:00Z');
  const y = d.getUTCFullYear();
  const m = d.getUTCMonth() + 1; // 1-12
  const startYear = m >= 7 ? y : y - 1;
  return startYear + '-07-01';
}

// A quick-select range over `series` (ascending by `snapshot_date`), ending
// at the series' own latest stored date — never `today`, so a preset can
// never select a range past the last snapshot — and clamped so `from` never
// precedes the series' earliest stored date. `null` for an empty series.
export function presetRange(series, preset) {
  if (!series || series.length === 0) return null;
  const to = series[series.length - 1].snapshot_date;
  const earliest = series[0].snapshot_date;
  let from;
  switch (preset) {
    case '1m': from = addMonths(to, -1); break;
    case '3m': from = addMonths(to, -3); break;
    case '6m': from = addMonths(to, -6); break;
    case '1y': from = addMonths(to, -12); break;
    case '2y': from = addMonths(to, -24); break;
    case '3y': from = addMonths(to, -36); break;
    case 'fytd': from = fyStart(to); break;
    case 'all':
    default: from = earliest; break;
  }
  if (from < earliest) from = earliest;
  return { from: from, to: to };
}

// The stored points with `snapshot_date` in `[from, to]` (both inclusive —
// a display concern distinct from the period-performance report's `(from,
// to]` accounting convention).
export function sliceSeries(series, from, to) {
  return (series || []).filter(function (p) {
    return p.snapshot_date >= from && p.snapshot_date <= to;
  });
}
