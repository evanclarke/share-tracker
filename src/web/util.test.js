//
// Unit tests for the pure helpers in util.js — the money-adjacent
// decimal-string logic the served-bundle string assertions in web.rs cannot
// execute. Run with Node's built-in runner (Node 22 or newer, no build step,
// no npm install; quote the glob — Node expands it itself):
//
//   node --test 'src/web/*.test.js'
//
// Test files live beside the modules as `src/web/*.test.js`. They are never
// servable: the served bundle is the explicit JS_MODULES allowlist in
// src/web.rs, and a Rust test there pins that no `*.test.js` file is listed
// (and that every non-test module is).
//
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  roundDecimalStr, groupThousands, padMinDp, decStrEq, numericDisplay,
  addDecimalStrings, decParts, mulToCents, frankingCreditFor, decEq,
  looksNumeric, columnKinds, columnLabel, tradeOrigin, periodReturnPct,
  holdingHasActivity, loadPref, savePref, pathSeg,
} from './util.js';

// ---- roundDecimalStr ----------------------------------------------------
test('roundDecimalStr reduces dp, half away from zero', () => {
  assert.equal(roundDecimalStr('1.234', 2), '1.23');
  assert.equal(roundDecimalStr('1.005', 2), '1.01'); // exact half rounds up…
  assert.equal(roundDecimalStr('-1.005', 2), '-1.01'); // …away from zero
  assert.equal(roundDecimalStr('1.0049', 2), '1.00');
  assert.equal(roundDecimalStr('2.5', 0), '3');
});

test('roundDecimalStr carries through on round-up', () => {
  assert.equal(roundDecimalStr('9.999', 2), '10.00');
  assert.equal(roundDecimalStr('99.995', 2), '100.00');
  assert.equal(roundDecimalStr('0.999', 2), '1.00');
});

test('roundDecimalStr pads a dp increase', () => {
  assert.equal(roundDecimalStr('1.5', 2), '1.50');
  assert.equal(roundDecimalStr('7', 2), '7.00');
  assert.equal(roundDecimalStr('+3.1', 2), '3.10'); // explicit plus sign
});

test('roundDecimalStr zero: never a negative zero', () => {
  assert.equal(roundDecimalStr('0', 2), '0.00');
  assert.equal(roundDecimalStr('-0.004', 2), '0.00');
  assert.equal(roundDecimalStr('-0', 2), '0.00');
});

test('roundDecimalStr is exact where float rounding drifts', () => {
  // 0.145 is 0.14499… as a float — toFixed(2) gives '0.14'; exact gives 0.15.
  assert.equal(roundDecimalStr('0.145', 2), '0.15');
  // Beyond Number's 53-bit integer precision.
  assert.equal(roundDecimalStr('90071992547409925.005', 2), '90071992547409925.01');
});

test('roundDecimalStr returns null for non-decimal input', () => {
  for (const bad of ['abc', '1.2.3', '', '1e3', '1,000', '.', '--1']) {
    assert.equal(roundDecimalStr(bad, 2), null, bad);
  }
});

// ---- groupThousands -----------------------------------------------------
test('groupThousands groups the integer part only', () => {
  assert.equal(groupThousands('1234567.89'), '1,234,567.89');
  assert.equal(groupThousands('1000'), '1,000');
  assert.equal(groupThousands('-1234567.8901'), '-1,234,567.8901');
  assert.equal(groupThousands('999.99'), '999.99');
  assert.equal(groupThousands('0.123456'), '0.123456');
});

// ---- padMinDp -----------------------------------------------------------
test('padMinDp pads up to the minimum dp without rounding down', () => {
  assert.equal(padMinDp('1.5', 4), '1.5000');
  assert.equal(padMinDp('3', 4), '3.0000');
  assert.equal(padMinDp('1.23456', 4), '1.23456'); // more precise: untouched
  assert.equal(padMinDp('2.0000', 4), '2.0000');
  assert.equal(padMinDp('-0.1', 4), '-0.1000');
});

// ---- decStrEq -----------------------------------------------------------
test('decStrEq is numeric equality over decimal strings', () => {
  assert.equal(decStrEq('1234.5', '1234.50'), true);
  assert.equal(decStrEq('0', '-0'), true);
  assert.equal(decStrEq('1234.5', '1234.51'), false);
  assert.equal(decStrEq('abc', '1'), false);
});

