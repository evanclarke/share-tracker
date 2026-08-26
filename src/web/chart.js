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
import { el, moneyText, groupThousands } from './util.js';

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

// One y-axis tick label: whole-dollar rounding (an axis label needs no
// cents), thousands-grouped by util.js's own `groupThousands` — never
// `Number.toLocaleString`, whose grouping and separator follow the browser
// locale and so disagreed with the tooltip (moneyText) and every table cell
// in any non-en locale (a de-DE browser drew `1.234` on the axis against
// `1,234` in the tooltip). Pure (no DOM), unit-tested by chart.test.js.
export function tickLabel(v) {
  return groupThousands(String(Math.round(v)));
}

export function svgEl(tag, attrs) {
  const n = document.createElementNS('http://www.w3.org/2000/svg', tag);
  if (attrs) for (const k in attrs) { if (attrs[k] != null) n.setAttribute(k, attrs[k]); }
  return n;
}

// The viewBox width to build the plot at, from the holder's measured pixel
// width. The viewBox is *not* a fixed constant, because the SVG scales
// uniformly (width:100%, height:auto): a fixed 860-wide drawing stretched
// across a wide window would grow its height, its axis type and its stroke
// weights along with it, which is what the old `max-width` ceiling existed to
// prevent. Building the viewBox at the measured width instead pins the scale
// factor at ~1, so extra window width becomes horizontal room for the series
// while the type and line weights keep their designed sizes.
//
// `CHART_WIDTH_FALLBACK` (the historic 860) covers a holder that has not been
// laid out yet — the first draw happens before the panel is attached to the
// document, where clientWidth is 0 — and the floor keeps a narrow viewport from
// collapsing the plot into its own left padding (there, the SVG's width:100%
// scales the floor-width drawing down, as it did at every width before).
export const CHART_WIDTH_FALLBACK = 860;
export const CHART_WIDTH_MIN = 480;
export function chartWidth(measured) {
  const w = Math.round(Number(measured));
  if (!Number.isFinite(w) || w <= 0) return CHART_WIDTH_FALLBACK;
  return Math.max(CHART_WIDTH_MIN, w);
}

// The x axis is a calendar grid rather than a per-point one: a faint vertical
// line every week, dated on as many of those lines as the plot is wide enough
// to carry. The weeks are counted back from the *latest* snapshot, so the
// right-hand edge is always a dated line and every line falls on that
// snapshot's own weekday — which, for a daily series, is a trading day.
export const WEEK_MS = 7 * 24 * 60 * 60 * 1000;
// The room a date label needs before the next one starts. A 'YYYY-MM-DD'
// measures 66.1px at the axis font size (measured in the browser, not
// estimated — every one is ten digits and two hyphens, so the figure barely
// varies), leaving ~8px of clearance. Labels thin out a week at a time until
// they clear it: weekly, fortnightly, four-weekly (the "monthly" step — kept a
// multiple of a week so every label still sits on a gridline and the spacing
// stays even), and on past that for a long range on a narrow plot. The
// clearance is deliberately slim: it is what lets a ~610px plot of a
// six-month range keep four-weekly dates (76px apart) instead of dropping to
// eight-weekly.
export const MIN_LABEL_GAP = 74;
// Below this the weekly lines stop reading as a grid and start reading as a
// grey wash, so the grid itself climbs the same ladder. Only a long range on a
// narrow plot gets there — a 3-year preset is ~156 weeks, which is under 8px a
// week from about 1250px down.
export const MIN_GRID_GAP = 8;
// Half a rendered 'YYYY-MM-DD' (66.1px, see MIN_LABEL_GAP). Every date is centred on its gridline —
// including the closing one — so the plot's right padding is derived from this
// rather than set independently: an end-anchored closing label would reach a
// whole half-label further left than the even spacing `MIN_LABEL_GAP` assumes
// and collide with the date before it.
export const DATE_LABEL_HALF = 33;

// 'YYYY-MM-DD' for a UTC timestamp — the inverse of the `snapshot_date +
// 'T00:00:00Z'` parse the x scale is built on, so a tick landing on a snapshot
// date prints that date back exactly.
export function isoDate(t) {
  return new Date(t).toISOString().slice(0, 10);
}

// The x-axis ticks for a plot `plotWidth` px wide spanning [tMin, tMax] (UTC
// ms). Ascending `{ t, label }`, `label` null for a gridline that carries no
// date. Both intervals are whole weeks: the gridline step is one week unless
// that would draw them closer than `MIN_GRID_GAP`, and the label step is the
// first multiple of the gridline step that clears `MIN_LABEL_GAP`.
export function weekTicks(tMin, tMax, plotWidth) {
  const span = tMax - tMin;
  const perWeek = span > 0 ? plotWidth / (span / WEEK_MS) : plotWidth;
  let gridWeeks = 1;
  while (perWeek * gridWeeks < MIN_GRID_GAP) gridWeeks *= 2;
  let labelWeeks = gridWeeks;
  while (perWeek * labelWeeks < MIN_LABEL_GAP) labelWeeks *= 2;
  const ticks = [];
  for (let k = 0; ; k++) {
    const t = tMax - k * gridWeeks * WEEK_MS;
    if (t < tMin) break;
    ticks.push({ t: t, label: k % (labelWeeks / gridWeeks) === 0 ? isoDate(t) : null });
  }
  ticks.reverse();
  // A span under two grid steps (a handful of days, or a preset narrower than
  // a stepped-up grid) would leave the axis with a single line and a single
  // date: name both ends instead, which is what the axis did before it had a
  // grid at all.
  if (ticks.length < 2) {
    return [{ t: tMin, label: isoDate(tMin) }, { t: tMax, label: isoDate(tMax) }];
  }
  return ticks;
}

// `fieldKey` names which of `SERIES_FIELDS` to plot (see seriesField), and
// `measuredWidth` the pixel width to build it at (see chartWidth).
export function seriesChart(points, fieldKey, measuredWidth) {
  const series = seriesField(fieldKey);
  if (!points || points.length < 2) {
    return el('p', { class: 'hint' }, 'The graph appears once two or more daily snapshots are stored.');
  }
  const W = chartWidth(measuredWidth), H = 280;
  const padL = 84, padR = DATE_LABEL_HALF + 6, padT = 12, padB = 30;
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
    label.textContent = tickLabel(v);
    chart.appendChild(label);
  }
  // With the axis no longer anchored at zero, a series that crosses zero (an
  // unrealised gain turning into a loss) needs the crossing drawn, or the
  // sign change is invisible.
  if (yMin < 0 && yMax > 0) {
    chart.appendChild(svgEl('line', { x1: padL, x2: W - padR, y1: y(0), y2: y(0), class: 'zero' }));
  }
  // The x axis: a weekly gridline, dated where the plot is wide enough to
  // carry the date (weekTicks). Drawn before the series, so the line and its
  // points sit over the grid.
  weekTicks(xMin, xMax, W - padL - padR).forEach(function (tick) {
    const xp = x(tick.t);
    chart.appendChild(svgEl('line', { x1: xp, x2: xp, y1: padT, y2: H - padB, class: 'grid' }));
    if (!tick.label) return;
    // Centred on the line, every one of them: `padR` leaves the closing date
    // its right half, and a leftmost date's left half reaches into the y-axis
    // gutter, which is empty at the axis's own baseline.
    const label = svgEl('text', { x: xp, y: H - 8, class: 'axis', 'text-anchor': 'middle' });
    label.textContent = tick.label;
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
