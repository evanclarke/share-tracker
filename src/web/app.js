//
// share-tracker single-page frontend: the rendering engine and entry point.
//
// No build step, no framework — native ES modules loaded straight by the
// browser (<script type="module">). This module holds the generic machinery:
// the filterable/sortable/paginated table every data view renders through,
// the config-driven entity list/form views, the report runner, the
// post-record action view, the bespoke Sells/Transfers flows (written
// atomically with their parcel allocations), and the hash router. What it
// renders is described in config.js (ENTITIES / REPORTS / ACTIONS), built
// from the field constructors and form wiring in forms.js, over the shared
// utilities in util.js.
//
import {
  el, toast, setMain, looksNumeric, isTimestamp, fmtLocalTimestamp, utcTooltip,
  cellText, numericDisplay, columnKinds, columnLabel, columnLabelMaps,
  fkLabelMaps, api, nextId, loadOptions, listingNamer, describeTrade,
} from './util.js';
import {
  field, txt, dec, dt, bool, fk,
  buildFieldInput, readFieldValue, wireGstBrokerage, allocationEditor,
} from './forms.js';
import { ENTITIES, REPORTS, ACTIONS } from './config.js';

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
// Default page size: tables with this many rows or fewer show no pager; a
// larger filtered result set is paged so only one page is in the DOM at once.
const PAGE_SIZE = 50;