// ---- numericDisplay kinds -----------------------------------------------
test('numericDisplay money: cent-rounded, grouped, lost precision on tip', () => {
  assert.deepEqual(numericDisplay('1234.5', 'money'), { text: '1,234.50', tip: null });
  assert.deepEqual(numericDisplay('1234.567', 'money'), { text: '1,234.57', tip: '1234.567' });
  assert.deepEqual(numericDisplay('-0.005', 'money'), { text: '-0.01', tip: '-0.005' });
  assert.deepEqual(numericDisplay('0', 'money'), { text: '0.00', tip: null });
});

test('numericDisplay rate/quantity: entered precision verbatim', () => {
  assert.deepEqual(numericDisplay('1234.5678', 'rate'), { text: '1234.5678', tip: null });
  assert.deepEqual(numericDisplay('100', 'quantity'), { text: '100', tip: null });
});

test('numericDisplay rate4: rounded to 4 dp, full value on hover when lost', () => {
  assert.deepEqual(numericDisplay('1.5', 'rate4'), { text: '1.5000', tip: null });
  assert.deepEqual(numericDisplay('1.2345', 'rate4'), { text: '1.2345', tip: null });
  assert.deepEqual(
    numericDisplay('3.3333333333', 'rate4'),
    { text: '3.3333', tip: '3.3333333333' },
  );
});

test('numericDisplay declines non-numeric values and kindless columns', () => {
  assert.equal(numericDisplay('abc', 'money'), null);
  assert.equal(numericDisplay('', 'money'), null);
  assert.equal(numericDisplay(null, 'money'), null);
  assert.equal(numericDisplay(true, 'money'), null);
  assert.equal(numericDisplay('1.5', undefined), null);
});

// ---- addDecimalStrings --------------------------------------------------
test('addDecimalStrings is exact where float addition drifts', () => {
  assert.equal(addDecimalStrings('0.1', '0.2'), '0.3');
  assert.equal(addDecimalStrings('2757.30', '1181.70'), '3939');
});

test('addDecimalStrings mixes scales and trims trailing zeros', () => {
  assert.equal(addDecimalStrings('1', '2'), '3');
  assert.equal(addDecimalStrings('1.05', '2.95'), '4');
  assert.equal(addDecimalStrings('1.005', '2'), '3.005');
});

test('addDecimalStrings handles negative operands and results', () => {
  assert.equal(addDecimalStrings('-1.5', '0.5'), '-1');
  assert.equal(addDecimalStrings('-0.5', '0.2'), '-0.3'); // result in (-1, 0)
  assert.equal(addDecimalStrings('0.5', '-0.5'), '0'); // never '-0'
  assert.equal(addDecimalStrings('-1', '-2.25'), '-3.25');
});

// ---- decParts / mulToCents / frankingCreditFor / decEq -------------------
test('decParts parses non-negative decimals, rejects everything else', () => {
  assert.deepEqual(decParts('123.45'), { units: 12345n, dp: 2 });
  assert.deepEqual(decParts('7'), { units: 7n, dp: 0 });
  for (const bad of ['-1', '', 'abc', '1.2.3', '.5']) {
    assert.equal(decParts(bad), null, bad);
  }
});

test('mulToCents rounds the exact product to the cent, half away from zero', () => {
  // The per-share cross-check the income form runs (demo fixture figures).
  assert.equal(mulToCents('0.14', '19695'), '2757.30');
  assert.equal(mulToCents('1.005', '1'), '1.01');
  assert.equal(mulToCents('0.333', '3'), '1.00');
  assert.equal(mulToCents('abc', '3'), null);
});

test('frankingCreditFor computes amount × 30/70, cent-rounded', () => {
  // The PLS example pinned in the code comment: 2757.30 → 1181.70.
  assert.equal(frankingCreditFor('2757.30'), '1181.70');
  assert.equal(frankingCreditFor('100'), '42.86');
  assert.equal(frankingCreditFor('0'), '0.00');
  assert.equal(frankingCreditFor('nope'), null);
});

test('decEq is numeric equality over the non-negative income figures', () => {
  assert.equal(decEq('1181.7', '1181.70'), true);
  assert.equal(decEq('1', '1.0000'), true);
  assert.equal(decEq('1', '1.01'), false);
  assert.equal(decEq('x', '1'), false);
});

// ---- looksNumeric / columnKinds / columnLabel ----------------------------
test('looksNumeric accepts plain decimals only', () => {
  assert.equal(looksNumeric('1.5'), true);
  assert.equal(looksNumeric('-2'), true);
  assert.equal(looksNumeric('1e3'), false);
  assert.equal(looksNumeric(''), false);
  assert.equal(looksNumeric(null), false);
  assert.equal(looksNumeric(true), false);
});

