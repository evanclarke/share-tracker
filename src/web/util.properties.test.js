//
// Randomised property tests for util.js's exact decimal-string arithmetic:
// each helper is checked against a BigInt reference over thousands of
// generated decimal strings — the 28-significant-digit, many-decimal-place
// values a rust_decimal API figure can carry, signed, with and without
// trailing zeros — rather than over the hand-picked cases util.test.js pins.
//
//   node --test 'src/web/*.test.js'
//
// No property-testing library (the tree has no npm dependencies): a seeded
// PRNG drives the generator, so a failure reproduces from the seed printed
// in its message. PROPERTY_SEED pins one; PROPERTY_CASES widens a run.
//
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  roundDecimalStr, decStrEq, addDecimalStrings, decCompare, mulToCents, frankingCreditFor,
  decEq, allocationSummary, numericDisplay, holdingHasActivity,
} from './util.js';

const CASES = Number(process.env.PROPERTY_CASES || 2000);
const SEED = Number(process.env.PROPERTY_SEED || Date.now() % 2147483647);

// mulberry32: small, seedable, good enough to spread cases.
function prng(seed) {
  let a = seed >>> 0;
  return function () {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function int(rand, lo, hi) { return lo + Math.floor(rand() * (hi - lo + 1)); }

// A decimal string as the API writes one: optional sign, an integer part,
// and 0–28 decimal places, sometimes with trailing zeros kept (rust_decimal
// preserves scale) and sometimes "-0"-shaped.
function decimal(rand, { signed = true } = {}) {
  const intDigits = int(rand, 1, 12);
  let intPart = String(int(rand, 1, 9));
  for (let i = 1; i < intDigits; i++) intPart += String(int(rand, 0, 9));
  if (rand() < 0.3) intPart = '0';
  const dp = rand() < 0.2 ? 0 : int(rand, 1, 28 - Math.min(intDigits, 27));
  let frac = '';
  for (let i = 0; i < dp; i++) frac += String(int(rand, 0, 9));
  if (dp > 0 && rand() < 0.2) frac = frac.slice(0, -1) + '0';
  const sign = signed && rand() < 0.4 ? '-' : '';
  return sign + intPart + (dp > 0 ? '.' + frac : '');
}

// Reference: a decimal string as an exact { n, scale } (value = n / 10^scale).
function ref(s) {
  const neg = s[0] === '-';
  const body = neg ? s.slice(1) : s;
  const i = body.indexOf('.');
  const scale = i < 0 ? 0 : body.length - i - 1;
  const n = BigInt(body.replace('.', ''));
  return { n: neg ? -n : n, scale };
}
function atScale(r, scale) { return r.n * 10n ** BigInt(scale - r.scale); }
function cmp(a, b) {
  const s = Math.max(a.scale, b.scale);
  const x = atScale(a, s), y = atScale(b, s);
  return x < y ? -1 : x > y ? 1 : 0;
}
// Half away from zero to `dp` places, as a canonical-scale reference.
function roundRef(r, dp) {
  if (r.scale <= dp) return { n: atScale(r, dp), scale: dp };
  const div = 10n ** BigInt(r.scale - dp);
  const mag = r.n < 0n ? -r.n : r.n;
  let q = mag / div;
  if ((mag % div) * 2n >= div) q += 1n;
  return { n: r.n < 0n ? -q : q, scale: dp };
}

function property(name, body) {
  test(name + ' (seed ' + SEED + ')', () => {
    const rand = prng(SEED);
    for (let i = 0; i < CASES; i++) body(rand, i);
  });
}

property('addDecimalStrings is exact and canonical', (rand) => {
  const a = decimal(rand), b = decimal(rand);
  const sum = addDecimalStrings(a, b);
  assert.equal(cmp(ref(sum), { n: atScale(ref(a), 28) + atScale(ref(b), 28), scale: 28 }), 0, a + ' + ' + b + ' = ' + sum);
  assert.doesNotMatch(sum, /\.\d*0$|\.$|^-0$|^-?0\d/, 'not canonical: ' + sum);
});

property('roundDecimalStr rounds half away from zero, exactly', (rand) => {
  const v = decimal(rand), dp = int(rand, 0, 6);
  const r = roundDecimalStr(v, dp);
  const want = roundRef(ref(v), dp);
  assert.equal(cmp(ref(r), want), 0, v + ' to ' + dp + 'dp gave ' + r);
  assert.equal(ref(r).scale, dp, v + ' to ' + dp + 'dp gave ' + r);
  assert.ok(!/^-0(\.0*)?$/.test(r), 'negative zero: ' + r);
});

property('decStrEq is exact numeric equality', (rand) => {
  const a = decimal(rand);
  // Half the time a numerically equal respelling, half a value one unit in
  // a far decimal place away.
  const b = rand() < 0.5
    ? (a.includes('.') ? a + '0'.repeat(int(rand, 0, 3)) : a + '.' + '0'.repeat(int(rand, 1, 3)))
    : addDecimalStrings(a, '0.' + '0'.repeat(int(rand, 0, 20)) + '1');
  assert.equal(decStrEq(a, b), cmp(ref(a), ref(b)) === 0, a + ' vs ' + b);
});

property('decCompare is the exact order', (rand) => {
  const a = decimal(rand), b = rand() < 0.2 ? a : decimal(rand);
  assert.equal(decCompare(a, b), cmp(ref(a), ref(b)), a + ' vs ' + b);
  assert.equal(decCompare(b, a), cmp(ref(b), ref(a)));
});

property('decEq matches the reference for non-negative figures', (rand) => {
  const a = decimal(rand, { signed: false });
  const b = rand() < 0.5 ? a + (a.includes('.') ? '00' : '.0') : decimal(rand, { signed: false });
  assert.equal(decEq(a, b), cmp(ref(a), ref(b)) === 0, a + ' vs ' + b);
});

property('mulToCents and frankingCreditFor round to the cent, half away from zero', (rand) => {
  const a = decimal(rand, { signed: false }), b = decimal(rand, { signed: false });
  const ra = ref(a), rb = ref(b);
  const product = { n: ra.n * rb.n, scale: ra.scale + rb.scale };
  assert.equal(cmp(ref(mulToCents(a, b)), roundRef(product, 2)), 0, a + ' × ' + b);
  // amount × 3/7, to the cent: compare 100·amount·3/7 against the cents.
  const cents = ref(frankingCreditFor(a));
  const exact = atScale(ra, ra.scale) * 300n; // value·10^scale·300 = 7·10^scale · (cents·…)
  const den = 7n * 10n ** BigInt(ra.scale);
  let q = exact / den;
  if ((exact % den) * 2n >= den) q += 1n;
  assert.equal(atScale(cents, 2), q, 'franking credit on ' + a);
});

property('allocationSummary matches exactly when the rows sum to the requirement', (rand) => {
  const rows = Array.from({ length: int(rand, 1, 6) }, () => decimal(rand, { signed: false }));
  const total = rows.reduce((t, q) => addDecimalStrings(t, q), '0');
  const off = rand() < 0.5 ? '0' : (rand() < 0.5 ? '' : '-') + '0.' + '0'.repeat(int(rand, 0, 20)) + '1';
  const required = addDecimalStrings(total, off);
  const s = allocationSummary(rows, required);
  const want = off === '0' ? 'match' : off[0] === '-' ? 'over' : 'short';
  assert.equal(s.status, want, rows.join(' + ') + ' against ' + required);
});

property('a money cell carries its full value on hover exactly when rounding changed it', (rand) => {
  const v = decimal(rand);
  const shown = numericDisplay(v, 'money');
  const changed = cmp(roundRef(ref(v), 2), ref(v)) !== 0;
  assert.equal(shown.tip !== null, changed, v + ' shown as ' + shown.text);
});

property('a holding with any non-zero figure has activity', (rand) => {
  const fields = ['opening_market_value', 'closing_market_value', 'purchases', 'sale_proceeds', 'income'];
  const h = Object.fromEntries(fields.map((f) => [f, '0.00']));
  const f = fields[int(rand, 0, fields.length - 1)];
  h[f] = decimal(rand);
  assert.equal(holdingHasActivity(h), ref(h[f]).n !== 0n, f + ' = ' + h[f]);
});
