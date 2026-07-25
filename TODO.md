# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture

## Top menu bar navigation and an overview-first home screen (REQUIREMENTS 2026-07-25)
Replace the flat left sidebar with a hover-expanding top menu bar (Activity / Reports /
Reference Data / Jobs, Reports as a grouped mega-menu), make `#/` a real home route
rendering the Portfolio Overview directly, add New trade/income/sell/transfer shortcut
buttons to the overview, and reflow the overview so headline figures sit above the fold.
See REQUIREMENTS.md 2026-07-25 for full context.
- [ ] New `src/web/nav.js` module: pure `navModel(entities, reports, menus)` plus
      `buildNav()`/`setActiveNav()`, replacing the sidebar code in `app.js`; add its
      `JS_MODULES` entry in `src/web.rs` (array size 5 → 6)
- [ ] `src/web/config.js`: rename ENTITIES' `group` to `menu` (values: Activity,
      Reference Data, Jobs — Jobs gains Closing Prices, Snapshots, Row History), add
      `menu`/`section` to REPORTS, add `MENUS` export, add `shortcuts` to the `overview`
      report entry
- [ ] `src/web/index.html` + `src/web/style.css`: drop the `#layout`/sidebar markup and
      CSS, add the menu bar (`.menubar`/`.menu`/`.menu-panel`/`.menu-section`) with
      CSS-driven hover/focus-within expansion, add `.price-form` for the demoted price
      override control
- [ ] `src/web/app.js`: empty-hash route renders the overview report directly (no
      `location.hash` redirect); `viewReport` renders `report.shortcuts` as a toolbar;
      split `renderPeriodSummary` into headline stats vs. detail so `performancePanel`
      can place the chart+range control between them; demote the price-override form off
      `.card` styling with a preceding "Holdings" heading
- [ ] `src/web/nav.test.js` (new): every entity/report appears in exactly one menu/section,
      menu and section ordering, href/data-key correctness including `custom` overrides
- [ ] `src/web.rs`: `top_menu_bar_ui_present`, `overview_is_the_home_screen`, strengthen
      `portfolio_overview_ui_present` with the shortcuts + reflow markers
- [ ] `scripts/ui-smoke.sh`: add a `#/` route check (heading, a shortcut label, a menu label)
- [ ] Docs: `docs/API.md` Web frontend section (menu bar, `#/` home route, shortcuts,
      `/static/nav.js`), README (Portfolio overview + Web UI bullets), `src/doc_checks.rs`
      test pinning the doc text