test('columnKinds maps classified columns and skips the rest', () => {
  assert.deepEqual(
    columnKinds(['total_cost_base', 'average_price', 'avg_cost_base_per_unit', 'quantity', 'id']),
    {
      total_cost_base: 'money',
      average_price: 'rate4',
      avg_cost_base_per_unit: 'rate4',
      quantity: 'quantity',
    },
  );
  // The ESS statement-AUD override columns format as money (label F is an
  // ESS Statements list column).
  assert.deepEqual(columnKinds(['aud_deferral_discount']), { aud_deferral_discount: 'money' });
});

test('columnLabel: overrides win, humaniser handles _id and acronyms', () => {
  assert.equal(columnLabel('exchange_mic'), 'Exchange'); // explicit override
  assert.equal(columnLabel('holding_account_id'), 'Account');
  assert.equal(columnLabel('listing_id'), 'Listing'); // trailing _id dropped
  assert.equal(columnLabel('fx_rate'), 'FX rate'); // acronym casing kept
  assert.equal(columnLabel('gst_on_brokerage'), 'GST on brokerage');
  assert.equal(columnLabel('date_paid'), 'Date paid');
  assert.equal(columnLabel('aud_deferral_discount'), 'Statement AUD deferral (F)');
});

// ---- tradeOrigin ----------------------------------------------------------
test('tradeOrigin: transfer legs name the transfer, Buy flags the cost base', () => {
  // The transfer-in Buy's brokerage column holds the moved parcel's cost
  // base (transfer.rs), so its label must say the figure is not a fee.
  assert.equal(
    tradeOrigin({ trade_type: 'Buy', transfer_id: 3 }),
    'Transfer #3 in (brokerage = carried cost base, not a fee)',
  );
  assert.equal(tradeOrigin({ trade_type: 'Sell', transfer_id: 3 }), 'Transfer #3 out');
});

test('tradeOrigin: rollover Buys carrying cost base on brokerage say so', () => {
  assert.equal(
    tradeOrigin({ trade_type: 'Buy', scrip_action_id: 1 }),
    'Scrip exchange (brokerage = carried cost base, not a fee)',
  );
  assert.equal(tradeOrigin({ trade_type: 'Sell', scrip_action_id: 1 }), 'Scrip exchange');
  assert.equal(
    tradeOrigin({ trade_type: 'Buy', demerger_action_id: 2 }),
    'Demerger (brokerage = carried cost base, not a fee)',
  );
  assert.equal(
    tradeOrigin({ trade_type: 'Buy', rights_action_id: 4 }),
    'Rights exercise (brokerage = rights cost)',
  );
});

test('tradeOrigin: remaining provenance links label plainly', () => {
  assert.equal(tradeOrigin({ trade_type: 'Buy', ess_statement_id: 9 }), 'ESS vest');
  assert.equal(tradeOrigin({ trade_type: 'Buy', inheritance_id: 1 }), 'Inheritance');
  assert.equal(tradeOrigin({ trade_type: 'Sell', buyback_action_id: 5 }), 'Buy-back');
  assert.equal(tradeOrigin({ trade_type: 'Sell', worthless_action_id: 6 }), 'Worthless shares');
});

test('tradeOrigin: ordinary trades have no origin (null links included)', () => {
  assert.equal(tradeOrigin({ trade_type: 'Buy', transfer_id: null, scrip_action_id: null }), '');
  assert.equal(tradeOrigin({ trade_type: 'Sell' }), '');
});

// ---- periodReturnPct --------------------------------------------------------
test('periodReturnPct: a window of a year or less shows the raw total_return_pct', () => {
  const r = { from: '2025-01-01', to: '2025-06-30', total_return_pct: '10.0000', money_weighted_return_pct: '9.5000' };
  assert.deepEqual(periodReturnPct(r), { value: '10.0000', annualized: false });
});

test('periodReturnPct: exactly 365 days still counts as "a year", not "over a year"', () => {
  const r = { from: '2023-01-01', to: '2024-01-01', total_return_pct: '10.0000', money_weighted_return_pct: '9.5000' };
  assert.deepEqual(periodReturnPct(r), { value: '10.0000', annualized: false });
});

test('periodReturnPct: a window over a year shows the annualised money-weighted return', () => {
  // Mirrors reports::period_performance's own test: a mid-window purchase
  // inflates total_return_pct (41.6667%) — money_weighted_return_pct
  // (29.9144%) is the correct annualised figure.
  const r = {
    from: '2025-01-01', to: '2026-01-02', // 366 days: just over the boundary
    total_return_pct: '41.6667', money_weighted_return_pct: '29.9144',
  };
  assert.deepEqual(periodReturnPct(r), { value: '29.9144', annualized: true });
});

