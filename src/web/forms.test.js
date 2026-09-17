//
// Unit tests for forms.js. `entitlementDefaultHint` and `fieldId` are pure;
// the label/control pairing helpers build DOM nodes, so they run over a
// minimal element stub — just enough of the DOM for util.js's `el` to
// construct the nodes, with no browser harness. The call sites that use them
// are pinned by the served-bundle assertions in web.rs. Run with Node's
// built-in runner:
//
//   node --test 'src/web/*.test.js'
//
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { entitlementDefaultHint, fieldId, labelControl, labelledField } from './forms.js';

// A stand-in element: `el` needs `document.createElement`, `setAttribute`,
// `append` (its children are checked with `instanceof Node`), and the
// `className` property. Nothing else in the pairing helpers touches the DOM.
class StubNode {
  constructor(tag) { this.tagName = tag; this.attrs = {}; this.children = []; this.className = ''; }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  append(child) { this.children.push(child); }
}

function withStubDom(fn) {
  const priorNode = globalThis.Node, priorDocument = globalThis.document;
  globalThis.Node = StubNode;
  globalThis.document = { createElement: function (tag) { return new StubNode(tag); } };
  try { return fn(); } finally { globalThis.Node = priorNode; globalThis.document = priorDocument; }
}

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

// `fieldId` is the one value both halves of a pairing read, so it decides
// whether the label's `for` names the control's `id`.
test('fieldId derives the id a label and its control share', () => {
  assert.equal(fieldId('snapshot_date'), 'f_snapshot_date');
  assert.equal(fieldId('alloc_qty', 3), 'f_alloc_qty_3');
  assert.equal(fieldId('price_override', 0), 'f_price_override_0');
});

// Repeated rows must not repeat an id: the allocation editor renders one
// parcel/quantity pair per row (twice over on the Transfer form) and the
// report screen one override input per listing.
test('fieldId keeps repeated renderings distinct', () => {
  const rows = [1, 2, 3].map(function (n) { return fieldId('alloc_qty', n); });
  assert.equal(new Set(rows).size, 3);
  assert.notEqual(fieldId('price_override', 7), fieldId('price_override', 8));
  // A single-control field keeps the plain name buildFieldInput also uses.
  assert.notEqual(fieldId('alloc_qty'), fieldId('alloc_qty', 1));
});

test('labelledField points the label at the control it wraps', () => {
  withStubDom(() => {
    const control = new StubNode('input');
    const field = labelledField('snapshot_date', 'Snapshot date', control);
    assert.equal(field.tagName, 'div');
    assert.equal(field.className, 'field');
    assert.equal(control.attrs.id, 'f_snapshot_date');
    const label = field.children[0];
    assert.equal(label.tagName, 'label');
    assert.equal(label.attrs.for, control.attrs.id);
    assert.equal(label.children[0], 'Snapshot date');
    // The control is the field's second child, after its label.
    assert.equal(field.children[1], control);
  });
});

test('labelControl keeps each repeated row its own id', () => {
  withStubDom(() => {
    const first = new StubNode('input'), second = new StubNode('input');
    const firstLabel = labelControl('alloc_qty', 'Quantity allocated', first, 1);
    const secondLabel = labelControl('alloc_qty', 'Quantity allocated', second, 2);
    assert.equal(firstLabel.attrs.for, first.attrs.id);
    assert.equal(secondLabel.attrs.for, second.attrs.id);
    assert.notEqual(first.attrs.id, second.attrs.id);
  });
});