// Every data table in the app — entity lists, the Sells list, and report
// tables — goes through this one renderer so they are uniformly filterable,
// sortable, and paginated. Each column has its own filter input (substring
// match on that column's text); the filters AND together, so you can e.g.
// filter currency to "USD" and date to "2024" at once. Click a column header
// to sort it (toggling ascending/descending). Filtering and sorting apply to
// the whole result set; only the current page's slice (PAGE_SIZE rows) is put
// in the DOM, with a prev/next pager + "showing m–n of total" count that
// appears only when the filtered total exceeds one page. `opts.actions`, if
// given, renders a trailing non-sortable, non-filtered Actions cell per row;
// `opts.statusField` renders that column as a status badge; `opts.labels`
// ({col: {id → label}}, from fkLabelMaps) shows the label instead of the raw
// foreign-key id — filtering and sorting follow the label, and the id moves to
// the tooltip. `opts.expand`, if given, is a synchronous `row => childSpec`
// (`{ rows, cols, opts }`, or a falsy value/empty `rows` for a childless row):
// a leading toggle column shows ▸/▾ for a row with children, and an expanded
// row's children render as a nested `filterableTable` in a full-width detail
// row underneath — `childSpec.opts` may itself carry `expand`, so a report
// with a two-level breakdown (year → disposal → parcel) nests by recursion.
// An "Expand all"/"Collapse all" pair sits above the table whenever any
// caller supplies `expand`.
function filterableTable(rows, cols, opts) {
  opts = opts || {};
  const statusField = opts.statusField;
  const actions = opts.actions;
  const expand = opts.expand;
  const labels = opts.labels || {};
  // Display kinds are derived from the column names alone (COLUMN_KINDS), so
  // every caller of filterableTable gets money rounding / rate precision with
  // no per-call wiring; numeric sorting still uses the raw underlying value.
  const kinds = columnKinds(cols);
  function displayText(row, c) {
    const map = labels[c];
    if (map && row[c] != null && map[row[c]] !== undefined) return map[row[c]];
    const nd = numericDisplay(row[c], kinds[c]);
    if (nd) return nd.text;
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
  let page = 0; // zero-based current page within the filtered/sorted result set
  // Rows currently shown expanded (row object identity is stable across
  // sort/filter/page re-renders, so a plain Set works as the key). Only
  // meaningful when `expand` is set.
  const expandedRows = new Set();

  const container = el('div');

  if (expand) {
    container.appendChild(el('div', { class: 'expand-all-bar' }, [
      el('button', {
        type: 'button', class: 'small link',
        onclick: function () {
          rows.forEach(function (row) {
            const spec = expand(row);
            if (spec && spec.rows && spec.rows.length) expandedRows.add(row);
          });
          renderBody();
        },
      }, 'Expand all'),
      el('button', {
        type: 'button', class: 'small link',
        onclick: function () { expandedRows.clear(); renderBody(); },
      }, 'Collapse all'),
    ]));
  }

  // Header row: click-to-sort column titles, shown as friendly labels
  // (columnLabel) while sorting/filtering stay keyed by the raw column name.
  const headCells = cols.map(function (c) {
    const indicator = el('span', { class: 'sort-ind' }, '');
    const th = el('th', { class: (numeric[c] ? 'num ' : '') + 'sortable' }, [columnLabel(c), indicator]);
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
  if (expand) headCells.unshift(el('th', { class: 'expand-col' }, ''));
  if (actions) headCells.push(el('th', null, 'Actions'));

  // Filter row: one input per column, AND-combined.
  const filterCells = cols.map(function (c) {
    const input = el('input', {
      type: 'search', class: 'table-filter', placeholder: 'Filter ' + columnLabel(c) + '…',
      oninput: function () {
        const v = this.value.trim().toLowerCase();
        if (v === '') delete filters[c]; else filters[c] = v;
        page = 0; // a changed filter re-pages from the first page
        renderBody();
      },
    });
    return el('th', { class: 'filter-cell' }, input);
  });
  if (expand) filterCells.unshift(el('th', { class: 'filter-cell' }));
  if (actions) filterCells.push(el('th', { class: 'filter-cell' }));

  const tbody = el('tbody');
  const thead = el('thead', null, [
    el('tr', null, headCells),
    el('tr', { class: 'filter-row' }, filterCells),
  ]);
  container.appendChild(el('table', null, [thead, tbody]));

  // Pager: a "showing m–n of total" count flanked by prev/next. Hidden when
  // the filtered total fits one page; updatePager toggles it per render.
  const pagerInfo = el('span', { class: 'pager-info' });
  const prevBtn = el('button', {
    type: 'button', class: 'small', onclick: function () { if (page > 0) { page--; renderBody(); } },
  }, '‹ Prev');
  const nextBtn = el('button', {
    type: 'button', class: 'small', onclick: function () { page++; renderBody(); },
  }, 'Next ›');
  const pager = el('div', { class: 'pager', hidden: true }, [prevBtn, pagerInfo, nextBtn]);
  container.appendChild(pager);

  // The whole filtered/sorted result set, paged only at render time so the
  // count and sort order always reflect the full set, never the visible page.
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

  // Show the pager (and set its count) only when the filtered total spills
  // past one page; `start` is the zero-based index of the first shown row.
  function updatePager(total, start, shown) {
    if (total <= PAGE_SIZE) { pager.hidden = true; return; }
    pager.hidden = false;
    pagerInfo.textContent = 'showing ' + (start + 1) + '–' + (start + shown) + ' of ' + total;
    prevBtn.disabled = start === 0;
    nextBtn.disabled = start + shown >= total;
  }

  function renderBody() {
    tbody.innerHTML = '';
    const vr = visibleRows();
    if (vr.length === 0) {
      const span = cols.length + (expand ? 1 : 0) + (actions ? 1 : 0);
      const filtered = Object.keys(filters).length > 0;
      tbody.appendChild(el('tr', null, el('td', { colspan: span, class: 'empty' },
        filtered ? 'No matching records.' : 'No records.')));
      updatePager(0, 0, 0);
      return;
    }
    // Clamp the page into range (e.g. after filtering shrinks the result set),
    // then put only that page's slice in the DOM.
    const lastPage = Math.ceil(vr.length / PAGE_SIZE) - 1;
    if (page > lastPage) page = lastPage;
    if (page < 0) page = 0;
    const start = page * PAGE_SIZE;
    const pageRows = vr.slice(start, start + PAGE_SIZE);
    updatePager(vr.length, start, pageRows.length);
    pageRows.forEach(function (row) {
      const tds = cols.map(function (c) {
        const v = row[c];
        if (statusField && c === statusField) {
          return el('td', null, el('span', { class: 'badge ' + cellText(v) }, cellText(v)));
        }
        if (isTimestamp(v)) {
          return el('td', { title: utcTooltip(v) }, cellText(v));
        }
        const text = displayText(row, c);
        // A labelled fk cell keeps its raw id reachable on the tooltip; a
        // money cell rounded for display keeps its full value there.
        let title = null;
        if (labels[c] && text !== cellText(v)) {
          title = 'id ' + cellText(v);
        } else {
          const nd = numericDisplay(v, kinds[c]);
          if (nd && nd.tip) title = nd.tip;
        }
        return el('td', { class: numeric[c] ? 'num' : null, title: title }, text);
      });

      let childSpec = null;
      if (expand) {
        childSpec = expand(row);
        const hasChildren = !!(childSpec && childSpec.rows && childSpec.rows.length);
        const toggle = hasChildren
          ? el('button', {
            type: 'button', class: 'expand-toggle',
            onclick: function () {
              if (expandedRows.has(row)) expandedRows.delete(row); else expandedRows.add(row);
              renderBody();
            },
          }, expandedRows.has(row) ? '▾' : '▸')
          : null;
        tds.unshift(el('td', { class: 'expand-col' }, toggle));
      }
      if (actions) tds.push(actions(row) || el('td'));
      tbody.appendChild(el('tr', null, tds));

      if (expand && childSpec && childSpec.rows && childSpec.rows.length && expandedRows.has(row)) {
        const span = cols.length + 1 + (actions ? 1 : 0);
        const childTable = filterableTable(childSpec.rows, childSpec.cols, childSpec.opts || {});
        tbody.appendChild(el('tr', { class: 'detail-row' },
          el('td', { colspan: span, class: 'detail-cell' }, childTable)));
      }
    });
  }

  renderBody();
  return container;
}

// Collects every `columns` list declared across an `expand` config chain
// (parent's own columns are not included — the caller already has those),
// so `dataTable` can fetch every descendant level's FK label maps up front:
// `filterableTable`'s `opts.expand` callback is synchronous, so no report
// table can await a label fetch lazily when a row first expands.
function expandColumns(expandCfg) {
  if (!expandCfg) return [];
  return (expandCfg.columns || []).concat(expandColumns(expandCfg.expand));
}

// Builds the synchronous `row => childSpec` `filterableTable` expects from a
// declarative `expand` config (see config.js REPORTS entries): `key` reads a
// nested array field on the row itself (e.g. a disposal's `parcels`); `from`
// + `matchOn` instead reads a sibling array from the same multi-`tables`
// response object (`context`), filtered to the rows matching this row's
// `matchOn` field (`matchOn: null` = every sibling row belongs to the single
// parent, as the what-if's one hypothetical disposal does). `labels` is the
// FK-label-map set already fetched for every level, shared unchanged down
// the recursion — every column name maps to the same source regardless of
// nesting depth.
function buildExpand(expandCfg, labels, context) {
  return function (row) {
    let childRows;
    if (expandCfg.key) {
      childRows = row[expandCfg.key] || [];
    } else if (expandCfg.from) {
      const all = (context && context[expandCfg.from]) || [];
      childRows = expandCfg.matchOn
        ? all.filter(function (r) { return r[expandCfg.matchOn] === row[expandCfg.matchOn]; })
        : all;
    } else {
      childRows = [];
    }
    if (!childRows.length) return null;
    const childOpts = { statusField: expandCfg.statusField, labels: labels };
    if (expandCfg.expand) childOpts.expand = buildExpand(expandCfg.expand, labels, context);
    return { rows: childRows, cols: expandCfg.columns || Object.keys(childRows[0]), opts: childOpts };
  };
}

// Report tables: read-only, no actions column. Foreign-key id columns render
// the referenced row's name (per FK_COLUMN_SOURCES), same as the entity lists.
// `expandCfg` (a REPORTS `expand` entry) turns each row into a expand-to-a-
// child-table row (see `buildExpand`); `context` is the whole multi-`tables`
// response object, needed only for an expand config's `from` sibling lookup.
async function dataTable(rows, columns, statusField, expandCfg, context) {
  if (!rows || rows.length === 0) return el('div', { class: 'empty' }, 'No records.');
  // A `key` expand reads its children from a nested field on the row itself
  // (e.g. `parcels`) — excluded from the parent's own columns, whether
  // `columns` was explicit or (as every report passes) auto-derived from the
  // first row's keys.
  let cols = columns || Object.keys(rows[0]);
  if (expandCfg && expandCfg.key) cols = cols.filter(function (c) { return c !== expandCfg.key; });
  const labels = await columnLabelMaps(cols.concat(expandColumns(expandCfg)));
  const opts = { statusField: statusField, labels: labels };
  if (expandCfg) opts.expand = buildExpand(expandCfg, labels, context);
  return filterableTable(rows, cols, opts);
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
  // deleteOnly: rows are created by an operation and immutable — no New or
  // Edit, but Delete stays as the undo path (e.g. rights sales).
  if (!entity.readonly && !entity.deleteOnly) {
    toolbar.appendChild(el('a', { href: '#/e/' + entity.slug + '/new' },
      el('button', { class: 'primary' }, '+ New ' + entity.title.replace(/s$/, ''))));
  }

  const cols = entity.columns || (rows[0] ? Object.keys(rows[0]) : entity.keyFields.concat(entity.fields).map(function (f) { return f.name; }));
  let table;
  if (rows.length === 0) {
    table = el('div', { class: 'empty' }, 'No records yet.');
  } else {
    // Foreign-key id columns render the referenced row's name, not the raw
    // id — resolved by column name, so a column with no editable field
    // (e.g. income's reinvestment_trade_id) is named too.
    const labels = await columnLabelMaps(cols);
    const actions = entity.readonly ? null : function (row) {
      const keyPath = entity.keyFields.map(function (kf) { return row[kf.name]; }).join('/');
      const td = el('td', { class: 'actions' });
      // A row action is a link (`href`) or an API DELETE (`del` + `confirm`)
      // — e.g. income's Undo reinvest, which drives DELETE /income/:id/reinvest.
      (entity.rowActions ? entity.rowActions(row) : []).forEach(function (a) {
        if (a.del) {
          td.appendChild(el('button', {
            class: 'link small danger',
            onclick: async function () {
              if (!confirm(a.confirm)) return;
              try {
                await api('DELETE', a.del);
                toast(a.label + ': done.');
                viewEntityList(entity);
              } catch (e) {
                toast(e.message, true);
              }
            },
          }, a.label));
        } else {
          td.appendChild(el('a', { href: a.href }, el('button', { class: 'link small' }, a.label)));
        }
      });
      if (entity.attachOwner) {
        td.appendChild(el('a', { href: '#/attachments/' + entity.attachOwner + '/' + row.id },
          el('button', { class: 'link small' }, 'Attachments')));
      }
      if (!entity.deleteOnly) {
        td.appendChild(el('a', { href: '#/e/' + entity.slug + '/edit/' + keyPath },
          el('button', { class: 'link small' }, 'Edit')));
      }
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
  dec('spot_fx_rate', 'Spot FX rate override', { optional: true, default: '', hint: 'Optional deliberate transaction-date spot rate (foreign units per AUD): when set it wins over the monthly RBA rate everywhere this trade converts to AUD. Use for a one-off purchase/sale of a large foreign asset (QC 18020); leave blank for the monthly default. Non-AUD trades only.' }),
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
      labels: await columnLabelMaps(cols),
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
    sellForesightLinks(),
    toolbar, table,
  ]));
}

// Foresight reports surfaced in the Sell flow: what a contemplated sale
// would do to franking credits, and the wash-sale pattern a loss Sell plus
// nearby Buy creates.
function sellForesightLinks() {
  return el('p', { class: 'hint' }, [
    'Before selling: ',
    el('a', { href: '#/r/franking-what-if' }, 'check the franking credits a sale would put at risk'),
    ' · ',
    el('a', { href: '#/r/wash-sales' }, 'review wash-sale flags'),
    ' · ',
    el('a', { href: '#/r/parcel-optimiser' }, 'compare parcel selections'),
    '.',
  ]);
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
    sellForesightLinks(),
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
      labels: await columnLabelMaps(cols),
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
  const buyParcels = await loadOptions('buyParcels');
  const allocEditor = allocationEditor(buyParcels, null, {
    heading: 'Parcels to move',
    hint: 'Each parcel must be a Buy/DRP of the chosen listing held in the source account, with enough remaining units. Moved units keep their cost base and acquisition date.',
    parcelLabel: 'Parcel to move',
    qtyLabel: 'Units to move',
    addLabel: '+ Add parcel',
  });
  form.appendChild(allocEditor.section);

  // The optional crypto network fee: the source parcels disposed of to cover
  // an on-chain transfer fee. Unlike the move, these units are a CGT event
  // (ATO: a holding reducing to cover a network fee is a disposal) — they
  // show up in the gains reports at the per-unit market value below.
  const feeEditor = allocationEditor(buyParcels, null, {
    heading: 'Network fee (optional)',
    hint: 'Leave empty for no fee. Otherwise: the source parcels disposed of to pay the on-chain network fee. These units are a CGT disposal (not moved) at the per-unit market value below — they surface a capital gain/loss in the realised-gains report.',
    parcelLabel: 'Fee parcel',
    qtyLabel: 'Fee units',
    addLabel: '+ Add fee parcel',
  });
  form.appendChild(feeEditor.section);
  const feePrice = field('fee_market_price', 'Fee market value per unit (AUD)', 'decimal',
    { required: false, hint: "The fee crypto's market value per unit at the transfer date — the disposal's capital proceeds. Required only when a fee parcel is set." });
  form.appendChild(await buildFieldInput(feePrice, null, false));

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
      const feeAllocs = feeEditor.read();
      if (feeAllocs.length) {
        body.fee_allocations = feeAllocs;
        body.fee_market_price = readFieldValue(feePrice, form);
      }
      const result = await api('PUT', '/transfers/' + await nextId('/transfers'), body);
      const n = result && result.transfer_ins ? result.transfer_ins.length : 0;
      const listingName = await listingNamer();
      const acct = (await fkLabelMaps({ a: 'holdingAccounts' })).a || {};
      const acctName = function (id) { return acct[id] || ('account ' + id); };
      const feeNote = result && result.fee_sale
        ? ' Network fee disposed as sell #' + result.fee_sale.id + ' (a CGT event).' : '';
      toast('Transferred ' + n + ' parcel(s) of ' + listingName(body.listing_id)
        + ' from ' + acctName(body.from_account_id) + ' to ' + acctName(body.to_account_id)
        + ' (transfer-out sell #' + (result && result.sell ? result.sell.id : '?') + ').' + feeNote);
      location.hash = '#/transfers';
    } catch (e) {
      toast(e.message, true);
    }
  });
  setMain(el('div', null, [
    el('h2', null, 'New Transfer'),
    el('p', { class: 'view-desc' }, 'Moves units between two holding accounts of the same owner — not a CGT event: nothing appears in the gains reports and each moved parcel keeps its cost base and acquisition date. An optional crypto network fee, paid in the transferred crypto, is the exception: those units are disposed of at market value and do surface a capital gain/loss.'),
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
      toast(action.toast(result, listingName, owner));
      location.hash = action.cancel;
    } catch (e) {
      toast(e.message, true);
    }
  });
  setMain(el('div', null, [
    el('h2', null, action.title(id, owner, listingName)),
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
const ATTACH_OWNER = {
  trade_id: { noun: 'trade', api: '/trades', name: function (o, listing) { return describeTrade(o, listing); } },
  income_id: { noun: 'distribution', api: '/income', name: function (o, listing) { return listing(o.listing_id) + ' on ' + o.date_paid; } },
  amma_statement_id: { noun: 'AMMA statement', api: '/amma_statements', name: function (o, listing) { return listing(o.listing_id) + ' FY' + o.tax_year_end_date; } },
};

async function viewAttachments(ownerField, ownerId) {
  const rows = await api('GET', '/attachments?' + ownerField + '=' + encodeURIComponent(ownerId));
  // Name the owning activity (e.g. "DRP 45 XASX:VDHG on 2024-12-20"), not a
  // bare "trade #5"; the id stays as secondary detail.
  const ownerSpec = ATTACH_OWNER[ownerField];
  let ownerName = (ownerSpec ? ownerSpec.noun : 'activity') + ' #' + ownerId;
  if (ownerSpec) {
    try {
      const owner = await api('GET', ownerSpec.api + '/' + ownerId);
      ownerName = ownerSpec.noun + ' ' + ownerSpec.name(owner, await listingNamer()) + ' (#' + ownerId + ')';
    } catch (e) { /* fall back to the noun + id wording */ }
  }
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
    'Files attached to ' + ownerName + '. Stored in the database.'));
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
      listing: l ? ((l.exchange_mic || 'Crypto') + ':' + l.ticker) : ('listing ' + p.listing_id),
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
  setMain(el('div', null, [header, await dataTable(snap.rows)]));
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

  // Summarise the live-fetched prices' as-of times into one "as at …" line
  // (the freshness of the valuation), plus a count of holdings the live fetch
  // could not value.
  function asAtSummary(rows) {
    const asOf = [];
    let unavailable = 0;
    (rows || []).forEach(function (r) {
      if (r && r.price_as_of) asOf.push(r.price_as_of);
      if (r && r.price_unavailable) unavailable += 1;
    });
    if (asOf.length === 0 && unavailable === 0) return null;
    const line = el('p', { class: 'hint as-at' });
    if (asOf.length > 0) {
      asOf.sort();
      const lo = asOf[0], hi = asOf[asOf.length - 1];
      const text = lo === hi
        ? 'Live prices as at ' + fmtLocalTimestamp(lo)
        : 'Live prices as at ' + fmtLocalTimestamp(lo) + ' – ' + fmtLocalTimestamp(hi);
      line.appendChild(el('span', { title: utcTooltip(hi) }, text));
    }
    if (unavailable > 0) {
      line.appendChild(el('span', { class: 'warn' },
        (asOf.length > 0 ? ' · ' : '') + unavailable + ' holding(s) had no live price'));
    }
    return line;
  }

  async function render(rows) {
    result.innerHTML = '';
    const asAt = asAtSummary(Array.isArray(rows) ? rows : [rows]);
    if (asAt) result.appendChild(asAt);
    // Reports with `tables` return one object whose listed keys each render
    // as a titled table (a non-array value renders as a one-row table).
    if (report.tables) {
      for (const t of report.tables) {
        const v = rows[t.key];
        const arr = Array.isArray(v) ? v : (v == null ? [] : [v]);
        result.appendChild(el('h3', null, t.title));
        // `rows` (the whole response object) is threaded through as the
        // `context` an `expand.from` sibling lookup reads (e.g. the parcel
        // optimiser's `strategies` table expanding from its `allocations`).
        // `t.columns`, when given, overrides the auto-derived column list —
        // e.g. to drop a flattened field with no business in this table
        // (the what-if's `years` rows flatten in `NetCapitalGainYear`'s
        // `disposals`, always empty here since the drilldown belongs to the
        // main report, not the hypothetical dry-run).
        result.appendChild(await dataTable(arr, t.columns || null, report.statusField, t.expand, rows));
      }
      return;
    }
    result.appendChild(await dataTable(rows, null, report.statusField, report.expand));
  }

  if (report.method === 'GET') {
    await render(await api('GET', report.api));
    setMain(el('div', null, [header, result]));
    return;
  }

  // Parameterised POST reports (the parcel optimiser, the pre-sale what-if):
  // the body comes from the configured `params` fields — the same field
  // constructors the entity forms use — and the report runs on submit only
  // (no auto-run: the inputs are required).
  if (report.params) {
    const form = el('form', { class: 'card' });
    for (const f of report.params) form.appendChild(await buildFieldInput(f));
    form.appendChild(el('div', { class: 'form-actions' }, [
      el('button', { type: 'submit', class: 'primary' }, 'Run report'),
    ]));
    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      const body = {};
      report.params.forEach(function (f) {
        const v = readFieldValue(f, form);
        if (v !== null) body[f.name] = v;
      });
      try {
        await render(await api('POST', report.api, body));
      } catch (e) {
        toast(e.message, true);
      }
    });
    setMain(el('div', null, [header, form, result]));
    return;
  }

  // POST reports value each held listing from the live price source by
  // default (live: true); the form below lets the user override specific
  // listings' prices (what-if) and pick an as-of date. An explicit price
  // wins over the live fetch.
  const listings = await api('GET', '/listings');
  const priceForm = el('form', { class: 'card' });
  priceForm.appendChild(el('h3', null, 'Price overrides (AUD, optional)'));
  priceForm.appendChild(el('p', { class: 'hint' },
    'Leave blank to value from the live price source. Enter a price to override that listing.'));
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
  function buildBody() {
    const prices = {};
    priceForm.querySelectorAll('[data-listing]').forEach(function (inp) {
      const v = (inp.value || '').trim();
      if (v !== '') prices[inp.getAttribute('data-listing')] = v;
    });
    const body = { prices: prices, live: true };
    if (report.asOfDate) {
      const d = (priceForm.querySelector('[name="as_of_date"]').value || '').trim();
      if (d !== '') body.as_of_date = d;
    }
    return body;
  }
  priceForm.addEventListener('submit', async function (ev) {
    ev.preventDefault();
    try {
      await render(await api('POST', report.api, buildBody()));
    } catch (e) {
      toast(e.message, true);
    }
  });
  setMain(el('div', null, [header, priceForm, result]));
  // Run live on first load so the valuation is shown without manual entry.
  try {
    await render(await api('POST', report.api, buildBody()));
  } catch (e) {
    toast(e.message, true);
  }
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