test('periodReturnPct: null money_weighted_return_pct falls back to —, not the raw figure', () => {
  const r = { from: '2020-01-01', to: '2023-01-01', total_return_pct: '1716.7300', money_weighted_return_pct: null };
  assert.deepEqual(periodReturnPct(r), { value: null, annualized: true });
});

// ---- holdingHasActivity -----------------------------------------------

function holdingRow(overrides) {
  return Object.assign({
    listing_id: 1, holding_account_id: 1,
    opening_market_value: '0', closing_market_value: '0',
    purchases: '0', sale_proceeds: '0', income: '0',
    capital_growth: '0', fx_movement: '0', total_return: '0',
  }, overrides);
}

test('holdingHasActivity: all-zero row (a holding closed before the period) has no activity', () => {
  assert.equal(holdingHasActivity(holdingRow()), false);
});

test('holdingHasActivity: "0.00" and "-0.00" spellings still count as zero', () => {
  assert.equal(holdingHasActivity(holdingRow({
    opening_market_value: '0.00', closing_market_value: '-0.00', purchases: '0.0',
  })), false);
});

test('holdingHasActivity: income alone makes a holding active', () => {
  assert.equal(holdingHasActivity(holdingRow({ income: '12.50', total_return: '12.50' })), true);
});

test('holdingHasActivity: a flat holding (unchanged value, no trades) is still active', () => {
  assert.equal(holdingHasActivity(holdingRow({
    opening_market_value: '1000', closing_market_value: '1000',
  })), true);
});

// ---- loadPref / savePref -----------------------------------------------

function stubStore() {
  const m = new Map();
  return {
    getItem: function (k) { return m.has(k) ? m.get(k) : null; },
    setItem: function (k, v) { m.set(k, v); },
    removeItem: function (k) { m.delete(k); },
  };
}

test('loadPref: falls back when nothing is stored', () => {
  assert.equal(loadPref('k', 'fallback', stubStore()), 'fallback');
});

test('savePref/loadPref: round-trips a stored value', () => {
  const store = stubStore();
  savePref('k', 'v', store);
  assert.equal(loadPref('k', 'fallback', store), 'v');
});

test('savePref: null/empty clears the preference back to the fallback', () => {
  const store = stubStore();
  savePref('k', 'v', store);
  savePref('k', null, store);
  assert.equal(loadPref('k', 'fallback', store), 'fallback');
  savePref('k', 'v', store);
  savePref('k', '', store);
  assert.equal(loadPref('k', 'fallback', store), 'fallback');
});

test('loadPref: a throwing store is treated as nothing stored', () => {
  const angry = {
    getItem: function () { throw new Error('storage disabled'); },
  };
  assert.equal(loadPref('k', 'fallback', angry), 'fallback');
});

test('savePref: a throwing store does not raise', () => {
  const angry = {
    setItem: function () { throw new Error('storage disabled'); },
  };
  assert.doesNotThrow(function () { savePref('k', 'v', angry); });
});

// ---- pathSeg ------------------------------------------------------------
test('pathSeg leaves the segments real routes actually use untouched', () => {
  // Every id, ISO date and slug the app links to is unreserved, so encoding
  // a route segment must not change any URL the SPA builds for itself.
  assert.equal(pathSeg('1'), '1');
  assert.equal(pathSeg(42), '42');
  assert.equal(pathSeg('2024-06-30'), '2024-06-30');
  assert.equal(pathSeg('open-parcels'), 'open-parcels');
  assert.equal(pathSeg('XASX:VAS'), 'XASX%3AVAS'); // ':' is reserved, still safe
});

test('pathSeg stops a hand-edited hash from re-shaping the request', () => {
  // A '?' would otherwise turn the rest of the segment into a query string,
  // and '/' would add path structure the route never intended.
  assert.equal(pathSeg('1?include_linked=true'), '1%3Finclude_linked%3Dtrue');
  assert.equal(pathSeg('../jobs'), '..%2Fjobs');
  assert.equal(pathSeg('a/b'), 'a%2Fb');
  assert.equal(pathSeg('#/e/trades'), '%23%2Fe%2Ftrades');
});

test('pathSeg over a composite key encodes the parts, not the separators', () => {
  // closing_prices is keyed by listing_id + date: the '/' between the parts is
  // real path structure and must survive, which is why the app maps pathSeg
  // over the parts rather than encoding the joined string.
  assert.equal(['1', '2024-06-30'].map(pathSeg).join('/'), '1/2024-06-30');
  assert.equal(['a/b', 'c'].map(pathSeg).join('/'), 'a%2Fb/c');
});
