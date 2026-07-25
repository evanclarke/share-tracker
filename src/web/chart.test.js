//
// Unit tests for chart.js's pure range helpers (presetRange, sliceSeries) —
// svgEl/seriesChart need a DOM and are exercised indirectly via the
// served-bundle assertions in web.rs. Run with Node's built-in runner:
//
//   node --test 'src/web/*.test.js'
//
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { presetRange, sliceSeries } from './chart.js';

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
