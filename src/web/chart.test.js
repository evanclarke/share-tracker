//
// Unit tests for chart.js's pure range helpers (presetRange, sliceSeries) —
// svgEl/seriesChart need a DOM and are exercised indirectly via the
// served-bundle assertions in web.rs. Run with Node's built-in runner:
//
//   node --test 'src/web/*.test.js'
//
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  presetRange, sliceSeries, yBounds, seriesField, pointNotes, tickLabel, chartWidth,
  weekTicks, isoDate, WEEK_MS, MIN_LABEL_GAP, MIN_GRID_GAP,
  SERIES_FIELDS, CHART_WIDTH_FALLBACK, CHART_WIDTH_MIN,
} from './chart.js';
import { groupThousands } from './util.js';

function series(dates) {
  return dates.map(function (d) { return { snapshot_date: d, market_value: '100', unrealised_gain: '10' }; });
}

// ---- presetRange ----------------------------------------------------------

test('presetRange: "all" spans the whole stored series', () => {
  const s = series(['2025-01-01', '2025-06-15', '2026-07-25']);
  assert.deepEqual(presetRange(s, 'all'), { from: '2025-01-01', to: '2026-07-25' });
});

test('presetRange: month-based presets end at the series\' own latest date, not today', () => {
  const s = series(['2020-01-01', '2026-04-25', '2026-07-25']);
  assert.deepEqual(presetRange(s, '1m'), { from: '2026-06-25', to: '2026-07-25' });
  assert.deepEqual(presetRange(s, '3m'), { from: '2026-04-25', to: '2026-07-25' });
  assert.deepEqual(presetRange(s, '6m'), { from: '2026-01-25', to: '2026-07-25' });
  assert.deepEqual(presetRange(s, '1y'), { from: '2025-07-25', to: '2026-07-25' });
  assert.deepEqual(presetRange(s, '2y'), { from: '2024-07-25', to: '2026-07-25' });
  assert.deepEqual(presetRange(s, '3y'), { from: '2023-07-25', to: '2026-07-25' });
});

test('presetRange: fytd is 1 July of the FY containing the latest date (July counts as the next FY)', () => {
  // 2026-07-25 is in FY2027 (ends 30 June 2027) → FY start 2026-07-01.
  assert.deepEqual(
    presetRange(series(['2020-01-01', '2026-07-25']), 'fytd'),
    { from: '2026-07-01', to: '2026-07-25' },
  );
  // 2026-05-10 is in FY2026 (ends 30 June 2026) → FY start 2025-07-01.
  assert.deepEqual(
    presetRange(series(['2020-01-01', '2026-05-10']), 'fytd'),
    { from: '2025-07-01', to: '2026-05-10' },
  );
});

test('presetRange: a preset never precedes the series\' earliest stored date', () => {
  const s1y = series(['2026-06-01', '2026-06-15', '2026-07-01']);
  assert.deepEqual(presetRange(s1y, '1y'), { from: '2026-06-01', to: '2026-07-01' });
  assert.deepEqual(presetRange(s1y, '2y'), { from: '2026-06-01', to: '2026-07-01' });
  assert.deepEqual(presetRange(s1y, '3y'), { from: '2026-06-01', to: '2026-07-01' });

  // Latest date 2026-05-10 is in FY2026 (start 2025-07-01), which precedes
  // the series' earliest stored date (2026-01-01) — clamp, don't reach
  // before the series began.
  const sFy = series(['2026-01-01', '2026-05-10']);
  assert.deepEqual(presetRange(sFy, 'fytd'), { from: '2026-01-01', to: '2026-05-10' });
});

test('presetRange: unknown preset falls back to "all"', () => {
  const s = series(['2025-01-01', '2026-07-25']);
  assert.deepEqual(presetRange(s, 'nonsense'), { from: '2025-01-01', to: '2026-07-25' });
});

test('presetRange: null for an empty or missing series', () => {
  assert.equal(presetRange([], '1m'), null);
  assert.equal(presetRange(null, '1m'), null);
});

// ---- sliceSeries ------------------------------------------------------------

test('sliceSeries: keeps points with snapshot_date within [from, to], both inclusive', () => {
  const s = series(['2026-06-01', '2026-06-15', '2026-07-01', '2026-07-15']);
  const sliced = sliceSeries(s, '2026-06-15', '2026-07-01');
  assert.deepEqual(sliced.map(function (p) { return p.snapshot_date; }), ['2026-06-15', '2026-07-01']);
});

test('sliceSeries: empty result when the range matches nothing', () => {
  const s = series(['2026-06-01', '2026-07-01']);
  assert.deepEqual(sliceSeries(s, '2020-01-01', '2020-01-02'), []);
});

test('sliceSeries: tolerates a missing series', () => {
  assert.deepEqual(sliceSeries(null, '2026-01-01', '2026-02-01'), []);
});

// ---- yBounds ----------------------------------------------------------------

