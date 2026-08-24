//
// The portfolio time-series graph (market value *or* unrealised gain over the
// stored daily snapshots) plus the pure range/scale helpers that drive its
// controls. The SVG is built directly (no build step, no chart library): one
// polyline over the snapshot dates, with stale points hollow and provisional
// (fallback-FX) points ringed.
//
// One series at a time, chosen by the panel's selector: market value and
// unrealised gain differ by an order of magnitude, so sharing one axis drew
// the smaller of the two as a flat line along the bottom of the plot and
// squashed the larger one's movement into a few pixels.
//
// `presetRange`/`sliceSeries`/`yBounds`/`seriesField` are pure functions (no
// DOM) so they are unit-tested directly by chart.test.js; `svgEl`/`seriesChart`
// need a DOM and are exercised indirectly via the served-bundle assertions in
// web.rs.
//
import { el, moneyText } from './util.js';

// The series the graph can plot, in selector order. `klass` styles the line
// and its point markers, `legendClass` the legend swatch.
export const SERIES_FIELDS = [
  { key: 'market_value', label: 'Market value', klass: 'line-mv', legendClass: 'legend-mv' },
  { key: 'unrealised_gain', label: 'Unrealised gain', klass: 'line-ug', legendClass: 'legend-ug' },
];

// The `SERIES_FIELDS` entry for `key`, falling back to the first (market
// value) for an unknown one — a remembered preference from an older build,
// or a cleared localStorage, selects the default rather than drawing nothing.
export function seriesField(key) {
  for (let i = 0; i < SERIES_FIELDS.length; i++) {
    if (SERIES_FIELDS[i].key === key) return SERIES_FIELDS[i];
  }
  return SERIES_FIELDS[0];
}

// The y-axis range for `field` over `points`: the plotted extremes with 10%
// of their span as headroom above and below.
//
// The axis deliberately does **not** include zero — anchoring it there is
// what made the lines read as flat — and the padding is 10% of the *span*,
// not of the values: 10% of a six-figure market value would re-flatten the
// very movement the zoom exists to show. A flat series (zero span, one
// repeated value) falls back to 10% of its own magnitude, so the line sits
// mid-plot instead of dividing by a zero range; an empty or all-non-numeric
// series gets an arbitrary 0..1 axis, which only the <2-point hint can see.
export function yBounds(points, field) {
  let lo = Infinity, hi = -Infinity;
  (points || []).forEach(function (p) {
    // `Number(null)` and `Number('')` are 0, and a spurious zero would drag
    // the axis back to the baseline this scaling exists to leave — so a
    // missing value is skipped before the conversion, not after it.
    if (p == null || p[field] == null || p[field] === '') return;
    const v = Number(p[field]);
    if (!Number.isFinite(v)) return;
    if (v < lo) lo = v;
    if (v > hi) hi = v;
  });
  if (lo === Infinity) return { min: 0, max: 1 };
  const pad = hi > lo ? (hi - lo) * 0.1 : Math.max(Math.abs(hi) * 0.1, 1);
  return { min: lo - pad, max: hi + pad };
}

// The qualifications on one plotted point, as tooltip prose: why the value
// may not be what a live report would show. The excluded note matters most —
// the line **steps** where the omitted listing's price series begins, and
// the step is a change in what is measured, not in value.
export function pointNotes(p) {
  const notes = [];
  if (p.stale) notes.push('stale snapshot');
  if (p.provisional) notes.push('provisional FX');
  if (p.price_carried_forward) notes.push('carried-forward price');
  if (p.holding_excluded) {
    notes.push('omits ' + (p.excluded_holdings || []).map(function (h) { return h.ticker; }).join(', '));
  }
  return notes.join('; ');
}

export function svgEl(tag, attrs) {
  const n = document.createElementNS('http://www.w3.org/2000/svg', tag);
  if (attrs) for (const k in attrs) { if (attrs[k] != null) n.setAttribute(k, attrs[k]); }
  return n;
}

