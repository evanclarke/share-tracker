//
// Unit tests for the pure helpers in forms.js. Only entitlementDefaultHint is
// pure today — the rest of the module wires a live form and is exercised by
// the served-bundle assertions in web.rs. Run with Node's built-in runner:
//
//   node --test 'src/web/*.test.js'
//
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { entitlementDefaultHint } from './forms.js';

// The income form never writes the entitlement date the user did not enter
// (SCENARIOS Y-e): the pay date is offered as the field's default in this
// hint — the "placeholder" `<input type="date">` will not render — and the
// input itself is left blank, so a blank stays a blank.
test('entitlementDefaultHint names the pay date currently entered', () => {
  const hint = entitlementDefaultHint('2025-07-05');
  assert.match(hint, /Leave blank/);
  assert.match(hint, /\(2025-07-05\)/);
  // It says what makes the blank worth keeping: it follows a later correction.
  assert.match(hint, /following the pay date/);
});

test('entitlementDefaultHint still states the default with no pay date yet', () => {
  const hint = entitlementDefaultHint('');
  assert.match(hint, /Leave blank/);
  assert.doesNotMatch(hint, /\(\)/); // no empty parenthetical
  assert.equal(entitlementDefaultHint(null), hint); // a null/undefined value reads the same
  assert.equal(entitlementDefaultHint(undefined), hint);
});

test('entitlementDefaultHint trims a padded value rather than printing it', () => {
  assert.equal(entitlementDefaultHint('  2025-06-25  '), entitlementDefaultHint('2025-06-25'));
});
