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
  looksNumeric, columnKinds, columnLabel, columnLinks, listingLinkFrom, defaultSortColumn,
  tradeOrigin,
  periodReturnPct,
  holdingHasActivity, loadPref, savePref, pathSeg, basePath, apiUrl, authEnabled,
  cellText, adjustmentPreviewText, allocationSummary, toastLifetime, moneyText,
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

// The prose form of the 'money' kind: same rounding, plain string, no hover
// (a sentence has no cell to hang a tooltip on) — the health banner's alerts
// and the tax report's subtotal lines. SCENARIOS Y-g: the banner printed the
// raw "12340.1234" a table shows as "12,340.12".
test('moneyText: cent-rounded and grouped, falling back to cellText', () => {
  assert.equal(moneyText('12340.1234'), '12,340.12');
  assert.equal(moneyText('1234.5'), '1,234.50');
  assert.equal(moneyText('-0.005'), '-0.01');
  assert.equal(moneyText(null), ''); // not numeric — cellText's rendering, not "null"
  assert.equal(moneyText('n/a'), 'n/a');
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

// ---- columnLinks --------------------------------------------------------
test('columnLinks: every column naming a listing drills into its activity ledger', () => {
  // Keyed off FK_COLUMN_SOURCES, so the counterpart-listing columns are
  // linked on the same footing as listing_id itself — nothing per-report.
  const links = columnLinks(['listing_id', 'scrip_listing_id', 'demerger_listing_id']);
  assert.equal(links.listing_id({ listing_id: 7 }), '#/r/activity/7');
  assert.equal(links.scrip_listing_id({ scrip_listing_id: 2 }), '#/r/activity/2');
  assert.equal(links.demerger_listing_id({ demerger_listing_id: 3 }), '#/r/activity/3');
});

test('columnLinks: a column naming no linkable source gets no link at all', () => {
  // An account or a trade has no screen worth landing on, and a plain data
  // column is not a foreign key — absent, not a function returning null, so
  // filterableTable renders the cell exactly as it did before.
  const links = columnLinks(['holding_account_id', 'sale_trade_id', 'quantity', 'ticker']);
  assert.deepEqual(links, {});
});

test('columnLinks: a null or blank id links nowhere', () => {
  // The performance report's whole-portfolio row and an expense attributed
  // to no listing both carry a null listing_id — a link to '#/r/activity/'
  // (or to 'null') would be a dead end.
  const links = columnLinks(['listing_id']);
  assert.equal(links.listing_id({ listing_id: null }), null);
  assert.equal(links.listing_id({ listing_id: '' }), null);
  assert.equal(links.listing_id({}), null);
});

test('columnLinks: no link back to the screen the reader is already on', () => {
  // The Listing Activity report's own holding summary names the listing the
  // report is about; linking it would re-run the screen you are looking at.
  const links = columnLinks(['listing_id'], '#/r/activity');
  assert.equal(links.listing_id({ listing_id: 7 }), null);
  // …and any other report's rows still link there.
  assert.equal(columnLinks(['listing_id'], '#/r/overview').listing_id({ listing_id: 7 }), '#/r/activity/7');
});

// ---- defaultSortColumn --------------------------------------------------
test('defaultSortColumn: the four date namings the API uses', () => {
  assert.equal(defaultSortColumn(['id', 'date', 'quantity']), 'date');
  assert.equal(defaultSortColumn(['id', 'date_paid']), 'date_paid');
  assert.equal(defaultSortColumn(['id', 'sale_date']), 'sale_date');
  assert.equal(defaultSortColumn(['id', 'uploaded_at']), 'uploaded_at');
});

test('defaultSortColumn: the row\'s own date leads a secondary one', () => {
  // Column order is the table's own, which puts the date the row happened on
  // ahead of a derived or related one.
  assert.equal(defaultSortColumn(['id', 'date', 'settlement_date']), 'date');
  assert.equal(defaultSortColumn(['sale_date', 'acquisition_date']), 'sale_date');
  assert.equal(defaultSortColumn(['price_date', 'fetched_at']), 'price_date');
});

test('defaultSortColumn: no date column leaves the server order alone', () => {
  // A listing, a holding, an exchange — nothing to open newest-first on.
  assert.equal(defaultSortColumn(['id', 'ticker', 'name', 'security_type']), null);
  assert.equal(defaultSortColumn(['listing_id', 'quantity', 'total_cost_base']), null);
  // A financial year is not a date column: those tables stay as sent.
  assert.equal(defaultSortColumn(['tax_year', 'net_capital_gain']), null);
  // Nor is a column that merely ends in the letters "at" or "date".
  assert.equal(defaultSortColumn(['format', 'update', 'flat']), null);
});

test('listingLinkFrom: a composed listing name drills through on its kept id', () => {
  // The Closing Prices screen and the price-health surfaces build the name
  // for display ("XASX:VAS", a bare ticker) and keep the id beside it.
  const link = listingLinkFrom('_listing_id');
  assert.equal(link({ listing: 'XASX:VAS', _listing_id: 4 }), '#/r/activity/4');
  assert.equal(link({ listing: 'listing 9', _listing_id: null }), null);
  assert.equal(link({}), null);
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

// ---- basePath / apiUrl --------------------------------------------------
// The reverse-proxy prefix is read from the shell's <meta name="base-path">.
// Node has no DOM, so these pin the no-document path: the app must behave
// exactly as it did before base paths existed when it is mounted at the root.
test('basePath is empty without a document', () => {
  assert.equal(basePath(), '');
});

test('apiUrl is the identity when mounted at the root', () => {
  assert.equal(apiUrl('/listings'), '/listings');
  assert.equal(apiUrl('/attachments/3/content'), '/attachments/3/content');
  assert.equal(apiUrl('/reports/net_capital_gain/export'), '/reports/net_capital_gain/export');
});

test('apiUrl prefixes a path when a base path is published', () => {
  // Stand in for the browser's document long enough to prove the prefixing:
  // basePath reads the meta tag on each call, so no module state to reset.
  globalThis.document = {
    querySelector: (sel) => (sel === 'meta[name="base-path"]'
      ? { getAttribute: () => '/share_tracker' }
      : null),
  };
  try {
    assert.equal(basePath(), '/share_tracker');
    assert.equal(apiUrl('/listings'), '/share_tracker/listings');
    // The result is still a root-absolute URL, so it resolves against the
    // origin rather than the current hash route.
    assert.ok(apiUrl('/trades').startsWith('/'));
  } finally {
    delete globalThis.document;
  }
});

test('a document without the meta tag means the root, not undefined', () => {
  globalThis.document = { querySelector: () => null };
  try {
    assert.equal(basePath(), '');
    assert.equal(apiUrl('/listings'), '/listings');
  } finally {
    delete globalThis.document;
  }
});

// ---- authEnabled ----------------------------------------------------------
// Read from the shell's <meta name="auth"> the same way basePath reads
// <meta name="base-path"> — see src/web.rs's index_html.
test('authEnabled is false without a document', () => {
  assert.equal(authEnabled(), false);
});

test('authEnabled is false when the meta tag is absent or empty', () => {
  globalThis.document = { querySelector: () => null };
  try {
    assert.equal(authEnabled(), false);
  } finally {
    delete globalThis.document;
  }
  globalThis.document = {
    querySelector: (sel) => (sel === 'meta[name="auth"]' ? { getAttribute: () => '' } : null),
  };
  try {
    assert.equal(authEnabled(), false);
  } finally {
    delete globalThis.document;
  }
});

test('authEnabled is true when the shell published [auth] as configured', () => {
  globalThis.document = {
    querySelector: (sel) => (sel === 'meta[name="auth"]' ? { getAttribute: () => '1' } : null),
  };
  try {
    assert.equal(authEnabled(), true);
  } finally {
    delete globalThis.document;
  }
});


// ---- cellText -----------------------------------------------------------
test('cellText renders a list-valued cell as sentences, not a comma run-on', () => {
  assert.equal(cellText(['first problem.', 'second problem.']), 'first problem. · second problem.');
  assert.equal(cellText([]), '');
  // Everything else is unchanged.
  assert.equal(cellText(null), '');
  assert.equal(cellText(true), 'yes');
  assert.equal(cellText('1234.5'), '1234.5');
});

// ---- adjustmentPreviewText ----------------------------------------------
const PARCELS = { 18: 'Buy 509 XASX:HNDQ on 2024-02-28', 19: 'Buy 1302 XASX:HNDQ on 2024-03-01' };
const label = (id) => PARCELS[id] || 'trade #' + id;

test('a reconciling generation preview lists the parcels and says the totals match', () => {
  const text = adjustmentPreviewText({
    created: [
      { trade_id: 18, quantity: '509', units_adjusted: '509' },
      { trade_id: 19, quantity: '1302', units_adjusted: '1302' },
    ],
    units_adjusted: '1811', units_held: '1811', difference: '0',
  }, label);
  assert.match(text, /Create 2 AMIT adjustment/);
  assert.match(text, /Buy 509 XASX:HNDQ on 2024-02-28 — 509/);
  assert.match(text, /Buy 1302 XASX:HNDQ on 2024-03-01 — 1302/);
  assert.match(text, /Adjusted units 1811 vs the statement’s units held 1811 — they match\./);
  assert.doesNotMatch(text, /MISMATCH/);
  // No split, so the two bases coincide and the dialog stays exactly as
  // short as it was — no bracket, no basis note (SCENARIOS Y-c).
  assert.doesNotMatch(text, /basis/);
  assert.doesNotMatch(text, /^  • .* \(/m);
});

// SCENARIOS Y-c: a split between an acquisition and the statement's year end
// leaves the stored quantity and the total on different unit bases. The rows
// carry both, so the list visibly reaches the total instead of looking like
// an arithmetic mistake.
test('a re-based row shows both unit bases, and the list adds up to the total', () => {
  const text = adjustmentPreviewText({
    created: [
      { trade_id: 10, quantity: '1000', units_adjusted: '2000' },
      { trade_id: 11, quantity: '5', units_adjusted: '5' },
    ],
    units_adjusted: '2005', units_held: '1000', difference: '1005',
  }, label);
  assert.match(text, /trade #10 — 1000 \(2000 in the statement year’s basis\)/);
  // The row a split did not move keeps its single, unbracketed figure.
  assert.match(text, /trade #11 — 5$/m);
  assert.match(text, /as-acquired units/);
  assert.match(text, /that is the one the total below counts/);
  // 2000 + 5 = 2005, which is now readable off the list itself.
  assert.match(text, /Adjusted units 2005 vs the statement’s units held 1000/);
  assert.match(text, /MISMATCH of 1005 units/);
});

test('bases equal but written differently show no bracket and no basis note', () => {
  const text = adjustmentPreviewText({
    created: [{ trade_id: 18, quantity: '509', units_adjusted: '509.00' }],
    units_adjusted: '509.00', units_held: '509', difference: '0',
  }, label);
  assert.match(text, /Buy 509 XASX:HNDQ on 2024-02-28 — 509$/m);
  assert.doesNotMatch(text, /basis/);
});

test('a mismatch is spelled out, not folded into the total', () => {
  const text = adjustmentPreviewText({
    created: [{ trade_id: 18, quantity: '509' }],
    units_adjusted: '509', units_held: '1811', difference: '-1302',
  }, label);
  assert.match(text, /MISMATCH of -1302 units/);
  assert.match(text, /AMIT Adjustment Cross-Check/);
});

test('a zero difference written with decimals still reads as matching', () => {
  const text = adjustmentPreviewText({
    created: [{ trade_id: 99, quantity: '10.5' }],
    units_adjusted: '10.50', units_held: '10.5', difference: '0.00',
  }, label);
  assert.match(text, /they match\./);
  // An unlabelled parcel falls back to its id rather than "undefined".
  assert.match(text, /trade #99 — 10\.5/);
});

// ---- allocationSummary --------------------------------------------------
// The allocation editor's running total (SCENARIOS Y-b). Exact
// decimal-string arithmetic throughout: parseFloat is what these tests exist
// to keep out.
test('a matching allocation reads as matching, whatever the trailing zeros', () => {
  const s = allocationSummary(['60', '40.00'], '100');
  assert.equal(s.total, '100');
  assert.equal(s.status, 'match');
  assert.equal(s.difference, '0');
  assert.equal(s.text, 'Allocated: 100 of 100 — matches.');
});

test('a shortfall names the amount short, not just that it is wrong', () => {
  const s = allocationSummary(['100', '100', '10'], '300');
  assert.equal(s.status, 'short');
  assert.equal(s.difference, '90');
  assert.equal(s.text, 'Allocated: 210 of 300 — 90 short.');
});

test('an over-allocation names the excess', () => {
  const s = allocationSummary(['100', '120'], '200');
  assert.equal(s.status, 'over');
  assert.equal(s.difference, '20');
  assert.equal(s.text, 'Allocated: 220 of 200 — 20 over.');
});

test('50 eight-decimal crypto quantities sum exactly (float would drift)', () => {
  const rows = new Array(50).fill('0.00000001');
  const s = allocationSummary(rows, '0.0000005');
  assert.equal(s.total, '0.0000005');
  assert.equal(s.status, 'match');
  // The float answer: 50 * 1e-8 is 5.000000000000001e-7, not 5e-7.
  assert.notEqual(String(rows.reduce((a, b) => a + Number(b), 0)), '5e-7');
});

test('the 50-parcel reproduction: one row typed 10 instead of 100', () => {
  const rows = new Array(50).fill('100');
  rows[17] = '10';
  const s = allocationSummary(rows, '5000');
  assert.equal(s.total, '4910');
  assert.equal(s.text, 'Allocated: 4910 of 5000 — 90 short.');
});

test('a target with nothing allocated yet is stated, not flagged as an error', () => {
  const s = allocationSummary(['', ''], '100');
  assert.equal(s.status, 'pending');
  assert.equal(s.text, 'Allocated: 0 of 100 so far.');
  // …but a row that is there and wrong flips it straight to a shortfall.
  assert.equal(allocationSummary(['1'], '100').status, 'short');
  assert.equal(allocationSummary(['x'], '100').status, 'short');
});

test('blank rows are skipped, never counted as zero or as NaN', () => {
  const s = allocationSummary(['', '  ', '100', null, undefined], '100');
  assert.equal(s.status, 'match');
  assert.equal(s.counted, 1);
  assert.equal(s.invalid, 0);
  assert.ok(!/NaN/.test(s.text), s.text);
});

test('a partially typed row is reported, not silently dropped', () => {
  const s = allocationSummary(['100', '1.'], '200');
  assert.equal(s.invalid, 1);
  assert.equal(s.total, '100');
  assert.equal(s.text, 'Allocated: 100 of 200 — 100 short. 1 row(s) not a valid quantity.');
});

test('with no required figure the line reports the running total alone', () => {
  const none = allocationSummary([], null);
  assert.equal(none.status, 'none');
  assert.equal(none.required, null);
  assert.equal(none.text, 'Nothing allocated yet.');
  const some = allocationSummary(['12.5', '7.5'], '');
  assert.equal(some.status, 'none');
  assert.equal(some.total, '20');
  assert.equal(some.text, 'Allocated: 20 across 2 parcel(s).');
});

test('an empty required figure never renders as NaN or undefined', () => {
  ['', null, undefined, 'abc'].forEach(function (req) {
    const s = allocationSummary(['1'], req);
    assert.equal(s.status, 'none');
    assert.ok(!/NaN|undefined/.test(s.text), String(req) + ': ' + s.text);
  });
});

// ---- toastLifetime ------------------------------------------------------
// SCENARIOS Y-a: an error toast used to auto-hide after 6 s, taking with it
// the only statement of why a write was refused (a delete blocked by nine
// dependant tables is a 251-character task list). The two tiers now differ in
// kind: a success toast expires, an error toast never does.
test('a success toast auto-hides after three seconds', () => {
  assert.deepEqual(toastLifetime(false), { persist: false, ms: 3000 });
  assert.deepEqual(toastLifetime(undefined), { persist: false, ms: 3000 });
});

test('an error toast never auto-hides — it persists until dismissed', () => {
  assert.equal(toastLifetime(true).persist, true);
  // Not "a longer timeout": there must be no duration at all to schedule.
  assert.equal(toastLifetime(true).ms, 0);
});
