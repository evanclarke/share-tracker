'use strict';
//
// share-tracker single-page frontend.
//
// No build step, no framework: a small config-driven engine renders a CRUD view
// for every domain entity and a table view for every report, all driving the
// existing JSON API on the same origin. Each entity is described once (its API
// path, key, and fields); generic list/form code does the rest. Post-record
// actions (DRP reinvestment, rights exercise, buy-back participation, scrip
// exchange, demerger) are likewise described once in ACTIONS and rendered by
// the generic viewAction. Sells and Transfers (written atomically with their
// parcel allocations) are the two flows with bespoke views.
//
(function () {
  // ---- tiny DOM helpers -------------------------------------------------
  function el(tag, attrs, children) {
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
      (Array.isArray(children) ? children : [children]).forEach(function (c) {
        if (c == null) return;
        n.appendChild(typeof c === 'object' ? c : document.createTextNode(String(c)));
      });
    }
    return n;
  }

  function toast(msg, isError) {
    const t = document.getElementById('toast');
    t.textContent = msg;
    t.className = isError ? 'error' : '';
    t.hidden = false;
    clearTimeout(toast._timer);
    toast._timer = setTimeout(function () { t.hidden = true; }, isError ? 6000 : 3000);
  }

  function setMain(node) {
    const app = document.getElementById('app');
    app.innerHTML = '';
    app.appendChild(node);
  }

  function looksNumeric(v) {
    return typeof v !== 'boolean' && v != null && v !== '' && /^-?\d+(\.\d+)?$/.test(String(v));
  }

  // Server timestamps (fetched_at, generated_at, uploaded_at, job last-run)
  // arrive as RFC 3339 UTC strings. Display them in the user's timezone, with
  // the UTC instant available on hover (utcTooltip → the cell's title attr).
  // Date-only fields (price_date, trade dates) don't match and pass through.
  const TIMESTAMP_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})$/;
  function isTimestamp(v) {
    return typeof v === 'string' && TIMESTAMP_RE.test(v);
  }
  function pad2(n) { return (n < 10 ? '0' : '') + n; }
  function fmtLocalTimestamp(v) {
    const d = new Date(v);
    return d.getFullYear() + '-' + pad2(d.getMonth() + 1) + '-' + pad2(d.getDate())
      + ' ' + pad2(d.getHours()) + ':' + pad2(d.getMinutes()) + ':' + pad2(d.getSeconds());
  }
  function utcTooltip(v) {
    const d = new Date(v);
    return d.getUTCFullYear() + '-' + pad2(d.getUTCMonth() + 1) + '-' + pad2(d.getUTCDate())
      + ' ' + pad2(d.getUTCHours()) + ':' + pad2(d.getUTCMinutes()) + ':' + pad2(d.getUTCSeconds()) + ' UTC';
  }

  function cellText(v) {
    if (v == null) return '';
    if (typeof v === 'boolean') return v ? 'yes' : 'no';
    if (isTimestamp(v)) return fmtLocalTimestamp(v);
    return String(v);
  }

  // ---- API client -------------------------------------------------------
  async function api(method, path, body) {
    const opts = { method: method, headers: {} };
    if (body !== undefined) {
      opts.headers['Content-Type'] = 'application/json';
      opts.body = JSON.stringify(body);
    }
    const res = await fetch(path, opts);
    if (!res.ok) {
      let detail = '';
      try { detail = (await res.text()).trim(); } catch (e) { /* ignore */ }
      throw new Error('HTTP ' + res.status + (detail ? ': ' + detail : ''));
    }
    const ct = res.headers.get('content-type') || '';
    return ct.indexOf('application/json') !== -1 ? res.json() : null;
  }

  async function nextId(apiPath) {
    const rows = await api('GET', apiPath);
    let max = 0;
    rows.forEach(function (r) { if (typeof r.id === 'number' && r.id > max) max = r.id; });
    return max + 1;
  }

  // Options for <select> fields, fetched fresh each render so newly created
  // referenced rows (e.g. a just-added listing) are always available.
  async function loadOptions(source) {
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
      case 'buyParcels': {
        const listing = await listingNamer();
        return (await api('GET', '/trades')).filter(function (t) { return t.trade_type !== 'Sell'; })
          .map(function (t) { return { value: t.id, label: t.id + ': ' + t.trade_type + ' ' + t.quantity + ' (' + listing(t.listing_id) + ', ' + t.date + ')' }; });
      }
      default:
        return [];
    }
  }

  // Human-readable labels for foreign-key id columns in list tables: the
  // stored id keeps driving the API (and edit-form selects), but tables show
  // the referenced row's natural name (e.g. "XNYS:ICE", "CHESS Personal").
  // Only sources with a natural short name are mapped — currency codes are
  // already readable, and trade/statement ids have no name to show.
  const TABLE_LABEL_SOURCES = {
    listings: { api: '/listings', label: function (l) { return (l.exchange_mic || 'Crypto') + ':' + l.ticker; } },
    holdingAccounts: { api: '/holding_accounts', label: function (a) { return a.name; } },
  };

  // specs: { columnName: sourceName, … } → { columnName: { id → label } },
  // fetching each distinct source once per render so the names stay fresh.
  async function fkLabelMaps(specs) {
    const bySource = {};
    const out = {};
    for (const col of Object.keys(specs)) {
      const sourceName = specs[col];
      const src = TABLE_LABEL_SOURCES[sourceName];
      if (!src) continue;
      if (!bySource[sourceName]) {
        const map = {};
        (await api('GET', src.api)).forEach(function (r) { map[r.id] = src.label(r); });
        bySource[sourceName] = map;
      }
      out[col] = bySource[sourceName];
    }
    return out;
  }

  // id → "MIC:TICKER" resolver for prose and option labels; an unknown/null
  // id falls back to the raw "listing N" wording.
  async function listingNamer() {
    const map = (await fkLabelMaps({ listing_id: 'listings' })).listing_id;
    return function (lid) { return map[lid] || ('listing ' + lid); };
  }

  // ---- field constructors ----------------------------------------------
  function field(name, label, type, extra) { return Object.assign({ name: name, label: label, type: type }, extra || {}); }
  const txt = function (n, l, x) { return field(n, l, 'text', x); };
  const dec = function (n, l, x) { return field(n, l, 'decimal', Object.assign({ default: '0' }, x || {})); };
  const int = function (n, l, x) { return field(n, l, 'int', x); };
  const dt = function (n, l, x) { return field(n, l, 'date', x); };
  const bool = function (n, l, x) { return field(n, l, 'bool', x); };
  const sel = function (n, l, options, x) { return field(n, l, 'select', Object.assign({ options: options }, x || {})); };
  const fk = function (n, l, source, x) { return field(n, l, 'select', Object.assign({ source: source, encode: 'int' }, x || {})); };

  // ---- entity configuration --------------------------------------------
  const ENTITIES = [
    {
      slug: 'exchanges', title: 'Exchanges', group: 'Reference data', api: '/exchanges',
      desc: 'Curated trading venues. Seeded with XASX (ASX) and XNYS (NYSE).',
      keyFields: [txt('mic', 'MIC', { required: true })],
      fields: [
        txt('name', 'Name', { required: true }),
        txt('country', 'Country', { required: true }),
        fk('currency', 'Default currency', 'currencies', { required: true, encode: 'string' }),
        txt('timezone', 'Timezone', { required: true, default: 'Australia/Sydney' }),
        int('settlement_days', 'Settlement days (T+N)', { required: true, default: '2' }),
        txt('close_time', 'Close time (HH:MM local)', { required: true, default: '16:00', hint: 'End of the regular session in the exchange timezone; closing prices are only collected after this.' }),
      ],
      columns: ['mic', 'name', 'country', 'currency', 'timezone', 'settlement_days', 'close_time'],
    },
    {
      slug: 'exchange_holidays', title: 'Exchange Holidays', group: 'Reference data', api: '/exchange_holidays',
      desc: 'Full-closure non-trading days; settlement skips these as well as weekends.',
      keyFields: [fk('mic', 'Exchange', 'exchanges', { required: true, encode: 'string' }), dt('holiday_date', 'Date', { required: true })],
      fields: [txt('name', 'Name', { required: true })],
      columns: ['mic', 'holiday_date', 'name'],
    },
    {
      slug: 'listings', title: 'Listings', group: 'Reference data', api: '/listings',
      desc: 'Securities you trade, each on a curated exchange — except Crypto listings, which have no exchange (leave it blank), settle same-day, and need a recognised digital-token ticker (e.g. BTC).',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        fk('exchange_mic', 'Exchange', 'exchanges', { optional: true, encode: 'string', hint: 'Required except for Crypto; must be blank for Crypto.' }),
        txt('ticker', 'Ticker', { required: true }),
        txt('name', 'Name', { required: true }),
        txt('isin', 'ISIN', { optional: true }),
        sel('security_type', 'Security type', ['Share', 'ETF', 'LIC', 'Trust', 'Crypto'], { required: true }),
        fk('currency', 'Currency', 'currencies', { required: true, encode: 'string' }),
        bool('amit', 'AMIT'),
        bool('preference', 'Preference share (90-day franking holding period)'),
      ],
      columns: ['id', 'exchange_mic', 'ticker', 'name', 'isin', 'security_type', 'currency', 'amit', 'preference'],
    },
    {
      slug: 'holding_accounts', title: 'Holding Accounts', group: 'Reference data', api: '/holding_accounts',
      desc: 'Where holdings sit within the one taxpayer (e.g. an employer share-plan account vs a personal broker account). The same listing can be held in several accounts at once, each with its own DRP enrolment; move parcels between them with a Transfer. Account 1 is the seeded default every write falls back to.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [txt('name', 'Name', { required: true })],
      columns: ['id', 'name'],
    },
    {
      slug: 'currencies', title: 'Currencies', group: 'Reference data', api: '/currencies', readonly: true,
      desc: 'Recognised ISO 4217 fiat and ISO 24165 token codes (import-managed).',
      columns: ['code', 'kind', 'numeric_code', 'name', 'short_name', 'minor_units', 'source'],
    },
    {
      slug: 'mic_registry', title: 'MIC Registry', group: 'Reference data', api: '/mic_registry', readonly: true,
      desc: 'ISO 10383 Market Identifier Codes (import-managed).',
      columns: ['mic', 'operating_mic', 'name', 'country_code', 'city', 'status', 'expiry_date'],
    },
    {
      slug: 'closing_prices', title: 'Closing Prices', group: 'Reference data', api: '/closing_prices', custom: 'prices',
      desc: 'Stored daily closing prices per held listing, collected by the price-import job.',
    },
    {
      slug: 'rba_fx_rates', title: 'RBA FX Rates', group: 'Reference data', api: '/rba_fx_rates', readonly: true,
      desc: 'Monthly RBA F11 rates (foreign units per AUD) used for ATO conversion (import-managed).',
      columns: ['id', 'currency', 'month', 'rate'],
    },
    {
      slug: 'trades', title: 'Trades', group: 'Activity', api: '/trades',
      desc: 'Buy and DRP acquisitions. Sells are entered under Sells so they always carry parcel allocations.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        sel('trade_type', 'Type', ['Buy', 'DRP'], { required: true }),
        dt('date', 'Trade date', { required: true }),
        dt('settlement_date', 'Settlement date', { optional: true, hint: 'Leave blank to auto-calculate (T+N business days, skipping weekends and holidays).' }),
        fk('listing_id', 'Listing', 'listings', { required: true }),
        dec('average_price', 'Average price', { required: true, default: '' }),
        dec('quantity', 'Quantity', { required: true, default: '' }),
        fk('currency', 'Currency', 'currencies', { required: true, encode: 'string' }),
        dec('brokerage', 'Brokerage'),
        bool('brokerage_includes_gst', 'Brokerage includes GST', { hint: 'Tick when the statement quotes brokerage GST-inclusive; the GST component (1/11, rounded to the cent) is derived automatically.' }),
        dec('gst_on_brokerage', 'GST on brokerage'),
        fk('brokerage_currency', 'Brokerage currency', 'currencies', { required: true, encode: 'string' }),
        dec('fx_rate', 'Manual FX rate', { default: '1', hint: 'Foreign units per AUD; fallback used only when no ATO rate exists. 1 for AUD.' }),
        txt('contract_note_ref', 'Contract note ref', { optional: true }),
        dec('statement_total', 'Statement total', { optional: true, default: '', hint: 'Optional cross-check in the brokerage currency: quantity × price + brokerage + GST. Rejected if it does not reconcile.' }),
        fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1' }),
      ],
      columns: ['id', 'trade_type', 'date', 'settlement_date', 'listing_id', 'average_price', 'quantity', 'currency', 'brokerage', 'statement_total', 'fx_rate', 'holding_account_id'],
      listFilter: function (row) { return row.trade_type !== 'Sell'; },
      attachOwner: 'trade_id',
      wireForm: wireGstBrokerage,
    },
    {
      slug: 'sells', title: 'Sells', group: 'Activity', api: '/sells', custom: 'sells',
      desc: 'Sell trades created atomically with their parcel allocations.',
    },
    {
      slug: 'transfers', title: 'Transfers', group: 'Activity', api: '/transfers', custom: 'transfers',
      desc: 'Moves between holding accounts (e.g. vested plan shares to a personal account) — not a CGT event.',
    },
    {
      slug: 'income', title: 'Income', group: 'Activity', api: '/income',
      desc: 'Dividends and trust distributions. The form captures what the payment advice prints — amount, franking treatment, the per-share figures — and the advanced toggle reveals the full tax-component breakdown; a DRP statement’s reinvestment can be entered in the same form.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        fk('listing_id', 'Listing', 'listings', { required: true }),
        dt('date_paid', 'Date paid', { required: true }),
        dec('amount_per_security', 'Amount per security', { optional: true, default: '', hint: 'Optional cross-check from the statement, supplied together with securities held: their product must equal the gross amount. Rejected if it does not reconcile.' }),
        dec('securities_held', 'Securities held', { optional: true, default: '' }),
        dt('ex_date', 'Ex date', { optional: true }),
        dec('franked_amount', 'Franked amount'),
        dec('unfranked_amount', 'Unfranked amount'),
        dec('foreign_source_income', 'Foreign source income'),
        dec('foreign_tax_paid', 'Foreign tax paid'),
        dec('tfn_withholding_tax', 'TFN withholding tax'),
        dec('franking_credits', 'Franking credits'),
        dec('lic_capital_gain_deduction', 'LIC capital gain deduction'),
        dec('conduit_foreign_income', 'Conduit foreign income'),
        bool('trust_income', 'Trust income'),
        fk('currency', 'Currency', 'currencies', { required: true, encode: 'string', default: 'AUD' }),
        fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The account the distribution was paid to — decides whose DRP enrolment applies.' }),
      ],
      wireForm: wireIncomeEntry,
      columns: ['id', 'listing_id', 'date_paid', 'franked_amount', 'unfranked_amount', 'franking_credits', 'currency', 'holding_account_id', 'reinvestment_trade_id'],
      rowActions: function (row) {
        return row.reinvestment_trade_id == null ? [{ label: 'Reinvest', href: '#/reinvest/' + row.id }] : [];
      },
      attachOwner: 'income_id',
    },
    {
      slug: 'amma_statements', title: 'AMMA Statements', group: 'Activity', api: '/amma_statements',
      desc: 'Annual AMIT Member Annual statements.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        fk('listing_id', 'Listing', 'listings', { required: true }),
        dt('tax_year_end_date', 'Tax year end', { required: true }),
        dec('units_held', 'Units held'),
        dt('date_received', 'Date received', { required: true }),
        dec('australian_interest', 'Australian interest'),
        dec('australian_dividends_unfranked', 'Australian dividends (unfranked)'),
        dec('franked_dividends', 'Franked dividends'),
        dec('franking_credits', 'Franking credits'),
        dec('net_rent', 'Net rent'),
        dec('foreign_income', 'Foreign income'),
        dec('foreign_tax_credits', 'Foreign tax credits'),
        dec('other_income', 'Other income'),
        dec('cgt_discount_gains', 'CGT discount gains'),
        dec('cgt_indexation_gains', 'CGT indexation gains'),
        dec('cgt_other_gains', 'CGT other gains'),
        dec('capital_losses_applied', 'Capital losses applied'),
        dec('tax_deferred_amount', 'Tax-deferred amount'),
        dec('tax_free_amount', 'Tax-free amount'),
        dec('cost_base_adjustment', 'Cost base adjustment (per unit)'),
        dec('tfn_withholding_tax', 'TFN withholding tax'),
        fk('currency', 'Currency', 'currencies', { required: true, encode: 'string', default: 'AUD' }),
        fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The registry issues one statement per holder account.' }),
      ],
      columns: ['id', 'listing_id', 'tax_year_end_date', 'units_held', 'cost_base_adjustment', 'currency', 'holding_account_id'],
      attachOwner: 'amma_statement_id',
    },
    {
      slug: 'amit_adjustments', title: 'AMIT Adjustments', group: 'Activity', api: '/amit_adjustments',
      desc: 'Links a purchase parcel (Buy/DRP trade) to an AMMA statement.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        fk('amma_statement_id', 'AMMA statement', 'amma', { required: true }),
        fk('trade_id', 'Trade (Buy/DRP)', 'buyParcels', { required: true }),
        dec('quantity', 'Quantity', { required: true, default: '' }),
      ],
      columns: ['id', 'amma_statement_id', 'trade_id', 'quantity'],
    },
    {
      slug: 'parcel_allocations', title: 'Parcel Allocations', group: 'Activity', api: '/parcel_allocations', readonly: true,
      desc: 'Sell→purchase parcel links (read-only; managed via Sells).',
      columns: ['id', 'sale_trade_id', 'purchase_trade_id', 'quantity_allocated'],
    },
    {
      slug: 'drp_enrolments', title: 'DRP Enrolments', group: 'Activity', api: '/drp_enrolments',
      desc: 'Dated DRP enrolment periods per (listing, holding account) — the same listing may be enrolled in one account and not another (blank unenrolment date = currently enrolled). Periods within an account must not overlap; unenrolling pays out the trailing carried residual.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        fk('listing_id', 'Listing', 'listings', { required: true }),
        fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1' }),
        dt('enrolment_date', 'Enrolment date', { required: true }),
        dt('unenrolment_date', 'Unenrolment date', { optional: true, hint: 'Leave blank while enrolled. Distributions with an ex date on or after this no longer reinvest.' }),
        sel('residual_handling', 'Residual handling', ['CarryForward', 'PayOut'], { required: true }),
      ],
      columns: ['id', 'listing_id', 'holding_account_id', 'enrolment_date', 'unenrolment_date', 'residual_handling'],
    },
    {
      slug: 'corporate_actions', title: 'Corporate Actions', group: 'Activity', api: '/corporate_actions',
      desc: 'Capital events against a listing: return of capital, share splits/consolidations, bonus issues, rights issues, off-market buy-backs, scrip-for-scrip takeovers, and demergers. The form shows only the chosen action type’s fields; rights issues, buy-backs, scrip-for-scrip takeovers, and demergers are executed after recording via the row’s Exercise / Participate / Exchange / Demerge action.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        sel('action_type', 'Action type', ['ReturnOfCapital', 'ShareSplit', 'BonusIssue', 'RightsIssue', 'BuyBack', 'ScripForScrip', 'Demerger'], { required: true }),
        fk('listing_id', 'Listing', 'listings', { required: true }),
        dt('date', 'Date', { required: true }),
        dec('amount_per_unit', 'Amount per unit', { optional: true, default: '' }),
        fk('currency', 'Currency', 'currencies', { optional: true, encode: 'string', default: '', hint: 'Currency of the per-unit amount(s).' }),
        dec('split_new_units', 'Split: new units', { optional: true, default: '' }),
        dec('split_old_units', 'Split: old units', { optional: true, default: '' }),
        dec('bonus_units', 'Bonus: units issued', { optional: true, default: '' }),
        dec('bonus_held_units', 'Bonus: per units held', { optional: true, default: '' }),
        dec('rights_units', 'Rights: new units', { optional: true, default: '' }),
        dec('rights_held_units', 'Rights: per units held', { optional: true, default: '' }),
        dec('exercise_price', 'Rights: exercise price per unit', { optional: true, default: '' }),
        dec('buyback_price', 'Buy-back: price per unit', { optional: true, default: '' }),
        dec('buyback_dividend', 'Buy-back: dividend per unit', { optional: true, default: '', hint: '0 (or blank) when the price has no dividend component.' }),
        dec('buyback_franking_credit', 'Buy-back: franking credit per unit', { optional: true, default: '', hint: 'Needs a dividend.' }),
        dec('buyback_market_value', 'Buy-back: market value per unit', { optional: true, default: '', hint: 'Had the buy-back not been proposed. Blank if the price is at or above it.' }),
        fk('scrip_listing_id', 'Scrip: replacement listing', 'listings', { optional: true, default: '', hint: 'Must differ from the listing being taken over.' }),
        dec('scrip_new_units', 'Scrip: new units', { optional: true, default: '' }),
        dec('scrip_old_units', 'Scrip: per old units', { optional: true, default: '' }),
        fk('demerger_listing_id', 'Demerger: demerged listing', 'listings', { optional: true, default: '', hint: 'Must differ from the head listing.' }),
        dec('demerger_new_units', 'Demerger: new units', { optional: true, default: '' }),
        dec('demerger_held_units', 'Demerger: per units held', { optional: true, default: '' }),
        dec('demerger_cost_base_pct', 'Demerger: cost base % to demerged entity', { optional: true, default: '', hint: 'The head-entity-advised percentage (0–100 exclusive), e.g. 5.063.' }),
      ],
      // The form renders only the selected action_type's field group (plus the
      // common fields above that appear in no group); the unchosen groups'
      // fields submit as null, exactly as their blank inputs used to. The
      // matching typeDescs entry scopes the form's description to the type.
      typeField: 'action_type',
      fieldGroups: {
        ReturnOfCapital: ['amount_per_unit', 'currency'],
        ShareSplit: ['split_new_units', 'split_old_units'],
        BonusIssue: ['bonus_units', 'bonus_held_units'],
        RightsIssue: ['rights_units', 'rights_held_units', 'exercise_price', 'currency'],
        BuyBack: ['buyback_price', 'buyback_dividend', 'buyback_franking_credit', 'buyback_market_value', 'currency'],
        ScripForScrip: ['scrip_listing_id', 'scrip_new_units', 'scrip_old_units'],
        Demerger: ['demerger_listing_id', 'demerger_new_units', 'demerger_held_units', 'demerger_cost_base_pct'],
      },
      typeDescs: {
        ReturnOfCapital: 'Return-of-capital payment (CGT event G1): the per-unit amount reduces the cost base of parcels held on the payment date; any excess over a parcel’s cost base is a capital gain in the Net Capital Gain report.',
        ShareSplit: 'Share split/consolidation (TD 2000/10): on the conversion date every “old units” become “new units” (2-for-1 split: new 2, old 1; 1-for-10 consolidation: new 1, old 10) — no CGT event, the parcels keep their total cost base and original acquisition date.',
        BonusIssue: 'Bonus issue (non-assessable): on the issue date every “held units” receive “bonus units” extra units (1-for-10 issue: bonus 1, held 10) — no CGT event, the cost base is apportioned over original + bonus shares and the acquisition date is preserved; bonus shares chosen in lieu of a dividend are a DRP trade, not entered here.',
        RightsIssue: 'Rights issue: units held before the record date earn “rights units” per “held units” at the exercise price (1-for-4 issue: rights 1, held 4) — recording the issue changes nothing; use the row’s Exercise action to create the new Buy parcel (acquired at the exercise date, cost base = exercise payment + any amount paid for the rights).',
        BuyBack: 'Off-market buy-back: record the per-unit buy-back price, the dividend component of that price and its franking credit (both 0 for a listed-company buy-back announced after 25 Oct 2022), and the market value had the buy-back not been proposed (blank if the price is at or above it); recording changes nothing — use the row’s Participate action to sell units into the buy-back, which creates the Sell at the capital proceeds (max(price, market value) − dividend) plus the dividend income row.',
        ScripForScrip: 'Scrip-for-scrip takeover (all-scrip, with rollover): on the exchange date every “old units” of this listing become “new units” of the replacement listing (1-for-1 merger: new 1, old 1) — recording changes nothing; use the row’s Exchange action to substitute every open parcel: the capital gain is disregarded and each replacement parcel carries the consumed parcel’s remaining cost base and acquisition date (the combined period counts toward the 12-month discount).',
        Demerger: 'Demerger (eligible, rollover chosen): on the demerger date every “held units” of this (head) listing receive “new units” of the demerged listing (BHP Steel’s 1-for-5: new 1, held 5), and the advised percentage of each parcel’s cost base moves to the new interests — recording changes nothing; use the row’s Demerge action to apportion every open parcel: any gain is disregarded, the head parcels keep the rest of the cost base and their acquisition dates, and the new parcels’ 12-month discount clock runs from the original acquisition.',
      },
      // Per-type label for the common date field (generic 'Date' until a type
      // is chosen).
      typeLabels: {
        date: {
          ReturnOfCapital: 'Payment date',
          ShareSplit: 'Conversion date',
          BonusIssue: 'Issue date',
          RightsIssue: 'Record date',
          BuyBack: 'Buy-back date',
          ScripForScrip: 'Exchange date',
          Demerger: 'Demerger date',
        },
      },
      columns: ['id', 'action_type', 'listing_id', 'date', 'amount_per_unit', 'currency', 'split_new_units', 'split_old_units', 'bonus_units', 'bonus_held_units', 'rights_units', 'rights_held_units', 'exercise_price', 'buyback_price', 'buyback_dividend', 'buyback_franking_credit', 'buyback_market_value', 'scrip_listing_id', 'scrip_new_units', 'scrip_old_units', 'demerger_listing_id', 'demerger_new_units', 'demerger_held_units', 'demerger_cost_base_pct'],
      rowActions: function (row) {
        if (row.action_type === 'RightsIssue') return [{ label: 'Exercise', href: '#/exercise/' + row.id }];
        if (row.action_type === 'BuyBack') return [{ label: 'Participate', href: '#/participate/' + row.id }];
        if (row.action_type === 'ScripForScrip') return [{ label: 'Exchange', href: '#/scrip-exchange/' + row.id }];
        if (row.action_type === 'Demerger') return [{ label: 'Demerge', href: '#/demerge/' + row.id }];
        return [];
      },
    },
    {
      slug: 'cgt_settings', title: 'CGT Settings', group: 'Activity', api: '/cgt_settings',
      desc: 'Opening carried-forward capital loss (pre-system loss years), applied as the starting balance in the Net Capital Gain report.',
      keyFields: [int('id', 'ID', { required: true, default: '1', hint: 'Singleton — always 1.' })],
      fields: [dec('opening_capital_loss', 'Opening capital loss carried forward', { required: true })],
      columns: ['id', 'opening_capital_loss'],
    },
    {
      slug: 'jobs', title: 'Jobs', group: 'Maintenance', api: '/jobs', custom: 'jobs',
      desc: 'Run scheduled maintenance jobs (backup, reference-data imports) on demand.',
    },
  ];

  const REPORTS = [
    { slug: 'overview', title: 'Portfolio Overview', api: '/portfolio/overview', method: 'POST', prices: true, desc: 'Open holdings per listing and holding account, with optional market value.' },
    { slug: 'open-parcels', title: 'Open Parcels', api: '/portfolio/open-parcels', method: 'GET', desc: 'Every open parcel: acquisition date, original cost base, AMIT and return-of-capital reductions, remaining quantity and adjusted cost base (AUD).' },
    { slug: 'unrealised-gains', title: 'Unrealised Gains', api: '/portfolio/unrealised-gains', method: 'POST', prices: true, asOfDate: true, desc: 'Per-holding (listing × holding account) unrealised gain/loss vs cost base.' },
    { slug: 'realised-gains', title: 'Realised Gains', api: '/portfolio/realised-gains', method: 'GET', desc: 'Per-sale capital gain/loss split into CGT buckets.' },
    { slug: 'performance', title: 'Performance', api: '/portfolio/performance', method: 'POST', prices: true, asOfDate: true, desc: 'Investment performance per holding and overall: total return, money-weighted return (% p.a.), trailing-12-month income yield.' },
    { slug: 'net-capital-gain', title: 'Net Capital Gain', api: '/portfolio/net-capital-gain', method: 'GET', export: true, desc: 'Assessable net capital gain per financial year.' },
    { slug: 'tax-summary', title: 'Tax Summary', api: '/portfolio/tax-summary', method: 'GET', export: true, desc: 'Income aggregated by Australian financial year.' },
    { slug: 'exchange-mic-validation', title: 'Exchange MIC Validation', api: '/reports/exchange_mic_validation', method: 'GET', statusField: 'registry_status', desc: 'Curated exchanges checked against the ISO MIC registry.' },
    { slug: 'settlement-holiday-coverage', title: 'Settlement Holiday Coverage', api: '/reports/settlement_holiday_coverage', method: 'GET', statusField: 'coverage_status', desc: 'Trades whose settlement window falls outside the seeded exchange-holiday calendars (settlement may have skipped weekends only).' },
    { slug: 'snapshots', title: 'Snapshots', custom: 'snapshots', api: '/report_snapshots', desc: 'Stored daily results of the price-dependent reports (portfolio overview, unrealised gains, performance), valued at the stored closing prices, with a time-series graph. A back-dated fact marks affected snapshots stale; regenerate them here.' },
  ];

  // ---- post-action configuration -----------------------------------------
  // Follow-up actions reached from an owning row (a distribution's Reinvest, a
  // corporate action's Exercise / Participate / Exchange / Demerge). Each is
  // one config entry — owner fetch, fields, optional parcel allocations, POST
  // endpoint, texts — rendered by the generic viewAction, mirroring how
  // ENTITIES drives viewEntityForm. The confirm-only actions (scrip exchange,
  // demerge) are the degenerate config with `fields: []`.
  const ACTIONS = [
    // DRP reinvestment: creates a DRP trade and links it to the distribution.
    {
      slug: 'reinvest', nav: 'income', ownerApi: '/income', cancel: '#/e/income', submit: 'Reinvest',
      post: function (id) { return '/income/' + id + '/reinvest'; },
      title: function (id) { return 'Reinvest distribution #' + id; },
      desc: function (income, listing) { return 'Creates a DRP trade for ' + listing(income.listing_id) + ' and links it back to this distribution. The holding must be DRP-enrolled.'; },
      fields: function (income) {
        return [
          dec('reinvestment_price', 'Reinvestment price', { required: true, default: '' }),
          dec('fx_rate', 'FX rate', { default: '1', hint: 'Optional; defaults to 1.' }),
          dt('date', 'Trade date', { optional: true, hint: 'Optional; defaults to the distribution pay date (' + income.date_paid + ').' }),
        ];
      },
      toast: function (trade) { return 'Reinvested into trade #' + (trade ? trade.id : '?') + '.'; },
    },
    // Rights exercise: creates the new Buy parcel (acquired at the exercise
    // date); the server caps cumulative exercised units at the entitlement.
    {
      slug: 'exercise', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Exercise',
      post: function (id) { return '/corporate_actions/' + id + '/exercise'; },
      title: function (id) { return 'Exercise rights issue #' + id; },
      desc: function (a, listing) { return 'Creates a Buy trade for ' + listing(a.listing_id) + ' at the exercise price (' + a.exercise_price + ' ' + a.currency + ' per unit): ' + a.rights_units + ' new unit(s) per ' + a.rights_held_units + ' held at the record date.'; },
      fields: function (a) {
        return [
          dt('date', 'Exercise date', { required: true, hint: 'The new parcel’s acquisition date; on or after the record date (' + a.date + ').' }),
          dec('units', 'Units acquired', { required: true, default: '' }),
          dec('rights_cost', 'Amount paid for the rights', { default: '0', hint: 'Total, in ' + a.currency + '. 0 for rights issued free.' }),
          dec('fx_rate', 'FX rate', { default: '1', hint: 'Optional; defaults to 1.' }),
          fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'Where the exercised parcel lands.' }),
        ];
      },
      toast: function (trade) { return 'Exercised into trade #' + (trade ? trade.id : '?') + '.'; },
    },
    // Buy-back participation: atomically creates the Sell at the capital
    // proceeds per unit (max(price, market value) − dividend) with the chosen
    // parcel allocations, plus the dividend-component income row if any.
    {
      slug: 'participate', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Participate',
      post: function (id) { return '/corporate_actions/' + id + '/participate'; },
      title: function (id) { return 'Participate in buy-back #' + id; },
      desc: function (a, listing) { return 'Creates a Sell trade for ' + listing(a.listing_id) + ' at the capital proceeds per unit — max(price ' + a.buyback_price + ', market value ' + (a.buyback_market_value || a.buyback_price) + ') − dividend ' + a.buyback_dividend + ' ' + a.currency + ' — plus the dividend-component income row when there is one.'; },
      fields: function (a) {
        return [
          dt('date', 'Participation date', { required: true, hint: 'The CGT event (acceptance) date; on or after the buy-back date (' + a.date + '). Also the dividend component’s pay date.' }),
          dec('units', 'Units sold into the buy-back', { required: true, default: '' }),
          dec('fx_rate', 'FX rate', { default: '1', hint: 'Optional; defaults to 1.' }),
          fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'The participating account: allocations may only consume its parcels.' }),
        ];
      },
      allocations: { hint: 'Allocations must sum exactly to the units sold. Each parcel must be a Buy/DRP with enough remaining units.' },
      toast: function (r) {
        const tradeId = r && r.trade ? r.trade.id : '?';
        return 'Sold into the buy-back as trade #' + tradeId + (r && r.income ? ' with dividend income #' + r.income.id : '') + '.';
      },
    },
    // Scrip-for-scrip exchange (confirm-only): POST takes no parameters — the
    // action's terms and the holdings at its date determine everything. It
    // atomically closes every open parcel of the original listing (the
    // rollover disregards the gain) and creates the replacement parcels
    // carrying each consumed parcel's remaining cost base and acquisition date.
    {
      slug: 'scrip-exchange', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Exchange',
      post: function (id) { return '/corporate_actions/' + id + '/exchange'; },
      title: function (id) { return 'Exchange scrip-for-scrip takeover #' + id; },
      desc: function (a, listing) { return 'Substitutes every open parcel of ' + listing(a.listing_id) + ' held at ' + a.date + ' with ' + a.scrip_new_units + ' unit(s) of ' + listing(a.scrip_listing_id) + ' per ' + a.scrip_old_units + ' held. The rollover disregards the capital gain; each replacement parcel carries its consumed parcel’s remaining cost base and acquisition date (the combined holding period counts toward the 12-month discount). Undo by deleting the closing Sell from the Sells view.'; },
      fields: [],
      toast: function (r) {
        const n = r && r.replacements ? r.replacements.length : 0;
        return 'Exchanged into ' + n + ' replacement parcel(s) via closing sell #' + (r && r.sell ? r.sell.id : '?') + '.';
      },
    },
    // Demerger (confirm-only): POST takes no parameters. It atomically closes
    // every open parcel of the head listing (the rollover disregards any gain)
    // and recreates each as a head replacement parcel plus a demerged-entity
    // parcel splitting the cost base by the advised percentage, both keeping
    // the consumed parcel's acquisition date.
    {
      slug: 'demerge', nav: 'corporate_actions', ownerApi: '/corporate_actions', cancel: '#/e/corporate_actions', submit: 'Demerge',
      post: function (id) { return '/corporate_actions/' + id + '/demerge'; },
      title: function (id) { return 'Demerge #' + id; },
      desc: function (a, listing) { return 'Apportions every open parcel of head listing ' + listing(a.listing_id) + ' held at ' + a.date + ': ' + a.demerger_cost_base_pct + '% of each parcel’s cost base moves to ' + a.demerger_new_units + ' unit(s) of demerged listing ' + listing(a.demerger_listing_id) + ' per ' + a.demerger_held_units + ' held; the head parcels keep the rest. Any gain is disregarded and both sides keep the original acquisition date (the 12-month discount clock). Undo by deleting the closing Sell from the Sells view.'; },
      fields: [],
      toast: function (r) {
        const n = r && r.demerged_replacements ? r.demerged_replacements.length : 0;
        return 'Demerged ' + n + ' parcel(s) via closing sell #' + (r && r.sell ? r.sell.id : '?') + '.';
      },
    },
  ];

  const entityBySlug = {};
  ENTITIES.forEach(function (e) { entityBySlug[e.slug] = e; });
  const reportBySlug = {};
  REPORTS.forEach(function (r) { reportBySlug[r.slug] = r; });
  const actionBySlug = {};
  ACTIONS.forEach(function (a) { actionBySlug[a.slug] = a; });

  // ---- navigation -------------------------------------------------------
  function buildNav() {
    const nav = document.getElementById('nav');
    nav.innerHTML = '';
    const groups = ['Reference data', 'Activity', 'Maintenance'];
    groups.forEach(function (g) {
      nav.appendChild(el('div', { class: 'group' }, g));
      ENTITIES.filter(function (e) { return e.group === g; }).forEach(function (e) {
        const href = e.custom ? '#/' + e.custom : '#/e/' + e.slug;
        nav.appendChild(el('a', { href: href, 'data-key': e.slug }, e.title));
      });
    });
    nav.appendChild(el('div', { class: 'group' }, 'Reports'));
    REPORTS.forEach(function (r) {
      nav.appendChild(el('a', { href: '#/r/' + r.slug, 'data-key': 'r:' + r.slug }, r.title));
    });
  }

  function setActiveNav(key) {
    document.querySelectorAll('#nav a').forEach(function (a) {
      a.classList.toggle('active', a.getAttribute('data-key') === key);
    });
  }

  // ---- generic table ----------------------------------------------------
  // Every data table in the app — entity lists, the Sells list, and report
  // tables — goes through this one renderer so they are uniformly filterable
  // and sortable. Each column has its own filter input (substring match on that
  // column's text); the filters AND together, so you can e.g. filter currency
  // to "USD" and date to "2024" at once. Click a column header to sort it
  // (toggling ascending/descending). `opts.actions`, if given, renders a
  // trailing non-sortable, non-filtered Actions cell per row; `opts.statusField`
  // renders that column as a status badge; `opts.labels` ({col: {id → label}},
  // from fkLabelMaps) shows the label instead of the raw foreign-key id —
  // filtering and sorting follow the label, and the id moves to the tooltip.
  function filterableTable(rows, cols, opts) {
    opts = opts || {};
    const statusField = opts.statusField;
    const actions = opts.actions;
    const labels = opts.labels || {};
    function displayText(row, c) {
      const map = labels[c];
      if (map && row[c] != null && map[row[c]] !== undefined) return map[row[c]];
      return cellText(row[c]);
    }
    // A column is numeric if any row has a numeric value there — used for
    // right-alignment and numeric (not lexicographic) sorting. A labelled
    // foreign-key column displays names, so it is never numeric.
    const numeric = {};
    cols.forEach(function (c) { numeric[c] = !labels[c] && rows.some(function (r) { return looksNumeric(r[c]); }); });

    let sortCol = null;
    let sortDir = 1; // 1 = ascending, -1 = descending
    const filters = {}; // column → lowercased substring; absent/empty = no filter

    const container = el('div');

    // Header row: click-to-sort column titles.
    const headCells = cols.map(function (c) {
      const indicator = el('span', { class: 'sort-ind' }, '');
      const th = el('th', { class: (numeric[c] ? 'num ' : '') + 'sortable' }, [c, indicator]);
      th._col = c;
      th._ind = indicator;
      th.addEventListener('click', function () {
        if (sortCol === c) sortDir = -sortDir; else { sortCol = c; sortDir = 1; }
        headCells.forEach(function (h) {
          h._ind.textContent = h._col === sortCol ? (sortDir === 1 ? ' ▲' : ' ▼') : '';
        });
        renderBody();
      });
      return th;
    });
    if (actions) headCells.push(el('th', null, 'Actions'));

    // Filter row: one input per column, AND-combined.
    const filterCells = cols.map(function (c) {
      const input = el('input', {
        type: 'search', class: 'table-filter', placeholder: 'Filter ' + c + '…',
        oninput: function () {
          const v = this.value.trim().toLowerCase();
          if (v === '') delete filters[c]; else filters[c] = v;
          renderBody();
        },
      });
      return el('th', { class: 'filter-cell' }, input);
    });
    if (actions) filterCells.push(el('th', { class: 'filter-cell' }));

    const tbody = el('tbody');
    const thead = el('thead', null, [
      el('tr', null, headCells),
      el('tr', { class: 'filter-row' }, filterCells),
    ]);
    container.appendChild(el('table', null, [thead, tbody]));

    function visibleRows() {
      let out = rows;
      const active = Object.keys(filters);
      if (active.length) {
        out = out.filter(function (row) {
          return active.every(function (c) { return displayText(row, c).toLowerCase().indexOf(filters[c]) !== -1; });
        });
      }
      if (sortCol != null) {
        out = out.slice().sort(function (a, b) {
          const av = a[sortCol], bv = b[sortCol];
          let cmp;
          if (numeric[sortCol] && looksNumeric(av) && looksNumeric(bv)) cmp = Number(av) - Number(bv);
          else cmp = displayText(a, sortCol).localeCompare(displayText(b, sortCol));
          return cmp * sortDir;
        });
      }
      return out;
    }

    function renderBody() {
      tbody.innerHTML = '';
      const vr = visibleRows();
      if (vr.length === 0) {
        const span = cols.length + (actions ? 1 : 0);
        const filtered = Object.keys(filters).length > 0;
        tbody.appendChild(el('tr', null, el('td', { colspan: span, class: 'empty' },
          filtered ? 'No matching records.' : 'No records.')));
        return;
      }
      vr.forEach(function (row) {
        const tds = cols.map(function (c) {
          const v = row[c];
          if (statusField && c === statusField) {
            return el('td', null, el('span', { class: 'badge ' + cellText(v) }, cellText(v)));
          }
          if (isTimestamp(v)) {
            return el('td', { title: utcTooltip(v) }, cellText(v));
          }
          const text = displayText(row, c);
          // A labelled fk cell keeps its raw id reachable on the tooltip.
          const idTip = labels[c] && text !== cellText(v) ? 'id ' + cellText(v) : null;
          return el('td', { class: numeric[c] ? 'num' : null, title: idTip }, text);
        });
        if (actions) tds.push(actions(row) || el('td'));
        tbody.appendChild(el('tr', null, tds));
      });
    }

    renderBody();
    return container;
  }

  // Report tables: read-only, no actions column.
  function dataTable(rows, columns, statusField) {
    if (!rows || rows.length === 0) return el('div', { class: 'empty' }, 'No records.');
    return filterableTable(rows, columns || Object.keys(rows[0]), { statusField: statusField });
  }

  // ---- entity list view -------------------------------------------------
  async function viewEntityList(entity) {
    setActiveNav(entity.slug);
    let rows = await api('GET', entity.api);
    if (entity.listFilter) rows = rows.filter(entity.listFilter);

    const header = el('div', null, [
      el('h2', null, entity.title),
      el('p', { class: 'view-desc' }, entity.desc),
    ]);

    const toolbar = el('div', { class: 'toolbar' });
    if (!entity.readonly) {
      toolbar.appendChild(el('a', { href: '#/e/' + entity.slug + '/new' },
        el('button', { class: 'primary' }, '+ New ' + entity.title.replace(/s$/, ''))));
    }

    const cols = entity.columns || (rows[0] ? Object.keys(rows[0]) : entity.keyFields.concat(entity.fields).map(function (f) { return f.name; }));
    let table;
    if (rows.length === 0) {
      table = el('div', { class: 'empty' }, 'No records yet.');
    } else {
      // Foreign-key columns render the referenced row's name, not the raw id.
      const labelSpecs = {};
      (entity.fields || []).forEach(function (f) {
        if (f.source && cols.indexOf(f.name) !== -1) labelSpecs[f.name] = f.source;
      });
      const labels = await fkLabelMaps(labelSpecs);
      const actions = entity.readonly ? null : function (row) {
        const keyPath = entity.keyFields.map(function (kf) { return row[kf.name]; }).join('/');
        const td = el('td', { class: 'actions' });
        (entity.rowActions ? entity.rowActions(row) : []).forEach(function (a) {
          td.appendChild(el('a', { href: a.href }, el('button', { class: 'link small' }, a.label)));
        });
        if (entity.attachOwner) {
          td.appendChild(el('a', { href: '#/attachments/' + entity.attachOwner + '/' + row.id },
            el('button', { class: 'link small' }, 'Attachments')));
        }
        td.appendChild(el('a', { href: '#/e/' + entity.slug + '/edit/' + keyPath },
          el('button', { class: 'link small' }, 'Edit')));
        td.appendChild(el('button', {
          class: 'link small danger',
          onclick: function () { deleteEntity(entity, keyPath, row); },
        }, 'Delete'));
        return td;
      };
      table = filterableTable(rows, cols, { actions: actions, labels: labels });
    }

    setMain(el('div', null, [header, toolbar, table]));
  }

  async function deleteEntity(entity, keyPath, row) {
    if (!confirm('Delete this ' + entity.title.replace(/s$/, '') + '?')) return;
    try {
      await api('DELETE', entity.api + '/' + keyPath);
      toast('Deleted.');
      viewEntityList(entity);
    } catch (e) {
      toast(e.message, true);
    }
  }

  // ---- entity form view -------------------------------------------------
  async function buildFieldInput(f, value, disabled) {
    const wrap = el('div', { class: 'field' });
    const id = 'f_' + f.name;
    wrap.appendChild(el('label', { for: id }, f.label + (f.required ? ' *' : '')));
    let input;
    if (f.type === 'select') {
      input = el('select', { id: id, name: f.name });
      const options = f.options
        ? f.options.map(function (o) { return typeof o === 'string' ? { value: o, label: o } : o; })
        : await loadOptions(f.source);
      const current = value != null ? value : f.default;
      if (f.optional || current == null) input.appendChild(el('option', { value: '' }, '—'));
      options.forEach(function (o) { input.appendChild(el('option', { value: o.value }, o.label)); });
      if (current != null) input.value = String(current);
      if (f.required) input.required = true;
    } else if (f.type === 'bool') {
      input = el('input', { id: id, name: f.name, type: 'checkbox' });
      if (value === true || (value == null && f.default === true)) input.checked = true;
    } else {
      input = el('input', { id: id, name: f.name, type: f.type === 'date' ? 'date' : 'text' });
      if (f.type === 'decimal') input.setAttribute('inputmode', 'decimal');
      if (f.type === 'int') input.setAttribute('inputmode', 'numeric');
      const v = value != null ? value : (f.default != null ? f.default : '');
      input.value = v == null ? '' : String(v);
      if (f.required) input.required = true;
    }
    if (disabled) input.disabled = true;
    wrap.appendChild(input);
    if (f.hint) wrap.appendChild(el('div', { class: 'hint' }, f.hint));
    return wrap;
  }

  function readFieldValue(f, formEl) {
    const inp = formEl.querySelector('[name="' + f.name + '"]');
    if (f.type === 'bool') return inp.checked;
    const raw = (inp.value || '').trim();
    if (raw === '') {
      // Decimal fields with a default send that default; everything else empty
      // becomes null (a nullable/optional column, or an omitted optional date).
      if (f.type === 'decimal' && f.default != null && f.default !== '') return f.default;
      return null;
    }
    const encode = f.encode || (f.type === 'int' ? 'int' : 'string');
    if (encode === 'int') return Number(raw);
    return raw;
  }

  // ---- GST-inclusive brokerage wiring ------------------------------------
  // Exact decimal-string addition (the operands are money; float would drift).
  function addDecimalStrings(a, b) {
    a = String(a); b = String(b);
    const dp = function (s) { const i = s.indexOf('.'); return i < 0 ? 0 : s.length - i - 1; };
    const scale = Math.max(dp(a), dp(b));
    const scaled = function (s) { return BigInt(s.replace('.', '') + '0'.repeat(scale - dp(s))); };
    if (scale === 0) return (scaled(a) + scaled(b)).toString();
    const sum = (scaled(a) + scaled(b)).toString().padStart(scale + 1, '0');
    return (sum.slice(0, -scale) + '.' + sum.slice(-scale)).replace(/(\.\d*?)0+$/, '$1').replace(/\.$/, '');
  }

  // Shared by the Buy/DRP trade form and the Sell form: ticking "Brokerage
  // includes GST" hides the GST field (the server derives GST as 1/11 of the
  // inclusive amount, rounded to the cent) and relabels brokerage; editing a
  // flagged trade re-presents the stored ex-GST brokerage + GST as the one
  // inclusive amount that was originally entered.
  function wireGstBrokerage(form, existing) {
    const flag = form.querySelector('[name="brokerage_includes_gst"]');
    const brokInput = form.querySelector('[name="brokerage"]');
    const brokLabel = form.querySelector('label[for="f_brokerage"]');
    const gstWrap = form.querySelector('[name="gst_on_brokerage"]').closest('.field');
    if (existing && existing.brokerage_includes_gst) {
      brokInput.value = addDecimalStrings(existing.brokerage, existing.gst_on_brokerage);
    }
    function apply() {
      gstWrap.style.display = flag.checked ? 'none' : '';
      brokLabel.textContent = flag.checked ? 'Brokerage (GST-inclusive)' : 'Brokerage';
    }
    flag.addEventListener('change', apply);
    apply();
  }

  // ---- income simple-entry wiring -----------------------------------------
  // Exact decimal-string arithmetic for the non-negative money figures the
  // income form computes client-side (float would drift). decParts parses
  // "123.45" → { units: 12345n, dp: 2 } (null for a non-decimal); the
  // divisions round to the cent, half away from zero, matching statements.
  function decParts(s) {
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
  function mulToCents(a, b) {
    const pa = decParts(a), pb = decParts(b);
    if (!pa || !pb) return null;
    return divToCents(pa.units * pb.units, 10n ** BigInt(pa.dp + pb.dp));
  }
  // The franking credit on a fully franked amount at the 30% corporate rate:
  // amount × 30/70, cent-rounded (PLS example: 2757.30 → 1181.70).
  function frankingCreditFor(amount) {
    const p = decParts(amount);
    if (!p) return null;
    return divToCents(p.units * 3n, 7n * 10n ** BigInt(p.dp));
  }
  // Numeric decimal-string equality (1181.7 matches 1181.70).
  function decEq(a, b) {
    const pa = decParts(a), pb = decParts(b);
    if (!pa || !pb) return false;
    const scale = BigInt(Math.max(pa.dp, pb.dp));
    return pa.units * 10n ** (scale - BigInt(pa.dp)) === pb.units * 10n ** (scale - BigInt(pb.dp));
  }

  // The tax components a registry payment advice doesn't print (or that the
  // simple franking selector derives): hidden until the advanced toggle.
  const INCOME_ADVANCED_FIELDS = [
    'ex_date', 'franked_amount', 'unfranked_amount', 'foreign_source_income',
    'foreign_tax_paid', 'tfn_withholding_tax', 'franking_credits',
    'lic_capital_gain_deduction', 'conduit_foreign_income', 'trust_income',
    'currency', 'holding_account_id',
  ];

  // Classify a stored row for the simple form: which franking-selector mode
  // re-presents it losslessly, and the single amount to show. Returns null
  // when only the advanced fields can represent it (the form then opens
  // advanced so nothing is hidden) — any advanced-only field off its
  // default, a partially franked split, or franking credits that aren't the
  // derived amount × 30/70.
  function incomeSimpleShape(existing) {
    if (!existing) return { mode: 'Unfranked', amount: '' };
    const nz = function (v) { return v != null && !decEq(String(v), '0'); };
    if (existing.ex_date != null || nz(existing.foreign_source_income) || nz(existing.foreign_tax_paid)
      || nz(existing.tfn_withholding_tax) || nz(existing.lic_capital_gain_deduction)
      || nz(existing.conduit_foreign_income) || existing.currency !== 'AUD'
      || existing.holding_account_id !== 1) return null;
    const franked = nz(existing.franked_amount), credits = nz(existing.franking_credits);
    if (!franked && !credits) {
      return { mode: existing.trust_income ? 'Trust' : 'Unfranked', amount: String(existing.unfranked_amount) };
    }
    if (franked && !existing.trust_income && !nz(existing.unfranked_amount)
      && decEq(String(existing.franking_credits), frankingCreditFor(String(existing.franked_amount)) || '')) {
      return { mode: 'FullyFranked', amount: String(existing.franked_amount) };
    }
    return null;
  }

  // The income form's simple-first behaviour: a payment-amount input plus a
  // franking selector stand in for the component fields (mapped onto the
  // body at submit), the per-share pair shows its computed product as a
  // live hint, and a "Reinvested under DRP" tick chains the existing
  // POST /income/:id/reinvest after the save (a reinvest failure leaves the
  // saved income standing — the row's Reinvest action is the retry path).
  function wireIncomeEntry(form, existing) {
    const shape = incomeSimpleShape(existing);

    // Advanced toggle, at the top of the form.
    const advFlag = el('input', { id: 'f_simple_advanced', name: 'simple_advanced', type: 'checkbox' });
    advFlag.checked = shape == null;
    form.insertBefore(
      el('div', { class: 'field' }, [el('label', { for: 'f_simple_advanced' }, 'Show advanced fields'), advFlag]),
      form.firstChild
    );

    // Simple section: amount + franking selector, between the date and the
    // per-share pair.
    const amountInput = el('input', { id: 'f_simple_amount', name: 'simple_amount', type: 'text' });
    amountInput.setAttribute('inputmode', 'decimal');
    const frankSel = el('select', { id: 'f_simple_franking', name: 'simple_franking' });
    [
      { value: 'Unfranked', label: 'Unfranked' },
      { value: 'FullyFranked', label: 'Fully franked (30%)' },
      { value: 'Trust', label: 'Trust distribution' },
    ].forEach(function (o) { frankSel.appendChild(el('option', { value: o.value }, o.label)); });
    if (shape) { amountInput.value = shape.amount; frankSel.value = shape.mode; }
    const frankHint = el('div', { class: 'hint' });
    const simpleSection = el('div', null, [
      el('div', { class: 'field' }, [el('label', { for: 'f_simple_amount' }, 'Amount'), amountInput,
        el('div', { class: 'hint' }, 'The statement’s gross payment in AUD.')]),
      el('div', { class: 'field' }, [el('label', { for: 'f_simple_franking' }, 'Franking'), frankSel, frankHint]),
    ]);
    const apsWrap = form.querySelector('[name="amount_per_security"]').closest('.field');
    form.insertBefore(simpleSection, apsWrap);

    function updateFrankHint() {
      const credit = frankSel.value === 'FullyFranked' ? frankingCreditFor(amountInput.value) : null;
      frankHint.textContent = frankSel.value === 'FullyFranked'
        ? 'Franking credits of ' + (credit || 'amount × 30/70') + ' will be recorded (amount × 30/70). Partially franked or non-30%-rate dividends: use the advanced fields.'
        : (frankSel.value === 'Trust' ? 'Recorded as unfranked trust income; the component breakdown arrives with the AMMA statement for AMIT funds.' : '');
    }
    amountInput.addEventListener('input', updateFrankHint);
    frankSel.addEventListener('change', updateFrankHint);
    updateFrankHint();

    // Live product hint for the per-share cross-check pair.
    const apsInput = form.querySelector('[name="amount_per_security"]');
    const heldInput = form.querySelector('[name="securities_held"]');
    const productHint = el('div', { class: 'hint' });
    heldInput.closest('.field').appendChild(productHint);
    function grossEntered() {
      if (!advFlag.checked) return amountInput.value.trim();
      const f = form.querySelector('[name="franked_amount"]').value.trim() || '0';
      const u = form.querySelector('[name="unfranked_amount"]').value.trim() || '0';
      const fo = form.querySelector('[name="foreign_source_income"]').value.trim() || '0';
      if (!decParts(f) || !decParts(u) || !decParts(fo)) return '';
      return addDecimalStrings(addDecimalStrings(f, u), fo);
    }
    function updateProductHint() {
      const aps = apsInput.value.trim(), held = heldInput.value.trim();
      const product = aps && held ? mulToCents(aps, held) : null;
      if (!product) { productHint.textContent = ''; productHint.className = 'hint'; return; }
      const gross = grossEntered();
      const matches = gross !== '' && decParts(gross) != null && decEq(product, gross);
      productHint.textContent = aps + ' × ' + held + ' = ' + product
        + (matches ? ' — matches the gross amount.' : ' — does not match the gross amount entered.');
      productHint.className = matches ? 'hint' : 'hint warn';
    }
    form.addEventListener('input', updateProductHint);
    updateProductHint();

    // DRP tick: only for a distribution not yet reinvested.
    let drpFlag = null, priceInput = null, drpDateInput = null, drpFxInput = null;
    if (!existing || existing.reinvestment_trade_id == null) {
      drpFlag = el('input', { id: 'f_simple_drp', name: 'simple_drp', type: 'checkbox' });
      priceInput = el('input', { id: 'f_drp_price', name: 'drp_price', type: 'text' });
      priceInput.setAttribute('inputmode', 'decimal');
      drpDateInput = el('input', { id: 'f_drp_date', name: 'drp_date', type: 'date' });
      drpFxInput = el('input', { id: 'f_drp_fx', name: 'drp_fx', type: 'text', value: '1' });
      drpFxInput.setAttribute('inputmode', 'decimal');
      const drpFields = el('div', null, [
        el('div', { class: 'field' }, [el('label', { for: 'f_drp_price' }, 'Reinvestment price *'), priceInput,
          el('div', { class: 'hint' }, 'Per-unit DRP price from the statement; whole units and the carried residual are computed server-side.')]),
        el('div', { class: 'field' }, [el('label', { for: 'f_drp_date' }, 'Reinvestment trade date'), drpDateInput,
          el('div', { class: 'hint' }, 'Optional; defaults to the pay date.')]),
        el('div', { class: 'field' }, [el('label', { for: 'f_drp_fx' }, 'Reinvestment FX rate'), drpFxInput]),
      ]);
      form.appendChild(el('div', { class: 'field' }, [el('label', { for: 'f_simple_drp' }, 'Reinvested under DRP'), drpFlag,
        el('div', { class: 'hint' }, 'Creates the linked DRP trade after saving (the holding must be DRP-enrolled).')]));
      form.appendChild(drpFields);
      function applyDrp() {
        drpFields.style.display = drpFlag.checked ? '' : 'none';
        priceInput.required = drpFlag.checked;
      }
      drpFlag.addEventListener('change', applyDrp);
      applyDrp();
    }

    function applyMode() {
      const adv = advFlag.checked;
      simpleSection.style.display = adv ? 'none' : '';
      amountInput.required = !adv;
      INCOME_ADVANCED_FIELDS.forEach(function (n) {
        const inp = form.querySelector('[name="' + n + '"]');
        if (inp) inp.closest('.field').style.display = adv ? '' : 'none';
      });
      updateProductHint();
    }
    advFlag.addEventListener('change', applyMode);
    applyMode();

    return {
      // Simple mode maps the amount through the franking selector onto the
      // component fields; advanced mode submits the fields as entered.
      transformBody: function (body) {
        if (advFlag.checked) return;
        const amount = amountInput.value.trim();
        if (!decParts(amount)) throw new Error('Amount must be a decimal number.');
        const mode = frankSel.value;
        body.franked_amount = mode === 'FullyFranked' ? amount : '0';
        body.unfranked_amount = mode === 'FullyFranked' ? '0' : amount;
        body.franking_credits = mode === 'FullyFranked' ? frankingCreditFor(amount) : '0';
        body.trust_income = mode === 'Trust';
      },
      afterSave: async function (id) {
        if (!drpFlag || !drpFlag.checked) return null;
        const body = { reinvestment_price: priceInput.value.trim() };
        if (drpDateInput.value) body.date = drpDateInput.value;
        if (drpFxInput.value.trim() !== '') body.fx_rate = drpFxInput.value.trim();
        try {
          const trade = await api('POST', '/income/' + id + '/reinvest', body);
          return 'Saved and reinvested into trade #' + (trade ? trade.id : '?') + '.';
        } catch (e) {
          toast('Income saved, but the reinvestment failed — ' + e.message + '. Retry from the row’s Reinvest action.', true);
          return '';
        }
      },
    };
  }

  // ---- allocation editor --------------------------------------------------
  // Shared parcel-allocation row builder used by the Sell form, the Transfer
  // form, and the buy-back Participate action: a list of (purchase-parcel
  // select, decimal quantity) rows with add/remove buttons. Returns the
  // section element to append to the form and a `read()` harvesting the rows
  // as [{ purchase_trade_id, quantity_allocated }] (blank rows skipped). The
  // three callers differ only in labels and hint text.
  function allocationEditor(parcelOptions, existingAllocs, labels) {
    labels = Object.assign({
      heading: 'Parcel allocations',
      parcelLabel: 'Purchase parcel',
      qtyLabel: 'Quantity allocated',
      addLabel: '+ Add allocation',
    }, labels);
    const list = el('div');
    function addRow(alloc) {
      const purchaseSel = el('select', { name: 'alloc_purchase' });
      parcelOptions.forEach(function (o) { purchaseSel.appendChild(el('option', { value: o.value }, o.label)); });
      if (alloc) purchaseSel.value = String(alloc.purchase_trade_id);
      const qtyInput = el('input', { type: 'text', inputmode: 'decimal', name: 'alloc_qty', value: alloc ? String(alloc.quantity_allocated) : '' });
      const row = el('div', { class: 'alloc-row' }, [
        el('div', { class: 'field' }, [el('label', null, labels.parcelLabel), purchaseSel]),
        el('div', { class: 'field' }, [el('label', null, labels.qtyLabel), qtyInput]),
        el('button', { type: 'button', class: 'small danger', onclick: function () { list.removeChild(row); } }, 'Remove'),
      ]);
      list.appendChild(row);
    }
    if (existingAllocs && existingAllocs.length) existingAllocs.forEach(addRow); else addRow(null);
    const section = el('div', null, [
      el('h3', null, labels.heading),
      el('p', { class: 'hint' }, labels.hint),
      list,
      el('button', { type: 'button', class: 'small', onclick: function () { addRow(null); } }, labels.addLabel),
    ]);
    function read() {
      const allocs = [];
      list.querySelectorAll('.alloc-row').forEach(function (r) {
        const pid = r.querySelector('[name="alloc_purchase"]').value;
        const qty = (r.querySelector('[name="alloc_qty"]').value || '').trim();
        if (pid !== '' && qty !== '') allocs.push({ purchase_trade_id: Number(pid), quantity_allocated: qty });
      });
      return allocs;
    }
    return { section: section, read: read };
  }

  async function viewEntityForm(entity, keyParts) {
    setActiveNav(entity.slug);
    const editing = keyParts != null;
    const existing = editing ? await api('GET', entity.api + '/' + keyParts.join('/')) : null;

    // Per-type field grouping: an entity with `fieldGroups` (keyed by the
    // `typeField` select's value) renders only the chosen type's group after
    // the common fields (those in no group); the group re-renders on type
    // change and the matching `typeDescs` entry scopes the description.
    const grouped = {};
    Object.keys(entity.fieldGroups || {}).forEach(function (t) {
      entity.fieldGroups[t].forEach(function (n) { grouped[n] = true; });
    });

    const form = el('form');
    // Key fields: editable on create (unless auto), disabled on edit.
    for (const kf of entity.keyFields) {
      if (kf.auto) continue;
      const val = existing ? existing[kf.name] : null;
      form.appendChild(await buildFieldInput(kf, val, editing));
    }
    for (const f of entity.fields) {
      if (grouped[f.name]) continue;
      const val = existing ? existing[f.name] : null;
      form.appendChild(await buildFieldInput(f, val, false));
    }

    if (entity.fieldGroups) {
      const fieldByName = {};
      entity.fields.forEach(function (f) { fieldByName[f.name] = f; });
      const typeDesc = el('p', { class: 'hint' });
      const typeWarn = el('p', { class: 'hint warn' });
      const typeSection = el('div');
      form.appendChild(typeDesc);
      form.appendChild(typeWarn);
      form.appendChild(typeSection);
      const typeSel = form.querySelector('[name="' + entity.typeField + '"]');
      // Values typed into a group survive flipping the type away and back:
      // each re-render first harvests the outgoing inputs into `draft`, which
      // wins over the stored row when the group renders again.
      const draft = {};
      // Renders are async (fk fields fetch their options); the sequence number
      // makes a stale render abandon itself instead of appending its fields
      // after a newer selection's.
      let renderSeq = 0;
      async function renderTypeFields() {
        typeSection.querySelectorAll('[name]').forEach(function (inp) {
          draft[inp.name] = inp.type === 'checkbox' ? inp.checked : inp.value;
        });
        const t = typeSel.value;
        typeDesc.textContent = (entity.typeDescs && entity.typeDescs[t])
          || (t ? '' : 'Choose an action type above to see its fields.');
        typeWarn.textContent = editing && t && t !== existing[entity.typeField]
          ? 'Saving as ' + t + ' clears the saved ' + existing[entity.typeField] + ' fields.'
          : '';
        // Common fields with a per-type label (e.g. the date) take it here.
        Object.keys(entity.typeLabels || {}).forEach(function (n) {
          const label = entity.typeLabels[n][t] || fieldByName[n].label;
          form.querySelector('label[for="f_' + n + '"]').textContent = label + (fieldByName[n].required ? ' *' : '');
        });
        const seq = ++renderSeq;
        const inputs = [];
        for (const n of entity.fieldGroups[t] || []) {
          const val = draft[n] !== undefined ? draft[n] : (existing ? existing[n] : null);
          inputs.push(await buildFieldInput(fieldByName[n], val, false));
        }
        if (seq !== renderSeq) return; // a newer selection rendered meanwhile
        typeSection.innerHTML = '';
        inputs.forEach(function (node) { typeSection.appendChild(node); });
      }
      typeSel.addEventListener('change', renderTypeFields);
      await renderTypeFields();
    }

    // Entity-specific form behaviour beyond what the field configs express
    // (e.g. the trades form's GST-inclusive brokerage toggle). A hook may
    // return submit-time extensions: transformBody(body) maps UI-only
    // controls onto the entity body before the PUT, and afterSave(idPath)
    // chains a follow-up call, returning the success toast text (null = the
    // default 'Saved.', '' = it already toasted, e.g. its own failure).
    const wired = entity.wireForm ? entity.wireForm(form, existing) : null;

    const actions = el('div', { class: 'form-actions' });
    actions.appendChild(el('button', { type: 'submit', class: 'primary' }, editing ? 'Save' : 'Create'));
    actions.appendChild(el('a', { href: '#/e/' + entity.slug }, el('button', { type: 'button' }, 'Cancel')));
    form.appendChild(actions);

    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        // Resolve the key path.
        let keyVals;
        if (editing) {
          keyVals = entity.keyFields.map(function (kf) { return existing[kf.name]; });
        } else {
          keyVals = [];
          for (const kf of entity.keyFields) {
            keyVals.push(kf.auto ? await nextId(entity.api) : readFieldValue(kf, form));
          }
        }
        const body = {};
        entity.fields.forEach(function (f) {
          // A field whose type group is not selected has no input; it submits
          // as null, exactly as its blank input used to.
          body[f.name] = form.querySelector('[name="' + f.name + '"]') ? readFieldValue(f, form) : null;
        });
        if (wired && wired.transformBody) wired.transformBody(body);
        await api('PUT', entity.api + '/' + keyVals.join('/'), body);
        const msg = wired && wired.afterSave ? await wired.afterSave(keyVals.join('/')) : null;
        if (msg !== '') toast(msg || 'Saved.');
        location.hash = '#/e/' + entity.slug;
      } catch (e) {
        toast(e.message, true);
      }
    });

    setMain(el('div', null, [
      el('h2', null, (editing ? 'Edit ' : 'New ') + entity.title.replace(/s$/, '')),
      el('p', { class: 'view-desc' }, entity.desc),
      el('div', { class: 'card' }, form),
    ]));
  }

  // ---- Sells (trade + allocations, atomic) ------------------------------
  const SELL_FIELDS = [
    dt('date', 'Trade date', { required: true }),
    dt('settlement_date', 'Settlement date', { optional: true, hint: 'Leave blank to auto-calculate.' }),
    fk('listing_id', 'Listing', 'listings', { required: true }),
    dec('average_price', 'Average price', { required: true, default: '' }),
    dec('quantity', 'Quantity', { required: true, default: '' }),
    fk('currency', 'Currency', 'currencies', { required: true, encode: 'string' }),
    dec('brokerage', 'Brokerage'),
    bool('brokerage_includes_gst', 'Brokerage includes GST', { hint: 'Tick when the statement quotes brokerage GST-inclusive; the GST component (1/11, rounded to the cent) is derived automatically.' }),
    dec('gst_on_brokerage', 'GST on brokerage'),
    fk('brokerage_currency', 'Brokerage currency', 'currencies', { required: true, encode: 'string' }),
    dec('fx_rate', 'Manual FX rate', { default: '1' }),
    txt('contract_note_ref', 'Contract note ref', { optional: true }),
    dec('statement_total', 'Statement total', { optional: true, default: '', hint: 'Optional cross-check in the brokerage currency: quantity × price − brokerage − GST (net proceeds). Rejected if it does not reconcile.' }),
    fk('holding_account_id', 'Holding account', 'holdingAccounts', { required: true, default: '1', hint: 'Allocations may only consume parcels held in this account.' }),
  ];

  async function viewSellsList() {
    setActiveNav('sells');
    const sells = (await api('GET', '/trades')).filter(function (t) { return t.trade_type === 'Sell'; });
    const cols = ['id', 'date', 'settlement_date', 'listing_id', 'average_price', 'quantity', 'currency', 'statement_total', 'holding_account_id'];
    const toolbar = el('div', { class: 'toolbar' }, [
      el('a', { href: '#/sells/new' }, el('button', { class: 'primary' }, '+ New Sell')),
    ]);
    let table;
    if (sells.length === 0) {
      table = el('div', { class: 'empty' }, 'No sell trades yet.');
    } else {
      table = filterableTable(sells, cols, {
        labels: await fkLabelMaps({ listing_id: 'listings', holding_account_id: 'holdingAccounts' }),
        actions: function (row) {
          return el('td', { class: 'actions' }, [
            el('a', { href: '#/sells/edit/' + row.id }, el('button', { class: 'link small' }, 'Edit')),
            el('button', {
              class: 'link small danger',
              onclick: async function () {
                if (!confirm('Delete this Sell and its allocations?')) return;
                try { await api('DELETE', '/sells/' + row.id); toast('Deleted.'); viewSellsList(); }
                catch (e) { toast(e.message, true); }
              },
            }, 'Delete'),
          ]);
        },
      });
    }
    setMain(el('div', null, [
      el('h2', null, 'Sells'),
      el('p', { class: 'view-desc' }, 'Sell trades, each persisted atomically with parcel allocations that must sum exactly to the sell quantity.'),
      toolbar, table,
    ]));
  }

  async function viewSellForm(id) {
    setActiveNav('sells');
    const editing = id != null;
    const existing = editing ? await api('GET', '/trades/' + id) : null;
    let existingAllocs = [];
    if (editing) {
      existingAllocs = (await api('GET', '/parcel_allocations')).filter(function (a) { return a.sale_trade_id === Number(id); });
    }

    const form = el('form');
    for (const f of SELL_FIELDS) {
      form.appendChild(await buildFieldInput(f, existing ? existing[f.name] : null, false));
    }
    wireGstBrokerage(form, existing);

    // Allocations: the shared editor, pre-filled with the existing rows.
    const allocEditor = allocationEditor(await loadOptions('buyParcels'), existingAllocs, {
      hint: 'Allocations must sum exactly to the sell quantity. Each parcel must be a Buy/DRP with enough remaining units.',
    });

    const actions = el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, editing ? 'Save Sell' : 'Create Sell'),
      el('a', { href: '#/sells' }, el('button', { type: 'button' }, 'Cancel')),
    ]);

    form.appendChild(allocEditor.section);
    form.appendChild(actions);
    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        const body = {};
        SELL_FIELDS.forEach(function (f) { body[f.name] = readFieldValue(f, form); });
        body.allocations = allocEditor.read();
        const sellId = editing ? Number(id) : await nextId('/trades');
        await api('PUT', '/sells/' + sellId, body);
        toast('Sell saved.');
        location.hash = '#/sells';
      } catch (e) {
        toast(e.message, true);
      }
    });

    setMain(el('div', null, [
      el('h2', null, editing ? 'Edit Sell' : 'New Sell'),
      el('div', { class: 'card' }, form),
    ]));
  }

  // ---- Transfers between holding accounts --------------------------------
  // A transfer moves units of one listing between two holding accounts of the
  // same owner — not a CGT event. PUT /transfers/:id records and executes it
  // atomically (a price-0 transfer-out Sell consuming the chosen parcels in
  // the source account plus transfer-in Buys in the destination carrying each
  // parcel's remaining cost base and acquisition date); DELETE removes the
  // whole group and restores the pre-transfer holding. A transfer is
  // immutable — delete and re-transfer to change it.
  async function viewTransfersList() {
    setActiveNav('transfers');
    const rows = await api('GET', '/transfers');
    const cols = ['id', 'listing_id', 'date', 'from_account_id', 'to_account_id'];
    const toolbar = el('div', { class: 'toolbar' }, [
      el('a', { href: '#/transfers/new' }, el('button', { class: 'primary' }, '+ New Transfer')),
    ]);
    let table;
    if (rows.length === 0) {
      table = el('div', { class: 'empty' }, 'No transfers yet.');
    } else {
      table = filterableTable(rows, cols, {
        labels: await fkLabelMaps({ listing_id: 'listings', from_account_id: 'holdingAccounts', to_account_id: 'holdingAccounts' }),
        actions: function (row) {
          return el('td', { class: 'actions' }, [
            el('button', {
              class: 'link small danger',
              onclick: async function () {
                if (!confirm('Delete this transfer and restore the pre-transfer holding?')) return;
                try { await api('DELETE', '/transfers/' + row.id); toast('Deleted.'); viewTransfersList(); }
                catch (e) { toast(e.message, true); }
              },
            }, 'Delete'),
          ]);
        },
      });
    }
    setMain(el('div', null, [
      el('h2', null, 'Transfers'),
      el('p', { class: 'view-desc' }, entityBySlug.transfers.desc + ' Each parcel keeps its cost base and acquisition date; deleting a transfer restores the pre-transfer holding.'),
      toolbar, table,
    ]));
  }

  async function viewTransferForm() {
    setActiveNav('transfers');
    const form = el('form');
    const fields = [
      fk('listing_id', 'Listing', 'listings', { required: true }),
      dt('date', 'Transfer date', { required: true }),
      fk('from_account_id', 'From holding account', 'holdingAccounts', { required: true }),
      fk('to_account_id', 'To holding account', 'holdingAccounts', { required: true }),
    ];
    for (const f of fields) form.appendChild(await buildFieldInput(f, null, false));

    // The parcels to move — the same shape as a Sell's allocations (the shared
    // editor); partial parcels allowed.
    const allocEditor = allocationEditor(await loadOptions('buyParcels'), null, {
      heading: 'Parcels to move',
      hint: 'Each parcel must be a Buy/DRP of the chosen listing held in the source account, with enough remaining units. Moved units keep their cost base and acquisition date.',
      parcelLabel: 'Parcel to move',
      qtyLabel: 'Units to move',
      addLabel: '+ Add parcel',
    });
    form.appendChild(allocEditor.section);

    form.appendChild(el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, 'Transfer'),
      el('a', { href: '#/transfers' }, el('button', { type: 'button' }, 'Cancel')),
    ]));
    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        const body = {};
        fields.forEach(function (f) { body[f.name] = readFieldValue(f, form); });
        body.allocations = allocEditor.read();
        const result = await api('PUT', '/transfers/' + await nextId('/transfers'), body);
        const n = result && result.transfer_ins ? result.transfer_ins.length : 0;
        toast('Transferred ' + n + ' parcel(s) via transfer-out sell #' + (result && result.sell ? result.sell.id : '?') + '.');
        location.hash = '#/transfers';
      } catch (e) {
        toast(e.message, true);
      }
    });
    setMain(el('div', null, [
      el('h2', null, 'New Transfer'),
      el('p', { class: 'view-desc' }, 'Moves units between two holding accounts of the same owner — not a CGT event: nothing appears in the gains reports and each moved parcel keeps its cost base and acquisition date.'),
      el('div', { class: 'card' }, form),
    ]));
  }

  // ---- generic post-action view ------------------------------------------
  // Renders one ACTIONS entry: fetch the owning record, render the action's
  // fields (and the shared allocation editor when it takes parcel
  // allocations), POST the body to the action endpoint, toast the result, and
  // return to the owner's list. Null field values are omitted so server-side
  // defaults apply; the no-field confirm-only actions POST without a body.
  async function viewAction(action, id) {
    setActiveNav(action.nav);
    const owner = await api('GET', action.ownerApi + '/' + id);
    // Action descriptions name the listings they touch rather than printing
    // raw ids; unknown/null ids fall back to the old "listing N" wording.
    const listingName = await listingNamer();
    const fields = typeof action.fields === 'function' ? action.fields(owner) : action.fields;
    const form = el('form');
    for (const f of fields) form.appendChild(await buildFieldInput(f, null, false));
    let allocEditor = null;
    if (action.allocations) {
      allocEditor = allocationEditor(await loadOptions('buyParcels'), null, action.allocations);
      form.appendChild(allocEditor.section);
    }
    form.appendChild(el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, action.submit),
      el('a', { href: action.cancel }, el('button', { type: 'button' }, 'Cancel')),
    ]));
    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        let body;
        if (fields.length || allocEditor) {
          body = {};
          fields.forEach(function (f) {
            const v = readFieldValue(f, form);
            if (v != null) body[f.name] = v;
          });
          if (allocEditor) body.allocations = allocEditor.read();
        }
        const result = await api('POST', action.post(id), body);
        toast(action.toast(result));
        location.hash = action.cancel;
      } catch (e) {
        toast(e.message, true);
      }
    });
    setMain(el('div', null, [
      el('h2', null, action.title(id)),
      el('p', { class: 'view-desc' }, action.desc(owner, listingName)),
      el('div', { class: 'card' }, form),
    ]));
  }

  // ---- document attachments ---------------------------------------------
  // Reached from a Trade / Income / AMMA row's "Attachments" action. Lists the
  // activity's attachments (metadata only — never the blob), uploads a new file
  // via multipart/form-data (POST /attachments), and links each row to its
  // download (GET /attachments/:id/content). The owner field name (trade_id /
  // income_id / amma_statement_id) is carried in the route.
  const ATTACH_OWNER_LABEL = {
    trade_id: 'trade', income_id: 'income', amma_statement_id: 'AMMA statement',
  };

  async function viewAttachments(ownerField, ownerId) {
    const rows = await api('GET', '/attachments?' + ownerField + '=' + encodeURIComponent(ownerId));
    // checksum is stored integrity metadata, not user-facing — not a column here.
    const cols = ['id', 'filename', 'content_type', 'byte_size', 'uploaded_at'];

    const container = el('div');
    function refresh() { viewAttachments(ownerField, ownerId); }

    let table;
    if (rows.length === 0) {
      table = el('div', { class: 'empty' }, 'No attachments yet.');
    } else {
      table = filterableTable(rows, cols, {
        actions: function (row) {
          return el('td', { class: 'actions' }, [
            el('a', { href: '/attachments/' + row.id + '/content', target: '_blank' },
              el('button', { class: 'link small' }, 'Download')),
            el('button', {
              class: 'link small danger',
              onclick: async function () {
                if (!confirm('Delete this attachment?')) return;
                try { await api('DELETE', '/attachments/' + row.id); toast('Deleted.'); refresh(); }
                catch (e) { toast(e.message, true); }
              },
            }, 'Delete'),
          ]);
        },
      });
    }

    // Upload form: a single file input posted as multipart/form-data. The
    // browser sets the multipart boundary and the part's Content-Type; the
    // server validates it against the allowlist (pdf/png/jpeg) and the 25 MB cap.
    const fileInput = el('input', { type: 'file', name: 'file', required: true, accept: '.pdf,.png,.jpg,.jpeg' });
    const uploadForm = el('form', { class: 'card' }, [
      el('div', { class: 'field' }, [el('label', null, 'Add a file'), fileInput]),
      el('p', { class: 'hint' }, 'Accepted: PDF, PNG, JPEG. Max 25 MB. Stored in the database.'),
      el('div', { class: 'form-actions' }, [el('button', { type: 'submit', class: 'primary' }, 'Upload')]),
    ]);
    uploadForm.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      if (!fileInput.files || fileInput.files.length === 0) { toast('Choose a file first.', true); return; }
      try {
        const fd = new FormData();
        fd.append(ownerField, String(ownerId));
        fd.append('file', fileInput.files[0]);
        const res = await fetch('/attachments', { method: 'POST', body: fd });
        if (!res.ok) {
          let detail = '';
          try { detail = (await res.text()).trim(); } catch (e) { /* ignore */ }
          throw new Error('HTTP ' + res.status + (detail ? ': ' + detail : ''));
        }
        toast('Uploaded.');
        refresh();
      } catch (e) {
        toast(e.message, true);
      }
    });

    container.appendChild(el('h2', null, 'Attachments'));
    container.appendChild(el('p', { class: 'view-desc' },
      'Files attached to ' + (ATTACH_OWNER_LABEL[ownerField] || 'activity') + ' #' + ownerId + '. Stored in the database.'));
    container.appendChild(uploadForm);
    container.appendChild(table);
    setMain(container);
  }

  // ---- maintenance jobs -------------------------------------------------
  const JOB_DESC = {
    'backup': 'Copy the database to a dated backup file beside it (skipped if today\'s already exists).',
    'rba-fx-import': 'Fetch the RBA F11 monthly FX rates and import any new months.',
    'mic-import': 'Fetch and refresh the ISO 10383 MIC registry.',
    'currency-import': 'Fetch ISO 4217 fiat and ISO 24165 token currencies.',
    'price-import': 'Store the latest complete trading day\'s closing price for every held listing (skips days already stored).',
    'report-snapshot': 'Store the price-dependent reports\' results for the latest date the whole portfolio can be valued at with final prices (skips a date already stored fresh).',
  };

  async function viewJobs() {
    setActiveNav('jobs');
    // GET /jobs returns each registered job with its last run (started/finished
    // timestamps, success flag, error text), or nulls if it has never run.
    const jobs = await api('GET', '/jobs');
    const rows = jobs.map(function (j) {
      return {
        job: j.name,
        description: JOB_DESC[j.name] || '',
        last_run: j.last_finished_at || '',
        status: j.last_started_at == null ? 'never' : (j.last_success ? 'ok' : 'failed'),
        error: j.last_error || '',
      };
    });
    const cols = ['job', 'description', 'last_run', 'status', 'error'];
    const table = filterableTable(rows, cols, {
      statusField: 'status',
      actions: function (row) {
        const btn = el('button', { class: 'small primary' }, 'Run now');
        btn.addEventListener('click', async function () {
          btn.disabled = true;
          btn.textContent = 'Running…';
          try {
            await api('POST', '/jobs/' + row.job);
            toast("Job '" + row.job + "' completed.");
          } catch (e) {
            toast(e.message, true);
          } finally {
            // Reload so the table reflects the freshly recorded last run.
            viewJobs();
          }
        });
        return el('td', { class: 'actions' }, btn);
      },
    });
    setMain(el('div', null, [
      el('h2', null, 'Jobs'),
      el('p', { class: 'view-desc' }, 'Trigger scheduled maintenance jobs on demand, and see when each last ran (and any error). Each also runs automatically on its cron schedule; running here is for retries or missed runs.'),
      table,
    ]));
  }

  // ---- closing prices -----------------------------------------------------
  // Stored daily closing prices (incl. errored fetches) with the two on-demand
  // actions: re-fetch one (listing, day) — typically to replace an errored row
  // — and backfill a listing over a date range. Collection otherwise runs on
  // the price-import job's schedule.
  async function viewClosingPrices() {
    setActiveNav('closing_prices');
    const listings = await api('GET', '/listings');
    const byId = {};
    listings.forEach(function (l) { byId[l.id] = l; });
    const prices = await api('GET', '/closing_prices');
    const rows = prices.map(function (p) {
      const l = byId[p.listing_id];
      return {
        listing: p.listing_id + (l ? ': ' + l.ticker : ''),
        date: p.price_date,
        price: p.price == null ? '' : p.price,
        currency: l ? l.currency : '',
        source: p.source,
        status: p.status,
        error: p.error || '',
        fetched_at: p.fetched_at,
        _listing_id: p.listing_id,
      };
    });
    const cols = ['listing', 'date', 'price', 'currency', 'source', 'status', 'error', 'fetched_at'];
    const table = filterableTable(rows, cols, {
      statusField: 'status',
      actions: function (row) {
        const btn = el('button', { class: 'small' }, 'Re-fetch');
        btn.addEventListener('click', async function () {
          btn.disabled = true;
          btn.textContent = 'Fetching…';
          try {
            const stored = await api('POST', '/closing_prices/fetch',
              { listing_id: row._listing_id, price_date: row.date });
            if (stored.status === 'ok') toast('Stored ' + stored.price + ' for ' + row.date + '.');
            else toast('Fetch failed again: ' + stored.error, true);
          } catch (e) {
            toast(e.message, true);
          }
          viewClosingPrices();
        });
        return el('td', { class: 'actions' }, btn);
      },
    });

    // Backfill a listing's history over a date range (e.g. after importing an
    // old trade): trading days only, days already stored ok are skipped.
    const backfillForm = el('form', { class: 'card' });
    backfillForm.appendChild(el('h3', null, 'Backfill'));
    const listingSel = el('select', null, listings.map(function (l) {
      return el('option', { value: l.id }, l.id + ': ' + l.ticker + ' (' + (l.exchange_mic || 'Crypto') + ')');
    }));
    const fromInp = el('input', { type: 'date', required: true });
    const toInp = el('input', { type: 'date', required: true });
    backfillForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Listing'), listingSel]));
    backfillForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'From'), fromInp]));
    backfillForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'To'), toInp]));
    backfillForm.appendChild(el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, 'Backfill'),
    ]));
    backfillForm.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        const s = await api('POST', '/closing_prices/backfill', {
          listing_id: Number(listingSel.value), from: fromInp.value, to: toInp.value,
        });
        toast('Backfill: ' + s.fetched_ok + ' fetched, ' + s.already_stored + ' already stored, '
          + s.errored + ' errored (' + s.trading_days + ' trading days).', s.errored > 0);
        viewClosingPrices();
      } catch (e) {
        toast(e.message, true);
      }
    });

    setMain(el('div', null, [
      el('h2', null, 'Closing Prices'),
      el('p', { class: 'view-desc' },
        'Daily closing prices per held listing, in the listing\'s quote currency, collected by the '
        + 'price-import job after each exchange\'s close (crypto at the UTC-midnight cut-off). '
        + 'A failed fetch shows as an errored row — re-fetch it here once the provider recovers.'),
      backfillForm,
      table,
    ]));
  }

  // ---- report snapshots ---------------------------------------------------
  // Stored daily results of the price-dependent reports plus the time-series
  // graph. The SVG is built directly (no build step, no chart library): two
  // polylines — market value and unrealised gain — over the snapshot dates,
  // with stale snapshots' points hollow.
  function svgEl(tag, attrs) {
    const n = document.createElementNS('http://www.w3.org/2000/svg', tag);
    if (attrs) for (const k in attrs) { if (attrs[k] != null) n.setAttribute(k, attrs[k]); }
    return n;
  }

  function seriesChart(points) {
    if (!points || points.length < 2) {
      return el('p', { class: 'hint' }, 'The graph appears once two or more daily snapshots are stored.');
    }
    const W = 860, H = 280, padL = 84, padR = 16, padT = 12, padB = 30;
    const xs = points.map(function (p) { return new Date(p.snapshot_date + 'T00:00:00Z').getTime(); });
    let yMin = 0, yMax = 1;
    points.forEach(function (p) {
      [Number(p.market_value), Number(p.unrealised_gain), 0].forEach(function (v) {
        if (v < yMin) yMin = v;
        if (v > yMax) yMax = v;
      });
    });
    const xMin = xs[0], xMax = xs[xs.length - 1];
    const x = function (t) { return padL + (t - xMin) / (xMax - xMin || 1) * (W - padL - padR); };
    const y = function (v) { return H - padB - (v - yMin) / (yMax - yMin) * (H - padT - padB); };
    const chart = svgEl('svg', { viewBox: '0 0 ' + W + ' ' + H, class: 'series-chart', role: 'img' });
    // Horizontal gridlines with AUD labels.
    for (let i = 0; i <= 4; i++) {
      const v = yMin + (yMax - yMin) * i / 4;
      chart.appendChild(svgEl('line', { x1: padL, x2: W - padR, y1: y(v), y2: y(v), class: 'grid' }));
      const label = svgEl('text', { x: padL - 6, y: y(v) + 4, 'text-anchor': 'end', class: 'axis' });
      label.textContent = Math.round(v).toLocaleString();
      chart.appendChild(label);
    }
    // First and last snapshot dates on the x axis.
    [0, points.length - 1].forEach(function (i) {
      const label = svgEl('text', {
        x: x(xs[i]), y: H - 8, 'text-anchor': i === 0 ? 'start' : 'end', class: 'axis',
      });
      label.textContent = points[i].snapshot_date;
      chart.appendChild(label);
    });
    // One line + point markers per series; a stale snapshot's point is hollow.
    [['market_value', 'line-mv'], ['unrealised_gain', 'line-ug']].forEach(function (s) {
      const field = s[0], klass = s[1];
      const path = points.map(function (p, i) { return x(xs[i]) + ',' + y(Number(p[field])); }).join(' ');
      chart.appendChild(svgEl('polyline', { points: path, class: klass, fill: 'none' }));
      points.forEach(function (p, i) {
        const dot = svgEl('circle', {
          cx: x(xs[i]), cy: y(Number(p[field])), r: 3,
          class: klass + (p.stale ? ' stale' : ''),
        });
        const tip = svgEl('title');
        tip.textContent = p.snapshot_date + ': ' + p[field] + (p.stale ? ' (stale)' : '');
        dot.appendChild(tip);
        chart.appendChild(dot);
      });
    });
    return el('div', null, [
      chart,
      el('p', { class: 'hint' }, [
        el('span', { class: 'legend-mv' }, '— market value'),
        ' ',
        el('span', { class: 'legend-ug' }, '— unrealised gain'),
        ' (AUD; hollow points are stale snapshots)',
      ]),
    ]);
  }

  async function viewSnapshots() {
    setActiveNav('r:snapshots');
    const series = await api('GET', '/report_snapshots/series');
    const metas = await api('GET', '/report_snapshots');

    // On-demand generation: a past date whose prices have been backfilled, a
    // stale date after recording a back-dated fact, or (date blank) the
    // latest date the whole portfolio can be valued at.
    const genForm = el('form', { class: 'card' });
    const dateInp = el('input', { type: 'date' });
    genForm.appendChild(el('h3', null, 'Generate / regenerate'));
    genForm.appendChild(el('div', { class: 'field' }, [el('label', null, 'Snapshot date'), dateInp]));
    genForm.appendChild(el('p', { class: 'hint' },
      'Blank = the latest date with final prices for every held listing. Every held listing needs an ok stored closing price on (or walked back to) the date — backfill prices first for past dates.'));
    genForm.appendChild(el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, 'Generate'),
    ]));
    genForm.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        const body = dateInp.value ? { date: dateInp.value } : {};
        const stored = await api('POST', '/report_snapshots/generate', body);
        toast('Stored ' + stored.length + ' snapshot(s) for ' + stored[0].snapshot_date + '.');
        viewSnapshots();
      } catch (e) {
        toast(e.message, true);
      }
    });

    const rows = metas.slice().reverse().map(function (m) {
      return {
        date: m.snapshot_date,
        report: m.report,
        generated_at: m.generated_at,
        status: m.stale ? 'stale' : 'ok',
      };
    });
    const table = filterableTable(rows, ['date', 'report', 'generated_at', 'status'], {
      statusField: 'status',
      actions: function (row) {
        const view = el('a', { href: '#/r/snapshots/' + row.report + '/' + row.date }, 'View');
        const regen = el('button', { class: 'small' }, 'Regenerate');
        regen.addEventListener('click', async function () {
          regen.disabled = true;
          regen.textContent = 'Generating…';
          try {
            await api('POST', '/report_snapshots/generate', { date: row.date });
            toast('Regenerated ' + row.date + '.');
          } catch (e) {
            toast(e.message, true);
          }
          viewSnapshots();
        });
        return el('td', { class: 'actions' }, [view, ' ', regen]);
      },
    });

    setMain(el('div', null, [
      el('h2', null, 'Snapshots'),
      el('p', { class: 'view-desc' },
        'Daily stored results of the price-dependent reports, valued at the stored closing prices '
        + '(AUD-converted), written by the report-snapshot job after the day\'s last close. '
        + 'A back-dated fact marks every snapshot dated on or after it stale — the stored result '
        + 'keeps showing, flagged, until regenerated. A day whose price fetches failed has no '
        + 'snapshot at all until the price re-run succeeds.'),
      el('div', { class: 'card' }, [
        el('h3', null, 'Market value and unrealised gain over time'),
        seriesChart(series),
      ]),
      genForm,
      table,
    ]));
  }

  async function viewSnapshotDetail(report, date) {
    setActiveNav('r:snapshots');
    const snap = await api('GET', '/report_snapshots/' + report + '/' + date);
    const header = el('div', null, [
      el('h2', null, 'Snapshot: ' + report + ' @ ' + date),
      el('p', { class: 'view-desc', title: utcTooltip(snap.generated_at) },
        'Generated ' + cellText(snap.generated_at) + '. '),
      el('p', null, el('a', { href: '#/r/snapshots' }, '← All snapshots')),
    ]);
    if (snap.stale) {
      header.appendChild(el('p', { class: 'hint warn' },
        'Stale: a back-dated fact was recorded after this snapshot was generated. The rows below are the stored (pre-fact) result — regenerate from the Snapshots view.'));
    }
    setMain(el('div', null, [header, dataTable(snap.rows)]));
  }

  // ---- reports ----------------------------------------------------------
  async function viewReport(report) {
    setActiveNav('r:' + report.slug);
    const header = el('div', null, [
      el('h2', null, report.title),
      el('p', { class: 'view-desc' }, report.desc),
    ]);
    // Tax-return-ready CSV download: reports flagged `export` serve the same
    // rows as CSV from `<api>/export` (Content-Disposition makes it a download).
    if (report.export) {
      header.appendChild(el('p', null,
        el('a', { href: report.api + '/export', class: 'export-link' }, 'Export CSV')));
    }
    const result = el('div');

    function render(rows) {
      result.innerHTML = '';
      result.appendChild(dataTable(rows, null, report.statusField));
    }

    if (report.method === 'GET') {
      render(await api('GET', report.api));
      setMain(el('div', null, [header, result]));
      return;
    }

    // POST reports take optional market prices (per listing) and an optional date.
    const listings = await api('GET', '/listings');
    const priceForm = el('form', { class: 'card' });
    priceForm.appendChild(el('h3', null, 'Current prices (AUD, optional)'));
    listings.forEach(function (l) {
      priceForm.appendChild(el('div', { class: 'field' }, [
        el('label', null, l.id + ': ' + l.ticker + ' (' + (l.exchange_mic || 'Crypto') + ')'),
        el('input', { type: 'text', inputmode: 'decimal', 'data-listing': l.id }),
      ]));
    });
    if (report.asOfDate) {
      priceForm.appendChild(el('div', { class: 'field' }, [
        el('label', null, 'As-of date'),
        el('input', { type: 'date', name: 'as_of_date' }),
      ]));
    }
    priceForm.appendChild(el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, 'Run report'),
    ]));
    priceForm.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        const prices = {};
        priceForm.querySelectorAll('[data-listing]').forEach(function (inp) {
          const v = (inp.value || '').trim();
          if (v !== '') prices[inp.getAttribute('data-listing')] = v;
        });
        const body = { prices: prices };
        if (report.asOfDate) {
          const d = (priceForm.querySelector('[name="as_of_date"]').value || '').trim();
          if (d !== '') body.as_of_date = d;
        }
        render(await api('POST', report.api, body));
      } catch (e) {
        toast(e.message, true);
      }
    });
    setMain(el('div', null, [header, priceForm, result]));
  }

  // ---- router -----------------------------------------------------------
  async function render() {
    const hash = (location.hash || '').replace(/^#/, '');
    const parts = hash.split('/').filter(Boolean);
    try {
      if (parts.length === 0) { location.hash = '#/r/overview'; return; }
      if (parts[0] === 'e') {
        const entity = entityBySlug[parts[1]];
        if (!entity) throw new Error('Unknown view');
        if (parts[2] === 'new') return await viewEntityForm(entity, null);
        if (parts[2] === 'edit') return await viewEntityForm(entity, parts.slice(3));
        return await viewEntityList(entity);
      }
      if (parts[0] === 'sells') {
        if (parts[1] === 'new') return await viewSellForm(null);
        if (parts[1] === 'edit') return await viewSellForm(parts[2]);
        return await viewSellsList();
      }
      if (parts[0] === 'transfers') {
        if (parts[1] === 'new') return await viewTransferForm();
        return await viewTransfersList();
      }
      if (actionBySlug[parts[0]]) return await viewAction(actionBySlug[parts[0]], parts[1]);
      if (parts[0] === 'attachments') return await viewAttachments(parts[1], parts[2]);
      if (parts[0] === 'jobs') return await viewJobs();
      if (parts[0] === 'prices') return await viewClosingPrices();
      if (parts[0] === 'r') {
        const report = reportBySlug[parts[1]];
        if (!report) throw new Error('Unknown report');
        if (report.custom === 'snapshots') {
          if (parts[2] && parts[3]) return await viewSnapshotDetail(parts[2], parts[3]);
          return await viewSnapshots();
        }
        return await viewReport(report);
      }
      throw new Error('Not found');
    } catch (e) {
      setMain(el('div', { class: 'error' }, e.message));
    }
  }

  buildNav();
  window.addEventListener('hashchange', render);
  render();
})();