test('yBounds: pads the plotted extremes by 10% of their span, not by 10% of the values', () => {
  // A market-value series that moves 20k on a 500k base: padding by 10% of
  // the *value* (450k..550k) would leave the line in the middle 20% of the
  // plot — the flat graph this replaced. 10% of the span keeps it filling it.
  const s = [{ market_value: '500000' }, { market_value: '510000' }, { market_value: '520000' }];
  assert.deepEqual(yBounds(s, 'market_value'), { min: 498000, max: 522000 });
});

test('yBounds: the axis is not anchored at zero', () => {
  const s = [{ market_value: '100' }, { market_value: '200' }];
  const b = yBounds(s, 'market_value');
  assert.equal(b.min, 90);
  assert.equal(b.max, 210);
});

test('yBounds: spans a series that crosses zero', () => {
  const s = [{ unrealised_gain: '-500' }, { unrealised_gain: '1500' }];
  assert.deepEqual(yBounds(s, 'unrealised_gain'), { min: -700, max: 1700 });
});

test('yBounds: a flat series falls back to 10% of its own magnitude, never a zero-height axis', () => {
  const flat = yBounds([{ market_value: '400' }, { market_value: '400' }], 'market_value');
  assert.deepEqual(flat, { min: 360, max: 440 });
  // …and a flat series *at* zero still gets a usable range.
  assert.deepEqual(
    yBounds([{ unrealised_gain: '0' }, { unrealised_gain: '0' }], 'unrealised_gain'),
    { min: -1, max: 1 },
  );
});

test('yBounds: ignores non-numeric points, and falls back to 0..1 when none are numeric', () => {
  const s = [{ market_value: '100' }, { market_value: null }, { market_value: '300' }];
  assert.deepEqual(yBounds(s, 'market_value'), { min: 80, max: 320 });
  assert.deepEqual(yBounds([{ market_value: null }], 'market_value'), { min: 0, max: 1 });
  assert.deepEqual(yBounds([], 'market_value'), { min: 0, max: 1 });
  assert.deepEqual(yBounds(null, 'market_value'), { min: 0, max: 1 });
});

// ---- seriesField ------------------------------------------------------------

test('seriesField: resolves each configured series, and falls back to market value', () => {
  assert.equal(seriesField('market_value').key, 'market_value');
  assert.equal(seriesField('unrealised_gain').key, 'unrealised_gain');
  // A preference remembered from an older build, or nothing stored at all.
  assert.equal(seriesField('total_cost_base').key, SERIES_FIELDS[0].key);
  assert.equal(seriesField(null).key, 'market_value');
  assert.equal(seriesField(undefined).key, 'market_value');
});

// ---- pointNotes -------------------------------------------------------------

test('pointNotes: an unqualified point carries no note', () => {
  assert.equal(pointNotes({ snapshot_date: '2026-07-01', market_value: '100' }), '');
});

test('pointNotes: names every qualification the point carries, and what a total omits', () => {
  assert.equal(pointNotes({ stale: true }), 'stale snapshot');
  assert.equal(pointNotes({ provisional: true }), 'provisional FX');
  assert.equal(pointNotes({ price_carried_forward: true }), 'carried-forward price');
  assert.equal(
    pointNotes({ holding_excluded: true, excluded_holdings: [{ ticker: 'LAC' }, { ticker: 'LAAC' }] }),
    'omits LAC, LAAC',
  );
  assert.equal(
    pointNotes({ stale: true, provisional: true }),
    'stale snapshot; provisional FX',
  );
});

// ---- tickLabel --------------------------------------------------------------

test('tickLabel: whole dollars with comma thousands grouping, sign kept', () => {
  assert.equal(tickLabel(1234567.89), '1,234,568');
  assert.equal(tickLabel(-98765.4), '-98,765');
  assert.equal(tickLabel(999.2), '999');
  assert.equal(tickLabel(0), '0');
});

test('tickLabel: grouping is util.js\'s own groupThousands, not the browser locale', () => {
  // The axis must read the same as the tooltip and every table cell whatever
  // the browser's locale — so the label is exactly the app's own grouping of
  // the rounded whole-dollar figure, never Number.toLocaleString's.
  [1234567.89, -98765.4, 1000, 12, 0].forEach(function (v) {
    assert.equal(tickLabel(v), groupThousands(String(Math.round(v))));
  });
});

// ---- chartWidth -----------------------------------------------------------

test('chartWidth: builds the viewBox at the measured width, so the SVG renders at ~1 scale', () => {
  assert.equal(chartWidth(1400), 1400);
  assert.equal(chartWidth(2560), 2560);
  // Rounded, since the measurement can be fractional on a scaled display.
  assert.equal(chartWidth(1279.6), 1280);
});

test('chartWidth: an unlaid-out holder (clientWidth 0) falls back to the default width', () => {
  assert.equal(chartWidth(0), CHART_WIDTH_FALLBACK);
  assert.equal(chartWidth(undefined), CHART_WIDTH_FALLBACK);
  assert.equal(chartWidth(null), CHART_WIDTH_FALLBACK);
  assert.equal(chartWidth(NaN), CHART_WIDTH_FALLBACK);
  assert.equal(chartWidth(-100), CHART_WIDTH_FALLBACK);
});