// `fieldKey` names which of `SERIES_FIELDS` to plot (see seriesField).
export function seriesChart(points, fieldKey) {
  const series = seriesField(fieldKey);
  if (!points || points.length < 2) {
    return el('p', { class: 'hint' }, 'The graph appears once two or more daily snapshots are stored.');
  }
  const W = 860, H = 280, padL = 84, padR = 16, padT = 12, padB = 30;
  const xs = points.map(function (p) { return new Date(p.snapshot_date + 'T00:00:00Z').getTime(); });
  const bounds = yBounds(points, series.key);
  const yMin = bounds.min, yMax = bounds.max;
  const xMin = xs[0], xMax = xs[xs.length - 1];
  const x = function (t) { return padL + (t - xMin) / (xMax - xMin || 1) * (W - padL - padR); };
  const y = function (v) { return H - padB - (v - yMin) / (yMax - yMin || 1) * (H - padT - padB); };
  const chart = svgEl('svg', {
    viewBox: '0 0 ' + W + ' ' + H, class: 'series-chart', role: 'img',
    'aria-label': series.label + ' over time',
  });
  // Horizontal gridlines with AUD labels.
  for (let i = 0; i <= 4; i++) {
    const v = yMin + (yMax - yMin) * i / 4;
    chart.appendChild(svgEl('line', { x1: padL, x2: W - padR, y1: y(v), y2: y(v), class: 'grid' }));
    const label = svgEl('text', { x: padL - 6, y: y(v) + 4, 'text-anchor': 'end', class: 'axis' });
    label.textContent = Math.round(v).toLocaleString();
    chart.appendChild(label);
  }
  // With the axis no longer anchored at zero, a series that crosses zero (an
  // unrealised gain turning into a loss) needs the crossing drawn, or the
  // sign change is invisible.
  if (yMin < 0 && yMax > 0) {
    chart.appendChild(svgEl('line', { x1: padL, x2: W - padR, y1: y(0), y2: y(0), class: 'zero' }));
  }
  // First and last snapshot dates on the x axis.
  [0, points.length - 1].forEach(function (i) {
    const label = svgEl('text', {
      x: x(xs[i]), y: H - 8, 'text-anchor': i === 0 ? 'start' : 'end', class: 'axis',
    });
    label.textContent = points[i].snapshot_date;
    chart.appendChild(label);
  });
  // The hover tooltip: an absolutely-positioned HTML box over the plot, so
  // the value reads in the page's own font and needs no text measuring. It
  // is placed in percentages of the viewBox, which map linearly to the
  // rendered box because the SVG scales uniformly (width:100%, height:auto).
  const tip = el('div', { class: 'chart-tip' });
  tip.hidden = true;
  function showTip(p, cx, cy) {
    tip.innerHTML = '';
    const notes = pointNotes(p);
    tip.appendChild(el('div', { class: 'chart-tip-date' }, p.snapshot_date));
    tip.appendChild(el('div', { class: 'chart-tip-value' }, moneyText(p[series.key]) + ' AUD'));
    if (notes) tip.appendChild(el('div', { class: 'chart-tip-note' }, notes));
    tip.style.left = (cx / W * 100) + '%';
    tip.style.top = (cy / H * 100) + '%';
    // Near an edge the centred box would overflow the plot, so anchor it to
    // the point's left or right instead of over it.
    tip.classList.toggle('tip-right', cx < W * 0.25);
    tip.classList.toggle('tip-left', cx > W * 0.75);
    tip.hidden = false;
  }
  function hideTip() { tip.hidden = true; }
  // The line, its point markers, and one invisible over-sized hit target per
  // point (a 3px dot is hard to hover). A stale snapshot's point is hollow,
  // a provisional one's (valued at a fallback-month FX rate) has a dashed
  // ring, one valued at a carried-forward close (a listing the provider
  // stopped quoting) an amber ring, and one whose totals *omit* a holding
  // (no price obtainable before the provider's series began) a red one.
  const klass = series.klass;
  const path = points.map(function (p, i) { return x(xs[i]) + ',' + y(Number(p[series.key])); }).join(' ');
  chart.appendChild(svgEl('polyline', { points: path, class: klass, fill: 'none' }));
  points.forEach(function (p, i) {
    const cx = x(xs[i]), cy = y(Number(p[series.key]));
    chart.appendChild(svgEl('circle', {
      cx: cx, cy: cy, r: 3,
      class: klass + (p.stale ? ' stale' : '') + (p.provisional ? ' provisional' : '')
        + (p.price_carried_forward ? ' carried' : '')
        + (p.holding_excluded ? ' excluded' : ''),
    }));
    const hit = svgEl('circle', { cx: cx, cy: cy, r: 10, class: 'hit' });
    hit.addEventListener('mouseenter', function () { showTip(p, cx, cy); });
    hit.addEventListener('mouseleave', hideTip);
    // Touch has no hover: a tap shows the same box, and a tap elsewhere on
    // the plot dismisses it.
    hit.addEventListener('click', function (ev) { ev.stopPropagation(); showTip(p, cx, cy); });
    chart.appendChild(hit);
  });
  const plot = el('div', { class: 'chart-plot' }, [chart, tip]);
  plot.addEventListener('click', hideTip);
  return el('div', null, [
    plot,
    el('p', { class: 'hint' }, [
      el('span', { class: series.legendClass }, '— ' + series.label),
      ' (AUD; hover a point for its value. Hollow points are stale snapshots, '
      + 'dashed rings provisional FX, amber rings a carried-forward price, '
      + 'red rings a total that omits a holding)',
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
