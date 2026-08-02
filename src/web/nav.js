//
// share-tracker frontend: the top menu bar. Split out of app.js once the
// sidebar it replaced outgrew a flat list (24 entities + 19 reports) — see
// config.js's `MENUS`/`menu`/`section` fields for the data this renders.
//
import { el, apiUrl, authEnabled } from './util.js';
import { ENTITIES, REPORTS, MENUS } from './config.js';

// Pure model builder — no DOM — so the menu structure is unit-testable
// (src/web/nav.test.js) independently of rendering. Returns one entry per
// menu in `menus` order, each holding its sections in first-appearance
// order; a section with `title: null` renders as a single unheaded column
// (Activity, Reference Data, and Jobs are single-column; Reports groups into
// titled columns via each report's `section`).
//
// An entity/report whose `menu` doesn't match any name in `menus` is
// silently dropped from the model, same as the old sidebar's `group` filter
// — nav.test.js is what catches a config typo, not a runtime throw, since a
// missing menu shouldn't take down the whole app.
export function navModel(entities, reports, menus) {
  const byMenu = {};
  menus.forEach(function (m) { byMenu[m] = { label: m, sections: [] }; });

  function sectionFor(menu, title) {
    const bucket = byMenu[menu];
    if (!bucket) return null;
    const key = title || null;
    let sec = bucket.sections.find(function (s) { return s.title === key; });
    if (!sec) {
      sec = { title: key, items: [] };
      bucket.sections.push(sec);
    }
    return sec;
  }

  entities.forEach(function (e) {
    const sec = sectionFor(e.menu, null);
    if (!sec) return;
    sec.items.push({
      href: e.custom ? '#/' + e.custom : '#/e/' + e.slug,
      key: e.slug,
      title: e.title,
    });
  });
  reports.forEach(function (r) {
    const sec = sectionFor(r.menu, r.section || null);
    if (!sec) return;
    sec.items.push({ href: '#/r/' + r.slug, key: 'r:' + r.slug, title: r.title });
  });

  return menus.map(function (m) { return byMenu[m]; });
}

function renderPanel(menu) {
  const panel = el('div', { class: 'menu-panel' });
  menu.sections.forEach(function (sec) {
    const col = el('div', { class: 'menu-section' });
    if (sec.title) col.appendChild(el('div', { class: 'menu-section-title' }, sec.title));
    sec.items.forEach(function (item) {
      col.appendChild(el('a', { href: item.href, 'data-key': item.key }, item.title));
    });
    panel.appendChild(col);
  });
  return panel;
}

// Opening is pure CSS (:hover / :focus-within on .menu, see style.css) so a
// mouse hover and a keyboard tab both work with no JS; these two listeners
// only close a panel a click/keyboard opened — a link click shouldn't leave
// its panel hanging open over the newly rendered view, and Escape should
// close whichever panel focus is currently inside.
function wireMenu(li) {
  li.addEventListener('click', function (ev) {
    if (ev.target.closest('a')) document.activeElement.blur();
  });
  li.addEventListener('keydown', function (ev) {
    if (ev.key === 'Escape') document.activeElement.blur();
  });
}

export function buildNav() {
  const nav = document.getElementById('nav');
  nav.innerHTML = '';
  const bar = el('ul', { class: 'menubar' });
  navModel(ENTITIES, REPORTS, MENUS).forEach(function (menu) {
    const li = el('li', { class: 'menu' }, [
      el('button', { type: 'button', class: 'menu-label' }, menu.label),
      renderPanel(menu),
    ]);
    wireMenu(li);
    bar.appendChild(li);
  });
  nav.appendChild(bar);
  // A real form POST (not a fetch(), not a hash route): logging out is a
  // state change, so it belongs on POST /logout, not a GET a prefetch or a
  // stray click-through could trigger — and a plain submit works with no JS
  // wiring beyond building the element, matching the login page it lands on.
  if (authEnabled()) {
    nav.appendChild(el('form', { method: 'post', action: apiUrl('/logout'), class: 'logout-form' }, [
      el('button', { type: 'submit' }, 'Log out'),
    ]));
  }
}

export function setActiveNav(key) {
  document.querySelectorAll('#nav a').forEach(function (a) {
    a.classList.toggle('active', a.getAttribute('data-key') === key);
  });
  // Also mark the owning menu label, so the current section reads as active
  // with the panel closed — the sidebar's border-accent highlight had no
  // menu-bar equivalent otherwise.
  document.querySelectorAll('.menu').forEach(function (li) {
    const active = li.querySelector('a.active') != null;
    li.querySelector('.menu-label').classList.toggle('active', active);
  });
}
