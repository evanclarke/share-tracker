//
// share-tracker frontend: shared utilities (no imports — the root of the
// module graph). DOM construction, the JSON API client, display formatting
// (numeric kinds, friendly column labels, foreign-key naming), and the exact
// decimal-string arithmetic used wherever the UI computes money (never
// parseFloat on money).
//
// ---- tiny DOM helpers -------------------------------------------------
export function el(tag, attrs, children) {
  const n = document.createElement(tag);
  if (attrs) {
    for (const k in attrs) {
      const v = attrs[k];
      if (v == null || v === false) continue;
      if (k === 'class') n.className = v;
      else if (k === 'html') n.innerHTML = v;
      else if (k.slice(0, 2) === 'on' && typeof v === 'function') n.addEventListener(k.slice(2), v);
      else if (v === true) n.setAttribute(k, '');
      else n.setAttribute(k, v);
    }
  }
  if (children != null) {
    // `append` (not `appendChild`) so a non-Node child is inserted as a Text
    // node by the DOM itself: per spec `append` takes (Node or DOMString) and
    // never parses markup, so a string child can only ever become text — the
    // `html` attribute above is this helper's one deliberate HTML entry point.
    (Array.isArray(children) ? children : [children]).forEach(function (c) {
      if (c == null) return;
      n.append(c instanceof Node ? c : String(c));
    });
  }
  return n;
}

// ---- toasts -----------------------------------------------------------
//
// Two tiers that differ in *kind*, not just in duration (SCENARIOS Y-a):
//
//   success — a receipt for something that worked. Auto-hides after 3 s; the
//             user has nothing to do with it.
//   error   — the only statement of why a write was refused, and frequently a
//             task list: deleting a listing nine other tables draw on answers
//             a 251-character 422 naming all nine, and the longest refusal the
//             API can produce is 638 characters. Once it hides, that text
//             exists nowhere else in the document and cannot be brought back,
//             so it does not hide on its own — it stays until the user
//             dismisses it.
//
// Three consequences, each deliberate:
//
//  * Toasts stack; a new one never replaces a standing one. Overwriting an
//    undismissed error with the next message loses exactly what the auto-hide
//    lost, in a new shape. Newest is the bottom item — the corner the single
//    toast used to occupy — and the stack scrolls rather than running off the
//    top of the viewport, so nothing is ever dropped to make room.
//  * A navigation does not clear anything. An error naming records to go and
//    remove is read *while* navigating to remove them; clearing it on the hash
//    change would destroy it at the moment it is being acted on. (Success
//    toasts cannot outstay their welcome anyway: several save paths set
//    `location.hash` immediately after toasting, so clearing on navigation
//    would mean the confirmation was never seen at all.) This is why nothing
//    outside this function touches the `#toast` element — `web.rs` pins that.
//  * `role="alert"` goes on the inserted item, not on the `#toast` container:
//    the container is `hidden` whenever the stack is empty, so it is not a
//    live region a screen reader is already watching. An element inserted
//    carrying `role="alert"` is announced; a success item gets the polite
//    `role="status"` instead, since it is not worth interrupting for.
//
// The signature stays `toast(msg, isError)` — ~25 call sites, none changed.

// How long a toast of each tier lives. Pure, and separate from the DOM work,
// so the "an error toast never auto-hides" rule is one testable expression
// rather than a ternary buried in a `setTimeout` argument (src/web/util.test.js).
export function toastLifetime(isError) {
  return isError ? { persist: true, ms: 0 } : { persist: false, ms: 3000 };
}

export function toast(msg, isError) {
  const t = document.getElementById('toast');
  const text = String(msg);
  const life = toastLifetime(isError);

  // The same message again (clicking Delete twice on the same row) re-inserts
  // the one already showing rather than stacking a second copy: the re-insert
  // is what makes a screen reader announce it again.
  Array.prototype.slice.call(t.children).forEach(function (prev) {
    if (prev.dataset.msg === text) prev.remove();
  });

  function dismiss() {
    clearTimeout(item._timer);
    item.remove();
    if (!t.firstChild) t.hidden = true;
  }

  const item = el('div', {
    class: 'toast-item' + (isError ? ' error' : ''),
    role: isError ? 'alert' : 'status',
    // Click anywhere on the toast dismisses it. The close button is a real
    // <button>, so it is in the tab order and answers Enter and Space for
    // free (all three measured), and Escape dismisses while it has focus —
    // the keydown reaches this handler by bubbling out of the button, which
    // is why the item itself needs no tabindex. The button carries no `name`,
    // so neither the form code (`form.querySelector('[name=…]')`) nor
    // `filterableTable` can pick it up, and it lives outside `#app`, where
    // every view renders.
    onclick: dismiss,
    onkeydown: function (e) { if (e.key === 'Escape') dismiss(); },
  }, [
    el('span', { class: 'toast-msg' }, text),
    isError
      ? el('button', {
        class: 'toast-close',
        type: 'button',
        'aria-label': 'Dismiss this message',
        onclick: function (e) { e.stopPropagation(); dismiss(); },
      }, '\u00d7')
      : null,
  ]);
  item.dataset.msg = text;

  t.hidden = false;
  t.appendChild(item);
  t.scrollTop = t.scrollHeight; // the newest item, when the stack has to scroll
  if (!life.persist) item._timer = setTimeout(dismiss, life.ms);
}

export function setMain(node) {
  const app = document.getElementById('app');
  app.innerHTML = '';
  app.appendChild(node);
}

// ---- persisted UI preferences -----------------------------------------

// Small localStorage wrapper for remembering a UI choice across reloads
// (e.g. the Portfolio Overview's last-used range preset and its hide-
// inactive-holdings checkbox — the app's first use of any client-side
// persistence). Storage access can throw (Safari private browsing, storage
// disabled) — treated the same as "nothing stored" rather than breaking the
// view. `store` defaults to the browser's localStorage; tests pass a stub
// implementing the same getItem/setItem/removeItem shape, since Node has no
// localStorage.
function defaultStore() {
  return typeof localStorage === 'undefined' ? null : localStorage;
}

export function loadPref(key, fallback, store) {
  const s = store || defaultStore();
  if (!s) return fallback;
  try {
    const v = s.getItem(key);
    return v == null ? fallback : v;
  } catch (e) {
    return fallback;
  }
}

