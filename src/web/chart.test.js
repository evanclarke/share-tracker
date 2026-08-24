//
// Unit tests for chart.js's pure range helpers (presetRange, sliceSeries) —
// svgEl/seriesChart need a DOM and are exercised indirectly via the
// served-bundle assertions in web.rs. Run with Node's built-in runner:
//
//   node --test 'src/web/*.test.js'
//
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { presetRange, sliceSeries, yBounds, seriesField, pointNotes, SERIES_FIELDS } from './chart.js';

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