test('chartWidth: a narrow viewport floors at the minimum and scales down instead', () => {
  assert.equal(chartWidth(320), CHART_WIDTH_MIN);
  assert.equal(chartWidth(CHART_WIDTH_MIN + 1), CHART_WIDTH_MIN + 1);
});
// ---- weekTicks ------------------------------------------------------------

// A span of `weeks` ending at 2024-10-02 (a Wednesday), and the plot width
// that gives it `perWeek` pixels a week.
function span(weeks) {
  const tMax = Date.UTC(2024, 9, 2);
  return { tMin: tMax - weeks * WEEK_MS, tMax: tMax };
}
function width(weeks, perWeek) { return weeks * perWeek; }
function labelled(ticks) { return ticks.filter(function (t) { return t.label !== null; }); }

test('weekTicks: gridlines are whole weeks back from the latest snapshot', () => {
  const s = span(4);
  const ticks = weekTicks(s.tMin, s.tMax, width(4, 200));
  assert.deepEqual(ticks.map(function (t) { return t.label; }),
    ['2024-09-04', '2024-09-11', '2024-09-18', '2024-09-25', '2024-10-02']);
  // Ascending, and the last line is the latest snapshot itself.
  assert.equal(ticks[ticks.length - 1].t, s.tMax);
  for (let i = 1; i < ticks.length; i++) assert.equal(ticks[i].t - ticks[i - 1].t, WEEK_MS);
});

test('weekTicks: every week is dated when each week has room for a date', () => {
  const s = span(12);
  const ticks = weekTicks(s.tMin, s.tMax, width(12, 100));
  assert.equal(ticks.length, 13);
  assert.equal(labelled(ticks).length, 13);
});

test('weekTicks: too narrow for weekly dates falls back to fortnightly, weekly lines kept', () => {
  const s = span(12);
  const ticks = weekTicks(s.tMin, s.tMax, width(12, 50));
  assert.equal(ticks.length, 13, 'still a line every week');
  assert.deepEqual(ticks.map(function (t) { return t.label !== null; }),
    [true, false, true, false, true, false, true, false, true, false, true, false, true]);
});

test('weekTicks: narrower again falls back to four-weekly dates, weekly lines kept', () => {
  const s = span(12);
  // 19px a week — the six-month range on a ~610px plot, i.e. the app at its
  // narrowest usable window: four-weekly (76px) still clears the label gap.
  const ticks = weekTicks(s.tMin, s.tMax, width(12, 19));
  assert.equal(ticks.length, 13, 'still a line every week');
  assert.deepEqual(labelled(ticks).map(function (t) { return t.label; }),
    ['2024-07-10', '2024-08-07', '2024-09-04', '2024-10-02']);
});

test('weekTicks: dated lines never sit closer than the label gap', () => {
  [[12, 100], [12, 50], [12, 25], [26, 30], [156, 8], [156, 3]].forEach(function (c) {
    const s = span(c[0]);
    const plot = width(c[0], c[1]);
    const dated = labelled(weekTicks(s.tMin, s.tMax, plot));
    const perMs = plot / (s.tMax - s.tMin);
    for (let i = 1; i < dated.length; i++) {
      assert.ok((dated[i].t - dated[i - 1].t) * perMs >= MIN_LABEL_GAP,
        `weeks=${c[0]} perWeek=${c[1]}: dates crowd`);
    }
  });
});

test('weekTicks: a long range on a narrow plot steps the gridlines up too', () => {
  // 3 years (~156 weeks) across 800px is ~5px a week — a grey wash, so the
  // grid itself doubles until the lines are legible.
  const s = span(156);
  const ticks = weekTicks(s.tMin, s.tMax, 800);
  const perMs = 800 / (s.tMax - s.tMin);
  for (let i = 1; i < ticks.length; i++) {
    assert.equal((ticks[i].t - ticks[i - 1].t) / WEEK_MS, 2, 'fortnightly gridlines');
    assert.ok((ticks[i].t - ticks[i - 1].t) * perMs >= MIN_GRID_GAP);
  }
});

test('weekTicks: a span under two weeks names both ends rather than one lone date', () => {
  const tMax = Date.UTC(2024, 9, 2);
  const tMin = tMax - 4 * 24 * 60 * 60 * 1000;
  assert.deepEqual(weekTicks(tMin, tMax, 800),
    [{ t: tMin, label: '2024-09-28' }, { t: tMax, label: '2024-10-02' }]);
});

test('isoDate: round-trips the plot\'s own UTC date parse', () => {
  assert.equal(isoDate(Date.UTC(2024, 9, 2)), '2024-10-02');
  assert.equal(isoDate(new Date('2025-01-01T00:00:00Z').getTime()), '2025-01-01');
});