// A `null`/`undefined`/empty-string value clears the preference (used when a
// custom range is applied, so the remembered preset doesn't linger) rather
// than storing an empty string.
export function savePref(key, value, store) {
  const s = store || defaultStore();
  if (!s) return;
  try {
    if (value == null || value === '') s.removeItem(key);
    else s.setItem(key, String(value));
  } catch (e) {
    // storage unavailable — the preference just won't persist this session
  }
}

export function looksNumeric(v) {
  return typeof v !== 'boolean' && v != null && v !== '' && /^-?\d+(\.\d+)?$/.test(String(v));
}

// Server timestamps (fetched_at, generated_at, uploaded_at, job last-run)
// arrive as RFC 3339 UTC strings. Display them in the user's timezone, with
// the UTC instant available on hover (utcTooltip → the cell's title attr).
// Date-only fields (price_date, trade dates) don't match and pass through.
const TIMESTAMP_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})$/;
export function isTimestamp(v) {
  return typeof v === 'string' && TIMESTAMP_RE.test(v);
}
function pad2(n) { return (n < 10 ? '0' : '') + n; }
export function fmtLocalTimestamp(v) {
  const d = new Date(v);
  return d.getFullYear() + '-' + pad2(d.getMonth() + 1) + '-' + pad2(d.getDate())
    + ' ' + pad2(d.getHours()) + ':' + pad2(d.getMinutes()) + ':' + pad2(d.getSeconds());
}
export function utcTooltip(v) {
  const d = new Date(v);
  return d.getUTCFullYear() + '-' + pad2(d.getUTCMonth() + 1) + '-' + pad2(d.getUTCDate())
    + ' ' + pad2(d.getUTCHours()) + ':' + pad2(d.getUTCMinutes()) + ':' + pad2(d.getUTCSeconds()) + ' UTC';
}

export function cellText(v) {
  if (v == null) return '';
  if (typeof v === 'boolean') return v ? 'yes' : 'no';
  // A list-valued cell (the AMIT adjustment cross-check's `problems`) reads
  // as sentences, not as `String(array)`'s comma run-on.
  if (Array.isArray(v)) return v.join(' · ');
  if (isTimestamp(v)) return fmtLocalTimestamp(v);
  return String(v);
}

// ---- numeric display formatting ---------------------------------------
// Display-only rounding (the JSON API and CSV exports keep full precision):
// monetary amounts read as currency — rounded to the cent and thousands-
// grouped — while per-unit rates and quantities keep their entered precision,
// because rounding a rate breaks statement reconciliation. A column's kind is
// looked up by name (COLUMN_KINDS), so every table — entity lists, the
// bespoke Sells/Transfers lists, and the report tables — inherits the rule
// with no per-screen wiring. All arithmetic is exact (BigInt on the decimal
// string), never parseFloat on money. When rounding a money cell loses
// precision the full value is shown on hover.

// Round a decimal string to `dp` places, half away from zero. Exact; returns
// null for a non-decimal input (the caller then falls back to verbatim text).
export function roundDecimalStr(value, dp) {
  let s = String(value).trim();
  let neg = false;
  if (s[0] === '+') s = s.slice(1);
  else if (s[0] === '-') { neg = true; s = s.slice(1); }
  if (!/^\d+(\.\d+)?$/.test(s)) return null;
  const dot = s.indexOf('.');
  const curDp = dot < 0 ? 0 : s.length - dot - 1;
  let units = BigInt(dot < 0 ? s : s.slice(0, dot) + s.slice(dot + 1));
  if (curDp <= dp) {
    units *= 10n ** BigInt(dp - curDp);
  } else {
    const div = 10n ** BigInt(curDp - dp);
    const rem = units % div;
    units /= div;
    if (rem * 2n >= div) units += 1n; // half away from zero (operand is non-negative here)
  }
  let str = units.toString();
  if (dp > 0) { str = str.padStart(dp + 1, '0'); str = str.slice(0, -dp) + '.' + str.slice(-dp); }
  return (neg && units !== 0n ? '-' : '') + str;
}

// Thousands-group the integer part of a signed/decimal plain decimal string.
export function groupThousands(s) {
  let neg = '';
  if (s[0] === '-') { neg = '-'; s = s.slice(1); }
  const dot = s.indexOf('.');
  const intPart = dot < 0 ? s : s.slice(0, dot);
  const frac = dot < 0 ? '' : s.slice(dot);
  return neg + intPart.replace(/\B(?=(\d{3})+(?!\d))/g, ',') + frac;
}

// Pad to at least `dp` fractional places without rounding (used after
// rounding to top up short values; entered rates keep their own precision).
export function padMinDp(s, dp) {
  s = String(s);
  const dot = s.indexOf('.');
  const curDp = dot < 0 ? 0 : s.length - dot - 1;
  if (curDp >= dp) return s;
  return (dot < 0 ? s + '.' : s) + '0'.repeat(dp - curDp);
}

// Numeric equality of two decimal strings (1234.5 == 1234.50), used to decide
// whether money rounding actually dropped precision (→ original on tooltip).
export function decStrEq(a, b) {
  const ra = roundDecimalStr(a, 12), rb = roundDecimalStr(b, 12);
  return ra != null && ra === rb;
}

// Which of period-performance's two percentage fields the overview panel
// should show. `total_return_pct` divides the period return by the opening
// balance alone — fine for a short window, but a mid-window purchase
// inflates it (it looks like a great year when most of the closing value is
// new money, not growth — see reports::period_performance's doc comment on
// the two fields), and reading it as-is over a period longer than a year is
// misleading either way (a raw multi-year percentage with no per-year
// framing). Windows over 365 days show `money_weighted_return_pct` instead
// — the server's annualised, cash-flow-aware money-weighted return (an
// IRR) — which is already a correct per-year figure, no client-side math
// needed. `r` is a period-performance response (`from`/`to`,
// `total_return_pct`, `money_weighted_return_pct`).
export function periodReturnPct(r) {
  const days = (new Date(r.to + 'T00:00:00Z') - new Date(r.from + 'T00:00:00Z')) / 86400000;
  return days > 365
    ? { value: r.money_weighted_return_pct, annualized: true }
    : { value: r.total_return_pct, annualized: false };
}

