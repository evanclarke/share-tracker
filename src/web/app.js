'use strict';
//
// share-tracker single-page frontend.
//
// No build step, no framework: a small config-driven engine renders a CRUD view
// for every domain entity and a table view for every report, all driving the
// existing JSON API on the same origin. Each entity is described once (its API
// path, key, and fields); generic list/form code does the rest. Sells (which
// must be written atomically with their parcel allocations) and DRP reinvestment
// are the two flows with bespoke views.
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

  function cellText(v) {
    if (v == null) return '';
    if (typeof v === 'boolean') return v ? 'yes' : 'no';
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
        return (await api('GET', '/listings')).map(function (l) { return { value: l.id, label: l.id + ': ' + l.ticker + ' (' + l.exchange_mic + ')' }; });
      case 'amma':
        return (await api('GET', '/amma_statements')).map(function (a) { return { value: a.id, label: a.id + ': listing ' + a.listing_id + ' FY' + a.tax_year_end_date }; });
      case 'buyParcels':
        return (await api('GET', '/trades')).filter(function (t) { return t.trade_type !== 'Sell'; })
          .map(function (t) { return { value: t.id, label: t.id + ': ' + t.trade_type + ' ' + t.quantity + ' (listing ' + t.listing_id + ', ' + t.date + ')' }; });
      default:
        return [];
    }
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
      ],
      columns: ['mic', 'name', 'country', 'currency', 'timezone', 'settlement_days'],
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
      desc: 'Securities you trade, each on a curated exchange.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        fk('exchange_mic', 'Exchange', 'exchanges', { required: true, encode: 'string' }),
        txt('ticker', 'Ticker', { required: true }),
        txt('name', 'Name', { required: true }),
        txt('isin', 'ISIN', { optional: true }),
        sel('security_type', 'Security type', ['Share', 'ETF', 'LIC', 'Trust'], { required: true }),
        fk('currency', 'Currency', 'currencies', { required: true, encode: 'string' }),
        bool('amit', 'AMIT'),
      ],
      columns: ['id', 'exchange_mic', 'ticker', 'name', 'isin', 'security_type', 'currency', 'amit'],
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
        dec('gst_on_brokerage', 'GST on brokerage'),
        fk('brokerage_currency', 'Brokerage currency', 'currencies', { required: true, encode: 'string' }),
        dec('fx_rate', 'Manual FX rate', { default: '1', hint: 'Foreign units per AUD; fallback used only when no ATO rate exists. 1 for AUD.' }),
        txt('contract_note_ref', 'Contract note ref', { optional: true }),
      ],
      columns: ['id', 'trade_type', 'date', 'settlement_date', 'listing_id', 'average_price', 'quantity', 'currency', 'brokerage', 'fx_rate'],
      listFilter: function (row) { return row.trade_type !== 'Sell'; },
    },
    {
      slug: 'sells', title: 'Sells', group: 'Activity', api: '/sells', custom: 'sells',
      desc: 'Sell trades created atomically with their parcel allocations.',
    },
    {
      slug: 'income', title: 'Income', group: 'Activity', api: '/income',
      desc: 'Dividends and trust distributions with a full tax-component breakdown.',
      keyFields: [int('id', 'ID', { auto: true })],
      fields: [
        fk('listing_id', 'Listing', 'listings', { required: true }),
        dt('date_paid', 'Date paid', { required: true }),
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
      ],
      columns: ['id', 'listing_id', 'date_paid', 'franked_amount', 'unfranked_amount', 'franking_credits', 'currency', 'reinvestment_trade_id'],
      rowActions: function (row) {
        return row.reinvestment_trade_id == null ? [{ label: 'Reinvest', href: '#/reinvest/' + row.id }] : [];
      },
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
      ],
      columns: ['id', 'listing_id', 'tax_year_end_date', 'units_held', 'cost_base_adjustment', 'currency'],
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
      desc: 'Holdings that reinvest their distributions in full.',
      keyFields: [fk('listing_id', 'Listing', 'listings', { required: true, encode: 'int' })],
      fields: [sel('residual_handling', 'Residual handling', ['CarryForward', 'PayOut'], { required: true })],
      columns: ['listing_id', 'residual_handling'],
    },
    {
      slug: 'jobs', title: 'Jobs', group: 'Maintenance', api: '/jobs', custom: 'jobs',
      desc: 'Run scheduled maintenance jobs (backup, reference-data imports) on demand.',
    },
  ];

  const REPORTS = [
    { slug: 'overview', title: 'Portfolio Overview', api: '/portfolio/overview', method: 'POST', prices: true, desc: 'Open holdings per listing, with optional market value.' },
    { slug: 'unrealised-gains', title: 'Unrealised Gains', api: '/portfolio/unrealised-gains', method: 'POST', prices: true, asOfDate: true, desc: 'Per-holding unrealised gain/loss vs cost base.' },
    { slug: 'realised-gains', title: 'Realised Gains', api: '/portfolio/realised-gains', method: 'GET', desc: 'Per-sale capital gain/loss split into CGT buckets.' },
    { slug: 'net-capital-gain', title: 'Net Capital Gain', api: '/portfolio/net-capital-gain', method: 'GET', desc: 'Assessable net capital gain per financial year.' },
    { slug: 'tax-summary', title: 'Tax Summary', api: '/portfolio/tax-summary', method: 'GET', desc: 'Income aggregated by Australian financial year.' },
    { slug: 'exchange-mic-validation', title: 'Exchange MIC Validation', api: '/reports/exchange_mic_validation', method: 'GET', statusField: 'registry_status', desc: 'Curated exchanges checked against the ISO MIC registry.' },
  ];

  const entityBySlug = {};
  ENTITIES.forEach(function (e) { entityBySlug[e.slug] = e; });
  const reportBySlug = {};
  REPORTS.forEach(function (r) { reportBySlug[r.slug] = r; });

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
  function dataTable(rows, columns, statusField) {
    if (!rows || rows.length === 0) return el('div', { class: 'empty' }, 'No records.');
    const cols = columns || Object.keys(rows[0]);
    const thead = el('tr', null, cols.map(function (c) {
      return el('th', { class: looksNumeric(rows[0][c]) ? 'num' : null }, c);
    }));
    const body = rows.map(function (row) {
      return el('tr', null, cols.map(function (c) {
        const v = row[c];
        if (statusField && c === statusField) {
          return el('td', null, el('span', { class: 'badge ' + cellText(v) }, cellText(v)));
        }
        return el('td', { class: looksNumeric(v) ? 'num' : null }, cellText(v));
      }));
    });
    return el('table', null, [el('thead', null, thead), el('tbody', null, body)]);
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
      const allCols = entity.readonly ? cols : cols.concat(['']);
      const thead = el('tr', null, allCols.map(function (c) {
        return el('th', { class: looksNumeric(rows[0][c]) ? 'num' : null }, c === '' ? 'Actions' : c);
      }));
      const body = rows.map(function (row) {
        const tds = cols.map(function (c) {
          return el('td', { class: looksNumeric(row[c]) ? 'num' : null }, cellText(row[c]));
        });
        if (!entity.readonly) {
          const keyPath = entity.keyFields.map(function (kf) { return row[kf.name]; }).join('/');
          const actions = el('td', { class: 'actions' });
          (entity.rowActions ? entity.rowActions(row) : []).forEach(function (a) {
            actions.appendChild(el('a', { href: a.href }, el('button', { class: 'link small' }, a.label)));
          });
          actions.appendChild(el('a', { href: '#/e/' + entity.slug + '/edit/' + keyPath },
            el('button', { class: 'link small' }, 'Edit')));
          actions.appendChild(el('button', {
            class: 'link small danger',
            onclick: function () { deleteEntity(entity, keyPath, row); },
          }, 'Delete'));
          tds.push(actions);
        }
        return el('tr', null, tds);
      });
      table = el('table', null, [el('thead', null, thead), el('tbody', null, body)]);
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

  async function viewEntityForm(entity, keyParts) {
    setActiveNav(entity.slug);
    const editing = keyParts != null;
    const existing = editing ? await api('GET', entity.api + '/' + keyParts.join('/')) : null;

    const form = el('form');
    // Key fields: editable on create (unless auto), disabled on edit.
    for (const kf of entity.keyFields) {
      if (kf.auto) continue;
      const val = existing ? existing[kf.name] : null;
      form.appendChild(await buildFieldInput(kf, val, editing));
    }
    for (const f of entity.fields) {
      const val = existing ? existing[f.name] : null;
      form.appendChild(await buildFieldInput(f, val, false));
    }

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
        entity.fields.forEach(function (f) { body[f.name] = readFieldValue(f, form); });
        await api('PUT', entity.api + '/' + keyVals.join('/'), body);
        toast('Saved.');
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
    dec('gst_on_brokerage', 'GST on brokerage'),
    fk('brokerage_currency', 'Brokerage currency', 'currencies', { required: true, encode: 'string' }),
    dec('fx_rate', 'Manual FX rate', { default: '1' }),
    txt('contract_note_ref', 'Contract note ref', { optional: true }),
  ];

  async function viewSellsList() {
    setActiveNav('sells');
    const sells = (await api('GET', '/trades')).filter(function (t) { return t.trade_type === 'Sell'; });
    const cols = ['id', 'date', 'settlement_date', 'listing_id', 'average_price', 'quantity', 'currency'];
    const toolbar = el('div', { class: 'toolbar' }, [
      el('a', { href: '#/sells/new' }, el('button', { class: 'primary' }, '+ New Sell')),
    ]);
    let table;
    if (sells.length === 0) {
      table = el('div', { class: 'empty' }, 'No sell trades yet.');
    } else {
      const thead = el('tr', null, cols.map(function (c) { return el('th', { class: looksNumeric(sells[0][c]) ? 'num' : null }, c); }).concat([el('th', null, 'Actions')]));
      const body = sells.map(function (row) {
        const tds = cols.map(function (c) { return el('td', { class: looksNumeric(row[c]) ? 'num' : null }, cellText(row[c])); });
        const actions = el('td', { class: 'actions' }, [
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
        tds.push(actions);
        return el('tr', null, tds);
      });
      table = el('table', null, [el('thead', null, thead), el('tbody', null, body)]);
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

    // Allocations builder.
    const parcelOptions = await loadOptions('buyParcels');
    const allocList = el('div');
    function addAllocRow(alloc) {
      const purchaseSel = el('select', { name: 'alloc_purchase' });
      parcelOptions.forEach(function (o) { purchaseSel.appendChild(el('option', { value: o.value }, o.label)); });
      if (alloc) purchaseSel.value = String(alloc.purchase_trade_id);
      const qtyInput = el('input', { type: 'text', inputmode: 'decimal', name: 'alloc_qty', value: alloc ? String(alloc.quantity_allocated) : '' });
      const row = el('div', { class: 'alloc-row' }, [
        el('div', { class: 'field' }, [el('label', null, 'Purchase parcel'), purchaseSel]),
        el('div', { class: 'field' }, [el('label', null, 'Quantity allocated'), qtyInput]),
        el('button', { type: 'button', class: 'small danger', onclick: function () { allocList.removeChild(row); } }, 'Remove'),
      ]);
      allocList.appendChild(row);
    }
    if (existingAllocs.length) existingAllocs.forEach(addAllocRow); else addAllocRow(null);

    const allocSection = el('div', null, [
      el('h3', null, 'Parcel allocations'),
      el('p', { class: 'hint' }, 'Allocations must sum exactly to the sell quantity. Each parcel must be a Buy/DRP with enough remaining units.'),
      allocList,
      el('button', { type: 'button', class: 'small', onclick: function () { addAllocRow(null); } }, '+ Add allocation'),
    ]);

    const actions = el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, editing ? 'Save Sell' : 'Create Sell'),
      el('a', { href: '#/sells' }, el('button', { type: 'button' }, 'Cancel')),
    ]);

    form.appendChild(allocSection);
    form.appendChild(actions);
    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        const body = {};
        SELL_FIELDS.forEach(function (f) { body[f.name] = readFieldValue(f, form); });
        const allocs = [];
        allocList.querySelectorAll('.alloc-row').forEach(function (r) {
          const pid = r.querySelector('[name="alloc_purchase"]').value;
          const qty = (r.querySelector('[name="alloc_qty"]').value || '').trim();
          if (pid !== '' && qty !== '') allocs.push({ purchase_trade_id: Number(pid), quantity_allocated: qty });
        });
        body.allocations = allocs;
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

  // ---- DRP reinvestment -------------------------------------------------
  async function viewReinvest(incomeId) {
    setActiveNav('income');
    const income = await api('GET', '/income/' + incomeId);
    const form = el('form');
    const fields = [
      dec('reinvestment_price', 'Reinvestment price', { required: true, default: '' }),
      dec('fx_rate', 'FX rate', { default: '1', hint: 'Optional; defaults to 1.' }),
      dt('date', 'Trade date', { optional: true, hint: 'Optional; defaults to the distribution pay date (' + income.date_paid + ').' }),
    ];
    for (const f of fields) form.appendChild(await buildFieldInput(f, null, false));
    form.appendChild(el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, 'Reinvest'),
      el('a', { href: '#/e/income' }, el('button', { type: 'button' }, 'Cancel')),
    ]));
    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      try {
        const body = { reinvestment_price: readFieldValue(fields[0], form) };
        const fxr = readFieldValue(fields[1], form);
        const d = readFieldValue(fields[2], form);
        if (fxr != null) body.fx_rate = fxr;
        if (d != null) body.date = d;
        const trade = await api('POST', '/income/' + incomeId + '/reinvest', body);
        toast('Reinvested into trade #' + (trade ? trade.id : '?') + '.');
        location.hash = '#/e/income';
      } catch (e) {
        toast(e.message, true);
      }
    });
    setMain(el('div', null, [
      el('h2', null, 'Reinvest distribution #' + incomeId),
      el('p', { class: 'view-desc' }, 'Creates a DRP trade for listing ' + income.listing_id + ' and links it back to this distribution. The holding must be DRP-enrolled.'),
      el('div', { class: 'card' }, form),
    ]));
  }

  // ---- maintenance jobs -------------------------------------------------
  const JOB_DESC = {
    'backup': 'Copy the database to a dated backup file beside it (skipped if today\'s already exists).',
    'rba-fx-import': 'Fetch the RBA F11 monthly FX rates and import any new months.',
    'mic-import': 'Fetch and refresh the ISO 10383 MIC registry.',
    'currency-import': 'Fetch ISO 4217 fiat and ISO 24165 token currencies.',
  };

  async function viewJobs() {
    setActiveNav('jobs');
    const names = await api('GET', '/jobs');
    const rows = names.map(function (name) {
      const status = el('span', { class: 'hint' }, '');
      const btn = el('button', { class: 'small primary' }, 'Run now');
      btn.addEventListener('click', async function () {
        const orig = btn.textContent;
        btn.disabled = true;
        btn.textContent = 'Running…';
        status.textContent = '';
        try {
          await api('POST', '/jobs/' + name);
          status.textContent = 'Completed.';
          toast("Job '" + name + "' completed.");
        } catch (e) {
          status.textContent = 'Failed.';
          toast(e.message, true);
        } finally {
          btn.disabled = false;
          btn.textContent = orig;
        }
      });
      return el('tr', null, [
        el('td', null, name),
        el('td', null, JOB_DESC[name] || ''),
        el('td', { class: 'actions' }, [btn, ' ', status]),
      ]);
    });
    const table = el('table', null, [
      el('thead', null, el('tr', null, [el('th', null, 'Job'), el('th', null, 'Description'), el('th', null, 'Action')])),
      el('tbody', null, rows),
    ]);
    setMain(el('div', null, [
      el('h2', null, 'Jobs'),
      el('p', { class: 'view-desc' }, 'Trigger scheduled maintenance jobs on demand. Each also runs automatically on its cron schedule; running here is for retries or missed runs.'),
      table,
    ]));
  }

  // ---- reports ----------------------------------------------------------
  async function viewReport(report) {
    setActiveNav('r:' + report.slug);
    const header = el('div', null, [
      el('h2', null, report.title),
      el('p', { class: 'view-desc' }, report.desc),
    ]);
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
        el('label', null, l.id + ': ' + l.ticker + ' (' + l.exchange_mic + ')'),
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
      if (parts[0] === 'reinvest') return await viewReinvest(parts[1]);
      if (parts[0] === 'jobs') return await viewJobs();
      if (parts[0] === 'r') {
        const report = reportBySlug[parts[1]];
        if (!report) throw new Error('Unknown report');
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
