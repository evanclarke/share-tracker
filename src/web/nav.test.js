//
// Unit tests for nav.js's pure navModel — the top menu bar's structure,
// independent of rendering (buildNav/setActiveNav need a DOM and are
// exercised indirectly via the served-bundle assertions in web.rs). Run with
// Node's built-in runner:
//
//   node --test 'src/web/*.test.js'
//
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { navModel } from './nav.js';
import { ENTITIES, REPORTS, MENUS } from './config.js';

test('navModel: every entity and report appears exactly once across all menus', () => {
  const model = navModel(ENTITIES, REPORTS, MENUS);
  const seen = {};
  model.forEach(function (menu) {
    menu.sections.forEach(function (sec) {
      sec.items.forEach(function (item) {
        seen[item.key] = (seen[item.key] || 0) + 1;
      });
    });
  });
  ENTITIES.forEach(function (e) {
    assert.equal(seen[e.slug], 1, `entity "${e.slug}" (menu: ${e.menu}) should appear exactly once — a typo\'d menu silently drops it`);
  });
  REPORTS.forEach(function (r) {
    const key = 'r:' + r.slug;
    assert.equal(seen[key], 1, `report "${r.slug}" (menu: ${r.menu}) should appear exactly once — a typo\'d menu silently drops it`);
  });
});

test('navModel: menus come out in the given order', () => {
  const model = navModel(ENTITIES, REPORTS, MENUS);
  assert.deepEqual(model.map(function (m) { return m.label; }), MENUS);
});

test('navModel: the Reports menu splits into its titled sections', () => {
  const model = navModel(ENTITIES, REPORTS, MENUS);
  const reportsMenu = model.find(function (m) { return m.label === 'Reports'; });
  const titles = reportsMenu.sections.map(function (s) { return s.title; });
  assert.deepEqual(titles, ['Portfolio', 'Decision support', 'CGT & tax', 'Cross-checks & alerts']);
});

test('navModel: Activity, Reference Data, and Jobs render as a single untitled section', () => {
  const model = navModel(ENTITIES, REPORTS, MENUS);
  ['Activity', 'Reference Data', 'Jobs'].forEach(function (label) {
    const menu = model.find(function (m) { return m.label === label; });
    assert.equal(menu.sections.length, 1, `${label} should have one section`);
    assert.equal(menu.sections[0].title, null);
  });
});

test('navModel: hrefs honour custom overrides and the slug/r:slug data-key scheme', () => {
  const model = navModel(ENTITIES, REPORTS, MENUS);
  const bySlug = {};
  model.forEach(function (m) {
    m.sections.forEach(function (s) {
      s.items.forEach(function (item) { bySlug[item.key] = item; });
    });
  });
  assert.equal(bySlug['closing_prices'].href, '#/prices');
  assert.equal(bySlug['jobs'].href, '#/jobs');
  assert.equal(bySlug['sells'].href, '#/sells');
  assert.equal(bySlug['transfers'].href, '#/transfers');
  assert.equal(bySlug['trades'].href, '#/e/trades');
  assert.equal(bySlug['r:overview'].href, '#/r/overview');
});

test('navModel: an item whose menu matches nothing in the given list is dropped, not thrown', () => {
  const model = navModel(
    [{ slug: 'x', title: 'X', menu: 'Nonexistent' }],
    [],
    ['Activity'],
  );
  const allItems = model.flatMap(function (m) { return m.sections.flatMap(function (s) { return s.items; }); });
  assert.equal(allItems.length, 0);
});