// The strict "no impact on this period" predicate for a period-performance
// per-holding contribution row (reports::period_performance's
// `HoldingPeriod`): false only when opening/closing market value, purchases,
// sale proceeds, and income are *all* exactly zero — which forces
// capital_growth/fx_movement/total_return to zero too (they're derived from
// these), i.e. the holding was fully closed before the period even started
// and has zero bearing on it. A holding that was merely flat (held
// throughout, unchanged value, no trades) still counts as active — only the
// all-zero case is filtered by the Portfolio Overview's "hide holdings with
// no activity" checkbox. Values are decimal *strings* (e.g. "0.00", possibly
// "-0.00"), so compare with the exact decStrEq helper above (signed, and
// normalises "-0.00" to zero) rather than the non-negative-only decEq, and
// never Number()/parseFloat.
const NO_ACTIVITY_FIELDS = [
  'opening_market_value', 'closing_market_value', 'purchases', 'sale_proceeds', 'income',
];
export function holdingHasActivity(h) {
  return !NO_ACTIVITY_FIELDS.every(function (f) { return decStrEq(h[f], '0'); });
}

// Format a numeric cell for display per its column kind. Returns null when
// the column has no kind or the value isn't numeric (caller uses cellText);
// otherwise { text, tip } where tip is the original value when money/rate4
// rounding lost precision (shown on hover), else null.
export function numericDisplay(value, kind) {
  if (!kind || !looksNumeric(value)) return null;
  if (kind === 'money') {
    const r = roundDecimalStr(value, 2);
    return { text: groupThousands(r), tip: decStrEq(r, value) ? null : String(value) };
  }
  if (kind === 'rate4') {
    const r = roundDecimalStr(value, 4);
    return { text: padMinDp(r, 4), tip: decStrEq(r, value) ? null : String(value) };
  }
  return { text: String(value), tip: null }; // rate / quantity: entered precision, verbatim
}

// A money figure written into *prose* rather than into a table cell: the
// same rounding every money cell gets ('money' above — cents, thousands
// grouped), returned as a plain string, because a sentence has no element to
// hang the full-value tooltip on. Use this — never cellText, and never the
// raw value — wherever money is concatenated into a message: the health
// banner's alerts (app.js) and the Annual Tax Report's subtotal and alert
// lines (taxreport.js). The underlying Decimal is exact-arithmetic, so a
// discount, subtotal or halved figure routinely carries four or more decimal
// places ("12340.1234") that read as noise in a sentence and disagree with
// the same figure tabulated two clicks away (SCENARIOS Y-g).
export function moneyText(value) {
  const nd = numericDisplay(value, 'money');
  return nd ? nd.text : cellText(value);
}

// ---- API client -------------------------------------------------------

// A hash-route segment about to be interpolated into an API path. Route
// segments come from location.hash, so they are user input, and `api` feeds
// its path straight to fetch: without encoding, a hand-edited URL can bend the
// request the SPA makes into a different one (a `?` in the segment turns the
// rest into a query string). A real id, ISO date or report slug contains only
// unreserved characters, so this is a no-op on every route the app itself
// links to. Query-string values are already encoded at their call site.
export function pathSeg(value) {
  return encodeURIComponent(String(value));
}

// The reverse-proxy path prefix the server is mounted under, read from the
// shell's <meta name="base-path"> (the server substitutes its configured
// `base_path` there — see src/web.rs). Empty when mounted at the root, which is
// the default, so every URL below is byte-for-byte what it was before.
//
// Read on each call rather than cached at module load: that keeps this a pure
// function of the document, so the Node unit tests — which have no DOM — get
// the root behaviour without a stub. Hash routes (`#/...`) need no prefixing;
// they are resolved by the browser against the current document.
export function basePath() {
  if (typeof document === 'undefined') return '';
  const meta = document.querySelector('meta[name="base-path"]');
  return meta ? meta.getAttribute('content') || '' : '';
}

// Prefix a root-absolute server path with the base path. Every URL the app
// sends to (or links at) the server goes through here — `api` below, the
// attachment upload/download, and the report CSV export — so a path is written
// once, as it appears in docs/API.md, and lands on the right origin path
// whether the app is served at / or at /share_tracker.
export function apiUrl(path) {
  return basePath() + path;
}

// Whether infra::auth is configured, from the shell's <meta name="auth">
// (server-substituted — see src/web.rs's index_html). Read on each call, same
// reasoning as basePath: a pure function of the document, so the Node unit
// tests (no DOM) get the auth-off behaviour without a stub.
export function authEnabled() {
  if (typeof document === 'undefined') return false;
  const meta = document.querySelector('meta[name="auth"]');
  return !!(meta && meta.getAttribute('content'));
}

export async function api(method, path, body) {
  const opts = { method: method, headers: {} };
  if (body !== undefined) {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(apiUrl(path), opts);
  if (res.status === 401 && authEnabled()) {
    // The session cookie is missing or expired: there is nothing this call
    // site can recover from, so send the browser to sign in again rather
    // than surface "HTTP 401" as if it were an ordinary rejection.
    window.location.assign(apiUrl('/login'));
    // Never resolves: the navigation above is about to tear this page down,
    // and nothing downstream should run against a request that never
    // actually succeeded.
    return new Promise(function () {});
  }
  if (!res.ok) {
    let detail = '';
    try { detail = (await res.text()).trim(); } catch (e) { /* ignore */ }
    throw new Error('HTTP ' + res.status + (detail ? ': ' + detail : ''));
  }
  const ct = res.headers.get('content-type') || '';
  return ct.indexOf('application/json') !== -1 ? res.json() : null;
}

export async function nextId(apiPath) {
  const rows = await api('GET', apiPath);
  let max = 0;
  rows.forEach(function (r) { if (typeof r.id === 'number' && r.id > max) max = r.id; });
  return max + 1;
}

// Options for <select> fields, fetched fresh each render so newly created
// referenced rows (e.g. a just-added listing) are always available.
export async function loadOptions(source) {
  switch (source) {
    case 'currencies':
      return (await api('GET', '/currencies')).map(function (c) { return { value: c.code, label: c.code + ' — ' + c.name }; });
    case 'exchanges':
      return (await api('GET', '/exchanges')).map(function (e) { return { value: e.mic, label: e.mic + ' — ' + e.name }; });
    case 'listings':
      return (await api('GET', '/listings')).map(function (l) { return { value: l.id, label: l.id + ': ' + l.ticker + ' (' + (l.exchange_mic || 'Crypto') + ')' }; });
    case 'holdingAccounts':
      return (await api('GET', '/holding_accounts')).map(function (a) { return { value: a.id, label: a.id + ': ' + a.name }; });
    case 'amma': {
      const listing = await listingNamer();
      return (await api('GET', '/amma_statements')).map(function (a) { return { value: a.id, label: a.id + ': ' + listing(a.listing_id) + ' FY' + a.tax_year_end_date }; });
    }
    // Every open-or-not Buy/DRP trade — carries listing_id, holding_account_id
    // and date so a caller can filter the picker itself (e.g. the rights-sale
    // anchoring picker, which needs parcels dated before a record date and so
    // can't use the already-remaining-filtered 'openParcels' below).
    case 'buyParcels': {
      const listing = await listingNamer();
      return (await api('GET', '/trades')).filter(function (t) { return t.trade_type !== 'Sell'; })
        .map(function (t) {
          return {
            value: t.id,
            label: t.id + ': ' + t.trade_type + ' ' + t.quantity + ' (' + listing(t.listing_id) + ', ' + t.date + ')',
            listing_id: t.listing_id,
            holding_account_id: t.holding_account_id,
            date: t.date,
          };
        });
    }
    // Open (not fully sold) Buy/DRP parcels only — carries listing_id and
    // holding_account_id so a caller can filter the picker down to parcels
    // that are actually valid for a given Sell/Transfer (matching listing and
    // account, remaining_quantity > 0 is already enforced by the report).
    case 'openParcels': {
      return (await api('GET', '/portfolio/open-parcels')).map(function (p) {
        return {
          value: p.trade_id,
          label: p.trade_id + ': ' + p.ticker + ' — ' + p.remaining_quantity + ' remaining (acquired ' + p.acquisition_date + ')',
          listing_id: p.listing_id,
          holding_account_id: p.holding_account_id,
        };
      });
    }
    default:
      return [];
  }
}

// A trade named for prose, table cells, option labels, and toasts: side,
// quantity, listing (MIC:TICKER), and date — an id alone is meaningless.
// `listingName` resolves the trade's listing; pass listingNamer's resolver.
export function describeTrade(t, listingName) {
  if (!t) return '?';
  return t.trade_type + ' ' + t.quantity + ' ' + listingName(t.listing_id) + ' on ' + t.date;
}

// The operation that created a trade, read off its provenance links ('' for
// an ordinary trade) — the Origin column on the Trades and Sells lists. The
// rollover-style Buys (transfer-in, scrip replacement, demerger) carry the
// moved parcel's cost base on the brokerage column with a zero price, and a
// rights-exercise Buy carries the rights cost there, so those labels spell it
// out — a four-figure "brokerage" on a transfer-in is a cost base, not a fee.
export function tradeOrigin(t) {
  const costBase = ' (brokerage = carried cost base, not a fee)';
  if (t.transfer_id != null) {
    return t.trade_type === 'Sell'
      ? 'Transfer #' + t.transfer_id + ' out'
      : 'Transfer #' + t.transfer_id + ' in' + costBase;
  }
  if (t.scrip_action_id != null) return 'Scrip exchange' + (t.trade_type === 'Sell' ? '' : costBase);
  if (t.demerger_action_id != null) return 'Demerger' + costBase;
  if (t.rights_action_id != null) return 'Rights exercise (brokerage = rights cost)';
  if (t.ess_statement_id != null) return 'ESS vest';
  if (t.inheritance_id != null) return 'Inheritance';
  if (t.buyback_action_id != null) return 'Buy-back';
  if (t.worthless_action_id != null) return 'Worthless shares';
  return '';
}

// The confirm text for an AMMA statement's AMIT-adjustment generation: the
// parcels it will cover, and Σ of their units against what the statement says
// was held. That comparison is the whole point of previewing — "are the
// current positions correct?" is checkable here rather than assumed — so a
// mismatch is spelled out rather than folded into a total. Pure (no DOM, no
// fetch) so it is unit-tested; `parcelLabel` names a trade id.
//
// Each row carries two unit bases and the dialog shows both where they differ
// (SCENARIOS Y-c): `quantity` is the parcel's as-acquired units — what gets
// stored, and what the AMIT Adjustments screen shows afterwards — while
// `units_adjusted` re-bases it into the statement year's, which is the basis
// the total and `units_held` are in. A share split between an acquisition and
// the year end is what pulls them apart, and listing the stored figures under
// a total in the other basis read as an arithmetic error. They coincide
// whenever no split intervenes, which is the ordinary case: the bracket and
// its explanatory line appear only when at least one row actually differs, so
// the ordinary dialog is exactly as short as it was.
export function adjustmentPreviewText(result, parcelLabel) {
  const rebased = function (a) {
    return a.units_adjusted != null && !decStrEq(a.units_adjusted, a.quantity)
      ? a.units_adjusted : null;
  };
  const anyRebased = result.created.some(function (a) { return rebased(a) != null; });
  const lines = result.created.map(function (a) {
    const other = rebased(a);
    return '  • ' + parcelLabel(a.trade_id) + ' — ' + a.quantity
      + (other == null ? '' : ' (' + other + ' in the statement year’s basis)');
  });
  const basisNote = anyRebased
    ? ['', 'Quantities are stored in each parcel’s own as-acquired units. A split has since '
      + 'changed the basis, so where it differs the statement year’s figure follows in brackets '
      + '— that is the one the total below counts.']
    : [];
  const totals = 'Adjusted units ' + result.units_adjusted
    + ' vs the statement’s units held ' + result.units_held;
  const verdict = decStrEq(result.difference, '0')
    ? ' — they match.'
    : '\n\n⚠ MISMATCH of ' + result.difference + ' units. Check the holdings are complete '
      + 'before proceeding — the AMIT Adjustment Cross-Check will keep this statement flagged '
      + 'until it is resolved.';
  return ['Create ' + result.created.length
    + ' AMIT adjustment(s) from the parcels held at the statement’s year end:']
    .concat(lines)
    .concat(basisNote)
    .concat(['', totals + verdict, '', 'Proceed?'])
    .join('\n');
}

// Preview an AMIT-adjustment generation and ask the user to confirm it,
// answering whether to go ahead. The preview POST writes nothing but answers
// the same 422 the write would, so a refusal surfaces here rather than after
// the confirmation. Shared by the AMMA form's chain-after-save tick and the
// standing Generate action (config.js `confirm`).
export async function confirmGeneratedAdjustments(path, body) {
  const preview = await api('POST', path, Object.assign({}, body || {}, { preview: true }));
  const byId = {};
  (await loadOptions('buyParcels')).forEach(function (p) { byId[p.value] = p.label; });
  return window.confirm(adjustmentPreviewText(preview, function (id) {
    return byId[id] || 'trade #' + id;
  }));
}

// Human-readable labels for foreign-key id columns: the stored id keeps
// driving the API (and edit-form selects), but tables/prose show the
// referenced row's natural name (e.g. "XNYS:ICE", "CHESS Personal",
// "DRP 45 XASX:VDHG on 2024-12-20"). Some sources' labels embed another
// (a trade names its listing), so `needs` lists the sources to build first
// and the label fn receives the already-built maps to consult.
const TABLE_LABEL_SOURCES = {
  listings: { api: '/listings', label: function (l) { return (l.exchange_mic || 'Crypto') + ':' + l.ticker; } },
  holdingAccounts: { api: '/holding_accounts', label: function (a) { return a.name; } },
  trades: {
    api: '/trades', needs: ['listings'],
    label: function (t, maps) { return describeTrade(t, listingNameFromMap(maps.listings)); },
  },
  amma: {
    api: '/amma_statements', needs: ['listings'],
    label: function (a, maps) { return listingNameFromMap(maps.listings)(a.listing_id) + ' FY' + a.tax_year_end_date; },
  },
};

// Which source names each foreign-key id column. Drives table-cell naming
// for both the generic entity lists and the report tables — column names are
// shared across the JSON API, so a new id column inherits naming by name
// alone. (currency/exchange_mic columns are already readable codes, so they
// are deliberately absent.)
const FK_COLUMN_SOURCES = {
  listing_id: 'listings', scrip_listing_id: 'listings', demerger_listing_id: 'listings',
  holding_account_id: 'holdingAccounts', account_id: 'holdingAccounts',
  from_account_id: 'holdingAccounts', to_account_id: 'holdingAccounts',
  trade_id: 'trades', sale_trade_id: 'trades', purchase_trade_id: 'trades',
  reinvestment_trade_id: 'trades', vest_trade_id: 'trades',
  amma_statement_id: 'amma',
};

// id → "MIC:TICKER" resolver over a prebuilt listings map; unknown/null id
// falls back to the raw "listing N" wording.
function listingNameFromMap(map) {
  return function (lid) { return (map && map[lid]) || ('listing ' + lid); };
}

// specs: { columnName: sourceName, … } → { columnName: { id → label } }.
// Each distinct source is fetched once per render (so names stay fresh) and
// built in dependency order, so a label fn can consult the maps it `needs`.
export async function fkLabelMaps(specs) {
  const sourceMaps = {};
  async function build(sourceName) {
    if (sourceMaps[sourceName]) return sourceMaps[sourceName];
    const src = TABLE_LABEL_SOURCES[sourceName];
    if (!src) return null;
    for (const dep of src.needs || []) await build(dep);
    const map = {};
    (await api('GET', src.api)).forEach(function (r) { map[r.id] = src.label(r, sourceMaps); });
    sourceMaps[sourceName] = map;
    return map;
  }
  const out = {};
  for (const col of Object.keys(specs)) {
    const m = await build(specs[col]);
    if (m) out[col] = m;
  }
  return out;
}

// Label maps for whichever of `cols` are foreign-key id columns (per
// FK_COLUMN_SOURCES) — the shared path for the report tables and the generic
// entity list, so every id column renders its referenced row's name.
export async function columnLabelMaps(cols) {
  const specs = {};
  cols.forEach(function (c) { if (FK_COLUMN_SOURCES[c]) specs[c] = FK_COLUMN_SOURCES[c]; });
  return fkLabelMaps(specs);
}

// Display kind per numeric column, looked up by name across every table.
// 'money' rounds to 2 dp + thousands grouping; 'rate' / 'quantity' keep the
// entered precision; 'rate4' is a derived-or-entered average price rounded
// to 4 dp (full value on hover when rounding drops precision).
// Numeric columns absent here (ids, financial years, counts, percentages)
// display verbatim. Because the map is keyed by column name — shared across
// the JSON API — a column reused on a new screen classifies once.
const COLUMN_KINDS = (function () {
  const k = {};
  const set = function (kind, names) { names.forEach(function (n) { k[n] = kind; }); };
  set('money', [
    // Trade / income / AMMA / settings monetary line items.
    'brokerage', 'gst_on_brokerage', 'statement_total', 'opening_capital_loss',
    'franked_amount', 'unfranked_amount', 'foreign_source_income', 'foreign_tax_paid',
    'tfn_withholding_tax', 'franking_credits', 'conduit_foreign_income',
    // The income row carries the LIC's advised attributable part; the tax
    // summary reports the 50% of it deductible at D8. Both are money.
    'lic_capital_gain_amount', 'lic_capital_gain_deduction',
    'australian_interest', 'australian_dividends_unfranked', 'franked_dividends', 'net_rent',
    'foreign_income', 'foreign_tax_credits', 'foreign_tax_credits_capital_gains',
    'other_income', 'cgt_discount_gains',
    'cgt_indexation_gains', 'cgt_other_gains', 'capital_losses_applied', 'tax_deferred_amount',
    'tax_free_amount',
    // Report AUD aggregates (portfolio, open-parcels, unrealised, realised,
    // performance, net-capital-gain, tax-summary incl. its amma_* lines).
    'total_cost_base', 'market_value', 'original_cost_base', 'amit_cost_base_reduction',
    'remaining_cost_base', 'return_of_capital_reduction', 'unrealised_gain_loss',
    'proceeds', 'cost_base', 'capital_gain_loss', 'discount_eligible_gain',
    'non_discountable_gain', 'capital_loss', 'invested', 'income', 'total_return',
    'discount_eligible_gains', 'net_discount_eligible_gain', 'other_gains', 'net_other_gain',
    'capital_losses', 'capital_loss_brought_forward', 'capital_loss_carried_forward',
    'cgt_discount', 'net_capital_gain', 'cgt_event_e10_gain', 'cgt_event_g1_gain',
    'cgt_event_c2_gain', 'dividends_assessable', 'interest_income',
    'foreign_interest_income', 'franking_credits_denied',
    'foreign_tax_offsets', 'foreign_tax_offset_excess',
    // The FITO capital-gains apportionment memo and the dividend-equivalent
    // (item 1/2) cash line: both AUD money on the tax summary, and both read
    // as raw four-decimal figures beside their cent-rounded neighbours until
    // they were classified here (SCENARIOS Y-d).
    'foreign_tax_offsets_cgt_discount_reduction', 'employment_income',
    // The tax summary's two non-AMIT trust distribution lines (13U / 13C),
    // which the company-dividend line used to swallow (SCENARIOS Z-f).
    'trust_income_unfranked', 'trust_franked_distributions',
    'amma_australian_interest', 'amma_dividends_unfranked', 'amma_franked_dividends',
    'amma_net_rent', 'amma_foreign_income', 'amma_other_income', 'amma_cgt_discount_gains',
    'amma_cgt_indexation_gains', 'amma_cgt_other_gains', 'amma_capital_losses_applied',
    // ESS statement discount line items + the tax-summary ESS aggregates.
    'taxed_upfront_eligible', 'taxed_upfront_not_eligible', 'deferral_discount',
    'pre_2009_cessation_discount', 'foreign_source_discount', 'tfn_withholding',
    'aud_taxed_upfront_eligible', 'aud_taxed_upfront_not_eligible', 'aud_deferral_discount',
    'aud_pre_2009_cessation_discount', 'aud_foreign_source_discount',
    'ess_discount_assessable', 'ess_taxed_upfront_reduction', 'ess_foreign_source_discount',
    // Inheritance cost-base components.
    'lpr_expenditure',
    // Rights sales: the carried cost of purchased rights.
    'rights_cost',
    // Franking at-risk report credit columns.
    'credits_attached', 'credits_at_risk', 'credits_denied', 'additional_credits_at_risk',
    // AMIT cash cross-check report.
    'cash_total_aud',
    // Listing activity ledger: the row's own money figure in AUD.
    'amount_aud',
    // DRP residual cash carried on a reinvestment trade (the row's own
    // currency), and the health report's duplicate/30-day-sale figures.
    // Classified against the day they are tabulated: the trade columns are
    // not on the Trades list today, and the health figures reach the screen
    // only through the banner's prose — which now formats them through
    // moneyText, by this same 'money' kind (SCENARIOS Y-g).
    'residual_brought_forward', 'residual_carried_forward', 'residual_paid_out',
    'cost_base_total', 'discount_total', 'statement_discount',
    // Investment-expense line items + the tax-summary deduction aggregates.
    'amount', 'gross_amount', 'gross_assessable_investment_income',
    'deductions_loan_interest', 'deductions_management_fee', 'deductions_advice_fee',
    'deductions_account_keeping_fee', 'deductions_subscription', 'deductions_other',
    // The same deductions re-cut by the question each is claimed at.
    'deductions_trust_distributions', 'deductions_foreign_income', 'deductions_foreign_debt',
    'deductions_dividend_and_interest',
    'deductions_total', 'net_assessable_investment_income',
    // Indexation cross-check (and the indexed figure realised gains carries
    // beside the discount one): the two methods' assessable gains, the
    // indexed cost base, and the difference between them — all AUD, all
    // advisory (SCENARIOS AA-a).
    'indexed_cost_base', 'discount_method_gain', 'indexation_method_gain',
    'indexation_advantage', 'discount_method_total', 'indexation_method_total',
    'indexation_advantage_total', 'capital_losses_realised',
    // Period-performance report: opening/closing values and the
    // capital/FX/income breakdown, all AUD.
    'opening_market_value', 'closing_market_value', 'purchases', 'sale_proceeds',
    'capital_growth', 'fx_movement', 'realised_capital_gain',
  ]);
  set('rate', [
    // Per-unit prices/rates entered from statements — rounding them would
    // break reconciliation, so they keep their precision. average_price is
    // the exception (see rate4 below): it's an average price, so it rounds.
    'fx_rate', 'spot_fx_rate', 'amount_per_security', 'cost_base_adjustment',
    'rate',
    'price', 'price_as_observed', 'reinvestment_price', 'exercise_price', 'amount_per_unit',
    'buyback_price', 'buyback_dividend', 'buyback_franking_credit', 'buyback_market_value',
    // The close a demerger states the security actually traded at on the last
    // pre-demerger day — a price entered by hand, kept verbatim like `price`.
    'demerger_close_price',
    // A scrip-for-scrip exchange's cash-per-old-unit and the market value of
    // one new unit — the two sides of its apportionment, both entered per
    // unit from the offer document.
    'scrip_cash_per_unit', 'scrip_market_value',
    'market_value_per_share', 'deductible_percentage', 'proceeds_per_right',
    // Period-performance report: the ATO/RBA rates used at each endpoint.
    'rate_from', 'rate_to',
    // Indexation cross-check: the published CPI for a quarter and the factor
    // derived from it (3 decimal places by the ATO's own rounding rule) —
    // both kept verbatim, like every other figure read off a published table.
    'cpi', 'indexation_factor',
  ]);
  // Average price figures — derived (avg_cost_base_per_unit, current_price)
  // or entered (average_price) — round to 4 dp for display; never
  // cent-rounded. current_price is always derived (a live quote converted to
  // AUD via plain Decimal division, which doesn't terminate cleanly), never
  // statement-entered, so it carries none of the 'rate' bucket's
  // reconciliation concern.
  set('rate4', ['avg_cost_base_per_unit', 'average_price', 'current_price']);
  set('quantity', [
    'quantity', 'quantity_allocated', 'securities_held', 'units_held', 'units',
    'original_quantity', 'remaining_quantity', 'quantity_held', 'cgt_discount_eligible_quantity',
    'split_new_units', 'split_old_units', 'bonus_units', 'bonus_held_units',
    // AMIT adjustment cross-check: the statement's units against the set's,
    // and (SCENARIOS Z-d) against the units actually open at the year end.
    'units_adjusted', 'units_open_at_year_end',
    'rights_units', 'rights_held_units', 'scrip_new_units', 'scrip_old_units',
    'demerger_new_units', 'demerger_held_units',
    // Wash-sale and franking at-risk report unit columns.
    'buy_quantity', 'entitled_units', 'disqualified_units',
    'disqualified_units_now', 'disqualified_units_after_sale',
    // Listing activity ledger: running units-held balance.
    'units_after',
    // Health report: units of an ESS parcel sold inside 30 days.
    'units_sold',
    // AMIT adjustment generation: the generated units against the
    // statement's (shown in the confirm dialog's own prose today).
    'difference',
  ]);
  return k;
})();

// { col: kind } for whichever of `cols` carry a display kind — the synchronous
// analogue of columnLabelMaps, threaded into filterableTable so the formatter
// runs on every table without per-call wiring.
export function columnKinds(cols) {
  const out = {};
  cols.forEach(function (c) { if (COLUMN_KINDS[c]) out[c] = COLUMN_KINDS[c]; });
  return out;
}

// ---- human-friendly column headings -----------------------------------
// Every table column header and filter placeholder reads through columnLabel,
// so the chrome around the data shows "Amount per security", "FX rate",
// "Account" — never the raw database/JSON field name. This is the labelling
// counterpart to the foreign-key naming above: that fixed raw id *values*;
// this fixes raw field *names* in the headers around them. Keyed by column
// name — shared across the JSON API — so a column reused on a new screen
// inherits its heading with no per-screen wiring, exactly like COLUMN_KINDS.
// (Form input labels and screen/section headings already read from their
// per-field `label` / `title` config, so they need no mapping here.)

// Acronyms kept in their canonical casing inside a humanised label, rather
// than title-cased to "Aud"/"Fx"/"Drp". Keyed lowercase; the humaniser looks
// each word up case-insensitively.
const LABEL_ACRONYMS = {
  id: 'ID', aud: 'AUD', fx: 'FX', mic: 'MIC', isin: 'ISIN', drp: 'DRP', ato: 'ATO',
  cgt: 'CGT', amit: 'AMIT', amma: 'AMMA', gst: 'GST', lic: 'LIC', fito: 'FITO',
  tfn: 'TFN', lpr: 'LPR',
};

// Default humaniser so a field with no explicit COLUMN_LABELS entry never
// renders a raw identifier: a trailing "_id" is dropped (the cell already
// shows the referenced row's name, so "listing_id" → "Listing"), the
// snake_case becomes sentence case, and known acronyms keep canonical casing.
function humanizeLabel(name) {
  let s = String(name);
  if (s.length > 3 && /_id$/.test(s)) s = s.slice(0, -3); // listing_id → Listing
  const words = s.split('_').filter(Boolean);
  if (words.length === 0) return String(name);
  return words.map(function (w, i) {
    const acr = LABEL_ACRONYMS[w.toLowerCase()];
    if (acr) return acr;
    return i === 0 ? w.charAt(0).toUpperCase() + w.slice(1) : w;
  }).join(' ');
}

// Explicit header overrides where the default humaniser reads wrong or a
// unit/qualifier aids reading. The report AUD aggregates carry "(AUD)" — the
// Australian-tax view converts every figure to AUD, so those columns are
// always AUD; the per-row entity tables deliberately get no currency
// qualifier, because their amounts are in the row's own currency column.
const COLUMN_LABELS = {
  exchange_mic: 'Exchange',
  // Row history's own three identity columns. The default humaniser drops a
  // trailing "_id" (right for a foreign key, whose cell shows the referenced
  // row's *name*) — but these cells show the raw ids themselves, and they are
  // what the Row History form is filled in with, so they read as the form's
  // own field labels: "History ID", "Table", "Row ID".
  history_id: 'History ID',
  table_name: 'Table',
  row_id: 'Row ID',
  // The rename chain's before/after exchange columns: kept in step with
  // exchange_mic above rather than humanised to "Old exchange MIC".
  old_exchange_mic: 'Old exchange',
  new_exchange_mic: 'New exchange',
  holding_account_id: 'Account',
  market_value: 'Market value (AUD)',
  total_cost_base: 'Total cost base (AUD)',
  proceeds: 'Proceeds (AUD)',
  capital_gain_loss: 'Capital gain/loss (AUD)',
  unrealised_gain_loss: 'Unrealised gain/loss (AUD)',
  net_capital_gain: 'Net capital gain (AUD)',
  avg_cost_base_per_unit: 'Average cost base per unit (AUD)',
  cash_total_aud: 'Cash total (AUD)',
  amount_aud: 'Amount (AUD)',
  units_after: 'Units after',
  fx_provisional: 'Provisional FX',
  // Why a trade's stored settlement date is not a trading day (weekend /
  // holiday), blank when it is one. The default humaniser would render
  // "Settlement non trading reason", which reads as three nouns in a row.
  settlement_non_trading_reason: 'Settlement non-trading day',
  // Where a trade's settlement date came from (computed / stated /
  // unrecorded): "Settlement date source" reads as a field name rather than a
  // column heading.
  settlement_date_source: 'Settlement source',
  // The row was valued at an earlier day's close because the provider stopped
  // quoting the security (listings.unpriced_from). The default humaniser
  // would render "Price carried forward", which reads like a question.
  price_carried_forward: 'Carried-forward price',
  // The listing's totals-excluding marker: the date the price provider's
  // series begins. The default humaniser would render "Unpriced before"
  // already; pinned here so it stays in step with the listings form's.
  unpriced_before: 'Unpriced before',
  // The date the price provider stopped quoting the security; the default
  // humaniser would render "Unpriced from" already, but the label is pinned
  // here so it stays in step with the listings form's.
  unpriced_from: 'Unpriced from',
  // The closing price as the provider served it, before it was restated into
  // the price date's own unit basis (see API.md, Closing prices). The default
  // humaniser would render "Price as observed" without saying what differs.
  price_as_observed: 'As served by provider',
  // The provider symbol a fetched row was actually fetched under (0038). The
  // default humaniser would render "Fetched symbol", which reads as an
  // adjective on the symbol rather than saying it is the symbol the request
  // was made with.
  fetched_symbol: 'Fetched under symbol',
  // The employer statement's stated AUD figure for label F (reported verbatim
  // by the tax summary); the default humaniser would render "Aud …".
  aud_deferral_discount: 'Statement AUD deferral (F)',
  // Attachments report: owner_id is a raw id (not resolved to a name — it can
  // point at six different tables), so it needs to read as an id, not a bare
  // "Owner" beside the "Owner description" column.
  owner_id: 'Owner ID',
  byte_size: 'Size (bytes)',
  // A memo column: the CFI figure sits *inside* the unfranked amount printed
  // beside it, so the heading says so — a reader must not add the two.
  conduit_foreign_income_aud: 'CFI, within unfranked (AUD)',
  // The tax summary's two non-AMIT trust distribution lines: a trust
  // distribution reports at question 13, not at 11S/11T with dividends from
  // companies, and the label is the whole point of the distinction
  // (docs/ato/tax-return-labels-2026.md, SCENARIOS Z-f).
  trust_income_unfranked: 'Trust income, unfranked 13U (AUD)',
  trust_franked_distributions: 'Franked distributions from trusts 13C (AUD)',
  // The tax summary's deductions cut by destination question rather than by
  // kind of expense: the label is the whole point of the column, so it is in
  // the heading (docs/ato/tax-return-labels-2026.md).
  deductions_trust_distributions: 'Deductions, trust distributions 13Y (AUD)',
  deductions_foreign_income: 'Deductions, foreign income 20M (AUD)',
  deductions_foreign_debt: 'Deductions, foreign debt D15 (AUD)',
  deductions_dividend_and_interest: 'Deductions, dividends/interest D7-D8 (AUD)',
  // The annual tax report's per-deduction destination label.
  ato_label: 'ATO label',
};

// The friendly heading for a column: an explicit override, else humanised.
export function columnLabel(c) {
  return COLUMN_LABELS[c] || humanizeLabel(c);
}

// id → "MIC:TICKER" resolver for prose and option labels; an unknown/null
// id falls back to the raw "listing N" wording.
export async function listingNamer() {
  return listingNameFromMap((await fkLabelMaps({ listing_id: 'listings' })).listing_id);
}

// ---- exact decimal-string arithmetic ------------------------------------
// Exact decimal-string addition (the operands are money; float would drift).
export function addDecimalStrings(a, b) {
  a = String(a); b = String(b);
  const dp = function (s) { const i = s.indexOf('.'); return i < 0 ? 0 : s.length - i - 1; };
  const scale = Math.max(dp(a), dp(b));
  const scaled = function (s) { return BigInt(s.replace('.', '') + '0'.repeat(scale - dp(s))); };
  let sum = scaled(a) + scaled(b);
  if (scale === 0) return sum.toString();
  const neg = sum < 0n;
  if (neg) sum = -sum; // pad/split on the magnitude — a '-' would break padStart
  const digits = sum.toString().padStart(scale + 1, '0');
  const abs = (digits.slice(0, -scale) + '.' + digits.slice(-scale)).replace(/(\.\d*?)0+$/, '$1').replace(/\.$/, '');
  return (neg && abs !== '0' ? '-' : '') + abs;
}

// Exact decimal-string arithmetic for the non-negative money figures the
// income form computes client-side (float would drift). decParts parses
// "123.45" → { units: 12345n, dp: 2 } (null for a non-decimal); the
// divisions round to the cent, half away from zero, matching statements.
export function decParts(s) {
  s = String(s).trim();
  if (!/^\d+(\.\d+)?$/.test(s)) return null;
  const i = s.indexOf('.');
  return { units: BigInt(s.replace('.', '')), dp: i < 0 ? 0 : s.length - i - 1 };
}
function divToCents(num, den) {
  const cents = ((num * 200n + den) / (den * 2n)).toString().padStart(3, '0');
  return cents.slice(0, -2) + '.' + cents.slice(-2);
}
// a × b rounded to the cent, as "x.xx" (null if either is not a decimal).
export function mulToCents(a, b) {
  const pa = decParts(a), pb = decParts(b);
  if (!pa || !pb) return null;
  return divToCents(pa.units * pb.units, 10n ** BigInt(pa.dp + pb.dp));
}
// The franking credit on a fully franked amount at the 30% corporate rate:
// amount × 30/70, cent-rounded (PLS example: 2757.30 → 1181.70).
export function frankingCreditFor(amount) {
  const p = decParts(amount);
  if (!p) return null;
  return divToCents(p.units * 3n, 7n * 10n ** BigInt(p.dp));
}
// Numeric decimal-string equality (1181.7 matches 1181.70).
export function decEq(a, b) {
  const pa = decParts(a), pb = decParts(b);
  if (!pa || !pb) return false;
  const scale = BigInt(Math.max(pa.dp, pb.dp));
  return pa.units * 10n ** (scale - BigInt(pa.dp)) === pb.units * 10n ** (scale - BigInt(pb.dp));
}

// ---- allocation running total -------------------------------------------
// The shared allocation editor's live "allocated vs required" line, kept pure
// so `util.test.js` can pin it. `quantities` are the raw strings typed into
// the rows; `required` is the figure they must sum to — the Sell's quantity,
// the buy-back's units, the rights sold — or null/'' where the caller has no
// target at all (a Transfer moves whole parcels, so any sum it reaches is
// legitimate; the line then just reports what has been allocated so far).
//
// Rows that are blank are skipped rather than counted as zero: an untouched
// row is not yet a mistake. A row that is *not* blank and not a decimal is
// counted separately and reported, so a typo can never silently drop out of
// the total. The arithmetic is exact decimal-string throughout
// (`addDecimalStrings`) — an 8-decimal crypto quantity across 50 parcels is
// exactly what parseFloat would drift on, and it is the case that has to
// work. SCENARIOS Y-b.
const DECIMAL_RE = /^-?\d+(\.\d+)?$/;
function negateDecimalString(s) { return s.charAt(0) === '-' ? s.slice(1) : '-' + s; }
export function allocationSummary(quantities, required) {
  let total = '0';
  let counted = 0, invalid = 0;
  (quantities || []).forEach(function (q) {
    const s = String(q == null ? '' : q).trim();
    if (s === '') return;
    if (!DECIMAL_RE.test(s)) { invalid += 1; return; }
    total = addDecimalStrings(total, s);
    counted += 1;
  });
  const req = String(required == null ? '' : required).trim();
  const hasReq = DECIMAL_RE.test(req);
  // required − total: '0' exactly when they agree, because addDecimalStrings
  // normalises the result (100.00 and 100 both land on '0').
  const signed = hasReq ? addDecimalStrings(req, negateDecimalString(total)) : '0';
  // 'pending' is a target, not yet a mistake: a form nobody has typed an
  // allocation into is merely incomplete (its required fields say so), so it
  // states the figure to reach without colouring it as an error.
  const status = !hasReq ? 'none'
    : counted === 0 && invalid === 0 ? 'pending'
      : signed === '0' ? 'match' : signed.charAt(0) === '-' ? 'over' : 'short';
  const difference = status === 'over' ? signed.slice(1) : status === 'short' ? signed : '0';
  let text;
  if (status === 'none') {
    text = counted === 0 ? 'Nothing allocated yet.'
      : 'Allocated: ' + total + ' across ' + counted + ' parcel(s).';
  } else if (status === 'pending') {
    text = 'Allocated: 0 of ' + req + ' so far.';
  } else if (status === 'match') {
    text = 'Allocated: ' + total + ' of ' + req + ' — matches.';
  } else {
    text = 'Allocated: ' + total + ' of ' + req + ' — ' + difference
      + (status === 'short' ? ' short.' : ' over.');
  }
  if (invalid > 0) text += ' ' + invalid + ' row(s) not a valid quantity.';
  return {
    total: total, counted: counted, invalid: invalid,
    required: hasReq ? req : null, status: status, difference: difference, text: text,
  };
}
